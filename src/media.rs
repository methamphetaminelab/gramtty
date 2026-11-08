use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, Limits, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{Decode, FileRef};
use crate::td::{Event, Td};

pub const MAX_ENTRIES: usize = 96;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNNING: usize = 2;
const MAX_SOURCE_BYTES: u64 = 48 * 1024 * 1024;
const MAX_TGS_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ANIMATION_FRAMES: usize = 30;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(50);
const MAX_DIMENSION: u32 = 8192;
const MAX_TRACKED_FILES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    File(i32),
    Mini { chat: i64, msg: i64, idx: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub src: Src,
    pub cols: u16,
    pub rows: u16,
    pub round: bool,
    pub animate: bool,
}

#[derive(Clone, Debug)]
pub enum Input {
    Path(String, Decode),
    Bytes(Arc<[u8]>),
}

pub struct Frames {
    pub frames: Vec<(SlicedProtocol, Duration)>,
    pub bytes: u64,
}

impl Frames {
    pub fn total(&self) -> Duration {
        self.frames.iter().map(|(_, d)| *d).sum()
    }

    pub fn at(&self, elapsed: Duration) -> Option<&SlicedProtocol> {
        let total = self.total();
        if self.frames.len() < 2 || total.is_zero() {
            return self.frames.first().map(|(p, _)| p);
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
        for (p, d) in &self.frames {
            if t < *d {
                return Some(p);
            }
            t -= *d;
        }
        self.frames.last().map(|(p, _)| p)
    }

    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }
}

pub struct Done {
    pub key: Key,
    pub result: Result<Frames, String>,
}

enum Slot {
    Pending,
    Ready(Frames),
    Failed(String),
}

struct Entry {
    slot: Slot,
    used: u64,
    clean: bool,
}

