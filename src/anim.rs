use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use image::DynamicImage;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Color;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{Attachment, Decode, FileRef, MediaKind};
use crate::td::Event;

pub const MAX_FPS: u32 = 30;
pub const MIN_FPS: u32 = 8;
pub const MAX_FRAMES: usize = 90;
pub const MAX_SECONDS: f32 = 6.0;
pub const DEFAULT_SECONDS: f32 = 3.0;
pub const MAX_FRAME_PIXELS: u64 = 320 * 240;
pub const MAX_CLIP_BYTES: u64 = 24 * 1024 * 1024;
pub const CACHE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const DECODE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RUNNING: usize = 1;
const MAX_STATES: usize = 512;
const MAX_REPORTED: usize = 64;
const CHUNK: usize = 4096;
const MAGIC: &[u8; 8] = b"GTANIM01";
const ID_MASK: u32 = 0x00FF_FFFF;
const PLACEHOLDER: char = '\u{10EEEE}';
const DIACRITICS: [char; 64] = [
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}', '\u{33D}', '\u{33E}', '\u{33F}',
    '\u{346}', '\u{34A}', '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}', '\u{357}',
    '\u{35B}', '\u{363}', '\u{364}', '\u{365}', '\u{366}', '\u{367}', '\u{368}', '\u{369}',
    '\u{36A}', '\u{36B}', '\u{36C}', '\u{36D}', '\u{36E}', '\u{36F}', '\u{483}', '\u{484}',
    '\u{485}', '\u{486}', '\u{487}', '\u{592}', '\u{593}', '\u{594}', '\u{595}', '\u{597}',
    '\u{598}', '\u{599}', '\u{59C}', '\u{59D}', '\u{59E}', '\u{59F}', '\u{5A0}', '\u{5A1}',
    '\u{5A8}', '\u{5A9}', '\u{5AB}', '\u{5AC}', '\u{5AF}', '\u{5C4}', '\u{610}', '\u{611}',
    '\u{612}', '\u{613}', '\u{614}', '\u{615}', '\u{616}', '\u{617}', '\u{657}', '\u{658}',
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub file: i32,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub path: String,
    pub decode: Decode,
    pub seconds: f32,
    pub box_px: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub gap: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub w: u32,
    pub h: u32,
    pub frames: Vec<Frame>,
}

#[derive(Debug)]
pub enum Outcome {
    Ready(PathBuf),
    Still,
}

#[derive(Debug)]
pub struct Done {
    pub key: Key,
    pub epoch: u64,
    pub result: Result<Outcome, String>,
}

enum Op {
    Cmd(String),
    Send {
        id: u32,
        cells: (u16, u16),
        clip: Clip,
        run: bool,
    },
}

enum State {
    Decoding,
    Ready(PathBuf),
    Still,
    Failed,
}

pub struct Anim {
    dir: Option<PathBuf>,
    tx: UnboundedSender<Event>,
    states: HashMap<Key, State>,
    queue: Vec<(Key, Job)>,
    running: usize,
    live: HashMap<Key, u32>,
    touched: HashSet<Key>,
    pending: Vec<Op>,
    paused: bool,
    want_paused: bool,
    next_id: u32,
    epoch: u64,
    errors: Vec<String>,
    reported: HashSet<String>,
}

impl Anim {
    pub fn new(tx: UnboundedSender<Event>) -> Self {
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
            ^ std::process::id().wrapping_mul(2_654_435_761);
        Self {
            dir: None,
            tx,
            states: HashMap::new(),
            queue: Vec::new(),
            running: 0,
            live: HashMap::new(),
            touched: HashSet::new(),
            pending: Vec::new(),
            paused: false,
            want_paused: false,
            next_id: seed & ID_MASK,
            epoch: 0,
            errors: Vec::new(),
            reported: HashSet::new(),
        }
    }

    pub fn enable(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    pub fn begin(&mut self, paused: bool) {
        self.touched.clear();
        self.want_paused = paused;
    }

    pub fn show(&mut self, key: Key, job: impl FnOnce() -> Job) -> Option<u32> {
        self.dir.as_ref()?;
        if let Some(&id) = self.live.get(&key) {
            self.touched.insert(key);
            return Some(id);
        }
        let path = match self.states.get(&key) {
            None => {
                if !self.queue.iter().any(|(k, _)| *k == key) {
                    self.queue.push((key, job()));
                }
                return None;
            }
            Some(State::Ready(path)) => path.clone(),
            Some(State::Decoding | State::Still | State::Failed) => return None,
        };
        let clip = match read_clip(&path) {
            Ok(clip) if clip.frames.len() > 1 => clip,
            Ok(_) => {
                self.states.insert(key, State::Still);
                return None;
            }
            Err(_) if !path.exists() => {
                self.states.remove(&key);
                return None;
            }
            Err(e) => {
                self.states.insert(key, State::Failed);
                self.report(format!("inline animation: {e}"));
                return None;
            }
        };
        let id = self.alloc();
        self.pending.push(Op::Send {
            id,
            cells: (key.cols, key.rows),
            clip,
            run: !self.want_paused,
        });
        self.live.insert(key, id);
        self.touched.insert(key);
        Some(id)
    }

    pub fn finish(&mut self, w: &mut impl Write) -> std::io::Result<()> {
        let mut gone: Vec<(u32, Key)> = self
            .live
            .iter()
            .filter(|(k, _)| !self.touched.contains(k))
            .map(|(k, id)| (*id, *k))
            .collect();
        gone.sort_unstable_by_key(|(id, _)| *id);
        for (id, key) in gone {
            self.live.remove(&key);
            self.pending.push(Op::Cmd(delete(id)));
        }
        if self.want_paused != self.paused {
            self.paused = self.want_paused;
            let mut ids: Vec<u32> = self.live.values().copied().collect();
            ids.sort_unstable();
            for id in ids {
                let cmd = if self.paused { stop(id) } else { start(id) };
                self.pending.push(Op::Cmd(cmd));
            }
        }
        for op in std::mem::take(&mut self.pending) {
            match op {
                Op::Cmd(cmd) => w.write_all(cmd.as_bytes())?,
                Op::Send {
                    id,
                    cells,
                    clip,
                    run,
                } => transmit_to(w, id, cells, &clip, run)?,
            }
        }
        Ok(())
    }

    pub fn end_frame(&mut self) {
        let queue = std::mem::take(&mut self.queue);
        let Some(dir) = self.dir.clone() else {
            return;
        };
        for (key, job) in queue {
            if self.running >= MAX_RUNNING {
                break;
            }
            if self.states.contains_key(&key) {
                continue;
            }
            self.states.insert(key, State::Decoding);
            self.running += 1;
            let tx = self.tx.clone();
            let epoch = self.epoch;
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || {
                let result = decode(&job, &dir);
                let _ = tx.send(Event::Anim(Done { key, epoch, result }));
            });
        }
        if self.states.len() > MAX_STATES {
            let live = &self.live;
            self.states
                .retain(|k, s| matches!(s, State::Decoding) || live.contains_key(k));
        }
    }

