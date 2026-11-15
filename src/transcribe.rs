use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const TIMEOUT: Duration = Duration::from_secs(600);
pub const STORE_LIMIT: usize = 5000;
pub const MISSING: &str =
    "no speech-to-text program found (whisper, whisper-ctranslate2); set [transcription] command";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    Stdout,
    File(PathBuf),
}

fn substitute(arg: &str, language: &str, file: &Path) -> String {
    let lang = if language.is_empty() { "auto" } else { language };
    arg.replace("{file}", &file.to_string_lossy())
        .replace("{lang}", lang)
}

fn text_file(out_dir: &Path, file: &Path) -> PathBuf {
    let stem = file.file_stem().unwrap_or_default();
    out_dir.join(format!("{}.txt", stem.to_string_lossy()))
}

pub fn detect(available: impl Fn(&str) -> bool) -> Option<&'static str> {
    ["whisper-ctranslate2", "whisper"]
        .into_iter()
        .find(|t| available(t))
}

pub fn command(
    configured: &[String],
    language: &str,
    file: &Path,
    out_dir: &Path,
    available: impl Fn(&str) -> bool,
) -> Option<(Vec<String>, Output)> {
    if !configured.is_empty() {
        let args = configured
            .iter()
            .map(|a| substitute(a, language, file))
            .collect();
        return Some((args, Output::Stdout));
    }
    let tool = detect(available)?;
    let mut args = vec![
        tool.to_owned(),
        file.to_string_lossy().into_owned(),
        "--output_format".to_owned(),
        "txt".to_owned(),
        "--output_dir".to_owned(),
        out_dir.to_string_lossy().into_owned(),
    ];
    args.push("--verbose".to_owned());
    args.push("True".to_owned());
    if !language.is_empty() {
        args.push("--language".to_owned());
        args.push(language.to_owned());
    }
    Some((args, Output::File(text_file(out_dir, file))))
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("no error output")
        .to_owned()
}

fn tidy(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_clock(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':').rev();
    let seconds: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let hours: f64 = match parts.next() {
        Some(h) => h.parse().ok()?,
        None => 0.0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

pub fn parse_segment(line: &str) -> Option<(f64, String)> {
    let rest = line.trim().strip_prefix('[')?;
    let (range, text) = rest.split_once(']')?;
    let (start, end) = range.split_once("-->")?;
    parse_clock(start)?;
    let end = parse_clock(end)?;
    let text = text.trim();
    (!text.is_empty()).then(|| (end, text.to_owned()))
}

pub async fn run(
    args: Vec<String>,
    output: Output,
    partial: impl FnMut(String, Option<f64>),
) -> Result<String, String> {
    run_within(args, output, TIMEOUT, partial).await
}

async fn run_within(
    args: Vec<String>,
    output: Output,
    limit: Duration,
    mut partial: impl FnMut(String, Option<f64>),
) -> Result<String, String> {
    let (program, rest) = args.split_first().ok_or("empty transcription command")?;
    if let Output::File(path) = &output
        && let Some(dir) = path.parent()
    {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut child = tokio::process::Command::new(program)
        .args(rest)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut stderr = child.stderr.take().ok_or("no stderr pipe")?;
    let errors = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });
    let stream = async {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut raw = Vec::new();
        let mut all = String::new();
        let mut chunks: Vec<String> = Vec::new();
        loop {
            raw.clear();
            if reader.read_until(b'\n', &mut raw).await? == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&raw).into_owned();
            all.push_str(&line);
            let (chunk, secs) = match &output {
                Output::Stdout => (Some(line.trim().to_owned()), None),
                Output::File(_) => match parse_segment(&line) {
                    Some((end, text)) => (Some(text), Some(end)),
                    None => (None, None),
                },
            };
            if let Some(chunk) = chunk.filter(|c| !c.is_empty()) {
                chunks.push(chunk);
                partial(chunks.join(" "), secs);
            }
        }
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, all, chunks))
    };
    let waited = tokio::time::timeout(limit, stream).await;
    let result = match waited {
        Err(_) => Err(format!(
            "{program} did not finish within {} minutes",
            limit.as_secs() / 60
        )),
        Ok(Err(e)) => Err(format!("{program}: {e}")),
        Ok(Ok((status, ..))) if !status.success() => {
            let stderr = errors.await.unwrap_or_default();
            Err(format!("{program} failed: {}", last_line(&stderr)))
        }
        Ok(Ok((_, all, chunks))) => match &output {
            Output::Stdout => Ok(all),
            Output::File(path) => match std::fs::read_to_string(path) {
                Ok(text) if !tidy(&text).is_empty() => Ok(text),
                _ if !chunks.is_empty() => Ok(chunks.join(" ")),
                Ok(text) => Ok(text),
                Err(e) => Err(format!("cannot read {}: {e}", path.display())),
            },
        },
    };
    if let Output::File(path) = &output {
        let _ = std::fs::remove_file(path);
    }
    let text = tidy(&result?);
    if text.is_empty() {
        return Err("no speech recognized".to_owned());
    }
    Ok(text)
}

