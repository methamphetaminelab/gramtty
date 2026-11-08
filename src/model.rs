#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    Pre,
    Link(Option<String>),
    Mention,
    Quote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub offset: usize,
    pub length: usize,
    pub kind: EntityKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub entities: Vec<Entity>,
}

impl RichText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            entities: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sender {
    User(i64),
    Chat(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState {
    Sent,
    Pending,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub id: i64,
    pub chat_id: i64,
    pub sender: Sender,
    pub date: i64,
    pub edited: bool,
    pub outgoing: bool,
    pub state: SendState,
    pub reply_to: Option<i64>,
    pub album: i64,
    pub media: Option<String>,
    pub attachments: Vec<Attachment>,
    pub body: RichText,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRef {
    pub id: i32,
    pub size: i64,
    pub path: String,
    pub done: bool,
    pub active: bool,
    pub downloaded: i64,
    pub uploading: bool,
    pub uploaded: i64,
}

impl FileRef {
    pub fn local(&self) -> Option<&str> {
        (self.done && !self.path.is_empty()).then_some(self.path.as_str())
    }

    pub fn percent(&self) -> Option<u8> {
        if self.done || self.size <= 0 || !self.active {
            return None;
        }
        Some((self.downloaded.clamp(0, self.size) * 100 / self.size) as u8)
    }

    pub fn upload_percent(&self) -> Option<u8> {
        if !self.uploading || self.size <= 0 {
            return None;
        }
        Some((self.uploaded.clamp(0, self.size) * 100 / self.size) as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decode {
    Raster,
    Tgs,
    Video,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visual {
    pub file: FileRef,
    pub decode: Decode,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Video,
    Animation,
    VideoNote,
    Voice,
    Audio,
    Sticker,
    Document,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub kind: MediaKind,
    pub file: FileRef,
    pub preview: Option<Visual>,
    pub full: Option<Visual>,
    pub mini: Option<std::sync::Arc<[u8]>>,
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    pub name: String,
    pub mime: String,
    pub waveform: Vec<u8>,
}

impl Attachment {
    pub fn new(kind: MediaKind, file: FileRef) -> Self {
        Self {
            kind,
            file,
            preview: None,
            full: None,
            mini: None,
            width: 0,
            height: 0,
            duration: 0,
            name: String::new(),
            mime: String::new(),
            waveform: Vec::new(),
        }
    }

    pub fn is_visual(&self) -> bool {
        self.preview.is_some() || self.mini.is_some()
    }

    pub fn viewable(&self) -> bool {
        self.full.is_some() || self.preview.is_some() || self.mini.is_some()
    }
}

pub fn groups(msgs: &[Msg]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, m) in msgs.iter().enumerate() {
        if let Some(last) = out.last_mut() {
            let prev = &msgs[last.end - 1];
            if m.album != 0 && prev.album == m.album && prev.sender == m.sender {
                last.end = i + 1;
                continue;
            }
        }
        out.push(i..i + 1);
    }
    out
}

pub fn group_of(groups: &[std::ops::Range<usize>], index: usize) -> usize {
    groups
        .partition_point(|g| g.end <= index)
        .min(groups.len().saturating_sub(1))
}

pub fn decode_waveform(bytes: &[u8]) -> Vec<u8> {
    let count = bytes.len() * 8 / 5;
    (0..count)
        .map(|i| {
            let bit = i * 5;
            let lo = u16::from(bytes[bit / 8]);
            let hi = u16::from(bytes.get(bit / 8 + 1).copied().unwrap_or(0));
            (((lo | (hi << 8)) >> (bit % 8)) & 31) as u8
        })
        .collect()
}

pub fn waveform_bars(values: &[u8], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let peak = values.iter().copied().max().unwrap_or(0).max(1) as usize;
    (0..width)
        .map(|col| {
            let from = col * values.len() / width;
            let to = ((col + 1) * values.len() / width).max(from + 1);
            let v = values[from..to.min(values.len())]
                .iter()
                .copied()
                .max()
                .unwrap_or(0) as usize;
            BARS[(v * (BARS.len() - 1) + peak / 2) / peak]
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct History {
    msgs: Vec<Msg>,
    pub reached_start: bool,
    pub loading_older: bool,
}

pub const PRELOAD_MARGIN: usize = 8;
pub const MAX_WINDOW: usize = 1500;

impl History {
    pub fn msgs(&self) -> &[Msg] {
        &self.msgs
    }

    pub fn len(&self) -> usize {
        self.msgs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.msgs.is_empty()
    }

    pub fn oldest_id(&self) -> Option<i64> {
        self.msgs.first().map(|m| m.id)
    }

    pub fn newest_id(&self) -> Option<i64> {
        self.msgs.last().map(|m| m.id)
    }

    pub fn index_of(&self, id: i64) -> Option<usize> {
        self.msgs.binary_search_by_key(&id, |m| m.id).ok()
    }

    pub fn get(&self, id: i64) -> Option<&Msg> {
        self.index_of(id).map(|i| &self.msgs[i])
    }

    pub fn get_mut(&mut self, id: i64) -> Option<&mut Msg> {
        self.index_of(id).map(|i| &mut self.msgs[i])
    }

    pub fn merge_older(&mut self, batch: Vec<Msg>) -> usize {
        self.loading_older = false;
        let before = self.msgs.len();
        if batch.is_empty() {
            self.reached_start = true;
            return 0;
        }
        for msg in batch {
            self.upsert(msg);
        }
        self.msgs.len() - before
    }

    pub fn upsert(&mut self, msg: Msg) -> bool {
        match self.msgs.binary_search_by_key(&msg.id, |m| m.id) {
            Ok(i) => {
                self.msgs[i] = msg;
                false
            }
            Err(i) => {
                self.msgs.insert(i, msg);
                true
            }
        }
    }

    pub fn replace_id(&mut self, old_id: i64, msg: Msg) {
        if let Some(i) = self.index_of(old_id) {
            self.msgs.remove(i);
        }
        self.upsert(msg);
    }

    pub fn remove(&mut self, ids: &[i64]) -> Vec<usize> {
        let mut removed: Vec<usize> = ids.iter().filter_map(|id| self.index_of(*id)).collect();
        removed.sort_unstable();
        for i in removed.iter().rev() {
            self.msgs.remove(*i);
        }
        removed
    }

    pub fn needs_older(&self, selected: Option<usize>) -> bool {
        if self.reached_start || self.loading_older {
            return false;
        }
        match selected {
            Some(i) => i < PRELOAD_MARGIN,
            None => self.msgs.is_empty(),
        }
    }

    pub fn trim_oldest(&mut self, keep: usize) -> usize {
        if self.msgs.len() <= keep {
            return 0;
        }
        let drop = self.msgs.len() - keep;
        self.msgs.drain(..drop);
        self.reached_start = false;
        drop
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListId {
    Main,
    Archive,
    Folder(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub list: ListId,
    pub order: i64,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatKind {
    Private(i64),
    Group,
    Supergroup,
    Channel,
    Secret(i64),
}

#[derive(Clone, Debug)]
pub struct ChatInfo {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    pub unread: i32,
    pub mentions: i32,
    pub last_read_inbox: i64,
    pub muted: bool,
    pub positions: Vec<Position>,
    pub last_message: Option<Msg>,
}

impl ChatInfo {
    pub fn set_position(&mut self, pos: Position) {
        self.positions.retain(|p| p.list != pos.list);
        if pos.order != 0 {
            self.positions.push(pos);
        }
    }

    pub fn position_in(&self, list: ListId) -> Option<&Position> {
        self.positions.iter().find(|p| p.list == list)
    }
}

pub fn sorted_chat_ids<'a>(chats: impl Iterator<Item = &'a ChatInfo>, list: ListId) -> Vec<i64> {
    let mut rows: Vec<(i64, i64)> = chats
        .filter_map(|c| c.position_in(list).map(|p| (p.order, c.id)))
        .collect();
    rows.sort_unstable_by(|a, b| b.cmp(a));
    rows.into_iter().map(|(_, id)| id).collect()
}

pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    let hay: String = c.iter().collect();
    let needle: String = q.iter().collect();
    if let Some(pos) = hay.find(&needle) {
        let bonus = if pos == 0 { 200 } else { 100 };
        return Some(bonus - hay[..pos].chars().count() as i32);
    }
    let mut qi = 0;
    let mut gaps = 0;
    let mut last: Option<usize> = None;
    for (i, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            if let Some(l) = last {
                gaps += (i - l - 1) as i32;
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(-gaps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: i64) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: Sender::User(7),
            date: id,
            edited: false,
            outgoing: false,
            state: SendState::Sent,
            reply_to: None,
            album: 0,
            media: None,
            attachments: Vec::new(),
            body: RichText::plain(format!("m{id}")),
        }
    }

    fn ids(h: &History) -> Vec<i64> {
        h.msgs().iter().map(|m| m.id).collect()
    }

    #[test]
    fn merge_older_sorts_and_dedupes() {
        let mut h = History::default();
        h.merge_older(vec![msg(30), msg(20), msg(10)]);
        assert_eq!(ids(&h), vec![10, 20, 30]);
        let added = h.merge_older(vec![msg(10), msg(5), msg(1)]);
        assert_eq!(added, 2);
        assert_eq!(ids(&h), vec![1, 5, 10, 20, 30]);
        assert!(!h.reached_start);
        h.merge_older(Vec::new());
        assert!(h.reached_start);
    }

    #[test]
    fn upsert_and_replace_pending() {
        let mut h = History::default();
        h.upsert(msg(10));
        h.upsert(msg(1_000_000));
        assert_eq!(h.newest_id(), Some(1_000_000));
        let mut sent = msg(11);
        sent.body = RichText::plain("sent");
        h.replace_id(1_000_000, sent);
        assert_eq!(ids(&h), vec![10, 11]);
        assert_eq!(h.get(11).unwrap().body.text, "sent");
        assert!(!h.upsert(msg(10)));
    }

    #[test]
    fn remove_reports_indices() {
        let mut h = History::default();
        h.merge_older((1..=5).map(msg).collect());
        assert_eq!(h.remove(&[4, 2, 99]), vec![1, 3]);
        assert_eq!(ids(&h), vec![1, 3, 5]);
    }

    #[test]
    fn needs_older_near_top_only() {
        let mut h = History::default();
        assert!(h.needs_older(None));
        h.merge_older((1..=50).map(msg).collect());
        assert!(!h.needs_older(None));
        assert!(!h.needs_older(Some(40)));
        assert!(h.needs_older(Some(3)));
        h.loading_older = true;
        assert!(!h.needs_older(Some(3)));
        h.loading_older = false;
        h.reached_start = true;
        assert!(!h.needs_older(Some(0)));
    }

    #[test]
    fn trim_keeps_newest() {
        let mut h = History::default();
        h.merge_older((1..=10).map(msg).collect());
        h.reached_start = true;
        assert_eq!(h.trim_oldest(4), 6);
        assert_eq!(ids(&h), vec![7, 8, 9, 10]);
        assert!(!h.reached_start);
        assert_eq!(h.trim_oldest(10), 0);
    }

    #[test]
    fn chat_order_follows_positions() {
        let mk = |id: i64, order: i64| ChatInfo {
            id,
            title: format!("c{id}"),
            kind: ChatKind::Group,
            unread: 0,
            mentions: 0,
            last_read_inbox: 0,
            muted: false,
            positions: vec![Position {
                list: ListId::Main,
                order,
                pinned: false,
            }],
            last_message: None,
        };
        let mut chats = [mk(1, 10), mk(2, 30), mk(3, 20)];
        assert_eq!(sorted_chat_ids(chats.iter(), ListId::Main), vec![2, 3, 1]);
        chats[1].set_position(Position {
            list: ListId::Main,
            order: 0,
            pinned: false,
        });
        assert_eq!(sorted_chat_ids(chats.iter(), ListId::Main), vec![3, 1]);
        assert!(sorted_chat_ids(chats.iter(), ListId::Archive).is_empty());
    }

    #[test]
    fn albums_group_consecutive_same_sender() {
        let mut ms: Vec<Msg> = (1..=7).map(msg).collect();
        for m in &mut ms[1..4] {
            m.album = 99;
        }
        ms[4].album = 99;
        ms[4].sender = Sender::User(8);
        ms[5].album = 5;
        ms[6].album = 5;
        let g = groups(&ms);
        assert_eq!(g, vec![0..1, 1..4, 4..5, 5..7]);
        assert_eq!(group_of(&g, 0), 0);
        assert_eq!(group_of(&g, 2), 1);
        assert_eq!(group_of(&g, 3), 1);
        assert_eq!(group_of(&g, 6), 3);
        assert_eq!(group_of(&g, 50), 3);
        assert!(groups(&[]).is_empty());
    }

    #[test]
    fn waveform_unpacks_five_bit_values() {
        let values = [31u8, 0, 17, 5, 31, 2, 9, 30];
        let mut bits: u64 = 0;
        for (i, v) in values.iter().enumerate() {
            bits |= u64::from(*v) << (i * 5);
        }
        let bytes: Vec<u8> = bits.to_le_bytes()[..5].to_vec();
        assert_eq!(decode_waveform(&bytes), values.to_vec());
        let bars = waveform_bars(&values, 4);
        assert_eq!(bars.chars().count(), 4);
        assert_eq!(bars.chars().next(), Some('█'));
        assert_eq!(waveform_bars(&[], 10), "");
        assert_eq!(waveform_bars(&[0, 0], 3), "▁▁▁");
    }

    #[test]
    fn file_progress() {
        let mut f = FileRef {
            id: 1,
            size: 200,
            downloaded: 50,
            active: true,
            ..FileRef::default()
        };
        assert_eq!(f.percent(), Some(25));
        assert_eq!(f.local(), None);
        let done = FileRef {
            done: true,
            path: "/x".to_owned(),
            downloaded: 200,
            ..f.clone()
        };
        assert_eq!(done.local(), Some("/x"));
        assert_eq!(done.percent(), None);
        f.active = false;
        assert_eq!(f.percent(), None);
        assert_eq!(f.upload_percent(), None);
        f.uploading = true;
        f.uploaded = 150;
        assert_eq!(f.upload_percent(), Some(75));
    }

    #[test]
    fn fuzzy_prefers_prefix_and_contiguous() {
        assert!(
            fuzzy_score("dev", "Developers").unwrap() > fuzzy_score("dev", "Rust devs").unwrap()
        );
        assert!(fuzzy_score("rd", "Rust devs").is_some());
        assert!(fuzzy_score("xyz", "Rust devs").is_none());
        assert_eq!(fuzzy_score("", "any"), Some(0));
    }
}
