use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::model::MediaKind;
use crate::td::Event;

pub fn mpv_args(kind: MediaKind, path: &str) -> Vec<String> {
    let mut args = vec!["--no-terminal".to_owned()];
    match kind {
        MediaKind::Voice | MediaKind::Audio => {
            args.push("--no-video".to_owned());
        }
        MediaKind::VideoNote => {
            args.extend(
                [
                    "--force-window=immediate",
                    "--loop-file=inf",
                    "--autofit=480x480",
                ]
                .map(str::to_owned),
            );
        }
        MediaKind::Animation | MediaKind::Sticker => {
            args.extend(
                [
                    "--force-window=immediate",
                    "--loop-file=inf",
                    "--autofit-larger=60%x60%",
                ]
                .map(str::to_owned),
            );
        }
        _ => {
            args.extend(
                ["--force-window=immediate", "--autofit-larger=80%x80%"].map(str::to_owned),
            );
        }
    }
    args.push("--".to_owned());
    args.push(path.to_owned());
    args
}

pub fn spawn_mpv(
    args: Vec<String>,
    token: u64,
    tx: UnboundedSender<Event>,
) -> Result<oneshot::Sender<()>, String> {
    let mut child = tokio::process::Command::new("mpv")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "mpv is required for playback; install mpv and make sure it is on PATH".to_owned()
            } else {
                format!("mpv: {e}")
            }
        })?;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let result = tokio::select! {
            status = child.wait() => match status {
                Ok(s) if matches!(s.code(), Some(1) | Some(2)) => {
                    Err(format!("mpv could not play the file (exit {})", s.code().unwrap_or(-1)))
                }
                Ok(_) => Ok(()),
                Err(e) => Err(format!("mpv: {e}")),
            },
            _ = stop_rx => child.kill().await.map_err(|e| format!("stop mpv: {e}")),
        };
        let _ = tx.send(Event::Player { token, result });
    });
    Ok(stop_tx)
}

const RUNNABLE_EXTENSIONS: [&str; 14] = [
    "desktop", "sh", "bash", "zsh", "fish", "run", "bin", "appimage", "exe", "msi", "bat", "cmd",
    "jar", "command",
];

pub fn runnable(path: &Path) -> Result<Option<&'static str>, String> {
    use std::os::unix::fs::PermissionsExt;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if RUNNABLE_EXTENSIONS.contains(&ext.as_str()) {
        return Ok(Some("this file type can run as a program"));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.permissions().mode() & 0o111 != 0 {
        return Ok(Some("the file is executable"));
    }
    Ok(None)
}

pub fn xdg_open(target: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || child.wait());
        })
        .map_err(|e| format!("xdg-open: {e}"))
}

pub fn parse_user_dirs(content: &str, home: &Path) -> Option<PathBuf> {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("XDG_DOWNLOAD_DIR="))?;
    let value = line["XDG_DOWNLOAD_DIR=".len()..].trim().trim_matches('"');
    if value.is_empty() {
        return None;
    }
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path != home).then_some(path)
}

pub fn downloads_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let from_file = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|c| parse_user_dirs(&c, &home));
    Ok(from_file.unwrap_or_else(|| home.join("Downloads")))
}

pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\0' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_owned();
    if trimmed.is_empty() {
        "file".to_owned()
    } else {
        trimmed
    }
}

pub fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let name = sanitize(name);
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

pub fn save_copy(src: &Path, dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let target = unique_target(dir, name);
    std::fs::copy(src, &target).map_err(|e| format!("copy to {}: {e}", target.display()))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpv_args_per_kind() {
        let v = mpv_args(MediaKind::Voice, "/a.ogg");
        assert!(v.contains(&"--no-video".to_owned()));
        assert!(!v.iter().any(|a| a.starts_with("--force-window")));
        assert_eq!(&v[v.len() - 2..], ["--", "/a.ogg"]);
        let n = mpv_args(MediaKind::VideoNote, "-x.mp4");
        assert!(n.contains(&"--loop-file=inf".to_owned()));
        assert_eq!(n.last().unwrap(), "-x.mp4");
        assert_eq!(n[n.len() - 2], "--");
        let g = mpv_args(MediaKind::Animation, "/g.mp4");
        assert!(g.contains(&"--loop-file=inf".to_owned()));
        let vid = mpv_args(MediaKind::Video, "/v.mp4");
        assert!(!vid.contains(&"--loop-file=inf".to_owned()));
        assert!(vid.contains(&"--force-window=immediate".to_owned()));
    }

    #[test]
    fn user_dirs_parsing() {
        let home = Path::new("/home/u");
        let c =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Загрузки\"\n";
        assert_eq!(
            parse_user_dirs(c, home),
            Some(PathBuf::from("/home/u/Загрузки"))
        );
        assert_eq!(
            parse_user_dirs("XDG_DOWNLOAD_DIR=\"/data/dl\"", home),
            Some(PathBuf::from("/data/dl"))
        );
        assert_eq!(parse_user_dirs("XDG_DOWNLOAD_DIR=\"$HOME/\"", home), None);
        assert_eq!(parse_user_dirs("", home), None);
    }

    #[test]
    fn save_picks_unique_names() {
        let dir = std::env::temp_dir().join(format!("gramtty-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src.bin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&src, b"abc").unwrap();
        let out = dir.join("out");
        let a = save_copy(&src, &out, "report.pdf").unwrap();
        let b = save_copy(&src, &out, "report.pdf").unwrap();
        let c = save_copy(&src, &out, "../evil/.hidden").unwrap();
        assert_eq!(a.file_name().unwrap(), "report.pdf");
        assert_eq!(b.file_name().unwrap(), "report (1).pdf");
        assert_eq!(c.parent().unwrap(), out);
        assert_eq!(std::fs::read(&b).unwrap(), b"abc");
        assert_eq!(sanitize("  "), "file");
        assert_eq!(unique_target(&out, "noext").file_name().unwrap(), "noext");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn runnable_files_are_not_handed_to_xdg_open() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gramtty-runnable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("notes.pdf");
        std::fs::write(&doc, b"x").unwrap();
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(runnable(&doc), Ok(None));
        let launcher = dir.join("Game.DESKTOP");
        std::fs::write(&launcher, b"[Desktop Entry]").unwrap();
        assert!(runnable(&launcher).unwrap().is_some());
        let exe = dir.join("tool");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(runnable(&exe), Ok(Some("the file is executable")));
        assert!(runnable(&dir.join("missing.pdf")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
