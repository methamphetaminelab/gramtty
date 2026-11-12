use std::collections::HashMap;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Popup, SearchView};
use crate::input::Edit;
use crate::model::{self, ChatKind, ListId, MediaFilter, Membership, Msg, Sender};
use crate::search::{
    self, CHAT_LIMIT, Candidate, DEBOUNCE, FindView, Hit, PAGE_AHEAD, Row, Scope, Section,
};
use crate::td::{Found, Peers, Td};

impl App {
    pub(super) fn open_find(&mut self, scope: Scope) {
        self.popup = Some(Popup::Find(FindView::new(scope)));
        self.run_find();
    }

    pub fn find_hits(&self) -> Vec<(Section, Hit)> {
        let Some(Popup::Find(v)) = &self.popup else {
            return Vec::new();
        };
        let query = v.query();
        let mut out = Vec::new();
        if v.scope == Scope::All && query.is_empty() {
            let recent: Vec<i64> = v
                .recent
                .iter()
                .copied()
                .filter(|id| self.chats.contains_key(id))
                .collect();
            out.extend(recent.iter().map(|id| (Section::Recent, Hit::Chat(*id))));
            let top = model::sorted_chat_ids(self.chats.values(), ListId::Main);
            out.extend(
                top.into_iter()
                    .filter(|id| !recent.contains(id))
                    .take(CHAT_LIMIT)
                    .map(|id| (Section::Chats, Hit::Chat(id))),
            );
        } else if v.scope == Scope::All {
            let chats = self.ranked_chats(v, query);
            let public: Vec<i64> = v.peers.as_ref().map_or_else(Vec::new, |p| {
                p.public
                    .iter()
                    .copied()
                    .filter(|id| self.chats.contains_key(id) && !chats.contains(&Hit::Chat(*id)))
                    .collect()
            });
            out.extend(chats.into_iter().map(|h| (Section::Chats, h)));
            out.extend(
                public
                    .into_iter()
                    .map(|id| (Section::Global, Hit::Chat(id))),
            );
        }
        if !query.is_empty() {
            out.extend((0..v.msgs.len()).map(|i| (Section::Messages, Hit::Message(i))));
        }
        out
    }

    fn ranked_chats(&self, v: &FindView, query: &str) -> Vec<Hit> {
        let (known, contacts): (&[i64], &[i64]) = v
            .peers
            .as_ref()
            .map_or((&[][..], &[][..]), |p| (&p.known[..], &p.contacts[..]));
        let mut private: HashMap<i64, i64> = HashMap::new();
        let mut cands: Vec<Candidate> = Vec::new();
        for c in self.chats.values() {
            if let ChatKind::Private(user) = c.kind {
                private.insert(user, c.id);
            }
            let server = known.iter().position(|k| *k == c.id);
            if c.positions.is_empty() && server.is_none() {
                continue;
            }
            let order = c
                .position_in(ListId::Main)
                .or(c.positions.first())
                .map_or(0, |p| p.order);
            cands.push(Candidate {
                hit: Hit::Chat(c.id),
                title: &c.title,
                usernames: self.chat_usernames(c.id),
                order,
                server,
            });
        }
        for (rank, user) in contacts.iter().enumerate() {
            let hit = private
                .get(user)
                .map_or(Hit::Contact(*user), |c| Hit::Chat(*c));
            if let Some(c) = cands.iter_mut().find(|c| c.hit == hit) {
                c.server = c.server.or(Some(rank));
                continue;
            }
            cands.push(Candidate {
                hit,
                title: self.users.get(user).map_or("", String::as_str),
                usernames: self
                    .user_meta
                    .get(user)
                    .map_or(&[][..], |m| &m.usernames[..]),
                order: 0,
                server: Some(rank),
            });
        }
        search::rank(query, &cands)
    }

    pub fn find_rows(&self) -> (Vec<(Section, Hit)>, Vec<Row>) {
        let hits = self.find_hits();
        let rows = match &self.popup {
            Some(Popup::Find(v)) => search::layout(&hits, &v.progress()),
            _ => Vec::new(),
        };
        (hits, rows)
    }

