#![cfg_attr(not(unix), allow(dead_code))]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MESSAGE_LIMIT: usize = 4096;
pub const MAX_REQUEST: usize = 1 << 20;
pub const REPLY_TIMEOUT_SECS: u64 = 30;
pub const NOT_RUNNING: &str = "gramtty is not running";
#[cfg(not(unix))]
pub const UNSUPPORTED: &str = "not supported on Windows yet";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Send {
        chat: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        code: Option<String>,
    },
    Unread,
    Chats,
    Open {
        chat: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        let data = match data {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        Self {
            ok: true,
            error: None,
            data,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(text.into()),
            data: Map::new(),
        }
    }

    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"cannot encode reply: {e}\"}}"));
        line.push('\n');
        line
    }
}

pub fn parse_request(line: &str) -> Result<Request, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))
}

pub fn parse_response(line: &str) -> Result<Response, String> {
    serde_json::from_str(line.trim()).map_err(|e| format!("bad reply from gramtty: {e}"))
}

pub fn socket_path() -> Result<PathBuf, String> {
    let overridden = std::env::var_os(format!("{}_DATA_DIR", crate::config::env_prefix()))
        .is_some_and(|v| !v.is_empty());
    socket_path_in(
        overridden,
        std::env::var_os("XDG_RUNTIME_DIR"),
        crate::config::data_dir,
    )
}

pub fn socket_path_in(
    data_overridden: bool,
    runtime_dir: Option<std::ffi::OsString>,
    data_dir: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    let name = format!("{}.sock", crate::config::APP_NAME);
    match runtime_dir.filter(|v| !v.is_empty()) {
        Some(dir) if !data_overridden => Ok(PathBuf::from(dir).join(name)),
        _ => Ok(data_dir()?.join(name)),
    }
}

fn utf16_len(c: char) -> usize {
    c.len_utf16()
}

pub fn split_text(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(2);
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for line in text.split_inclusive('\n') {
        let len: usize = line.chars().map(utf16_len).sum();
        if used + len <= limit {
            current.push_str(line);
            used += len;
            continue;
        }
        if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            used = 0;
        }
        if len <= limit {
            current.push_str(line);
            used = len;
            continue;
        }
        for c in line.chars() {
            let n = utf16_len(c);
            if used + n > limit {
                parts.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += n;
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts.retain(|p| !p.trim().is_empty());
    parts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatKey {
    pub id: i64,
    pub title: String,
    pub usernames: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Lookup(String),
    Failed(String),
}

impl ResolveError {
    pub fn message(&self) -> String {
        match self {
            Self::Lookup(name) => format!(
                "{name} is not known to gramtty yet: open that chat once in gramtty or use its numeric id"
            ),
            Self::Failed(text) => text.clone(),
        }
    }
}

fn describe(chats: &[&ChatKey]) -> String {
    let mut list: Vec<String> = chats
        .iter()
        .take(10)
        .map(|c| format!("{} ({})", c.title, c.id))
        .collect();
    if chats.len() > 10 {
        list.push(format!("and {} more", chats.len() - 10));
    }
    list.join(", ")
}

pub fn resolve_chat(
    query: &str,
    chats: &[ChatKey],
    my_id: Option<i64>,
) -> Result<i64, ResolveError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ResolveError::Failed("the chat is empty".to_owned()));
    }
    if query.eq_ignore_ascii_case("me") {
        let id = my_id.ok_or_else(|| {
            ResolveError::Failed("gramtty does not know your account yet".to_owned())
        })?;
        return chats
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.id)
            .ok_or_else(|| {
                ResolveError::Failed(
                    "Saved Messages is not loaded yet, try again in a moment".to_owned(),
                )
            });
    }
    if let Ok(id) = query.parse::<i64>() {
        return chats
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.id)
            .ok_or_else(|| ResolveError::Failed(format!("no chat with id {id}")));
    }
    if let Some(name) = query.strip_prefix('@') {
        return chats
            .iter()
            .find(|c| c.usernames.iter().any(|u| u.eq_ignore_ascii_case(name)))
            .map(|c| c.id)
            .ok_or_else(|| ResolveError::Lookup(query.to_owned()));
    }
    let lower = query.to_lowercase();
    let exact: Vec<&ChatKey> = chats
        .iter()
        .filter(|c| c.title.to_lowercase() == lower)
        .collect();
    let found = if exact.is_empty() {
        chats
            .iter()
            .filter(|c| c.title.to_lowercase().contains(&lower))
            .collect()
    } else {
        exact
    };
    match found.as_slice() {
        [] => Err(ResolveError::Failed(format!("no chat matches {query:?}"))),
        [one] => Ok(one.id),
        many => Err(ResolveError::Failed(format!(
            "{query:?} matches several chats: {}; use the id",
            describe(many)
        ))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    pub request: Request,
    pub json: bool,
}

pub fn is_command(name: &str) -> bool {
    matches!(name, "send" | "unread" | "chats" | "open")
}

const SEND_USAGE: &str =
    "usage: gramtty send <chat> [--file PATH]... [--code [LANG]] [--] [text...]";

fn lang_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '#' | '.' | '-'))
        && !s.starts_with('-')
}

pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err("usage: gramtty <send|unread|chats|open> ...".to_owned());
    };
    match command.as_str() {
        "send" => parse_send(rest),
        "unread" | "chats" => {
            let mut json = false;
            for a in rest {
                match a.as_str() {
                    "--json" => json = true,
                    _ => return Err(format!("usage: gramtty {command} [--json]")),
                }
            }
            let request = if command == "unread" {
                Request::Unread
            } else {
                Request::Chats
            };
            Ok(Cli { request, json })
        }
        "open" => match rest {
            [chat] if !chat.is_empty() => Ok(Cli {
                request: Request::Open { chat: chat.clone() },
                json: false,
            }),
            _ => Err("usage: gramtty open <chat>".to_owned()),
        },
        other => Err(format!("unknown command {other:?}")),
    }
}

fn parse_send(rest: &[String]) -> Result<Cli, String> {
    let mut it = rest.iter().peekable();
    let chat = it.next().ok_or_else(|| SEND_USAGE.to_owned())?.clone();
    let mut files = Vec::new();
    let mut code = None;
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--" => {
                words.extend(it.by_ref().cloned());
            }
            "--file" => {
                let path = it.next().ok_or_else(|| "--file needs a path".to_owned())?;
                files.push(path.clone());
            }
            "--code" => {
                let lang = it.next_if(|n| lang_like(n)).cloned().unwrap_or_default();
                code = Some(lang);
            }
            _ if arg.starts_with("--file=") => files.push(arg["--file=".len()..].to_owned()),
            _ if arg.starts_with("--code=") => code = Some(arg["--code=".len()..].to_owned()),
            _ if arg.starts_with("--") && arg.len() > 2 => {
                return Err(format!("unknown option {arg:?}; {SEND_USAGE}"));
            }
            _ => words.push(arg.clone()),
        }
    }
    if chat.is_empty() || chat.starts_with("--") {
        return Err(SEND_USAGE.to_owned());
    }
    Ok(Cli {
        request: Request::Send {
            chat,
            text: words.join(" "),
            files,
            code,
        },
        json: false,
    })
}

