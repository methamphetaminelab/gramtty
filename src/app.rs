use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tdlib_rs::enums::{AuthorizationState, ConnectionState, OptionValue, Update};

use crate::config::ApiCredentials;
use crate::format;
use crate::input::{Edit, TextInput};
use crate::keymap::{Action, Chord, Feed, Keymap};
use crate::model::{self, ChatInfo, History, ListId, MAX_WINDOW, Msg, Sender};
use crate::td::{self, Event, Td};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Folders,
    Chats,
    Messages,
    Info,
}

impl Pane {
    const ORDER: [Pane; 4] = [Pane::Folders, Pane::Chats, Pane::Messages, Pane::Info];

    fn index(self) -> usize {
        Pane::ORDER.iter().position(|p| *p == self).unwrap_or(2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Starting,
    Qr(Option<String>),
    Phone,
    Code { phone: String, via: String },
    Password { hint: String },
    Email,
    EmailCode { pattern: String },
    Blocked(String),
    Ready,
    Closing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginMode {
    Qr,
    Phone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Reply,
    Edit,
    Delete,
    OpenUrl,
    OpenChat,
    MarkRead,
    LoadMore,
}

pub struct MenuItem {
    pub key: char,
    pub label: &'static str,
    pub action: MenuAction,
}

pub enum Popup {
    Menu {
        title: &'static str,
        items: Vec<MenuItem>,
        sel: usize,
    },
    ConfirmDelete {
        chat_id: i64,
        message_id: i64,
    },
    Switcher {
        input: TextInput,
        sel: usize,
    },
    ConfirmLogout,
}

#[derive(Default)]
pub struct Composer {
    pub input: TextInput,
    pub reply_to: Option<i64>,
    pub edit: Option<i64>,
    pub active: bool,
}

pub struct Filter {
    pub pane: Pane,
    pub input: TextInput,
    pub active: bool,
}

pub struct App {
    pub td: Td,
    creds: ApiCredentials,
    data_dir: PathBuf,
    pub keymap: Keymap,
    pub auth: Auth,
    login_mode: LoginMode,
    restart_after_close: bool,
    pub login_input: TextInput,
    pub auth_error: Option<String>,
    pub quitting: bool,
    pub exit_ready: bool,
    pub force_clear: bool,
    pub connection: Option<&'static str>,
    pub my_id: Option<i64>,
    pub users: HashMap<i64, String>,
    pub chats: HashMap<i64, ChatInfo>,
    pub folders: Vec<(i32, String)>,
    list_cache: HashMap<ListId, Vec<i64>>,
    loaded_lists: Vec<ListId>,
    pub focus: Pane,
    pub show_folders: bool,
    pub show_chats: bool,
    pub show_info: bool,
    pub folder_sel: usize,
    pub chat_sel: usize,
    pub filter: Option<Filter>,
    pub open_chat: Option<i64>,
    pub histories: HashMap<i64, History>,
    pub msg_sel: Option<usize>,
    pub msg_top: usize,
    pub bottom_offset: usize,
    pub msg_view_height: usize,
    pub want_older: bool,
    last_viewed: HashMap<i64, i64>,
    pub composer: Composer,
    pub pending: Vec<Chord>,
    pub popup: Option<Popup>,
    pub status: Option<(String, bool)>,
}

impl App {
    pub fn new(td: Td, creds: ApiCredentials, data_dir: PathBuf, keymap: Keymap) -> Self {
        Self {
            td,
            creds,
            data_dir,
            keymap,
            auth: Auth::Starting,
            login_mode: LoginMode::Qr,
            restart_after_close: false,
            login_input: TextInput::default(),
            auth_error: None,
            quitting: false,
            exit_ready: false,
            force_clear: false,
            connection: None,
            my_id: None,
            users: HashMap::new(),
            chats: HashMap::new(),
            folders: Vec::new(),
            list_cache: HashMap::new(),
            loaded_lists: Vec::new(),
            focus: Pane::Chats,
            show_folders: true,
            show_chats: true,
            show_info: false,
            folder_sel: 0,
            chat_sel: 0,
            filter: None,
            open_chat: None,
            histories: HashMap::new(),
            msg_sel: None,
            msg_top: 0,
            bottom_offset: 0,
            msg_view_height: 20,
            want_older: false,
            last_viewed: HashMap::new(),
            composer: Composer::default(),
            pending: Vec::new(),
            popup: None,
            status: None,
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error));
    }

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Input(TermEvent::Key(key)) if key.kind != KeyEventKind::Release => {
                self.on_key(key)
            }
            Event::Input(TermEvent::Paste(text)) => self.on_paste(&text),
            Event::Input(_) => {}
            Event::Update(client_id, update) => {
                if client_id == self.td.client_id {
                    self.on_update(*update);
                }
            }
            Event::History {
                chat_id,
                msgs,
                initial,
            } => self.on_history(chat_id, msgs, initial),
            Event::EditText {
                chat_id,
                message_id,
                text,
            } => {
                if self.open_chat == Some(chat_id) {
                    self.composer = Composer {
                        input: TextInput::with_text(&text),
                        reply_to: None,
                        edit: Some(message_id),
                        active: true,
                    };
                    self.focus = Pane::Messages;
                }
            }
            Event::AuthError(e) => {
                if self.login_mode == LoginMode::Qr && matches!(self.auth, Auth::Qr(None)) {
                    self.login_mode = LoginMode::Phone;
                    self.auth = Auth::Phone;
                    self.auth_error = Some(format!("QR login unavailable: {e}"));
                } else {
                    self.auth_error = Some(e);
                }
            }
            Event::Error(e) => self.set_status(e, true),
        }
    }

