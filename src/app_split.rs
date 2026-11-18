use std::collections::BTreeSet;

use super::{App, Composer, Pane, SearchView};
use crate::keymap::Action;
use crate::model::{self, Sender};
use crate::td;

pub const OPEN_HINT: &str = "split: open another chat here";

pub struct SideView {
    pub open_chat: Option<i64>,
    pub open_topic: Option<i32>,
    pub msg_sel: Option<usize>,
    pub msg_top: usize,
    pub bottom_offset: usize,
    pub composer: Composer,
    pub marked: BTreeSet<i64>,
    pub edit_stash: Option<model::Draft>,
    pub chat_search: Option<SearchView>,
    pub pending_jump: Option<(i64, i64)>,
    pub jump_inflight: Option<(i64, i64)>,
    pub cmd_sel: usize,
}

impl App {
    pub fn swap_side(&mut self) {
        let Some(side) = self.side.as_mut() else {
            return;
        };
        std::mem::swap(&mut self.open_chat, &mut side.open_chat);
        std::mem::swap(&mut self.open_topic, &mut side.open_topic);
        std::mem::swap(&mut self.msg_sel, &mut side.msg_sel);
        std::mem::swap(&mut self.msg_top, &mut side.msg_top);
        std::mem::swap(&mut self.bottom_offset, &mut side.bottom_offset);
        std::mem::swap(&mut self.composer, &mut side.composer);
        std::mem::swap(&mut self.marked, &mut side.marked);
        std::mem::swap(&mut self.edit_stash, &mut side.edit_stash);
        std::mem::swap(&mut self.chat_search, &mut side.chat_search);
        std::mem::swap(&mut self.pending_jump, &mut side.pending_jump);
        std::mem::swap(&mut self.jump_inflight, &mut side.jump_inflight);
        std::mem::swap(&mut self.cmd_sel, &mut side.cmd_sel);
    }

    pub fn side_chat(&self) -> Option<i64> {
        self.side.as_ref().and_then(|s| s.open_chat)
    }

    pub fn side_title(&self) -> Option<String> {
        self.side_chat()
            .map(|id| self.sender_name(Sender::Chat(id)))
    }

    pub(super) fn split_open(&mut self) {
        let Some(chat) = self.open_chat else {
            self.set_status("open a chat first", false);
            return;
        };
        if self.side.is_some() {
            let key = self.keymap.label_for(Action::SplitClose).unwrap_or_default();
            self.set_status(format!("already split; {key} closes the focused pane"), false);
            return;
        }
        self.side = Some(SideView {
            open_chat: Some(chat),
            open_topic: self.open_topic,
            msg_sel: self.msg_sel,
            msg_top: self.msg_top,
            bottom_offset: self.bottom_offset,
            composer: Composer::default(),
            marked: self.marked.clone(),
            edit_stash: None,
            chat_search: None,
            pending_jump: None,
            jump_inflight: None,
            cmd_sel: 0,
        });
        self.side_left = true;
        let key = self.keymap.label_for(Action::SplitFocus).unwrap_or_default();
        self.set_status(format!("{OPEN_HINT}; {key} switches panes"), false);
    }

    pub(super) fn split_focus(&mut self) {
        if self.side.is_none() {
            let key = self.keymap.label_for(Action::SplitOpen).unwrap_or_default();
            self.set_status(format!("no split; {key} starts one"), false);
            return;
        }
        self.swap_side();
        self.side_left = !self.side_left;
        self.focus = Pane::Messages;
        self.mark_read();
        self.maybe_load_older();
    }

    fn side_draft(&mut self) -> Option<td::DraftSave> {
        let live = self.draft_key();
        self.swap_side();
        let save = if self.draft_key() == live {
            None
        } else {
            self.leave_draft()
        };
        self.swap_side();
        save
    }

    pub(super) fn side_drafts(&mut self) -> Vec<td::DraftSave> {
        if self.side.is_none() {
            return Vec::new();
        }
        self.side_draft().into_iter().collect()
    }

    pub(super) fn split_close_focused(&mut self) {
        if self.side.is_none() {
            self.set_status("no split to close", false);
            return;
        }
        if self.side_chat() != self.open_chat {
            self.split_focus();
        }
        self.split_close();
    }

    pub(super) fn split_close(&mut self) {
        if self.side.is_none() {
            self.set_status("no split to close", false);
            return;
        }
        if let Some(save) = self.side_draft() {
            self.td.set_draft(save);
        }
        let parked = self.side_chat();
        self.side = None;
        self.side_left = false;
        if let Some(chat) = parked
            && self.open_chat != Some(chat)
        {
            self.td.close_chat(chat);
            self.histories.remove(&chat);
        }
        self.set_status("split closed; the other pane stays", false);
    }

