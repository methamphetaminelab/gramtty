use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

use crate::model::fuzzy_score;

pub const ALBUM_MAX: usize = 10;
pub const PHOTO_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const CAPTION_MAX: usize = 1024;
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DIR_ENTRIES: usize = 5000;
const UPLOAD_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Photo,
    Video,
    Animation,
    Audio,
    Document,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Photo => "photo",
            Kind::Video => "video",
            Kind::Animation => "GIF",
            Kind::Audio => "audio",
            Kind::Document => "file",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub kind: Kind,
    pub as_file: bool,
}

impl Pending {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let meta =
            std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        let name = path
            .file_name()
            .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
        Ok(Self {
            path: path.to_path_buf(),
            name,
            size: meta.len(),
            kind: classify(path, meta.len()),
            as_file: false,
        })
    }

    pub fn send_kind(&self) -> Kind {
        if self.as_file {
            Kind::Document
        } else {
            self.kind
        }
    }

    pub fn can_toggle(&self) -> bool {
        self.kind != Kind::Document
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn classify(path: &Path, size: u64) -> Kind {
    match extension(path).as_str() {
        "jpg" | "jpeg" | "png" | "webp" if size <= PHOTO_MAX_BYTES => Kind::Photo,
        "gif" => Kind::Animation,
        "mp4" | "m4v" | "mov" | "webm" | "mkv" => Kind::Video,
        "mp3" | "m4a" | "flac" | "ogg" | "oga" | "opus" | "wav" | "aac" => Kind::Audio,
        _ => Kind::Document,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Media,
    Document,
    Audio,
    Single,
}

fn group_of(kind: Kind) -> Group {
    match kind {
        Kind::Photo | Kind::Video => Group::Media,
        Kind::Document => Group::Document,
        Kind::Audio => Group::Audio,
        Kind::Animation => Group::Single,
    }
}

pub fn plan(items: &[Pending]) -> Vec<Vec<usize>> {
    let mut buckets: Vec<(Group, Vec<usize>)> = Vec::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let g = group_of(item.send_kind());
        if g == Group::Single {
            buckets.push((g, vec![i]));
            continue;
        }
        match buckets.iter_mut().find(|(bg, _)| *bg == g) {
            Some((_, v)) => v.push(i),
            None => buckets.push((g, vec![i])),
        }
    }
    for (_, idx) in buckets {
        for chunk in idx.chunks(ALBUM_MAX) {
            out.push(chunk.to_vec());
        }
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub enum ClipKind {
    Files(String),
    Image(String),
    Text(String),
    Empty,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn clip_kinds(types: &str) -> Vec<ClipKind> {
    let types: Vec<&str> = types
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    for files in ["text/uri-list", "x-special/gnome-copied-files"] {
        if types.contains(&files) {
            out.push(ClipKind::Files(files.to_owned()));
        }
    }
    let image = ["image/png", "image/jpeg", "image/webp", "image/gif"]
        .into_iter()
        .find(|m| types.contains(m))
        .map(str::to_owned)
        .or_else(|| {
            types
                .iter()
                .find(|t| t.starts_with("image/"))
                .map(|t| (*t).to_owned())
        });
    if let Some(m) = image {
        out.push(ClipKind::Image(m));
    }
    let text = [
        "text/plain;charset=utf-8",
        "UTF8_STRING",
        "text/plain",
        "STRING",
        "TEXT",
    ]
    .into_iter()
    .find(|m| types.contains(m));
    if let Some(m) = text {
        out.push(ClipKind::Text(m.to_owned()));
    }
    if out.is_empty() {
        out.push(ClipKind::Empty);
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|c| (*c as char).to_digit(16)),
                b.get(i + 2).and_then(|c| (*c as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    text.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            let path = if rest.starts_with('/') {
                rest
            } else {
                &rest[rest.find('/')?..]
            };
            Some(PathBuf::from(std::ffi::OsString::from_vec(percent_decode(
                path,
            ))))
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
pub enum Clip {
    Files(Vec<PathBuf>),
    Image(PathBuf),
    Text(String),
}

#[cfg(target_os = "linux")]
async fn wl_paste(args: &[&str]) -> Result<Vec<u8>, String> {
    let child = tokio::process::Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "Ctrl+V needs wl-paste (install wl-clipboard); terminal paste still works for text"
                    .to_owned()
            } else {
                format!("wl-paste: {e}")
            }
        })?;
    let out = tokio::time::timeout(CLIPBOARD_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "wl-paste did not answer within 5 s".to_owned())?
        .map_err(|e| format!("wl-paste: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if err.contains("No selection") || err.contains("Nothing is copied") {
            return Ok(Vec::new());
        }
        return Err(format!("wl-paste failed: {err}"));
    }
    Ok(out.stdout)
}

pub fn unique_upload_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut n = 0u32;
    loop {
        let name = if n == 0 {
            format!("{stem}-{ms}.{ext}")
        } else {
            format!("{stem}-{ms}-{n}.{ext}")
        };
        let p = dir.join(name);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

pub fn ensure_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

#[cfg(target_os = "linux")]
pub async fn read_clipboard(uploads: PathBuf) -> Result<Clip, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        return Err(
            "Ctrl+V reads the Wayland clipboard, but WAYLAND_DISPLAY is not set; use terminal paste"
                .to_owned(),
        );
    }
    let types = String::from_utf8_lossy(&wl_paste(&["--list-types"]).await?).into_owned();
    for kind in clip_kinds(&types) {
        match kind {
            ClipKind::Files(mime) => {
                let raw = wl_paste(&["--no-newline", "--type", &mime]).await?;
                let paths: Vec<PathBuf> = parse_uri_list(&String::from_utf8_lossy(&raw))
                    .into_iter()
                    .filter(|p| p.is_file())
                    .collect();
                if !paths.is_empty() {
                    return Ok(Clip::Files(paths));
                }
            }
            ClipKind::Image(mime) => {
                let bytes = wl_paste(&["--no-newline", "--type", &mime]).await?;
                if bytes.is_empty() {
                    continue;
                }
                ensure_dir(&uploads)?;
                let path = unique_upload_path(&uploads, "clipboard", image_extension(&mime));
                tokio::fs::write(&path, &bytes)
                    .await
                    .map_err(|e| format!("cannot save clipboard image: {e}"))?;
                return Ok(Clip::Image(path));
            }
            ClipKind::Text(mime) => {
                let raw = wl_paste(&["--no-newline", "--type", &mime]).await?;
                return Ok(Clip::Text(String::from_utf8_lossy(&raw).into_owned()));
            }
            ClipKind::Empty => return Err("clipboard is empty".to_owned()),
        }
    }
    Err("clipboard has nothing to paste".to_owned())
}

#[cfg(target_os = "linux")]
pub async fn copy_text(text: String) -> Result<(), String> {
    wl_copy(text.into_bytes(), None).await
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn to_png(path: &Path) -> Result<Vec<u8>, String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .decode()
        .map_err(|e| format!("cannot decode {}: {e}", path.display()))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("cannot encode PNG: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(target_os = "linux")]
pub async fn copy_image(path: PathBuf) -> Result<(), String> {
    let png = tokio::task::spawn_blocking(move || to_png(&path))
        .await
        .map_err(|e| format!("image conversion failed: {e}"))??;
    wl_copy(png, Some("image/png")).await
}

#[cfg(target_os = "linux")]
async fn wl_copy(data: Vec<u8>, mime: Option<&str>) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        return Err("copying needs a Wayland session (WAYLAND_DISPLAY is not set)".to_owned());
    }
    let mut cmd = tokio::process::Command::new("wl-copy");
    if let Some(mime) = mime {
        cmd.args(["--type", mime]);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "copying needs wl-copy (install wl-clipboard)".to_owned()
            } else {
                format!("wl-copy: {e}")
            }
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "wl-copy: no stdin".to_owned())?;
    stdin
        .write_all(&data)
        .await
        .map_err(|e| format!("wl-copy: {e}"))?;
    drop(stdin);
    let status = tokio::time::timeout(CLIPBOARD_TIMEOUT, child.wait())
        .await
        .map_err(|_| "wl-copy did not finish within 5 s".to_owned())?
        .map_err(|e| format!("wl-copy: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy failed ({status})"))
    }
}

#[cfg(target_os = "macos")]
pub async fn read_clipboard(_uploads: PathBuf) -> Result<Clip, String> {
    let child = tokio::process::Command::new("pbpaste")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("pbpaste: {e}"))?;
    let out = tokio::time::timeout(CLIPBOARD_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "pbpaste did not answer within 5 s".to_owned())?
        .map_err(|e| format!("pbpaste: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pbpaste failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if out.stdout.is_empty() {
        return Err(
            "clipboard has no text; pasting images and files from the clipboard is not supported on macOS yet"
                .to_owned(),
        );
    }
    Ok(Clip::Text(
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

#[cfg(target_os = "macos")]
pub async fn copy_text(text: String) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("pbcopy: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pbcopy: no stdin".to_owned())?;
    stdin
        .write_all(text.as_bytes())
        .await
        .map_err(|e| format!("pbcopy: {e}"))?;
    drop(stdin);
    let status = tokio::time::timeout(CLIPBOARD_TIMEOUT, child.wait())
        .await
        .map_err(|_| "pbcopy did not finish within 5 s".to_owned())?
        .map_err(|e| format!("pbcopy: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy failed ({status})"))
    }
}

#[cfg(target_os = "macos")]
pub async fn copy_image(_path: PathBuf) -> Result<(), String> {
    Err("copying images to the clipboard is not supported on macOS yet".to_owned())
}

pub fn prune_uploads(dir: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > UPLOAD_KEEP);
        if old && path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    pub title: String,
    pub performer: String,
}

#[derive(Deserialize)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    duration: Option<String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
}

