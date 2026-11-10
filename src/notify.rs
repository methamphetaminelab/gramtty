use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::sync::mpsc::UnboundedSender;

use crate::td::Event;

pub const MAX_AGE_SECS: i64 = 120;

#[derive(Clone, Copy, Debug, Default)]
pub struct Candidate {
    pub enabled: bool,
    pub outgoing: bool,
    pub muted: bool,
    pub mention: bool,
    pub viewing: bool,
    pub age: i64,
}

pub fn worth_inbox(c: &Candidate) -> bool {
    !c.outgoing && c.age < MAX_AGE_SECS && (!c.muted || c.mention)
}

pub fn should_notify(c: &Candidate) -> bool {
    c.enabled && !c.viewing && worth_inbox(c)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    None,
    Path(String),
    Download(i32),
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn notify_args(title: &str, body: &str, icon: Option<&str>) -> Vec<String> {
    let mut args = vec!["--app-name".to_owned(), crate::config::APP_NAME.to_owned()];
    if let Some(icon) = icon {
        args.push("--icon".to_owned());
        args.push(icon.to_owned());
    }
    args.extend(["--".to_owned(), escape_markup(title), escape_markup(body)]);
    args
}

#[cfg(any(target_os = "macos", test))]
pub fn applescript_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(any(target_os = "macos", test))]
pub fn osascript_args(title: &str, body: &str) -> Vec<String> {
    vec![
        "-e".to_owned(),
        format!(
            "display notification {} with title {}",
            applescript_string(body),
            applescript_string(title)
        ),
    ]
}

#[cfg(target_os = "linux")]
const NOTIFIER: &str = "notify-send";
#[cfg(target_os = "linux")]
const NOTIFIER_MISSING: &str =
    "desktop notifications need notify-send (libnotify); disable them in Space o";
#[cfg(target_os = "macos")]
const NOTIFIER: &str = "osascript";
#[cfg(target_os = "macos")]
const NOTIFIER_MISSING: &str = "desktop notifications need osascript; disable them in Space o";

#[cfg(target_os = "linux")]
fn notifier_args(title: &str, body: &str, icon: Option<&str>) -> Vec<String> {
    notify_args(title, body, icon)
}

#[cfg(target_os = "macos")]
fn notifier_args(title: &str, body: &str, _icon: Option<&str>) -> Vec<String> {
    osascript_args(title, body)
}