    pub(super) fn split_forward(&mut self) {
        let Some(target) = self.side_chat() else {
            let key = self.keymap.label_for(Action::SplitOpen).unwrap_or_default();
            self.set_status(format!("no split; {key} starts one"), false);
            return;
        };
        if let Some(why) = self.send_blocked(target) {
            self.set_status(why, true);
            return;
        }
        let msgs = self.action_msgs();
        let Some(from) = msgs.first().map(|m| m.chat_id) else {
            let key = |a| self.keymap.label_for(a).unwrap_or_default();
            let text = format!(
                "select a message ({}) or mark some ({}), then {}",
                key(Action::SelectPrevious),
                key(Action::MarkMessage),
                key(Action::SplitForward),
            );
            self.set_status(text, true);
            return;
        };
        let ids: Vec<i64> = msgs
            .iter()
            .filter(|m| self.can_forward(m))
            .map(|m| m.id)
            .collect();
        if ids.is_empty() {
            self.set_status("this message cannot be forwarded", true);
            return;
        }
        self.td.forward(target, from, ids, false);
        self.marked.clear();
        let name = self.sender_name(Sender::Chat(target));
        self.set_status(format!("forwarded to {name}"), false);
    }

    pub(super) fn side_shows(&self, chat: i64) -> bool {
        self.side_chat() == Some(chat)
    }

    pub(super) fn side_adopt_draft(&mut self) {
        let Some(key) = self.draft_key() else {
            return;
        };
        let Some(draft) = self.drafts.get(&key).cloned() else {
            return;
        };
        if self.side.is_none() {
            return;
        }
        self.swap_side();
        if self.draft_key() == Some(key)
            && self.composer.input.is_empty()
            && self.composer.attachments.is_empty()
            && self.composer.edit.is_none()
        {
            self.set_composer_draft(&draft);
        }
        self.swap_side();
    }

    pub(super) fn side_history_replaced(&mut self, chat: i64) {
        if let Some(side) = self.side.as_mut()
            && side.open_chat == Some(chat)
        {
            side.msg_sel = None;
            side.msg_top = 0;
            side.bottom_offset = 0;
        }
    }