pub enum Look<'a> {
    Ready(&'a Frames),
    Pending,
    Hidden,
    Failed(&'a str),
}

pub struct Media {
    picker: Picker,
    tx: UnboundedSender<Event>,
    cache: HashMap<Key, Entry>,
    queue: Vec<(Key, Input)>,
    running: usize,
    frame: u64,
    files: HashMap<i32, FileRef>,
    requested: HashSet<i32>,
    failed_files: HashMap<i32, String>,
    downloads: Vec<(i32, i32)>,
}

impl Media {
    pub fn new(picker: Picker, tx: UnboundedSender<Event>) -> Self {
        Self {
            picker,
            tx,
            cache: HashMap::new(),
            queue: Vec::new(),
            running: 0,
            frame: 1,
            files: HashMap::new(),
            requested: HashSet::new(),
            failed_files: HashMap::new(),
            downloads: Vec::new(),
        }
    }

    pub fn font(&self) -> (u16, u16) {
        let f = self.picker.font_size();
        (f.width.max(1), f.height.max(1))
    }

    pub fn protocol_name(&self) -> String {
        format!("{:?}", self.picker.protocol_type()).to_lowercase()
    }

    pub fn file(&self, f: &FileRef) -> FileRef {
        self.files.get(&f.id).cloned().unwrap_or_else(|| f.clone())
    }

    pub fn on_file(&mut self, f: FileRef) -> bool {
        let done = f.local().is_some();
        if done {
            self.requested.remove(&f.id);
            self.failed_files.remove(&f.id);
        }
        if self.files.len() >= MAX_TRACKED_FILES {
            let requested = &self.requested;
            self.files
                .retain(|id, known| known.active || requested.contains(id));
        }
        self.files.insert(f.id, f);
        done
    }

    pub fn download_failed(&mut self, file_id: i32, error: String) {
        self.requested.remove(&file_id);
        self.failed_files.insert(file_id, error);
    }

    pub fn file_error(&self, file_id: i32) -> Option<&str> {
        self.failed_files.get(&file_id).map(String::as_str)
    }

    pub fn want_download(&mut self, file_id: i32, priority: i32) {
        if file_id != 0
            && !self.requested.contains(&file_id)
            && !self.failed_files.contains_key(&file_id)
            && !self.downloads.iter().any(|(id, _)| *id == file_id)
        {
            self.downloads.push((file_id, priority));
        }
    }

    pub fn force_download(&mut self, file_id: i32, priority: i32) {
        self.failed_files.remove(&file_id);
        self.requested.remove(&file_id);
        self.want_download(file_id, priority);
    }

    pub fn look(&mut self, key: Key, input: Input, overlay: bool) -> Look<'_> {
        let frame = self.frame;
        if !self.cache.contains_key(&key) {
            if !self.queue.iter().any(|(k, _)| *k == key) {
                self.queue.push((key, input));
            }
            return Look::Pending;
        }
        let Some(entry) = self.cache.get_mut(&key) else {
            return Look::Pending;
        };
        entry.used = frame;
        match &entry.slot {
            Slot::Pending => Look::Pending,
            Slot::Failed(e) => Look::Failed(e),
            Slot::Ready(frames) => {
                if overlay && !entry.clean {
                    Look::Hidden
                } else {
                    if !overlay {
                        entry.clean = true;
                    }
                    Look::Ready(frames)
                }
            }
        }
    }

    pub fn end_frame(&mut self, td: &Td) {
        for (id, priority) in std::mem::take(&mut self.downloads) {
            if self.requested.insert(id) {
                td.download_file(id, priority);
            }
        }
        let queue = std::mem::take(&mut self.queue);
        for (key, input) in queue {
            if self.running >= MAX_RUNNING {
                break;
            }
            if self.cache.contains_key(&key) {
                continue;
            }
            self.cache.insert(
                key,
                Entry {
                    slot: Slot::Pending,
                    used: self.frame,
                    clean: false,
                },
            );
            self.running += 1;
            let picker = self.picker.clone();
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = render(&picker, &input, key);
                let _ = tx.send(Event::Image(Done { key, result }));
            });
        }
        self.evict();
        self.frame += 1;
    }

    pub fn on_done(&mut self, done: Done) {
        self.running = self.running.saturating_sub(1);
        if let Some(entry) = self.cache.get_mut(&done.key) {
            entry.slot = match done.result {
                Ok(frames) => Slot::Ready(frames),
                Err(e) => Slot::Failed(e),
            };
            entry.clean = false;
        }
    }

    #[cfg(test)]
    pub fn has_work(&self) -> bool {
        !self.queue.is_empty() || !self.downloads.is_empty()
    }

    fn bytes(&self) -> u64 {
        self.cache
            .values()
            .map(|e| match &e.slot {
                Slot::Ready(f) => f.bytes,
                _ => 0,
            })
            .sum()
    }

    fn evict(&mut self) {
        let mut bytes = self.bytes();
        while self.cache.len() > MAX_ENTRIES || bytes > MAX_BYTES {
            let victim = self
                .cache
                .iter()
                .filter(|(_, e)| !matches!(e.slot, Slot::Pending) && e.used < self.frame)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            let Some(key) = victim else {
                break;
            };
            if let Some(Entry {
                slot: Slot::Ready(f),
                ..
            }) = self.cache.remove(&key)
            {
                bytes = bytes.saturating_sub(f.bytes);
            }
        }
    }

    pub fn stats(&self) -> (usize, u64) {
        (self.cache.len(), self.bytes())
    }

    #[cfg(test)]
    pub fn insert_ready(&mut self, key: Key, frames: Frames, used: u64) {
        self.cache.insert(
            key,
            Entry {
                slot: Slot::Ready(frames),
                used,
                clean: false,
            },
        );
    }

    #[cfg(test)]
    pub fn contains(&self, key: &Key) -> bool {
        self.cache.contains_key(key)
    }

    #[cfg(test)]
    pub fn picker(&self) -> &Picker {
        &self.picker
    }
}