pub fn ffprobe_args(path: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "stream=codec_type,width,height,duration:stream_tags=rotate:stream_side_data=rotation:format=duration:format_tags=title,artist".into(),
        "-of".into(),
        "json".into(),
        "--".into(),
        path.to_string_lossy().into_owned(),
    ]
}

fn tag(tags: &std::collections::HashMap<String, String>, key: &str) -> String {
    tags.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim().to_owned())
        .unwrap_or_default()
}

fn seconds(s: &Option<String>) -> Option<f64> {
    s.as_deref()?.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub fn parse_probe(json: &str) -> Result<Probe, String> {
    let p: ProbeJson = serde_json::from_str(json).map_err(|e| format!("ffprobe output: {e}"))?;
    let mut out = Probe::default();
    let video = p
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    if let Some(v) = video {
        out.width = v.width.unwrap_or(0);
        out.height = v.height.unwrap_or(0);
        let rotate = v
            .side_data_list
            .iter()
            .find_map(|d| d.get("rotation").and_then(serde_json::Value::as_i64))
            .or_else(|| tag(&v.tags, "rotate").parse().ok())
            .unwrap_or(0);
        if rotate.rem_euclid(180) == 90 {
            std::mem::swap(&mut out.width, &mut out.height);
        }
    }
    let dur = p
        .format
        .as_ref()
        .and_then(|f| seconds(&f.duration))
        .or_else(|| p.streams.iter().find_map(|s| seconds(&s.duration)))
        .unwrap_or(0.0);
    out.duration = dur.round().clamp(0.0, i32::MAX as f64) as i32;
    if let Some(f) = &p.format {
        out.title = tag(&f.tags, "title");
        out.performer = tag(&f.tags, "artist");
    }
    Ok(out)
}

pub fn thumbnail_args(path: &Path, out: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        path.to_string_lossy().into_owned(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        "scale=320:320:force_original_aspect_ratio=decrease".into(),
        "-q:v".into(),
        "4".into(),
        out.to_string_lossy().into_owned(),
    ]
}

pub enum ToolResult<T> {
    Ok(T),
    Missing,
    Failed(String),
}

async fn run_tool(program: &str, args: &[String]) -> ToolResult<Vec<u8>> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ToolResult::Missing,
        Err(e) => return ToolResult::Failed(format!("{program}: {e}")),
    };
    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Err(_) => ToolResult::Failed(format!("{program} timed out")),
        Ok(Err(e)) => ToolResult::Failed(format!("{program}: {e}")),
        Ok(Ok(o)) if !o.status.success() => ToolResult::Failed(format!(
            "{program}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Ok(Ok(o)) => ToolResult::Ok(o.stdout),
    }
}

