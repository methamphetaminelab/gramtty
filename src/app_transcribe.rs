use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{App, ChatKind, FileJob, SearchView, attachment_name};
use crate::model::{MediaKind, Msg};
use crate::td::Found;
use crate::transcribe::{self, Output, Store};

pub type Key = (i64, i64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum View<'a> {
    Working { partial: &'a str, status: String },
    Text(&'a str),
}

pub fn percent(done_secs: f64, duration_secs: i32) -> Option<u32> {
    if duration_secs <= 0 || !done_secs.is_finite() || done_secs < 0.0 {
        return None;
    }
    Some(((done_secs / f64::from(duration_secs) * 100.0) as u32).min(99))
}

fn working_status(
    elapsed: Option<Duration>,
    partial: Option<&(String, Option<f64>)>,
    duration_secs: i32,
) -> String {
    let Some(elapsed) = elapsed else {
        return "transcribing…".to_owned();
    };
    let secs = elapsed.as_secs();
    match partial {
        None => format!("transcribing… loading model, {secs} s"),
        Some((_, done)) => match done.and_then(|d| percent(d, duration_secs)) {
            Some(p) => format!("transcribing… {p}%"),
            None => format!("transcribing… {secs} s"),
        },
    }
}

pub struct Transcripts {
    store: Store,
    shown: HashSet<Key>,
    pending: HashSet<Key>,
    ready: VecDeque<(Key, PathBuf)>,
    running: Option<Key>,
    partial: HashMap<Key, (String, Option<f64>)>,
    started: HashMap<Key, Instant>,
    available: RefCell<Option<(Vec<String>, bool)>>,
    pub(super) probe: fn(&str) -> bool,
}

fn on_path(program: &str) -> bool {
    crate::config::find_in_path(program).is_some()
}

impl Default for Transcripts {
    fn default() -> Self {
        Self {
            store: Store::default(),
            shown: HashSet::new(),
            pending: HashSet::new(),
            ready: VecDeque::new(),
            running: None,
            partial: HashMap::new(),
            started: HashMap::new(),
            available: RefCell::new(None),
            probe: on_path,
        }
    }
}

impl Transcripts {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.store.len() == 0
            && self.store.path().is_none()
            && self.shown.is_empty()
            && self.pending.is_empty()
            && self.ready.is_empty()
            && self.running.is_none()
            && self.partial.is_empty()
            && self.started.is_empty()
    }

    pub fn view(&self, msg: &Msg) -> Option<View<'_>> {
        if !msg
            .attachments
            .first()
            .is_some_and(|a| spoken(a.kind))
        {
            return None;
        }
        let key = (msg.chat_id, msg.id);
        if self.pending.contains(&key) {
            let partial = self.partial.get(&key);
            let duration = msg.attachments.first().map_or(0, |a| a.duration);
            let elapsed = self.started.get(&key).map(|t| t.elapsed());
            return Some(View::Working {
                partial: partial.map_or("", |(text, _)| text.as_str()),
                status: working_status(elapsed, partial, duration),
            });
        }
        if !self.shown.contains(&key) {
            return None;
        }
        self.store.get(key.0, key.1).map(View::Text)
    }

    #[cfg(test)]
    pub fn show(&mut self, chat_id: i64, msg_id: i64, text: &str) {
        self.store
            .insert(chat_id, msg_id, text.to_owned(), false)
            .unwrap();
        self.pending.remove(&(chat_id, msg_id));
        self.shown.insert((chat_id, msg_id));
    }

    #[cfg(test)]
    pub fn hide(&mut self, chat_id: i64, msg_id: i64) {
        self.shown.remove(&(chat_id, msg_id));
    }

    #[cfg(test)]
    pub fn work(&mut self, chat_id: i64, msg_id: i64) {
        self.pending.insert((chat_id, msg_id));
    }

    #[cfg(test)]
    pub fn progress(&mut self, chat_id: i64, msg_id: i64, text: &str, done: Option<f64>, ago: u64) {
        let key = (chat_id, msg_id);
        self.pending.insert(key);
        self.started.insert(key, Instant::now() - Duration::from_secs(ago));
        if !text.is_empty() {
            self.partial.insert(key, (text.to_owned(), done));
        }
    }

    pub fn cached_in(&self, chat_id: i64) -> impl Iterator<Item = (i64, &str)> {
        self.store.of_chat(chat_id)
    }
}

fn spoken(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Voice | MediaKind::VideoNote)
}

impl App {
    pub(super) fn transcribable(msg: &Msg) -> bool {
        !msg.deleted && msg.attachments.first().is_some_and(|a| spoken(a.kind))
    }

    pub(super) fn transcription_available(&self) -> bool {
        let configured = &self.settings.transcription.command;
        let mut cache = self.transcripts.available.borrow_mut();
        if let Some((command, found)) = cache.as_ref()
            && command == configured
        {
            return *found;
        }
        let found = !configured.is_empty() || transcribe::detect(self.transcripts.probe).is_some();
        *cache = Some((configured.clone(), found));
        found
    }

    pub(super) fn transcribe_tick(&self) -> Option<Duration> {
        (!self.transcripts.started.is_empty()).then_some(Duration::from_secs(1))
    }

    pub(super) fn on_transcript_partial(
        &mut self,
        chat_id: i64,
        msg_id: i64,
        text: String,
        done_secs: Option<f64>,
    ) {
        let key = (chat_id, msg_id);
        if self.transcripts.pending.contains(&key) && self.transcripts.running == Some(key) {
            self.transcripts.partial.insert(key, (text, done_secs));
        }
    }