pub fn fit_cells(
    px_w: i32,
    px_h: i32,
    font: (u16, u16),
    max_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    let max_cols = max_cols.max(1);
    let max_rows = max_rows.max(1);
    if px_w <= 0 || px_h <= 0 {
        return (max_cols.min(20), max_rows.min(6));
    }
    let (fw, fh) = (f64::from(font.0.max(1)), f64::from(font.1.max(1)));
    let (w, h) = (f64::from(px_w), f64::from(px_h));
    let scale = (f64::from(max_cols) * fw / w)
        .min(f64::from(max_rows) * fh / h)
        .min(1.0);
    let cols = ((w * scale / fw).round() as u16).clamp(1, max_cols);
    let rows = ((h * scale / fh).round() as u16).clamp(1, max_rows);
    (cols, rows)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

pub fn album_layout(dims: &[(i32, i32)], font: (u16, u16), max_w: u16, row_h: u16) -> Vec<Cell> {
    let max_w = max_w.max(1);
    let row_h = row_h.max(1);
    let mut out = Vec::with_capacity(dims.len());
    let (mut x, mut y) = (0u16, 0u16);
    for &(pw, ph) in dims {
        let (w, _) = fit_cells(pw, ph, font, max_w, row_h);
        let w = if pw > 0 && ph > 0 {
            let exact = f64::from(row_h) * f64::from(font.1.max(1)) * f64::from(pw)
                / (f64::from(ph) * f64::from(font.0.max(1)));
            (exact.round() as u16).clamp(2.min(max_w), max_w)
        } else {
            w
        };
        if x > 0 && x + w > max_w {
            x = 0;
            y += row_h + 1;
        }
        out.push(Cell { x, y, w, h: row_h });
        x += w + 1;
    }
    out
}

pub fn layout_height(cells: &[Cell]) -> u16 {
    cells.iter().map(|c| c.y + c.h).max().unwrap_or(0)
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_DIMENSION);
    l.max_image_height = Some(MAX_DIMENSION);
    l.max_alloc = Some(256 * 1024 * 1024);
    l
}

fn check_size(path: &str, max: u64) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.len() > max {
        return Err(format!(
            "file too large to preview ({})",
            crate::format::file_size(meta.len() as i64)
        ));
    }
    Ok(())
}

pub fn decode_raster(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("decode: {e}"))?;
    reader.limits(limits());
    reader.decode().map_err(|e| format!("decode: {e}"))
}

fn read_tgs(path: &str) -> Result<Vec<u8>, String> {
    check_size(path, MAX_SOURCE_BYTES)?;
    let raw = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .take(MAX_TGS_JSON_BYTES + 1)
            .read_to_end(&mut out)
            .map_err(|e| format!("tgs: {e}"))?;
        if out.len() as u64 > MAX_TGS_JSON_BYTES {
            return Err("tgs: animation too large".to_owned());
        }
        Ok(out)
    } else {
        Ok(raw)
    }
}