fn key_string(chat_id: i64, msg_id: i64) -> String {
    format!("{chat_id}:{msg_id}")
}

fn parse_key(key: &str) -> Option<(i64, i64)> {
    let (chat, msg) = key.split_once(':')?;
    Some((chat.parse().ok()?, msg.parse().ok()?))
}

struct Persisted<'a>(Vec<(String, &'a str)>);

impl Serialize for Persisted<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

struct Loaded(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Loaded {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Loaded;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of message keys to transcripts")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Loaded, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = m.next_entry::<String, String>()? {
                    out.push(entry);
                }
                Ok(Loaded(out))
            }
        }
        d.deserialize_map(V)
    }
}

#[derive(Default)]
pub struct Store {
    path: Option<PathBuf>,
    order: VecDeque<(i64, i64)>,
    texts: HashMap<(i64, i64), (String, bool)>,
}

impl Store {
    pub fn open(path: PathBuf) -> (Store, Option<String>) {
        let mut store = Store {
            path: Some(path.clone()),
            ..Store::default()
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (store, None),
            Err(e) => {
                return (
                    store,
                    Some(format!("cannot read {}: {e}; it will be replaced", path.display())),
                );
            }
        };
        match serde_json::from_str::<Loaded>(&raw) {
            Ok(Loaded(entries)) => {
                for (key, text) in entries {
                    if let Some((chat, msg)) = parse_key(&key) {
                        store.put(chat, msg, text, true);
                    }
                }
                (store, None)
            }
            Err(e) => (
                store,
                Some(format!(
                    "transcripts file {} is unreadable ({e}); it will be replaced",
                    path.display()
                )),
            ),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn get(&self, chat_id: i64, msg_id: i64) -> Option<&str> {
        self.texts.get(&(chat_id, msg_id)).map(|(t, _)| t.as_str())
    }

    pub fn of_chat(&self, chat_id: i64) -> impl Iterator<Item = (i64, &str)> {
        self.order
            .iter()
            .filter(move |(c, _)| *c == chat_id)
            .filter_map(move |(c, m)| self.get(*c, *m).map(|t| (*m, t)))
    }

    fn put(&mut self, chat_id: i64, msg_id: i64, text: String, persist: bool) {
        let key = (chat_id, msg_id);
        if self.texts.insert(key, (text, persist)).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > STORE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.texts.remove(&old);
            }
        }
    }