pub fn send(title: String, body: String, icon: Option<String>, tx: UnboundedSender<Event>) {
    tokio::spawn(async move {
        let args = notifier_args(&title, &body, icon.as_deref());
        let result = tokio::process::Command::new(NOTIFIER)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;
        let err = match result {
            Ok(out) if out.status.success() => return,
            Ok(out) => format!(
                "{NOTIFIER} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => NOTIFIER_MISSING.to_owned(),
            Err(e) => format!("{NOTIFIER}: {e}"),
        };
        let _ = tx.send(Event::NotifyFailed(err));
    });
}

pub const DEFAULT_SOUND: &[u8] = include_bytes!("../assets/msg_incoming.ogg");
pub const DEFAULT_SOUND_NAME: &str = "msg_incoming.ogg";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundEnv {
    pub canberra: bool,
    pub pw_play: bool,
    pub afplay: bool,
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

impl SoundEnv {
    pub fn detect() -> Self {
        Self {
            canberra: on_path("canberra-gtk-play"),
            pw_play: on_path("pw-play"),
            afplay: cfg!(target_os = "macos") && on_path("afplay"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundCommand {
    pub program: String,
    pub args: Vec<String>,
    pub needs_default: bool,
}

fn decibels(volume: u8) -> String {
    format!("{:.1}", 20.0 * (f64::from(volume) / 100.0).log10())
}

fn pw_play(volume: u8, file: &Path, stereo: bool) -> SoundCommand {
    let mut args = vec![
        "--volume".to_owned(),
        format!("{:.2}", f64::from(volume) / 100.0),
    ];
    if stereo {
        args.extend(["--channels", "2", "--channel-map", "FL,FR"].map(str::to_owned));
    }
    args.push(file.to_string_lossy().into_owned());
    SoundCommand {
        program: "pw-play".to_owned(),
        args,
        needs_default: stereo,
    }
}

fn canberra(volume: u8, file: &Path, needs_default: bool) -> SoundCommand {
    SoundCommand {
        program: "canberra-gtk-play".to_owned(),
        args: vec![
            "-f".to_owned(),
            file.to_string_lossy().into_owned(),
            "-V".to_owned(),
            decibels(volume),
        ],
        needs_default,
    }
}

const MACOS_DEFAULT_SOUND: &str = "/System/Library/Sounds/Glass.aiff";

fn afplay(volume: u8, file: &Path) -> SoundCommand {
    SoundCommand {
        program: "afplay".to_owned(),
        args: vec![
            "-v".to_owned(),
            format!("{:.2}", f64::from(volume) / 100.0),
            file.to_string_lossy().into_owned(),
        ],
        needs_default: false,
    }
}

pub fn sound_command(
    n: &crate::config::Notifications,
    env: &SoundEnv,
    default_file: &Path,
) -> Result<Option<SoundCommand>, String> {
    let volume = n.volume.min(100);
    if !n.sound || volume == 0 {
        return Ok(None);
    }
    let custom = n.sound_file.trim();
    let (file, builtin) = if custom.is_empty() {
        (default_file, true)
    } else {
        (Path::new(custom), false)
    };
    if env.pw_play {
        return Ok(Some(pw_play(volume, file, builtin)));
    }
    if env.canberra {
        return Ok(Some(canberra(volume, file, builtin)));
    }
    if env.afplay {
        let file = if builtin {
            Path::new(MACOS_DEFAULT_SOUND)
        } else {
            file
        };
        return Ok(Some(afplay(volume, file)));
    }
    Err("notification sound needs pw-play or canberra-gtk-play".to_owned())
}

fn write_default_sound(path: &Path) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() == DEFAULT_SOUND.len() as u64) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("ogg.tmp");
    std::fs::write(&tmp, DEFAULT_SOUND)?;
    std::fs::rename(&tmp, path)
}

pub fn play(cmd: SoundCommand, default_file: PathBuf, tx: UnboundedSender<Event>) {
    tokio::spawn(async move {
        if cmd.needs_default {
            let path = default_file.clone();
            let written = tokio::task::spawn_blocking(move || write_default_sound(&path)).await;
            let err = match written {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("cannot write {}: {e}", default_file.display())),
                Err(e) => Some(format!("notification sound: {e}")),
            };
            if let Some(e) = err {
                let _ = tx.send(Event::SoundFailed(e));
                return;
            }
        }
        let result = tokio::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;
        let err = match result {
            Ok(out) if out.status.success() => return,
            Ok(out) => format!(
                "{} failed: {}",
                cmd.program,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => format!("{}: {e}", cmd.program),
        };
        let _ = tx.send(Event::SoundFailed(format!(
            "notification sound: {err}; turn it off in Space o"
        )));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifies_only_new_unmuted_or_mentions() {
        let base = Candidate {
            enabled: true,
            age: 3,
            ..Candidate::default()
        };
        assert!(should_notify(&base));
        assert!(!should_notify(&Candidate {
            enabled: false,
            ..base
        }));
        assert!(!should_notify(&Candidate {
            outgoing: true,
            ..base
        }));
        assert!(!should_notify(&Candidate {
            viewing: true,
            ..base
        }));
        assert!(!should_notify(&Candidate { age: 600, ..base }));
        assert!(!should_notify(&Candidate {
            muted: true,
            ..base
        }));
        assert!(should_notify(&Candidate {
            muted: true,
            mention: true,
            ..base
        }));
        assert!(worth_inbox(&Candidate {
            viewing: true,
            enabled: false,
            ..base
        }));
        assert!(!worth_inbox(&Candidate {
            muted: true,
            ..base
        }));
    }

    #[test]
    fn notify_command_includes_icon_only_when_known() {
        assert_eq!(
            notify_args("Chat", "hi", None),
            ["--app-name", "gramtty", "--", "Chat", "hi"]
        );
        assert_eq!(
            notify_args("Chat", "-x", Some("/p/a.jpg")),
            [
                "--app-name",
                "gramtty",
                "--icon",
                "/p/a.jpg",
                "--",
                "Chat",
                "-x"
            ]
        );
    }

    #[test]
    fn notify_command_escapes_markup_in_title_and_body() {
        let args = notify_args(
            "<b>Tom & Jerry</b>",
            "<a href=\"https://evil\">click</a> <img src='file:///home/u/x.png'/> &amp; a < b",
            None,
        );
        assert_eq!(args[3], "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;");
        assert_eq!(
            args[4],
            "&lt;a href=&quot;https://evil&quot;&gt;click&lt;/a&gt; &lt;img src=&#39;file:///home/u/x.png&#39;/&gt; &amp;amp; a &lt; b"
        );
        for arg in &args[3..] {
            assert!(!arg.contains('<') && !arg.contains('>') && !arg.contains('"'));
            assert!(!arg.contains('\''));
        }
        assert_eq!(
            escape_markup("plain text, no markup"),
            "plain text, no markup"
        );
    }

    #[test]
    fn sound_command_picks_tool_and_applies_volume() {
        let mut n = crate::config::Notifications::default();
        let def = Path::new("/c/msg_incoming.ogg");
        let all = SoundEnv {
            canberra: true,
            pw_play: true,
            afplay: true,
        };
        n.volume = 50;
        let c = sound_command(&n, &all, def).unwrap().unwrap();
        assert_eq!(c.program, "pw-play");
        assert_eq!(
            c.args,
            [
                "--volume",
                "0.50",
                "--channels",
                "2",
                "--channel-map",
                "FL,FR",
                "/c/msg_incoming.ogg"
            ]
        );
        assert!(c.needs_default);
        let cb = SoundEnv {
            pw_play: false,
            ..all.clone()
        };
        let c = sound_command(&n, &cb, def).unwrap().unwrap();
        assert_eq!(c.program, "canberra-gtk-play");
        assert_eq!(c.args, ["-f", "/c/msg_incoming.ogg", "-V", "-6.0"]);
        assert!(c.needs_default);
        n.sound_file = "/m/ding.ogg".to_owned();
        let c = sound_command(&n, &all, def).unwrap().unwrap();
        assert_eq!(c.args, ["--volume", "0.50", "/m/ding.ogg"]);
        assert!(!c.needs_default);
        let c = sound_command(&n, &cb, def).unwrap().unwrap();
        assert_eq!(c.args, ["-f", "/m/ding.ogg", "-V", "-6.0"]);
        assert!(!c.needs_default);
        n.sound_file.clear();
        n.volume = 100;
        let c = sound_command(&n, &cb, def).unwrap().unwrap();
        assert_eq!(c.args[3], "0.0");
        n.volume = 0;
        assert_eq!(sound_command(&n, &all, def), Ok(None));
        n.volume = 40;
        n.sound = false;
        assert_eq!(sound_command(&n, &all, def), Ok(None));
        n.sound = true;
        assert!(sound_command(&n, &SoundEnv::default(), def).is_err());
    }

    #[test]
    fn afplay_is_the_last_resort_and_uses_a_system_sound_by_default() {
        let mut n = crate::config::Notifications {
            volume: 50,
            ..Default::default()
        };
        let def = Path::new("/c/msg_incoming.ogg");
        let mac = SoundEnv {
            afplay: true,
            ..SoundEnv::default()
        };
        let c = sound_command(&n, &mac, def).unwrap().unwrap();
        assert_eq!(c.program, "afplay");
        assert_eq!(c.args, ["-v", "0.50", MACOS_DEFAULT_SOUND]);
        assert!(!c.needs_default);
        n.sound_file = "/m/ding.aiff".to_owned();
        let c = sound_command(&n, &mac, def).unwrap().unwrap();
        assert_eq!(c.args, ["-v", "0.50", "/m/ding.aiff"]);
        let both = SoundEnv {
            pw_play: true,
            ..mac
        };
        assert_eq!(
            sound_command(&n, &both, def).unwrap().unwrap().program,
            "pw-play"
        );
    }

    #[test]
    fn osascript_notification_escapes_applescript_strings() {
        assert_eq!(
            osascript_args("Tom \"T\"", "a\\b \" & quit"),
            [
                "-e",
                "display notification \"a\\\\b \\\" & quit\" with title \"Tom \\\"T\\\"\""
            ]
        );
        assert_eq!(applescript_string("plain"), "\"plain\"");
    }

    #[test]
    fn default_sound_is_embedded_stereo_vorbis() {
        assert_eq!(&DEFAULT_SOUND[..4], b"OggS");
        let head = &DEFAULT_SOUND[28..];
        assert_eq!(&head[..7], b"\x01vorbis");
        assert_eq!(head[11], 2);
    }

    #[test]
    fn default_sound_is_written_once_and_refreshed_when_different() {
        let dir = std::env::temp_dir().join(format!("gramtty-sound-{}", std::process::id()));
        let path = dir.join("sub").join(DEFAULT_SOUND_NAME);
        write_default_sound(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), DEFAULT_SOUND);
        std::fs::write(&path, b"old").unwrap();
        write_default_sound(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), DEFAULT_SOUND);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