pub fn render_lottie(
    json: &[u8],
    box_px: (u32, u32),
    animate: bool,
) -> Result<Vec<(DynamicImage, Duration)>, String> {
    use thorvg::{ColorSpace, EngineOption, Paint as _, Thorvg};
    let engine = Thorvg::init().map_err(|e| format!("lottie: init: {e}"))?;
    let mut anim = engine
        .lottie_animation()
        .map_err(|e| format!("lottie: {e}"))?;
    anim.load_data(json)
        .map_err(|e| format!("lottie: load: {e}"))?;
    let (sw, sh) = anim
        .picture()
        .size()
        .map_err(|e| format!("lottie: size: {e}"))?;
    if !sw.is_finite() || !sh.is_finite() || sw <= 0.0 || sh <= 0.0 {
        return Err("lottie: invalid size".to_owned());
    }
    let scale = (box_px.0 as f32 / sw).min(box_px.1 as f32 / sh);
    let w = ((sw * scale).round() as u32).clamp(1, MAX_DIMENSION);
    let h = ((sh * scale).round() as u32).clamp(1, MAX_DIMENSION);
    anim.set_size(w as f32, h as f32)
        .map_err(|e| format!("lottie: resize: {e}"))?;
    let total = anim.total_frame().map_err(|e| format!("lottie: {e}"))?;
    let duration = anim.duration().map_err(|e| format!("lottie: {e}"))?;
    let source_frames = if total.is_finite() && total >= 1.0 {
        total.floor() as usize
    } else {
        1
    };
    let count = if animate {
        let by_time = if duration.is_finite() && duration > 0.0 {
            (duration / MIN_FRAME_DELAY.as_secs_f32()).floor().max(1.0) as usize
        } else {
            MAX_ANIMATION_FRAMES
        };
        source_frames.min(MAX_ANIMATION_FRAMES).min(by_time).max(1)
    } else {
        1
    };
    let delay = if count > 1 && duration.is_finite() && duration > 0.0 {
        Duration::from_secs_f32(duration / count as f32).max(MIN_FRAME_DELAY)
    } else {
        Duration::from_millis(100)
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let index = if count > 1 {
            i * (source_frames - 1) / (count - 1)
        } else {
            0
        };
        if index > 0 {
            anim.set_frame(index as f32)
                .map_err(|e| format!("lottie: frame {index}: {e}"))?;
        }
        let picture = anim
            .picture()
            .duplicate()
            .ok_or_else(|| "lottie: duplicate frame".to_owned())?;
        let mut pixels = vec![0u32; (w * h) as usize];
        let mut canvas = engine
            .sw_canvas(EngineOption::Default)
            .map_err(|e| format!("lottie: canvas: {e}"))?;
        unsafe { canvas.set_target(&mut pixels, w, w, h, ColorSpace::ABGR8888S) }
            .map_err(|e| format!("lottie: target: {e}"))?;
        canvas.add(picture).map_err(|e| format!("lottie: {e}"))?;
        canvas
            .draw(true)
            .map_err(|e| format!("lottie: draw: {e}"))?;
        canvas.sync().map_err(|e| format!("lottie: sync: {e}"))?;
        drop(canvas);
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        let img = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| "lottie: buffer".to_owned())?;
        out.push((DynamicImage::ImageRgba8(img), delay));
    }
    Ok(out)
}

pub fn ffmpeg_args(path: &str, alpha_codec: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if alpha_codec {
        args.extend(["-c:v".to_owned(), "libvpx-vp9".to_owned()]);
    }
    args.extend(
        [
            "-i",
            path,
            "-frames:v",
            "1",
            "-an",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );
    args
}

fn run_ffmpeg(path: &str, alpha_codec: bool) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("ffmpeg")
        .args(ffmpeg_args(path, alpha_codec))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg is required for video previews; install ffmpeg".to_owned()
            } else {
                format!("ffmpeg: {e}")
            }
        })?;
    if out.status.success() && !out.stdout.is_empty() {
        return Ok(out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().last().unwrap_or("no frame decoded").trim();
    Err(format!("ffmpeg: {last}"))
}

pub fn video_frame(path: &str) -> Result<DynamicImage, String> {
    check_size(path, u64::MAX)?;
    let mut magic = [0u8; 4];
    let webm = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map(|_| magic == [0x1a, 0x45, 0xdf, 0xa3])
        .unwrap_or(false);
    let png = if webm {
        run_ffmpeg(path, true).or_else(|_| run_ffmpeg(path, false))?
    } else {
        run_ffmpeg(path, false)?
    };
    decode_raster(&png)
}

pub fn load_frames(
    input: &Input,
    box_px: (u32, u32),
    animate: bool,
) -> Result<Vec<(DynamicImage, Duration)>, String> {
    let still = |img: DynamicImage| vec![(img, Duration::ZERO)];
    match input {
        Input::Bytes(b) => decode_raster(b).map(still),
        Input::Path(path, Decode::Raster) => {
            check_size(path, MAX_SOURCE_BYTES)?;
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            decode_raster(&bytes).map(still)
        }
        Input::Path(path, Decode::Tgs) => render_lottie(&read_tgs(path)?, box_px, animate),
        Input::Path(path, Decode::Video) => video_frame(path).map(still),
    }
}

pub fn fit_canvas(img: &DynamicImage, w: u32, h: u32) -> RgbaImage {
    let w = w.max(1);
    let h = h.max(1);
    let resized = if img.width() == w && img.height() == h {
        img.to_rgba8()
    } else {
        img.resize(w, h, FilterType::Triangle).to_rgba8()
    };
    if resized.width() == w && resized.height() == h {
        return resized;
    }
    let mut canvas = RgbaImage::new(w, h);
    let x = (w - resized.width().min(w)) / 2;
    let y = (h - resized.height().min(h)) / 2;
    image::imageops::overlay(&mut canvas, &resized, i64::from(x), i64::from(y));
    canvas
}

pub fn circle_mask(img: &mut RgbaImage) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = w.min(h) / 2.0;
    for (x, y, p) in img.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - cx;
        let dy = y as f32 + 0.5 - cy;
        if dx * dx + dy * dy > r * r {
            p.0[3] = 0;
        }
    }
}

