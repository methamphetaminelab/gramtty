use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{MediaKind, Msg, MsgMeta, RichText, SendState, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    User(i64),
    Chat(i64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub sender: Author,
    pub date: i64,
    #[serde(default)]
    pub edit_date: i64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub media: Option<String>,
    #[serde(default)]
    pub outgoing: bool,
    #[serde(default)]
    pub topic: Option<i32>,
    #[serde(default)]
    pub reply_to: Option<i64>,
    #[serde(default)]
    pub album: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Record {
    Deleted {
        chat: i64,
        id: i64,
        at: i64,
        msg: Snapshot,
    },
    Edited {
        chat: i64,
        id: i64,
        at: i64,
        prev: Snapshot,
    },
}

impl Record {
    fn key(&self) -> (i64, i64) {
        match self {
            Record::Deleted { chat, id, .. } | Record::Edited { chat, id, .. } => (*chat, *id),
        }
    }
}

pub struct Archive {
    path: Option<PathBuf>,
    records: Vec<Record>,
    limit: usize,
}

fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "[photo]",
        MediaKind::Video => "[video]",
        MediaKind::Animation => "[GIF]",
        MediaKind::VideoNote => "[video message]",
        MediaKind::Voice => "[voice message]",
        MediaKind::Audio => "[audio]",
        MediaKind::Sticker => "[sticker]",
        MediaKind::Document => "[file]",
    }
}

pub fn snapshot(m: &Msg) -> Snapshot {
    let media = match (&m.media, m.attachments.first()) {
        (Some(label), _) => Some(label.clone()),
        (None, Some(a)) if a.name.is_empty() => Some(kind_label(a.kind).to_owned()),
        (None, Some(a)) => Some(format!("{} {}", kind_label(a.kind), a.name)),
        (None, None) => None,
    };
    Snapshot {
        sender: match m.sender {
            Sender::User(id) => Author::User(id),
            Sender::Chat(id) => Author::Chat(id),
        },
        date: m.date,
        edit_date: m.meta.edit_date,
        text: m.body.text.clone(),
        media,
        outgoing: m.outgoing,
        topic: m.topic,
        reply_to: m.reply_to,
        album: m.album,
    }
}

pub fn restore(chat_id: i64, id: i64, s: &Snapshot) -> Msg {
    Msg {
        id,
        chat_id,
        sender: match s.sender {
            Author::User(id) => Sender::User(id),
            Author::Chat(id) => Sender::Chat(id),
        },
        date: s.date,
        edited: s.edit_date > 0,
        outgoing: s.outgoing,
        state: SendState::Sent,
        reply_to: s.reply_to,
        album: s.album,
        media: s.media.clone(),
        attachments: Vec::new(),
        body: RichText::plain(s.text.clone()),
        pinned: false,
        mention: false,
        unread_reaction: false,
        reactions: Vec::new(),
        keyboard: None,
        forward: None,
        topic: s.topic,
        ttl: 0,
        deleted: true,
        meta: MsgMeta {
            edit_date: s.edit_date,
            can_be_saved: true,
            ..MsgMeta::default()
        },
    }
}