    pub fn on_done(&mut self, done: Done) {
        self.running = self.running.saturating_sub(1);
        if done.epoch != self.epoch {
            return;
        }
        let state = match done.result {
            Ok(Outcome::Ready(path)) => State::Ready(path),
            Ok(Outcome::Still) => State::Still,
            Err(e) => {
                self.report(format!("inline animation: {e}"));
                State::Failed
            }
        };
        self.states.insert(done.key, state);
    }

    pub fn take_error(&mut self) -> Option<String> {
        (!self.errors.is_empty()).then(|| std::mem::take(&mut self.errors).join("; "))
    }

    pub fn reset(&mut self) {
        self.epoch += 1;
        self.states.clear();
        self.queue.clear();
        self.touched.clear();
        self.errors.clear();
        self.reported.clear();
        let mut ids: Vec<u32> = self.live.drain().map(|(_, id)| id).collect();
        ids.sort_unstable();
        self.pending.retain(|op| matches!(op, Op::Cmd(_)));
        for id in ids {
            self.pending.push(Op::Cmd(delete(id)));
        }
    }

    pub fn shutdown(&mut self, w: &mut impl Write) -> std::io::Result<()> {
        self.touched.clear();
        self.want_paused = self.paused;
        self.finish(w)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty() && self.queue.is_empty() && self.live.is_empty()
    }

    #[cfg(test)]
    pub fn live_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.live.values().copied().collect();
        ids.sort_unstable();
        ids
    }

    #[cfg(test)]
    pub fn live_files(&self) -> Vec<i32> {
        let mut files: Vec<i32> = self.live.keys().map(|k| k.file).collect();
        files.sort_unstable();
        files
    }

    #[cfg(test)]
    pub fn mark_ready(&mut self, key: Key, path: PathBuf) {
        self.states.insert(key, State::Ready(path));
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    pub fn reserve_id(&mut self) -> u32 {
        self.alloc()
    }

    fn alloc(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1) & ID_MASK;
            if self.next_id != 0 && !self.live.values().any(|id| *id == self.next_id) {
                return self.next_id;
            }
        }
    }

    fn report(&mut self, msg: String) {
        if self.reported.len() >= MAX_REPORTED {
            self.reported.clear();
        }
        if self.reported.insert(msg.clone()) {
            self.errors.push(msg);
        }
    }
}

pub fn kitty_terminal(term: Option<&str>, window: Option<&str>) -> bool {
    window.is_some_and(|w| !w.trim().is_empty()) || term == Some("xterm-kitty")
}

pub fn source(att: &Attachment) -> Option<(FileRef, Decode, f32)> {
    match att.kind {
        MediaKind::Sticker => {
            let v = att.full.as_ref()?;
            matches!(v.decode, Decode::Tgs | Decode::Video).then(|| (v.file.clone(), v.decode, 0.0))
        }
        MediaKind::Animation => {
            let fits = u64::try_from(att.file.size).is_ok_and(|s| s <= MAX_SOURCE_BYTES);
            let seconds = if att.duration > 0 {
                att.duration as f32 + 1.0
            } else {
                0.0
            };
            fits.then(|| (att.file.clone(), Decode::Video, seconds))
        }
        _ => None,
    }
}

pub fn fit_box(box_px: (u32, u32)) -> (u32, u32) {
    let (w, h) = (box_px.0.max(1), box_px.1.max(1));
    let px = u64::from(w) * u64::from(h);
    if px <= MAX_FRAME_PIXELS {
        return (w, h);
    }
    let s = (MAX_FRAME_PIXELS as f64 / px as f64).sqrt();
    (
        ((f64::from(w) * s).floor() as u32).max(1),
        ((f64::from(h) * s).floor() as u32).max(1),
    )
}

pub fn timing(w: u32, h: u32, seconds: f32) -> (u32, usize) {
    let frame = (u64::from(w) * u64::from(h) * 4).max(1);
    let budget = ((MAX_CLIP_BYTES / frame) as usize).clamp(1, MAX_FRAMES);
    let secs = if seconds.is_finite() && seconds > 0.0 {
        seconds.min(MAX_SECONDS)
    } else {
        DEFAULT_SECONDS
    };
    let fps = ((budget as f32 / secs).floor() as u32).clamp(MIN_FPS, MAX_FPS);
    let frames = ((secs * fps as f32).ceil() as usize).clamp(1, budget);
    (fps, frames)
}

pub fn plan(box_px: (u32, u32), seconds: f32) -> Plan {
    let (w, h) = fit_box(box_px);
    let (fps, frames) = timing(w, h, seconds);
    Plan { w, h, fps, frames }
}

pub fn gap_ms(i: usize, fps: u32) -> u32 {
    let fps = u64::from(fps.max(1));
    let at = |n: u64| (n * 1000 + fps / 2) / fps;
    (at(i as u64 + 1) - at(i as u64)) as u32
}

pub fn lottie_pick(source_frames: usize, duration: f32, w: u32, h: u32) -> Vec<(usize, Duration)> {
    let source_frames = source_frames.max(1);
    let secs = if duration.is_finite() && duration > 0.0 {
        duration
    } else {
        source_frames as f32 / 30.0
    };
    let src_fps = f64::from((source_frames as f32 / secs).max(1.0));
    let (fps, budget) = timing(w, h, secs);
    let fps = f64::from(fps).min(src_fps);
    let count = ((f64::from(secs.min(MAX_SECONDS)) * fps).round() as usize).clamp(1, budget);
    (0..count)
        .map(|i| {
            let t0 = i as f64 / fps;
            let t1 = (i + 1) as f64 / fps;
            let index = ((t0 * src_fps + 1e-6).floor() as usize).min(source_frames - 1);
            let gap = (t1 * 1000.0).round() - (t0 * 1000.0).round();
            (index, Duration::from_millis((gap as u64).max(1)))
        })
        .collect()
}

struct Builder {
    w: u32,
    h: u32,
    frames: Vec<Frame>,
    last: Vec<u8>,
}