    fn transcript_command(&self, file: &Path) -> Option<(Vec<String>, Output)> {
        let out_dir = crate::config::cache_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("transcribe");
        let cfg = &self.settings.transcription;
        transcribe::command(&cfg.command, &cfg.language, file, &out_dir, self.transcripts.probe)
    }

    pub(super) fn open_transcripts(&mut self, path: PathBuf) {
        if self.transcripts.store.path() == Some(path.as_path()) {
            return;
        }
        let (store, warning) = Store::open(path);
        self.transcripts.store = store;
        if let Some(w) = warning {
            self.set_status(w, true);
        }
    }

    pub(super) fn add_cached_transcripts(&self, sv: &SearchView, found: &mut Found) {
        let (Some(chat_id), Some(query)) = (sv.chat_id, sv.searched_for.as_deref()) else {
            return;
        };
        if query.is_empty() || sv.pinned || sv.sender.is_some() || sv.media.is_some() {
            return;
        }
        let Some(history) = self.histories.get(&chat_id) else {
            return;
        };
        let floor = match found.next {
            Some(_) => match found.msgs.iter().map(|m| m.id).min() {
                Some(low) => low,
                None => return,
            },
            None => i64::MIN,
        };
        let needle = query.to_lowercase();
        let mut extra: Vec<Msg> = self
            .transcripts
            .cached_in(chat_id)
            .filter(|(id, text)| *id >= floor && text.to_lowercase().contains(&needle))
            .filter(|(id, _)| !found.msgs.iter().any(|m| m.id == *id))
            .filter_map(|(id, _)| history.get(id).filter(|m| !m.deleted).cloned())
            .collect();
        let fresh = extra
            .iter()
            .filter(|m| !sv.results.iter().any(|r| r.id == m.id))
            .count();
        found.total += fresh as i32;
        found.msgs.append(&mut extra);
        found.msgs.sort_by_key(|m| std::cmp::Reverse(m.id));
    }

    pub(super) fn transcribe_message(&mut self, msg: &Msg) {
        let Some(att) = msg.attachments.first().filter(|a| spoken(a.kind)) else {
            self.set_status("nothing to transcribe in this message", false);
            return;
        };
        let key = (msg.chat_id, msg.id);
        if self.transcripts.store.get(key.0, key.1).is_some() {
            if !self.transcripts.shown.remove(&key) {
                self.transcripts.shown.insert(key);
            }
            return;
        }
        if self.transcripts.pending.contains(&key) {
            self.set_status("already transcribing this message", false);
            return;
        }
        if self.transcript_command(Path::new("probe")).is_none() {
            self.set_status(transcribe::MISSING, true);
            return;
        }
        if !self.file_reachable(&att.file) {
            self.set_status("this file is not available", true);
            return;
        }
        let name = attachment_name(att, msg.id);
        let file = att.file.clone();
        self.transcripts.pending.insert(key);
        self.queue_file_job(&file, &name, FileJob::Transcribe(key.0, key.1));
    }

    pub(super) fn transcript_downloaded(&mut self, path: &str, chat_id: i64, msg_id: i64) {
        let key = (chat_id, msg_id);
        if !self.transcripts.pending.contains(&key) {
            return;
        }
        self.transcripts.ready.push_back((key, PathBuf::from(path)));
        self.pump_transcripts();
    }

    pub(super) fn transcript_download_failed(&mut self, file_id: i32) {
        let failed: Vec<Key> = self
            .file_jobs
            .iter()
            .filter(|(id, _)| *id == file_id)
            .filter_map(|(_, job)| match job {
                FileJob::Transcribe(c, m) => Some((*c, *m)),
                _ => None,
            })
            .collect();
        for key in failed {
            self.transcripts.pending.remove(&key);
        }
    }

    fn pump_transcripts(&mut self) {
        while self.transcripts.running.is_none() {
            let Some((key, path)) = self.transcripts.ready.pop_front() else {
                return;
            };
            match self.transcript_command(&path) {
                Some((args, output)) => {
                    self.transcripts.running = Some(key);
                    self.transcripts.started.insert(key, Instant::now());
                    self.transcripts.partial.remove(&key);
                    self.td.transcribe(key.0, key.1, args, output);
                }
                None => {
                    self.transcripts.pending.remove(&key);
                    self.set_status(transcribe::MISSING, true);
                }
            }
        }
    }

    pub(super) fn on_transcript(&mut self, chat_id: i64, msg_id: i64, result: Result<String, String>) {
        let key = (chat_id, msg_id);
        if self.transcripts.running == Some(key) {
            self.transcripts.running = None;
        }
        self.transcripts.started.remove(&key);
        self.transcripts.partial.remove(&key);
        if self.transcripts.pending.remove(&key) {
            match result {
                Ok(text) => {
                    let persist = self
                        .chats
                        .get(&chat_id)
                        .is_some_and(|c| !c.protected && !matches!(c.kind, ChatKind::Secret(_)));
                    if let Err(e) = self.transcripts.store.insert(chat_id, msg_id, text, persist) {
                        self.set_status(crate::td::failed("save the transcript", &e), true);
                    }
                    self.transcripts.shown.insert(key);
                }
                Err(e) => self.set_status(crate::td::failed("transcribe the message", &e), true),
            }
        }
        self.pump_transcripts();
    }
}