pub fn render(picker: &Picker, input: &Input, key: Key) -> Result<Frames, String> {
    let f = picker.font_size();
    let px = (
        u32::from(key.cols) * u32::from(f.width.max(1)),
        u32::from(key.rows) * u32::from(f.height.max(1)),
    );
    let images = load_frames(input, px, key.animate)?;
    let mut frames = Vec::with_capacity(images.len());
    let mut bytes = 0u64;
    for (img, delay) in images {
        let mut canvas = fit_canvas(&img, px.0, px.1);
        drop(img);
        if key.round {
            circle_mask(&mut canvas);
        }
        bytes += u64::from(px.0) * u64::from(px.1) * 16 / 3;
        let proto = SlicedProtocol::new_with_resize(
            picker,
            DynamicImage::ImageRgba8(canvas),
            Size::new(key.cols, key.rows),
            Resize::Fit(None),
        )
        .map_err(|e| format!("encode: {e}"))?;
        frames.push((proto, delay));
    }
    Ok(Frames { frames, bytes })
}

pub fn input_for(file: &FileRef, decode: Decode) -> Option<Input> {
    file.local().map(|p| Input::Path(p.to_owned(), decode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".work/fixtures")
    }

    fn fixture(name: &str) -> Option<String> {
        let p = fixtures().join(name);
        p.exists().then(|| p.to_string_lossy().into_owned())
    }

    #[test]
    fn fit_cells_keeps_aspect_and_bounds() {
        let font = (10, 20);
        assert_eq!(fit_cells(1280, 720, font, 60, 15), (53, 15));
        assert_eq!(fit_cells(720, 1280, font, 60, 15), (17, 15));
        assert_eq!(fit_cells(100, 40, font, 60, 15), (10, 2));
        assert_eq!(fit_cells(512, 512, font, 24, 8), (16, 8));
        assert_eq!(fit_cells(4000, 10, font, 60, 15), (60, 1));
        assert_eq!(fit_cells(0, 0, font, 60, 15), (20, 6));
        assert_eq!(fit_cells(10, 10, font, 0, 0), (1, 1));
    }

    #[test]
    fn album_layout_wraps_rows() {
        let font = (10, 20);
        let cells = album_layout(
            &[(800, 600), (600, 800), (1000, 1000), (1600, 900)],
            font,
            40,
            6,
        );
        assert_eq!(
            cells[0],
            Cell {
                x: 0,
                y: 0,
                w: 16,
                h: 6
            }
        );
        assert_eq!(
            cells[1],
            Cell {
                x: 17,
                y: 0,
                w: 9,
                h: 6
            }
        );
        assert_eq!(
            cells[2],
            Cell {
                x: 27,
                y: 0,
                w: 12,
                h: 6
            }
        );
        assert_eq!(
            cells[3],
            Cell {
                x: 0,
                y: 7,
                w: 21,
                h: 6
            }
        );
        assert_eq!(layout_height(&cells), 13);
        let wide = album_layout(&[(5000, 100)], font, 30, 6);
        assert_eq!(wide[0].w, 30);
        assert_eq!(layout_height(&[]), 0);
    }

    #[test]
    fn frames_cycle_by_delay() {
        let picker = Picker::halfblocks();
        let img = || DynamicImage::ImageRgba8(RgbaImage::new(10, 20));
        let mk = || {
            SlicedProtocol::new_with_resize(&picker, img(), Size::new(1, 1), Resize::Fit(None))
                .unwrap()
        };
        let f = Frames {
            frames: vec![
                (mk(), Duration::from_millis(100)),
                (mk(), Duration::from_millis(50)),
            ],
            bytes: 0,
        };
        assert!(f.animated());
        assert_eq!(f.total(), Duration::from_millis(150));
        let first = f.at(Duration::ZERO).unwrap() as *const _;
        let second = f.at(Duration::from_millis(120)).unwrap() as *const _;
        assert_ne!(first, second);
        assert_eq!(f.at(Duration::from_millis(150)).unwrap() as *const _, first);
    }

    #[test]
    fn canvas_centers_and_masks() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            100,
            50,
            image::Rgba([255, 0, 0, 255]),
        ));
        let mut c = fit_canvas(&img, 40, 40);
        assert_eq!((c.width(), c.height()), (40, 40));
        assert_eq!(c.get_pixel(20, 0).0[3], 0);
        assert_eq!(c.get_pixel(20, 20).0, [255, 0, 0, 255]);
        let mut sq = fit_canvas(
            &DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))),
            40,
            40,
        );
        circle_mask(&mut sq);
        assert_eq!(sq.get_pixel(0, 0).0[3], 0);
        assert_eq!(sq.get_pixel(20, 20).0[3], 255);
        circle_mask(&mut c);
    }

    #[test]
    fn ffmpeg_args_are_a_list() {
        let a = ffmpeg_args("/tmp/a b.webm", true);
        assert!(a.windows(2).any(|w| w == ["-c:v", "libvpx-vp9"]));
        assert!(a.contains(&"/tmp/a b.webm".to_owned()));
        assert!(!ffmpeg_args("x", false).contains(&"libvpx-vp9".to_owned()));
    }

    fn key(cols: u16, rows: u16, animate: bool) -> Key {
        Key {
            src: Src::File(1),
            cols,
            rows,
            round: false,
            animate,
        }
    }

    #[test]
    fn fixtures_render_through_pipeline() {
        let picker = Picker::halfblocks();
        let cases = [
            ("photo.png", Decode::Raster),
            ("photo.jpg", Decode::Raster),
            ("sticker.webp", Decode::Raster),
            ("sticker.tgs", Decode::Tgs),
            ("sticker.webm", Decode::Video),
            ("video.mp4", Decode::Video),
        ];
        let mut ran = 0;
        for (name, decode) in cases {
            let Some(path) = fixture(name) else {
                continue;
            };
            let input = Input::Path(path, decode);
            let frames = render(&picker, &input, key(12, 6, false))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(frames.frames.len(), 1, "{name}");
            assert_eq!(frames.frames[0].0.size(), Size::new(12, 6), "{name}");
            ran += 1;
        }
        if let Some(path) = fixture("sticker.tgs") {
            let frames = render(&picker, &Input::Path(path, Decode::Tgs), key(8, 4, true)).unwrap();
            assert!(frames.animated());
            assert!(frames.frames.len() <= MAX_ANIMATION_FRAMES);
        }
        if let Some(path) = fixture("sticker.webm") {
            let img = video_frame(&path).unwrap();
            assert!(img.to_rgba8().pixels().any(|p| p.0[3] == 0));
        }
        let bad = render(
            &picker,
            &Input::Path("/nonexistent/x.png".to_owned(), Decode::Raster),
            key(4, 4, false),
        );
        assert!(bad.is_err());
        let mini = render(
            &picker,
            &Input::Bytes(Arc::from(&b"not an image"[..])),
            key(4, 4, false),
        );
        assert!(mini.is_err());
        assert!(ran >= 6 || std::env::var_os("GRAMTTY_REQUIRE_FIXTURES").is_none());
        if std::env::var_os("GRAMTTY_REQUIRE_FIXTURES").is_some() {
            assert_eq!(ran, 6);
        }
    }

    #[test]
    fn cache_evicts_least_recent_offscreen() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = Media::new(Picker::halfblocks(), tx);
        let picker = m.picker().clone();
        let frames = |bytes: u64| Frames {
            frames: vec![(
                SlicedProtocol::new_with_resize(
                    &picker,
                    DynamicImage::ImageRgba8(RgbaImage::new(10, 20)),
                    Size::new(1, 1),
                    Resize::Fit(None),
                )
                .unwrap(),
                Duration::ZERO,
            )],
            bytes,
        };
        for i in 0..(MAX_ENTRIES as u16 + 10) {
            m.insert_ready(key(i + 1, 1, false), frames(1), u64::from(i));
        }
        m.frame = 10_000;
        m.evict();
        assert_eq!(m.stats().0, MAX_ENTRIES);
        assert!(!m.contains(&key(1, 1, false)));
        assert!(m.contains(&key(MAX_ENTRIES as u16 + 10, 1, false)));
        let mut m2 = Media::new(Picker::halfblocks(), m.tx.clone());
        m2.insert_ready(key(1, 1, false), frames(MAX_BYTES), 5);
        m2.insert_ready(key(2, 1, false), frames(MAX_BYTES / 2), 7);
        m2.frame = 7;
        m2.evict();
        assert!(!m2.contains(&key(1, 1, false)));
        assert!(m2.contains(&key(2, 1, false)));
        assert!(matches!(
            m2.look(key(2, 1, false), Input::Bytes(Arc::from(&[][..])), true),
            Look::Hidden
        ));
        assert!(matches!(
            m2.look(key(2, 1, false), Input::Bytes(Arc::from(&[][..])), false),
            Look::Ready(_)
        ));
        assert!(matches!(
            m2.look(key(2, 1, false), Input::Bytes(Arc::from(&[][..])), true),
            Look::Ready(_)
        ));
        assert!(matches!(
            m2.look(key(9, 1, false), Input::Bytes(Arc::from(&[][..])), false),
            Look::Pending
        ));
        assert!(m2.has_work());
    }

    #[test]
    fn file_state_merges_and_download_dedupes() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = Media::new(Picker::halfblocks(), tx);
        let stale = FileRef {
            id: 7,
            size: 100,
            ..FileRef::default()
        };
        m.want_download(7, 1);
        m.want_download(7, 1);
        assert_eq!(m.downloads.len(), 1);
        assert!(!m.on_file(FileRef {
            active: true,
            downloaded: 40,
            ..stale.clone()
        }));
        assert_eq!(m.file(&stale).percent(), Some(40));
        assert!(m.on_file(FileRef {
            done: true,
            path: "/f".to_owned(),
            downloaded: 100,
            ..stale.clone()
        }));
        assert_eq!(m.file(&stale).local(), Some("/f"));
        m.download_failed(9, "nope".to_owned());
        m.want_download(9, 1);
        assert_eq!(m.file_error(9), Some("nope"));
        assert_eq!(m.downloads.len(), 1);
        m.force_download(9, 1);
        assert_eq!(m.downloads.len(), 2);
    }
}
