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
    pub media: Option<String>,
    pub body: RichText,
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
            media: None,
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
    fn fuzzy_prefers_prefix_and_contiguous() {
        assert!(
            fuzzy_score("dev", "Developers").unwrap() > fuzzy_score("dev", "Rust devs").unwrap()
        );
        assert!(fuzzy_score("rd", "Rust devs").is_some());
        assert!(fuzzy_score("xyz", "Rust devs").is_none());
        assert_eq!(fuzzy_score("", "any"), Some(0));
    }
}