pub async fn probe(path: &Path) -> ToolResult<Probe> {
    match run_tool("ffprobe", &ffprobe_args(path)).await {
        ToolResult::Ok(out) => match parse_probe(&String::from_utf8_lossy(&out)) {
            Ok(p) => ToolResult::Ok(p),
            Err(e) => ToolResult::Failed(e),
        },
        ToolResult::Missing => ToolResult::Missing,
        ToolResult::Failed(e) => ToolResult::Failed(e),
    }
}

pub async fn thumbnail(path: &Path, uploads: &Path) -> ToolResult<PathBuf> {
    if let Err(e) = ensure_dir(uploads) {
        return ToolResult::Failed(e);
    }
    let out = unique_upload_path(uploads, "thumb", "jpg");
    match run_tool("ffmpeg", &thumbnail_args(path, &out)).await {
        ToolResult::Ok(_) if out.is_file() => ToolResult::Ok(out),
        ToolResult::Ok(_) => ToolResult::Failed("ffmpeg produced no thumbnail".to_owned()),
        ToolResult::Missing => ToolResult::Missing,
        ToolResult::Failed(e) => ToolResult::Failed(e),
    }
}

pub fn expand(input: &str, cwd: &Path, home: &Path) -> PathBuf {
    if input == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.join(rest);
    }
    let p = Path::new(input);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