    fn on_paste(&mut self, text: &str) {
        if self.auth != Auth::Ready {
            self.login_input.insert_str(text.trim());
        } else if self.composer.active {
            self.composer.input.insert_str(text);
        } else if let Some(Popup::Switcher { input, sel }) = &mut self.popup {
            input.insert_str(&format::one_line(text));
            *sel = 0;
        } else if let Some(f) = self.filter.as_mut().filter(|f| f.active) {
            f.input.insert_str(&format::one_line(text));
        }
    }

    fn on_update(&mut self, update: Update) {
        match update {
            Update::AuthorizationState(s) => self.on_auth_state(s.authorization_state),
            Update::ConnectionState(s) => {
                self.connection = match s.state {
                    ConnectionState::WaitingForNetwork => Some("Waiting for network..."),
                    ConnectionState::ConnectingToProxy => Some("Connecting to proxy..."),
                    ConnectionState::Connecting => Some("Connecting..."),
                    ConnectionState::Updating => Some("Updating..."),
                    ConnectionState::Ready => None,
                }
            }
            Update::Option(o) => {
                if o.name == "my_id"
                    && let OptionValue::Integer(v) = o.value
                {
                    self.my_id = Some(v.value);
                }
            }
            Update::User(u) => {
                self.users.insert(u.user.id, td::user_name(&u.user));
            }
            Update::NewChat(c) => {
                let info = td::convert_chat(c.chat);
                self.chats.insert(info.id, info);
                self.list_cache.clear();
            }
            Update::ChatTitle(t) => {
                if let Some(c) = self.chats.get_mut(&t.chat_id) {
                    c.title = t.title;
                }
            }
            Update::ChatPosition(p) => {
                if let Some(c) = self.chats.get_mut(&p.chat_id) {
                    c.set_position(td::convert_position(&p.position));
                    self.list_cache.clear();
                }
            }
            Update::ChatLastMessage(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.last_message = u.last_message.map(td::convert_message);
                    for p in &u.positions {
                        c.set_position(td::convert_position(p));
                    }
                    self.list_cache.clear();
                }
            }
            Update::ChatReadInbox(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.unread = u.unread_count;
                    c.last_read_inbox = u.last_read_inbox_message_id;
                }
            }
            Update::ChatUnreadMentionCount(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.mentions = u.unread_mention_count;
                }
            }
            Update::ChatNotificationSettings(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.muted = !u.notification_settings.use_default_mute_for
                        && u.notification_settings.mute_for > 0;
                }
            }
            Update::ChatFolders(f) => {
                self.folders = f
                    .chat_folders
                    .into_iter()
                    .map(|f| (f.id, f.name.text.text))
                    .collect();
                self.list_cache.clear();
                self.folder_sel = self
                    .folder_sel
                    .min(self.folder_entries().len().saturating_sub(1));
            }
            Update::NewMessage(m) => {
                let msg = td::convert_message(m.message);
                let chat_id = msg.chat_id;
                if let Some(h) = self.histories.get_mut(&chat_id) {
                    let incoming = !msg.outgoing;
                    let following = self.msg_sel.is_none() && self.bottom_offset == 0;
                    h.upsert(msg);
                    if self.open_chat == Some(chat_id) && following {
                        let dropped = if h.len() > MAX_WINDOW {
                            h.trim_oldest(MAX_WINDOW)
                        } else {
                            0
                        };
                        self.msg_top = self.msg_top.saturating_sub(dropped);
                        if incoming {
                            self.mark_read();
                        }
                    }
                }
            }
            Update::MessageSendSucceeded(u) => {
                let msg = td::convert_message(u.message);
                if let Some(h) = self.histories.get_mut(&msg.chat_id) {
                    h.replace_id(u.old_message_id, msg);
                }
            }
            Update::MessageSendFailed(u) => {
                let msg = td::convert_message(u.message);
                let reason = td::friendly_error(&u.error);
                if let Some(h) = self.histories.get_mut(&msg.chat_id) {
                    h.replace_id(u.old_message_id, msg);
                }
                self.set_status(format!("send failed: {reason}"), true);
            }
            Update::MessageContent(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    let (media, body) = td::convert_content(u.new_content);
                    m.media = media;
                    m.body = body;
                }
            }
            Update::MessageEdited(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.edited = u.edit_date > 0;
                }
            }
            Update::DeleteMessages(u) => {
                if !u.is_permanent || u.from_cache {
                    return;
                }
                if let Some(h) = self.histories.get_mut(&u.chat_id) {
                    let removed = h.remove(&u.message_ids);
                    let len = h.len();
                    if self.open_chat == Some(u.chat_id) {
                        if let Some(sel) = self.msg_sel {
                            let shift = removed.iter().filter(|i| **i < sel).count();
                            let sel = sel.saturating_sub(shift);
                            self.msg_sel = if len == 0 {
                                None
                            } else {
                                Some(sel.min(len - 1))
                            };
                        }
                        self.msg_top = self.msg_top.min(len.saturating_sub(1));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_auth_state(&mut self, state: AuthorizationState) {
        self.auth_error = None;
        match state {
            AuthorizationState::WaitTdlibParameters => {
                let db = self.data_dir.join("db");
                let files = self.data_dir.join("files");
                self.td.set_parameters(
                    db.to_string_lossy().into_owned(),
                    files.to_string_lossy().into_owned(),
                    self.creds.api_id,
                    self.creds.api_hash.clone(),
                );
                self.auth = Auth::Starting;
            }
            AuthorizationState::WaitPhoneNumber => match self.login_mode {
                LoginMode::Qr => {
                    self.auth = Auth::Qr(None);
                    self.td.request_qr();
                }
                LoginMode::Phone => {
                    self.auth = Auth::Phone;
                    self.login_input.clear();
                }
            },
            AuthorizationState::WaitOtherDeviceConfirmation(c) => {
                if self.login_mode == LoginMode::Qr {
                    self.auth = Auth::Qr(Some(c.link));
                } else {
                    self.restart_login();
                }
            }
            AuthorizationState::WaitCode(c) => {
                let via = match c.code_info.r#type {
                    tdlib_rs::enums::AuthenticationCodeType::TelegramMessage(_) => {
                        "Telegram message"
                    }
                    tdlib_rs::enums::AuthenticationCodeType::Sms(_) => "SMS",
                    tdlib_rs::enums::AuthenticationCodeType::Call(_) => "phone call",
                    tdlib_rs::enums::AuthenticationCodeType::FlashCall(_) => "flash call",
                    tdlib_rs::enums::AuthenticationCodeType::Fragment(_) => "Fragment",
                    _ => "another method",
                };
                self.auth = Auth::Code {
                    phone: c.code_info.phone_number,
                    via: via.to_owned(),
                };
                self.login_input.clear();
            }
            AuthorizationState::WaitPassword(p) => {
                self.auth = Auth::Password {
                    hint: p.password_hint,
                };
                self.login_input.clear();
            }
            AuthorizationState::WaitEmailAddress(_) => {
                self.auth = Auth::Email;
                self.login_input.clear();
            }
            AuthorizationState::WaitEmailCode(c) => {
                self.auth = Auth::EmailCode {
                    pattern: c.code_info.email_address_pattern,
                };
                self.login_input.clear();
            }
            AuthorizationState::WaitRegistration(_) => {
                self.auth = Auth::Blocked(
                    "This phone number has no Telegram account. Sign up in an official app first."
                        .to_owned(),
                );
            }
            AuthorizationState::WaitPremiumPurchase(_) => {
                self.auth = Auth::Blocked(
                    "Telegram requires Premium to log in from this number. Use QR login instead."
                        .to_owned(),
                );
            }
            AuthorizationState::Ready => {
                self.auth = Auth::Ready;
                self.login_input.clear();
                self.ensure_list_loaded(ListId::Main);
            }
            AuthorizationState::LoggingOut | AuthorizationState::Closing => {
                self.auth = Auth::Closing;
            }
            AuthorizationState::Closed => {
                if self.quitting {
                    self.exit_ready = true;
                } else if self.restart_after_close {
                    self.restart_after_close = false;
                    self.reset_session();
                    self.auth = Auth::Starting;
                    self.td.recreate();
                    self.td.start();
                } else {
                    self.auth = Auth::Blocked(
                        "TDLib session closed (logged out elsewhere?). Restart to log in again."
                            .to_owned(),
                    );
                }
            }
        }
    }

    fn reset_session(&mut self) {
        self.connection = None;
        self.my_id = None;
        self.users.clear();
        self.chats.clear();
        self.folders.clear();
        self.list_cache.clear();
        self.loaded_lists.clear();
        self.focus = Pane::Chats;
        self.folder_sel = 0;
        self.chat_sel = 0;
        self.filter = None;
        self.open_chat = None;
        self.histories.clear();
        self.msg_sel = None;
        self.msg_top = 0;
        self.bottom_offset = 0;
        self.want_older = false;
        self.last_viewed.clear();
        self.composer = Composer::default();
        self.pending.clear();
        self.popup = None;
        self.status = None;
    }

    pub fn log_out(&mut self) {
        self.login_mode = LoginMode::Qr;
        self.restart_login();
    }

    fn restart_login(&mut self) {
        self.restart_after_close = true;
        self.auth = Auth::Starting;
        self.td.log_out();
    }

    pub fn request_quit(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        if let Some(chat) = self.open_chat {
            self.td.close_chat(chat);
        }
        self.td.close();
    }

    fn on_login_key(&mut self, key: KeyEvent) {
        match &self.auth {
            Auth::Qr(_) => match key.code {
                KeyCode::Char('p') => {
                    self.login_mode = LoginMode::Phone;
                    self.auth_error = None;
                    if matches!(self.auth, Auth::Qr(Some(_))) {
                        self.restart_login();
                    } else {
                        self.auth = Auth::Phone;
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
                _ => {}
            },
            Auth::Blocked(_) | Auth::Starting | Auth::Closing | Auth::Ready => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.request_quit();
                }
            }
            Auth::Phone => match key.code {
                KeyCode::Esc => {
                    self.login_mode = LoginMode::Qr;
                    self.auth = Auth::Qr(None);
                    self.auth_error = None;
                    self.td.request_qr();
                }
                KeyCode::Enter => {
                    let phone: String = self
                        .login_input
                        .text()
                        .chars()
                        .filter(|c| c.is_ascii_digit() || *c == '+')
                        .collect();
                    if phone.len() < 6 {
                        self.auth_error =
                            Some("enter the full number with country code".to_owned());
                    } else {
                        self.auth_error = None;
                        self.td.submit_phone(phone);
                    }
                }
                _ => {
                    self.login_input.handle_key(&key, false);
                }
            },
            Auth::Code { .. } | Auth::Password { .. } | Auth::Email | Auth::EmailCode { .. } => {
                if key.code == KeyCode::Enter {
                    let value = self.login_input.text().trim().to_owned();
                    if value.is_empty() {
                        return;
                    }
                    self.auth_error = None;
                    match &self.auth {
                        Auth::Code { .. } => self.td.submit_code(value),
                        Auth::Password { .. } => {
                            self.td.submit_password(self.login_input.text().to_owned())
                        }
                        Auth::Email => self.td.submit_email(value),
                        _ => self.td.submit_email_code(value),
                    }
                    self.login_input.clear();
                } else if key.code == KeyCode::Esc {
                    if self.login_input.text().is_empty()
                        && !matches!(self.auth, Auth::Password { .. })
                    {
                        self.login_mode = LoginMode::Phone;
                        self.auth_error = None;
                        self.restart_login();
                    }
                    self.login_input.clear();
                } else {
                    self.login_input.handle_key(&key, false);
                }
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return;
        }
        if self.quitting {
            return;
        }
        if self.auth != Auth::Ready {
            self.on_login_key(key);
            return;
        }
        if self.popup.is_some() {
            self.on_popup_key(key);
            return;
        }
        if self.composer.active {
            self.on_composer_key(key);
            return;
        }
        if self.filter.as_ref().is_some_and(|f| f.active) {
            self.on_filter_key(key);
            return;
        }
        self.status = None;
        let chord = Chord::from_event(&key);
        if !self.pending.is_empty() && key.code == KeyCode::Esc {
            self.pending.clear();
            return;
        }
        if self.pending.is_empty() {
            match key.code {
                KeyCode::Esc => {
                    if self.filter.take().is_none() {
                        self.msg_sel = None;
                        self.bottom_offset = 0;
                        self.mark_read();
                    }
                    return;
                }
                KeyCode::Enter => {
                    self.on_enter();
                    return;
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.dispatch(Action::SelectNext, chord);
                    return;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.dispatch(Action::SelectPrevious, chord);
                    return;
                }
                _ => {}
            }
        }
        match self.keymap.feed(&self.pending, chord) {
            Feed::Action(a) => {
                self.pending.clear();
                self.dispatch(a, chord);
            }
            Feed::Pending => self.pending.push(chord),
            Feed::Unbound => self.pending.clear(),
        }
    }

    fn dispatch(&mut self, action: Action, chord: Chord) {
        match action {
            Action::ClosePopup | Action::Quit => {
                if action == Action::Quit || self.keymap.actions_for(chord).contains(&Action::Quit)
                {
                    self.request_quit();
                }
            }
            Action::FocusFolderPane => {
                self.show_folders = true;
                self.focus = Pane::Folders;
            }
            Action::FocusChatPane => {
                self.show_chats = true;
                self.focus = Pane::Chats;
            }
            Action::FocusMessagePane => self.focus = Pane::Messages,
            Action::FocusInfoPane => {
                self.show_info = true;
                self.focus = Pane::Info;
            }
            Action::CycleFocusNext => self.cycle_focus(1),
            Action::CycleFocusPrevious => self.cycle_focus(3),
            Action::SelectNext => self.move_selection(1),
            Action::SelectPrevious => self.move_selection(-1),
            Action::HalfPageDown => self.move_selection(self.half_page()),
            Action::HalfPageUp => self.move_selection(-self.half_page()),
            Action::JumpTop => self.move_selection(isize::MIN / 2),
            Action::JumpBottom => self.move_selection(isize::MAX / 2),
            Action::ScrollViewportDown => self.scroll_viewport(1),
            Action::ScrollViewportUp => self.scroll_viewport(-1),
            Action::StartComposer => self.start_composer(None, None),
            Action::OpenPaneFilter => match self.focus {
                Pane::Folders | Pane::Chats => {
                    let keep = self.filter.as_ref().filter(|f| f.pane == self.focus);
                    let input = keep.map(|f| f.input.clone()).unwrap_or_default();
                    self.filter = Some(Filter {
                        pane: self.focus,
                        input,
                        active: true,
                    });
                }
                _ => self.set_status("message search arrives in a later stage", false),
            },
            Action::ReplyMessage => self.message_action(MenuAction::Reply),
            Action::DeleteMessage => self.message_action(MenuAction::Delete),
            Action::EditMessage => self.message_action(MenuAction::Edit),
            Action::OpenMessageUrl => self.message_action(MenuAction::OpenUrl),
            Action::ToggleFolderPane => {
                self.show_folders = !self.show_folders;
                self.fix_focus();
            }
            Action::ToggleChatPane => {
                self.show_chats = !self.show_chats;
                self.fix_focus();
            }
            Action::ToggleInfoPane => {
                self.show_info = !self.show_info;
                self.fix_focus();
            }
            Action::OpenPaneActions => self.open_action_menu(),
            Action::RefreshScreen => self.force_clear = true,
            Action::LogOut => self.popup = Some(Popup::ConfirmLogout),
            Action::ChatSwitcher => {
                self.popup = Some(Popup::Switcher {
                    input: TextInput::default(),
                    sel: 0,
                })
            }
        }
    }

    pub fn pane_visible(&self, pane: Pane) -> bool {
        match pane {
            Pane::Folders => self.show_folders,
            Pane::Chats => self.show_chats,
            Pane::Messages => true,
            Pane::Info => self.show_info,
        }
    }

    fn fix_focus(&mut self) {
        if !self.pane_visible(self.focus) {
            self.focus = Pane::Messages;
        }
    }

    fn cycle_focus(&mut self, step: usize) {
        let mut idx = self.focus.index();
        for _ in 0..4 {
            idx = (idx + step) % 4;
            if self.pane_visible(Pane::ORDER[idx]) {
                self.focus = Pane::ORDER[idx];
                return;
            }
        }
    }

    fn half_page(&self) -> isize {
        match self.focus {
            Pane::Messages => (self.msg_view_height / 6).max(1) as isize,
            _ => (self.msg_view_height / 2).max(1) as isize,
        }
    }

    pub fn folder_entries(&self) -> Vec<(ListId, String)> {
        let mut out = vec![(ListId::Main, "All chats".to_owned())];
        out.extend(
            self.folders
                .iter()
                .map(|(id, name)| (ListId::Folder(*id), name.clone())),
        );
        out.push((ListId::Archive, "Archive".to_owned()));
        if let Some(f) = self.filter.as_ref().filter(|f| f.pane == Pane::Folders) {
            out.retain(|(_, name)| model::fuzzy_score(f.input.text(), name).is_some());
        }
        out
    }

    pub fn current_list(&self) -> ListId {
        self.folder_entries()
            .get(self.folder_sel)
            .map_or(ListId::Main, |(id, _)| *id)
    }

    pub fn refresh_lists(&mut self) {
        let list = self.current_list();
        if !self.list_cache.contains_key(&list) {
            let ids = model::sorted_chat_ids(self.chats.values(), list);
            self.list_cache.insert(list, ids);
        }
    }

    pub fn visible_chats(&self) -> Vec<i64> {
        let list = self.current_list();
        let ids = self.list_cache.get(&list).cloned().unwrap_or_default();
        match self.filter.as_ref().filter(|f| f.pane == Pane::Chats) {
            Some(f) if !f.input.is_empty() => ids
                .into_iter()
                .filter(|id| {
                    self.chats
                        .get(id)
                        .is_some_and(|c| model::fuzzy_score(f.input.text(), &c.title).is_some())
                })
                .collect(),
            _ => ids,
        }
    }

    fn ensure_list_loaded(&mut self, list: ListId) {
        if !self.loaded_lists.contains(&list) {
            self.loaded_lists.push(list);
            self.td.load_chats(list);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Pane::Folders => {
                let n = self.folder_entries().len();
                self.folder_sel = step(self.folder_sel, delta, n);
                self.chat_sel = 0;
                let list = self.current_list();
                self.ensure_list_loaded(list);
                self.refresh_lists();
            }
            Pane::Chats => {
                self.refresh_lists();
                let n = self.visible_chats().len();
                self.chat_sel = step(self.chat_sel, delta, n);
                if n > 0 && self.chat_sel + 10 >= n {
                    self.td.load_chats(self.current_list());
                }
            }
            Pane::Messages => self.move_message_selection(delta),
            Pane::Info => {}
        }
    }

    fn move_message_selection(&mut self, delta: isize) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let len = self.histories.get(&chat_id).map_or(0, History::len);
        if len == 0 {
            return;
        }
        self.bottom_offset = 0;
        let cur = self.msg_sel.unwrap_or(len);
        let next = cur as isize + delta;
        if next >= len as isize {
            self.msg_sel = None;
        } else {
            self.msg_sel = Some(next.max(0) as usize);
        }
        self.mark_read();
        self.maybe_load_older();
    }

    fn scroll_viewport(&mut self, delta: isize) {
        if self.focus != Pane::Messages {
            return;
        }
        match self.msg_sel {
            None => {
                self.bottom_offset = if delta < 0 {
                    self.bottom_offset + 3
                } else {
                    self.bottom_offset.saturating_sub(3)
                };
                self.want_older = true;
            }
            Some(_) => {
                self.msg_top = (self.msg_top as isize + delta).max(0) as usize;
                self.maybe_load_older();
            }
        }
    }

    pub fn maybe_load_older(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        let near_top = self.msg_sel.map(|s| s.min(self.msg_top));
        let wanted = std::mem::take(&mut self.want_older);
        if (h.needs_older(near_top) || (wanted && !h.reached_start && !h.loading_older))
            && let Some(oldest) = h.oldest_id()
        {
            h.loading_older = true;
            self.td.load_history(chat_id, oldest, false);
        }
    }

    fn on_history(&mut self, chat_id: i64, msgs: Vec<Msg>, initial: bool) {
        let open = self.open_chat == Some(chat_id);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        let got = msgs.len();
        let added = h.merge_older(msgs);
        if open {
            if let Some(sel) = self.msg_sel.as_mut() {
                *sel += added;
            }
            if self.msg_sel.is_some() {
                self.msg_top += added;
            }
        }
        if initial
            && got > 0
            && h.len() < 30
            && let Some(oldest) = h.oldest_id()
        {
            h.loading_older = true;
            self.td.load_history(chat_id, oldest, true);
        }
        if open && initial {
            self.mark_read();
        }
    }

    fn mark_read(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get(&chat_id) else {
            return;
        };
        let target = match self.msg_sel {
            Some(i) => h.msgs().get(i).map(|m| m.id),
            None if self.bottom_offset == 0 => h.newest_id(),
            None => None,
        };
        let Some(id) = target else {
            return;
        };
        let unread = self
            .chats
            .get(&chat_id)
            .is_some_and(|c| c.unread > 0 || c.last_read_inbox < id);
        if unread && self.last_viewed.get(&chat_id).is_none_or(|v| *v < id) {
            self.last_viewed.insert(chat_id, id);
            self.td.view_messages(chat_id, vec![id]);
        }
    }

    fn on_enter(&mut self) {
        match self.focus {
            Pane::Folders => {
                if self.show_chats {
                    self.focus = Pane::Chats;
                }
            }
            Pane::Chats => {
                self.refresh_lists();
                if let Some(id) = self.visible_chats().get(self.chat_sel).copied() {
                    self.open(id);
                }
            }
            Pane::Messages => {
                if self.msg_sel.is_some() {
                    self.open_action_menu();
                }
            }
            Pane::Info => {}
        }
    }

    pub fn open(&mut self, chat_id: i64) {
        if self.open_chat != Some(chat_id) {
            if let Some(old) = self.open_chat.take() {
                self.td.close_chat(old);
                self.histories.remove(&old);
            }
            self.open_chat = Some(chat_id);
            self.td.open_chat(chat_id);
            let mut h = History::default();
            h.loading_older = true;
            self.histories.insert(chat_id, h);
            self.td.load_history(chat_id, 0, true);
            self.composer = Composer::default();
        }
        self.msg_sel = None;
        self.msg_top = 0;
        self.bottom_offset = 0;
        self.focus = Pane::Messages;
    }

    fn start_composer(&mut self, reply_to: Option<i64>, edit: Option<i64>) {
        if self.open_chat.is_none() {
            self.set_status("open a chat first", false);
            return;
        }
        if reply_to.is_some() || self.composer.edit.is_some() {
            self.composer.edit = None;
        }
        if reply_to.is_some() {
            self.composer.reply_to = reply_to;
        }
        if edit.is_some() {
            self.composer.edit = edit;
        }
        self.composer.active = true;
        self.focus = Pane::Messages;
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.composer.active = false;
                if self.composer.edit.take().is_some() {
                    self.composer.input.clear();
                }
                self.composer.reply_to = None;
            }
            KeyCode::Enter if key.modifiers.is_empty() => self.submit_composer(),
            _ => {
                self.composer.input.handle_key(&key, true);
            }
        }
    }

    fn submit_composer(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let text = self.composer.input.text().trim_end().to_owned();
        if text.trim().is_empty() {
            return;
        }
        self.td
            .send_text(chat_id, text, self.composer.reply_to, self.composer.edit);
        self.composer = Composer::default();
        self.msg_sel = None;
        self.bottom_offset = 0;
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        let Some(f) = self.filter.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.filter = None,
            KeyCode::Enter => {
                f.active = false;
                if f.input.is_empty() {
                    self.filter = None;
                }
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            _ => {
                if let Edit::Changed = f.input.handle_key(&key, false) {
                    match f.pane {
                        Pane::Folders => self.folder_sel = 0,
                        _ => self.chat_sel = 0,
                    }
                    self.refresh_lists();
                }
            }
        }
    }

    pub fn selected_message(&self) -> Option<&Msg> {
        let h = self.histories.get(&self.open_chat?)?;
        h.msgs().get(self.msg_sel?)
    }

    fn message_action(&mut self, action: MenuAction) {
        if self.focus != Pane::Messages {
            return;
        }
        let Some(msg) = self.selected_message().cloned() else {
            self.set_status("select a message first (k)", false);
            return;
        };
        match action {
            MenuAction::Reply => self.start_composer(Some(msg.id), None),
            MenuAction::Edit => {
                if !msg.outgoing {
                    self.set_status("you can only edit your own messages", false);
                } else {
                    self.td
                        .fetch_edit_text(msg.chat_id, msg.id, msg.body.clone());
                }
            }
            MenuAction::Delete => {
                self.popup = Some(Popup::ConfirmDelete {
                    chat_id: msg.chat_id,
                    message_id: msg.id,
                })
            }
            MenuAction::OpenUrl => match format::urls(&msg.body).first() {
                Some(url) => open_url(&format::normalize_url(url)).unwrap_or_else(|e| {
                    self.set_status(format!("open url: {e}"), true);
                }),
                None => self.set_status("no link in this message", false),
            },
            _ => {}
        }
    }

    fn open_action_menu(&mut self) {
        let items = match self.focus {
            Pane::Messages if self.msg_sel.is_some() => vec![
                MenuItem {
                    key: 'R',
                    label: "Reply",
                    action: MenuAction::Reply,
                },
                MenuItem {
                    key: 'e',
                    label: "Edit",
                    action: MenuAction::Edit,
                },
                MenuItem {
                    key: 'd',
                    label: "Delete",
                    action: MenuAction::Delete,
                },
                MenuItem {
                    key: 'o',
                    label: "Open link",
                    action: MenuAction::OpenUrl,
                },
            ],
            Pane::Chats => vec![
                MenuItem {
                    key: 'o',
                    label: "Open chat",
                    action: MenuAction::OpenChat,
                },
                MenuItem {
                    key: 'm',
                    label: "Mark as read",
                    action: MenuAction::MarkRead,
                },
            ],
            Pane::Folders => vec![MenuItem {
                key: 'l',
                label: "Load more chats",
                action: MenuAction::LoadMore,
            }],
            _ => return,
        };
        let title = match self.focus {
            Pane::Messages => "Message actions",
            Pane::Chats => "Chat actions",
            _ => "Folder actions",
        };
        self.popup = Some(Popup::Menu {
            title,
            items,
            sel: 0,
        });
    }

    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Reply | MenuAction::Edit | MenuAction::Delete | MenuAction::OpenUrl => {
                self.message_action(action)
            }
            MenuAction::OpenChat => self.on_enter(),
            MenuAction::MarkRead => {
                self.refresh_lists();
                if let Some(chat) = self
                    .visible_chats()
                    .get(self.chat_sel)
                    .and_then(|id| self.chats.get(id))
                    && let Some(last) = &chat.last_message
                {
                    self.td.view_messages(chat.id, vec![last.id]);
                }
            }
            MenuAction::LoadMore => self.td.load_chats(self.current_list()),
        }
    }

    pub fn switcher_results(&self, query: &str) -> Vec<i64> {
        let mut scored: Vec<(i32, i64, i64)> = self
            .chats
            .values()
            .filter(|c| !c.positions.is_empty())
            .filter_map(|c| {
                let order = c.position_in(ListId::Main).map_or(0, |p| p.order);
                model::fuzzy_score(query, &c.title).map(|s| (s, order, c.id))
            })
            .collect();
        scored.sort_unstable_by(|a, b| b.cmp(a));
        scored.into_iter().take(50).map(|(_, _, id)| id).collect()
    }

    fn on_popup_key(&mut self, key: KeyEvent) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match popup {
            Popup::Menu { items, sel, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => *sel = (*sel + 1) % items.len(),
                KeyCode::Char('n') if ctrl => *sel = (*sel + 1) % items.len(),
                KeyCode::Char('k') | KeyCode::Up => *sel = (*sel + items.len() - 1) % items.len(),
                KeyCode::Char('p') if ctrl => *sel = (*sel + items.len() - 1) % items.len(),
                KeyCode::Enter => {
                    let action = items[*sel].action;
                    self.popup = None;
                    self.run_menu_action(action);
                }
                KeyCode::Char(c) => {
                    if let Some(item) = items.iter().find(|i| i.key == c) {
                        let action = item.action;
                        self.popup = None;
                        self.run_menu_action(action);
                    }
                }
                _ => {}
            },
            Popup::ConfirmDelete {
                chat_id,
                message_id,
            } => {
                let (chat_id, message_id) = (*chat_id, *message_id);
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        self.popup = None;
                        self.td.delete_message(chat_id, message_id, true);
                    }
                    KeyCode::Char('m') => {
                        self.popup = None;
                        self.td.delete_message(chat_id, message_id, false);
                    }
                    KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => self.popup = None,
                    _ => {}
                }
            }
            Popup::ConfirmLogout => match key.code {
                KeyCode::Char('y') => {
                    self.popup = None;
                    self.log_out();
                }
                KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                    self.popup = None
                }
                _ => {}
            },
            Popup::Switcher { input, sel } => match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down => *sel += 1,
                KeyCode::Char('n') if ctrl => *sel += 1,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Char('p') if ctrl => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    let query = input.text().to_owned();
                    let sel = *sel;
                    let results = self.switcher_results(&query);
                    self.popup = None;
                    if let Some(id) = results.get(sel.min(results.len().saturating_sub(1))) {
                        self.open(*id);
                    }
                }
                _ => {
                    if let Edit::Changed = input.handle_key(&key, false) {
                        *sel = 0;
                    }
                }
            },
        }
    }

    pub fn sender_name(&self, sender: Sender) -> String {
        match sender {
            Sender::User(id) => self
                .users
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("user {id}")),
            Sender::Chat(id) => self
                .chats
                .get(&id)
                .map_or_else(|| format!("chat {id}"), |c| c.title.clone()),
        }
    }

    pub fn me_name(&self) -> Option<String> {
        self.my_id.and_then(|id| self.users.get(&id).cloned())
    }

    pub fn after_draw(&mut self) {
        if self.want_older {
            self.maybe_load_older();
        }
    }
}

fn step(cur: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as isize)
        .saturating_add(delta)
        .clamp(0, len as isize - 1) as usize
}

fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || child.wait());
        })
        .map_err(|e| format!("xdg-open: {e}"))
}