    pub(super) fn split_chat_removed(&mut self, chat: i64) {
        let Some(side) = &self.side else {
            return;
        };
        if side.open_chat == Some(chat) {
            self.side = None;
            self.side_left = false;
            if let Some(live) = self.open_chat
                && live != chat
            {
                self.td.close_chat(chat);
                self.histories.remove(&chat);
            }
        } else if self.open_chat.is_none() {
            self.swap_side();
            self.side = None;
            self.side_left = false;
            self.mark_read();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{count, fake_app, msg, open_with_history, press};
    use super::super::{App, Pane};
    use crate::config::Settings;
    use crate::keymap::Action;
    use crate::input::TextInput;
    use crate::td::RequestLog;
    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    fn chord(app: &mut App, key: char) {
        press(app, K::Char('w'), M::CONTROL);
        press(app, K::Char(key), M::NONE);
    }

    fn type_into(app: &mut App, text: &str) {
        app.composer.input = TextInput::with_text(text);
    }

    fn split_app() -> (App, RequestLog) {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.msg_sel = Some(5);
        app.msg_top = 3;
        app.bottom_offset = 0;
        type_into(&mut app, "ten");
        chord(&mut app, 'v');
        open_with_history(&mut app, 20);
        app.msg_sel = Some(7);
        app.msg_top = 4;
        type_into(&mut app, "twenty");
        (app, log)
    }

    type View = (
        Option<i64>,
        Option<usize>,
        usize,
        usize,
        String,
        Option<i64>,
        Option<usize>,
        String,
        bool,
    );

    fn view(app: &App) -> View {
        let side = app.side.as_ref().unwrap();
        (
            app.open_chat,
            app.msg_sel,
            app.msg_top,
            app.bottom_offset,
            app.composer.input.text().to_owned(),
            side.open_chat,
            side.msg_sel,
            side.composer.input.text().to_owned(),
            app.side_left,
        )
    }

    fn entries(log: &RequestLog) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    #[test]
    fn split_open_parks_a_copy_and_opening_another_chat_leaves_it_alone() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.msg_sel = Some(5);
        app.msg_top = 3;
        type_into(&mut app, "typed");
        chord(&mut app, 'v');
        let side = app.side.as_ref().unwrap();
        assert_eq!(side.open_chat, Some(10));
        assert_eq!((side.msg_sel, side.msg_top), (Some(5), 3));
        assert!(side.composer.input.is_empty());
        assert!(app.side_left);
        assert_eq!(app.composer.input.text(), "typed");
        let closes = count(&log, "close chat");
        open_with_history(&mut app, 20);
        let side = app.side.as_ref().unwrap();
        assert_eq!(side.open_chat, Some(10));
        assert_eq!((side.msg_sel, side.msg_top), (Some(5), 3));
        assert!(app.histories.contains_key(&10));
        assert_eq!(count(&log, "close chat"), closes);
        assert_eq!(app.open_chat, Some(20));
        chord(&mut app, 'v');
        assert_eq!(app.side.as_ref().unwrap().open_chat, Some(10));
    }

    #[test]
    fn split_open_needs_a_chat() {
        let (mut app, _log) = fake_app(Settings::default());
        chord(&mut app, 'v');
        assert!(app.side.is_none());
    }

    #[test]
    fn split_focus_swaps_everything_and_twice_is_the_identity() {
        let (mut app, _log) = split_app();
        app.side.as_mut().unwrap().composer.input = TextInput::with_text("side");
        app.side.as_mut().unwrap().msg_top = 9;
        let before = view(&app);
        chord(&mut app, 'w');
        assert_eq!(app.open_chat, Some(10));
        assert_eq!((app.msg_sel, app.msg_top), (Some(5), 9));
        assert_eq!(app.composer.input.text(), "side");
        let side = app.side.as_ref().unwrap();
        assert_eq!(side.open_chat, Some(20));
        assert_eq!(side.composer.input.text(), "twenty");
        assert!(!app.side_left);
        assert_eq!(app.focus, Pane::Messages);
        chord(&mut app, 'w');
        assert_eq!(view(&app), before);
    }

    #[test]
    fn swap_side_issues_no_requests() {
        let (mut app, log) = split_app();
        let before = entries(&log);
        app.swap_side();
        app.swap_side();
        assert_eq!(entries(&log), before);
    }

    #[test]
    fn split_close_closes_the_focused_pane_and_saves_its_draft() {
        let (mut app, log) = split_app();
        app.side.as_mut().unwrap().composer.input = TextInput::with_text("parked text");
        chord(&mut app, 'q');
        assert!(app.side.is_none());
        assert!(!app.side_left);
        assert_eq!(app.open_chat, Some(10));
        assert_eq!(app.composer.input.text(), "parked text");
        assert!(!app.histories.contains_key(&20));
        assert!(app.histories.contains_key(&10));
        assert!(entries(&log).contains(&"draft 20 topic None \"twenty\" reply None".to_owned()));
        assert_eq!(count(&log, "close chat"), 1);
        assert!(app.drafts.contains_key(&(20, None)));
        let status = app.status.as_ref().unwrap().0.clone();
        assert!(status.contains("the other pane stays"), "{status}");
    }

    #[test]
    fn split_close_of_the_same_chat_twice_keeps_the_live_draft_and_history() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        chord(&mut app, 'v');
        type_into(&mut app, "live text");
        let before = entries(&log);
        chord(&mut app, 'q');
        assert!(app.side.is_none());
        assert!(app.histories.contains_key(&10));
        assert_eq!(app.composer.input.text(), "live text");
        assert_eq!(entries(&log).len(), before.len());
    }

    #[test]
    fn split_forward_sends_the_selected_message_to_the_parked_chat() {
        let (mut app, log) = split_app();
        for id in (1..=40).map(|i| i * 10) {
            app.histories.get_mut(&20).unwrap().get_mut(id).unwrap().meta.can_be_saved = true;
        }
        app.msg_sel = app.histories[&20].index_of(300);
        chord(&mut app, 'f');
        assert!(entries(&log).contains(&"forward 20 [300] -> 10".to_owned()));
        let status = app.status.as_ref().unwrap().0.clone();
        assert!(status.contains("chat 10"), "{status}");
        app.marked.extend([100, 200, 400]);
        chord(&mut app, 'f');
        assert!(entries(&log).contains(&"forward 20 [100, 200, 400] -> 10".to_owned()));
        assert!(app.marked.is_empty());
    }

    #[test]
    fn closing_the_live_chat_promotes_the_parked_one() {
        let (mut app, _log) = split_app();
        app.side.as_mut().unwrap().composer.input = TextInput::with_text("side");
        app.close_removed_chat(20);
        assert!(app.side.is_none());
        assert_eq!(app.open_chat, Some(10));
        assert_eq!((app.msg_sel, app.msg_top), (Some(5), 3));
        assert_eq!(app.composer.input.text(), "side");
        assert!(app.histories.contains_key(&10));
    }

    #[test]
    fn removing_the_parked_chat_ends_the_split() {
        let (mut app, log) = split_app();
        app.close_removed_chat(10);
        assert!(app.side.is_none());
        assert_eq!(app.open_chat, Some(20));
        assert!(!app.histories.contains_key(&10));
        assert_eq!(count(&log, "close chat"), 1);
    }

