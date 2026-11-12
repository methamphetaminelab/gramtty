use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use unicode_width::UnicodeWidthStr;

use crate::input::TextInput;
use crate::model::Msg;
use crate::td::{Cursor, Peers};

pub const DEBOUNCE: Duration = Duration::from_millis(300);
pub const CHAT_LIMIT: usize = 50;
pub const PAGE_AHEAD: usize = 5;

const LAYOUT_RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
const LAYOUT_EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
const SERVER_ONLY: i32 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Messages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Recent,
    Chats,
    Global,
    Messages,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Recent => "Recent",
            Section::Chats => "Chats and contacts",
            Section::Global => "Global search",
            Section::Messages => "Messages",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Chat(i64),
    Contact(i64),
    Message(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Waiting,
    Done,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Header(Section, String),
    Hit(usize, Hit),
    Note(String, bool),
}

pub struct Progress<'a> {
    pub scope: Scope,
    pub query: &'a str,
    pub peers: Phase,
    pub peer_errors: &'a [String],
    pub msgs: Phase,
    pub total: i32,
    pub more: Phase,
}

pub struct FindView {
    pub scope: Scope,
    pub input: TextInput,
    pub sel: usize,
    pub due: Option<Instant>,
    pub sent: Option<String>,
    pub recent: Vec<i64>,
    pub peers_token: u64,
    pub peers: Option<Peers>,
    pub token: u64,
    pub msgs: Vec<Msg>,
    pub next: Option<Cursor>,
    pub total: i32,
    pub loading: bool,
    pub paging: bool,
    pub error: Option<String>,
}

impl FindView {
    pub fn new(scope: Scope) -> Self {
        FindView {
            scope,
            input: TextInput::default(),
            sel: 0,
            due: None,
            sent: None,
            recent: Vec::new(),
            peers_token: 0,
            peers: None,
            token: 0,
            msgs: Vec::new(),
            next: None,
            total: 0,
            loading: false,
            paging: false,
            error: None,
        }
    }

    pub fn query(&self) -> &str {
        self.input.text().trim()
    }

    pub fn progress(&self) -> Progress<'_> {
        let query = self.query();
        let pending = self.due.is_some() || self.sent.as_deref() != Some(query);
        let peers = if self.scope == Scope::Messages || query.is_empty() {
            Phase::Idle
        } else if pending || self.peers.is_none() {
            Phase::Waiting
        } else {
            Phase::Done
        };
        let msgs = if query.is_empty() {
            Phase::Idle
        } else if pending || (self.loading && !self.paging) {
            Phase::Waiting
        } else {
            match &self.error {
                Some(e) if self.msgs.is_empty() => Phase::Failed(e.clone()),
                _ => Phase::Done,
            }
        };
        let more = match (&self.error, self.loading && self.paging) {
            (_, true) => Phase::Waiting,
            (Some(e), false) if !self.msgs.is_empty() => Phase::Failed(e.clone()),
            _ if self.next.is_some() => Phase::Idle,
            _ => Phase::Done,
        };
        Progress {
            scope: self.scope,
            query,
            peers,
            peer_errors: self.peers.as_ref().map_or(&[], |p| &p.errors[..]),
            msgs,
            total: self.total,
            more,
        }
    }
}

