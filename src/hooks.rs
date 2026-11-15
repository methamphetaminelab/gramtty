use std::time::Duration;

use serde_json::{Value, json};

use crate::model::{ChatKind, Msg, Sender};

pub const MAX_RUNNING: usize = 4;
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub enum Admit {
    Run,
    Drop { first: bool },
}

#[derive(Debug, Default)]
pub struct Gate {
    running: usize,
    dropping: bool,
}

impl Gate {
    pub fn admit(&mut self) -> Admit {
        if self.running < MAX_RUNNING {
            self.running += 1;
            self.dropping = false;
            return Admit::Run;
        }
        let first = !self.dropping;
        self.dropping = true;
        Admit::Drop { first }
    }

    pub fn finished(&mut self) {
        self.running = self.running.saturating_sub(1);
        if self.running < MAX_RUNNING {
            self.dropping = false;
        }
    }
}

pub fn kind_name(kind: &ChatKind) -> &'static str {
    match kind {
        ChatKind::Private(_) => "private",
        ChatKind::Group => "group",
        ChatKind::Supergroup => "supergroup",
        ChatKind::Channel => "channel",
        ChatKind::Secret(_) => "secret",
    }
}

fn media_name(msg: &Msg) -> Option<String> {
    use crate::model::MediaKind as K;
    match msg.attachments.first().map(|a| a.kind) {
        Some(K::Photo) => Some("photo".to_owned()),
        Some(K::Video) => Some("video".to_owned()),
        Some(K::Animation) => Some("animation".to_owned()),
        Some(K::VideoNote) => Some("video_note".to_owned()),
        Some(K::Voice) => Some("voice".to_owned()),
        Some(K::Audio) => Some("audio".to_owned()),
        Some(K::Sticker) => Some("sticker".to_owned()),
        Some(K::Document) => Some("document".to_owned()),
        None => msg.media.clone(),
    }
}

pub fn payload(
    msg: &Msg,
    chat_title: &str,
    chat_kind: &ChatKind,
    sender_name: &str,
    muted: bool,
    mention: bool,
) -> Value {
    let sender_id = match msg.sender {
        Sender::User(id) | Sender::Chat(id) => id,
    };
    json!({
        "chat_id": msg.chat_id,
        "chat_title": chat_title,
        "chat_kind": kind_name(chat_kind),
        "sender_id": sender_id,
        "sender_name": sender_name,
        "message_id": msg.id,
        "date": msg.date,
        "text": msg.body.text,
        "media": media_name(msg),
        "muted": muted,
        "mention": mention,
    })
}

pub async fn run(command: &[String], line: String) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let Some((program, args)) = command.split_first() else {
        return Err("hook: the command is empty".to_owned());
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("hook {program}: cannot start: {e}"))?;
    let stdin = child.stdin.take();
    let work = async {
        if let Some(mut stdin) = stdin {
            let mut data = line.into_bytes();
            data.push(b'\n');
            let _ = stdin.write_all(&data).await;
            let _ = stdin.shutdown().await;
        }
        child.wait().await
    };
    match tokio::time::timeout(TIMEOUT, work).await {
        Err(_) => Err(format!(
            "hook {program}: killed after {} s",
            TIMEOUT.as_secs()
        )),
        Ok(Err(e)) => Err(format!("hook {program}: {e}")),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("hook {program}: {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gramtty-hook-{}-{name}", std::process::id()))
    }

    #[test]
    fn the_gate_admits_four_and_reports_the_first_drop_once() {
        let mut gate = Gate::default();
        for _ in 0..MAX_RUNNING {
            assert_eq!(gate.admit(), Admit::Run);
        }
        assert_eq!(gate.admit(), Admit::Drop { first: true });
        assert_eq!(gate.admit(), Admit::Drop { first: false });
        gate.finished();
        assert_eq!(gate.admit(), Admit::Run);
        assert_eq!(gate.admit(), Admit::Drop { first: true });
    }

    #[tokio::test]
    async fn a_hook_receives_the_payload_on_stdin_as_one_line() {
        let file = temp("stdin");
        let _ = std::fs::remove_file(&file);
        let command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "cat > \"$1\"".to_owned(),
            "sh".to_owned(),
            file.to_string_lossy().into_owned(),
        ];
        run(&command, "{\"a\":1}".to_owned()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}\n");
        let _ = std::fs::remove_file(&file);
    }

    #[tokio::test]
    async fn a_failing_hook_is_an_error() {
        let fail = vec!["sh".to_owned(), "-c".to_owned(), "exit 3".to_owned()];
        let err = run(&fail, String::new()).await.unwrap_err();
        assert!(err.contains("hook sh") && err.contains('3'), "{err}");
        let missing = vec!["/nonexistent/gramtty-hook".to_owned()];
        let err = run(&missing, String::new()).await.unwrap_err();
        assert!(err.contains("cannot start"), "{err}");
        assert!(run(&[], String::new()).await.is_err());
    }

    #[tokio::test]
    async fn a_hook_that_ignores_stdin_is_not_an_error() {
        let ok = vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()];
        run(&ok, "x".repeat(200_000)).await.unwrap();
    }
}