    #[test]
    fn focus_switch_marks_the_new_live_chat_read_without_opening_or_closing() {
        let (mut app, log) = split_app();
        let opens = count(&log, "open chat");
        let closes = count(&log, "close chat");
        let views = entries(&log).iter().filter(|e| e.starts_with("view 10")).count();
        app.msg_sel = None;
        app.side.as_mut().unwrap().msg_sel = None;
        app.histories.get_mut(&10).unwrap().upsert(msg(10, 500, 2));
        assert_eq!(entries(&log).iter().filter(|e| e.starts_with("view 10")).count(), views);
        chord(&mut app, 'w');
        let now = entries(&log).iter().filter(|e| e.starts_with("view 10")).count();
        assert_eq!(now, views + 1);
        assert_eq!(count(&log, "open chat"), opens);
        assert_eq!(count(&log, "close chat"), closes);
    }

    #[test]
    fn logging_out_clears_the_split() {
        let (mut app, _log) = split_app();
        app.reset_session();
        assert!(app.side.is_none());
        assert!(app.per_account_leftovers().is_empty(), "{:?}", app.per_account_leftovers());
    }

    fn to_chats(app: &mut App) {
        app.show_chats = true;
        app.show_folders = true;
        app.show_info = true;
        app.focus = Pane::Chats;
    }

    fn live_chat(app: &App) -> Option<i64> {
        app.open_chat
    }

    #[test]
    fn right_moves_walk_chats_left_pane_right_pane_info() {
        let (mut app, _log) = split_app();
        to_chats(&mut app);
        assert!(app.side_left);
        press(&mut app, K::Tab, M::NONE);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(live_chat(&app), Some(10));
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(live_chat(&app), Some(20));
        press(&mut app, K::Tab, M::NONE);
        assert_eq!(app.focus, Pane::Info);
    }

    #[test]
    fn left_moves_walk_info_right_pane_left_pane_chats() {
        let (mut app, _log) = split_app();
        to_chats(&mut app);
        app.focus = Pane::Info;
        press(&mut app, K::BackTab, M::SHIFT);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(live_chat(&app), Some(20));
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(live_chat(&app), Some(10));
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.focus, Pane::Chats);
    }

    #[test]
    fn entering_from_the_right_lands_on_the_right_pane_even_when_parked_there() {
        let (mut app, _log) = split_app();
        to_chats(&mut app);
        chord(&mut app, 'w');
        assert!(!app.side_left);
        app.focus = Pane::Info;
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(live_chat(&app), Some(20));
        assert!(app.side_left);
        app.focus = Pane::Chats;
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(live_chat(&app), Some(10));
        assert!(!app.side_left);
    }

    #[test]
    fn number_three_toggles_panes_only_when_messages_is_focused() {
        let (mut app, _log) = split_app();
        to_chats(&mut app);
        press(&mut app, K::Char('3'), M::NONE);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(live_chat(&app), Some(20));
        press(&mut app, K::Char('3'), M::NONE);
        assert_eq!(live_chat(&app), Some(10));
        press(&mut app, K::Char('3'), M::NONE);
        assert_eq!(live_chat(&app), Some(20));
    }

    #[test]
    fn hidden_panes_are_skipped_around_the_split() {
        let (mut app, _log) = split_app();
        to_chats(&mut app);
        app.show_info = false;
        app.focus = Pane::Messages;
        assert!(app.side_left);
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.focus, Pane::Folders);
    }

    #[test]
    fn without_a_split_panes_cycle_as_before() {
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        to_chats(&mut app);
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.focus, Pane::Messages);
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.focus, Pane::Info);
    }

    fn labels(app: &App) -> (String, String, String) {
        let l = |a| app.keymap.label_for(a).unwrap_or_default();
        (
            l(Action::SelectPrevious),
            l(Action::MarkMessage),
            l(Action::SplitForward),
        )
    }

    #[test]
    fn ctrl_w_f_reaches_forward_and_without_a_selection_names_the_keys_as_an_error() {
        let (mut app, log) = split_app();
        app.focus = Pane::Messages;
        app.msg_sel = None;
        app.status = None;
        chord(&mut app, 'f');
        let (k, m, f) = labels(&app);
        assert_eq!(
            app.status,
            Some((format!("select a message ({k}) or mark some ({m}), then {f}"), true))
        );
        assert_eq!(count(&log, "forward"), 0);
        for id in (1..=40).map(|i| i * 10) {
            app.histories.get_mut(&20).unwrap().get_mut(id).unwrap().meta.can_be_saved = true;
        }
        app.msg_sel = app.histories[&20].index_of(300);
        app.status = None;
        chord(&mut app, 'f');
        assert_eq!(count(&log, "forward"), 1);
        assert_eq!(app.status, Some(("forwarded to chat 10".to_owned(), false)));
    }
}