pub fn layout(hits: &[(Section, Hit)], p: &Progress) -> Vec<Row> {
    let mut rows = Vec::new();
    let searching = Row::Note("Searching…".to_owned(), false);
    for section in [
        Section::Recent,
        Section::Chats,
        Section::Global,
        Section::Messages,
    ] {
        let items: Vec<Row> = hits
            .iter()
            .enumerate()
            .filter(|(_, (s, _))| *s == section)
            .map(|(i, (_, h))| Row::Hit(i, *h))
            .collect();
        match section {
            Section::Recent | Section::Chats => {
                if !items.is_empty() {
                    rows.push(Row::Header(section, String::new()));
                    rows.extend(items);
                }
            }
            Section::Global => {
                let errors = p.peer_errors.iter().map(|e| Row::Note(e.clone(), true));
                match p.peers {
                    Phase::Waiting => {
                        rows.push(Row::Header(section, String::new()));
                        rows.push(searching.clone());
                    }
                    Phase::Done if !items.is_empty() || !p.peer_errors.is_empty() => {
                        rows.push(Row::Header(section, String::new()));
                        rows.extend(items);
                        rows.extend(errors);
                    }
                    _ => {}
                }
            }
            Section::Messages => match &p.msgs {
                Phase::Waiting => {
                    rows.push(Row::Header(section, String::new()));
                    rows.push(searching.clone());
                }
                Phase::Failed(e) => {
                    rows.push(Row::Header(section, String::new()));
                    rows.push(Row::Note(e.clone(), true));
                }
                Phase::Done if !items.is_empty() => {
                    let total = p.total.max(items.len() as i32);
                    let found = if total == 1 {
                        "1 found".to_owned()
                    } else {
                        format!("{total} found")
                    };
                    rows.push(Row::Header(section, found));
                    rows.extend(items);
                    match &p.more {
                        Phase::Waiting => rows.push(Row::Note("Loading more…".to_owned(), false)),
                        Phase::Failed(e) => rows.push(Row::Note(e.clone(), true)),
                        _ => {}
                    }
                }
                _ => {}
            },
        }
    }
    if rows.is_empty() {
        let note = if !p.query.is_empty() {
            format!("No results for “{}”", p.query)
        } else if p.scope == Scope::Messages {
            "Type to search messages in all chats".to_owned()
        } else {
            "Type to search chats, people, channels and messages".to_owned()
        };
        rows.push(Row::Note(note, false));
    }
    rows
}

pub fn row_of(rows: &[Row], hit: usize) -> usize {
    rows.iter()
        .position(|r| matches!(r, Row::Hit(i, _) if *i == hit))
        .unwrap_or(0)
}

pub fn next_section(hits: &[(Section, Hit)], sel: usize, forward: bool) -> usize {
    let Some((current, _)) = hits.get(sel) else {
        return 0;
    };
    let starts: Vec<usize> = (0..hits.len())
        .filter(|&i| i == 0 || hits[i - 1].0 != hits[i].0)
        .collect();
    let at = starts
        .iter()
        .rposition(|&s| hits[s].0 == *current)
        .unwrap_or(0);
    if forward {
        starts.get(at + 1).copied().unwrap_or(starts[0])
    } else if starts[at] != sel {
        starts[at]
    } else {
        starts[(at + starts.len() - 1) % starts.len()]
    }
}

pub struct Candidate<'a> {
    pub hit: Hit,
    pub title: &'a str,
    pub usernames: &'a [String],
    pub order: i64,
    pub server: Option<usize>,
}