    pub fn is_member(&self, chat_id: i64) -> bool {
        let Some(c) = self.chats.get(&chat_id) else {
            return true;
        };
        match c.kind {
            ChatKind::Supergroup | ChatKind::Channel => self
                .groups
                .get(&c.group_id)
                .is_none_or(|g| g.status != Membership::Left),
            _ => true,
        }
    }

    pub(super) fn find_edited(&mut self) {
        let Some(Popup::Find(v)) = &mut self.popup else {
            return;
        };
        v.sel = 0;
        if v.query().is_empty() {
            self.run_find();
            return;
        }
        v.due = Some(Instant::now() + DEBOUNCE);
        v.peers = None;
        v.peers_token = 0;
        v.token = 0;
        v.msgs.clear();
        v.next = None;
        v.total = 0;
        v.error = None;
        v.loading = false;
        v.paging = false;
    }

    fn run_find(&mut self) {
        let Some(Popup::Find(v)) = &mut self.popup else {
            return;
        };
        let query = v.query().to_owned();
        v.due = None;
        v.sent = Some(query.clone());
        v.peers = None;
        v.peers_token = 0;
        v.token = 0;
        v.msgs.clear();
        v.next = None;
        v.total = 0;
        v.error = None;
        v.loading = false;
        v.paging = false;
        if v.scope == Scope::All {
            self.next_token += 1;
            v.peers_token = self.next_token;
            self.td.find_peers(self.next_token, query.clone());
        }
        if !query.is_empty() {
            self.next_token += 1;
            v.token = self.next_token;
            v.loading = true;
            self.td
                .search(self.next_token, None, query, false, None, None, None);
        }
    }

    pub(super) fn search_due(&self) -> Option<Instant> {
        match &self.popup {
            Some(Popup::Find(v)) => v.due,
            Some(Popup::Search(sv)) => sv.due,
            _ => None,
        }
    }

    pub(super) fn flush_search(&mut self, now: Instant) {
        if self.search_due().is_some_and(|due| due <= now) {
            match &self.popup {
                Some(Popup::Find(_)) => self.run_find(),
                Some(Popup::Search(_)) => self.run_search(),
                _ => {}
            }
        }
    }

    fn find_selected(&self) -> Option<Hit> {
        let Some(Popup::Find(v)) = &self.popup else {
            return None;
        };
        self.find_hits().get(v.sel).map(|(_, h)| *h)
    }

    pub(super) fn on_peers(&mut self, token: u64, peers: Peers) {
        let keep = self.find_selected();
        let Some(Popup::Find(v)) = &mut self.popup else {
            return;
        };
        if v.peers_token != token {
            return;
        }
        if v.sent.as_deref() == Some("") {
            v.recent = peers.known;
        } else {
            v.peers = Some(peers);
        }
        let hits = self.find_hits();
        if let Some(i) = keep.and_then(|k| hits.iter().position(|(_, h)| *h == k))
            && let Some(Popup::Find(v)) = &mut self.popup
        {
            v.sel = i;
        }
    }

    pub(super) fn on_found(&mut self, token: u64, result: Result<Found, String>) {
        match &mut self.popup {
            Some(Popup::Search(sv)) if sv.token == token => {
                apply_found(sv, result);
                return;
            }
            Some(Popup::Find(v)) if v.token == token => {
                v.loading = false;
                match result {
                    Ok(found) => {
                        append(&mut v.msgs, found.msgs, v.paging);
                        v.next = found.next;
                        v.total = found.total;
                        v.error = None;
                    }
                    Err(e) => {
                        if !v.paging {
                            v.msgs.clear();
                        }
                        v.error = Some(e);
                    }
                }
                v.paging = false;
                return;
            }
            _ => {}
        }
        let follow = match self.chat_search.as_mut() {
            Some(sv) if sv.token == token => {
                apply_found(sv, result);
                std::mem::take(&mut sv.follow)
            }
            _ => return,
        };
        if follow {
            self.step_search(1);
        }
    }