impl Builder {
    fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            frames: Vec::new(),
            last: Vec::new(),
        }
    }

    fn push(&mut self, raw: &[u8], gap: u32) -> Result<(), String> {
        let want = self.w as usize * self.h as usize * 4;
        if raw.len() != want {
            return Err(format!("frame has {} bytes, expected {want}", raw.len()));
        }
        if let Some(prev) = self.frames.last_mut()
            && self.last == raw
        {
            prev.gap = prev.gap.saturating_add(gap);
            return Ok(());
        }
        self.frames.push(Frame {
            gap: gap.max(1),
            data: deflate(raw)?,
        });
        self.last.clear();
        self.last.extend_from_slice(raw);
        Ok(())
    }

    fn finish(self) -> Result<Clip, String> {
        if self.frames.is_empty() {
            return Err("no frames decoded".to_owned());
        }
        Ok(Clip {
            w: self.w,
            h: self.h,
            frames: self.frames,
        })
    }
}

fn deflate(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(raw).map_err(|e| format!("deflate: {e}"))?;
    enc.finish().map_err(|e| format!("deflate: {e}"))
}

pub fn tgs_clip(path: &str, box_px: (u32, u32)) -> Result<Clip, String> {
    let json = crate::media::read_tgs(path)?;
    let (w, h) = fit_box(box_px);
    let mut b = Builder::new(w, h);
    crate::media::lottie_frames(
        &json,
        (w, h),
        |source, duration| lottie_pick(source, duration, w, h),
        |img, delay| {
            let canvas = crate::media::fit_canvas(&DynamicImage::ImageRgba8(img), w, h);
            b.push(
                canvas.as_raw(),
                delay.as_millis().min(u128::from(u32::MAX)) as u32,
            )
        },
    )?;
    b.finish()
}

pub fn ffmpeg_args(path: &str, alpha: bool, p: &Plan) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if alpha {
        args.extend(["-c:v".to_owned(), "libvpx-vp9".to_owned()]);
    }
    let (w, h) = (p.w, p.h);
    let filter = format!(
        "fps={},scale={w}:{h}:force_original_aspect_ratio=decrease:flags=bilinear,format=rgba,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=0x00000000",
        p.fps
    );
    args.extend([
        "-i".to_owned(),
        path.to_owned(),
        "-an".to_owned(),
        "-sn".to_owned(),
        "-vf".to_owned(),
        filter,
        "-frames:v".to_owned(),
        p.frames.to_string(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "rgba".to_owned(),
        "-".to_owned(),
    ]);
    args
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

fn ffmpeg_clip(args: &[String], p: &Plan, timeout: Duration) -> Result<Clip, String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError;
    let mut child = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "ffmpeg is required for animated stickers and GIFs; install ffmpeg".to_owned()
            } else {
                format!("ffmpeg: {e}")
            }
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg: no stdout pipe".to_owned())?;
    let stderr = crate::media::drain(
        child
            .stderr
            .take()
            .ok_or_else(|| "ffmpeg: no stderr pipe".to_owned())?,
    );
    let frame_len = p.w as usize * p.h as usize * 4;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
    let reader = std::thread::spawn(move || -> std::io::Result<()> {
        loop {
            let mut buf = vec![0u8; frame_len];
            if !read_full(&mut stdout, &mut buf)? || tx.send(buf).is_err() {
                return Ok(());
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let mut b = Builder::new(p.w, p.h);
    let mut i = 0;
    let mut failed = None;
    let timed_out = loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(frame) => {
                if failed.is_none()
                    && let Err(e) = b.push(&frame, gap_ms(i, p.fps))
                {
                    failed = Some(e);
                }
                i += 1;
            }
            Err(RecvTimeoutError::Disconnected) => break false,
            Err(RecvTimeoutError::Timeout) => break true,
        }
    };
    if timed_out {
        child.kill().map_err(|e| format!("ffmpeg: kill: {e}"))?;
    }
    drop(rx);
    let status = child.wait().map_err(|e| format!("ffmpeg: {e}"))?;
    reader
        .join()
        .map_err(|_| "ffmpeg: frame reader panicked".to_owned())?
        .map_err(|e| format!("ffmpeg: {e}"))?;
    let err = stderr
        .join()
        .map_err(|_| "ffmpeg: stderr reader panicked".to_owned())?
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if timed_out {
        return Err(format!("ffmpeg timed out after {} s", timeout.as_secs()));
    }
    if let Some(e) = failed {
        return Err(e);
    }
    if b.frames.is_empty() {
        let err = String::from_utf8_lossy(&err);
        let last = err.lines().last().unwrap_or("no frame decoded").trim();
        return Err(if status.success() {
            "ffmpeg: no frame decoded".to_owned()
        } else {
            format!("ffmpeg: {last}")
        });
    }
    b.finish()
}

fn is_webm(path: &str) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok_and(|_| magic == [0x1a, 0x45, 0xdf, 0xa3])
}

pub fn video_clip(path: &str, box_px: (u32, u32), seconds: f32) -> Result<Clip, String> {
    let p = plan(box_px, seconds);
    let run = |alpha| ffmpeg_clip(&ffmpeg_args(path, alpha, &p), &p, DECODE_TIMEOUT);
    if is_webm(path) {
        run(true).or_else(|_| run(false))
    } else {
        run(false)
    }
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn encode_clip(clip: &Clip) -> Vec<u8> {
    let size: usize = clip.frames.iter().map(|f| f.data.len() + 8).sum();
    let mut out = Vec::with_capacity(MAGIC.len() + 12 + size);
    out.extend_from_slice(MAGIC);
    put(&mut out, clip.w);
    put(&mut out, clip.h);
    put(&mut out, clip.frames.len() as u32);
    for f in &clip.frames {
        put(&mut out, f.gap);
        put(&mut out, f.data.len() as u32);
        out.extend_from_slice(&f.data);
    }
    out
}

fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "truncated animation cache file".to_owned())
}

fn header(bytes: &[u8]) -> Result<(u32, u32, usize), String> {
    if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return Err("not an animation cache file".to_owned());
    }
    let (w, h, n) = (word(bytes, 8)?, word(bytes, 12)?, word(bytes, 16)? as usize);
    let frame = u64::from(w) * u64::from(h) * 4;
    if w == 0 || h == 0 || frame > MAX_CLIP_BYTES || n == 0 || n > MAX_FRAMES {
        return Err(format!("bad animation cache header {w}x{h}x{n}"));
    }
    Ok((w, h, n))
}