    pub fn insert(
        &mut self,
        chat_id: i64,
        msg_id: i64,
        text: String,
        persist: bool,
    ) -> Result<(), String> {
        self.put(chat_id, msg_id, text, persist);
        if persist { self.save() } else { Ok(()) }
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let rows = self
            .order
            .iter()
            .filter_map(|k| {
                let (text, persist) = self.texts.get(k)?;
                persist.then(|| (key_string(k.0, k.1), text.as_str()))
            })
            .collect();
        let data = serde_json::to_vec(&Persisted(rows))
            .map_err(|e| format!("cannot encode transcripts: {e}"))?;
        if let Some(dir) = path.parent() {
            crate::config::create_private_dir(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .map_err(|e| format!("cannot open {}: {e}", tmp.display()))?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        drop(file);
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("cannot replace {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| (*a).to_owned()).collect()
    }

    fn file() -> PathBuf {
        PathBuf::from("/c/voice_7.oga")
    }

    fn out() -> PathBuf {
        PathBuf::from("/o")
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gramtty-transcribe-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn configured_command_replaces_placeholders() {
        let cfg = s(&["whisper-cli", "-m", "m.bin", "-l", "{lang}", "-f", "{file}"]);
        let got = command(&cfg, "ru", &file(), &out(), |_| false).unwrap();
        assert_eq!(
            got,
            (
                s(&["whisper-cli", "-m", "m.bin", "-l", "ru", "-f", "/c/voice_7.oga"]),
                Output::Stdout
            )
        );
        let auto = command(&cfg, "", &file(), &out(), |_| false).unwrap();
        assert_eq!(auto.0[4], "auto");
    }

    #[test]
    fn configured_command_beats_installed_tools() {
        let got = command(&s(&["x", "{file}"]), "", &file(), &out(), |_| true).unwrap();
        assert_eq!(got.0, s(&["x", "/c/voice_7.oga"]));
    }

    #[test]
    fn ctranslate2_is_preferred() {
        let got = command(&[], "de", &file(), &out(), |_| true).unwrap();
        assert_eq!(
            got,
            (
                s(&[
                    "whisper-ctranslate2",
                    "/c/voice_7.oga",
                    "--output_format",
                    "txt",
                    "--output_dir",
                    "/o",
                    "--verbose",
                    "True",
                    "--language",
                    "de"
                ]),
                Output::File(PathBuf::from("/o/voice_7.txt"))
            )
        );
    }

    #[test]
    fn openai_whisper_is_the_fallback() {
        let got = command(&[], "", &file(), &out(), |t| t == "whisper").unwrap();
        assert_eq!(
            got,
            (
                s(&[
                    "whisper",
                    "/c/voice_7.oga",
                    "--output_format",
                    "txt",
                    "--output_dir",
                    "/o",
                    "--verbose",
                    "True"
                ]),
                Output::File(PathBuf::from("/o/voice_7.txt"))
            )
        );
        let ru = command(&[], "ru", &file(), &out(), |t| t == "whisper").unwrap();
        assert_eq!(&ru.0[ru.0.len() - 2..], &s(&["--language", "ru"])[..]);
    }

    #[test]
    fn whisper_cpp_is_not_auto_detected() {
        assert!(command(&[], "", &file(), &out(), |t| t == "whisper-cli").is_none());
    }

    #[test]
    fn no_tool_means_no_command() {
        assert!(command(&[], "", &file(), &out(), |_| false).is_none());
    }

    #[tokio::test]
    async fn stdout_is_trimmed_and_joined() {
        let args = s(&["sh", "-c", "printf ' hello\\nworld \\n'"]);
        assert_eq!(run(args, Output::Stdout, |_, _| {}).await.unwrap(), "hello world");
    }

    #[tokio::test]
    async fn file_output_is_read_and_removed() {
        let d = dir("file");
        let path = d.join("nested").join("out.txt");
        let args = s(&["sh", "-c", "printf hi > \"$1\"", "sh", &path.to_string_lossy()]);
        let got = run(args, Output::File(path.clone()), |_, _| {}).await.unwrap();
        assert_eq!(got, "hi");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn failure_reports_last_stderr_line() {
        let args = s(&["sh", "-c", "echo first >&2; echo boom >&2; exit 3"]);
        let err = run(args, Output::Stdout, |_, _| {}).await.unwrap_err();
        assert_eq!(err, "sh failed: boom");
    }

    #[tokio::test]
    async fn empty_result_is_an_error() {
        let args = s(&["sh", "-c", "printf ' \\n\\n'"]);
        assert_eq!(
            run(args, Output::Stdout, |_, _| {}).await.unwrap_err(),
            "no speech recognized"
        );
    }

    #[tokio::test]
    async fn missing_program_is_an_error() {
        let args = s(&["gramtty-no-such-program-xyz"]);
        let err = run(args, Output::Stdout, |_, _| {}).await.unwrap_err();
        assert!(err.starts_with("gramtty-no-such-program-xyz:"), "{err}");
    }

    #[tokio::test]
    async fn slow_command_times_out() {
        let args = s(&["sh", "-c", "sleep 5"]);
        let err = run_within(args, Output::Stdout, Duration::from_millis(100), |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
    }

    #[test]
    fn store_round_trip_keeps_order() {
        let d = dir("store");
        let path = d.join("transcripts.json");
        let (mut store, warn) = Store::open(path.clone());
        assert!(warn.is_none());
        store.insert(5, 20, "second \"quoted\"".to_owned(), true).unwrap();
        store.insert(5, 10, "first".to_owned(), true).unwrap();
        store.insert(6, 1, "other chat".to_owned(), true).unwrap();
        let (again, warn) = Store::open(path.clone());
        assert!(warn.is_none());
        assert_eq!(again.get(5, 20), Some("second \"quoted\""));
        assert_eq!(again.get(6, 1), Some("other chat"));
        assert_eq!(again.get(9, 9), None);
        assert_eq!(again.of_chat(5).collect::<Vec<_>>(), [(20, "second \"quoted\""), (10, "first")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn memory_only_entries_are_not_written() {
        let d = dir("memory");
        let path = d.join("transcripts.json");
        let (mut store, _) = Store::open(path.clone());
        store.insert(1, 1, "kept".to_owned(), true).unwrap();
        store.insert(2, 2, "secret".to_owned(), false).unwrap();
        assert_eq!(store.get(2, 2), Some("secret"));
        let (again, _) = Store::open(path);
        assert_eq!(again.get(1, 1), Some("kept"));
        assert_eq!(again.get(2, 2), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn store_is_capped_dropping_oldest() {
        let mut store = Store::default();
        for i in 0..(STORE_LIMIT as i64 + 3) {
            store.insert(1, i, format!("t{i}"), true).unwrap();
        }
        assert_eq!(store.len(), STORE_LIMIT);
        assert_eq!(store.get(1, 0), None);
        assert_eq!(store.get(1, 2), None);
        assert_eq!(store.get(1, 3), Some("t3"));
        assert_eq!(store.get(1, STORE_LIMIT as i64 + 2), Some("t5002"));
    }

    #[test]
    fn corrupt_file_warns_once_and_is_replaced() {
        let d = dir("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("transcripts.json");
        std::fs::write(&path, "{not json").unwrap();
        let (mut store, warn) = Store::open(path.clone());
        assert!(warn.unwrap().contains("unreadable"));
        assert_eq!(store.len(), 0);
        store.insert(1, 2, "fresh".to_owned(), true).unwrap();
        let (again, warn) = Store::open(path);
        assert!(warn.is_none());
        assert_eq!(again.get(1, 2), Some("fresh"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn segment_lines_are_parsed_with_their_end_time() {
        assert_eq!(
            parse_segment("[00:00.000 --> 00:04.320]  Some text"),
            Some((4.32, "Some text".to_owned()))
        );
        assert_eq!(
            parse_segment("[01:02:03.500 --> 01:02:05.000] late words"),
            Some((3725.0, "late words".to_owned()))
        );
        assert_eq!(
            parse_segment("[00:10.000 --> 00:12.000]  windows\r"),
            Some((12.0, "windows".to_owned()))
        );
    }

    #[test]
    fn other_lines_are_not_segments() {
        for line in [
            "Detecting language using up to the first 30 seconds.",
            "Detected language: Russian",
            "/usr/lib/python3/site-packages/whisper/transcribe.py:115: UserWarning: FP16",
            "[warn] something",
            "[00:00.000 --> 00:01.000]   ",
            "[00:00.000 -> 00:01.000] text",
            "[aa:bb.ccc --> 00:01.000] text",
            "",
        ] {
            assert_eq!(parse_segment(line), None, "{line}");
        }
    }

    fn timed_partials() -> (Vec<(String, Option<f64>, Duration)>, std::time::Instant) {
        (Vec::new(), std::time::Instant::now())
    }

    #[tokio::test]
    async fn segments_stream_before_the_process_ends() {
        let d = dir("stream");
        let path = d.join("missing.txt");
        let args = s(&[
            "sh",
            "-c",
            "printf '[00:00.000 --> 00:02.000]  hello\\n'; sleep 0.2; printf '[00:02.000 --> 00:04.000]  world\\n'",
        ]);
        let (mut got, start) = timed_partials();
        let text = run(args, Output::File(path), |t, secs| {
            got.push((t, secs, start.elapsed()));
        })
        .await
        .unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].0.as_str(), got[0].1), ("hello", Some(2.0)));
        assert_eq!((got[1].0.as_str(), got[1].1), ("hello world", Some(4.0)));
        assert!(got[1].2 - got[0].2 >= Duration::from_millis(150), "{got:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn configured_stdout_lines_are_partial_chunks_without_seconds() {
        let args = s(&["sh", "-c", "printf 'one\\n\\ntwo\\n'"]);
        let mut got = Vec::new();
        let text = run(args, Output::Stdout, |t, secs| got.push((t, secs)))
            .await
            .unwrap();
        assert_eq!(text, "one two");
        assert_eq!(
            got,
            [("one".to_owned(), None), ("one two".to_owned(), None)]
        );
    }

    #[tokio::test]
    async fn file_text_wins_over_segments_and_non_segments_are_ignored() {
        let d = dir("wins");
        let path = d.join("out.txt");
        let args = s(&[
            "sh",
            "-c",
            "echo 'Detected language: English'; echo '[00:00.000 --> 00:01.000] draft'; printf 'final text' > \"$1\"",
            "sh",
            &path.to_string_lossy(),
        ]);
        let mut got = Vec::new();
        let text = run(args, Output::File(path), |t, secs| got.push((t, secs)))
            .await
            .unwrap();
        assert_eq!(text, "final text");
        assert_eq!(got, [("draft".to_owned(), Some(1.0))]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