fn write_private(path: &Path, data: &[u8], append: bool) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).mode(0o600);
    if append {
        opts.append(true);
    } else {
        opts.write(true).truncate(true);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    file.write_all(data)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

impl Archive {
    pub fn memory(limit: usize) -> Archive {
        Archive {
            path: None,
            records: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn open(path: PathBuf, limit: usize) -> Result<(Archive, usize), String> {
        let mut archive = Archive {
            path: Some(path.clone()),
            records: Vec::new(),
            limit: limit.max(1),
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((archive, 0)),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        let mut corrupt = 0;
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Record>(line) {
                Ok(r) => archive.records.push(r),
                Err(_) => corrupt += 1,
            }
        }
        if archive.records.len() > archive.limit || corrupt > 0 {
            archive.compact()?;
        }
        Ok((archive, corrupt))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    fn compact(&mut self) -> Result<(), String> {
        if self.records.len() > self.limit {
            let keep = (self.limit * 9 / 10).max(1);
            let drop = self.records.len() - keep;
            self.records.drain(..drop);
        }
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        let mut out = String::new();
        for r in &self.records {
            out.push_str(&serde_json::to_string(r).map_err(|e| e.to_string())?);
            out.push('\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        write_private(&tmp, out.as_bytes(), false)?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("cannot save {}: {e}", path.display()))
    }

    fn push(&mut self, record: Record) -> Result<(), String> {
        let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        self.records.push(record);
        if self.records.len() > self.limit {
            return self.compact();
        }
        match &self.path {
            Some(path) => write_private(path, format!("{line}\n").as_bytes(), true),
            None => Ok(()),
        }
    }

    pub fn is_deleted(&self, chat: i64, id: i64) -> bool {
        self.records
            .iter()
            .any(|r| matches!(r, Record::Deleted { .. }) && r.key() == (chat, id))
    }

    pub fn record_deleted(&mut self, m: &Msg, at: i64) -> Result<bool, String> {
        if self.is_deleted(m.chat_id, m.id) {
            return Ok(false);
        }
        self.push(Record::Deleted {
            chat: m.chat_id,
            id: m.id,
            at,
            msg: snapshot(m),
        })?;
        Ok(true)
    }

    pub fn record_edit(&mut self, m: &Msg, at: i64) -> Result<(), String> {
        self.push(Record::Edited {
            chat: m.chat_id,
            id: m.id,
            at,
            prev: snapshot(m),
        })
    }

    pub fn deleted_between(&self, chat: i64, lo: i64, hi: i64) -> Vec<Msg> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Deleted {
                    chat: c, id, msg, ..
                } if *c == chat && (lo..=hi).contains(id) => Some(restore(chat, *id, msg)),
                _ => None,
            })
            .collect()
    }

    pub fn edits(&self, chat: i64, id: i64) -> Vec<(i64, &Snapshot)> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Edited { at, prev, .. } if r.key() == (chat, id) => Some((*at, prev)),
                _ => None,
            })
            .collect()
    }

    pub fn has_edits(&self, chat: i64, id: i64) -> bool {
        self.records
            .iter()
            .any(|r| matches!(r, Record::Edited { .. }) && r.key() == (chat, id))
    }

    pub fn clear(&mut self) -> Result<usize, String> {
        let n = self.records.len();
        self.records.clear();
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot remove {}: {e}", path.display())),
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(chat: i64, id: i64, text: &str) -> Msg {
        let mut m = restore(
            chat,
            id,
            &Snapshot {
                sender: Author::User(2),
                date: 1_700_000_000 + id,
                edit_date: 0,
                text: text.to_owned(),
                media: None,
                outgoing: false,
                topic: None,
                reply_to: None,
                album: 0,
            },
        );
        m.deleted = false;
        m
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gramtty-archive-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn deleted_and_edits_survive_reopen_and_file_is_private() {
        let d = dir("reopen");
        let path = d.join("archive").join("1.jsonl");
        let (mut a, corrupt) = Archive::open(path.clone(), 100).unwrap();
        assert_eq!((a.len(), corrupt), (0, 0));
        assert!(a.record_deleted(&msg(10, 5, "secret plan"), 1).unwrap());
        assert!(!a.record_deleted(&msg(10, 5, "secret plan"), 2).unwrap());
        a.record_edit(&msg(10, 7, "first"), 3).unwrap();
        a.record_edit(&msg(10, 7, "second"), 4).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (b, corrupt) = Archive::open(path.clone(), 100).unwrap();
        assert_eq!(corrupt, 0);
        let restored = b.deleted_between(10, 0, 100);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].body.text, "secret plan");
        assert!(restored[0].deleted);
        assert!(b.deleted_between(11, 0, 100).is_empty());
        assert!(b.deleted_between(10, 6, 100).is_empty());
        let edits: Vec<&str> = b
            .edits(10, 7)
            .iter()
            .map(|(_, s)| s.text.as_str())
            .collect();
        assert_eq!(edits, ["first", "second"]);
        assert!(b.has_edits(10, 7) && !b.has_edits(10, 5));
        let mut b = b;
        assert_eq!(b.clear().unwrap(), 3);
        assert!(!path.exists());
        let (c, _) = Archive::open(path, 100).unwrap();
        assert_eq!(c.len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn storage_is_bounded_and_corrupt_lines_are_counted() {
        let d = dir("bounded");
        let path = d.join("1.jsonl");
        let (mut a, _) = Archive::open(path.clone(), 10).unwrap();
        for i in 0..25 {
            a.record_deleted(&msg(10, i, "x"), i).unwrap();
        }
        assert!(a.len() <= 10, "{}", a.len());
        assert!(a.is_deleted(10, 24));
        assert!(!a.is_deleted(10, 0));
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, a.len());
        let mut raw = std::fs::read_to_string(&path).unwrap();
        raw.push_str("{not json\n");
        std::fs::write(&path, raw).unwrap();
        let (b, corrupt) = Archive::open(path.clone(), 10).unwrap();
        assert_eq!(corrupt, 1);
        assert_eq!(b.len(), a.len());
        let (c, corrupt) = Archive::open(path, 3).unwrap();
        assert_eq!(corrupt, 0);
        assert!(c.len() <= 3);
        let _ = std::fs::remove_dir_all(&d);
    }
}