pub fn absolutize(path: &str, cwd: &std::path::Path) -> String {
    cwd.join(path).to_string_lossy().into_owned()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn format_unread(data: &Map<String, Value>) -> String {
    let total = data.get("total");
    let count = |key: &str| {
        total
            .and_then(|t| t.get(key))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    let mut out = format!(
        "{}, {}, {}\n",
        plural(count("chats"), "chat", "chats"),
        plural(count("messages"), "message", "messages"),
        plural(count("mentions"), "mention", "mentions")
    );
    for chat in data
        .get("chats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let n = |key: &str| chat.get(key).and_then(Value::as_i64).unwrap_or(0);
        let title = chat.get("title").and_then(Value::as_str).unwrap_or("");
        let mut line = format!("{title} ({}): {}", n("id"), n("unread"));
        if n("mentions") > 0 {
            line.push_str(&format!(
                ", {}",
                plural(n("mentions"), "mention", "mentions")
            ));
        }
        if chat.get("muted").and_then(Value::as_bool) == Some(true) {
            line.push_str(", muted");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn format_chats(data: &Map<String, Value>) -> String {
    let mut out = String::new();
    for chat in data
        .get("chats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = chat.get("id").and_then(Value::as_i64).unwrap_or(0);
        let title = chat.get("title").and_then(Value::as_str).unwrap_or("");
        let kind = chat.get("kind").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("{id}\t{kind}\t{title}"));
        if let Some(user) = chat.get("username").and_then(Value::as_str) {
            out.push_str(&format!("\t@{user}"));
        }
        out.push('\n');
    }
    out
}

pub fn render(cli: &Cli, response: &Response) -> Result<String, String> {
    if !response.ok {
        return Err(response
            .error
            .clone()
            .unwrap_or_else(|| "gramtty reported an error".to_owned()));
    }
    if cli.json {
        return Ok(format!("{}\n", Value::Object(response.data.clone())));
    }
    Ok(match cli.request {
        Request::Unread => format_unread(&response.data),
        Request::Chats => format_chats(&response.data),
        Request::Send { .. } => {
            let queued = response
                .data
                .get("queued")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            format!("queued {}\n", plural(queued, "message", "messages"))
        }
        Request::Open { .. } => "opened\n".to_owned(),
    })
}

pub fn unread_data(total: (i64, i64, i64), chats: Vec<Value>) -> Value {
    json!({
        "total": {"chats": total.0, "messages": total.1, "mentions": total.2},
        "chats": chats,
    })
}

#[cfg(unix)]
pub use unix::{SocketGuard, bind, call, serve};

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::mpsc::UnboundedSender;

    use super::{MAX_REQUEST, NOT_RUNNING, REPLY_TIMEOUT_SECS, Request, Response};
    use crate::td::Event;

    pub struct SocketGuard(pub PathBuf);

    impl Drop for SocketGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    pub async fn bind(path: &Path) -> Result<UnixListener, String> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => {
                let probe =
                    tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path)).await;
                if matches!(probe, Ok(Ok(_))) {
                    return Err(format!(
                        "another gramtty is already listening on {}; command line control is off in this one",
                        path.display()
                    ));
                }
                std::fs::remove_file(path)
                    .map_err(|e| format!("cannot remove stale {}: {e}", path.display()))?;
            }
            Ok(_) => {
                return Err(format!(
                    "{} exists and is not a socket; command line control is off",
                    path.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect {}: {e}", path.display())),
        }
        let listener = UnixListener::bind(path)
            .map_err(|e| format!("cannot listen on {}: {e}", path.display()))?;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(path);
            return Err(format!("cannot restrict {}: {e}", path.display()));
        }
        Ok(listener)
    }

    pub fn serve(listener: UnixListener, tx: UnboundedSender<Event>) {
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(REPLY_TIMEOUT_SECS + 5),
                        connection(stream, tx),
                    )
                    .await;
                });
            }
        });
    }

    async fn read_request(
        read: &mut tokio::net::unix::OwnedReadHalf,
    ) -> Result<Option<String>, String> {
        let mut reader = BufReader::new(read.take(MAX_REQUEST as u64 + 1));
        let mut buf = Vec::new();
        let n = reader
            .read_until(b'\n', &mut buf)
            .await
            .map_err(|e| format!("cannot read the request: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        if buf.len() > MAX_REQUEST {
            return Err("request is too large (limit 1 MiB)".to_owned());
        }
        String::from_utf8(buf)
            .map(Some)
            .map_err(|_| "request is not valid UTF-8".to_owned())
    }

    async fn connection(stream: UnixStream, tx: UnboundedSender<Event>) {
        let (mut read, mut write) = stream.into_split();
        let line = match read_request(&mut read).await {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) => {
                let _ = write.write_all(Response::error(e).line().as_bytes()).await;
                let _ = write.shutdown().await;
                let mut sink = tokio::io::sink();
                let drain = tokio::io::copy(&mut read, &mut sink);
                let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
                return;
            }
        };
        let response = match super::parse_request(&line) {
            Err(e) => Response::error(e),
            Ok(request) => {
                let (reply, answer) = tokio::sync::oneshot::channel();
                if tx.send(Event::Ipc { request, reply }).is_err() {
                    Response::error("gramtty is shutting down")
                } else {
                    match tokio::time::timeout(Duration::from_secs(REPLY_TIMEOUT_SECS), answer)
                        .await
                    {
                        Ok(Ok(response)) => response,
                        Ok(Err(_)) => Response::error("gramtty dropped the request"),
                        Err(_) => Response::error("gramtty did not answer within 30 s"),
                    }
                }
            }
        };
        let _ = write.write_all(response.line().as_bytes()).await;
        let _ = write.shutdown().await;
    }

    pub fn call(path: &Path, request: &Request) -> Result<Response, String> {
        use std::io::{BufRead, Write};
        let mut stream =
            std::os::unix::net::UnixStream::connect(path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    NOT_RUNNING.to_owned()
                }
                _ => format!("cannot connect to {}: {e}", path.display()),
            })?;
        let timeout = Some(Duration::from_secs(REPLY_TIMEOUT_SECS + 10));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        line.push('\n');
        if line.len() > MAX_REQUEST {
            return Err("the input is too large (limit 1 MiB)".to_owned());
        }
        stream
            .write_all(line.as_bytes())
            .map_err(|e| format!("cannot send the request: {e}"))?;
        let mut reply = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut reply)
            .map_err(|e| format!("cannot read the reply: {e}"))?;
        if reply.is_empty() {
            return Err("gramtty closed the connection without an answer".to_owned());
        }
        super::parse_response(&reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| (*a).to_owned()).collect()
    }

    fn key(id: i64, title: &str, users: &[&str]) -> ChatKey {
        ChatKey {
            id,
            title: title.to_owned(),
            usernames: users.iter().map(|u| (*u).to_owned()).collect(),
        }
    }

    fn utf16(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn requests_round_trip_as_one_json_line() {
        let r = parse_request(r#"{"cmd":"send","chat":"me","text":"hi","code":""}"#).unwrap();
        assert_eq!(
            r,
            Request::Send {
                chat: "me".to_owned(),
                text: "hi".to_owned(),
                files: Vec::new(),
                code: Some(String::new()),
            }
        );
        assert_eq!(
            parse_request(r#"{"cmd":"unread"}"#).unwrap(),
            Request::Unread
        );
        assert_eq!(
            parse_request("{\"cmd\":\"chats\"}\n").unwrap(),
            Request::Chats
        );
        assert!(parse_request("{\"cmd\":\"nope\"}").is_err());
        assert!(parse_request("not json").is_err());
        assert!(parse_request(r#"{"cmd":"open"}"#).is_err());
        let line = serde_json::to_string(&Request::Open {
            chat: "x".to_owned(),
        })
        .unwrap();
        assert_eq!(line, r#"{"cmd":"open","chat":"x"}"#);
    }

    #[test]
    fn responses_carry_ok_error_and_flat_data() {
        let ok = Response::ok(json!({"queued": 2}));
        assert_eq!(ok.line(), "{\"ok\":true,\"queued\":2}\n");
        let bad = Response::error("nope");
        assert_eq!(bad.line(), "{\"ok\":false,\"error\":\"nope\"}\n");
        assert_eq!(parse_response(&ok.line()).unwrap(), ok);
        assert_eq!(parse_response(&bad.line()).unwrap(), bad);
    }

    #[test]
    fn socket_lives_in_the_runtime_dir_unless_the_data_dir_is_overridden() {
        let data = || Ok(PathBuf::from("/data"));
        let run = Some(std::ffi::OsString::from("/run/user/1"));
        assert_eq!(
            socket_path_in(false, run.clone(), data).unwrap(),
            PathBuf::from("/run/user/1/gramtty.sock")
        );
        assert_eq!(
            socket_path_in(true, run, data).unwrap(),
            PathBuf::from("/data/gramtty.sock")
        );
        assert_eq!(
            socket_path_in(false, None, data).unwrap(),
            PathBuf::from("/data/gramtty.sock")
        );
        assert_eq!(
            socket_path_in(false, Some(std::ffi::OsString::new()), data).unwrap(),
            PathBuf::from("/data/gramtty.sock")
        );
    }

    #[test]
    fn short_text_stays_whole_and_empty_text_gives_nothing() {
        assert_eq!(split_text("hello\nworld\n", 4096), ["hello\nworld\n"]);
        assert!(split_text("", 10).is_empty());
        assert!(split_text("  \n\n", 10).is_empty());
    }

    #[test]
    fn long_text_splits_at_line_boundaries() {
        let parts = split_text("aaaa\nbbbb\ncccc\n", 10);
        assert_eq!(parts, ["aaaa\nbbbb\n", "cccc\n"]);
        assert_eq!(parts.concat(), "aaaa\nbbbb\ncccc\n");
    }

    #[test]
    fn an_overlong_line_is_hard_split() {
        let line = "x".repeat(25);
        let parts = split_text(&format!("ab\n{line}\ncd"), 10);
        assert_eq!(parts[0], "ab\n");
        assert!(parts.iter().all(|p| utf16(p) <= 10));
        assert_eq!(parts.concat(), format!("ab\n{line}\ncd"));
    }

    #[test]
    fn splitting_counts_utf16_and_never_cuts_a_char() {
        let text = "😀".repeat(7);
        let parts = split_text(&text, 5);
        assert!(parts.iter().all(|p| utf16(p) <= 5));
        assert_eq!(parts.concat(), text);
        assert_eq!(parts.len(), 4);
        let big = "é".repeat(5000);
        let parts = split_text(&big, MESSAGE_LIMIT);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.concat(), big);
    }

    #[test]
    fn me_and_ids_resolve_only_to_known_chats() {
        let chats = [key(1, "Saved", &[]), key(5, "Bob", &[])];
        assert_eq!(resolve_chat("me", &chats, Some(1)), Ok(1));
        assert_eq!(resolve_chat("ME", &chats, Some(1)), Ok(1));
        assert!(resolve_chat("me", &chats, None).is_err());
        assert!(resolve_chat("me", &chats, Some(9)).is_err());
        assert_eq!(resolve_chat("5", &chats, Some(1)), Ok(5));
        assert!(resolve_chat("6", &chats, Some(1)).is_err());
        assert!(resolve_chat("-100123", &chats, Some(1)).is_err());
    }

    #[test]
    fn usernames_match_case_insensitively_or_ask_for_a_lookup() {
        let chats = [key(5, "Bob", &["BobTheBuilder"])];
        assert_eq!(resolve_chat("@bobthebuilder", &chats, None), Ok(5));
        let err = resolve_chat("@nobody", &chats, None).unwrap_err();
        assert!(matches!(err, ResolveError::Lookup(_)));
        assert!(err.message().contains("open that chat once"));
    }

    #[test]
    fn titles_match_exactly_before_by_substring() {
        let chats = [
            key(1, "Work", &[]),
            key(2, "Work friends", &[]),
            key(3, "Family", &[]),
            key(4, "Family chat", &[]),
        ];
        assert_eq!(resolve_chat("work", &chats, None), Ok(1));
        assert_eq!(resolve_chat("friends", &chats, None), Ok(2));
        let err = resolve_chat("fam", &chats, None).unwrap_err().message();
        assert!(err.contains("Family (3)") && err.contains("Family chat (4)"));
        assert!(
            resolve_chat("zzz", &chats, None)
                .unwrap_err()
                .message()
                .contains("no chat")
        );
    }

    #[test]
    fn ambiguity_lists_at_most_ten_candidates() {
        let chats: Vec<ChatKey> = (0..15)
            .map(|i| key(i + 100, &format!("Team {i}"), &[]))
            .collect();
        let err = resolve_chat("team", &chats, None).unwrap_err().message();
        assert_eq!(err.matches(" (1").count(), 10);
        assert!(err.contains("5 more"));
    }

    #[test]
    fn send_arguments_parse() {
        let cli = parse_cli(&args(&[
            "send",
            "me",
            "--file",
            "a.txt",
            "--code",
            "diff",
            "--file=b.png",
            "hello",
            "world",
        ]))
        .unwrap();
        assert_eq!(
            cli.request,
            Request::Send {
                chat: "me".to_owned(),
                text: "hello world".to_owned(),
                files: vec!["a.txt".to_owned(), "b.png".to_owned()],
                code: Some("diff".to_owned()),
            }
        );
        assert!(!cli.json);
    }

    #[test]
    fn code_takes_a_language_only_when_one_follows() {
        let code = |a: &[&str]| match parse_cli(&args(a)).unwrap().request {
            Request::Send { code, text, .. } => (code, text),
            _ => panic!("not send"),
        };
        assert_eq!(
            code(&["send", "me", "--code"]),
            (Some(String::new()), String::new())
        );
        assert_eq!(
            code(&["send", "me", "--code", "--", "rust text"]),
            (Some(String::new()), "rust text".to_owned())
        );
        assert_eq!(
            code(&["send", "me", "--code=py", "x"]),
            (Some("py".to_owned()), "x".to_owned())
        );
        assert_eq!(code(&["send", "me"]), (None, String::new()));
    }

    #[test]
    fn double_dash_ends_options() {
        let cli = parse_cli(&args(&["send", "me", "--", "--file", "x"])).unwrap();
        assert_eq!(
            cli.request,
            Request::Send {
                chat: "me".to_owned(),
                text: "--file x".to_owned(),
                files: Vec::new(),
                code: None,
            }
        );
    }

    #[test]
    fn negative_chat_ids_are_accepted() {
        let cli = parse_cli(&args(&["send", "-100123", "hi"])).unwrap();
        assert!(matches!(cli.request, Request::Send { ref chat, .. } if chat == "-100123"));
        let open = parse_cli(&args(&["open", "-5"])).unwrap();
        assert_eq!(
            open.request,
            Request::Open {
                chat: "-5".to_owned()
            }
        );
    }

    #[test]
    fn bad_arguments_are_usage_errors() {
        for bad in [
            &["send"][..],
            &["send", "me", "--file"],
            &["send", "me", "--bogus"],
            &["unread", "--x"],
            &["chats", "extra"],
            &["open"],
            &["open", "a", "b"],
            &["frobnicate"],
        ] {
            assert!(parse_cli(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn list_commands_take_json() {
        let u = parse_cli(&args(&["unread", "--json"])).unwrap();
        assert_eq!((u.request, u.json), (Request::Unread, true));
        let c = parse_cli(&args(&["chats"])).unwrap();
        assert_eq!((c.request, c.json), (Request::Chats, false));
    }

    #[test]
    fn paths_are_made_absolute() {
        let cwd = std::path::Path::new("/work");
        assert_eq!(absolutize("a/b.txt", cwd), "/work/a/b.txt");
        assert_eq!(absolutize("/etc/x", cwd), "/etc/x");
    }

    #[test]
    fn unread_is_printed_for_humans_and_as_json() {
        let data = unread_data(
            (2, 5, 1),
            vec![
                json!({"id": 10, "title": "Team", "unread": 3, "mentions": 1, "muted": false}),
                json!({"id": 20, "title": "Bob", "unread": 2, "mentions": 0, "muted": true}),
            ],
        );
        let response = Response::ok(data);
        let human = Cli {
            request: Request::Unread,
            json: false,
        };
        assert_eq!(
            render(&human, &response).unwrap(),
            "2 chats, 5 messages, 1 mention\nTeam (10): 3, 1 mention\nBob (20): 2, muted\n"
        );
        let json = Cli {
            request: Request::Unread,
            json: true,
        };
        let out = render(&json, &response).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["total"]["messages"], 5);
        assert!(parsed.get("ok").is_none());
    }

    #[test]
    fn chats_are_printed_one_per_line() {
        let response = Response::ok(json!({"chats": [
            {"id": 1, "title": "Alice", "kind": "private", "username": "alice"},
            {"id": 2, "title": "News", "kind": "channel", "username": null},
        ]}));
        let cli = Cli {
            request: Request::Chats,
            json: false,
        };
        assert_eq!(
            render(&cli, &response).unwrap(),
            "1\tprivate\tAlice\t@alice\n2\tchannel\tNews\n"
        );
    }

    #[cfg(unix)]
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gramtty-ipc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stale_socket_file_is_replaced_and_gets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("stale");
        let path = dir.join("g.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let listener = bind(&path).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_socket_is_never_replaced() {
        let dir = scratch("live");
        let path = dir.join("g.sock");
        let first = bind(&path).await.unwrap();
        let err = bind(&path).await.unwrap_err();
        assert!(err.contains("already listening"), "{err}");
        assert!(path.exists());
        drop(first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_regular_file_at_the_socket_path_is_left_alone() {
        let dir = scratch("file");
        let path = dir.join("g.sock");
        std::fs::write(&path, "keep").unwrap();
        assert!(bind(&path).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_guard_removes_the_socket_file() {
        let dir = scratch("guard");
        let path = dir.join("g.sock");
        let listener = bind(&path).await.unwrap();
        let guard = SocketGuard(path.clone());
        drop(listener);
        drop(guard);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    async fn exchange(path: &std::path::Path, raw: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
        stream.write_all(raw).await.unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).await.unwrap();
        out
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_and_oversized_requests_get_error_replies() {
        let dir = scratch("bad");
        let path = dir.join("g.sock");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        serve(bind(&path).await.unwrap(), tx);
        let garbage = parse_response(&exchange(&path, b"not json\n").await).unwrap();
        assert!(!garbage.ok);
        assert!(garbage.error.unwrap().contains("bad request"));
        let big = vec![b'x'; MAX_REQUEST + 10];
        let big: &'static [u8] = Box::leak(big.into_boxed_slice());
        let reply = parse_response(&exchange(&path, big).await).unwrap();
        assert!(reply.error.unwrap().contains("too large"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_error_response_becomes_an_error() {
        let cli = Cli {
            request: Request::Unread,
            json: false,
        };
        assert_eq!(
            render(&cli, &Response::error("boom")),
            Err("boom".to_owned())
        );
    }
}