    pub(super) fn more_results(&mut self) {
        let at = match &self.popup {
            Some(Popup::Find(v)) => match self.find_hits().get(v.sel) {
                Some(&(_, Hit::Message(i))) => Some(i),
                _ => None,
            },
            _ => None,
        };
        match &mut self.popup {
            Some(Popup::Search(sv)) if sv.sel + PAGE_AHEAD >= sv.rows() => {
                next_page(sv, &self.td, &mut self.next_token)
            }
            Some(Popup::Find(v)) if at.is_some_and(|i| i + PAGE_AHEAD >= v.msgs.len()) => {
                let idle = !v.loading && v.error.is_none() && v.due.is_none();
                if let Some(cursor) = v.next.clone().filter(|_| idle) {
                    self.next_token += 1;
                    v.token = self.next_token;
                    v.loading = true;
                    v.paging = true;
                    let query = v.sent.clone().unwrap_or_default();
                    self.td
                        .search(v.token, None, query, false, None, None, Some(cursor));
                }
            }
            _ => {}
        }
    }

    pub(super) fn on_find_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let hits = self.find_hits();
        let Some(Popup::Find(v)) = &mut self.popup else {
            return;
        };
        let last = hits.len().saturating_sub(1);
        let cur = v.sel.min(last);
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Down => v.sel = (cur + 1).min(last),
            KeyCode::Char('n') if ctrl => v.sel = (cur + 1).min(last),
            KeyCode::Up => v.sel = cur.saturating_sub(1),
            KeyCode::Char('p') if ctrl => v.sel = cur.saturating_sub(1),
            KeyCode::Tab => v.sel = search::next_section(&hits, cur, true),
            KeyCode::BackTab => v.sel = search::next_section(&hits, cur, false),
            KeyCode::Enter => match hits.get(cur) {
                Some(&(_, hit)) => self.open_hit(hit, alt),
                None if v.due.is_some() => self.run_find(),
                None => {}
            },
            _ => {
                if let Edit::Changed = v.input.handle_key(&key, false) {
                    self.find_edited();
                }
            }
        }
        self.more_results();
    }

    fn open_hit(&mut self, hit: Hit, join: bool) {
        let Some(Popup::Find(v)) = &self.popup else {
            return;
        };
        match hit {
            Hit::Message(i) => {
                let Some(m) = v.msgs.get(i) else {
                    return;
                };
                let (chat, id, topic) = (m.chat_id, m.id, m.topic);
                self.popup = None;
                self.jump_to_in(chat, id, topic);
            }
            Hit::Chat(chat) => {
                self.popup = None;
                self.open_found_chat(chat, join);
            }
            Hit::Contact(user) => {
                self.popup = None;
                let chat = self
                    .chats
                    .values()
                    .find(|c| c.kind == ChatKind::Private(user))
                    .map(|c| c.id);
                match chat {
                    Some(chat) => self.open_found_chat(chat, false),
                    None => {
                        self.td.open_private_chat(user);
                        let name = self.sender_name(Sender::User(user));
                        self.set_status(format!("opening chat with {name}…"), false);
                    }
                }
            }
        }
    }

    pub(super) fn open_found_chat(&mut self, chat_id: i64, join: bool) {
        if !self.chats.contains_key(&chat_id) {
            self.set_status("this chat is not loaded yet, try again", true);
            return;
        }
        self.td.add_recent_found(chat_id);
        self.open_any(chat_id);
        if self.is_member(chat_id) {
            return;
        }
        let title = self.sender_name(Sender::Chat(chat_id));
        if join {
            self.td.join_chat(chat_id);
            self.set_status(format!("joining {title}…"), false);
        } else {
            self.set_status(
                format!("{title}: not joined · Alt-Enter on it in search joins"),
                false,
            );
        }
    }

    pub(super) fn open_search(&mut self, chat_id: Option<i64>, pinned: bool) {
        let resume = |s: &mut SearchView| chat_id.is_some() && s.chat_id == chat_id && !s.browse;
        if !pinned && let Some(sv) = self.chat_search.take_if(resume) {
            self.popup = Some(Popup::Search(sv));
            return;
        }
        let mut sv = SearchView::new(chat_id, pinned);
        sv.pinned = pinned;
        self.popup = Some(Popup::Search(sv));
        if pinned {
            self.run_search();
        }
    }

    pub(super) fn open_from_sender(&mut self, chat_id: i64, sender: Sender) {
        let mut sv = SearchView::new(Some(chat_id), true);
        sv.sender = Some(sender);
        self.popup = Some(Popup::Search(sv));
        self.run_search();
    }

    pub(super) fn open_media_list(&mut self, chat_id: i64, media: MediaFilter) {
        let mut sv = SearchView::new(Some(chat_id), true);
        sv.media = Some(media);
        self.popup = Some(Popup::Search(sv));
        self.run_search();
    }

    pub(super) fn run_search(&mut self) {
        self.next_token += 1;
        let token = self.next_token;
        let Some(Popup::Search(sv)) = &mut self.popup else {
            return;
        };
        let query = sv.input.text().trim().to_owned();
        sv.due = None;
        sv.searched_for = Some(query.clone());
        sv.next = None;
        sv.total = 0;
        sv.paging = false;
        sv.follow = false;
        sv.error = None;
        sv.results.clear();
        sv.sel = 0;
        if query.is_empty() && !sv.filtered() {
            sv.token = 0;
            sv.loading = false;
            return;
        }
        sv.token = token;
        sv.loading = true;
        self.td.search(
            token, sv.chat_id, query, sv.pinned, sv.sender, sv.media, None,
        );
    }

    pub(super) fn search_edited(&mut self) {
        let Some(Popup::Search(sv)) = &mut self.popup else {
            return;
        };
        sv.sel = 0;
        sv.token = 0;
        sv.loading = false;
        sv.results.clear();
        sv.next = None;
        sv.total = 0;
        sv.error = None;
        sv.due = Some(Instant::now() + DEBOUNCE);
    }

    pub(super) fn on_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Popup::Search(sv)) = &mut self.popup else {
            return;
        };
        let last = sv.rows().saturating_sub(1);
        let nav = sv.listing();
        let down = key.code == KeyCode::Down
            || (ctrl && key.code == KeyCode::Char('n'))
            || (nav && key.code == KeyCode::Char('j'));
        let up = key.code == KeyCode::Up
            || (ctrl && key.code == KeyCode::Char('p'))
            || (nav && key.code == KeyCode::Char('k'));
        let chips = !nav && sv.chat_id.is_some();
        match key.code {
            KeyCode::Esc => {
                self.popup = None;
                self.chat_search = None;
            }
            KeyCode::Char('q') if nav => self.popup = None,
            _ if down => sv.sel = (sv.sel + 1).min(last),
            _ if up => sv.sel = sv.sel.saturating_sub(1),
            KeyCode::Char('/') if nav => {
                sv.browse = false;
                sv.pinned = false;
            }
            KeyCode::Tab | KeyCode::BackTab if chips => {
                sv.media = cycle_media(sv.media, key.code == KeyCode::Tab);
                self.run_search();
            }
            KeyCode::Backspace if chips && sv.input.is_empty() && sv.filtered() => {
                if sv.media.is_some() {
                    sv.media = None;
                } else {
                    sv.sender = None;
                }
                self.run_search();
            }
            KeyCode::Enter => self.search_enter(),
            _ if !nav => {
                if let Edit::Changed = sv.input.handle_key(&key, false) {
                    self.search_edited();
                }
            }
            _ => {}
        }
        self.more_results();
    }

    fn search_enter(&mut self) {
        let Some(Popup::Search(sv)) = &mut self.popup else {
            return;
        };
        if let Some(date) = sv.date().filter(|_| sv.sel == 0)
            && let Some(chat) = sv.chat_id
        {
            self.popup = None;
            self.chat_search = None;
            match search::day_end(date) {
                Some(ts) => {
                    self.td.message_by_date(chat, ts);
                    self.set_status(format!("going to {date}…"), false);
                }
                None => self.set_status(format!("cannot go to {date}"), true),
            }
            return;
        }
        let fresh = sv.listing()
            || (sv.due.is_none() && sv.searched_for.as_deref() == Some(sv.input.text().trim()));
        if !fresh {
            self.run_search();
            return;
        }
        let Some(m) = sv.picked() else {
            return;
        };
        let (chat, id, topic) = (m.chat_id, m.id, m.topic);
        let Some(Popup::Search(sv)) = self.popup.take() else {
            return;
        };
        let status = sv.chat_id.map(|_| self.step_status(&sv));
        if sv.chat_id.is_some() {
            self.chat_search = Some(sv);
        }
        self.jump_to_in(chat, id, topic);
        if let Some(status) = status {
            self.set_status(status, false);
        }
    }

    fn step_status(&self, sv: &SearchView) -> String {
        let query = sv.searched_for.as_deref().unwrap_or("");
        let label = if !query.is_empty() {
            format!("“{query}”")
        } else if let Some(s) = sv.sender {
            format!("from {}", self.sender_name(s))
        } else if let Some(m) = sv.media {
            m.label().to_owned()
        } else if sv.pinned {
            "pinned".to_owned()
        } else {
            "results".to_owned()
        };
        let at = sv.sel.saturating_sub(sv.offset()) + 1;
        let total = sv.total.max(sv.results.len() as i32);
        format!("{label}: {at} of {total} · n/N next/previous")
    }

    pub(super) fn step_search(&mut self, delta: isize) {
        let open = self.open_chat;
        let Some(sv) = self
            .chat_search
            .as_mut()
            .filter(|s| open.is_some() && s.chat_id == open)
        else {
            self.set_status(
                "no search in this chat: / in the messages pane starts one",
                false,
            );
            return;
        };
        let offset = sv.offset();
        let next = sv.sel.saturating_sub(offset) as isize + delta;
        if next < 0 {
            self.set_status("no newer results", false);
            return;
        }
        let Some(m) = sv.results.get(next as usize) else {
            if sv.next.is_some() && sv.error.is_none() {
                sv.follow = true;
                if !sv.loading {
                    next_page(sv, &self.td, &mut self.next_token);
                }
                self.set_status("loading more results…", false);
            } else {
                self.set_status("no older results", false);
            }
            return;
        };
        let (chat, id, topic) = (m.chat_id, m.id, m.topic);
        sv.sel = next as usize + offset;
        let status = self.chat_search.as_ref().map(|sv| self.step_status(sv));
        self.jump_to_in(chat, id, topic);
        if let Some(status) = status {
            self.set_status(status, false);
        }
    }
}