pub fn decode_clip(bytes: &[u8]) -> Result<Clip, String> {
    let (w, h, n) = header(bytes)?;
    let mut at = MAGIC.len() + 12;
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let gap = word(bytes, at)?;
        let len = word(bytes, at + 4)? as usize;
        let data = bytes
            .get(at + 8..at + 8 + len)
            .ok_or_else(|| "truncated animation cache file".to_owned())?;
        frames.push(Frame {
            gap,
            data: data.to_vec(),
        });
        at += 8 + len;
    }
    if at != bytes.len() {
        return Err("trailing data in animation cache file".to_owned());
    }
    Ok(Clip { w, h, frames })
}

pub fn read_clip(path: &Path) -> Result<Clip, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_clip(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_header(path: &Path) -> Result<(u32, u32, usize), String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = [0u8; 20];
    f.read_exact(&mut head)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    header(&head)
}

pub fn write_clip(path: &Path, clip: &Clip) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, encode_clip(clip)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn cache_name(job: &Job, len: u64, stamp: u128) -> String {
    let mut h = DefaultHasher::new();
    (
        MAGIC,
        MAX_FPS,
        MAX_FRAMES,
        MAX_FRAME_PIXELS,
        MAX_CLIP_BYTES,
        MAX_SECONDS.to_bits(),
    )
        .hash(&mut h);
    (
        &job.path,
        job.decode,
        job.seconds.to_bits(),
        job.box_px,
        len,
        stamp,
    )
        .hash(&mut h);
    format!("{:016x}.anim", h.finish())
}

fn touch(path: &Path) -> Result<(), String> {
    std::fs::File::options()
        .append(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn prune(dir: &Path, cap: u64, keep: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", entry.path().display())),
        };
        if !meta.is_file() {
            continue;
        }
        let at = meta
            .modified()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        total += meta.len();
        files.push((at, meta.len(), entry.path()));
    }
    if total <= cap {
        return Ok(());
    }
    files.sort_by_key(|(at, _, _)| *at);
    for (_, len, path) in files {
        if total <= cap / 4 * 3 {
            break;
        }
        if path == keep {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        total = total.saturating_sub(len);
    }
    Ok(())
}

fn ensure_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

pub fn decode(job: &Job, dir: &Path) -> Result<Outcome, String> {
    let meta = std::fs::metadata(&job.path).map_err(|e| format!("{}: {e}", job.path))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "file too large to animate ({})",
            crate::format::file_size(meta.len() as i64)
        ));
    }
    let stamp = meta
        .modified()
        .map_err(|e| format!("{}: {e}", job.path))?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(cache_name(job, meta.len(), stamp));
    if let Ok((_, _, n)) = read_header(&path) {
        touch(&path)?;
        return Ok(outcome(path, n));
    }
    let clip = match job.decode {
        Decode::Tgs => tgs_clip(&job.path, job.box_px)?,
        Decode::Video => video_clip(&job.path, job.box_px, job.seconds)?,
        Decode::Raster => return Err("not an animation".to_owned()),
    };
    ensure_dir(dir)?;
    write_clip(&path, &clip)?;
    prune(dir, CACHE_BYTES, &path)?;
    Ok(outcome(path, clip.frames.len()))
}

fn outcome(path: PathBuf, frames: usize) -> Outcome {
    if frames > 1 {
        Outcome::Ready(path)
    } else {
        Outcome::Still
    }
}

fn chunked(
    w: &mut impl Write,
    b64: &mut String,
    first: &str,
    rest: &str,
    payload: &[u8],
) -> std::io::Result<()> {
    b64.clear();
    base64::engine::general_purpose::STANDARD.encode_string(payload, b64);
    let count = b64.len().div_ceil(CHUNK).max(1);
    for i in 0..count {
        let c = &b64[(i * CHUNK).min(b64.len())..((i + 1) * CHUNK).min(b64.len())];
        let keys = if i == 0 { first } else { rest };
        let more = u8::from(i + 1 < count);
        write!(w, "\x1b_G{keys},m={more};{c}\x1b\\")?;
    }
    Ok(())
}

#[cfg(test)]
pub fn transmit(id: u32, cols: u16, rows: u16, clip: &Clip, run: bool) -> String {
    let mut out = Vec::new();
    transmit_to(&mut out, id, (cols, rows), clip, run).unwrap();
    String::from_utf8(out).unwrap()
}

pub fn transmit_to(
    w: &mut impl Write,
    id: u32,
    cells: (u16, u16),
    clip: &Clip,
    run: bool,
) -> std::io::Result<()> {
    let (cols, rows) = cells;
    let mut b64 = String::new();
    for (i, f) in clip.frames.iter().enumerate() {
        if i == 0 {
            let first = format!(
                "a=T,U=1,i={id},f=32,o=z,t=d,s={},v={},c={cols},r={rows},q=2",
                clip.w, clip.h
            );
            chunked(w, &mut b64, &first, "q=2", &f.data)?;
        } else {
            let first = format!(
                "a=f,i={id},f=32,o=z,s={},v={},X=1,z={},q=2",
                clip.w,
                clip.h,
                f.gap.max(1)
            );
            chunked(w, &mut b64, &first, "a=f,q=2", &f.data)?;
        }
    }
    if let Some(root) = clip.frames.first() {
        write!(w, "\x1b_Ga=a,i={id},r=1,z={},q=2\x1b\\", root.gap.max(1))?;
    }
    if run && clip.frames.len() > 1 {
        w.write_all(start(id).as_bytes())?;
    }
    Ok(())
}

pub fn start(id: u32) -> String {
    format!("\x1b_Ga=a,i={id},s=3,v=1,q=2\x1b\\")
}

pub fn stop(id: u32) -> String {
    format!("\x1b_Ga=a,i={id},s=1,q=2\x1b\\")
}

pub fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

pub fn placeholders(buf: &mut Buffer, area: Rect, id: u32, at: (u16, isize), size: (u16, u16)) {
    let (x, y) = at;
    let (cols, rows) = size;
    let fg = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
    let msb = DIACRITICS[((id >> 24) as usize).min(DIACRITICS.len() - 1)];
    let mut symbol = String::with_capacity(16);
    for r in 0..rows {
        let sy = y + r as isize;
        if sy < 0 {
            continue;
        }
        if sy >= area.height as isize {
            break;
        }
        let Some(&row) = DIACRITICS.get(usize::from(r)) else {
            break;
        };
        for c in 0..cols {
            let sx = x.saturating_add(c);
            if sx >= area.width {
                break;
            }
            let Some(cell) = buf.cell_mut((area.x + sx, area.y + sy as u16)) else {
                continue;
            };
            symbol.clear();
            symbol.push(PLACEHOLDER);
            if c == 0 {
                symbol.push(row);
                symbol.push(DIACRITICS[0]);
                symbol.push(msb);
            }
            cell.set_symbol(&symbol)
                .set_fg(fg)
                .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::MIN));
        }
    }
}