pub fn split_query(input: &str, cwd: &Path, home: &Path) -> (PathBuf, String, String) {
    match input.rfind('/') {
        Some(i) => {
            let head = &input[..=i];
            (
                expand(head, cwd, home),
                head.to_owned(),
                input[i + 1..].to_owned(),
            )
        }
        None if input == "~" => (home.to_path_buf(), "~/".to_owned(), String::new()),
        None => (cwd.to_path_buf(), String::new(), input.to_owned()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
}

pub fn list_dir(dir: &Path) -> Result<Vec<DirEntry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in rd.take(MAX_DIR_ENTRIES) {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        out.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            dir: path.is_dir(),
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

pub fn filter_entries(entries: &[DirEntry], filter: &str) -> Vec<usize> {
    let show_hidden = filter.starts_with('.');
    let mut scored: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| show_hidden || !e.name.starts_with('.'))
        .filter_map(|(i, e)| fuzzy_score(filter, &e.name).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(name: &str, kind: Kind) -> Pending {
        Pending {
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_owned(),
            size: 1000,
            kind,
            as_file: false,
        }
    }

    #[test]
    fn classifies_by_extension_and_size() {
        let p = Path::new;
        assert_eq!(classify(p("/a/Pic.JPG"), 2000), Kind::Photo);
        assert_eq!(
            classify(p("/a/pic.png"), PHOTO_MAX_BYTES + 1),
            Kind::Document
        );
        assert_eq!(classify(p("a.webp"), 10), Kind::Photo);
        assert_eq!(classify(p("clip.mp4"), 10), Kind::Video);
        assert_eq!(classify(p("clip.MOV"), 10), Kind::Video);
        assert_eq!(classify(p("fun.gif"), 10), Kind::Animation);
        assert_eq!(classify(p("song.mp3"), 10), Kind::Audio);
        assert_eq!(classify(p("song.flac"), 10), Kind::Audio);
        assert_eq!(classify(p("report.pdf"), 10), Kind::Document);
        assert_eq!(classify(p("Makefile"), 10), Kind::Document);
        let mut x = pending("a.jpg", Kind::Photo);
        assert_eq!(x.send_kind(), Kind::Photo);
        x.as_file = true;
        assert_eq!(x.send_kind(), Kind::Document);
        assert!(!pending("a.pdf", Kind::Document).can_toggle());
    }

    #[test]
    fn plans_albums_by_type_and_splits_over_ten() {
        let mut items: Vec<Pending> = (0..12)
            .map(|i| pending(&format!("{i}.jpg"), Kind::Photo))
            .collect();
        items.insert(3, pending("v.mp4", Kind::Video));
        items.push(pending("a.pdf", Kind::Document));
        items.push(pending("b.gif", Kind::Animation));
        items.push(pending("c.zip", Kind::Document));
        items.push(pending("s.mp3", Kind::Audio));
        let plan = plan(&items);
        assert_eq!(plan[0], (0..10).collect::<Vec<_>>());
        assert_eq!(plan[1], vec![10, 11, 12]);
        assert_eq!(plan[2], vec![13, 15]);
        assert_eq!(plan[3], vec![14]);
        assert_eq!(plan[4], vec![16]);
        assert_eq!(plan.len(), 5);
        let mut as_file = vec![
            pending("a.jpg", Kind::Photo),
            pending("b.pdf", Kind::Document),
        ];
        as_file[0].as_file = true;
        assert_eq!(super::plan(&as_file), vec![vec![0, 1]]);
        let many: Vec<Pending> = (0..25)
            .map(|i| pending(&format!("{i}.mp4"), Kind::Video))
            .collect();
        let sizes: Vec<usize> = super::plan(&many).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![10, 10, 5]);
        assert!(super::plan(&[]).is_empty());
    }

    #[test]
    fn parses_clipboard_types() {
        let files = "x-special/gnome-copied-files\ntext/uri-list\ntext/plain;charset=utf-8\n";
        assert_eq!(
            clip_kinds(files),
            vec![
                ClipKind::Files("text/uri-list".into()),
                ClipKind::Files("x-special/gnome-copied-files".into()),
                ClipKind::Text("text/plain;charset=utf-8".into()),
            ]
        );
        assert_eq!(
            clip_kinds("text/html\nimage/bmp\nimage/png\n"),
            vec![ClipKind::Image("image/png".into())]
        );
        assert_eq!(
            clip_kinds("image/x-foo\nUTF8_STRING\nTEXT"),
            vec![
                ClipKind::Image("image/x-foo".into()),
                ClipKind::Text("UTF8_STRING".into())
            ]
        );
        assert_eq!(clip_kinds("\n"), vec![ClipKind::Empty]);
        assert_eq!(clip_kinds("application/x-thing"), vec![ClipKind::Empty]);
        assert_eq!(image_extension("image/jpeg"), "jpg");
        assert_eq!(image_extension("image/x-foo"), "png");
    }

    #[test]
    fn parses_uri_lists() {
        let list = "# comment\r\nfile:///home/u/My%20Photo.jpg\r\nfile://localhost/tmp/a.txt\nhttps://x.org/y\n\ncopy\nfile:///tmp/%D1%84.png";
        assert_eq!(
            parse_uri_list(list),
            vec![
                PathBuf::from("/home/u/My Photo.jpg"),
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/ф.png"),
            ]
        );
        assert_eq!(
            parse_uri_list("file:///bad%zz"),
            vec![PathBuf::from("/bad%zz")]
        );
        assert_eq!(
            parse_uri_list("file:///end%2"),
            vec![PathBuf::from("/end%2")]
        );
    }

    #[test]
    fn parses_ffprobe_json() {
        let json = r#"{"programs":[],"streams":[{"codec_type":"audio","duration":"12.0"},
            {"codec_type":"video","width":1920,"height":1080,"side_data_list":[{"rotation":-90}]}],
            "format":{"duration":"83.6","tags":{"TITLE":"Song","artist":"Band"}}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!((p.width, p.height, p.duration), (1080, 1920, 84));
        assert_eq!((p.title.as_str(), p.performer.as_str()), ("Song", "Band"));
        let audio =
            parse_probe(r#"{"streams":[{"codec_type":"audio","duration":"3.4"}]}"#).unwrap();
        assert_eq!((audio.width, audio.duration), (0, 3));
        assert!(parse_probe("nope").is_err());
        let args = ffprobe_args(Path::new("-x.mp4"));
        assert_eq!(args[args.len() - 2], "--");
        let t = thumbnail_args(Path::new("/v.mp4"), Path::new("/t.jpg"));
        assert_eq!(t.last().map(String::as_str), Some("/t.jpg"));
    }

    #[test]
    fn file_picker_query_and_filtering() {
        let cwd = Path::new("/work");
        let home = Path::new("/home/u");
        assert_eq!(
            split_query("", cwd, home),
            (PathBuf::from("/work"), String::new(), String::new())
        );
        assert_eq!(
            split_query("~/Pic", cwd, home),
            (PathBuf::from("/home/u/"), "~/".to_owned(), "Pic".to_owned())
        );
        assert_eq!(
            split_query("~", cwd, home),
            (PathBuf::from("/home/u"), "~/".to_owned(), String::new())
        );
        assert_eq!(
            split_query("src/ma", cwd, home),
            (
                PathBuf::from("/work/src/"),
                "src/".to_owned(),
                "ma".to_owned()
            )
        );
        assert_eq!(
            split_query("/etc/", cwd, home),
            (PathBuf::from("/etc/"), "/etc/".to_owned(), String::new())
        );
        let e = |name: &str, dir: bool| DirEntry {
            name: name.to_owned(),
            dir,
        };
        let entries = vec![
            e(".config", true),
            e("Documents", true),
            e("main.rs", false),
            e("media.rs", false),
            e("README", false),
        ];
        assert_eq!(filter_entries(&entries, ""), vec![1, 2, 3, 4]);
        assert_eq!(filter_entries(&entries, "ma"), vec![2, 3]);
        assert_eq!(filter_entries(&entries, "mrs"), vec![2, 3]);
        assert_eq!(filter_entries(&entries, ".c"), vec![0]);
        assert_eq!(filter_entries(&entries, "doc"), vec![1]);
        assert!(filter_entries(&entries, "zzz").is_empty());
    }

    #[test]
    fn real_dir_listing_and_pending() {
        let dir = std::env::temp_dir().join(format!("gramtty-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.png"), [0u8; 10]).unwrap();
        let list = list_dir(&dir).unwrap();
        assert_eq!(
            list,
            vec![
                DirEntry {
                    name: "b.png".into(),
                    dir: false
                },
                DirEntry {
                    name: "sub".into(),
                    dir: true
                }
            ]
        );
        let p = Pending::from_path(&dir.join("b.png")).unwrap();
        assert_eq!(
            (p.kind, p.size, p.name.as_str()),
            (Kind::Photo, 10, "b.png")
        );
        assert!(Pending::from_path(&dir.join("sub")).is_err());
        assert!(Pending::from_path(&dir.join("missing")).is_err());
        let u = unique_upload_path(&dir, "clipboard", "png");
        assert!(u.to_string_lossy().ends_with(".png"));
        prune_uploads(&dir).unwrap();
        assert!(dir.join("b.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