pub fn rank(query: &str, candidates: &[Candidate]) -> Vec<Hit> {
    let mut scored: Vec<(i32, i64, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let s = score(query, c.title, c.usernames)
                .or_else(|| c.server.map(|r| SERVER_ONLY - r.min(100) as i32))?;
            Some((s, c.order, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut out: Vec<Hit> = Vec::new();
    for (_, _, i) in scored {
        let hit = candidates[i].hit;
        if !out.contains(&hit) {
            out.push(hit);
        }
        if out.len() == CHAT_LIMIT {
            break;
        }
    }
    out
}

pub fn score(query: &str, title: &str, usernames: &[String]) -> Option<i32> {
    let raw = query.trim();
    let q = fold(raw);
    if q.is_empty() {
        return Some(0);
    }
    let direct = score_folded(&q, title, usernames);
    let swapped = swap_layout(&raw.to_lowercase())
        .filter(|_| q.chars().count() >= 3)
        .and_then(|alt| score_folded(&fold(&alt), title, usernames))
        .map(|s| s - 1);
    direct.max(swapped)
}

fn score_folded(q: &str, title: &str, usernames: &[String]) -> Option<i32> {
    let mut best: Option<i32> = None;
    let mut bump = |s: i32| best = Some(best.map_or(s, |b| b.max(s)));
    let name_q = q.trim_start_matches('@');
    if !name_q.is_empty() {
        for name in usernames {
            let name = fold(name);
            if name == name_q {
                bump(950);
            } else if name.starts_with(name_q) {
                bump(800);
            }
        }
    }
    if q.starts_with('@') {
        return best;
    }
    let t = fold(title);
    if t == q {
        bump(1000);
    } else if t.starts_with(q) {
        bump(900);
    }
    let tw = words(&t);
    let qw = words(q);
    if !qw.is_empty() && qw.iter().all(|w| tw.iter().any(|t| t.starts_with(w))) {
        let first = tw.iter().position(|t| t.starts_with(qw[0])).unwrap_or(0);
        bump(700 - first.min(50) as i32);
    }
    if q.chars().count() >= 2 && !q.contains(char::is_whitespace) {
        let initials: String = tw.iter().filter_map(|w| w.chars().next()).collect();
        if initials.starts_with(q) {
            bump(400);
        }
    }
    if t.contains(q) {
        bump(300);
    } else if !qw.is_empty() && qw.iter().all(|w| t.contains(w)) {
        bump(250);
    }
    best
}

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        push_folded(&mut out, c);
    }
    out
}

fn push_folded(out: &mut String, c: char) {
    for l in c.to_lowercase() {
        if !('\u{300}'..='\u{36f}').contains(&l) {
            out.push(base_letter(l));
        }
    }
}

fn base_letter(c: char) -> char {
    match c {
        'ё' => 'е',
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'į' | 'ı' => 'i',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ţ' | 'ť' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

pub fn swap_layout(text: &str) -> Option<String> {
    let ru: Vec<char> = LAYOUT_RU.chars().collect();
    let en: Vec<char> = LAYOUT_EN.chars().collect();
    let to_ru = text.chars().find(|c| c.is_alphabetic())?.is_ascii();
    let (from, to) = if to_ru { (&en, &ru) } else { (&ru, &en) };
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match from.iter().position(|f| *f == c) {
            Some(i) => out.push(to[i]),
            None if c.is_alphabetic() => return None,
            None => out.push(c),
        }
    }
    Some(out)
}

pub fn marks(text: &str, query: &str) -> Vec<(usize, usize)> {
    let mut folded: Vec<char> = Vec::with_capacity(text.len());
    let mut origin: Vec<(usize, usize)> = Vec::with_capacity(text.len());
    let mut buf = String::new();
    for (i, c) in text.char_indices() {
        buf.clear();
        push_folded(&mut buf, c);
        for f in buf.chars() {
            folded.push(f);
            origin.push((i, i + c.len_utf8()));
        }
    }
    let mut out: Vec<(usize, usize)> = Vec::new();
    for word in fold(query).split_whitespace() {
        let w: Vec<char> = word.chars().collect();
        if w.len() > folded.len() {
            continue;
        }
        let starts: Vec<usize> = (0..=folded.len() - w.len())
            .filter(|&i| folded[i..i + w.len()] == w[..])
            .collect();
        let at_word: Vec<usize> = starts
            .iter()
            .copied()
            .filter(|&i| i == 0 || !folded[i - 1].is_alphanumeric())
            .collect();
        let pick = if at_word.is_empty() { starts } else { at_word };
        out.extend(
            pick.into_iter()
                .map(|i| (origin[i].0, origin[i + w.len() - 1].1)),
        );
    }
    out.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(out.len());
    for (a, b) in out {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

pub fn excerpt_start(text: &str, marks: &[(usize, usize)], width: usize) -> usize {
    let Some(&(first, end)) = marks.first() else {
        return 0;
    };
    if text[..end].width() < width {
        return 0;
    }
    let lead = width / 3;
    let mut start = first;
    for (i, _) in text[..first].char_indices().rev() {
        if text[i..first].width() > lead {
            break;
        }
        start = i;
    }
    match text[start..first].find(' ') {
        Some(space) if start + space + 1 < first => start + space + 1,
        _ => start,
    }
}

pub fn parse_date(query: &str, today: NaiveDate) -> Option<NaiveDate> {
    let q = query.trim();
    let sep = ['-', '.', '/'].into_iter().find(|s| q.contains(*s))?;
    let parts: Vec<&str> = q.split(sep).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let num = |s: &str| s.parse::<u32>().ok();
    let (y, m, d) = if parts[0].len() == 4 && sep == '-' {
        (num(parts[0])? as i32, num(parts[1])?, num(parts[2])?)
    } else if sep != '-' && parts[0].len() <= 2 && parts[1].len() <= 2 {
        let year = match parts[2].len() {
            4 => num(parts[2])? as i32,
            2 => 2000 + num(parts[2])? as i32,
            _ => return None,
        };
        (year, num(parts[1])?, num(parts[0])?)
    } else {
        return None;
    };
    NaiveDate::from_ymd_opt(y, m, d).filter(|date| date.year() > 1970 && *date <= today)
}

pub fn day_end(date: NaiveDate) -> Option<i32> {
    let end = date.and_hms_opt(23, 59, 59)?;
    let ts = Local.from_local_datetime(&end).latest()?.timestamp();
    i32::try_from(ts).ok()
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn fold_ignores_case_yo_and_diacritics() {
        assert_eq!(fold("Ёжик в Тумане"), "ежик в тумане");
        assert_eq!(fold("Café Ünïcode ŁÓDŹ"), "cafe unicode lodz");
        assert_eq!(fold("e\u{301}tude"), "etude");
    }

    #[test]
    fn score_orders_exact_prefix_word_and_substring() {
        let none: Vec<String> = Vec::new();
        let exact = score("rust", "Rust", &none).unwrap();
        let prefix = score("rust", "Rustaceans", &none).unwrap();
        let word = score("rust", "Learning Rust", &none).unwrap();
        let initials = score("lr", "Learning Rust", &none).unwrap();
        let inside = score("ust", "Learning Rust", &none).unwrap();
        assert!(exact > prefix && prefix > word && word > initials && initials > inside);
        assert!(score("xyz", "Learning Rust", &none).is_none());
        assert_eq!(score("  ", "anything", &none), Some(0));
    }

    #[test]
    fn score_matches_word_prefixes_in_any_order() {
        let none: Vec<String> = Vec::new();
        assert!(score("smi ali", "Alice Smith", &none).is_some());
        assert!(score("ал см", "Алиса Смирнова", &none).is_some());
        assert!(score("ali smx", "Alice Smith", &none).is_none());
    }

    #[test]
    fn score_folds_yo_and_accents_on_both_sides() {
        let none: Vec<String> = Vec::new();
        assert!(score("ежик", "Ёжик", &none).is_some());
        assert!(score("ёжик", "Ежик", &none).is_some());
        assert!(score("cafe", "Café Paris", &none).is_some());
        assert!(score("CAFÉ", "cafe paris", &none).is_some());
    }

    #[test]
    fn score_uses_usernames_and_at_prefix() {
        let alice = names(&["alice_dev"]);
        assert_eq!(score("@alice_dev", "Someone", &alice), Some(950));
        assert_eq!(score("alice", "Someone", &alice), Some(800));
        assert!(score("@some", "Someone", &alice).is_none());
        assert!(score("some", "Someone", &alice).is_some());
    }

    #[test]
    fn score_forgives_wrong_keyboard_layout() {
        let none: Vec<String> = Vec::new();
        assert_eq!(swap_layout("ghbdtn").as_deref(), Some("привет"));
        assert_eq!(swap_layout("руддщ").as_deref(), Some("hello"));
        assert_eq!(swap_layout("abcж"), None);
        assert!(score("ghbdtn", "Привет всем", &none).is_some());
        assert!(score("кгые", "Rust", &none).is_some());
        let direct = score("рус", "Русский чат", &none).unwrap();
        let swapped = score("hec", "Русский чат", &none).unwrap();
        assert!(direct > swapped);
        assert!(
            score("ab", "фи", &none).is_none(),
            "too short to guess a layout"
        );
    }

    #[test]
    fn rank_sorts_by_score_then_order_and_keeps_server_hits() {
        let none: Vec<String> = Vec::new();
        let cands = [
            Candidate {
                hit: Hit::Chat(1),
                title: "Old Rust chat",
                usernames: &none,
                order: 10,
                server: None,
            },
            Candidate {
                hit: Hit::Chat(2),
                title: "Rust",
                usernames: &none,
                order: 1,
                server: None,
            },
            Candidate {
                hit: Hit::Chat(3),
                title: "Fresh Rust news",
                usernames: &none,
                order: 50,
                server: None,
            },
            Candidate {
                hit: Hit::Chat(4),
                title: "Ржавчина",
                usernames: &none,
                order: 5,
                server: Some(0),
            },
            Candidate {
                hit: Hit::Chat(5),
                title: "Unrelated",
                usernames: &none,
                order: 99,
                server: None,
            },
            Candidate {
                hit: Hit::Chat(2),
                title: "Rust",
                usernames: &none,
                order: 1,
                server: Some(1),
            },
        ];
        assert_eq!(
            rank("rust", &cands),
            vec![Hit::Chat(2), Hit::Chat(3), Hit::Chat(1), Hit::Chat(4)]
        );
    }

    #[test]
    fn marks_highlight_word_starts_with_folding() {
        let text = "Ёлка и елки: ЁЛКИ-палки";
        let got: Vec<&str> = marks(text, "елк")
            .iter()
            .map(|&(a, b)| &text[a..b])
            .collect();
        assert_eq!(got, vec!["Ёлк", "елк", "ЁЛК"]);
        let text = "concatenate the cat";
        let got: Vec<&str> = marks(text, "cat")
            .iter()
            .map(|&(a, b)| &text[a..b])
            .collect();
        assert_eq!(got, vec!["cat"]);
        assert_eq!(marks(text, "the cat"), vec![(12, 15), (16, 19)]);
        let text = "concatenate";
        assert_eq!(marks(text, "cat"), vec![(3, 6)]);
        assert!(marks(text, "dog").is_empty());
        assert!(marks(text, "").is_empty());
    }

    #[test]
    fn excerpt_starts_near_a_far_match() {
        let text = format!("{} needle and more", "word ".repeat(30));
        let m = marks(&text, "needle");
        assert_eq!(excerpt_start(&text, &m, 200), 0);
        let start = excerpt_start(&text, &m, 30);
        assert!(start > 0 && start < m[0].0);
        assert!(text[start..m[0].0].width() <= 10);
        assert!(text[start..].starts_with("word "));
    }

    #[test]
    fn parse_date_accepts_full_dates_only() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        let may = NaiveDate::from_ymd_opt(2024, 5, 1);
        assert_eq!(parse_date("2024-05-01", today), may);
        assert_eq!(parse_date(" 01.05.2024 ", today), may);
        assert_eq!(parse_date("1.5.24", today), may);
        assert_eq!(parse_date("01/05/2024", today), may);
        assert_eq!(parse_date("2027-01-01", today), None, "future");
        assert_eq!(parse_date("2024-02-30", today), None);
        assert_eq!(parse_date("1.5", today), None);
        assert_eq!(parse_date("v1.5.24", today), None);
        assert_eq!(parse_date("01-05-2024", today), None);
        assert_eq!(parse_date("hello", today), None);
        assert!(day_end(may.unwrap()).is_some());
    }

    fn progress<'a>(query: &'a str, peers: Phase, msgs: Phase) -> Progress<'a> {
        Progress {
            scope: Scope::All,
            query,
            peers,
            peer_errors: &[],
            msgs,
            total: 0,
            more: Phase::Done,
        }
    }

    #[test]
    fn layout_groups_sections_in_order_with_notes() {
        let hits = vec![
            (Section::Chats, Hit::Chat(1)),
            (Section::Chats, Hit::Contact(7)),
            (Section::Global, Hit::Chat(9)),
            (Section::Messages, Hit::Message(0)),
            (Section::Messages, Hit::Message(1)),
        ];
        let mut p = progress("rust", Phase::Done, Phase::Done);
        p.total = 40;
        p.more = Phase::Waiting;
        let rows = layout(&hits, &p);
        assert_eq!(
            rows,
            vec![
                Row::Header(Section::Chats, String::new()),
                Row::Hit(0, Hit::Chat(1)),
                Row::Hit(1, Hit::Contact(7)),
                Row::Header(Section::Global, String::new()),
                Row::Hit(2, Hit::Chat(9)),
                Row::Header(Section::Messages, "40 found".to_owned()),
                Row::Hit(3, Hit::Message(0)),
                Row::Hit(4, Hit::Message(1)),
                Row::Note("Loading more…".to_owned(), false),
            ]
        );
        assert_eq!(row_of(&rows, 3), 6);
    }

    #[test]
    fn layout_shows_searching_errors_and_empty_states() {
        let hits = vec![(Section::Chats, Hit::Chat(1))];
        let rows = layout(&hits, &progress("ru", Phase::Waiting, Phase::Waiting));
        assert_eq!(
            rows[2..],
            [
                Row::Header(Section::Global, String::new()),
                Row::Note("Searching…".to_owned(), false),
                Row::Header(Section::Messages, String::new()),
                Row::Note("Searching…".to_owned(), false),
            ]
        );
        let errors = vec!["global: timeout".to_owned()];
        let mut p = progress("ru", Phase::Done, Phase::Failed("network down".to_owned()));
        p.peer_errors = &errors;
        let rows = layout(&[], &p);
        assert_eq!(
            rows,
            vec![
                Row::Header(Section::Global, String::new()),
                Row::Note("global: timeout".to_owned(), true),
                Row::Header(Section::Messages, String::new()),
                Row::Note("network down".to_owned(), true),
            ]
        );
        let rows = layout(&[], &progress("zzz", Phase::Done, Phase::Done));
        assert_eq!(
            rows,
            vec![Row::Note("No results for “zzz”".to_owned(), false)]
        );
        let rows = layout(&[], &progress("", Phase::Idle, Phase::Idle));
        assert!(matches!(&rows[..], [Row::Note(n, false)] if n.starts_with("Type to search")));
    }

    #[test]
    fn tab_jumps_between_sections() {
        let hits = vec![
            (Section::Chats, Hit::Chat(1)),
            (Section::Chats, Hit::Chat(2)),
            (Section::Global, Hit::Chat(3)),
            (Section::Messages, Hit::Message(0)),
            (Section::Messages, Hit::Message(1)),
        ];
        assert_eq!(next_section(&hits, 0, true), 2);
        assert_eq!(next_section(&hits, 2, true), 3);
        assert_eq!(next_section(&hits, 4, true), 0);
        assert_eq!(next_section(&hits, 4, false), 3);
        assert_eq!(next_section(&hits, 3, false), 2);
        assert_eq!(next_section(&hits, 0, false), 3);
        assert_eq!(next_section(&[], 0, true), 0);
    }

    #[test]
    fn progress_tracks_debounce_and_paging() {
        let mut v = FindView::new(Scope::All);
        v.input = TextInput::with_text("rust");
        v.due = Some(Instant::now());
        let p = v.progress();
        assert_eq!((p.peers, p.msgs), (Phase::Waiting, Phase::Waiting));
        v.due = None;
        v.sent = Some("rust".to_owned());
        v.peers = Some(Peers::default());
        v.msgs = Vec::new();
        let p = v.progress();
        assert_eq!((p.peers, p.msgs), (Phase::Done, Phase::Done));
        v.error = Some("flood".to_owned());
        assert_eq!(v.progress().msgs, Phase::Failed("flood".to_owned()));
        v.scope = Scope::Messages;
        assert_eq!(v.progress().peers, Phase::Idle);
    }
}
