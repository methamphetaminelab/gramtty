#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Entity {
    pub offset: usize,
    pub length: usize,
    pub kind: EntityKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SendState {
    Sent,
    Pending,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    pub pinned: bool,
    pub mention: bool,
    pub unread_reaction: bool,
    pub reactions: Vec<Reaction>,
    pub keyboard: Option<crate::bots::Keyboard>,
    pub forward: Option<Origin>,
    pub topic: Option<i32>,
    pub ttl: i32,
    pub deleted: bool,
    pub meta: MsgMeta,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MsgMeta {
    pub edit_date: i64,
    pub forward_date: i64,
    pub views: i32,
    pub forwards: i32,
    pub replies: i32,
    pub can_be_saved: bool,
    pub channel_post: bool,
    pub reply_chat: i64,
    pub via_bot: i64,
    pub signature: String,
    pub story: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    Sender(Sender),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Reaction {
    pub label: String,
    pub emoji: Option<String>,
    pub custom: i64,
    pub count: i32,
    pub chosen: bool,
}

pub fn reaction_chip(label: Option<&str>) -> String {
    label
        .and_then(crate::format::emoji_cell)
        .unwrap_or_else(|| crate::format::EMOJI_PLACEHOLDER.to_owned())
}

pub fn reactions_text(
    reactions: &[Reaction],
    custom: &std::collections::HashMap<i64, String>,
) -> String {
    reactions
        .iter()
        .map(|r| {
            let label = if r.custom == 0 {
                Some(r.label.as_str())
            } else {
                custom.get(&r.custom).map(String::as_str)
            };
            format!(
                "{} {}{}",
                reaction_chip(label),
                r.count,
                if r.chosen { "•" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("  ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileTarget {
    User(i64),
    Chat(i64),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub title: String,
    pub kind: String,
    pub id: i64,
    pub user_id: Option<i64>,
    pub usernames: Vec<String>,
    pub phone: String,
    pub bio: String,
    pub common_chats: Option<i32>,
    pub members: Option<i32>,
    pub links: Vec<String>,
    pub commands: Vec<crate::bots::Command>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserMeta {
    pub status: crate::presence::Status,
    pub bot: bool,
    pub usernames: Vec<String>,
    pub photo: Option<FileRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxItem {
    pub chat_id: i64,
    pub msg_id: i64,
    pub sender: Sender,
    pub preview: String,
    pub date: i64,
    pub mention: bool,
    pub topic: Option<i32>,
}

pub fn preview(m: &Msg) -> String {
    let text = crate::format::one_line(&m.body.text);
    match (&m.media, text.is_empty()) {
        (Some(media), true) => crate::format::one_line(media),
        (Some(media), false) => format!("{} {text}", crate::format::one_line(media)),
        (None, _) => text,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Visual {
    pub file: FileRef,
    pub decode: Decode,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    pub older_since: Option<std::time::Instant>,
    pub older_retry_at: Option<std::time::Instant>,
    pub detached: bool,
    pub loading_newer: bool,
    pub epoch: u64,
    pub topic: Option<i32>,
    pub general: bool,
}

pub const PRELOAD_MARGIN: usize = 8;
pub const OLDER_INDICATOR_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
pub const OLDER_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);
pub const MAX_WINDOW: usize = 1500;

impl History {
    pub fn for_topic(topic: Option<i32>, general: bool) -> Self {
        Self {
            topic,
            general,
            ..Self::default()
        }
    }

    pub fn accepts(&self, msg: &Msg) -> bool {
        topic_matches(msg.topic, self.topic, self.general)
    }

    pub fn send_topic(&self) -> Option<i32> {
        self.topic.filter(|_| !self.general)
    }

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

    pub fn start_older(&mut self) {
        self.loading_older = true;
        self.older_since = Some(std::time::Instant::now());
    }

    pub fn finish_older(&mut self) {
        self.loading_older = false;
        self.older_since = None;
    }

    pub fn older_failed(&mut self, now: std::time::Instant) {
        self.finish_older();
        self.older_retry_at = Some(now + OLDER_RETRY_AFTER);
    }

    pub fn can_load_older(&self) -> bool {
        !self.reached_start
            && !self.loading_older
            && self
                .older_retry_at
                .is_none_or(|t| std::time::Instant::now() >= t)
    }

    pub fn older_indicator(&self, now: std::time::Instant) -> bool {
        self.loading_older
            && !self.reached_start
            && self
                .older_since
                .is_some_and(|t| now.saturating_duration_since(t) >= OLDER_INDICATOR_DELAY)
    }

    pub fn merge_older(&mut self, batch: Vec<Msg>) -> usize {
        self.finish_older();
        self.older_retry_at = None;
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
        if !self.can_load_older() {
            return false;
        }
        match selected {
            Some(i) => i < PRELOAD_MARGIN,
            None => self.msgs.is_empty(),
        }
    }

    pub fn needs_newer(&self, selected: Option<usize>) -> bool {
        if !self.detached || self.loading_newer {
            return false;
        }
        selected.is_none_or(|i| i + PRELOAD_MARGIN >= self.msgs.len())
    }

    pub fn merge_newer(&mut self, batch: Vec<Msg>, latest: Option<i64>) -> usize {
        self.loading_newer = false;
        let before = self.msgs.len();
        let reaches_latest = latest.is_some_and(|l| batch.iter().any(|m| m.id >= l));
        for msg in batch {
            self.upsert(msg);
        }
        let added = self.msgs.len() - before;
        if added == 0 || reaches_latest {
            self.detached = false;
        }
        added
    }

    pub fn place_window(&mut self, mut batch: Vec<Msg>, latest: Option<i64>) -> bool {
        batch.sort_by_key(|m| m.id);
        batch.dedup_by_key(|m| m.id);
        let (Some(lo), Some(hi)) = (batch.first().map(|m| m.id), batch.last().map(|m| m.id)) else {
            return false;
        };
        let overlaps = matches!(
            (self.oldest_id(), self.newest_id()),
            (Some(o), Some(n)) if lo <= n && hi >= o
        );
        if overlaps {
            for msg in batch {
                self.upsert(msg);
            }
            return false;
        }
        self.detached = !latest.is_some_and(|l| hi >= l);
        self.msgs = batch;
        self.reached_start = false;
        self.finish_older();
        self.older_retry_at = None;
        self.loading_newer = false;
        self.epoch += 1;
        true
    }

    pub fn reset_to_latest(&mut self) {
        self.msgs.clear();
        self.reached_start = false;
        self.start_older();
        self.older_retry_at = None;
        self.loading_newer = false;
        self.detached = false;
        self.epoch += 1;
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
    pub reactions: i32,
    pub last_read_inbox: i64,
    pub last_read_outbox: i64,
    pub photo: Option<FileRef>,
    pub muted: bool,
    pub use_default_mute: bool,
    pub positions: Vec<Position>,
    pub last_message: Option<Msg>,
    pub group_id: i64,
    pub secret_id: i32,
    pub forum: bool,
    pub auto_delete: i32,
    pub perms: Rights,
    pub protected: bool,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rights {
    pub manage_topics: bool,
    pub create_topics: bool,
    pub pin: bool,
    pub change_info: bool,
}

impl Rights {
    pub const ALL: Rights = Rights {
        manage_topics: true,
        create_topics: true,
        pin: true,
        change_info: true,
    };

    pub fn can_create_topic(self) -> bool {
        self.create_topics || self.manage_topics
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Membership {
    Creator,
    Admin(Rights),
    #[default]
    Member,
    Restricted(Rights),
    Left,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupMeta {
    pub forum: bool,
    pub status: Membership,
}

pub fn effective_rights(status: Membership, chat: Rights) -> Rights {
    match status {
        Membership::Creator => Rights::ALL,
        Membership::Admin(r) => Rights {
            manage_topics: r.manage_topics,
            create_topics: r.manage_topics || chat.create_topics,
            pin: r.pin || chat.pin,
            change_info: r.change_info || chat.change_info,
        },
        Membership::Member => Rights {
            manage_topics: false,
            ..chat
        },
        Membership::Restricted(r) => Rights {
            manage_topics: false,
            create_topics: r.create_topics && chat.create_topics,
            pin: r.pin && chat.pin,
            change_info: r.change_info && chat.change_info,
        },
        Membership::Left => Rights::default(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretState {
    Pending,
    Ready,
    Closed,
}

impl SecretState {
    pub fn label(self) -> &'static str {
        match self {
            SecretState::Pending => "waiting for the other side to come online",
            SecretState::Ready => "end-to-end encrypted, this device only",
            SecretState::Closed => "closed",
        }
    }

    pub fn header(self) -> &'static str {
        match self {
            SecretState::Pending => "secret chat: waiting",
            SecretState::Ready => "secret chat",
            SecretState::Closed => "secret chat: closed",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            SecretState::Pending => "waiting",
            SecretState::Ready => "",
            SecretState::Closed => "closed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretChat {
    pub id: i32,
    pub user_id: i64,
    pub state: SecretState,
    pub outbound: bool,
}

pub const SECRET_TTL_CHOICES: [i32; 7] = [0, 5, 30, 60, 3600, 86400, 604800];
pub const CHAT_TTL_CHOICES: [i32; 4] = [0, 86400, 604800, 2678400];

pub fn ttl_choices(secret: bool) -> Vec<i32> {
    if secret {
        SECRET_TTL_CHOICES.to_vec()
    } else {
        CHAT_TTL_CHOICES.to_vec()
    }
}

pub fn can_set_ttl(kind: &ChatKind, rights: Rights) -> bool {
    match kind {
        ChatKind::Private(_) | ChatKind::Secret(_) => true,
        _ => rights.change_info,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topic {
    pub id: i32,
    pub name: String,
    pub icon: String,
    pub color: i32,
    pub general: bool,
    pub closed: bool,
    pub hidden: bool,
    pub outgoing: bool,
    pub pinned: bool,
    pub order: i64,
    pub unread: i32,
    pub mentions: i32,
    pub reactions: i32,
    pub last_read_inbox: i64,
    pub last_read_outbox: i64,
    pub muted: bool,
    pub use_default_mute: bool,
    pub last_message: Option<Msg>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TopicPerms {
    pub pin: bool,
    pub close: bool,
    pub hide: bool,
}

pub fn topic_perms(t: &Topic, r: Rights) -> TopicPerms {
    TopicPerms {
        pin: r.manage_topics,
        close: !t.general && (r.manage_topics || t.outgoing),
        hide: t.general && r.manage_topics,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TopicCursor {
    pub date: i32,
    pub message_id: i64,
    pub topic_id: i32,
}

#[derive(Clone, Debug, Default)]
pub struct TopicList {
    topics: Vec<Topic>,
    pub next: Option<TopicCursor>,
    pub loading: bool,
    pub loaded: bool,
    pub total: i32,
}

pub const TOPIC_PAGE: i32 = 50;
pub const TOPIC_PRELOAD: usize = 5;

impl TopicList {
    pub fn topics(&self) -> &[Topic] {
        &self.topics
    }

    pub fn get(&self, id: i32) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: i32) -> Option<&mut Topic> {
        self.topics.iter_mut().find(|t| t.id == id)
    }

    pub fn merge_page(&mut self, page: Vec<Topic>, next: Option<TopicCursor>, total: i32) {
        self.loading = false;
        self.loaded = true;
        self.total = total;
        let empty = page.is_empty();
        for t in page {
            self.put(t);
        }
        self.next = if empty { None } else { next };
        self.sort();
    }

    pub fn upsert(&mut self, t: Topic) {
        self.put(t);
        self.sort();
    }

    fn put(&mut self, t: Topic) {
        match self.topics.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t,
            None => self.topics.push(t),
        }
    }

    pub fn apply_info(&mut self, info: Topic) -> bool {
        let Some(t) = self.get_mut(info.id) else {
            return false;
        };
        t.name = info.name;
        t.icon = info.icon;
        t.color = info.color;
        t.general = info.general;
        t.closed = info.closed;
        t.hidden = info.hidden;
        t.outgoing = info.outgoing;
        self.sort();
        true
    }

    fn sort(&mut self) {
        self.topics.sort_by(|a, b| {
            let rank = |t: &Topic| (!(t.general && t.hidden), t.pinned, t.order, t.id);
            rank(b).cmp(&rank(a))
        });
    }

    pub fn needs_more(&self, sel: Option<usize>) -> bool {
        !self.loading
            && self.next.is_some()
            && sel.is_some_and(|i| i + TOPIC_PRELOAD >= self.topics.len())
    }

    pub fn note_message(&mut self, msg: &Msg) -> bool {
        let top = self
            .topics
            .iter()
            .filter(|t| !t.pinned)
            .map(|t| t.order)
            .max()
            .unwrap_or(0);
        let Some(t) = msg.topic.and_then(|id| self.get_mut(id)) else {
            return false;
        };
        let newer = t.last_message.as_ref().is_none_or(|m| m.id < msg.id);
        if !newer {
            return true;
        }
        t.last_message = Some(msg.clone());
        if !msg.outgoing && msg.id > t.last_read_inbox {
            t.unread += 1;
        }
        if !t.pinned && t.order < top {
            t.order = top + 1;
        }
        self.sort();
        true
    }

    pub fn mark_read_up_to(&mut self, id: i32, last_read: i64) {
        if let Some(t) = self.get_mut(id) {
            t.last_read_inbox = t.last_read_inbox.max(last_read);
            if t.last_message
                .as_ref()
                .is_some_and(|m| m.id <= t.last_read_inbox)
            {
                t.unread = 0;
            }
        }
    }
}

pub fn topic_matches(msg_topic: Option<i32>, view: Option<i32>, general: bool) -> bool {
    match view {
        None => true,
        Some(v) => msg_topic == Some(v) || (general && msg_topic.is_none()),
    }
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
            pinned: false,
            mention: false,
            unread_reaction: false,
            reactions: Vec::new(),
            keyboard: None,
            forward: None,
            topic: None,
            ttl: 0,
            deleted: false,
            meta: Default::default(),
        }
    }

    fn ids(h: &History) -> Vec<i64> {
        h.msgs().iter().map(|m| m.id).collect()
    }

    #[test]
    fn far_window_replaces_and_detaches_then_newer_pages_reattach() {
        let mut h = History::default();
        h.merge_older((900..=950).rev().map(msg).collect());
        let epoch = h.epoch;
        assert!(h.place_window((90..=130).map(msg).collect(), Some(950)));
        assert_eq!(ids(&h).first(), Some(&90));
        assert_eq!(ids(&h).last(), Some(&130));
        assert!(h.detached);
        assert!(!h.reached_start);
        assert_ne!(
            h.epoch, epoch,
            "older pages for the old window are now stale"
        );
        assert!(h.needs_newer(None));
        assert!(h.needs_newer(Some(h.len() - 1)));
        assert!(!h.needs_newer(Some(0)));
        h.loading_newer = true;
        assert!(!h.needs_newer(None));
        assert_eq!(
            h.merge_newer((130..=180).rev().map(msg).collect(), Some(950)),
            50
        );
        assert!(h.detached);
        assert!(!h.loading_newer);
        assert_eq!(h.merge_newer((180..=230).map(msg).collect(), Some(950)), 50);
        assert_eq!(h.merge_newer(vec![msg(940), msg(950)], Some(950)), 2);
        assert!(!h.detached, "the latest message is in the window again");
        assert!(!h.needs_newer(None));
    }

    #[test]
    fn overlapping_window_extends_instead_of_replacing() {
        let mut h = History::default();
        h.merge_older((100..=150).map(msg).collect());
        let epoch = h.epoch;
        assert!(!h.place_window((80..=110).map(msg).collect(), Some(150)));
        assert_eq!(ids(&h).first(), Some(&80));
        assert_eq!(h.len(), 71);
        assert!(!h.detached);
        assert_eq!(h.epoch, epoch);
        assert!(!h.place_window(Vec::new(), Some(150)));
    }

    #[test]
    fn empty_newer_page_ends_the_gap_and_reset_starts_over() {
        let mut h = History::default();
        h.place_window(vec![msg(5), msg(6)], None);
        assert!(h.detached);
        assert_eq!(h.merge_newer(vec![msg(6)], None), 0);
        assert!(!h.detached);
        let epoch = h.epoch;
        h.reset_to_latest();
        assert!(h.is_empty());
        assert!(h.loading_older);
        assert!(!h.detached);
        assert_eq!(h.epoch, epoch + 1);
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
    fn older_indicator_waits_before_showing() {
        let mut h = History::default();
        h.merge_older((1..=50).map(msg).collect());
        let t0 = std::time::Instant::now();
        assert!(!h.older_indicator(t0));
        h.start_older();
        let started = h.older_since.unwrap();
        assert!(!h.older_indicator(started));
        assert!(!h.older_indicator(started + std::time::Duration::from_millis(299)));
        assert!(h.older_indicator(started + OLDER_INDICATOR_DELAY));
        h.merge_older((60..=70).map(msg).collect());
        assert!(!h.older_indicator(started + std::time::Duration::from_secs(5)));
        h.start_older();
        h.merge_older(Vec::new());
        assert!(h.reached_start);
        h.loading_older = true;
        h.older_since = Some(started);
        assert!(!h.older_indicator(started + std::time::Duration::from_secs(5)));
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
            reactions: 0,
            last_read_inbox: 0,
            last_read_outbox: 0,
            photo: None,
            muted: false,
            use_default_mute: false,
            positions: vec![Position {
                list: ListId::Main,
                order,
                pinned: false,
            }],
            last_message: None,
            group_id: 0,
            secret_id: 0,
            forum: false,
            auto_delete: 0,
            perms: Rights::default(),
            protected: false,
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
    fn reactions_and_preview_text() {
        let r = |label: &str, count: i32, chosen: bool| Reaction {
            label: label.to_owned(),
            emoji: Some(label.to_owned()),
            custom: 0,
            count,
            chosen,
        };
        let custom_emoji = |id: i64, count: i32| Reaction {
            label: "•".to_owned(),
            emoji: None,
            custom: id,
            count,
            chosen: false,
        };
        let none = std::collections::HashMap::new();
        assert_eq!(
            reactions_text(
                &[r("❤", 3, false), r("👍", 1, true), r("🔥", 1, false)],
                &none
            ),
            "❤\u{FE0F} 3  👍 1•  🔥 1"
        );
        assert_eq!(reactions_text(&[custom_emoji(7, 1)], &none), "• 1");
        let known = std::collections::HashMap::from([(7, "🎉".to_owned())]);
        assert_eq!(
            reactions_text(&[custom_emoji(7, 2), r("❤", 1, false)], &known),
            "🎉 2  ❤\u{FE0F} 1"
        );
        let blank =
            std::collections::HashMap::from([(7, String::new()), (8, "\u{E000}".to_owned())]);
        assert_eq!(
            reactions_text(&[custom_emoji(7, 1), custom_emoji(8, 2)], &blank),
            "• 1  • 2"
        );
        assert_eq!(reactions_text(&[], &none), "");
        let mut m = msg(1);
        m.body = RichText::plain("hi\nthere");
        assert_eq!(preview(&m), "hi there");
        m.media = Some("[photo]".to_owned());
        assert_eq!(preview(&m), "[photo] hi there");
        m.body = RichText::default();
        assert_eq!(preview(&m), "[photo]");
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

    fn topic(id: i32, order: i64) -> Topic {
        Topic {
            id,
            name: format!("t{id}"),
            order,
            ..Topic::default()
        }
    }

    fn topic_ids(l: &TopicList) -> Vec<i32> {
        l.topics().iter().map(|t| t.id).collect()
    }

    #[test]
    fn topics_sort_pinned_first_then_order_and_hidden_general_last() {
        let mut l = TopicList::default();
        let mut general = topic(1, 900);
        general.general = true;
        general.hidden = true;
        let mut pinned = topic(4, 10);
        pinned.pinned = true;
        let cursor = TopicCursor {
            date: 5,
            message_id: 50,
            topic_id: 3,
        };
        l.merge_page(
            vec![topic(2, 300), general, topic(3, 500), pinned],
            Some(cursor),
            9,
        );
        assert_eq!(topic_ids(&l), vec![4, 3, 2, 1]);
        assert!(l.loaded && !l.loading);
        assert_eq!(l.next, Some(cursor));
        assert_eq!(l.total, 9);
        l.get_mut(1).unwrap().hidden = false;
        l.upsert(l.get(1).unwrap().clone());
        assert_eq!(topic_ids(&l), vec![4, 1, 3, 2]);
    }

    #[test]
    fn topic_pages_dedupe_and_stop_on_empty_page() {
        let mut l = TopicList::default();
        let c1 = TopicCursor {
            date: 1,
            message_id: 1,
            topic_id: 2,
        };
        l.merge_page(vec![topic(1, 30), topic(2, 20)], Some(c1), 4);
        assert!(!l.needs_more(None), "nothing selected in the topic list");
        assert!(
            l.needs_more(Some(0)),
            "selection is near the end of the page"
        );
        l.loading = true;
        assert!(!l.needs_more(Some(1)), "already loading");
        l.merge_page(vec![topic(2, 20), topic(3, 10)], Some(c1), 4);
        assert_eq!(topic_ids(&l), vec![1, 2, 3]);
        l.merge_page(Vec::new(), Some(c1), 4);
        assert_eq!(l.next, None);
        assert!(!l.needs_more(Some(2)));
    }

    #[test]
    fn new_topic_messages_bump_order_count_unread_and_reads_clear_it() {
        let mut l = TopicList::default();
        let mut pinned = topic(9, 1);
        pinned.pinned = true;
        l.merge_page(vec![topic(1, 30), topic(2, 20), pinned], None, 3);
        let mut m = msg(100);
        m.topic = Some(2);
        assert!(l.note_message(&m));
        assert_eq!(
            topic_ids(&l),
            vec![9, 2, 1],
            "pinned stays first, active topic goes up"
        );
        assert_eq!(l.get(2).unwrap().unread, 1);
        assert_eq!(
            l.get(2).unwrap().last_message.as_ref().map(|m| m.id),
            Some(100)
        );
        assert!(l.note_message(&m), "duplicate update is ignored");
        assert_eq!(l.get(2).unwrap().unread, 1);
        let mut own = msg(101);
        own.topic = Some(2);
        own.outgoing = true;
        l.note_message(&own);
        assert_eq!(l.get(2).unwrap().unread, 1);
        let mut unknown = msg(102);
        unknown.topic = Some(77);
        assert!(!l.note_message(&unknown));
        l.mark_read_up_to(2, 100);
        assert_eq!(
            l.get(2).unwrap().unread,
            1,
            "101 is newer than the read mark"
        );
        l.mark_read_up_to(2, 101);
        assert_eq!(l.get(2).unwrap().unread, 0);
        let mut info = topic(1, 0);
        info.name = "Renamed".to_owned();
        info.closed = true;
        assert!(l.apply_info(info));
        assert_eq!(l.get(1).unwrap().name, "Renamed");
        assert_eq!(
            l.get(1).unwrap().order,
            30,
            "info updates keep counters and order"
        );
        assert!(!l.apply_info(topic(55, 0)));
    }

    #[test]
    fn topic_views_filter_messages_and_general_sends_without_topic() {
        assert!(topic_matches(Some(3), None, false));
        assert!(topic_matches(None, None, false));
        assert!(topic_matches(Some(3), Some(3), false));
        assert!(!topic_matches(Some(4), Some(3), false));
        assert!(!topic_matches(None, Some(3), false));
        assert!(topic_matches(None, Some(1), true));
        let h = History::for_topic(Some(1), true);
        assert_eq!(h.send_topic(), None);
        let h = History::for_topic(Some(5), false);
        assert_eq!(h.send_topic(), Some(5));
        let mut m = msg(1);
        m.topic = Some(5);
        assert!(h.accepts(&m));
        m.topic = Some(6);
        assert!(!h.accepts(&m));
        assert_eq!(History::default().send_topic(), None);
    }

    #[test]
    fn topic_actions_are_gated_by_rights() {
        let member = effective_rights(
            Membership::Member,
            Rights {
                create_topics: true,
                pin: true,
                ..Rights::default()
            },
        );
        assert!(!member.manage_topics && member.can_create_topic());
        let admin = effective_rights(
            Membership::Admin(Rights {
                manage_topics: true,
                ..Rights::default()
            }),
            Rights::default(),
        );
        assert!(admin.manage_topics && admin.can_create_topic());
        assert_eq!(
            effective_rights(Membership::Creator, Rights::default()),
            Rights::ALL
        );
        assert_eq!(
            effective_rights(Membership::Left, Rights::ALL),
            Rights::default()
        );
        let restricted = effective_rights(
            Membership::Restricted(Rights::default()),
            Rights {
                create_topics: true,
                ..Rights::default()
            },
        );
        assert!(!restricted.can_create_topic());

        let mut t = topic(5, 1);
        assert_eq!(topic_perms(&t, member), TopicPerms::default());
        t.outgoing = true;
        assert_eq!(
            topic_perms(&t, member),
            TopicPerms {
                close: true,
                ..TopicPerms::default()
            },
            "the creator of a topic may close it"
        );
        assert_eq!(
            topic_perms(&t, admin),
            TopicPerms {
                pin: true,
                close: true,
                hide: false
            }
        );
        let mut general = topic(1, 1);
        general.general = true;
        assert_eq!(
            topic_perms(&general, admin),
            TopicPerms {
                pin: true,
                close: false,
                hide: true
            }
        );
        assert!(can_set_ttl(&ChatKind::Secret(2), Rights::default()));
        assert!(can_set_ttl(&ChatKind::Private(2), Rights::default()));
        assert!(!can_set_ttl(&ChatKind::Supergroup, member));
        assert!(can_set_ttl(&ChatKind::Supergroup, Rights::ALL));
        assert_eq!(ttl_choices(true)[1], 5);
        assert!(ttl_choices(false).iter().all(|v| v % 86_400 == 0));
    }

    #[test]
    fn secret_state_labels() {
        assert_eq!(SecretState::Ready.short(), "");
        assert_eq!(SecretState::Pending.short(), "waiting");
        assert_eq!(SecretState::Closed.short(), "closed");
        assert!(SecretState::Ready.label().contains("this device"));
    }
}