#[cfg(test)]
pub fn test_lottie(fps: u32, frames: u32) -> String {
    format!(
        r#"{{"v":"5.5.2","fr":{fps},"ip":0,"op":{frames},"w":512,"h":512,"nm":"t","ddd":0,"assets":[],"layers":[{{"ddd":0,"ind":1,"ty":4,"nm":"s","sr":1,"ks":{{"o":{{"a":0,"k":100}},"r":{{"a":1,"k":[{{"t":0,"s":[0],"i":{{"x":[0.5],"y":[0.5]}},"o":{{"x":[0.5],"y":[0.5]}}}},{{"t":{frames},"s":[360]}}]}},"p":{{"a":0,"k":[256,256,0]}},"a":{{"a":0,"k":[0,0,0]}},"s":{{"a":0,"k":[100,100,100]}}}},"ao":0,"shapes":[{{"ty":"gr","it":[{{"ty":"rc","d":1,"s":{{"a":0,"k":[200,200]}},"p":{{"a":0,"k":[0,0]}},"r":{{"a":0,"k":20}},"nm":"r"}},{{"ty":"fl","c":{{"a":0,"k":[0.2,0.6,1,1]}},"o":{{"a":0,"k":100}},"r":1,"nm":"f"}},{{"ty":"tr","p":{{"a":0,"k":[0,0]}},"a":{{"a":0,"k":[0,0]}},"s":{{"a":0,"k":[100,100]}},"r":{{"a":0,"k":0}},"o":{{"a":0,"k":100}}}}],"nm":"g"}}],"ip":0,"op":{frames},"st":0,"bm":0}}]}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("gramtty-anim-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn require_tools() -> bool {
        std::env::var_os("GRAMTTY_REQUIRE_FIXTURES").is_some()
    }

    fn ffmpeg_make(dir: &Path, name: &str, lavfi: &str, codec: &[&str]) -> Option<String> {
        let out = dir.join(name);
        let mut args: Vec<String> = ["-v", "error", "-y", "-f", "lavfi", "-i", lavfi]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        args.extend(codec.iter().map(|s| (*s).to_owned()));
        args.push(out.to_string_lossy().into_owned());
        match std::process::Command::new("ffmpeg").args(&args).status() {
            Ok(s) if s.success() => Some(out.to_string_lossy().into_owned()),
            Ok(s) => panic!("ffmpeg {args:?}: {s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !require_tools() => None,
            Err(e) => panic!("ffmpeg: {e}"),
        }
    }

    fn inflate(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(data)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn solid(w: u32, h: u32, v: u8) -> Vec<u8> {
        [v, 0, 0, 255].repeat((w * h) as usize)
    }

    fn clip(w: u32, h: u32, frames: &[(u8, u32)]) -> Clip {
        let mut b = Builder::new(w, h);
        for &(v, gap) in frames {
            b.push(&solid(w, h, v), gap).unwrap();
        }
        b.finish().unwrap()
    }

    #[test]
    fn plans_cap_size_rate_frames_and_bytes() {
        assert_eq!(fit_box((170, 168)), (170, 168));
        let (w, h) = fit_box((600, 315));
        assert!(u64::from(w) * u64::from(h) <= MAX_FRAME_PIXELS, "{w}x{h}");
        assert!((f64::from(w) / f64::from(h) - 600.0 / 315.0).abs() < 0.02);
        assert_eq!(fit_box((0, 0)), (1, 1));
        for (box_px, secs) in [
            ((170, 168), 3.0),
            ((600, 315), 2.0),
            ((600, 315), 6.0),
            ((600, 315), 60.0),
            ((40, 40), 0.5),
            ((4000, 4000), 0.0),
            ((300, 300), f32::NAN),
        ] {
            let p = plan(box_px, secs);
            assert!(p.fps <= MAX_FPS && p.fps >= MIN_FPS, "{p:?}");
            assert!(p.frames >= 1 && p.frames <= MAX_FRAMES, "{p:?}");
            let bytes = u64::from(p.w) * u64::from(p.h) * 4 * p.frames as u64;
            assert!(bytes <= MAX_CLIP_BYTES, "{p:?} {bytes}");
            assert!(u64::from(p.w) * u64::from(p.h) <= MAX_FRAME_PIXELS, "{p:?}");
        }
        let sticker = plan((170, 168), 3.0);
        assert_eq!((sticker.fps, sticker.frames), (30, 90));
        let long = plan((600, 315), 60.0);
        assert!(
            long.frames as f32 / long.fps as f32 <= MAX_SECONDS + 0.01,
            "{long:?}"
        );
        assert!(long.fps < MAX_FPS, "{long:?}");
        assert_eq!(plan((100, 100), 0.0), plan((100, 100), DEFAULT_SECONDS));
        let short = plan((100, 100), 1.0);
        assert_eq!((short.fps, short.frames), (30, 30));
        assert_eq!((0..30).map(|i| gap_ms(i, 30)).sum::<u32>(), 1000);
        assert_eq!((0..7).map(|i| gap_ms(i, 7)).sum::<u32>(), 1000);
        assert!((0..30).all(|i| (33..=34).contains(&gap_ms(i, 30))));
    }

    #[test]
    fn lottie_sampling_halves_sixty_fps_and_keeps_the_timeline() {
        let picks = lottie_pick(180, 3.0, 170, 168);
        assert_eq!(picks.len(), 90);
        assert!(picks.iter().enumerate().all(|(i, (idx, _))| *idx == i * 2));
        let total: u128 = picks.iter().map(|(_, d)| d.as_millis()).sum();
        assert_eq!(total, 3000);
        let slow = lottie_pick(20, 2.0, 170, 168);
        assert_eq!(slow.len(), 20);
        assert!(slow.iter().all(|(_, d)| *d == Duration::from_millis(100)));
        let long = lottie_pick(600, 10.0, 170, 168);
        assert!(long.len() <= MAX_FRAMES);
        assert!(long.iter().all(|(i, _)| *i < 600));
        assert_eq!(lottie_pick(0, 0.0, 10, 10).len(), 1);
    }

    #[test]
    fn tgs_frames_are_rendered_at_the_on_screen_size_within_limits() {
        let dir = scratch("tgs");
        let path = dir.join("a.tgs");
        std::fs::write(&path, test_lottie(60, 180)).unwrap();
        let clip = tgs_clip(path.to_str().unwrap(), (170, 168)).unwrap();
        assert_eq!((clip.w, clip.h), (170, 168));
        assert!(
            clip.frames.len() > 30 && clip.frames.len() <= MAX_FRAMES,
            "{}",
            clip.frames.len()
        );
        let total: u32 = clip.frames.iter().map(|f| f.gap).sum();
        assert!((2990..=3010).contains(&total), "{total}");
        for f in &clip.frames {
            assert_eq!(inflate(&f.data).len(), 170 * 168 * 4);
        }
        let first = inflate(&clip.frames[0].data);
        assert_eq!(first[3], 0, "corners stay transparent");
        assert!(first.chunks(4).any(|p| p[3] == 255));
        assert_ne!(first, inflate(&clip.frames[10].data));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn video_frames_follow_the_plan_and_merge_repeated_frames() {
        let dir = scratch("video");
        let Some(fast) = ffmpeg_make(
            &dir,
            "fast.mp4",
            "testsrc2=size=96x64:rate=60:duration=4",
            &["-pix_fmt", "yuv420p"],
        ) else {
            return;
        };
        let p = plan((96, 64), 4.0);
        let c = video_clip(&fast, (96, 64), 4.0).unwrap();
        assert_eq!((c.w, c.h), (96, 64));
        assert!(
            c.frames.len() <= p.frames && c.frames.len() > p.frames / 2,
            "{} {p:?}",
            c.frames.len()
        );
        assert!(p.fps <= MAX_FPS);
        assert!(c.frames.iter().all(|f| f.gap >= 1000 / MAX_FPS));
        let slow = ffmpeg_make(
            &dir,
            "slow.mp4",
            "testsrc2=size=96x64:rate=5:duration=2",
            &["-pix_fmt", "yuv420p"],
        )
        .unwrap();
        let c = video_clip(&slow, (96, 64), 2.0).unwrap();
        assert!(c.frames.len() <= 11, "{}", c.frames.len());
        let total: u32 = c.frames.iter().map(|f| f.gap).sum();
        assert!((1900..=2100).contains(&total), "{total}");
        assert!(
            c.frames
                .iter()
                .take(c.frames.len() - 1)
                .all(|f| f.gap >= 150),
            "{:?}",
            c.frames.iter().map(|f| f.gap).collect::<Vec<_>>()
        );
        let wide = video_clip(&slow, (200, 64), 2.0).unwrap();
        let px = inflate(&wide.frames[0].data);
        assert_eq!(px.len(), 200 * 64 * 4);
        assert_eq!(px[3], 0, "letterbox is transparent");
        let alpha = ffmpeg_make(
            &dir,
            "alpha.webm",
            "color=c=green@0.0:s=64x64:d=1:r=10,format=yuva420p[bg];color=c=yellow:s=16x16:d=1:r=10,format=yuva420p[b];[bg][b]overlay=x='t*40':y=16",
            &["-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0"],
        )
        .unwrap();
        let c = video_clip(&alpha, (64, 64), 0.0).unwrap();
        assert!(c.frames.len() > 1);
        let px = inflate(&c.frames[0].data);
        assert!(
            px.chunks(4).any(|p| p[3] == 0),
            "keeps the sticker's transparency"
        );
        assert!(px.chunks(4).any(|p| p[3] == 255));
        let bad = dir.join("bad.mp4");
        std::fs::write(&bad, b"not a video").unwrap();
        assert!(video_clip(bad.to_str().unwrap(), (64, 64), 1.0).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decode_caches_on_disk_and_prunes_to_the_cap() {
        let dir = scratch("cache");
        let src = dir.join("s.tgs");
        std::fs::write(&src, test_lottie(30, 30)).unwrap();
        let cache = dir.join("anim");
        let job = Job {
            path: src.to_string_lossy().into_owned(),
            decode: Decode::Tgs,
            seconds: 0.0,
            box_px: (60, 60),
        };
        let Outcome::Ready(path) = decode(&job, &cache).unwrap() else {
            panic!("an animated sticker is ready");
        };
        let clip = read_clip(&path).unwrap();
        assert_eq!((clip.w, clip.h), (60, 60));
        assert!(clip.frames.len() > 1);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let Outcome::Ready(again) = decode(&job, &cache).unwrap() else {
            panic!("cached");
        };
        assert_eq!(again, path);
        assert!(std::fs::metadata(&path).unwrap().modified().unwrap() > before);
        let still = dir.join("still.tgs");
        std::fs::write(&still, test_lottie(30, 1)).unwrap();
        let still_job = Job {
            path: still.to_string_lossy().into_owned(),
            ..job.clone()
        };
        assert!(matches!(
            decode(&still_job, &cache).unwrap(),
            Outcome::Still
        ));
        let raster = Job {
            decode: Decode::Raster,
            ..job.clone()
        };
        assert!(decode(&raster, &dir.join("other")).is_err());
        let full = dir.join("full");
        std::fs::create_dir_all(&full).unwrap();
        for i in 0..8u64 {
            let p = full.join(format!("{i}.anim"));
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let t = SystemTime::now() - Duration::from_secs(100 - i);
            std::fs::File::options()
                .append(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        let keep = full.join("0.anim");
        prune(&full, 400, &keep).unwrap();
        let left: HashSet<String> = std::fs::read_dir(&full)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.contains("0.anim"), "{left:?}");
        assert!(left.contains("7.anim"), "{left:?}");
        assert!(!left.contains("1.anim"), "{left:?}");
        assert!(left.len() * 100 <= 300, "{left:?}");
        prune(&full, 10_000, &keep).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clip_files_roundtrip_and_reject_damage() {
        let c = clip(4, 3, &[(1, 40), (1, 40), (2, 33), (3, 50)]);
        assert_eq!(
            c.frames.len(),
            3,
            "a repeated frame extends the previous gap"
        );
        assert_eq!(c.frames[0].gap, 80);
        let bytes = encode_clip(&c);
        assert_eq!(decode_clip(&bytes).unwrap(), c);
        assert!(decode_clip(&bytes[..bytes.len() - 1]).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode_clip(&extra).is_err());
        let mut magic = bytes.clone();
        magic[0] = b'X';
        assert!(decode_clip(&magic).is_err());
        let mut huge = bytes.clone();
        huge[16..20].copy_from_slice(&(MAX_FRAMES as u32 + 1).to_le_bytes());
        assert!(decode_clip(&huge).is_err());
        assert!(Builder::new(2, 2).push(&[0; 3], 10).is_err());
        assert!(Builder::new(2, 2).finish().is_err());
    }

    fn commands(seq: &str) -> Vec<(String, String)> {
        seq.split("\x1b_G")
            .skip(1)
            .map(|c| {
                let c = c.strip_suffix("\x1b\\").expect("each command ends with ST");
                let (keys, data) = c.split_once(';').unwrap_or((c, ""));
                (keys.to_owned(), data.to_owned())
            })
            .collect()
    }

    #[test]
    fn transmission_sends_every_frame_once_then_starts_the_loop() {
        let mut seed = 0x2545_F491_u32;
        let big: Vec<u8> = (0..64 * 64 * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut b = Builder::new(64, 64);
        b.push(&big, 40).unwrap();
        b.push(&solid(64, 64, 9), 33).unwrap();
        b.push(&solid(64, 64, 10), 50).unwrap();
        let c = b.finish().unwrap();
        let seq = transmit(4242, 7, 3, &c, true);
        let cmds = commands(&seq);
        assert!(
            cmds.iter()
                .all(|(_, d)| d.len() <= CHUNK && d.len() % 4 == 0)
        );
        assert_eq!(
            cmds[0].0,
            "a=T,U=1,i=4242,f=32,o=z,t=d,s=64,v=64,c=7,r=3,q=2,m=1"
        );
        let mut frames: Vec<(String, Vec<u8>)> = Vec::new();
        let mut payload = String::new();
        let mut head = String::new();
        for (keys, data) in &cmds {
            if keys.starts_with("a=a") {
                continue;
            }
            if payload.is_empty() {
                head = keys.clone();
            } else {
                assert!(
                    keys == "q=2,m=1"
                        || keys == "q=2,m=0"
                        || keys == "a=f,q=2,m=1"
                        || keys == "a=f,q=2,m=0",
                    "{keys}"
                );
            }
            payload.push_str(data);
            if keys.ends_with("m=0") {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(std::mem::take(&mut payload))
                    .unwrap();
                frames.push((head.clone(), inflate(&bytes)));
            }
        }
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].1, big);
        assert_eq!(
            frames[1].0,
            "a=f,i=4242,f=32,o=z,s=64,v=64,X=1,z=33,q=2,m=0"
        );
        assert_eq!(frames[1].1, solid(64, 64, 9));
        assert!(frames[2].0.contains("z=50"));
        let tail: Vec<&str> = cmds.iter().rev().take(2).map(|(k, _)| k.as_str()).collect();
        assert_eq!(tail, ["a=a,i=4242,s=3,v=1,q=2", "a=a,i=4242,r=1,z=40,q=2"]);
        assert_eq!(seq.matches("a=T").count(), 1);
        let paused = transmit(4242, 7, 3, &c, false);
        assert!(!paused.contains("s=3"));
        assert!(paused.contains("a=a,i=4242,r=1,z=40"));
        assert_eq!(stop(5), "\x1b_Ga=a,i=5,s=1,q=2\x1b\\");
        assert_eq!(start(5), "\x1b_Ga=a,i=5,s=3,v=1,q=2\x1b\\");
        assert_eq!(delete(5), "\x1b_Ga=d,d=I,i=5,q=2\x1b\\");
    }

    #[test]
    fn placeholders_match_ratatui_image_and_clip_to_the_area() {
        use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};
        let id = 0x00AB_CDEF;
        let area = Rect::new(2, 1, 10, 4);
        let mut ours = Buffer::empty(Rect::new(0, 0, 14, 6));
        placeholders(&mut ours, area, id, (1, -2), (5, 5));
        let kitty = ratatui_image::protocol::kitty::Kitty::new(
            DynamicImage::ImageRgba8(image::RgbaImage::new(50, 100)),
            ratatui::layout::Size::new(5, 5),
            id,
            false,
            false,
        )
        .unwrap();
        let proto = SlicedProtocol::Kitty(kitty);
        let mut scratch = Buffer::empty(Rect::new(0, 0, 14, 6));
        ratatui::widgets::Widget::render(
            SlicedImage::new(&proto, SignedPosition { x: 1, y: -2 }),
            area,
            &mut scratch,
        );
        let mut theirs = Buffer::empty(Rect::new(0, 0, 14, 6));
        ratatui::widgets::Widget::render(
            SlicedImage::new(&proto, SignedPosition { x: 1, y: -2 }),
            area,
            &mut theirs,
        );
        assert!(scratch[(3, 1)].symbol().contains("\x1b_G"));
        assert_eq!(ours, theirs);
        assert_eq!(ours[(3, 1)].symbol(), "\u{10EEEE}\u{30E}\u{305}\u{305}");
        assert_eq!(ours[(4, 1)].symbol(), "\u{10EEEE}");
        assert_eq!(
            ours[(3, 4)].symbol(),
            " ",
            "rows past the image stay untouched"
        );
        assert_eq!(ours[(2, 1)].symbol(), " ");
        let mut edge = Buffer::empty(Rect::new(0, 0, 6, 2));
        placeholders(&mut edge, Rect::new(0, 0, 6, 2), 7, (4, 1), (5, 5));
        assert_eq!(edge[(4, 1)].symbol(), "\u{10EEEE}\u{305}\u{305}\u{305}");
        assert_eq!(edge[(5, 1)].fg, Color::Rgb(0, 0, 7));
        assert_eq!(edge[(4, 0)].symbol(), " ");
    }

    fn flush(a: &mut Anim) -> String {
        let mut out = Vec::new();
        a.finish(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn bye(a: &mut Anim) -> String {
        let mut out = Vec::new();
        a.shutdown(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn manager(dir: &Path) -> Anim {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a = Anim::new(tx);
        a.enable(dir.to_path_buf());
        a
    }

    fn job() -> Job {
        Job {
            path: "/nonexistent".to_owned(),
            decode: Decode::Tgs,
            seconds: 0.0,
            box_px: (10, 10),
        }
    }

    #[test]
    fn manager_transmits_once_pauses_and_deletes_what_left_the_screen() {
        let dir = scratch("manager");
        let file = dir.join("a.anim");
        write_clip(&file, &clip(8, 8, &[(1, 40), (2, 40), (3, 40)])).unwrap();
        let mut a = manager(&dir);
        let k1 = Key {
            file: 1,
            cols: 4,
            rows: 2,
        };
        let k2 = Key { file: 2, ..k1 };
        a.begin(false);
        assert_eq!(
            a.show(k1, job),
            None,
            "unknown clips are queued for decoding"
        );
        assert_eq!(a.queued(), 1);
        assert_eq!(a.show(k1, job), None);
        assert_eq!(a.queued(), 1, "queued once");
        assert!(flush(&mut a).is_empty());
        a.mark_ready(k1, file.clone());
        a.mark_ready(k2, file.clone());
        a.begin(false);
        let id1 = a.show(k1, job).unwrap();
        let id2 = a.show(k2, job).unwrap();
        assert_ne!(id1, id2);
        assert!(id1 <= ID_MASK && id1 != 0);
        let out = flush(&mut a);
        assert_eq!(out.matches("a=T").count(), 2);
        assert_eq!(out.matches("a=f,i=").count(), 4);
        assert_eq!(out.matches(",s=3,").count(), 2);
        a.begin(false);
        assert_eq!(a.show(k1, job), Some(id1));
        assert_eq!(a.show(k1, job), Some(id1));
        assert_eq!(a.show(k2, job), Some(id2));
        assert!(
            flush(&mut a).is_empty(),
            "nothing is sent while kitty animates"
        );
        a.begin(true);
        a.show(k1, job);
        a.show(k2, job);
        assert_eq!(
            flush(&mut a),
            format!("{}{}", stop(id1.min(id2)), stop(id1.max(id2)))
        );
        a.begin(true);
        a.show(k1, job);
        assert_eq!(flush(&mut a), delete(id2));
        assert_eq!(a.live_ids(), vec![id1]);
        a.begin(true);
        a.show(k1, job);
        let back = a.show(k2, job).unwrap();
        let out = flush(&mut a);
        assert!(out.contains(&format!("a=T,U=1,i={back},")));
        assert!(!out.contains("s=3"), "transmitted paused");
        a.begin(false);
        a.show(k1, job);
        a.show(k2, job);
        let out = flush(&mut a);
        assert!(out.contains(&start(id1)) && out.contains(&start(back)));
        a.begin(false);
        assert_eq!(
            flush(&mut a),
            format!("{}{}", delete(id1.min(back)), delete(id1.max(back)))
        );
        assert!(a.live_ids().is_empty());
        a.begin(false);
        a.show(k1, job);
        flush(&mut a);
        a.reset();
        assert!(a.is_empty());
        assert!(flush(&mut a).starts_with("\x1b_Ga=d,d=I,"));
        a.begin(false);
        a.show(k1, job);
        assert!(bye(&mut a).is_empty());
        let mut off = Anim::new(tokio::sync::mpsc::unbounded_channel().0);
        off.begin(false);
        assert_eq!(off.show(k1, job), None);
        assert_eq!(off.queued(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn single_frame_and_unreadable_clips_stay_static_and_report_once() {
        let dir = scratch("still");
        let one = dir.join("one.anim");
        write_clip(&one, &clip(4, 4, &[(1, 40)])).unwrap();
        let mut a = manager(&dir);
        let k = Key {
            file: 1,
            cols: 2,
            rows: 1,
        };
        a.mark_ready(k, one);
        a.begin(false);
        assert_eq!(a.show(k, job), None);
        assert_eq!(a.show(k, job), None);
        let pruned = Key { file: 2, ..k };
        a.mark_ready(pruned, dir.join("pruned.anim"));
        assert_eq!(a.show(pruned, job), None);
        assert_eq!(a.queued(), 0);
        assert_eq!(a.show(pruned, job), None);
        assert_eq!(
            a.queued(),
            1,
            "a clip pruned from the cache is decoded again"
        );
        assert_eq!(a.take_error(), None);
        let bad = Key { file: 5, ..k };
        let damaged = dir.join("damaged.anim");
        std::fs::write(&damaged, b"GTANIM01 broken").unwrap();
        a.mark_ready(bad, damaged);
        assert_eq!(a.show(bad, job), None);
        assert!(flush(&mut a).is_empty());
        let err = a.take_error().unwrap();
        assert!(err.contains("damaged.anim"), "{err}");
        assert_eq!(a.take_error(), None);
        a.on_done(Done {
            key: Key { file: 3, ..k },
            epoch: 0,
            result: Err("boom".to_owned()),
        });
        a.on_done(Done {
            key: Key { file: 4, ..k },
            epoch: 0,
            result: Err("boom".to_owned()),
        });
        assert_eq!(a.take_error().as_deref(), Some("inline animation: boom"));
        a.reset();
        a.on_done(Done {
            key: k,
            epoch: 0,
            result: Ok(Outcome::Ready(dir.join("x"))),
        });
        assert!(a.is_empty(), "results from before a reset are dropped");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_kitty_itself_gets_native_animation() {
        assert!(kitty_terminal(Some("xterm-kitty"), None));
        assert!(kitty_terminal(Some("xterm-256color"), Some("3")));
        assert!(!kitty_terminal(Some("xterm-ghostty"), None));
        assert!(!kitty_terminal(Some("tmux-256color"), Some(" ")));
        assert!(!kitty_terminal(None, None));
    }

    #[test]
    fn sources_cover_animated_stickers_and_gifs_only() {
        use crate::model::Visual;
        let file = |id, size| FileRef {
            id,
            size,
            ..FileRef::default()
        };
        let visual = |decode| Visual {
            file: file(1, 100),
            decode,
            width: 512,
            height: 512,
        };
        let mut tgs = Attachment::new(MediaKind::Sticker, file(1, 100));
        tgs.full = Some(visual(Decode::Tgs));
        assert_eq!(source(&tgs).map(|s| s.1), Some(Decode::Tgs));
        let mut webm = tgs.clone();
        webm.full = Some(visual(Decode::Video));
        assert_eq!(source(&webm).map(|s| s.1), Some(Decode::Video));
        let mut webp = tgs.clone();
        webp.full = Some(visual(Decode::Raster));
        assert!(source(&webp).is_none());
        let mut gif = Attachment::new(MediaKind::Animation, file(5, 300_000));
        gif.duration = 2;
        let (f, d, secs) = source(&gif).unwrap();
        assert_eq!((f.id, d, secs), (5, Decode::Video, 3.0));
        gif.file.size = MAX_SOURCE_BYTES as i64 + 1;
        assert!(source(&gif).is_none());
        for kind in [MediaKind::Video, MediaKind::VideoNote, MediaKind::Photo] {
            let mut a = Attachment::new(kind, file(9, 10));
            a.full = Some(visual(Decode::Video));
            assert!(source(&a).is_none(), "{kind:?}");
        }
    }
}