fn apply_found(sv: &mut SearchView, result: Result<Found, String>) {
    sv.loading = false;
    match result {
        Ok(found) => {
            if !sv.paging {
                sv.sel = 0;
            }
            append(&mut sv.results, found.msgs, sv.paging);
            sv.next = found.next;
            sv.total = found.total;
            sv.error = None;
        }
        Err(e) => {
            if !sv.paging {
                sv.results.clear();
                sv.sel = 0;
            }
            sv.error = Some(e);
        }
    }
    sv.paging = false;
}

fn append(list: &mut Vec<Msg>, more: Vec<Msg>, paging: bool) {
    if !paging {
        *list = more;
        return;
    }
    for m in more {
        if !list.iter().any(|x| x.chat_id == m.chat_id && x.id == m.id) {
            list.push(m);
        }
    }
}

fn next_page(sv: &mut SearchView, td: &Td, next_token: &mut u64) {
    if sv.loading || sv.error.is_some() || sv.due.is_some() {
        return;
    }
    let Some(cursor) = sv.next.clone() else {
        return;
    };
    *next_token += 1;
    sv.token = *next_token;
    sv.loading = true;
    sv.paging = true;
    let query = sv.searched_for.clone().unwrap_or_default();
    td.search(
        sv.token,
        sv.chat_id,
        query,
        sv.pinned,
        sv.sender,
        sv.media,
        Some(cursor),
    );
}

fn cycle_media(current: Option<MediaFilter>, forward: bool) -> Option<MediaFilter> {
    let all = MediaFilter::ALL;
    let at = current.and_then(|m| all.iter().position(|x| *x == m));
    let n = all.len() + 1;
    let pos = at.map_or(0, |i| i + 1);
    let next = if forward {
        (pos + 1) % n
    } else {
        (pos + n - 1) % n
    };
    next.checked_sub(1).map(|i| all[i])
}

#[cfg(test)]
mod tests {
    use super::cycle_media;
    use crate::model::MediaFilter;

    #[test]
    fn media_chips_cycle_through_all_and_back() {
        assert_eq!(cycle_media(None, true), Some(MediaFilter::Photos));
        assert_eq!(cycle_media(Some(MediaFilter::Music), true), None);
        assert_eq!(cycle_media(None, false), Some(MediaFilter::Music));
        assert_eq!(cycle_media(Some(MediaFilter::Photos), false), None);
    }
}
