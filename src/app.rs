use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tdlib_rs::enums::{AuthorizationState, ConnectionState, OptionValue, Update};

use crate::bots::{self, ButtonAction, Command, Keyboard};
use crate::config::{ApiCredentials, BorderStyle, MessageLayout, OpenFocus, Settings, ThemeChoice};
use crate::format;
use crate::input::{Edit, TextInput};
use crate::keymap::{Action, Chord, Feed, Keymap};
use crate::media::Media;
use crate::model::{
    self, Attachment, ChatInfo, ChatKind, Decode, FileRef, History, InboxItem, ListId, MAX_WINDOW,
    MediaKind, Msg, Profile, ProfileTarget, Sender, UserMeta,
};
use crate::notify;
use crate::player;
use crate::presence::{self, Activity};
use crate::stickers::{self, Library, Tab};
use crate::td::{self, Event, Td};
use crate::theme::Theme;
use crate::upload::{self, Clip, DirEntry, Pending};
use crate::voice::{self, Stop};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingOpen {
    chat_id: i64,
    last_read: i64,
    unread: i32,
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
    ReadMarks,
    TopicReadMarks,
    LoadMore,
    Play,
    View,
    Save,
    OpenExternal,
    CancelSend,
    Copy,
    React,
    Forward,
    Pin,
    GoToReply,
    SenderProfile,
    ReactionList,
    Buttons,
    ChatProfile,
    ShowPinned,
    SearchChat,
    OpenTopic,
    TopicMarkRead,
    TopicMute,
    TopicPin,
    TopicClose,
    TopicHideGeneral,
    TopicLink,
    NewTopic,
    SetTtl,
    CloseSecret,
    DeleteSecret,
    Details,
    EditHistory,
    FromSender,
    CopyQuote,
    ReplyQuote,
    Translate,
    SaveMessage,
    CopyImage,
    Members,
    EventLog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    Mention,
    Reaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Chat(i64),
    Topic(i64, i32),
    Loading(i64),
}

impl Row {
    pub fn chat(self) -> i64 {
        match self {
            Row::Chat(c) | Row::Topic(c, _) | Row::Loading(c) => c,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileJob {
    Play(MediaKind, (i64, usize)),
    Open,
    Save(String),
    CopyImage,
}

pub struct Playing {
    pub token: u64,
    pub target: (i64, usize),
    pub label: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
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
        message_ids: Vec<i64>,
    },
    Viewer {
        chat_id: i64,
        items: Vec<(i64, usize)>,
        pos: usize,
    },
    Switcher {
        input: TextInput,
        sel: usize,
        forward: Option<(i64, Vec<i64>)>,
    },
    ConfirmLogout,
    FilePicker(FilePicker),
    Stickers(stickers::Picker),
    Profile(ProfileView),
    Reactions(ReactionPicker),
    ReactionList {
        chat_id: i64,
        msg_id: i64,
        items: Option<Vec<(Sender, String)>>,
        sel: usize,
    },
    Search(SearchView),
    Buttons(ButtonsView),
    ConfirmUrl(String),
    Options {
        sel: usize,
    },
    Inbox {
        sel: usize,
    },
    NewTopic {
        chat_id: i64,
        input: TextInput,
    },
    Ttl {
        chat_id: i64,
        choices: Vec<i32>,
        sel: usize,
    },
    ConfirmSecret {
        chat_id: i64,
        secret_id: i32,
        delete: bool,
    },
    Details {
        chat_id: i64,
        msg_id: i64,
        flags: Option<Result<td::Flags, String>>,
        scroll: u16,
    },
    EditHistory {
        chat_id: i64,
        msg_id: i64,
        scroll: u16,
    },
    ConfirmClearDeleted,
    QuotePick {
        chat_id: i64,
        msg_id: i64,
        input: TextInput,
    },
    Translate {
        chat_id: i64,
        msg_id: i64,
        lang: String,
        result: Option<Result<String, String>>,
        scroll: u16,
    },
    Members(MembersView),
    EventLog(LogView),
}

pub struct MembersView {
    pub chat_id: i64,
    pub supergroup_id: i64,
    pub filter: crate::admin::MemberFilter,
    pub input: TextInput,
    pub page: Option<Result<crate::admin::MemberPage, String>>,
    pub sel: usize,
    pub token: u64,
}

pub struct LogView {
    pub chat_id: i64,
    pub entries: Vec<crate::admin::LogEntry>,
    pub sel: usize,
    pub loading: bool,
    pub done: bool,
    pub error: Option<String>,
}

pub struct ProfileView {
    pub target: ProfileTarget,
    pub data: Option<Profile>,
    pub error: Option<String>,
    pub scroll: u16,
}

pub struct ReactionPicker {
    pub chat_id: i64,
    pub msg_id: i64,
    pub emojis: Vec<String>,
    pub sel: usize,
    pub loading: bool,
    pub note: Option<String>,
}

pub struct SearchView {
    pub chat_id: Option<i64>,
    pub pinned: bool,
    pub sender: Option<Sender>,
    pub input: TextInput,
    pub searched_for: Option<String>,
    pub results: Vec<Msg>,
    pub sel: usize,
    pub token: u64,
    pub loading: bool,
    pub error: Option<String>,
}

impl SearchView {
    pub fn listing(&self) -> bool {
        self.pinned || self.sender.is_some()
    }
}

pub struct ButtonsView {
    pub chat_id: i64,
    pub msg_id: i64,
    pub keyboard: Keyboard,
    pub sel: usize,
}

pub const OPTIONS: [(&str, &str); 26] = [
    (
        "Desktop notifications",
        "notify-send for new messages in unmuted chats and mentions",
    ),
    (
        "Don't mark as read",
        "never send read receipts automatically; Space m marks a chat read",
    ),
    (
        "Ghost mode",
        "never online, no typing status, no read receipts",
    ),
    (
        "Notification sound",
        "play a sound with notifications; [notifications] sound_file sets a custom file",
    ),
    (
        "Sound volume",
        "h/l or ←/→ change by 10, Enter/Space steps up",
    ),
    (
        "Border style",
        "rounded, plain, thick or double borders on panes and popups",
    ),
    (
        "Theme",
        "terminal (follows the terminal's colours), default, high-contrast, light, mono or custom (theme.toml, reloaded on save)",
    ),
    (
        "Write theme.toml",
        "save the current theme as theme.toml to start a custom one",
    ),
    (
        "Open chat focus",
        "smart: composer if unread fit on screen, else first unread; composer; messages",
    ),
    (
        "After send: next unread chat",
        "after sending, open the next unread chat when this one has nothing unread",
    ),
    (
        "Message layout",
        "classic, cards, compact, minimal, split or bubbles; theme.toml layout.messages sets the base",
    ),
    (
        "Avatar shape",
        "square, rounded or circle avatars; theme.toml layout.avatar_shape sets the base",
    ),
    (
        "Show IDs",
        "user, chat and message IDs in profiles and chat info (message details always show them)",
    ),
    (
        "Times with seconds",
        "show seconds in every message time; theme.toml layout.seconds sets the base",
    ),
    (
        "Exact numbers",
        "1234 instead of 1.2K for views, forwards, members and unread counters",
    ),
    (
        "Keep deleted messages",
        "keep messages deleted by others and earlier versions of edited ones on this device only; never in protected or secret chats",
    ),
    (
        "Clear saved deleted messages",
        "erase everything kept by the option above",
    ),
    (
        "Hide messages from blocked users",
        "in groups, show a one-line placeholder instead; Enter on it shows the message",
    ),
    (
        "Hide stories",
        "hide shared stories and story mentions in chats",
    ),
    (
        "Folder counters",
        "unread chats, unread messages or off on the folder list",
    ),
    (
        "Folder counters: unmuted only",
        "count only chats that are not muted",
    ),
    (
        "Hide \"All chats\"",
        "drop the All chats entry from the folder list when you have folders",
    ),
    (
        "Translate to",
        "target language for t in the message menu (translated by Telegram); h/l change",
    ),
    (
        "Send photos as files",
        "attach photos as original files by default; f in the attachment list still switches one",
    ),
    (
        "Save target",
        "S in the message menu forwards there; Enter sets the open chat, again resets to Saved Messages",
    ),
    (
        "Hide my phone number",
        "show \"hidden\" instead of your own number in your profile, handy for screenshots",
    ),
];

pub const OPTION_GROUPS: [(&str, &[usize]); 5] = [
    ("Notifications", &[0, SOUND_OPTION, VOLUME_OPTION]),
    (
        "Privacy",
        &[
            1,
            2,
            KEEP_DELETED_OPTION,
            CLEAR_DELETED_OPTION,
            HIDE_BLOCKED_OPTION,
            HIDE_STORIES_OPTION,
            HIDE_PHONE_OPTION,
        ],
    ),
    (
        "Appearance",
        &[
            THEME_OPTION,
            BORDER_OPTION,
            LAYOUT_OPTION,
            AVATAR_OPTION,
            WRITE_THEME_OPTION,
            SECONDS_OPTION,
            SHOW_IDS_OPTION,
            EXACT_OPTION,
            FOLDER_COUNTER_OPTION,
            FOLDER_UNMUTED_OPTION,
            HIDE_ALL_CHATS_OPTION,
        ],
    ),
    (
        "Chats",
        &[TRANSLATE_OPTION, PHOTOS_AS_FILES_OPTION, SAVE_TARGET_OPTION],
    ),
    ("Behaviour", &[OPEN_FOCUS_OPTION, AFTER_SEND_OPTION]),
];

pub fn option_order() -> impl Iterator<Item = usize> {
    OPTION_GROUPS
        .iter()
        .flat_map(|(_, items)| items.iter().copied())
}

pub fn option_key(index: usize) -> char {
    match index {
        0..=8 => (b'1' + index as u8) as char,
        9 => '0',
        AVATAR_OPTION => 'a',
        SHOW_IDS_OPTION => 'i',
        SECONDS_OPTION => 's',
        EXACT_OPTION => 'n',
        KEEP_DELETED_OPTION => 'd',
        CLEAR_DELETED_OPTION => 'c',
        HIDE_BLOCKED_OPTION => 'b',
        HIDE_STORIES_OPTION => 't',
        FOLDER_COUNTER_OPTION => 'f',
        FOLDER_UNMUTED_OPTION => 'u',
        HIDE_ALL_CHATS_OPTION => 'x',
        TRANSLATE_OPTION => 'g',
        PHOTOS_AS_FILES_OPTION => 'p',
        SAVE_TARGET_OPTION => 'v',
        HIDE_PHONE_OPTION => 'z',
        _ => 'm',
    }
}

pub fn option_for_key(c: char) -> Option<usize> {
    if c.is_ascii_digit() {
        return Some((c as usize + 9 - '0' as usize) % 10);
    }
    (0..OPTIONS.len()).find(|i| *i >= 10 && option_key(*i) == c)
}

pub const SOUND_OPTION: usize = 3;
pub const VOLUME_OPTION: usize = 4;
pub const BORDER_OPTION: usize = 5;
pub const THEME_OPTION: usize = 6;
pub const WRITE_THEME_OPTION: usize = 7;
pub const OPEN_FOCUS_OPTION: usize = 8;
pub const AFTER_SEND_OPTION: usize = 9;
pub const LAYOUT_OPTION: usize = 10;
pub const AVATAR_OPTION: usize = 11;
pub const SHOW_IDS_OPTION: usize = 12;
pub const SECONDS_OPTION: usize = 13;
pub const EXACT_OPTION: usize = 14;
pub const KEEP_DELETED_OPTION: usize = 15;
pub const CLEAR_DELETED_OPTION: usize = 16;
pub const HIDE_BLOCKED_OPTION: usize = 17;
pub const HIDE_STORIES_OPTION: usize = 18;
pub const QUOTE_MAX: usize = 1024;
pub const FOLDER_COUNTER_OPTION: usize = 19;
pub const FOLDER_UNMUTED_OPTION: usize = 20;
pub const HIDE_ALL_CHATS_OPTION: usize = 21;
pub const TRANSLATE_OPTION: usize = 22;
pub const PHOTOS_AS_FILES_OPTION: usize = 23;
pub const SAVE_TARGET_OPTION: usize = 24;
pub const HIDE_PHONE_OPTION: usize = 25;

pub struct FilePicker {
    pub input: TextInput,
    pub sel: usize,
    pub dir: PathBuf,
    pub entries: Vec<DirEntry>,
    pub error: Option<String>,
    requested: Option<PathBuf>,
}

impl FilePicker {
    pub fn query(
        &self,
        cwd: &std::path::Path,
        home: &std::path::Path,
    ) -> (PathBuf, String, String) {
        upload::split_query(self.input.text(), cwd, home)
    }

    pub fn matches(&self, cwd: &std::path::Path, home: &std::path::Path) -> Vec<usize> {
        let (dir, _, filter) = self.query(cwd, home);
        if dir != self.dir {
            return Vec::new();
        }
        upload::filter_entries(&self.entries, &filter)
    }
}

pub struct Recording {
    pub started: Instant,
    pub chat_id: i64,
    pub topic: Option<i32>,
    pub reply_to: Option<i64>,
    pub stopping: bool,
    pub stop: Option<tokio::sync::oneshot::Sender<Stop>>,
    pub levels: voice::Levels,
}

#[derive(Default)]
pub struct Composer {
    pub input: TextInput,
    pub reply_to: Option<i64>,
    pub edit: Option<i64>,
    pub active: bool,
    pub attachments: Vec<Pending>,
    pub att_sel: Option<usize>,
    pub pasting: bool,
    pub quote: Option<(String, i32)>,
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
    pub folder_rows: Cell<usize>,
    pub chat_rows: Cell<usize>,
    pub popup_rows: Cell<usize>,
    pub msg_view_width: usize,
    pub want_older: bool,
    pub custom_emoji: HashMap<i64, String>,
    custom_emoji_asked: std::collections::HashSet<i64>,
    pub want_emoji: Vec<i64>,
    last_viewed: HashMap<i64, i64>,
    pub composer: Composer,
    pub pending: Vec<Chord>,
    pub popup: Option<Popup>,
    pub status: Option<(String, bool)>,
    pub media: Media,
    pub playing: Option<Playing>,
    next_token: u64,
    file_jobs: Vec<(i32, FileJob)>,
    pub anim_start: Instant,
    pub animating: bool,
    uploads_dir: PathBuf,
    pub cwd: PathBuf,
    pub home: PathBuf,
    picker_last: String,
    pub recording: Option<Recording>,
    pub stickers: Library,
    pub settings: Settings,
    pub settings_path: Option<PathBuf>,
    pub theme_path: Option<PathBuf>,
    theme_base: Rc<Theme>,
    theme_gen: u64,
    frame_theme: Option<(u64, Rc<Theme>)>,
    pub palette_query: bool,
    pub user_meta: HashMap<i64, UserMeta>,
    pub typing: HashMap<i64, Vec<presence::Typing>>,
    pub term_focused: bool,
    online_sent: Option<bool>,
    typing_sent: Option<(i64, Instant)>,
    pub bot_commands: HashMap<i64, Vec<Command>>,
    pub cmd_sel: usize,
    reply_kb_ids: HashMap<i64, i64>,
    reply_kb: HashMap<i64, Option<Keyboard>>,
    pub inbox: std::collections::VecDeque<InboxItem>,
    notify_broken: bool,
    pending_jump: Option<(i64, i64)>,
    jump_inflight: Option<(i64, i64)>,
    pub unread_counts: HashMap<ListId, i32>,
    pub unread_unmuted: HashMap<ListId, i32>,
    pub unread_messages: HashMap<ListId, (i32, i32)>,
    scope_muted: [bool; 3],
    pub pinned_msg: HashMap<i64, Msg>,
    sound_broken: bool,
    pub sound_env: Option<notify::SoundEnv>,
    pub layout_cache: crate::ui::LayoutCache,
    pub lang: format::Lang,
    pending_open: Option<PendingOpen>,
    pub groups: HashMap<i64, model::GroupMeta>,
    pub secret_chats: HashMap<i32, model::SecretChat>,
    pub topics: HashMap<i64, model::TopicList>,
    pub expanded: Option<i64>,
    pub open_topic: Option<i32>,
    topic_select: Option<i64>,
    topic_unread: Option<i64>,
    pub archive: crate::archive::Archive,
    pub blocked: std::collections::HashSet<Sender>,
    blocked_loaded: bool,
    pub revealed: std::collections::HashSet<(i64, i64)>,
}

impl App {
    pub fn new(
        td: Td,
        creds: ApiCredentials,
        data_dir: PathBuf,
        keymap: Keymap,
        picker: ratatui_image::picker::Picker,
    ) -> Self {
        let media = Media::new(picker, td.sender());
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
        let uploads_dir = data_dir.join("uploads");
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
            folder_rows: Cell::new(10),
            chat_rows: Cell::new(10),
            popup_rows: Cell::new(10),
            msg_view_width: 80,
            want_older: false,
            custom_emoji: HashMap::new(),
            custom_emoji_asked: std::collections::HashSet::new(),
            want_emoji: Vec::new(),
            last_viewed: HashMap::new(),
            composer: Composer::default(),
            pending: Vec::new(),
            popup: None,
            status: None,
            media,
            playing: None,
            next_token: 0,
            file_jobs: Vec::new(),
            anim_start: Instant::now(),
            animating: false,
            uploads_dir,
            cwd,
            home,
            picker_last: String::new(),
            recording: None,
            stickers: Library::default(),
            settings: Settings::default(),
            settings_path: None,
            theme_path: None,
            theme_base: Rc::new(Theme::default()),
            theme_gen: 0,
            frame_theme: None,
            palette_query: false,
            user_meta: HashMap::new(),
            typing: HashMap::new(),
            term_focused: true,
            online_sent: None,
            typing_sent: None,
            bot_commands: HashMap::new(),
            cmd_sel: 0,
            reply_kb_ids: HashMap::new(),
            reply_kb: HashMap::new(),
            inbox: std::collections::VecDeque::new(),
            notify_broken: false,
            pending_jump: None,
            jump_inflight: None,
            unread_counts: HashMap::new(),
            unread_unmuted: HashMap::new(),
            unread_messages: HashMap::new(),
            scope_muted: [false; 3],
            pinned_msg: HashMap::new(),
            sound_broken: false,
            sound_env: None,
            layout_cache: crate::ui::LayoutCache::default(),
            lang: if cfg!(test) {
                format::Lang::En
            } else {
                format::Lang::detect()
            },
            pending_open: None,
            groups: HashMap::new(),
            secret_chats: HashMap::new(),
            topics: HashMap::new(),
            expanded: None,
            open_topic: None,
            topic_select: None,
            topic_unread: None,
            archive: crate::archive::Archive::memory(crate::config::KEEP_DELETED_LIMIT),
            blocked: std::collections::HashSet::new(),
            blocked_loaded: false,
            revealed: std::collections::HashSet::new(),
        }
    }

    pub fn uploads_dir(&self) -> &std::path::Path {
        &self.uploads_dir
    }

    pub fn tick_interval(&self) -> Option<std::time::Duration> {
        if self.animating || self.recording.as_ref().is_some_and(|r| !r.stopping) {
            Some(std::time::Duration::from_millis(50))
        } else if !self.typing.is_empty() {
            Some(std::time::Duration::from_millis(500))
        } else {
            None
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error));
    }

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Input(TermEvent::Key(key)) if key.kind != KeyEventKind::Release => {
                self.handle_key(key)
            }
            Event::Input(TermEvent::Paste(text)) => self.on_paste(&text),
            Event::Input(TermEvent::FocusGained) => {
                self.term_focused = true;
                self.apply_online();
            }
            Event::Input(TermEvent::FocusLost) => {
                self.term_focused = false;
                self.apply_online();
            }
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
                epoch,
            } => self.on_history(chat_id, msgs, initial, epoch),
            Event::HistoryFailed {
                chat_id,
                epoch,
                error,
            } => {
                if let Some(h) = self.histories.get_mut(&chat_id)
                    && h.epoch == epoch
                {
                    h.older_failed(std::time::Instant::now());
                }
                self.set_status(format!("load history: {error}"), true);
            }
            Event::NewerHistory {
                chat_id,
                msgs,
                epoch,
            } => self.on_newer_history(chat_id, msgs, epoch),
            Event::HistoryAround {
                chat_id,
                target,
                result,
            } => self.on_history_around(chat_id, target, result),
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
                        ..Composer::default()
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
            Event::File(f) => self.on_file(f),
            Event::DownloadFailed { file_id, error } => {
                self.media.download_failed(file_id, error.clone());
                let had_job = self.file_jobs.iter().any(|(id, _)| *id == file_id);
                self.file_jobs.retain(|(id, _)| *id != file_id);
                if had_job {
                    self.set_status(format!("download failed: {error}"), true);
                }
            }
            Event::Image(done) => self.media.on_done(done),
            Event::Player { token, result } => {
                if self.playing.as_ref().is_some_and(|p| p.token == token) {
                    self.playing = None;
                }
                if let Err(e) = result {
                    self.set_status(e, true);
                }
            }
            Event::Saved(result) => match result {
                Ok(path) => self.set_status(format!("saved to {path}"), false),
                Err(e) => self.set_status(format!("save failed: {e}"), true),
            },
            Event::Notice(text) => self.set_status(text, false),
            Event::Clipboard(result) => self.on_clipboard(result),
            Event::DirListing { dir, result } => self.on_dir_listing(dir, result),
            Event::Voice(result) => self.on_voice(result),
            Event::Stickers(data) => self.on_sticker_data(data),
            Event::StickersFailed(e) => self.set_status(format!("stickers: {e}"), true),
            Event::NotifyFailed(e) => {
                self.notify_broken = true;
                self.set_status(e, true);
            }
            Event::Profile(target, result) => {
                if let Some(Popup::Profile(p)) = &mut self.popup
                    && p.target == target
                {
                    match result {
                        Ok(data) => p.data = Some(data),
                        Err(e) => p.error = Some(e),
                    }
                }
            }
            Event::BotCommands { chat_id, result } => match result {
                Ok(cmds) => {
                    self.bot_commands.insert(chat_id, cmds);
                }
                Err(e) => self.set_status(format!("bot commands: {e}"), true),
            },
            Event::CallbackAnswer(result) => match result {
                Ok(a) => {
                    if !a.url.is_empty() {
                        self.popup = Some(Popup::ConfirmUrl(a.url));
                    }
                    if !a.text.is_empty() {
                        self.set_status(format!("🤖 {}", a.text), a.alert);
                    } else if self.status.as_ref().is_some_and(|(t, _)| t == WAITING_BOT) {
                        self.status = None;
                    }
                }
                Err(e) => self.set_status(format!("button: {e}"), true),
            },
            Event::AvailableReactions {
                chat_id,
                message_id,
                result,
            } => {
                if let Some(Popup::Reactions(r)) = &mut self.popup
                    && r.chat_id == chat_id
                    && r.msg_id == message_id
                {
                    r.loading = false;
                    match result {
                        Ok(c) => {
                            r.emojis = c.emojis;
                            r.note = c.note;
                        }
                        Err(e) => r.note = Some(e),
                    }
                }
            }
            Event::CustomEmoji(result) => match result {
                Ok(names) => self.custom_emoji.extend(names),
                Err(e) => self.set_status(format!("custom emoji: {e}"), true),
            },
            Event::ReactionList {
                chat_id,
                message_id,
                result,
            } => {
                if let Some(Popup::ReactionList {
                    chat_id: c,
                    msg_id,
                    items,
                    ..
                }) = &mut self.popup
                    && *c == chat_id
                    && *msg_id == message_id
                {
                    match result {
                        Ok(list) => *items = Some(list),
                        Err(e) => {
                            self.popup = None;
                            self.set_status(format!("reactions: {e}"), true);
                        }
                    }
                }
            }
            Event::Properties {
                chat_id,
                message_id,
                result,
            } => {
                if let Some(Popup::Details {
                    chat_id: c,
                    msg_id,
                    flags,
                    ..
                }) = &mut self.popup
                    && *c == chat_id
                    && *msg_id == message_id
                {
                    *flags = Some(result);
                }
            }
            Event::Blocked(result) => match result {
                Ok(list) => self.blocked = list.into_iter().collect(),
                Err(e) => {
                    self.blocked_loaded = false;
                    self.set_status(format!("blocked users not loaded: {e}"), true);
                }
            },
            Event::Search { token, result } => {
                if let Some(Popup::Search(sv)) = &mut self.popup
                    && sv.token == token
                {
                    sv.loading = false;
                    sv.sel = 0;
                    match result {
                        Ok(list) => {
                            sv.results = list;
                            sv.error = None;
                        }
                        Err(e) => {
                            sv.results.clear();
                            sv.error = Some(e);
                        }
                    }
                }
            }
            Event::Mention { chat_id, result } => match result {
                Ok(Some((id, topic))) => {
                    self.status = None;
                    self.jump_to_in(chat_id, id, topic);
                }
                Ok(None) => self.set_status("no unread mentions found", false),
                Err(e) => self.set_status(format!("find mention: {e}"), true),
            },
            Event::Reaction { chat_id, result } => match result {
                Ok(Some((id, topic))) => {
                    self.status = None;
                    self.jump_to_in(chat_id, id, topic);
                }
                Ok(None) => self.set_status("no unread reactions found", false),
                Err(e) => self.set_status(format!("find reaction: {e}"), true),
            },
            Event::Fetched(msg) => {
                if self.reply_kb_ids.get(&msg.chat_id) == Some(&msg.id) {
                    self.reply_kb
                        .insert(msg.chat_id, msg.keyboard.filter(|k| !k.inline));
                }
            }
            Event::Pinned { chat_id, result } => match result {
                Ok(Some(m)) => {
                    self.pinned_msg.insert(chat_id, m);
                }
                Ok(None) => {
                    self.pinned_msg.remove(&chat_id);
                }
                Err(e) => self.set_status(format!("pinned message: {e}"), true),
            },
            Event::SoundFailed(e) => {
                self.sound_broken = true;
                self.set_status(e, true);
            }
            Event::Translated {
                chat_id,
                message_id,
                result,
            } => self.on_translated(chat_id, message_id, result),
            Event::Members { token, result } => {
                if let Some(Popup::Members(mv)) = &mut self.popup
                    && mv.token == token
                {
                    mv.page = Some(result);
                    mv.sel = 0;
                }
            }
            Event::ActionLog {
                chat_id,
                from,
                result,
            } => {
                if let Some(Popup::EventLog(lv)) = &mut self.popup
                    && lv.chat_id == chat_id
                {
                    lv.loading = false;
                    match result {
                        Ok(entries) => {
                            lv.done = entries.len() < crate::admin::LOG_PAGE;
                            if from == 0 {
                                lv.entries = entries;
                                lv.sel = 0;
                            } else {
                                lv.entries.extend(entries);
                            }
                            lv.error = None;
                        }
                        Err(e) => lv.error = Some(e),
                    }
                }
            }
            Event::ImageCopied(result) => match result {
                Ok(()) => self.set_status("photo copied to clipboard", false),
                Err(e) => self.set_status(format!("copy photo: {e}"), true),
            },
            Event::SettingsSaved(result) => {
                if let Err(e) = result {
                    self.set_status(format!("options not saved: {e}"), true);
                }
            }
            Event::ThemeFile(Ok(())) => self.reload_theme_file(),
            Event::ThemeFile(Err(e)) => self.set_status(e, true),
            Event::Topics { chat_id, result } => self.on_topics(chat_id, result),
            Event::TopicRefreshed { chat_id, result } => match result {
                Ok(t) => {
                    if let Some(list) = self.topics.get_mut(&chat_id) {
                        list.upsert(t);
                    }
                }
                Err(e) => self.set_status(format!("topic: {e}"), true),
            },
            Event::TopicCreated { chat_id, result } => match result {
                Ok(t) => {
                    let id = t.id;
                    self.topics.entry(chat_id).or_default().upsert(t);
                    self.td.refresh_topic(chat_id, id);
                    self.enter_topic(chat_id, id);
                }
                Err(e) => self.set_status(format!("create topic: {e}"), true),
            },
            Event::TopicLink(Ok(link)) => self.copy_text(link),
            Event::TopicLink(Err(e)) => self.set_status(format!("topic link: {e}"), true),
            Event::SecretCreated(Ok(chat_id)) => {
                self.popup = None;
                self.set_status("secret chat created; it exists only on this device", false);
                self.enter_chat(chat_id);
            }
            Event::SecretCreated(Err(e)) => self.set_status(format!("secret chat: {e}"), true),
        }
    }

    fn on_file(&mut self, f: FileRef) {
        let id = f.id;
        if !self.media.on_file(f) {
            return;
        }
        let Some(path) = self
            .media
            .file(&FileRef {
                id,
                ..FileRef::default()
            })
            .local()
            .map(str::to_owned)
        else {
            return;
        };
        let jobs: Vec<FileJob> = self
            .file_jobs
            .iter()
            .filter(|(fid, _)| *fid == id)
            .map(|(_, j)| j.clone())
            .collect();
        self.file_jobs.retain(|(fid, _)| *fid != id);
        for job in jobs {
            self.run_file_job(&path, job);
        }
    }

    fn run_file_job(&mut self, path: &str, job: FileJob) {
        match job {
            FileJob::Play(kind, target) => {
                self.stop_playback();
                self.next_token += 1;
                let token = self.next_token;
                let label = std::path::Path::new(path)
                    .file_name()
                    .map_or_else(|| "media".to_owned(), |n| n.to_string_lossy().into_owned());
                match player::spawn_mpv(player::mpv_args(kind, path), token, self.td.sender()) {
                    Ok(stop) => {
                        let audio = matches!(kind, MediaKind::Voice | MediaKind::Audio);
                        self.playing = Some(Playing {
                            token,
                            target,
                            label: label.clone(),
                            stop: Some(stop),
                        });
                        if audio {
                            self.set_status(format!("▶ playing {label} (x to stop)"), false);
                        } else {
                            self.set_status(format!("opened {label} in mpv"), false);
                        }
                    }
                    Err(e) => self.set_status(e, true),
                }
            }
            FileJob::CopyImage => {
                self.td.copy_image(PathBuf::from(path));
                self.set_status("copying photo...", false);
            }
            FileJob::Open => match player::runnable(std::path::Path::new(path)) {
                Err(e) => self.set_status(e, true),
                Ok(Some(why)) => self.set_status(
                    format!(
                        "not opening {path}: {why}; save it and open it yourself if you trust it"
                    ),
                    true,
                ),
                Ok(None) => match player::xdg_open(path) {
                    Ok(()) => self.set_status("opened with xdg-open", false),
                    Err(e) => self.set_status(e, true),
                },
            },
            FileJob::Save(name) => {
                let tx = self.td.sender();
                let src = PathBuf::from(path);
                let dir = match player::downloads_dir() {
                    Ok(d) => d,
                    Err(e) => {
                        self.set_status(format!("save failed: {e}"), true);
                        return;
                    }
                };
                tokio::task::spawn_blocking(move || {
                    let result =
                        player::save_copy(&src, &dir, &name).map(|p| p.display().to_string());
                    let _ = tx.send(Event::Saved(result));
                });
                self.set_status("saving...", false);
            }
        }
    }

    pub fn stop_playback(&mut self) {
        if let Some(mut p) = self.playing.take()
            && let Some(stop) = p.stop.take()
        {
            let _ = stop.send(());
        }
    }

    fn queue_file_job(&mut self, file: &FileRef, name: &str, job: FileJob) {
        let file = self.media.file(file);
        if let Some(path) = file.local().map(str::to_owned) {
            self.run_file_job(&path, job);
            return;
        }
        if file.id == 0 {
            self.set_status("this file is not available", true);
            return;
        }
        if !self.file_jobs.contains(&(file.id, job.clone())) {
            self.file_jobs.push((file.id, job));
        }
        self.media.force_download(file.id, 32);
        let size = if file.size > 0 {
            format!(" ({})", format::file_size(file.size))
        } else {
            String::new()
        };
        self.set_status(format!("downloading {name}{size}..."), false);
    }

    fn on_paste(&mut self, text: &str) {
        if self.auth != Auth::Ready {
            self.login_input.insert_str(text.trim());
        } else if let Some(Popup::Switcher { input, sel, .. }) = &mut self.popup {
            input.insert_str(&format::one_line(text));
            *sel = 0;
        } else if let Some(Popup::FilePicker(fp)) = &mut self.popup {
            fp.input.insert_str(&format::one_line(text));
            fp.sel = 0;
            self.refresh_picker_listing();
        } else if let Some(Popup::Stickers(sp)) = &mut self.popup {
            sp.query.insert_str(&format::one_line(text));
            self.sticker_query_changed();
        } else if let Some(Popup::Search(sv)) = &mut self.popup {
            if !sv.listing() {
                sv.input.insert_str(&format::one_line(text));
            }
        } else if self.composer.active && self.recording.is_none() {
            self.composer.input.insert_str(text);
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
                    self.account_archive(v.value);
                }
            }
            Update::User(u) => {
                self.users.insert(u.user.id, td::user_name(&u.user));
                self.user_meta.insert(u.user.id, td::user_meta(&u.user));
            }
            Update::UserStatus(u) => {
                self.user_meta.entry(u.user_id).or_default().status = td::convert_status(&u.status);
            }
            Update::ChatAction(u) => {
                let sender = td::convert_sender(&u.sender_id);
                if self.my_id.is_some_and(|me| sender == Sender::User(me)) {
                    return;
                }
                let topic = td::msg_topic(&u.topic_id);
                let list = self.typing.entry(u.chat_id).or_default();
                list.retain(|t| !(t.sender == sender && t.topic == topic));
                if let Some(a) = td::convert_activity(&u.action) {
                    list.push(presence::Typing::new(sender, a, topic));
                }
                if list.is_empty() {
                    self.typing.remove(&u.chat_id);
                }
            }
            Update::ChatReplyMarkup(u) => {
                self.reply_kb.remove(&u.chat_id);
                if u.reply_markup_message_id == 0 {
                    self.reply_kb_ids.remove(&u.chat_id);
                } else {
                    self.reply_kb_ids
                        .insert(u.chat_id, u.reply_markup_message_id);
                    if self.open_chat == Some(u.chat_id) {
                        self.ensure_reply_keyboard(u.chat_id);
                    }
                }
            }
            Update::UnreadChatCount(u) => {
                let list = td::from_chat_list(&u.chat_list);
                self.unread_counts.insert(list, u.unread_count);
                self.unread_unmuted.insert(list, u.unread_unmuted_count);
            }
            Update::UnreadMessageCount(u) => {
                self.unread_messages.insert(
                    td::from_chat_list(&u.chat_list),
                    (u.unread_count, u.unread_unmuted_count),
                );
            }
            Update::ScopeNotificationSettings(u) => {
                let i = match u.scope {
                    tdlib_rs::enums::NotificationSettingsScope::PrivateChats => 0,
                    tdlib_rs::enums::NotificationSettingsScope::GroupChats => 1,
                    tdlib_rs::enums::NotificationSettingsScope::ChannelChats => 2,
                };
                self.scope_muted[i] = u.notification_settings.mute_for > 0;
            }
            Update::MessageInteractionInfo(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    let info = u.interaction_info.as_ref();
                    m.reactions = td::convert_reactions(info);
                    m.meta.views = info.map_or(0, |i| i.view_count);
                    m.meta.forwards = info.map_or(0, |i| i.forward_count);
                    m.meta.replies = info
                        .and_then(|i| i.reply_info.as_ref())
                        .map_or(0, |r| r.reply_count);
                }
            }
            Update::MessageIsPinned(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.pinned = u.is_pinned;
                }
                if self.open_chat == Some(u.chat_id) {
                    self.td.load_pinned(u.chat_id);
                }
            }
            Update::ChatReadOutbox(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.last_read_outbox = u.last_read_outbox_message_id;
                }
            }
            Update::ChatPhoto(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.photo = u.photo.as_ref().map(|p| td::convert_file(&p.small));
                }
            }
            Update::Supergroup(u) => {
                let meta = model::GroupMeta {
                    forum: u.supergroup.is_forum,
                    status: td::convert_membership(&u.supergroup.status),
                };
                let id = u.supergroup.id;
                self.groups.insert(id, meta);
                for c in self.chats.values_mut().filter(|c| c.group_id == id) {
                    c.forum = meta.forum;
                }
                if !meta.forum
                    && let Some(chat) = self.expanded
                    && self.chats.get(&chat).is_some_and(|c| c.group_id == id)
                {
                    self.expanded = None;
                }
            }
            Update::SecretChat(u) => {
                let sc = td::convert_secret(&u.secret_chat);
                self.secret_chats.insert(sc.id, sc);
            }
            Update::ChatPermissions(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.perms = td::convert_perms(&u.permissions);
                }
            }
            Update::ChatHasProtectedContent(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.protected = u.has_protected_content;
                }
            }
            Update::ChatBlockList(u) => {
                let sender = self.sender_of_chat(u.chat_id);
                if matches!(u.block_list, Some(tdlib_rs::enums::BlockList::Main)) {
                    self.blocked.insert(sender);
                } else {
                    self.blocked.remove(&sender);
                }
            }
            Update::ChatMessageAutoDeleteTime(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.auto_delete = u.message_auto_delete_time;
                }
            }
            Update::ForumTopicInfo(u) => {
                let info = u.info;
                let chat_id = info.chat_id;
                let topic = td::convert_topic_info(info);
                let id = topic.id;
                if let Some(list) = self.topics.get_mut(&chat_id)
                    && !list.apply_info(topic)
                    && list.loaded
                {
                    self.td.refresh_topic(chat_id, id);
                }
            }
            Update::ForumTopic(u) => {
                let Some(list) = self.topics.get_mut(&u.chat_id) else {
                    return;
                };
                let Some(mut t) = list.get(u.forum_topic_id).cloned() else {
                    return;
                };
                t.pinned = u.is_pinned;
                t.last_read_outbox = u.last_read_outbox_message_id;
                t.mentions = u.unread_mention_count;
                t.reactions = u.unread_reaction_count;
                t.muted = u.notification_settings.mute_for > 0;
                t.use_default_mute = u.notification_settings.use_default_mute_for;
                let advanced = u.last_read_inbox_message_id > t.last_read_inbox;
                list.upsert(t);
                list.mark_read_up_to(u.forum_topic_id, u.last_read_inbox_message_id);
                if advanced && list.get(u.forum_topic_id).is_some_and(|t| t.unread > 0) {
                    self.td.refresh_topic(u.chat_id, u.forum_topic_id);
                }
            }
            Update::NewChat(c) => {
                if c.chat.reply_markup_message_id != 0 {
                    self.reply_kb_ids
                        .insert(c.chat.id, c.chat.reply_markup_message_id);
                }
                let mut info = td::convert_chat(c.chat);
                info.forum = self.groups.get(&info.group_id).is_some_and(|g| g.forum);
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
            Update::ChatUnreadReactionCount(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.reactions = u.unread_reaction_count;
                }
            }
            Update::MessageUnreadReactions(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.unread_reaction = !u.unread_reactions.is_empty();
                }
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.reactions = u.unread_reaction_count;
                }
            }
            Update::MessageMentionRead(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.mention = false;
                }
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.mentions = u.unread_mention_count;
                }
            }
            Update::ChatNotificationSettings(u) => {
                if let Some(c) = self.chats.get_mut(&u.chat_id) {
                    c.muted = u.notification_settings.mute_for > 0;
                    c.use_default_mute = u.notification_settings.use_default_mute_for;
                }
            }
            Update::ChatFolders(f) => {
                let before = self.current_list();
                self.folders = f
                    .chat_folders
                    .into_iter()
                    .map(|f| (f.id, f.name.text.text))
                    .collect();
                self.list_cache.clear();
                if self.folder_entries().iter().any(|(id, _)| *id == before) {
                    self.reselect_folder(before);
                } else {
                    self.folder_sel = self
                        .folder_sel
                        .min(self.folder_entries().len().saturating_sub(1));
                    let now = self.current_list();
                    self.ensure_list_loaded(now);
                }
            }
            Update::NewMessage(m) => {
                let msg = td::convert_message(m.message);
                let chat_id = msg.chat_id;
                if msg.meta.story && self.settings.privacy.hide_stories {
                    return;
                }
                let blocked = self.hidden_blocked(&msg);
                if let Some(list) = self.typing.get_mut(&chat_id) {
                    list.retain(|t| t.sender != msg.sender);
                    if list.is_empty() {
                        self.typing.remove(&chat_id);
                    }
                }
                if !blocked {
                    self.on_incoming(&msg);
                }
                if let Some(list) = self.topics.get_mut(&chat_id)
                    && let Some(topic) = msg.topic
                    && !list.note_message(&msg)
                    && list.loaded
                {
                    self.td.refresh_topic(chat_id, topic);
                }
                let open = self.open_chat == Some(chat_id);
                if open && msg.outgoing && self.window_detached() {
                    self.jump_to_latest();
                }
                if let Some(h) = self.histories.get_mut(&chat_id) {
                    if (h.detached && h.index_of(msg.id).is_none()) || !h.accepts(&msg) {
                        return;
                    }
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
                let story = matches!(
                    u.new_content,
                    tdlib_rs::enums::MessageContent::MessageStory(_)
                );
                let content = td::convert_content(u.new_content);
                self.on_content_change(u.chat_id, u.message_id, &content);
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.meta.story = story;
                    m.media = content.media;
                    m.attachments = content.attachments;
                    m.body = content.body;
                }
            }
            Update::MessageEdited(u) => {
                if let Some(m) = self
                    .histories
                    .get_mut(&u.chat_id)
                    .and_then(|h| h.get_mut(u.message_id))
                {
                    m.edited = u.edit_date > 0;
                    m.meta.edit_date = i64::from(u.edit_date);
                    m.keyboard = td::convert_markup(u.reply_markup);
                }
            }
            Update::File(u) => self.on_file(td::convert_file(&u.file)),
            Update::DeleteMessages(u) => {
                if !u.is_permanent || u.from_cache {
                    return;
                }
                let kept = self.on_deleted(u.chat_id, &u.message_ids);
                if let Some(h) = self.histories.get_mut(&u.chat_id) {
                    for id in &kept {
                        if let Some(m) = h.get_mut(*id) {
                            m.deleted = true;
                        }
                    }
                    let gone: Vec<i64> = u
                        .message_ids
                        .iter()
                        .copied()
                        .filter(|id| !kept.contains(id))
                        .collect();
                    let removed = h.remove(&gone);
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
                self.online_sent = None;
                self.apply_online();
                self.blocked_loaded = false;
                if self.settings.privacy.hide_blocked {
                    self.ensure_blocked_loaded();
                }
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
        self.file_jobs.clear();
        self.stop_playback();
        self.cancel_recording();
        self.stickers = Library::default();
        self.user_meta.clear();
        self.typing.clear();
        self.online_sent = None;
        self.typing_sent = None;
        self.bot_commands.clear();
        self.reply_kb_ids.clear();
        self.reply_kb.clear();
        self.inbox.clear();
        self.pending_jump = None;
        self.jump_inflight = None;
        self.unread_counts.clear();
        self.unread_unmuted.clear();
        self.unread_messages.clear();
        self.cmd_sel = 0;
        self.scope_muted = [false; 3];
        self.pinned_msg.clear();
        self.layout_cache = crate::ui::LayoutCache::default();
        self.pending_open = None;
        self.groups.clear();
        self.secret_chats.clear();
        self.topics.clear();
        self.expanded = None;
        self.open_topic = None;
        self.topic_select = None;
        self.topic_unread = None;
        self.archive = crate::archive::Archive::memory(self.settings.privacy.keep_deleted_limit);
        self.blocked.clear();
        self.blocked_loaded = false;
        self.revealed.clear();
        self.media.reset();
    }

    #[cfg(test)]
    fn per_account_leftovers(&self) -> Vec<&'static str> {
        let checks = [
            ("connection", self.connection.is_none()),
            ("my_id", self.my_id.is_none()),
            ("users", self.users.is_empty()),
            ("chats", self.chats.is_empty()),
            ("folders", self.folders.is_empty()),
            ("list_cache", self.list_cache.is_empty()),
            ("loaded_lists", self.loaded_lists.is_empty()),
            ("filter", self.filter.is_none()),
            ("open_chat", self.open_chat.is_none()),
            ("histories", self.histories.is_empty()),
            ("msg_sel", self.msg_sel.is_none()),
            ("last_viewed", self.last_viewed.is_empty()),
            ("composer", self.composer.input.text().is_empty()),
            ("attachments", self.composer.attachments.is_empty()),
            ("reply_to", self.composer.reply_to.is_none()),
            ("pending", self.pending.is_empty()),
            ("popup", self.popup.is_none()),
            ("media", self.media.is_empty()),
            ("playing", self.playing.is_none()),
            ("file_jobs", self.file_jobs.is_empty()),
            ("recording", self.recording.is_none()),
            ("user_meta", self.user_meta.is_empty()),
            ("typing", self.typing.is_empty()),
            ("online_sent", self.online_sent.is_none()),
            ("typing_sent", self.typing_sent.is_none()),
            ("bot_commands", self.bot_commands.is_empty()),
            ("cmd_sel", self.cmd_sel == 0),
            ("reply_kb_ids", self.reply_kb_ids.is_empty()),
            ("reply_kb", self.reply_kb.is_empty()),
            ("inbox", self.inbox.is_empty()),
            ("pending_jump", self.pending_jump.is_none()),
            ("jump_inflight", self.jump_inflight.is_none()),
            ("unread_counts", self.unread_counts.is_empty()),
            ("unread_unmuted", self.unread_unmuted.is_empty()),
            ("unread_messages", self.unread_messages.is_empty()),
            ("scope_muted", self.scope_muted == [false; 3]),
            ("pinned_msg", self.pinned_msg.is_empty()),
            ("layout_cache", self.layout_cache.len() == 0),
            ("pending_open", self.pending_open.is_none()),
            ("groups", self.groups.is_empty()),
            ("secret_chats", self.secret_chats.is_empty()),
            ("topics", self.topics.is_empty()),
            ("expanded", self.expanded.is_none()),
            ("open_topic", self.open_topic.is_none()),
            ("topic_select", self.topic_select.is_none()),
            ("topic_unread", self.topic_unread.is_none()),
            (
                "archive",
                self.archive.len() == 0 && self.archive.path().is_none(),
            ),
            ("blocked", self.blocked.is_empty()),
            ("blocked_loaded", !self.blocked_loaded),
            ("revealed", self.revealed.is_empty()),
        ];
        checks
            .into_iter()
            .filter(|(_, empty)| !empty)
            .map(|(name, _)| name)
            .collect()
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
        self.stop_playback();
        self.cancel_recording();
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

    fn handle_key(&mut self, key: KeyEvent) {
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
        self.pending_open = None;
        if self.recording.is_some() {
            self.on_record_key(key);
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
            Action::PageDown => self.move_selection(self.page()),
            Action::PageUp => self.move_selection(-self.page()),
            Action::JumpTop => self.move_selection(isize::MIN / 2),
            Action::JumpBottom if self.focus == Pane::Messages && self.window_detached() => {
                self.jump_to_latest()
            }
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
                Pane::Messages => match self.open_chat {
                    Some(chat) => self.open_search(Some(chat), false),
                    None => self.set_status("open a chat first", false),
                },
                Pane::Info => {}
            },
            Action::ReplyMessage => self.message_action(MenuAction::Reply),
            Action::DeleteMessage => self.message_action(MenuAction::Delete),
            Action::EditMessage => self.message_action(MenuAction::Edit),
            Action::OpenMessageUrl => self.message_action(MenuAction::OpenUrl),
            Action::PlayMedia => self.message_action(MenuAction::Play),
            Action::ViewAttachment => self.message_action(MenuAction::View),
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
                    forward: None,
                })
            }
            Action::AttachFile => self.open_file_picker(),
            Action::RecordVoice => self.start_recording(),
            Action::OpenStickers => self.open_sticker_picker(0),
            Action::CopyMessage => self.message_action(MenuAction::Copy),
            Action::ReactMessage => self.message_action(MenuAction::React),
            Action::ForwardMessage => self.message_action(MenuAction::Forward),
            Action::PinMessage => self.message_action(MenuAction::Pin),
            Action::GoToReply => self.message_action(MenuAction::GoToReply),
            Action::ShowMessageProfile => self.message_action(MenuAction::SenderProfile),
            Action::ShowReactionUsers => self.message_action(MenuAction::ReactionList),
            Action::CopyQuote => self.message_action(MenuAction::CopyQuote),
            Action::TranslateMessage => self.message_action(MenuAction::Translate),
            Action::SaveMessage => self.message_action(MenuAction::SaveMessage),
            Action::OpenProfile => match self.focused_chat() {
                Some(chat) => self.open_profile(ProfileTarget::Chat(chat)),
                None => self.set_status("open a chat first", false),
            },
            Action::ShowPinned => match self.focused_chat() {
                Some(chat) => self.open_search(Some(chat), true),
                None => self.set_status("open a chat first", false),
            },
            Action::SearchMessages => self.open_search(None, false),
            Action::OpenInbox => self.popup = Some(Popup::Inbox { sel: 0 }),
            Action::MarkChatRead => match self.focused_chat() {
                Some(chat) => self.mark_chat_read(chat),
                None => self.set_status("open a chat first", false),
            },
            Action::OpenBotKeyboard => self.open_buttons(),
            Action::OpenOptions => self.popup = Some(Popup::Options { sel: 0 }),
            Action::OpenNextUnread => {
                self.open_next_unread();
            }
            Action::JumpToNextMention => self.jump_next_unread(Unread::Mention),
            Action::JumpToNextReaction => self.jump_next_unread(Unread::Reaction),
            Action::JumpToPinned => {
                let target = self
                    .open_chat
                    .and_then(|chat| self.pinned_msg.get(&chat).map(|m| (chat, m.id, m.topic)));
                match target {
                    Some((chat, id, topic)) => {
                        self.focus = Pane::Messages;
                        let topic = topic.or(self.open_topic);
                        self.jump_to_in(chat, id, topic);
                    }
                    None => self.set_status("no pinned message in this chat", false),
                }
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

    fn page(&self) -> isize {
        match self.focus {
            Pane::Messages => (self.msg_view_height / 3).max(1) as isize,
            Pane::Folders => self.folder_rows.get().max(1) as isize,
            _ => self.chat_rows.get().max(1) as isize,
        }
    }

    pub fn folder_badge(&self, list: ListId) -> Option<String> {
        let a = &self.settings.appearance;
        let unmuted = a.folder_counter_unmuted_only;
        let n = match a.folder_counter {
            crate::config::FolderCounter::Off => return None,
            crate::config::FolderCounter::Chats => if unmuted {
                self.unread_unmuted.get(&list)
            } else {
                self.unread_counts.get(&list)
            }
            .copied()
            .unwrap_or_else(|| self.local_unread(list, unmuted, false)),
            crate::config::FolderCounter::Messages => self
                .unread_messages
                .get(&list)
                .map(|(all, un)| if unmuted { *un } else { *all })
                .unwrap_or_else(|| self.local_unread(list, unmuted, true)),
        };
        (n > 0).then(|| self.count(i64::from(n)))
    }

    fn local_unread(&self, list: ListId, unmuted: bool, messages: bool) -> i32 {
        self.chats
            .values()
            .filter(|c| c.position_in(list).is_some() && c.unread > 0)
            .filter(|c| !unmuted || !self.is_muted(c.id))
            .map(|c| if messages { c.unread } else { 1 })
            .sum()
    }

    fn reselect_folder(&mut self, list: ListId) {
        let entries = self.folder_entries();
        self.folder_sel = entries.iter().position(|(id, _)| *id == list).unwrap_or(0);
        if self.current_list() != list {
            self.chat_sel = 0;
            let now = self.current_list();
            self.ensure_list_loaded(now);
            self.refresh_lists();
        }
    }

    pub fn folder_entries(&self) -> Vec<(ListId, String)> {
        let mut out = Vec::new();
        if !(self.settings.appearance.hide_all_chats && !self.folders.is_empty()) {
            out.push((ListId::Main, "All chats".to_owned()));
        }
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

    pub fn chat_rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for id in self.visible_chats() {
            out.push(Row::Chat(id));
            if self.expanded != Some(id) {
                continue;
            }
            match self.topics.get(&id) {
                Some(list) => {
                    out.extend(list.topics().iter().map(|t| Row::Topic(id, t.id)));
                    if list.loading {
                        out.push(Row::Loading(id));
                    }
                }
                None => out.push(Row::Loading(id)),
            }
        }
        out
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.chat_rows().get(self.chat_sel).copied()
    }

    fn select_row(&mut self, row: Row) {
        self.refresh_lists();
        if let Some(i) = self.chat_rows().iter().position(|r| *r == row) {
            self.chat_sel = i;
        }
    }

    pub fn is_forum(&self, chat_id: i64) -> bool {
        self.chats.get(&chat_id).is_some_and(|c| c.forum)
    }

    pub fn topic(&self, chat_id: i64, topic: i32) -> Option<&model::Topic> {
        self.topics.get(&chat_id).and_then(|l| l.get(topic))
    }

    pub fn open_topic_info(&self) -> Option<&model::Topic> {
        self.topic(self.open_chat?, self.open_topic?)
    }

    pub fn topic_muted(&self, chat_id: i64, topic: i32) -> bool {
        match self.topic(chat_id, topic) {
            Some(t) if !t.use_default_mute => t.muted,
            _ => self.is_muted(chat_id),
        }
    }

    pub fn rights(&self, chat_id: i64) -> model::Rights {
        let Some(c) = self.chats.get(&chat_id) else {
            return model::Rights::default();
        };
        match c.kind {
            ChatKind::Private(_) | ChatKind::Secret(_) => model::Rights {
                change_info: true,
                pin: true,
                ..model::Rights::default()
            },
            ChatKind::Group => c.perms,
            ChatKind::Supergroup | ChatKind::Channel => {
                let status = self
                    .groups
                    .get(&c.group_id)
                    .map_or(model::Membership::Member, |g| g.status);
                model::effective_rights(status, c.perms)
            }
        }
    }

    pub fn secret_chat(&self, chat_id: i64) -> Option<&model::SecretChat> {
        let c = self.chats.get(&chat_id)?;
        if !matches!(c.kind, ChatKind::Secret(_)) {
            return None;
        }
        self.secret_chats.get(&c.secret_id)
    }

    pub fn send_topic(&self) -> Option<i32> {
        self.open_chat
            .and_then(|id| self.histories.get(&id))
            .and_then(History::send_topic)
    }

    fn expand(&mut self, chat_id: i64) {
        self.expanded = Some(chat_id);
        let list = self.topics.entry(chat_id).or_default();
        if !list.loaded && !list.loading {
            list.loading = true;
            self.td.load_topics(chat_id, None);
        }
    }

    fn maybe_more_topics(&mut self) {
        let Some(chat) = self.expanded else {
            return;
        };
        let at = match self.selected_row() {
            Some(Row::Topic(c, t)) if c == chat => self
                .topics
                .get(&chat)
                .and_then(|l| l.topics().iter().position(|x| x.id == t)),
            Some(Row::Loading(c)) if c == chat => self.topics.get(&chat).map(|l| l.topics().len()),
            _ => None,
        };
        if let Some(list) = self.topics.get_mut(&chat)
            && list.needs_more(at)
        {
            list.loading = true;
            self.td.load_topics(chat, list.next);
        }
    }

    fn on_topics(&mut self, chat_id: i64, result: Result<td::TopicPage, String>) {
        let list = self.topics.entry(chat_id).or_default();
        match result {
            Ok(page) => list.merge_page(page.topics, page.next, page.total),
            Err(e) => {
                list.loading = false;
                list.loaded = true;
                self.topic_select = None;
                self.topic_unread = None;
                self.set_status(format!("topics: {e}"), true);
                return;
            }
        }
        if self.topic_unread == Some(chat_id) {
            self.topic_unread = None;
            self.topic_select = None;
            match self.next_unread_topic(chat_id, None) {
                Some(t) => self.enter_topic(chat_id, t),
                None => self.set_status("no unread topics", false),
            }
            return;
        }
        if self.topic_select == Some(chat_id) {
            self.topic_select = None;
            self.select_first_topic(chat_id);
        }
    }

    fn select_first_topic(&mut self, chat_id: i64) {
        if self.focus != Pane::Chats || self.expanded != Some(chat_id) {
            return;
        }
        let Some(list) = self.topics.get(&chat_id) else {
            return;
        };
        let pick = list
            .topics()
            .iter()
            .find(|t| t.unread > 0)
            .or_else(|| list.topics().first())
            .map(|t| t.id);
        if let Some(t) = pick {
            self.select_row(Row::Topic(chat_id, t));
        }
    }

    fn next_unread_topic(&self, chat_id: i64, current: Option<i32>) -> Option<i32> {
        let list = self.topics.get(&chat_id)?;
        let ids: Vec<i32> = list.topics().iter().map(|t| t.id).collect();
        let skip_muted = self.settings.navigation.skip_muted;
        next_matching(&ids, current, false, |id| {
            list.get(id).is_some_and(|t| t.unread > 0)
                && !(skip_muted && self.topic_muted(chat_id, id))
        })
    }

    pub fn enter_forum(&mut self, chat_id: i64) {
        self.open_view(chat_id, None);
        self.expand(chat_id);
        self.show_chats = true;
        self.focus = Pane::Chats;
        self.select_row(Row::Chat(chat_id));
        if self.topics.get(&chat_id).is_some_and(|l| l.loaded) {
            self.select_first_topic(chat_id);
        } else {
            self.topic_select = Some(chat_id);
        }
    }

    pub fn enter_topic(&mut self, chat_id: i64, topic: i32) {
        let (last_read, unread) = self
            .topic(chat_id, topic)
            .map_or((0, 0), |t| (t.last_read_inbox, t.unread));
        self.open_view(chat_id, Some(topic));
        if self.is_forum(chat_id) {
            self.expand(chat_id);
        }
        self.select_row(Row::Topic(chat_id, topic));
        self.pending_open = Some(PendingOpen {
            chat_id,
            last_read,
            unread,
        });
        self.resolve_open();
    }

    fn open_any(&mut self, chat_id: i64) {
        if self.is_forum(chat_id) {
            self.enter_forum(chat_id);
        } else {
            self.enter_chat(chat_id);
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
                let n = self.chat_rows().len();
                self.chat_sel = step(self.chat_sel, delta, n);
                if n > 0 && self.chat_sel + 10 >= n {
                    self.td.load_chats(self.current_list());
                }
                self.maybe_more_topics();
            }
            Pane::Messages => self.move_message_selection(delta),
            Pane::Info => {}
        }
    }

    fn move_message_selection(&mut self, delta: isize) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get(&chat_id) else {
            return;
        };
        let groups = model::groups(h.msgs());
        if groups.is_empty() {
            return;
        }
        self.bottom_offset = 0;
        let cur = match self.msg_sel {
            Some(i) => model::group_of(&groups, i),
            None => groups.len(),
        };
        let next = cur as isize + delta;
        if next >= groups.len() as isize {
            self.msg_sel = if h.detached {
                groups.last().map(|g| g.start)
            } else {
                None
            };
        } else {
            self.msg_sel = Some(groups[next.max(0) as usize].start);
        }
        self.mark_read();
        self.maybe_load_older();
        self.maybe_load_newer();
    }

    fn scroll_viewport(&mut self, delta: isize) {
        if self.focus != Pane::Messages {
            return;
        }
        self.bottom_offset = if delta < 0 {
            self.bottom_offset + 3
        } else {
            self.bottom_offset.saturating_sub(3)
        };
        if delta > 0 && self.bottom_offset == 0 {
            self.maybe_load_newer();
        }
        if self.msg_sel.is_none() {
            self.want_older = true;
        } else {
            self.maybe_load_older();
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
        if (h.needs_older(near_top) || (wanted && h.can_load_older()))
            && let Some(oldest) = h.oldest_id()
        {
            h.start_older();
            self.td
                .load_history(chat_id, oldest, false, h.epoch, h.topic);
        }
    }

    fn maybe_load_newer(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        if h.needs_newer(self.msg_sel)
            && let Some(newest) = h.newest_id()
        {
            h.loading_newer = true;
            self.td.load_newer(chat_id, newest, h.epoch, h.topic);
        }
    }

    pub fn window_detached(&self) -> bool {
        self.open_chat
            .and_then(|id| self.histories.get(&id))
            .is_some_and(|h| h.detached)
    }

    fn latest_id(&self, chat_id: i64) -> Option<i64> {
        self.chats
            .get(&chat_id)
            .and_then(|c| c.last_message.as_ref())
            .map(|m| m.id)
    }

    fn on_newer_history(&mut self, chat_id: i64, msgs: Vec<Msg>, epoch: u64) {
        let latest = self.latest_id(chat_id);
        let open = self.open_chat == Some(chat_id);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        if h.epoch != epoch {
            return;
        }
        let lo = h.newest_id().map_or(1, |n| n + 1);
        let top = msgs.iter().map(|m| m.id).max().unwrap_or(0);
        let hi = if latest.is_some_and(|l| top >= l) {
            i64::MAX
        } else {
            top
        };
        let msgs = self.enrich(chat_id, msgs, lo, hi);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        h.merge_newer(msgs, latest);
        if open && h.len() > MAX_WINDOW {
            let dropped = h.trim_oldest(MAX_WINDOW);
            self.msg_top = self.msg_top.saturating_sub(dropped);
            if let Some(sel) = self.msg_sel.as_mut() {
                *sel = sel.saturating_sub(dropped);
            }
        }
    }

    fn on_history_around(&mut self, chat_id: i64, target: i64, result: Result<Vec<Msg>, String>) {
        if self.jump_inflight == Some((chat_id, target)) {
            self.jump_inflight = None;
        }
        if self.pending_jump != Some((chat_id, target)) || self.open_chat != Some(chat_id) {
            return;
        }
        let msgs = match result {
            Ok(msgs) => msgs,
            Err(e) => {
                self.pending_jump = None;
                self.set_status(e, true);
                return;
            }
        };
        let latest = self.latest_id(chat_id);
        let lo = msgs.iter().map(|m| m.id).min().unwrap_or(1);
        let top = msgs.iter().map(|m| m.id).max().unwrap_or(0);
        let hi = if latest.is_some_and(|l| top >= l) {
            i64::MAX
        } else {
            top
        };
        let msgs = self.enrich(chat_id, msgs, lo, hi);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        h.place_window(msgs, latest);
        if h.index_of(target).is_none() {
            self.pending_jump = None;
            self.set_status("that message could not be loaded", true);
            return;
        }
        self.continue_jump();
    }

    pub fn jump_to_latest(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        self.pending_jump = None;
        self.jump_inflight = None;
        h.reset_to_latest();
        self.td.load_history(chat_id, 0, true, h.epoch, h.topic);
        self.msg_sel = None;
        self.msg_top = 0;
        self.bottom_offset = 0;
    }

    fn on_history(&mut self, chat_id: i64, msgs: Vec<Msg>, initial: bool, epoch: u64) {
        let open = self.open_chat == Some(chat_id);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        if h.epoch != epoch {
            return;
        }
        let got = msgs.len();
        let raw_oldest = msgs.iter().map(|m| m.id).min();
        let lo = if got == 0 { 1 } else { raw_oldest.unwrap_or(1) };
        let hi = match h.oldest_id() {
            Some(o) => o - 1,
            None => i64::MAX,
        };
        let msgs = self.enrich(chat_id, msgs, lo, hi);
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        if got > 0 && msgs.is_empty() {
            let progress = match (raw_oldest, h.oldest_id()) {
                (Some(raw), Some(oldest)) => raw < oldest,
                (Some(_), None) => true,
                _ => false,
            };
            if progress {
                h.start_older();
                self.td
                    .load_history(chat_id, raw_oldest.unwrap_or(0), false, h.epoch, h.topic);
                return;
            }
            h.merge_older(Vec::new());
            return;
        }
        let prev_oldest = h.oldest_id();
        let added = h.merge_older(msgs);
        if prev_oldest.is_some() && h.oldest_id() == prev_oldest {
            h.reached_start = true;
        }
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
            h.start_older();
            self.td
                .load_history(chat_id, oldest, true, h.epoch, h.topic);
        }
        let deciding = self.pending_open.is_some_and(|p| p.chat_id == chat_id);
        if open && initial && !deciding {
            self.mark_read();
        }
        if open {
            self.continue_jump();
            self.resolve_open();
        }
    }

    fn mark_read(&mut self) {
        if !self.settings.auto_read() {
            return;
        }
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
        let topic = h.topic;
        if topic.is_none() && self.is_forum(chat_id) {
            return;
        }
        let flagged = h.get(id).is_some_and(|m| m.mention || m.unread_reaction);
        let unread = match topic.and_then(|t| self.topic(chat_id, t)) {
            Some(t) => t.unread > 0 || t.last_read_inbox < id,
            None => self
                .chats
                .get(&chat_id)
                .is_some_and(|c| c.unread > 0 || c.last_read_inbox < id),
        };
        let fresh = unread && self.last_viewed.get(&chat_id).is_none_or(|v| *v < id);
        if !fresh && !flagged {
            return;
        }
        self.td.view_messages(chat_id, vec![id], topic);
        if flagged && let Some(m) = self.histories.get_mut(&chat_id).and_then(|h| h.get_mut(id)) {
            m.mention = false;
            m.unread_reaction = false;
        }
        if fresh {
            self.last_viewed.insert(chat_id, id);
            if let Some(t) = topic
                && let Some(list) = self.topics.get_mut(&chat_id)
            {
                list.mark_read_up_to(t, id);
            }
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
                match self.selected_row() {
                    Some(Row::Chat(id)) if self.is_forum(id) => {
                        if self.expanded == Some(id) && self.open_chat == Some(id) {
                            self.expanded = None;
                            self.select_row(Row::Chat(id));
                        } else {
                            self.enter_forum(id);
                        }
                    }
                    Some(Row::Chat(id)) => self.enter_chat(id),
                    Some(Row::Topic(chat, t)) => self.enter_topic(chat, t),
                    _ => {}
                }
            }
            Pane::Messages => {
                if let Some(m) = self.selected_message().filter(|m| self.hidden_blocked(m)) {
                    let key = (m.chat_id, m.id);
                    self.revealed.insert(key);
                } else if self.msg_sel.is_some() {
                    self.open_action_menu();
                }
            }
            Pane::Info => {}
        }
    }

    pub fn open(&mut self, chat_id: i64) {
        let same = self.open_chat == Some(chat_id);
        let topic = if same { self.open_topic } else { None };
        self.open_view(chat_id, topic);
    }

    pub fn open_view(&mut self, chat_id: i64, topic: Option<i32>) {
        let same_chat = self.open_chat == Some(chat_id);
        if same_chat && self.open_topic != topic {
            let epoch = self.histories.get(&chat_id).map_or(0, |h| h.epoch + 1);
            self.reset_history(chat_id, topic, epoch);
        }
        if !same_chat {
            if let Some(old) = self.open_chat.take() {
                self.td.close_chat(old);
                self.histories.remove(&old);
            }
            self.open_chat = Some(chat_id);
            self.td.open_chat(chat_id);
            self.reset_history(chat_id, topic, 0);
            self.pinned_msg.remove(&chat_id);
            self.td.load_pinned(chat_id);
            let channel = self
                .chats
                .get(&chat_id)
                .is_some_and(|c| c.kind == ChatKind::Channel);
            if !channel && !self.bot_commands.contains_key(&chat_id) {
                self.bot_commands.insert(chat_id, Vec::new());
                self.td.load_bot_commands(chat_id);
            }
            self.ensure_reply_keyboard(chat_id);
        }
        self.msg_sel = None;
        self.msg_top = 0;
        self.bottom_offset = 0;
        self.focus = Pane::Messages;
    }

    fn reset_history(&mut self, chat_id: i64, topic: Option<i32>, epoch: u64) {
        let general = topic.is_some_and(|t| self.topic(chat_id, t).is_some_and(|x| x.general));
        let mut h = History::for_topic(topic, general);
        h.epoch = epoch;
        h.start_older();
        self.td.load_history(chat_id, 0, true, h.epoch, topic);
        self.histories.insert(chat_id, h);
        self.open_topic = topic;
        self.last_viewed.remove(&chat_id);
        self.composer = Composer::default();
        self.pending_jump = None;
        self.jump_inflight = None;
        self.pending_open = None;
        self.cmd_sel = 0;
    }

    pub fn enter_chat(&mut self, chat_id: i64) {
        let (last_read, unread) = self
            .chats
            .get(&chat_id)
            .map_or((0, 0), |c| (c.last_read_inbox, c.unread));
        self.open(chat_id);
        if let Some(i) = self.visible_chats().iter().position(|id| *id == chat_id) {
            self.chat_sel = i;
        }
        self.pending_open = Some(PendingOpen {
            chat_id,
            last_read,
            unread,
        });
        self.resolve_open();
    }

    fn resolve_open(&mut self) {
        let Some(p) = self.pending_open else {
            return;
        };
        if self.open_chat != Some(p.chat_id) {
            self.pending_open = None;
            return;
        }
        let mode = self.settings.navigation.open_chat_focus;
        if mode == OpenFocus::Composer || p.unread <= 0 {
            self.pending_open = None;
            self.finish_open(mode != OpenFocus::Messages, None);
            return;
        }
        let Some(h) = self.histories.get_mut(&p.chat_id) else {
            self.pending_open = None;
            return;
        };
        if h.loading_older {
            return;
        }
        let complete = h.reached_start || h.oldest_id().is_some_and(|o| o <= p.last_read);
        if !complete
            && h.len() + 50 <= MAX_WINDOW
            && let Some(oldest) = h.oldest_id()
        {
            h.start_older();
            self.td
                .load_history(p.chat_id, oldest, false, h.epoch, h.topic);
            return;
        }
        let first = h
            .msgs()
            .iter()
            .position(|m| m.id > p.last_read && !m.outgoing);
        let first = if complete {
            first
        } else {
            self.set_status("first unread message is too far back to load", false);
            first.map(|_| 0)
        };
        self.pending_open = None;
        let Some(i) = first else {
            self.finish_open(mode != OpenFocus::Messages, None);
            return;
        };
        let fits = mode == OpenFocus::Smart
            && crate::ui::lines_from(self, p.chat_id, i) <= self.msg_view_height;
        self.finish_open(fits, (!fits).then_some(i));
    }

    fn finish_open(&mut self, composer: bool, first_unread: Option<usize>) {
        self.focus = Pane::Messages;
        if let Some(i) = first_unread {
            self.msg_sel = Some(i);
            self.msg_top = i;
            self.bottom_offset = 0;
        }
        if composer {
            self.start_composer(None, None);
        }
        self.mark_read();
        self.maybe_load_older();
    }

    fn chat_order(&mut self) -> Vec<i64> {
        self.refresh_lists();
        self.list_cache
            .get(&self.current_list())
            .cloned()
            .unwrap_or_default()
    }

    pub fn open_next_unread(&mut self) -> bool {
        if let Some(chat) = self.open_chat
            && self.is_forum(chat)
            && let Some(t) = self.next_unread_topic(chat, self.open_topic)
        {
            self.enter_topic(chat, t);
            return true;
        }
        let order = self.chat_order();
        let skip_muted = self.settings.navigation.skip_muted;
        let unread = |id: i64| self.chats.get(&id).is_some_and(|c| c.unread > 0);
        let found = next_matching(&order, self.open_chat, false, |id| {
            unread(id) && !(skip_muted && self.is_muted(id))
        });
        if let Some(id) = found {
            if !self.is_forum(id) {
                self.enter_chat(id);
                return true;
            }
            self.enter_forum(id);
            self.focus = Pane::Messages;
            match self.next_unread_topic(id, None) {
                Some(t) => self.enter_topic(id, t),
                None if self.topics.get(&id).is_some_and(|l| l.loaded) => {}
                None => {
                    self.topic_select = None;
                    self.topic_unread = Some(id);
                }
            }
            return true;
        }
        let muted = order
            .iter()
            .filter(|id| Some(**id) != self.open_chat && unread(**id) && self.is_muted(**id))
            .count();
        let text = if skip_muted && muted > 0 {
            format!("no unread chats ({muted} muted skipped)")
        } else {
            "no unread chats".to_owned()
        };
        self.set_status(text, false);
        false
    }

    pub fn unread_counts(&self, chat_id: i64, topic: Option<i32>) -> (i32, i32) {
        match topic.and_then(|t| self.topic(chat_id, t)) {
            Some(t) => (t.mentions, t.reactions),
            None => self
                .chats
                .get(&chat_id)
                .map_or((0, 0), |c| (c.mentions, c.reactions)),
        }
    }

    fn jump_next_unread(&mut self, kind: Unread) {
        let pick = |m: i32, r: i32| match kind {
            Unread::Mention => m,
            Unread::Reaction => r,
        };
        let order = self.chat_order();
        let has = |id: i64| {
            self.chats
                .get(&id)
                .is_some_and(|c| pick(c.mentions, c.reactions) > 0)
        };
        let here = self.open_chat.and_then(|open| {
            let topic = self.open_topic.filter(|_| self.is_forum(open));
            let (m, r) = self.unread_counts(open, topic);
            if topic.is_some() && pick(m, r) > 0 {
                Some((open, topic))
            } else {
                has(open).then_some((open, None))
            }
        });
        let found =
            here.or_else(|| next_matching(&order, self.open_chat, true, has).map(|id| (id, None)));
        let (what, none) = match kind {
            Unread::Mention => ("mention", "no unread mentions"),
            Unread::Reaction => ("reaction", "no unread reactions"),
        };
        match found {
            Some((id, topic)) => {
                match kind {
                    Unread::Mention => self.td.find_unread_mention(id, topic),
                    Unread::Reaction => self.td.find_unread_reaction(id, topic),
                }
                self.set_status(format!("looking for the next {what}..."), false);
            }
            None => self.set_status(none, false),
        }
    }

    fn has_unread_after_view(&self, chat_id: i64) -> bool {
        let Some(c) = self.chats.get(&chat_id) else {
            return false;
        };
        if c.unread <= 0 {
            return false;
        }
        let seen = self
            .last_viewed
            .get(&chat_id)
            .copied()
            .unwrap_or(0)
            .max(c.last_read_inbox);
        self.histories
            .get(&chat_id)
            .is_some_and(|h| h.msgs().iter().any(|m| !m.outgoing && m.id > seen))
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
            self.composer.quote = None;
        }
        if edit.is_some() {
            self.composer.edit = edit;
        }
        self.composer.active = true;
        self.focus = Pane::Messages;
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        if self.composer.att_sel.is_some() {
            self.on_attachment_key(key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let suggestions = self.command_suggestions().len();
        if suggestions > 0 {
            match key.code {
                KeyCode::Tab => {
                    self.complete_command();
                    return;
                }
                KeyCode::Down => {
                    self.cmd_sel = (self.cmd_sel + 1).min(suggestions - 1);
                    return;
                }
                KeyCode::Char('n') if ctrl => {
                    self.cmd_sel = (self.cmd_sel + 1).min(suggestions - 1);
                    return;
                }
                KeyCode::Up => {
                    self.cmd_sel = self.cmd_sel.saturating_sub(1);
                    return;
                }
                KeyCode::Char('p') if ctrl => {
                    self.cmd_sel = self.cmd_sel.saturating_sub(1);
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Esc => {
                self.composer.active = false;
                self.focus = Pane::Messages;
                if self.composer.edit.take().is_some() {
                    self.composer.input.clear();
                }
                self.composer.reply_to = None;
                self.composer.quote = None;
            }
            KeyCode::Enter if key.modifiers.is_empty() => self.submit_composer(),
            KeyCode::Char('v') if ctrl => self.paste_clipboard(),
            KeyCode::Char('o') if ctrl => self.open_file_picker(),
            KeyCode::Char('r') if ctrl => self.start_recording(),
            KeyCode::Char('s') if ctrl => self.open_sticker_picker(0),
            KeyCode::Char('g') if ctrl => self.open_sticker_picker(2),
            KeyCode::Char('k') if ctrl => self.open_buttons(),
            KeyCode::Char('n') if ctrl => {
                self.open_next_unread();
            }
            KeyCode::Tab if !self.composer.attachments.is_empty() => {
                self.composer.att_sel = Some(self.composer.attachments.len() - 1);
            }
            KeyCode::Delete
                if !self.composer.attachments.is_empty() && self.composer.input.at_end() =>
            {
                self.composer.attachments.pop();
            }
            _ => {
                if let Edit::Changed = self.composer.input.handle_key(&key, true) {
                    self.cmd_sel = 0;
                    self.note_typing();
                }
            }
        }
    }

    fn on_attachment_key(&mut self, key: KeyEvent) {
        let c = &mut self.composer;
        let len = c.attachments.len();
        let Some(sel) = c.att_sel.filter(|s| *s < len) else {
            c.att_sel = None;
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('i') => c.att_sel = None,
            KeyCode::Enter => {
                c.att_sel = None;
                self.submit_composer();
            }
            KeyCode::Char('j') | KeyCode::Down => c.att_sel = Some((sel + 1).min(len - 1)),
            KeyCode::Char('k') | KeyCode::Up => c.att_sel = Some(sel.saturating_sub(1)),
            KeyCode::Char('J') if sel + 1 < len => {
                c.attachments.swap(sel, sel + 1);
                c.att_sel = Some(sel + 1);
            }
            KeyCode::Char('K') if sel > 0 => {
                c.attachments.swap(sel, sel - 1);
                c.att_sel = Some(sel - 1);
            }
            KeyCode::Char('f') => {
                let a = &mut c.attachments[sel];
                if a.can_toggle() {
                    a.as_file = !a.as_file;
                }
            }
            KeyCode::Char('d' | 'x') | KeyCode::Delete | KeyCode::Backspace => {
                c.attachments.remove(sel);
                c.att_sel = if c.attachments.is_empty() {
                    None
                } else {
                    Some(sel.min(c.attachments.len() - 1))
                };
            }
            _ => {}
        }
    }

    fn submit_composer(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        if let Some(why) = self.cannot_send(chat_id) {
            self.set_status(why, true);
            return;
        }
        let text = self.composer.input.text().trim_end().to_owned();
        let editing = self.composer.edit.is_some();
        if !self.composer.attachments.is_empty() {
            let items = std::mem::take(&mut self.composer.attachments);
            self.td.send_uploads(
                chat_id,
                items,
                text,
                self.composer.reply_to,
                self.uploads_dir.clone(),
                self.send_topic(),
            );
        } else if text.trim().is_empty() {
            return;
        } else {
            let quote = self
                .composer
                .reply_to
                .and(self.composer.quote.clone())
                .filter(|_| self.composer.edit.is_none());
            self.td.send_text(
                chat_id,
                text,
                self.composer.reply_to,
                self.composer.edit,
                self.send_topic(),
                quote,
            );
        }
        self.composer = Composer {
            active: true,
            ..Composer::default()
        };
        self.msg_sel = None;
        self.bottom_offset = 0;
        if self.settings.navigation.after_send_next_unread
            && !editing
            && !self.has_unread_after_view(chat_id)
        {
            self.open_next_unread();
        }
    }

    fn cannot_send(&self, chat_id: i64) -> Option<&'static str> {
        let c = self.chats.get(&chat_id)?;
        if !matches!(c.kind, ChatKind::Secret(_)) {
            return None;
        }
        match self.secret_chats.get(&c.secret_id).map(|s| s.state) {
            Some(model::SecretState::Pending) => {
                Some("secret chat is not ready yet: waiting for the other side to come online")
            }
            Some(model::SecretState::Closed) => Some("this secret chat is closed"),
            _ => None,
        }
    }

    fn can_attach(&mut self) -> bool {
        if self.open_chat.is_none() {
            self.set_status("open a chat first", false);
            return false;
        }
        if self.composer.edit.is_some() {
            self.set_status("attachments can't be added while editing a message", true);
            return false;
        }
        true
    }

    fn paste_clipboard(&mut self) {
        if self.composer.pasting {
            return;
        }
        self.composer.pasting = true;
        let tx = self.td.sender();
        let dir = self.uploads_dir.clone();
        tokio::spawn(async move {
            let result = upload::read_clipboard(dir).await;
            let _ = tx.send(Event::Clipboard(result));
        });
    }

    fn on_clipboard(&mut self, result: Result<Clip, String>) {
        self.composer.pasting = false;
        match result {
            Ok(Clip::Text(text)) => {
                if self.composer.active {
                    self.composer.input.insert_str(&text);
                }
            }
            Ok(Clip::Image(path)) => self.attach_paths(vec![path]),
            Ok(Clip::Files(paths)) => self.attach_paths(paths),
            Err(e) => self.set_status(e, true),
        }
    }

    pub fn attach_paths(&mut self, paths: Vec<PathBuf>) {
        if !self.can_attach() {
            return;
        }
        let mut errors = Vec::new();
        let mut added = 0;
        for p in paths {
            match Pending::from_path(&p) {
                Ok(mut item) => {
                    item.as_file =
                        self.settings.chats.photos_as_files && item.kind == upload::Kind::Photo;
                    self.composer.attachments.push(item);
                    added += 1;
                }
                Err(e) => errors.push(e),
            }
        }
        if added > 0 {
            self.composer.active = true;
            self.focus = Pane::Messages;
        }
        if !errors.is_empty() {
            self.set_status(errors.join("; "), true);
        } else if added > 0 {
            let n = self.composer.attachments.len();
            self.set_status(
                format!(
                    "{n} attachment{} · Enter send · Tab select · Delete remove last",
                    if n == 1 { "" } else { "s" }
                ),
                false,
            );
        }
    }

    fn open_file_picker(&mut self) {
        if !self.can_attach() {
            return;
        }
        self.start_composer(None, None);
        self.popup = Some(Popup::FilePicker(FilePicker {
            input: TextInput::with_text(&self.picker_last),
            sel: 0,
            dir: PathBuf::new(),
            entries: Vec::new(),
            error: None,
            requested: None,
        }));
        self.refresh_picker_listing();
    }

    fn refresh_picker_listing(&mut self) {
        let Some(Popup::FilePicker(fp)) = &mut self.popup else {
            return;
        };
        let (dir, _, _) = fp.query(&self.cwd, &self.home);
        if dir == fp.dir || fp.requested.as_ref() == Some(&dir) {
            return;
        }
        fp.requested = Some(dir.clone());
        let tx = self.td.sender();
        tokio::task::spawn_blocking(move || {
            let result = upload::list_dir(&dir);
            let _ = tx.send(Event::DirListing { dir, result });
        });
    }

    fn on_dir_listing(&mut self, dir: PathBuf, result: Result<Vec<DirEntry>, String>) {
        let Some(Popup::FilePicker(fp)) = &mut self.popup else {
            return;
        };
        if fp.requested.as_ref() != Some(&dir) {
            return;
        }
        fp.requested = None;
        fp.dir = dir;
        fp.sel = 0;
        match result {
            Ok(entries) => {
                fp.entries = entries;
                fp.error = None;
            }
            Err(e) => {
                fp.entries.clear();
                fp.error = Some(e);
            }
        }
    }

    fn on_file_picker_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let (cwd, home) = (self.cwd.clone(), self.home.clone());
        let Some(Popup::FilePicker(fp)) = &mut self.popup else {
            return;
        };
        let matches = fp.matches(&cwd, &home);
        let chosen = matches
            .get(fp.sel.min(matches.len().saturating_sub(1)))
            .map(|i| fp.entries[*i].clone());
        let (_, prefix, _) = fp.query(&cwd, &home);
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Down => fp.sel = (fp.sel + 1).min(matches.len().saturating_sub(1)),
            KeyCode::Char('n') if ctrl => {
                fp.sel = (fp.sel + 1).min(matches.len().saturating_sub(1))
            }
            KeyCode::Up => fp.sel = fp.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => fp.sel = fp.sel.saturating_sub(1),
            KeyCode::Tab | KeyCode::Enter => {
                let typed = upload::expand(fp.input.text(), &cwd, &home);
                match chosen {
                    Some(e) if e.dir => {
                        fp.input = TextInput::with_text(&format!("{prefix}{}/", e.name));
                        fp.sel = 0;
                        self.refresh_picker_listing();
                    }
                    Some(e) if key.code == KeyCode::Tab => {
                        fp.input = TextInput::with_text(&format!("{prefix}{}", e.name));
                    }
                    Some(e) => {
                        let path = upload::expand(&format!("{prefix}{}", e.name), &cwd, &home);
                        self.picker_last = prefix;
                        self.popup = None;
                        self.attach_paths(vec![path]);
                    }
                    None if key.code == KeyCode::Enter && typed.is_file() => {
                        self.picker_last = prefix;
                        self.popup = None;
                        self.attach_paths(vec![typed]);
                    }
                    None => {}
                }
            }
            _ => {
                if let Edit::Changed = fp.input.handle_key(&key, false) {
                    fp.sel = 0;
                    self.refresh_picker_listing();
                }
            }
        }
    }

    fn open_sticker_picker(&mut self, tab: usize) {
        if !self.can_attach() {
            return;
        }
        self.td.load_stickers();
        self.popup = Some(Popup::Stickers(stickers::Picker::new(tab)));
        self.ensure_tab_loaded();
    }

    fn ensure_tab_loaded(&mut self) {
        let Some(Popup::Stickers(sp)) = &self.popup else {
            return;
        };
        if let Tab::Set(id) = sp.current_tab(&self.stickers)
            && let Some(set) = self.stickers.sets.iter_mut().find(|s| s.id == id)
            && set.items.is_none()
            && !set.loading
        {
            set.loading = true;
            self.td.load_sticker_set(id);
        }
    }

    fn on_sticker_data(&mut self, data: stickers::Data) {
        if let Some((query, items)) = self.stickers.apply(data)
            && let Some(Popup::Stickers(sp)) = &mut self.popup
            && sp.query.text().trim() == query
        {
            sp.remote = items;
            sp.remote_for = query;
            sp.searching = false;
        }
        self.ensure_tab_loaded();
    }

    fn sticker_query_changed(&mut self) {
        let chat = self.open_chat.unwrap_or(0);
        let Some(Popup::Stickers(sp)) = &mut self.popup else {
            return;
        };
        sp.sel = 0;
        sp.scroll = 0;
        let q = sp.query.text().trim().to_owned();
        if !q.is_empty() && q != sp.remote_for {
            sp.searching = true;
            self.td.search_stickers(q, chat);
        }
    }

    fn on_sticker_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Popup::Stickers(sp)) = &mut self.popup else {
            return;
        };
        let items = sp.visible(&self.stickers);
        let cols = sp.cols.max(1) as isize;
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Tab => {
                sp.switch_tab(&self.stickers, 1);
                self.ensure_tab_loaded();
            }
            KeyCode::BackTab => {
                sp.switch_tab(&self.stickers, -1);
                self.ensure_tab_loaded();
            }
            KeyCode::Right => sp.move_sel(1, items.len()),
            KeyCode::Char('n') if ctrl => sp.move_sel(1, items.len()),
            KeyCode::Left => sp.move_sel(-1, items.len()),
            KeyCode::Char('p') if ctrl => sp.move_sel(-1, items.len()),
            KeyCode::Down => sp.move_sel(cols, items.len()),
            KeyCode::Up => sp.move_sel(-cols, items.len()),
            KeyCode::PageDown => sp.move_sel(cols * sp.rows.max(1) as isize, items.len()),
            KeyCode::PageUp => sp.move_sel(-cols * sp.rows.max(1) as isize, items.len()),
            KeyCode::Home => sp.move_sel(-(items.len() as isize), items.len()),
            KeyCode::End => sp.move_sel(items.len() as isize, items.len()),
            KeyCode::Enter => {
                let Some(item) = items.get(sp.sel).cloned() else {
                    return;
                };
                let Some(chat_id) = self.open_chat else {
                    return;
                };
                self.popup = None;
                let reply = self.composer.reply_to.take();
                let topic = self.send_topic();
                self.td.send_item(chat_id, item, reply, topic);
                self.msg_sel = None;
                self.bottom_offset = 0;
            }
            _ => {
                if let Edit::Changed = sp.query.handle_key(&key, false) {
                    self.sticker_query_changed();
                }
            }
        }
    }

    fn start_recording(&mut self) {
        let Some(chat_id) = self.open_chat else {
            self.set_status("open a chat first", false);
            return;
        };
        if self.recording.is_some() {
            return;
        }
        if self.composer.edit.is_some() {
            self.set_status("finish editing before recording a voice message", true);
            return;
        }
        let enc = match voice::check_tools() {
            Ok(e) => e,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        let rec = match voice::Recorder::start() {
            Ok(r) => r,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        let levels = rec.levels();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let tx = self.td.sender();
        let dir = self.uploads_dir.clone();
        tokio::spawn(async move {
            let result = match rec.run(stop_rx).await {
                Ok(Some(pcm)) => voice::encode(pcm, enc, &dir).await.map(Some),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            let _ = tx.send(Event::Voice(result));
        });
        self.status = None;
        self.focus = Pane::Messages;
        if self.settings.send_chat_actions() {
            self.td.send_chat_action(
                chat_id,
                tdlib_rs::enums::ChatAction::RecordingVoiceNote,
                self.send_topic(),
            );
        }
        self.recording = Some(Recording {
            started: Instant::now(),
            chat_id,
            topic: self.send_topic(),
            reply_to: self.composer.reply_to,
            stopping: false,
            stop: Some(stop_tx),
            levels,
        });
    }

    fn stop_recording(&mut self, how: Stop) {
        if let Some(r) = self.recording.as_mut()
            && let Some(stop) = r.stop.take()
        {
            r.stopping = true;
            let _ = stop.send(how);
        }
    }

    fn cancel_recording(&mut self) {
        self.stop_recording(Stop::Cancel);
    }

    fn on_record_key(&mut self, key: KeyEvent) {
        if self.recording.as_ref().is_some_and(|r| r.stopping) {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.stop_recording(Stop::Cancel),
            KeyCode::Enter => self.stop_recording(Stop::Send),
            KeyCode::Char('r') if ctrl => self.stop_recording(Stop::Send),
            _ => {}
        }
    }

    fn on_voice(&mut self, result: Result<Option<voice::VoiceNote>, String>) {
        let Some(rec) = self.recording.take() else {
            return;
        };
        match result {
            Ok(Some(note)) => {
                self.td
                    .send_voice(rec.chat_id, note, rec.reply_to, rec.topic);
                if rec.reply_to.is_some() && self.composer.reply_to == rec.reply_to {
                    self.composer.reply_to = None;
                }
                self.msg_sel = None;
                self.bottom_offset = 0;
            }
            Ok(None) => self.set_status("recording cancelled", false),
            Err(e) => self.set_status(format!("voice message: {e}"), true),
        }
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
            KeyCode::PageDown => self.move_selection(self.page()),
            KeyCode::PageUp => self.move_selection(-self.page()),
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

    pub fn selected_group(&self) -> Vec<&Msg> {
        let Some(h) = self.open_chat.and_then(|c| self.histories.get(&c)) else {
            return Vec::new();
        };
        let Some(sel) = self.msg_sel.filter(|s| *s < h.len()) else {
            return Vec::new();
        };
        let groups = model::groups(h.msgs());
        let g = groups[model::group_of(&groups, sel)].clone();
        h.msgs()[g].iter().collect()
    }

    pub fn selected_message(&self) -> Option<&Msg> {
        self.selected_group().first().copied()
    }

    fn group_attachments(&self) -> Vec<(i64, usize, Attachment)> {
        self.selected_group()
            .iter()
            .flat_map(|m| {
                m.attachments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (m.id, i, a.clone()))
            })
            .collect()
    }

    pub fn attachment(&self, chat_id: i64, msg_id: i64, idx: usize) -> Option<&Attachment> {
        self.histories
            .get(&chat_id)?
            .get(msg_id)?
            .attachments
            .get(idx)
    }

    fn open_viewer(&mut self, chat_id: i64, items: Vec<(i64, usize)>, pos: usize) {
        if items.is_empty() {
            self.set_status("nothing to view in this message", false);
            return;
        }
        self.anim_start = Instant::now();
        self.popup = Some(Popup::Viewer {
            chat_id,
            pos: pos.min(items.len() - 1),
            items,
        });
    }

    fn play_attachment(&mut self, chat_id: i64, msg_id: i64, idx: usize, att: Attachment) {
        if self
            .playing
            .as_ref()
            .is_some_and(|p| p.target == (msg_id, idx))
        {
            self.stop_playback();
            self.set_status("stopped", false);
            return;
        }
        let in_viewer = matches!(self.popup, Some(Popup::Viewer { .. }));
        let still = match att.kind {
            MediaKind::Photo => true,
            MediaKind::Sticker => att.full.as_ref().is_none_or(|v| v.decode != Decode::Video),
            _ => false,
        };
        let name = attachment_name(&att, msg_id);
        if still && !in_viewer {
            let items = self.viewable_items();
            let pos = items
                .iter()
                .position(|it| *it == (msg_id, idx))
                .unwrap_or(0);
            self.open_viewer(chat_id, items, pos);
        } else if still || att.kind == MediaKind::Document {
            self.queue_file_job(&att.file, &name, FileJob::Open);
        } else {
            self.queue_file_job(&att.file, &name, FileJob::Play(att.kind, (msg_id, idx)));
        }
    }

    fn viewable_items(&self) -> Vec<(i64, usize)> {
        self.group_attachments()
            .into_iter()
            .filter(|(_, _, a)| a.viewable())
            .map(|(m, i, _)| (m, i))
            .collect()
    }

    fn viewer_current(&self) -> Option<(i64, i64, usize, Attachment)> {
        let Some(Popup::Viewer {
            chat_id,
            items,
            pos,
        }) = &self.popup
        else {
            return None;
        };
        let (msg_id, idx) = *items.get(*pos)?;
        let att = self.attachment(*chat_id, msg_id, idx)?.clone();
        Some((*chat_id, msg_id, idx, att))
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
                } else if !self.composer.attachments.is_empty() {
                    self.set_status("send or remove the pending attachments first", true);
                } else {
                    self.td
                        .fetch_edit_text(msg.chat_id, msg.id, msg.body.clone());
                }
            }
            MenuAction::Delete => {
                let message_ids = self.selected_group().iter().map(|m| m.id).collect();
                self.popup = Some(Popup::ConfirmDelete {
                    chat_id: msg.chat_id,
                    message_ids,
                })
            }
            MenuAction::Play => match self.group_attachments().into_iter().next() {
                Some((msg_id, idx, att)) => self.play_attachment(msg.chat_id, msg_id, idx, att),
                None => self.set_status("no media in this message", false),
            },
            MenuAction::View => {
                let items = self.viewable_items();
                self.open_viewer(msg.chat_id, items, 0);
            }
            MenuAction::Save | MenuAction::OpenExternal => {
                match self.group_attachments().into_iter().next() {
                    Some((msg_id, _, att)) => {
                        let name = attachment_name(&att, msg_id);
                        let job = if action == MenuAction::Save {
                            FileJob::Save(name.clone())
                        } else {
                            FileJob::Open
                        };
                        self.queue_file_job(&att.file, &name, job);
                    }
                    None => self.set_status("no file in this message", false),
                }
            }
            MenuAction::CancelSend => {
                let ids: Vec<i64> = self
                    .selected_group()
                    .iter()
                    .filter(|m| m.state == model::SendState::Pending)
                    .map(|m| m.id)
                    .collect();
                if ids.is_empty() {
                    self.set_status("nothing is being sent", false);
                } else {
                    self.td.delete_messages(msg.chat_id, ids, true);
                    self.set_status("sending cancelled", false);
                }
            }
            MenuAction::OpenUrl => match format::links(&msg.body).into_iter().next() {
                Some(link) if link.hidden => self.popup = Some(Popup::ConfirmUrl(link.url)),
                Some(link) => {
                    player::xdg_open(&format::normalize_url(&link.url)).unwrap_or_else(|e| {
                        self.set_status(format!("open url: {e}"), true);
                    })
                }
                None => self.set_status("no link in this message", false),
            },
            MenuAction::Copy => {
                let text = self.group_text();
                if text.is_empty() {
                    self.set_status("no text to copy", false);
                } else {
                    self.copy_text(text);
                }
            }
            MenuAction::React => {
                self.popup = Some(Popup::Reactions(ReactionPicker {
                    chat_id: msg.chat_id,
                    msg_id: msg.id,
                    emojis: Vec::new(),
                    sel: 0,
                    loading: true,
                    note: None,
                }));
                self.td.load_available_reactions(msg.chat_id, msg.id);
            }
            MenuAction::Forward => {
                let ids = self.selected_group().iter().map(|m| m.id).collect();
                self.popup = Some(Popup::Switcher {
                    input: TextInput::default(),
                    sel: 0,
                    forward: Some((msg.chat_id, ids)),
                });
            }
            MenuAction::Pin => {
                self.td.pin_message(msg.chat_id, msg.id, !msg.pinned);
                self.set_status(
                    if msg.pinned {
                        "unpinning..."
                    } else {
                        "pinning..."
                    },
                    false,
                );
            }
            MenuAction::GoToReply => match msg.reply_to {
                Some(id) => self.jump_to(msg.chat_id, id),
                None => self.set_status("this message is not a reply", false),
            },
            MenuAction::SenderProfile => self.open_profile(match msg.sender {
                Sender::User(id) => ProfileTarget::User(id),
                Sender::Chat(id) => ProfileTarget::Chat(id),
            }),
            MenuAction::ReactionList => {
                if msg.reactions.is_empty() {
                    self.set_status("no reactions on this message", false);
                } else {
                    self.popup = Some(Popup::ReactionList {
                        chat_id: msg.chat_id,
                        msg_id: msg.id,
                        items: None,
                        sel: 0,
                    });
                    self.td.load_reaction_list(msg.chat_id, msg.id);
                }
            }
            MenuAction::Buttons => self.open_buttons(),
            MenuAction::Details => self.open_details(msg.chat_id, msg.id),
            MenuAction::EditHistory => {
                if self.archive.has_edits(msg.chat_id, msg.id) {
                    self.popup = Some(Popup::EditHistory {
                        chat_id: msg.chat_id,
                        msg_id: msg.id,
                        scroll: 0,
                    });
                } else {
                    self.set_status("no saved earlier versions of this message", false);
                }
            }
            MenuAction::FromSender => self.open_from_sender(msg.chat_id, msg.sender),
            MenuAction::CopyQuote => {
                let text = self.group_text();
                if text.is_empty() {
                    self.set_status("no text to quote", false);
                } else {
                    let when = format!(
                        "{} {}",
                        format::date_label(msg.date),
                        format::time_label(msg.date)
                    );
                    let author = self.sender_name(msg.sender);
                    self.copy_text(format::quote_block(&text, &author, &when));
                }
            }
            MenuAction::ReplyQuote => {
                if msg.body.text.trim().is_empty() {
                    self.set_status("no text to quote", false);
                } else {
                    self.popup = Some(Popup::QuotePick {
                        chat_id: msg.chat_id,
                        msg_id: msg.id,
                        input: TextInput::with_text(&msg.body.text),
                    });
                }
            }
            MenuAction::Translate => self.translate_message(&msg),
            MenuAction::SaveMessage => {
                let Some(target) = self.save_target() else {
                    self.set_status("not logged in yet", true);
                    return;
                };
                let ids = self.selected_group().iter().map(|m| m.id).collect();
                self.td.forward(target, msg.chat_id, ids);
                let name = self.save_target_name();
                self.set_status(format!("saved to {name}"), false);
            }
            MenuAction::CopyImage => {
                let photo = self
                    .group_attachments()
                    .into_iter()
                    .find(|(_, _, a)| a.kind == MediaKind::Photo);
                match photo {
                    Some((msg_id, _, att)) => {
                        let name = attachment_name(&att, msg_id);
                        self.queue_file_job(&att.file, &name, FileJob::CopyImage);
                    }
                    None => self.set_status("no photo in this message", false),
                }
            }
            _ => {}
        }
    }

    fn group_text(&self) -> String {
        self.selected_group()
            .iter()
            .map(|m| m.body.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn save_target(&self) -> Option<i64> {
        match self.settings.chats.save_to_chat {
            0 => self.my_id,
            id => Some(id),
        }
    }

    pub fn save_target_name(&self) -> String {
        match self.settings.chats.save_to_chat {
            0 => "Saved Messages".to_owned(),
            id => self.sender_name(Sender::Chat(id)),
        }
    }

    fn translate_blocked(&self, chat_id: i64) -> Option<&'static str> {
        let chat = self.chats.get(&chat_id)?;
        if matches!(chat.kind, ChatKind::Secret(_)) {
            Some("translation is off in secret chats: it would send the text to a server")
        } else if chat.protected {
            Some("translation is off in chats with protected content")
        } else {
            None
        }
    }

    fn translate_message(&mut self, msg: &Msg) {
        if let Some(why) = self.translate_blocked(msg.chat_id) {
            self.set_status(why, true);
            return;
        }
        if msg.body.text.trim().is_empty() {
            self.set_status("no text to translate", false);
            return;
        }
        let lang = self.settings.chats.translate_to.clone();
        self.popup = Some(Popup::Translate {
            chat_id: msg.chat_id,
            msg_id: msg.id,
            lang: lang.clone(),
            result: None,
            scroll: 0,
        });
        if msg.deleted {
            self.on_translated(
                msg.chat_id,
                msg.id,
                Err("the message was deleted".to_owned()),
            );
        } else {
            self.td.translate(msg.chat_id, msg.id, lang);
        }
    }

    fn on_translated(&mut self, chat_id: i64, message_id: i64, result: Result<String, String>) {
        let Some(Popup::Translate {
            chat_id: c,
            msg_id,
            result: slot,
            ..
        }) = &mut self.popup
        else {
            return;
        };
        if *c != chat_id || *msg_id != message_id {
            return;
        }
        *slot = Some(
            result.map_err(|e| format!("Telegram: {e}. Translation may need Telegram Premium")),
        );
    }

    fn open_action_menu(&mut self) {
        self.refresh_lists();
        let has_media = !self.group_attachments().is_empty();
        let sending = self
            .selected_group()
            .iter()
            .any(|m| m.state == model::SendState::Pending);
        let sel = self.selected_message();
        let pinned = sel.is_some_and(|m| m.pinned);
        let reply = sel.is_some_and(|m| m.reply_to.is_some());
        let reacted = sel.is_some_and(|m| !m.reactions.is_empty());
        let buttons = self
            .selected_group()
            .iter()
            .any(|m| m.keyboard.as_ref().is_some_and(|k| k.inline));
        let has_text = !self.group_text().is_empty();
        let translatable = sel.is_some_and(|m| self.translate_blocked(m.chat_id).is_none());
        let has_photo = self
            .group_attachments()
            .iter()
            .any(|(_, _, a)| a.kind == MediaKind::Photo);
        let items = match self.focus {
            Pane::Messages if self.msg_sel.is_some() => {
                let mut items = vec![
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
                ];
                if sending {
                    items.insert(
                        0,
                        MenuItem {
                            key: 'c',
                            label: "Cancel sending",
                            action: MenuAction::CancelSend,
                        },
                    );
                }
                items.extend([
                    MenuItem {
                        key: 'y',
                        label: "Copy text",
                        action: MenuAction::Copy,
                    },
                    MenuItem {
                        key: 'r',
                        label: "React",
                        action: MenuAction::React,
                    },
                    MenuItem {
                        key: 'f',
                        label: "Forward",
                        action: MenuAction::Forward,
                    },
                    MenuItem {
                        key: 'P',
                        label: if pinned { "Unpin" } else { "Pin" },
                        action: MenuAction::Pin,
                    },
                    MenuItem {
                        key: 'p',
                        label: "Sender profile",
                        action: MenuAction::SenderProfile,
                    },
                ]);
                if reply {
                    items.push(MenuItem {
                        key: 'g',
                        label: "Go to replied message",
                        action: MenuAction::GoToReply,
                    });
                }
                if reacted {
                    items.push(MenuItem {
                        key: 'u',
                        label: "Who reacted",
                        action: MenuAction::ReactionList,
                    });
                }
                if buttons {
                    items.push(MenuItem {
                        key: 'b',
                        label: "Bot buttons",
                        action: MenuAction::Buttons,
                    });
                }
                items.push(MenuItem {
                    key: 'i',
                    label: "Details",
                    action: MenuAction::Details,
                });
                items.push(MenuItem {
                    key: 'a',
                    label: "Messages from this sender",
                    action: MenuAction::FromSender,
                });
                if has_text {
                    items.extend([
                        MenuItem {
                            key: 'Y',
                            label: "Copy as quote",
                            action: MenuAction::CopyQuote,
                        },
                        MenuItem {
                            key: 'Q',
                            label: "Reply with quote",
                            action: MenuAction::ReplyQuote,
                        },
                    ]);
                    if translatable {
                        items.push(MenuItem {
                            key: 't',
                            label: "Translate",
                            action: MenuAction::Translate,
                        });
                    }
                }
                items.push(MenuItem {
                    key: 'S',
                    label: if self.settings.chats.save_to_chat == 0 {
                        "Save to Saved Messages"
                    } else {
                        "Save to the save target chat"
                    },
                    action: MenuAction::SaveMessage,
                });
                if has_photo {
                    items.push(MenuItem {
                        key: 'C',
                        label: "Copy photo",
                        action: MenuAction::CopyImage,
                    });
                }
                if sel.is_some_and(|m| self.archive.has_edits(m.chat_id, m.id)) {
                    items.push(MenuItem {
                        key: 'h',
                        label: "Edit history",
                        action: MenuAction::EditHistory,
                    });
                }
                if sel.is_some_and(|m| m.deleted) {
                    items.retain(|i| {
                        matches!(
                            i.action,
                            MenuAction::Copy
                                | MenuAction::CopyQuote
                                | MenuAction::SenderProfile
                                | MenuAction::GoToReply
                                | MenuAction::Details
                                | MenuAction::FromSender
                                | MenuAction::EditHistory
                        )
                    });
                }
                if has_media {
                    items.extend([
                        MenuItem {
                            key: 'x',
                            label: "Play / open media",
                            action: MenuAction::Play,
                        },
                        MenuItem {
                            key: 'v',
                            label: "View attachment",
                            action: MenuAction::View,
                        },
                        MenuItem {
                            key: 's',
                            label: "Save to Downloads",
                            action: MenuAction::Save,
                        },
                        MenuItem {
                            key: 'O',
                            label: "Open with xdg-open",
                            action: MenuAction::OpenExternal,
                        },
                    ]);
                }
                items
            }
            Pane::Chats if matches!(self.selected_row(), Some(Row::Topic(..))) => {
                let Some(Row::Topic(chat, topic)) = self.selected_row() else {
                    return;
                };
                self.popup = Some(Popup::Menu {
                    title: "Topic actions",
                    items: self.topic_menu(chat, topic),
                    sel: 0,
                });
                return;
            }
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
                MenuItem {
                    key: 'p',
                    label: "Profile",
                    action: MenuAction::ChatProfile,
                },
                MenuItem {
                    key: 'P',
                    label: "Pinned messages",
                    action: MenuAction::ShowPinned,
                },
                MenuItem {
                    key: '/',
                    label: "Search in chat",
                    action: MenuAction::SearchChat,
                },
            ],
            Pane::Folders => vec![MenuItem {
                key: 'l',
                label: "Load more chats",
                action: MenuAction::LoadMore,
            }],
            _ => return,
        };
        let mut items = items;
        if self.focus == Pane::Chats
            && let Some(chat) = self.selected_row().map(Row::chat)
        {
            items.extend(self.chat_extra_menu(chat));
        }
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

    pub fn is_admin(&self, chat: i64) -> bool {
        self.chats.get(&chat).is_some_and(|c| {
            matches!(c.kind, ChatKind::Supergroup | ChatKind::Channel)
                && self.groups.get(&c.group_id).is_some_and(|g| {
                    matches!(
                        g.status,
                        model::Membership::Creator | model::Membership::Admin(_)
                    )
                })
        })
    }

    fn open_members(&mut self, chat: i64) {
        let Some(c) = self.chats.get(&chat) else {
            return;
        };
        if !matches!(c.kind, ChatKind::Supergroup | ChatKind::Channel) {
            self.set_status("members list works in supergroups and channels", false);
            return;
        }
        self.popup = Some(Popup::Members(MembersView {
            chat_id: chat,
            supergroup_id: c.group_id,
            filter: crate::admin::MemberFilter::Recent,
            input: TextInput::default(),
            page: None,
            sel: 0,
            token: 0,
        }));
        self.reload_members();
    }

    fn reload_members(&mut self) {
        self.next_token += 1;
        let token = self.next_token;
        let Some(Popup::Members(mv)) = &mut self.popup else {
            return;
        };
        mv.token = token;
        mv.page = None;
        mv.sel = 0;
        self.td.load_members(
            token,
            mv.supergroup_id,
            mv.filter,
            mv.input.text().to_owned(),
        );
    }

    fn open_event_log(&mut self, chat: i64) {
        if !self.is_admin(chat) {
            self.set_status("recent actions are only visible to admins", false);
            return;
        }
        self.popup = Some(Popup::EventLog(LogView {
            chat_id: chat,
            entries: Vec::new(),
            sel: 0,
            loading: true,
            done: false,
            error: None,
        }));
        self.td.load_event_log(chat, 0);
    }

    fn on_members_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Popup::Members(mv)) = &mut self.popup else {
            return;
        };
        let len = match &mv.page {
            Some(Ok(p)) => p.members.len(),
            _ => 0,
        };
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Tab | KeyCode::BackTab => {
                mv.filter = mv.filter.step(key.code == KeyCode::Tab);
                self.reload_members();
            }
            KeyCode::Down => mv.sel = (mv.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Char('n') if ctrl => mv.sel = (mv.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Up => mv.sel = mv.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => mv.sel = mv.sel.saturating_sub(1),
            KeyCode::Enter => {
                let sender = match &mv.page {
                    Some(Ok(p)) => p.members.get(mv.sel).map(|m| m.sender),
                    _ => None,
                };
                if let Some(sender) = sender {
                    self.open_profile(match sender {
                        Sender::User(id) => ProfileTarget::User(id),
                        Sender::Chat(id) => ProfileTarget::Chat(id),
                    });
                }
            }
            _ => {
                if let Edit::Changed = mv.input.handle_key(&key, false) {
                    self.reload_members();
                }
            }
        }
    }

    fn on_event_log_key(&mut self, key: KeyEvent) {
        let Some(Popup::EventLog(lv)) = &mut self.popup else {
            return;
        };
        let len = lv.entries.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
            KeyCode::Char('j') | KeyCode::Down => lv.sel = (lv.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => lv.sel = lv.sel.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(e) = lv.entries.get(lv.sel) {
                    let who = e.target.unwrap_or(e.actor);
                    self.open_profile(match who {
                        Sender::User(id) => ProfileTarget::User(id),
                        Sender::Chat(id) => ProfileTarget::Chat(id),
                    });
                }
                return;
            }
            _ => return,
        }
        self.more_event_log();
    }

    fn more_event_log(&mut self) {
        let Some(Popup::EventLog(lv)) = &mut self.popup else {
            return;
        };
        if lv.loading || lv.done || lv.error.is_some() || lv.sel + 1 < lv.entries.len() {
            return;
        }
        let Some(last) = lv.entries.last().map(|e| e.id) else {
            return;
        };
        lv.loading = true;
        let chat = lv.chat_id;
        self.td.load_event_log(chat, last);
    }

    fn on_quote_key(&mut self, key: KeyEvent) {
        let Some(Popup::QuotePick {
            chat_id,
            msg_id,
            input,
        }) = &mut self.popup
        else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Enter => {
                let (chat_id, msg_id) = (*chat_id, *msg_id);
                let fragment = input.text().trim().to_owned();
                let full = self
                    .histories
                    .get(&chat_id)
                    .and_then(|h| h.get(msg_id))
                    .map(|m| m.body.text.clone())
                    .unwrap_or_default();
                if fragment.encode_utf16().count() > QUOTE_MAX {
                    self.set_status(
                        format!("a quote can be at most {QUOTE_MAX} characters"),
                        true,
                    );
                    return;
                }
                match format::quote_position(&full, &fragment) {
                    Some(pos) => {
                        self.popup = None;
                        self.start_composer(Some(msg_id), None);
                        self.composer.quote = Some((fragment, pos));
                    }
                    None => self.set_status(
                        "the quote must be a part of the message text; trim it with Backspace, C-w, C-u, Home/End",
                        true,
                    ),
                }
            }
            _ => {
                input.handle_key(&key, false);
            }
        }
    }

    fn chat_extra_menu(&self, chat: i64) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let Some(c) = self.chats.get(&chat) else {
            return items;
        };
        if self.is_admin(chat) {
            items.extend([
                MenuItem {
                    key: 'M',
                    label: "Members",
                    action: MenuAction::Members,
                },
                MenuItem {
                    key: 'L',
                    label: "Recent actions",
                    action: MenuAction::EventLog,
                },
            ]);
        }
        let (mentions, reactions) = self.unread_counts(chat, None);
        if mentions > 0 || reactions > 0 {
            items.push(MenuItem {
                key: 'R',
                label: "Read mentions and reactions",
                action: MenuAction::ReadMarks,
            });
        }
        let rights = self.rights(chat);
        if c.forum && rights.can_create_topic() {
            items.push(MenuItem {
                key: 'n',
                label: "New topic",
                action: MenuAction::NewTopic,
            });
        }
        let secret = matches!(c.kind, ChatKind::Secret(_));
        if model::can_set_ttl(&c.kind, rights) {
            items.push(MenuItem {
                key: 't',
                label: if secret {
                    "Self-destruct timer"
                } else {
                    "Auto-delete timer"
                },
                action: MenuAction::SetTtl,
            });
        }
        if secret {
            let closed = self
                .secret_chat(chat)
                .is_some_and(|s| s.state == model::SecretState::Closed);
            if !closed {
                items.push(MenuItem {
                    key: 'x',
                    label: "Close secret chat",
                    action: MenuAction::CloseSecret,
                });
            }
            items.push(MenuItem {
                key: 'D',
                label: "Delete secret chat",
                action: MenuAction::DeleteSecret,
            });
        }
        items
    }

    pub fn topic_menu(&self, chat: i64, topic: i32) -> Vec<MenuItem> {
        let Some(t) = self.topic(chat, topic) else {
            return Vec::new();
        };
        let rights = self.rights(chat);
        let perms = model::topic_perms(t, rights);
        let mut items = vec![
            MenuItem {
                key: 'o',
                label: "Open topic",
                action: MenuAction::OpenTopic,
            },
            MenuItem {
                key: 'm',
                label: "Mark topic as read",
                action: MenuAction::TopicMarkRead,
            },
            MenuItem {
                key: 'M',
                label: if self.topic_muted(chat, topic) {
                    "Unmute topic"
                } else {
                    "Mute topic"
                },
                action: MenuAction::TopicMute,
            },
        ];
        if t.mentions > 0 || t.reactions > 0 {
            items.push(MenuItem {
                key: 'R',
                label: "Read mentions and reactions",
                action: MenuAction::TopicReadMarks,
            });
        }
        if perms.pin {
            items.push(MenuItem {
                key: 'P',
                label: if t.pinned { "Unpin topic" } else { "Pin topic" },
                action: MenuAction::TopicPin,
            });
        }
        if perms.close {
            items.push(MenuItem {
                key: 'c',
                label: if t.closed {
                    "Reopen topic"
                } else {
                    "Close topic"
                },
                action: MenuAction::TopicClose,
            });
        }
        if perms.hide {
            items.push(MenuItem {
                key: 'h',
                label: if t.hidden {
                    "Show General"
                } else {
                    "Hide General"
                },
                action: MenuAction::TopicHideGeneral,
            });
        }
        items.push(MenuItem {
            key: 'y',
            label: "Copy topic link",
            action: MenuAction::TopicLink,
        });
        if rights.can_create_topic() {
            items.push(MenuItem {
                key: 'n',
                label: "New topic",
                action: MenuAction::NewTopic,
            });
        }
        items
    }

    fn topic_action(&mut self, action: MenuAction) {
        let Some((chat, topic)) = self.focused_topic() else {
            self.set_status("select a topic first", false);
            return;
        };
        let Some(t) = self.topic(chat, topic).cloned() else {
            return;
        };
        match action {
            MenuAction::OpenTopic => self.enter_topic(chat, topic),
            MenuAction::TopicMarkRead => self.mark_topic_read(chat, topic),
            MenuAction::TopicReadMarks => self.read_marks(chat, Some(topic)),
            MenuAction::TopicMute => {
                let mute = !self.topic_muted(chat, topic);
                self.td.set_topic_muted(chat, topic, mute);
                if let Some(x) = self.topics.get_mut(&chat).and_then(|l| l.get_mut(topic)) {
                    x.use_default_mute = false;
                    x.muted = mute;
                }
                self.set_status(if mute { "topic muted" } else { "topic unmuted" }, false);
            }
            MenuAction::TopicPin => self.td.pin_topic(chat, topic, !t.pinned),
            MenuAction::TopicClose => self.td.close_topic(chat, topic, !t.closed),
            MenuAction::TopicHideGeneral => self.td.hide_general(chat, !t.hidden),
            MenuAction::TopicLink => self.td.topic_link(chat, topic),
            _ => {}
        }
    }

    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::OpenTopic
            | MenuAction::TopicMarkRead
            | MenuAction::TopicReadMarks
            | MenuAction::TopicMute
            | MenuAction::TopicPin
            | MenuAction::TopicClose
            | MenuAction::TopicHideGeneral
            | MenuAction::TopicLink => self.topic_action(action),
            MenuAction::NewTopic => {
                if let Some(chat) = self.focused_chat() {
                    self.popup = Some(Popup::NewTopic {
                        chat_id: chat,
                        input: TextInput::default(),
                    });
                }
            }
            MenuAction::SetTtl => {
                let Some(chat) = self.focused_chat() else {
                    return;
                };
                let Some(c) = self.chats.get(&chat) else {
                    return;
                };
                let choices = model::ttl_choices(matches!(c.kind, ChatKind::Secret(_)));
                let sel = choices
                    .iter()
                    .position(|v| *v == c.auto_delete)
                    .unwrap_or(0);
                self.popup = Some(Popup::Ttl {
                    chat_id: chat,
                    choices,
                    sel,
                });
            }
            MenuAction::CloseSecret | MenuAction::DeleteSecret => {
                let Some(chat) = self.focused_chat() else {
                    return;
                };
                let Some(secret_id) = self.chats.get(&chat).map(|c| c.secret_id) else {
                    return;
                };
                self.popup = Some(Popup::ConfirmSecret {
                    chat_id: chat,
                    secret_id,
                    delete: action == MenuAction::DeleteSecret,
                });
            }
            MenuAction::Reply
            | MenuAction::Edit
            | MenuAction::Delete
            | MenuAction::OpenUrl
            | MenuAction::Play
            | MenuAction::View
            | MenuAction::Save
            | MenuAction::OpenExternal
            | MenuAction::CancelSend
            | MenuAction::Copy
            | MenuAction::React
            | MenuAction::Forward
            | MenuAction::Pin
            | MenuAction::GoToReply
            | MenuAction::SenderProfile
            | MenuAction::ReactionList
            | MenuAction::Buttons
            | MenuAction::Details
            | MenuAction::EditHistory
            | MenuAction::FromSender
            | MenuAction::CopyQuote
            | MenuAction::ReplyQuote
            | MenuAction::Translate
            | MenuAction::SaveMessage
            | MenuAction::CopyImage => self.message_action(action),
            MenuAction::Members => {
                if let Some(chat) = self.focused_chat() {
                    self.open_members(chat);
                }
            }
            MenuAction::EventLog => {
                if let Some(chat) = self.focused_chat() {
                    self.open_event_log(chat);
                }
            }
            MenuAction::OpenChat => self.on_enter(),
            MenuAction::MarkRead => {
                if let Some(chat) = self.focused_chat() {
                    self.mark_chat_read(chat);
                }
            }
            MenuAction::ReadMarks => {
                if let Some(chat) = self.focused_chat() {
                    self.read_marks(chat, None);
                }
            }
            MenuAction::ChatProfile => {
                if let Some(chat) = self.focused_chat() {
                    self.open_profile(ProfileTarget::Chat(chat));
                }
            }
            MenuAction::ShowPinned => {
                if let Some(chat) = self.focused_chat() {
                    self.open_search(Some(chat), true);
                }
            }
            MenuAction::SearchChat => {
                if let Some(chat) = self.focused_chat() {
                    self.open_search(Some(chat), false);
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

    fn page_popup(&mut self, key: &KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let edge = matches!(key.code, KeyCode::Home | KeyCode::End);
        let Some(delta) = page_delta(key.code, self.popup_rows.get()) else {
            return false;
        };
        let typed_len = match &self.popup {
            Some(Popup::Switcher { input, .. }) => self.switcher_results(input.text()).len(),
            Some(Popup::FilePicker(fp)) => fp.matches(&self.cwd, &self.home).len(),
            _ => 0,
        };
        let inbox_len = self.inbox.len();
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        match popup {
            Popup::Menu { items, sel, .. } => *sel = step(*sel, delta, items.len()),
            Popup::Viewer { items, pos, .. } => {
                let delta = page_delta(key.code, VIEWER_PAGE).unwrap_or(delta);
                let next = step(*pos, delta, items.len());
                if next != *pos {
                    *pos = next;
                    self.anim_start = Instant::now();
                }
            }
            Popup::Switcher { sel, .. } | Popup::FilePicker(FilePicker { sel, .. }) if !edge => {
                *sel = step(*sel, delta, typed_len)
            }
            Popup::Profile(p) => match key.code {
                KeyCode::End => return false,
                KeyCode::Home => p.scroll = 0,
                _ => {
                    let page = u16::try_from(delta.unsigned_abs()).unwrap_or(u16::MAX);
                    p.scroll = if delta < 0 {
                        p.scroll.saturating_sub(page)
                    } else {
                        p.scroll.saturating_add(page)
                    };
                }
            },
            Popup::Reactions(r) => r.sel = step(r.sel, delta, r.emojis.len()),
            Popup::ReactionList { items, sel, .. } => {
                *sel = step(*sel, delta, items.as_ref().map_or(0, Vec::len))
            }
            Popup::Search(sv) if sv.listing() || !edge => {
                sv.sel = step(sv.sel, delta, sv.results.len())
            }
            Popup::Buttons(b) => b.sel = step(b.sel, delta, b.keyboard.buttons().len()),
            Popup::Options { sel } => *sel = shift_option(*sel, delta),
            Popup::Inbox { sel } => *sel = step(*sel, delta, inbox_len),
            Popup::Ttl { choices, sel, .. } => *sel = step(*sel, delta, choices.len()),
            _ => return false,
        }
        true
    }

    fn on_popup_key(&mut self, key: KeyEvent) {
        if self.page_popup(&key) {
            return;
        }
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match popup {
            Popup::FilePicker(_) => self.on_file_picker_key(key),
            Popup::Stickers(_) => self.on_sticker_key(key),
            Popup::Profile(_)
            | Popup::Reactions(_)
            | Popup::ReactionList { .. }
            | Popup::Search(_)
            | Popup::Buttons(_)
            | Popup::ConfirmUrl(_)
            | Popup::Options { .. }
            | Popup::Inbox { .. } => self.on_stage4_popup_key(key),
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
            Popup::Viewer { items, pos, .. } => {
                let len = items.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') => self.popup = None,
                    KeyCode::Char('j' | 'l' | 'n') | KeyCode::Down | KeyCode::Right => {
                        if *pos + 1 < len {
                            *pos += 1;
                            self.anim_start = Instant::now();
                        }
                    }
                    KeyCode::Char('k' | 'h' | 'p') | KeyCode::Up | KeyCode::Left => {
                        if *pos > 0 {
                            *pos -= 1;
                            self.anim_start = Instant::now();
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Enter => {
                        if let Some((chat_id, msg_id, idx, att)) = self.viewer_current() {
                            self.play_attachment(chat_id, msg_id, idx, att);
                        }
                    }
                    KeyCode::Char('s') | KeyCode::Char('o') | KeyCode::Char('O') => {
                        if let Some((_, msg_id, _, att)) = self.viewer_current() {
                            let name = attachment_name(&att, msg_id);
                            let job = if key.code == KeyCode::Char('s') {
                                FileJob::Save(name.clone())
                            } else {
                                FileJob::Open
                            };
                            self.queue_file_job(&att.file, &name, job);
                        }
                    }
                    _ => {}
                }
            }
            Popup::ConfirmDelete {
                chat_id,
                message_ids,
            } => {
                let (chat_id, message_ids) = (*chat_id, message_ids.clone());
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        self.popup = None;
                        self.td.delete_messages(chat_id, message_ids, true);
                    }
                    KeyCode::Char('m') => {
                        self.popup = None;
                        self.td.delete_messages(chat_id, message_ids, false);
                    }
                    KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => self.popup = None,
                    _ => {}
                }
            }
            Popup::NewTopic { chat_id, input } => match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Enter => {
                    let name = input.text().trim().to_owned();
                    let chat_id = *chat_id;
                    if name.is_empty() {
                        self.set_status("type a topic name", false);
                    } else {
                        self.popup = None;
                        self.td.create_topic(chat_id, name);
                        self.set_status("creating topic...", false);
                    }
                }
                _ => {
                    input.handle_key(&key, false);
                }
            },
            Popup::Ttl {
                chat_id,
                choices,
                sel,
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => *sel = (*sel + 1) % choices.len(),
                KeyCode::Char('k') | KeyCode::Up => {
                    *sel = (*sel + choices.len() - 1) % choices.len()
                }
                KeyCode::Enter => {
                    let (chat_id, secs) = (*chat_id, choices[*sel]);
                    self.popup = None;
                    self.td.set_auto_delete(chat_id, secs);
                    self.set_status(format!("timer: {}", format::ttl(secs)), false);
                }
                _ => {}
            },
            Popup::Members(_) => self.on_members_key(key),
            Popup::EventLog(_) => self.on_event_log_key(key),
            Popup::QuotePick { .. } => self.on_quote_key(key),
            Popup::Translate { scroll, result, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Char('y') => {
                    if let Some(Ok(text)) = result {
                        let text = text.clone();
                        self.copy_text(text);
                    }
                }
                _ => {}
            },
            Popup::Details { scroll, .. } | Popup::EditHistory { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                _ => {}
            },
            Popup::ConfirmClearDeleted => match key.code {
                KeyCode::Char('y') => {
                    self.popup = Some(Popup::Options {
                        sel: CLEAR_DELETED_OPTION,
                    });
                    self.clear_deleted();
                }
                KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                    self.popup = Some(Popup::Options {
                        sel: CLEAR_DELETED_OPTION,
                    })
                }
                _ => {}
            },
            Popup::ConfirmSecret {
                chat_id,
                secret_id,
                delete,
            } => {
                let (chat_id, secret_id, delete) = (*chat_id, *secret_id, *delete);
                match key.code {
                    KeyCode::Char('y') => {
                        self.popup = None;
                        let open = self
                            .secret_chats
                            .get(&secret_id)
                            .is_none_or(|s| s.state != model::SecretState::Closed);
                        if delete {
                            self.td.delete_secret_chat(chat_id, secret_id, open);
                            if self.open_chat == Some(chat_id) {
                                self.td.close_chat(chat_id);
                                self.histories.remove(&chat_id);
                                self.open_chat = None;
                                self.open_topic = None;
                                self.focus = Pane::Chats;
                            }
                            self.set_status("secret chat deleted", false);
                        } else {
                            self.td.close_secret_chat(secret_id);
                            self.set_status("secret chat closed", false);
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                        self.popup = None
                    }
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
            Popup::Switcher {
                input,
                sel,
                forward,
            } => match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down => *sel += 1,
                KeyCode::Char('n') if ctrl => *sel += 1,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Char('p') if ctrl => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    let query = input.text().to_owned();
                    let sel = *sel;
                    let forward = forward.take();
                    let results = self.switcher_results(&query);
                    self.popup = None;
                    if let Some(id) = results.get(sel.min(results.len().saturating_sub(1))) {
                        match forward {
                            Some((from, ids)) => {
                                self.td.forward(*id, from, ids);
                                let title = self.sender_name(Sender::Chat(*id));
                                self.set_status(format!("forwarded to {title}"), false);
                            }
                            None => {
                                let id = *id;
                                self.open_any(id);
                            }
                        }
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
        let ids: Vec<i64> = std::mem::take(&mut self.want_emoji)
            .into_iter()
            .filter(|id| self.custom_emoji_asked.insert(*id))
            .collect();
        if !ids.is_empty() {
            self.td.load_custom_emoji(ids);
        }
        self.media.end_frame(&self.td);
    }
}

const WAITING_BOT: &str = "waiting for the bot...";
const INBOX_MAX: usize = 200;
const TYPING_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl App {
    pub fn apply_online(&mut self) {
        if self.auth != Auth::Ready {
            return;
        }
        let want = self.settings.online(self.term_focused);
        if self.online_sent != Some(want) {
            self.online_sent = Some(want);
            self.td.set_online(want);
        }
    }

    fn note_typing(&mut self) {
        if !self.settings.send_chat_actions()
            || self.composer.edit.is_some()
            || self.composer.input.text().trim().is_empty()
        {
            return;
        }
        let Some(chat) = self.open_chat else {
            return;
        };
        let now = Instant::now();
        if self
            .typing_sent
            .is_some_and(|(c, t)| c == chat && now.duration_since(t) < TYPING_EVERY)
        {
            return;
        }
        self.typing_sent = Some((chat, now));
        let topic = self.send_topic();
        self.td
            .send_chat_action(chat, tdlib_rs::enums::ChatAction::Typing, topic);
    }

    pub fn focused_chat(&mut self) -> Option<i64> {
        if self.focus == Pane::Chats {
            self.refresh_lists();
            if let Some(row) = self.selected_row() {
                return Some(row.chat());
            }
        }
        self.open_chat
    }

    fn focused_topic(&mut self) -> Option<(i64, i32)> {
        if self.focus == Pane::Chats {
            self.refresh_lists();
            return match self.selected_row() {
                Some(Row::Topic(c, t)) => Some((c, t)),
                _ => None,
            };
        }
        Some((self.open_chat?, self.open_topic?))
    }

    pub fn mark_topic_read(&mut self, chat_id: i64, topic: i32) {
        let newest = self
            .histories
            .get(&chat_id)
            .filter(|h| h.topic == Some(topic))
            .and_then(History::newest_id)
            .or_else(|| {
                self.topic(chat_id, topic)
                    .and_then(|t| t.last_message.as_ref().map(|m| m.id))
            });
        let Some(id) = newest else {
            self.set_status("nothing to mark as read", false);
            return;
        };
        if self.open_chat == Some(chat_id) && self.open_topic == Some(topic) {
            self.last_viewed.insert(chat_id, id);
        }
        self.td.view_messages(chat_id, vec![id], Some(topic));
        let (mentions, reactions) = self.unread_counts(chat_id, Some(topic));
        if mentions > 0 {
            self.td.read_topic_mentions(chat_id, topic);
        }
        if reactions > 0 {
            self.td.read_topic_reactions(chat_id, topic);
        }
        if let Some(list) = self.topics.get_mut(&chat_id) {
            list.mark_read_up_to(topic, id);
        }
        self.set_status("topic marked as read", false);
    }

    pub fn read_marks(&mut self, chat_id: i64, topic: Option<i32>) {
        let (mentions, reactions) = self.unread_counts(chat_id, topic);
        if mentions <= 0 && reactions <= 0 {
            self.set_status("no unread mentions or reactions", false);
            return;
        }
        match topic {
            Some(t) => {
                if mentions > 0 {
                    self.td.read_topic_mentions(chat_id, t);
                }
                if reactions > 0 {
                    self.td.read_topic_reactions(chat_id, t);
                }
            }
            None => {
                if mentions > 0 {
                    self.td.read_all_mentions(chat_id);
                }
                if reactions > 0 {
                    self.td.read_all_reactions(chat_id);
                }
            }
        }
        if let Some(h) = self.histories.get_mut(&chat_id) {
            let ids: Vec<i64> = h
                .msgs()
                .iter()
                .filter(|m| topic.is_none() || m.topic == topic)
                .map(|m| m.id)
                .collect();
            for id in ids {
                if let Some(m) = h.get_mut(id) {
                    m.mention = false;
                    m.unread_reaction = false;
                }
            }
        }
        self.set_status("mentions and reactions marked as read", false);
    }

    pub fn mark_chat_read(&mut self, chat_id: i64) {
        if self.is_forum(chat_id) {
            if self.open_chat == Some(chat_id)
                && let Some(t) = self.open_topic
            {
                self.mark_topic_read(chat_id, t);
                return;
            }
            let unread: Vec<(i32, i64)> = self
                .topics
                .get(&chat_id)
                .map(|l| {
                    l.topics()
                        .iter()
                        .filter(|t| t.unread > 0 || t.mentions > 0 || t.reactions > 0)
                        .filter_map(|t| t.last_message.as_ref().map(|m| (t.id, m.id)))
                        .collect()
                })
                .unwrap_or_default();
            if !unread.is_empty() {
                for (t, _) in &unread {
                    self.mark_topic_read(chat_id, *t);
                }
                self.set_status(format!("{} topics marked as read", unread.len()), false);
                return;
            }
        }
        let newest = self
            .histories
            .get(&chat_id)
            .and_then(History::newest_id)
            .or_else(|| {
                self.chats
                    .get(&chat_id)
                    .and_then(|c| c.last_message.as_ref().map(|m| m.id))
            });
        let Some(id) = newest else {
            self.set_status("nothing to mark as read", false);
            return;
        };
        self.last_viewed.insert(chat_id, id);
        self.td.view_messages(chat_id, vec![id], None);
        let (mentions, reactions) = self.unread_counts(chat_id, None);
        if mentions > 0 {
            self.td.read_all_mentions(chat_id);
        }
        if reactions > 0 {
            self.td.read_all_reactions(chat_id);
        }
        self.set_status("marked as read", false);
    }

    pub fn is_muted(&self, chat_id: i64) -> bool {
        let Some(c) = self.chats.get(&chat_id) else {
            return false;
        };
        if !c.use_default_mute {
            return c.muted;
        }
        let scope = match c.kind {
            ChatKind::Private(_) | ChatKind::Secret(_) => 0,
            ChatKind::Group | ChatKind::Supergroup => 1,
            ChatKind::Channel => 2,
        };
        self.scope_muted[scope]
    }

    fn on_incoming(&mut self, msg: &Msg) {
        if msg.outgoing {
            return;
        }
        let following = self.msg_sel.is_none() && self.bottom_offset == 0;
        let sound_on = self.settings.notifications.sound && !self.sound_broken;
        let cand = notify::Candidate {
            enabled: self.settings.notifications.desktop && !self.notify_broken,
            outgoing: msg.outgoing,
            muted: match msg.topic {
                Some(t) => self.topic_muted(msg.chat_id, t),
                None => self.is_muted(msg.chat_id),
            },
            mention: msg.mention,
            viewing: self.term_focused
                && self.open_chat == Some(msg.chat_id)
                && following
                && self
                    .histories
                    .get(&msg.chat_id)
                    .is_some_and(|h| h.accepts(msg)),
            age: unix_now() - msg.date,
        };
        if !notify::worth_inbox(&cand) {
            return;
        }
        let preview = model::preview(msg);
        self.inbox.push_front(InboxItem {
            chat_id: msg.chat_id,
            msg_id: msg.id,
            sender: msg.sender,
            preview: preview.clone(),
            date: msg.date,
            mention: msg.mention,
            topic: msg.topic,
        });
        self.inbox.truncate(INBOX_MAX);
        if notify::should_notify(&notify::Candidate {
            enabled: sound_on,
            ..cand
        }) {
            self.play_notification_sound();
        }
        if !notify::should_notify(&cand) {
            return;
        }
        let chat = self.sender_name(Sender::Chat(msg.chat_id));
        let from = self.sender_name(msg.sender);
        let private = self
            .chats
            .get(&msg.chat_id)
            .is_some_and(|c| matches!(c.kind, ChatKind::Private(_) | ChatKind::Secret(_)));
        let body = if private || from == chat {
            preview
        } else {
            format!("{from}: {preview}")
        };
        let icon = self.notify_icon(msg, private);
        self.td.notify(chat, format::truncate(&body, 300), icon);
    }

    fn notify_icon(&self, msg: &Msg, private: bool) -> notify::Icon {
        let photo = match msg.sender {
            Sender::User(id) if private => self.user_meta.get(&id).and_then(|u| u.photo.as_ref()),
            _ => self.chats.get(&msg.chat_id).and_then(|c| c.photo.as_ref()),
        };
        match photo.map(|p| self.media.file(p)) {
            Some(f) => match f.local() {
                Some(path) => notify::Icon::Path(path.to_owned()),
                None if f.id != 0 => notify::Icon::Download(f.id),
                None => notify::Icon::None,
            },
            None => notify::Icon::None,
        }
    }

    fn play_notification_sound(&mut self) {
        let env = self.sound_env.get_or_insert_with(notify::SoundEnv::detect);
        let file = match crate::config::cache_dir() {
            Ok(dir) => dir.join(notify::DEFAULT_SOUND_NAME),
            Err(e) => {
                self.sound_broken = true;
                self.set_status(format!("notification sound: {e}"), true);
                return;
            }
        };
        match notify::sound_command(&self.settings.notifications, env, &file) {
            Ok(Some(cmd)) => self.td.play_sound(cmd, file),
            Ok(None) => {}
            Err(e) => {
                self.sound_broken = true;
                self.set_status(format!("{e}; turn the sound off in Space o"), true);
            }
        }
    }

    pub fn prune_typing(&mut self, now: Instant) {
        self.typing.retain(|_, list| {
            list.retain(|t| !t.expired(now));
            !list.is_empty()
        });
    }

    pub fn typing_line(&self, chat_id: i64, topic: Option<i32>) -> Option<String> {
        let now = Instant::now();
        let named: Vec<(String, Activity)> = self
            .typing
            .get(&chat_id)?
            .iter()
            .filter(|t| !t.expired(now) && (topic.is_none() || t.topic == topic))
            .map(|t| {
                let name = self.sender_name(t.sender);
                (
                    name.split(' ').next().unwrap_or(&name).to_owned(),
                    t.activity,
                )
            })
            .collect();
        presence::typing_text_in(self.lang, &named, false)
    }

    pub fn user_status(&self, user_id: i64) -> Option<String> {
        if self.my_id == Some(user_id) {
            return None;
        }
        let meta = self.user_meta.get(&user_id)?;
        Some(presence::status_text_in(
            self.lang,
            meta.status,
            meta.bot,
            unix_now(),
        ))
    }

    pub fn sender_photo(&self, sender: Sender) -> Option<FileRef> {
        match sender {
            Sender::User(id) => self.user_meta.get(&id).and_then(|u| u.photo.clone()),
            Sender::Chat(id) => self.chats.get(&id).and_then(|c| c.photo.clone()),
        }
    }

    pub fn profile_photo(&self, target: ProfileTarget) -> Option<FileRef> {
        match target {
            ProfileTarget::User(id) => self.sender_photo(Sender::User(id)),
            ProfileTarget::Chat(id) => {
                self.sender_photo(Sender::Chat(id))
                    .or_else(|| match self.chats.get(&id)?.kind {
                        ChatKind::Private(uid) | ChatKind::Secret(uid) => {
                            self.sender_photo(Sender::User(uid))
                        }
                        _ => None,
                    })
            }
        }
    }

    pub fn is_online(&self, user_id: i64) -> bool {
        self.my_id != Some(user_id)
            && self
                .user_meta
                .get(&user_id)
                .is_some_and(|m| m.status.is_online(unix_now()))
    }

    pub fn chat_subtitle(&self, chat_id: i64) -> Option<String> {
        let c = self.chats.get(&chat_id)?;
        let mut parts: Vec<String> = Vec::new();
        if let ChatKind::Private(uid) | ChatKind::Secret(uid) = c.kind
            && let Some(s) = self.user_status(uid)
        {
            parts.push(s);
        }
        if let Some(sc) = self.secret_chat(chat_id) {
            parts.push(sc.state.header().to_owned());
        }
        if c.auto_delete > 0 {
            let g = crate::theme::get().glyphs.timer.clone();
            parts.push(format!("{g} {}", format::ttl(c.auto_delete)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    pub fn view_title(&self) -> String {
        let Some(c) = self.open_chat.and_then(|id| self.chats.get(&id)) else {
            return "Messages".to_owned();
        };
        match self.open_topic_info() {
            Some(t) => {
                let closed = if t.closed { " (closed)" } else { "" };
                format!("{} › {}{closed}", c.title, t.name)
            }
            None if c.forum => format!("{} › all topics", c.title),
            None => c.title.clone(),
        }
    }

    pub fn command_suggestions(&self) -> Vec<&Command> {
        let c = &self.composer;
        if !c.active || c.edit.is_some() || !c.attachments.is_empty() {
            return Vec::new();
        }
        match self.open_chat.and_then(|id| self.bot_commands.get(&id)) {
            Some(list) => bots::filter_commands(list, c.input.text()),
            None => Vec::new(),
        }
    }

    fn complete_command(&mut self) {
        let multiple = self
            .open_chat
            .and_then(|id| self.bot_commands.get(&id))
            .is_some_and(|l| l.iter().any(|c| l.first().is_some_and(|f| f.bot != c.bot)));
        let list = self.command_suggestions();
        let Some(cmd) = list.get(self.cmd_sel.min(list.len().saturating_sub(1))) else {
            return;
        };
        let text = bots::completion(cmd, multiple);
        self.composer.input = TextInput::with_text(&text);
        self.cmd_sel = 0;
    }

    fn ensure_reply_keyboard(&mut self, chat_id: i64) {
        let Some(&id) = self.reply_kb_ids.get(&chat_id) else {
            return;
        };
        if self.reply_kb.contains_key(&chat_id) {
            return;
        }
        if let Some(m) = self.histories.get(&chat_id).and_then(|h| h.get(id)) {
            let kb = m.keyboard.clone().filter(|k| !k.inline);
            self.reply_kb.insert(chat_id, kb);
            return;
        }
        self.reply_kb.insert(chat_id, None);
        self.td.fetch_message(chat_id, id);
    }

    #[cfg(test)]
    pub fn set_reply_keyboard_id(&mut self, chat_id: i64, msg_id: i64) {
        self.reply_kb_ids.insert(chat_id, msg_id);
    }

    pub fn reply_keyboard(&self) -> Option<(i64, &Keyboard)> {
        let chat = self.open_chat?;
        let id = *self.reply_kb_ids.get(&chat)?;
        let from_history = self
            .histories
            .get(&chat)
            .and_then(|h| h.get(id))
            .and_then(|m| m.keyboard.as_ref())
            .filter(|k| !k.inline);
        from_history
            .or_else(|| self.reply_kb.get(&chat).and_then(Option::as_ref))
            .map(|k| (id, k))
    }

    fn open_buttons(&mut self) {
        let inline = self.selected_group().iter().find_map(|m| {
            m.keyboard
                .as_ref()
                .filter(|k| k.inline)
                .map(|k| (m.chat_id, m.id, k.clone()))
        });
        let target = inline.or_else(|| {
            let chat = self.open_chat?;
            self.reply_keyboard().map(|(id, k)| (chat, id, k.clone()))
        });
        match target {
            Some((chat_id, msg_id, keyboard)) if !keyboard.buttons().is_empty() => {
                self.popup = Some(Popup::Buttons(ButtonsView {
                    chat_id,
                    msg_id,
                    keyboard,
                    sel: 0,
                }));
            }
            _ => self.set_status("no bot buttons here (select a message with buttons)", false),
        }
    }

    fn press_button(&mut self, chat_id: i64, msg_id: i64, button: bots::Button) {
        self.popup = None;
        match button.action {
            ButtonAction::Callback(data) => {
                self.td.press_callback(chat_id, msg_id, data);
                self.set_status(WAITING_BOT, false);
            }
            ButtonAction::Url(url) => self.popup = Some(Popup::ConfirmUrl(url)),
            ButtonAction::User(id) => self.open_profile(ProfileTarget::User(id)),
            ButtonAction::CopyText(text) => self.copy_text(text),
            ButtonAction::Text => {
                let topic = self.send_topic();
                self.td
                    .send_text(chat_id, button.text, None, None, topic, None);
                self.msg_sel = None;
                self.bottom_offset = 0;
            }
            ButtonAction::Unsupported(what) => {
                self.set_status(format!("{what} are not supported in gramtty"), false)
            }
        }
    }

    fn copy_text(&mut self, text: String) {
        self.td.copy_text(text);
    }

    pub fn can_start_secret(&self, user_id: i64) -> bool {
        self.my_id != Some(user_id) && !self.user_meta.get(&user_id).is_some_and(|m| m.bot)
    }

    pub fn open_profile(&mut self, target: ProfileTarget) {
        self.popup = Some(Popup::Profile(ProfileView {
            target,
            data: None,
            error: None,
            scroll: 0,
        }));
        self.td.load_profile(target);
    }

    fn open_search(&mut self, chat_id: Option<i64>, pinned: bool) {
        self.popup = Some(Popup::Search(SearchView {
            chat_id,
            pinned,
            sender: None,
            input: TextInput::default(),
            searched_for: None,
            results: Vec::new(),
            sel: 0,
            token: 0,
            loading: false,
            error: None,
        }));
        if pinned {
            self.run_search();
        }
    }

    fn run_search(&mut self) {
        self.next_token += 1;
        let token = self.next_token;
        let Some(Popup::Search(sv)) = &mut self.popup else {
            return;
        };
        let query = sv.input.text().trim().to_owned();
        if query.is_empty() && !sv.listing() {
            return;
        }
        sv.token = token;
        sv.loading = true;
        sv.searched_for = Some(query.clone());
        self.td
            .search(token, sv.chat_id, query, sv.pinned, sv.sender);
    }

    pub fn jump_to_in(&mut self, chat_id: i64, msg_id: i64, topic: Option<i32>) {
        let view = topic.filter(|_| self.is_forum(chat_id));
        if self.open_chat != Some(chat_id) || self.open_topic != view {
            self.open_view(chat_id, view);
            if let Some(t) = view {
                self.expand(chat_id);
                self.select_row(Row::Topic(chat_id, t));
            }
        }
        self.jump_to(chat_id, msg_id);
    }

    pub fn jump_to(&mut self, chat_id: i64, msg_id: i64) {
        if self.open_chat != Some(chat_id) {
            self.open(chat_id);
        }
        self.focus = Pane::Messages;
        self.pending_jump = Some((chat_id, msg_id));
        self.continue_jump();
    }

    fn continue_jump(&mut self) {
        let Some((chat_id, msg_id)) = self.pending_jump else {
            return;
        };
        if self.open_chat != Some(chat_id) {
            self.pending_jump = None;
            return;
        }
        let Some(h) = self.histories.get_mut(&chat_id) else {
            self.pending_jump = None;
            return;
        };
        if let Some(i) = h.index_of(msg_id) {
            self.pending_jump = None;
            self.msg_sel = Some(i);
            self.msg_top = i;
            self.bottom_offset = 0;
            self.mark_read();
            return;
        }
        if self.jump_inflight == Some((chat_id, msg_id)) {
            return;
        }
        let inside = matches!(
            (h.oldest_id(), h.newest_id()),
            (Some(o), Some(n)) if o < msg_id && msg_id < n
        );
        if inside {
            self.pending_jump = None;
            self.set_status("that message was deleted", true);
            return;
        }
        self.jump_inflight = Some((chat_id, msg_id));
        let topic = h.topic;
        self.td.load_around(chat_id, msg_id, topic);
        self.set_status("loading the message...", false);
    }

    fn save_settings(&mut self) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        let settings = self.settings.clone();
        let tx = self.td.sender();
        tokio::task::spawn_blocking(move || {
            let result = crate::config::save_settings(&path, &settings);
            let _ = tx.send(Event::SettingsSaved(result));
        });
    }

    pub fn option_value(&self, index: usize) -> String {
        let check = |on: bool| if on { "[x]" } else { "[ ]" }.to_owned();
        match index {
            0 => check(self.settings.notifications.desktop),
            1 => check(self.settings.privacy.dont_mark_read),
            2 => check(self.settings.privacy.ghost_mode),
            SOUND_OPTION => check(self.settings.notifications.sound),
            VOLUME_OPTION => format!("{}%", self.settings.volume()),
            THEME_OPTION => match self.settings.appearance.theme {
                ThemeChoice::Custom => "custom (theme.toml)".to_owned(),
                other => other.name().to_owned(),
            },
            WRITE_THEME_OPTION => self
                .theme_path
                .as_ref()
                .map_or_else(|| "unavailable".to_owned(), |p| p.display().to_string()),
            OPEN_FOCUS_OPTION => self.settings.navigation.open_chat_focus.name().to_owned(),
            AFTER_SEND_OPTION => check(self.settings.navigation.after_send_next_unread),
            BORDER_OPTION => self.border_style().name().to_owned(),
            LAYOUT_OPTION => self.message_layout().name().to_owned(),
            AVATAR_OPTION => self.avatar_shape().name().to_owned(),
            SHOW_IDS_OPTION => check(self.settings.appearance.show_ids),
            SECONDS_OPTION => check(self.seconds()),
            EXACT_OPTION => check(self.settings.appearance.exact_numbers),
            KEEP_DELETED_OPTION => check(self.settings.privacy.keep_deleted),
            CLEAR_DELETED_OPTION => match self.archive.len() {
                0 => "nothing saved".to_owned(),
                n => format!("{n} saved"),
            },
            HIDE_BLOCKED_OPTION => check(self.settings.privacy.hide_blocked),
            HIDE_STORIES_OPTION => check(self.settings.privacy.hide_stories),
            FOLDER_COUNTER_OPTION => self.settings.appearance.folder_counter.name().to_owned(),
            FOLDER_UNMUTED_OPTION => check(self.settings.appearance.folder_counter_unmuted_only),
            HIDE_ALL_CHATS_OPTION => check(self.settings.appearance.hide_all_chats),
            TRANSLATE_OPTION => self.settings.chats.translate_to.clone(),
            PHOTOS_AS_FILES_OPTION => check(self.settings.chats.photos_as_files),
            SAVE_TARGET_OPTION => self.save_target_name(),
            HIDE_PHONE_OPTION => check(self.settings.privacy.hide_phone),
            _ => String::new(),
        }
    }

    pub fn seconds(&self) -> bool {
        self.settings
            .appearance
            .seconds
            .unwrap_or(self.theme_base.layout.seconds)
    }

    pub fn count(&self, n: i64) -> String {
        format::count(n, self.settings.appearance.exact_numbers)
    }

    pub fn message_layout(&self) -> MessageLayout {
        self.settings
            .appearance
            .layout
            .unwrap_or(self.theme_base.layout.message_layout)
    }

    pub fn avatar_shape(&self) -> crate::theme::AvatarShape {
        self.settings
            .appearance
            .avatar_shape
            .unwrap_or(self.theme_base.layout.avatar_shape)
    }

    pub fn border_style(&self) -> BorderStyle {
        self.settings
            .appearance
            .border_style
            .unwrap_or(self.theme_base.layout.border_style)
    }

    pub fn theme_key(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        (
            self.theme_gen,
            self.settings.appearance.layout,
            self.settings.appearance.border_style,
            self.settings.appearance.avatar_shape,
            self.settings.appearance.seconds,
            self.settings.appearance.exact_numbers,
            self.settings.privacy.hide_blocked,
            self.settings.privacy.keep_deleted,
        )
            .hash(&mut h);
        h.finish()
    }

    pub fn frame_theme(&mut self) -> Rc<Theme> {
        let key = self.theme_key();
        if let Some((k, t)) = &self.frame_theme
            && *k == key
        {
            return t.clone();
        }
        let mut t = (*self.theme_base).clone();
        t.layout.message_layout = self.message_layout();
        t.layout.border_style = self.border_style();
        t.layout.avatar_shape = self.avatar_shape();
        t.layout.seconds = self.seconds();
        let t = Rc::new(t);
        self.frame_theme = Some((key, t.clone()));
        t
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme_base = Rc::new(theme);
        self.theme_gen += 1;
        self.frame_theme = None;
        self.force_clear = true;
    }

    fn select_theme(&mut self, choice: ThemeChoice) {
        self.palette_query = true;
        let mut notes = Vec::new();
        let loaded = match (&self.theme_path, choice) {
            (Some(path), _) => crate::theme::load(choice, path, &mut notes),
            (None, ThemeChoice::Custom) => Ok(Theme::default()),
            (None, other) => Ok(Theme::preset(other)),
        };
        match loaded {
            Ok(theme) => {
                self.settings.appearance.theme = choice;
                self.settings.appearance.layout = None;
                self.settings.appearance.border_style = None;
                self.settings.appearance.avatar_shape = None;
                self.set_theme(theme);
                self.msg_top = self.msg_sel.unwrap_or(0);
                if !notes.is_empty() {
                    self.set_status(notes.join("; "), true);
                }
            }
            Err(e) => self.set_status(format!("theme {} not applied: {e}", choice.name()), true),
        }
    }

    pub fn palette_answered(&mut self, palette: Option<crate::termcolors::TermPalette>) {
        match palette {
            Some(p) => crate::termcolors::set(Some(p)),
            None => {
                self.set_status(
                    "terminal did not report its colours; keeping the previous palette",
                    true,
                );
            }
        }
        let choice = self.settings.appearance.theme;
        let mut notes = Vec::new();
        let loaded = match (&self.theme_path, choice) {
            (Some(path), _) => crate::theme::load(choice, path, &mut notes),
            (None, ThemeChoice::Custom) => Ok(Theme::default()),
            (None, other) => Ok(Theme::preset(other)),
        };
        match loaded {
            Ok(theme) => self.set_theme(theme),
            Err(e) => self.set_status(format!("theme not reloaded: {e}"), true),
        }
        if !notes.is_empty() {
            self.set_status(notes.join("; "), true);
        }
    }

    pub fn reload_theme_file(&mut self) {
        if self.settings.appearance.theme != ThemeChoice::Custom {
            return;
        }
        let Some(path) = self.theme_path.clone() else {
            return;
        };
        let mut notes = Vec::new();
        match crate::theme::load_custom(&path, &mut notes) {
            Ok(theme) => {
                if theme != *self.theme_base {
                    self.set_theme(theme);
                }
                if notes.is_empty() {
                    self.set_status(format!("theme reloaded from {}", path.display()), false);
                } else {
                    self.set_status(notes.join("; "), true);
                }
            }
            Err(e) => self.set_status(
                format!("theme not reloaded, keeping the old one: {e}"),
                true,
            ),
        }
    }

    fn write_theme_file(&mut self) {
        let Some(path) = self.theme_path.clone() else {
            self.set_status("no config directory for theme.toml", true);
            return;
        };
        if path.exists() {
            self.set_status(
                format!("{} already exists; move it away first", path.display()),
                true,
            );
            return;
        }
        let choice = match self.settings.appearance.theme {
            ThemeChoice::Custom => ThemeChoice::Terminal,
            other => other,
        };
        let result = self.frame_theme().to_toml(choice).and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
        });
        match result {
            Ok(()) => self.set_status(
                format!("wrote {}; pick theme \"custom\" to use it", path.display()),
                false,
            ),
            Err(e) => self.set_status(e, true),
        }
    }

    pub fn toggle_option(&mut self, index: usize) {
        self.change_option(index, None);
    }

    pub fn change_option(&mut self, index: usize, dir: Option<bool>) {
        let flip = |v: bool| dir.unwrap_or(!v);
        let fwd = dir.unwrap_or(true);
        match index {
            0 => {
                let n = &mut self.settings.notifications;
                n.desktop = flip(n.desktop);
                self.notify_broken = false;
            }
            1 => {
                let p = &mut self.settings.privacy;
                p.dont_mark_read = flip(p.dont_mark_read);
            }
            2 => {
                let p = &mut self.settings.privacy;
                p.ghost_mode = flip(p.ghost_mode);
            }
            SOUND_OPTION => {
                let n = &mut self.settings.notifications;
                n.sound = flip(n.sound);
                self.sound_broken = false;
            }
            VOLUME_OPTION => {
                let v = self.settings.volume();
                self.settings.notifications.volume = match dir {
                    None if v >= 100 => 0,
                    None | Some(true) => (v + 10).min(100),
                    Some(false) => v.saturating_sub(10),
                };
            }
            THEME_OPTION => {
                let next = self.settings.appearance.theme.step(fwd);
                self.select_theme(next);
            }
            WRITE_THEME_OPTION => {
                if dir.is_none() {
                    self.write_theme_file();
                }
                return;
            }
            OPEN_FOCUS_OPTION => {
                let nav = &mut self.settings.navigation;
                nav.open_chat_focus = nav.open_chat_focus.step(fwd);
            }
            AFTER_SEND_OPTION => {
                let nav = &mut self.settings.navigation;
                nav.after_send_next_unread = flip(nav.after_send_next_unread);
            }
            BORDER_OPTION => {
                self.settings.appearance.border_style = Some(self.border_style().step(fwd));
            }
            LAYOUT_OPTION => {
                self.settings.appearance.layout = Some(self.message_layout().step(fwd));
            }
            AVATAR_OPTION => {
                self.settings.appearance.avatar_shape = Some(self.avatar_shape().step(fwd));
            }
            SHOW_IDS_OPTION => {
                let a = &mut self.settings.appearance;
                a.show_ids = flip(a.show_ids);
            }
            SECONDS_OPTION => {
                self.settings.appearance.seconds = Some(flip(self.seconds()));
            }
            EXACT_OPTION => {
                let a = &mut self.settings.appearance;
                a.exact_numbers = flip(a.exact_numbers);
            }
            KEEP_DELETED_OPTION => {
                let p = &mut self.settings.privacy;
                p.keep_deleted = flip(p.keep_deleted);
                if !p.keep_deleted {
                    self.drop_deleted_from_view();
                }
            }
            CLEAR_DELETED_OPTION => {
                if dir.is_none() {
                    self.popup = Some(Popup::ConfirmClearDeleted);
                }
                return;
            }
            HIDE_BLOCKED_OPTION => {
                let p = &mut self.settings.privacy;
                p.hide_blocked = flip(p.hide_blocked);
                if p.hide_blocked {
                    self.ensure_blocked_loaded();
                }
            }
            HIDE_STORIES_OPTION => {
                let p = &mut self.settings.privacy;
                p.hide_stories = flip(p.hide_stories);
            }
            FOLDER_COUNTER_OPTION => {
                let a = &mut self.settings.appearance;
                a.folder_counter = a.folder_counter.step(fwd);
            }
            FOLDER_UNMUTED_OPTION => {
                let a = &mut self.settings.appearance;
                a.folder_counter_unmuted_only = flip(a.folder_counter_unmuted_only);
            }
            HIDE_ALL_CHATS_OPTION => {
                let current = self.current_list();
                let a = &mut self.settings.appearance;
                a.hide_all_chats = flip(a.hide_all_chats);
                self.reselect_folder(current);
            }
            TRANSLATE_OPTION => self.settings.chats.step_language(fwd),
            HIDE_PHONE_OPTION => {
                let p = &mut self.settings.privacy;
                p.hide_phone = flip(p.hide_phone);
            }
            PHOTOS_AS_FILES_OPTION => {
                let c = &mut self.settings.chats;
                c.photos_as_files = flip(c.photos_as_files);
            }
            SAVE_TARGET_OPTION => {
                if dir.is_some() {
                    return;
                }
                let open = self.open_chat.unwrap_or(0);
                let c = &mut self.settings.chats;
                c.save_to_chat = if open == 0 || c.save_to_chat == open || Some(open) == self.my_id
                {
                    0
                } else {
                    open
                };
            }
            _ => return,
        }
        self.apply_online();
        self.save_settings();
    }

    fn on_stage4_popup_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let down = matches!(key.code, KeyCode::Down) || (ctrl && key.code == KeyCode::Char('n'));
        let up = matches!(key.code, KeyCode::Up) || (ctrl && key.code == KeyCode::Char('p'));
        let close = key.code == KeyCode::Esc;
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        match popup {
            Popup::Profile(p) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => p.scroll = p.scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => p.scroll = p.scroll.saturating_sub(1),
                KeyCode::Char('a') => {
                    let sender = match p.target {
                        ProfileTarget::User(u) => Some(Sender::User(u)),
                        ProfileTarget::Chat(c) => p
                            .data
                            .as_ref()
                            .and_then(|d| d.user_id)
                            .map(Sender::User)
                            .or(Some(Sender::Chat(c))),
                    };
                    match (self.open_chat, sender) {
                        (Some(chat), Some(s)) => self.open_from_sender(chat, s),
                        _ => self.set_status("open a chat first", false),
                    }
                }
                KeyCode::Char('S') => {
                    let user = p.data.as_ref().and_then(|d| d.user_id);
                    match user.filter(|u| self.can_start_secret(*u)) {
                        Some(u) => {
                            self.td.create_secret_chat(u);
                            self.set_status("starting a secret chat...", false);
                        }
                        None => self.set_status("secret chats are only with people", false),
                    }
                }
                _ => {}
            },
            Popup::Reactions(r) => {
                let n = r.emojis.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Char('l' | 'j') | KeyCode::Right | KeyCode::Down if n > 0 => {
                        r.sel = (r.sel + 1) % n
                    }
                    KeyCode::Char('h' | 'k') | KeyCode::Left | KeyCode::Up if n > 0 => {
                        r.sel = (r.sel + n - 1) % n
                    }
                    KeyCode::Enter | KeyCode::Char(' ') if n > 0 => {
                        let (chat_id, msg_id) = (r.chat_id, r.msg_id);
                        let emoji = r.emojis[r.sel.min(n - 1)].clone();
                        let chosen = self
                            .histories
                            .get(&chat_id)
                            .and_then(|h| h.get(msg_id))
                            .is_some_and(|m| {
                                m.reactions
                                    .iter()
                                    .any(|x| x.chosen && x.emoji.as_deref() == Some(&emoji))
                            });
                        self.popup = None;
                        self.td.set_reaction(chat_id, msg_id, emoji, !chosen);
                    }
                    _ => {}
                }
            }
            Popup::ReactionList { items, sel, .. } => {
                let n = items.as_ref().map_or(0, Vec::len);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Char('j') | KeyCode::Down => {
                        *sel = (*sel + 1).min(n.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Enter => {
                        if let Some((sender, _)) = items.as_ref().and_then(|i| i.get(*sel)) {
                            let target = match *sender {
                                Sender::User(id) => ProfileTarget::User(id),
                                Sender::Chat(id) => ProfileTarget::Chat(id),
                            };
                            self.open_profile(target);
                        }
                    }
                    _ => {}
                }
            }
            Popup::Search(sv) => {
                let n = sv.results.len();
                let nav = sv.listing();
                if close || (nav && key.code == KeyCode::Char('q')) {
                    self.popup = None;
                } else if down || (nav && key.code == KeyCode::Char('j')) {
                    sv.sel = (sv.sel + 1).min(n.saturating_sub(1));
                } else if up || (nav && key.code == KeyCode::Char('k')) {
                    sv.sel = sv.sel.saturating_sub(1);
                } else if key.code == KeyCode::Enter {
                    let fresh = nav || sv.searched_for.as_deref() == Some(sv.input.text().trim());
                    if fresh && let Some(m) = sv.results.get(sv.sel) {
                        let (chat, id, topic) = (m.chat_id, m.id, m.topic);
                        self.popup = None;
                        self.jump_to_in(chat, id, topic);
                    } else if !fresh {
                        self.run_search();
                    }
                } else if !nav {
                    sv.input.handle_key(&key, false);
                }
            }
            Popup::Buttons(b) => {
                let n = b.keyboard.buttons().len();
                let pick = match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.popup = None;
                        return;
                    }
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::Right | KeyCode::Char('l') => {
                        b.sel = (b.sel + 1).min(n.saturating_sub(1));
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up | KeyCode::Left => {
                        b.sel = b.sel.saturating_sub(1);
                        None
                    }
                    KeyCode::Enter => Some(b.sel),
                    KeyCode::Char(c) => bots::key_index(c),
                    _ => None,
                };
                if let Some(i) = pick
                    && let Some(button) = b.keyboard.button(i).cloned()
                {
                    let (chat_id, msg_id) = (b.chat_id, b.msg_id);
                    self.press_button(chat_id, msg_id, button);
                }
            }
            Popup::ConfirmUrl(url) => match key.code {
                KeyCode::Char('y') => {
                    let url = format::normalize_url(url);
                    self.popup = None;
                    if let Err(e) = player::xdg_open(&url) {
                        self.set_status(e, true);
                    }
                }
                KeyCode::Char('c') => {
                    let url = url.clone();
                    self.popup = None;
                    self.copy_text(url);
                }
                KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                    self.popup = None
                }
                _ => {}
            },
            Popup::Options { sel } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Char('j') | KeyCode::Down => *sel = shift_option(*sel, 1),
                KeyCode::Char('k') | KeyCode::Up => *sel = shift_option(*sel, -1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let i = *sel;
                    self.toggle_option(i);
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    let i = *sel;
                    self.change_option(i, Some(false));
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    let i = *sel;
                    self.change_option(i, Some(true));
                }
                KeyCode::Char(c) => {
                    if let Some(i) = option_for_key(c) {
                        *sel = i;
                        self.toggle_option(i);
                    }
                }
                _ => {}
            },
            Popup::Inbox { sel } => {
                let n = self.inbox.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Char('j') | KeyCode::Down => {
                        *sel = (*sel + 1).min(n.saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Char('c') => {
                        self.inbox.clear();
                        self.popup = None;
                    }
                    KeyCode::Enter => {
                        if let Some(item) = self.inbox.get(*sel) {
                            let (chat, id, topic) = (item.chat_id, item.msg_id, item.topic);
                            self.popup = None;
                            self.jump_to_in(chat, id, topic);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

pub fn attachment_name(att: &Attachment, msg_id: i64) -> String {
    if !att.name.is_empty() && att.kind != MediaKind::Sticker && att.kind != MediaKind::Photo {
        return att.name.clone();
    }
    let ext = match att.kind {
        MediaKind::Photo => "jpg",
        MediaKind::Video | MediaKind::Animation | MediaKind::VideoNote => "mp4",
        MediaKind::Voice => "ogg",
        MediaKind::Audio => "mp3",
        MediaKind::Sticker => match att.full.as_ref().map(|v| v.decode) {
            Some(Decode::Tgs) => "tgs",
            Some(Decode::Video) => "webm",
            _ => "webp",
        },
        MediaKind::Document => "bin",
    };
    let stem = match att.kind {
        MediaKind::Photo => "photo",
        MediaKind::Video => "video",
        MediaKind::Animation => "animation",
        MediaKind::VideoNote => "video_message",
        MediaKind::Voice => "voice",
        MediaKind::Audio => "audio",
        MediaKind::Sticker => "sticker",
        MediaKind::Document => "file",
    };
    format!("{stem}_{}.{ext}", msg_id >> 20)
}

fn next_matching<T: Copy + PartialEq>(
    order: &[T],
    current: Option<T>,
    include_current: bool,
    pred: impl Fn(T) -> bool,
) -> Option<T> {
    let n = order.len();
    let (first, count) = match current.and_then(|c| order.iter().position(|id| *id == c)) {
        Some(s) if include_current => (s, n),
        Some(s) => (s + 1, n - 1),
        None => (0, n),
    };
    (0..count)
        .map(|k| order[(first + k) % n])
        .find(|id| pred(*id))
}

const VIEWER_PAGE: usize = 10;

fn page_delta(code: KeyCode, page: usize) -> Option<isize> {
    let page = page.max(1) as isize;
    match code {
        KeyCode::PageDown => Some(page),
        KeyCode::PageUp => Some(-page),
        KeyCode::Home => Some(isize::MIN / 2),
        KeyCode::End => Some(isize::MAX / 2),
        _ => None,
    }
}

impl App {
    pub fn hidden_blocked(&self, m: &Msg) -> bool {
        self.settings.privacy.hide_blocked
            && !m.outgoing
            && self.blocked.contains(&m.sender)
            && !self.revealed.contains(&(m.chat_id, m.id))
            && self
                .chats
                .get(&m.chat_id)
                .is_some_and(|c| matches!(c.kind, ChatKind::Group | ChatKind::Supergroup))
    }

    pub fn storable(&self, m: &Msg) -> bool {
        m.meta.can_be_saved
            && m.ttl == 0
            && !m.deleted
            && m.state == model::SendState::Sent
            && self
                .chats
                .get(&m.chat_id)
                .is_some_and(|c| !c.protected && !matches!(c.kind, ChatKind::Secret(_)))
    }

    fn cached_msg(&self, chat_id: i64, id: i64) -> Option<Msg> {
        self.histories
            .get(&chat_id)
            .and_then(|h| h.get(id))
            .cloned()
            .or_else(|| {
                self.chats
                    .get(&chat_id)
                    .and_then(|c| c.last_message.clone())
                    .filter(|m| m.id == id)
            })
    }

    pub fn open_archive(&mut self, path: PathBuf) {
        let limit = self.settings.privacy.keep_deleted_limit;
        match crate::archive::Archive::open(path, limit) {
            Ok((archive, corrupt)) => {
                self.archive = archive;
                if corrupt > 0 {
                    let where_ = self
                        .archive
                        .path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    self.set_status(
                        format!("{corrupt} unreadable lines dropped from {where_}"),
                        true,
                    );
                }
            }
            Err(e) => {
                self.archive = crate::archive::Archive::memory(limit);
                self.set_status(format!("saved deleted messages unavailable: {e}"), true);
            }
        }
    }

    fn account_archive(&mut self, my_id: i64) {
        let path = self.data_dir.join("archive").join(format!("{my_id}.jsonl"));
        if self.archive.path() != Some(path.as_path()) {
            self.open_archive(path);
        }
    }

    fn ensure_blocked_loaded(&mut self) {
        if !self.blocked_loaded && self.auth == Auth::Ready {
            self.blocked_loaded = true;
            self.td.load_blocked();
        }
    }

    fn drop_deleted_from_view(&mut self) {
        let Some(chat_id) = self.open_chat else {
            return;
        };
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        let ids: Vec<i64> = h
            .msgs()
            .iter()
            .filter(|m| m.deleted)
            .map(|m| m.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        h.remove(&ids);
        let len = h.len();
        self.msg_sel = self.msg_sel.filter(|_| len > 0).map(|s| s.min(len - 1));
        self.msg_top = self.msg_top.min(len.saturating_sub(1));
    }

    fn clear_deleted(&mut self) {
        match self.archive.clear() {
            Ok(n) => {
                self.drop_deleted_from_view();
                self.set_status(format!("cleared {n} saved items"), false);
            }
            Err(e) => self.set_status(format!("not cleared: {e}"), true),
        }
    }

    fn enrich(&self, chat_id: i64, mut msgs: Vec<Msg>, lo: i64, hi: i64) -> Vec<Msg> {
        if self.settings.privacy.hide_stories {
            msgs.retain(|m| !m.meta.story);
        }
        if !self.settings.privacy.keep_deleted || lo > hi {
            return msgs;
        }
        let Some(h) = self.histories.get(&chat_id) else {
            return msgs;
        };
        let have: std::collections::HashSet<i64> = msgs.iter().map(|m| m.id).collect();
        msgs.extend(
            self.archive
                .deleted_between(chat_id, lo, hi)
                .into_iter()
                .filter(|m| !have.contains(&m.id) && h.index_of(m.id).is_none() && h.accepts(m)),
        );
        msgs.sort_by_key(|m| std::cmp::Reverse(m.id));
        msgs
    }

    fn on_deleted(&mut self, chat_id: i64, ids: &[i64]) -> Vec<i64> {
        if !self.settings.privacy.keep_deleted {
            return Vec::new();
        }
        let now = unix_now();
        let mut kept = Vec::new();
        for id in ids {
            let Some(m) = self.cached_msg(chat_id, *id).filter(|m| self.storable(m)) else {
                continue;
            };
            match self.archive.record_deleted(&m, now) {
                Ok(_) => kept.push(*id),
                Err(e) => {
                    self.set_status(format!("deleted message not saved: {e}"), true);
                    break;
                }
            }
        }
        kept
    }

    fn on_content_change(&mut self, chat_id: i64, id: i64, new: &td::Content) {
        if !self.settings.privacy.keep_deleted {
            return;
        }
        let Some(old) = self.cached_msg(chat_id, id).filter(|m| self.storable(m)) else {
            return;
        };
        let file = |a: &[Attachment]| a.first().map(|a| a.file.id);
        if old.body.text == new.body.text && file(&old.attachments) == file(&new.attachments) {
            return;
        }
        if let Err(e) = self.archive.record_edit(&old, unix_now()) {
            self.set_status(format!("edit history not saved: {e}"), true);
        }
    }

    fn sender_of_chat(&self, chat_id: i64) -> Sender {
        match self.chats.get(&chat_id).map(|c| &c.kind) {
            Some(ChatKind::Private(u)) => Sender::User(*u),
            _ => Sender::Chat(chat_id),
        }
    }

    fn open_from_sender(&mut self, chat_id: i64, sender: Sender) {
        self.popup = Some(Popup::Search(SearchView {
            chat_id: Some(chat_id),
            pinned: false,
            sender: Some(sender),
            input: TextInput::default(),
            searched_for: None,
            results: Vec::new(),
            sel: 0,
            token: 0,
            loading: false,
            error: None,
        }));
        self.run_search();
    }

    fn open_details(&mut self, chat_id: i64, msg_id: i64) {
        let deleted = self.cached_msg(chat_id, msg_id).is_some_and(|m| m.deleted);
        self.popup = Some(Popup::Details {
            chat_id,
            msg_id,
            flags: None,
            scroll: 0,
        });
        if !deleted {
            self.td.load_properties(chat_id, msg_id);
        }
    }

    pub fn details_rows(&self, chat_id: i64, msg_id: i64) -> Option<Vec<(String, String)>> {
        let m = self.cached_msg(chat_id, msg_id)?;
        let flags = match &self.popup {
            Some(Popup::Details { flags, .. }) => flags.clone(),
            _ => None,
        };
        let reply = m.reply_to.and_then(|r| {
            let h = self.histories.get(&chat_id)?;
            let r = h.get(r)?;
            Some(format!(
                "{}: {}",
                self.sender_name(r.sender),
                format::truncate(&model::preview(r), 40)
            ))
        });
        let ctx = crate::details::Ctx {
            sender: self.sender_name(m.sender),
            chat: self.sender_name(Sender::Chat(chat_id)),
            origin: m.forward.as_ref().map(|o| match o {
                model::Origin::Sender(s) => self.sender_name(*s),
                model::Origin::Name(n) => n.clone(),
            }),
            reply,
            flags,
            protected: self.chats.get(&chat_id).is_some_and(|c| c.protected),
            exact: self.settings.appearance.exact_numbers,
            edits: self.archive.edits(chat_id, msg_id).len(),
        };
        Some(crate::details::lines(&m, &ctx))
    }

    pub fn edit_versions(&self, chat_id: i64, msg_id: i64) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .archive
            .edits(chat_id, msg_id)
            .into_iter()
            .map(|(_, s)| {
                let when = if s.edit_date > 0 {
                    format!("edited {}", format::stamp(s.edit_date))
                } else {
                    format!("sent {}", format::stamp(s.date))
                };
                let text = match (&s.media, s.text.is_empty()) {
                    (Some(m), true) => m.clone(),
                    (Some(m), false) => format!("{m} {}", s.text),
                    (None, _) => s.text.clone(),
                };
                (when, text)
            })
            .collect();
        if let Some(m) = self.cached_msg(chat_id, msg_id) {
            let when = if m.meta.edit_date > 0 {
                format!("now · edited {}", format::stamp(m.meta.edit_date))
            } else {
                "now".to_owned()
            };
            out.push((when, model::preview(&m)));
        }
        out
    }
}

fn shift_option(cur: usize, delta: isize) -> usize {
    let order: Vec<usize> = option_order().collect();
    let pos = order.iter().position(|i| *i == cur).unwrap_or(0);
    order[step(pos, delta, order.len())]
}

fn step(cur: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as isize)
        .saturating_add(delta)
        .clamp(0, len as isize - 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatInfo, Position, RichText, SendState};
    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    fn msg(chat_id: i64, id: i64, sender: i64) -> Msg {
        Msg {
            id,
            chat_id,
            sender: Sender::User(sender),
            date: unix_now() - 5,
            edited: false,
            outgoing: sender == 1,
            state: SendState::Sent,
            reply_to: None,
            album: 0,
            media: None,
            attachments: Vec::new(),
            body: RichText::plain(format!("text {id}")),
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

    fn chat(id: i64, kind: ChatKind, muted: bool) -> ChatInfo {
        ChatInfo {
            id,
            title: format!("chat {id}"),
            kind,
            unread: 2,
            mentions: 1,
            reactions: 0,
            last_read_inbox: 0,
            last_read_outbox: 0,
            photo: None,
            muted,
            use_default_mute: false,
            positions: vec![Position {
                list: ListId::Main,
                order: id,
                pinned: false,
            }],
            last_message: Some(msg(id, 7, 2)),
            group_id: 0,
            secret_id: 0,
            forum: false,
            auto_delete: 0,
            perms: crate::model::Rights::default(),
            protected: false,
        }
    }

    fn fake_app(settings: Settings) -> (App, td::RequestLog) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (td, log) = Td::recording(tx);
        let creds = ApiCredentials {
            api_id: 1,
            api_hash: "x".to_owned(),
        };
        let mut app = App::new(
            td,
            creds,
            std::env::temp_dir(),
            Keymap::default(),
            ratatui_image::picker::Picker::halfblocks(),
        );
        app.auth = Auth::Ready;
        app.settings = settings;
        app.my_id = Some(1);
        app.users.insert(1, "Me".to_owned());
        app.users.insert(2, "Alice Smith".to_owned());
        app.chats.insert(10, chat(10, ChatKind::Supergroup, false));
        app.chats.insert(20, chat(20, ChatKind::Private(2), false));
        app.chats.insert(30, chat(30, ChatKind::Group, true));
        app.sound_env = Some(notify::SoundEnv {
            canberra: true,
            pw_play: false,
        });
        (app, log)
    }

    fn press(app: &mut App, code: K, mods: M) {
        app.handle(Event::Input(TermEvent::Key(KeyEvent::new(code, mods))));
    }

    fn leader(app: &mut App, c: char) {
        press(app, K::Char(' '), M::NONE);
        press(app, K::Char(c), M::NONE);
    }

    fn open_with_history(app: &mut App, chat: i64) {
        app.open(chat);
        app.handle(Event::History {
            chat_id: chat,
            msgs: (1..=40).rev().map(|i| msg(chat, i * 10, 2)).collect(),
            initial: true,
            epoch: 0,
        });
    }

    fn history_ids(app: &App, chat: i64) -> Vec<i64> {
        app.histories[&chat].msgs().iter().map(|m| m.id).collect()
    }

    #[test]
    fn jumping_far_back_loads_around_the_target_and_lazy_loads_back_to_latest() {
        let (mut app, log) = fake_app(no_read());
        app.chats.get_mut(&10).unwrap().last_message = Some(msg(10, 400, 2));
        app.open(10);
        app.handle(Event::History {
            chat_id: 10,
            msgs: (351..=400).rev().map(|i| msg(10, i, 2)).collect(),
            initial: true,
            epoch: 0,
        });
        app.msg_view_height = 30;
        app.jump_to(10, 5);
        assert_eq!(count(&log, "load around"), 1);
        assert_eq!(count(&log, "load history"), 1, "no page-by-page walk back");
        app.continue_jump();
        assert_eq!(count(&log, "load around"), 1, "one request per jump");
        app.handle(Event::HistoryAround {
            chat_id: 10,
            target: 5,
            result: Ok((1..=25).rev().map(|i| msg(10, i, 2)).collect()),
        });
        assert_eq!(history_ids(&app, 10), (1..=25).collect::<Vec<_>>());
        let h = &app.histories[&10];
        assert!(h.detached);
        assert_eq!(app.msg_sel.map(|i| h.msgs()[i].id), Some(5));
        assert!(app.status.as_ref().is_none_or(|(_, err)| !err));
        app.handle(Event::History {
            chat_id: 10,
            msgs: (401..=450).map(|i| msg(10, i, 2)).collect(),
            initial: true,
            epoch: 0,
        });
        assert_eq!(
            history_ids(&app, 10).last(),
            Some(&25),
            "stale page ignored"
        );
        let n = history_ids(&app, 10).len();
        let groups = model::groups(app.histories[&10].msgs());
        press(&mut app, K::PageDown, M::NONE);
        press(&mut app, K::PageDown, M::NONE);
        press(&mut app, K::PageDown, M::NONE);
        assert_eq!(app.msg_sel, groups.last().map(|g| g.start));
        assert!(
            app.msg_sel.is_some_and(|i| i < n),
            "stays on the window bottom"
        );
        assert_eq!(count(&log, "load newer"), 1);
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(count(&log, "load newer"), 1, "one newer page at a time");
        let epoch = app.histories[&10].epoch;
        app.handle(Event::NewerHistory {
            chat_id: 10,
            msgs: (25..=74).rev().map(|i| msg(10, i, 2)).collect(),
            epoch,
        });
        assert_eq!(history_ids(&app, 10).last(), Some(&74));
        assert!(app.histories[&10].detached);
        press(&mut app, K::Char('j'), M::NONE);
        let id = app.msg_sel.map(|i| app.histories[&10].msgs()[i].id);
        assert_eq!(id, Some(26));
        let before = count(&log, "load history");
        press(&mut app, K::Char('G'), M::NONE);
        assert_eq!(
            count(&log, "load history"),
            before + 1,
            "G reloads the latest page"
        );
        let h = &app.histories[&10];
        assert!(h.is_empty() && !h.detached && h.loading_older);
        assert_eq!(app.msg_sel, None);
        let epoch = h.epoch;
        app.handle(Event::History {
            chat_id: 10,
            msgs: (351..=400).rev().map(|i| msg(10, i, 2)).collect(),
            initial: true,
            epoch,
        });
        assert_eq!(history_ids(&app, 10).last(), Some(&400));
    }

    #[test]
    fn older_history_is_requested_once_per_page_and_stops_at_the_beginning() {
        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        let initial = count(&log, "load history");
        let older = |_: &App| count(&log, "load history") - initial;
        for _ in 0..20 {
            app.want_older = true;
            app.after_draw();
        }
        assert_eq!(
            older(&app),
            1,
            "one request in flight, frames and scroll ticks do not repeat it"
        );
        assert!(app.histories[&10].loading_older);
        app.handle(Event::History {
            chat_id: 10,
            msgs: (1..=9).rev().map(|i| msg(10, i, 2)).collect(),
            initial: false,
            epoch: 0,
        });
        assert_eq!(history_ids(&app, 10).first(), Some(&1));
        for _ in 0..5 {
            app.want_older = true;
            app.after_draw();
        }
        assert_eq!(older(&app), 2);
        app.handle(Event::History {
            chat_id: 10,
            msgs: (1..=3).rev().map(|i| msg(10, i, 2)).collect(),
            initial: false,
            epoch: 0,
        });
        assert_eq!(older(&app), 2, "a page with nothing older does not loop");
        assert!(app.histories[&10].reached_start);
        for _ in 0..5 {
            app.want_older = true;
            app.after_draw();
            app.maybe_load_older();
        }
        assert_eq!(older(&app), 2, "beginning reached: no more requests");
        assert!(!app.histories[&10].loading_older);

        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        let initial = count(&log, "load history");
        app.want_older = true;
        app.after_draw();
        app.handle(Event::History {
            chat_id: 10,
            msgs: Vec::new(),
            initial: false,
            epoch: 0,
        });
        assert!(app.histories[&10].reached_start);
        app.want_older = true;
        app.after_draw();
        assert_eq!(
            count(&log, "load history") - initial,
            1,
            "empty page = beginning"
        );

        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        let initial = count(&log, "load history");
        app.want_older = true;
        app.after_draw();
        app.handle(Event::HistoryFailed {
            chat_id: 10,
            epoch: 0,
            error: "FLOOD_WAIT (420)".to_owned(),
        });
        let h = &app.histories[&10];
        assert!(!h.loading_older && !h.reached_start);
        assert!(!h.older_indicator(std::time::Instant::now() + std::time::Duration::from_secs(9)));
        assert!(matches!(&app.status, Some((s, true)) if s.contains("FLOOD_WAIT")));
        for _ in 0..5 {
            app.want_older = true;
            app.after_draw();
        }
        assert_eq!(
            count(&log, "load history") - initial,
            1,
            "no retry storm after a failure"
        );
        app.histories.get_mut(&10).unwrap().older_retry_at = Some(std::time::Instant::now());
        app.want_older = true;
        app.after_draw();
        assert_eq!(
            count(&log, "load history") - initial,
            2,
            "retries after the pause"
        );
    }

    #[test]
    fn newer_pages_that_reach_the_latest_message_close_the_gap() {
        let (mut app, _) = fake_app(no_read());
        app.chats.get_mut(&10).unwrap().last_message = Some(msg(10, 400, 2));
        open_with_history(&mut app, 10);
        app.histories.get_mut(&10).unwrap().trim_oldest(5);
        app.jump_to(10, 5);
        app.handle(Event::HistoryAround {
            chat_id: 10,
            target: 5,
            result: Ok((1..=30).map(|i| msg(10, i, 2)).collect()),
        });
        let epoch = app.histories[&10].epoch;
        app.handle(Event::NewerHistory {
            chat_id: 10,
            msgs: vec![msg(10, 30, 2), msg(10, 390, 2), msg(10, 400, 2)],
            epoch,
        });
        assert!(!app.histories[&10].detached);
        app.handle(Event::NewerHistory {
            chat_id: 10,
            msgs: vec![msg(10, 999, 2)],
            epoch: epoch + 5,
        });
        assert_eq!(history_ids(&app, 10).last(), Some(&400));
    }

    #[test]
    fn deleted_jump_target_reports_instead_of_loading_forever() {
        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        app.jump_to(10, 3);
        app.handle(Event::HistoryAround {
            chat_id: 10,
            target: 3,
            result: Err("that message was deleted".to_owned()),
        });
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("deleted"))
        );
        assert_eq!(app.pending_jump, None);
        assert_eq!(history_ids(&app, 10).len(), 40, "window untouched");
        app.jump_to(10, 15);
        assert_eq!(
            count(&log, "load around"),
            1,
            "a gap inside the window is a deletion"
        );
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("deleted"))
        );
    }

    #[test]
    fn page_keys_move_the_chat_and_folder_selection_by_a_visible_page() {
        let (mut app, _) = fake_app(Settings::default());
        for id in 100..130 {
            app.chats.insert(id, chat(id, ChatKind::Group, false));
        }
        app.list_cache.clear();
        app.focus = Pane::Chats;
        app.chat_rows.set(7);
        press(&mut app, K::PageDown, M::NONE);
        assert_eq!(app.chat_sel, 7);
        press(&mut app, K::PageDown, M::NONE);
        assert_eq!(app.chat_sel, 14);
        press(&mut app, K::PageUp, M::NONE);
        assert_eq!(app.chat_sel, 7);
        press(&mut app, K::End, M::NONE);
        assert_eq!(app.chat_sel, app.visible_chats().len() - 1);
        press(&mut app, K::PageDown, M::NONE);
        assert_eq!(app.chat_sel, app.visible_chats().len() - 1);
        press(&mut app, K::Home, M::NONE);
        assert_eq!(app.chat_sel, 0);
        press(&mut app, K::PageUp, M::NONE);
        assert_eq!(app.chat_sel, 0);
        app.folders = (1..=6).map(|i| (i, format!("f{i}"))).collect();
        app.focus = Pane::Folders;
        app.folder_rows.set(3);
        press(&mut app, K::PageDown, M::NONE);
        assert_eq!(app.folder_sel, 3);
        press(&mut app, K::End, M::NONE);
        assert_eq!(app.folder_sel, 7);
        press(&mut app, K::PageUp, M::NONE);
        assert_eq!(app.folder_sel, 4);
    }

    #[test]
    fn page_keys_in_messages_are_two_half_pages_and_home_end_jump() {
        let (mut app, _) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.focus = Pane::Messages;
        app.msg_view_height = 30;
        press(&mut app, K::Char('u'), M::CONTROL);
        press(&mut app, K::Char('u'), M::CONTROL);
        let two_halves = app.msg_sel;
        press(&mut app, K::End, M::NONE);
        assert_eq!(app.msg_sel, None, "End returns to the latest message");
        press(&mut app, K::PageUp, M::NONE);
        assert!(two_halves.is_some());
        assert_eq!(app.msg_sel, two_halves);
        press(&mut app, K::Home, M::NONE);
        assert_eq!(app.msg_sel, Some(0));
        press(&mut app, K::PageDown, M::NONE);
        let groups = model::groups(app.histories[&10].msgs());
        assert_eq!(app.msg_sel, Some(groups[10].start));
    }

    #[test]
    fn page_keys_work_in_popups_and_leave_home_end_to_text_inputs() {
        let (mut app, _) = fake_app(Settings::default());
        for id in 100..130 {
            app.chats.insert(id, chat(id, ChatKind::Group, false));
        }
        app.popup_rows.set(4);
        app.popup = Some(Popup::ReactionList {
            chat_id: 10,
            msg_id: 1,
            items: Some(
                (0..20)
                    .map(|i| (Sender::User(i), "👍".to_owned()))
                    .collect(),
            ),
            sel: 0,
        });
        press(&mut app, K::PageDown, M::NONE);
        press(&mut app, K::PageDown, M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::ReactionList { sel: 8, .. })
        ));
        press(&mut app, K::End, M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::ReactionList { sel: 19, .. })
        ));
        press(&mut app, K::PageUp, M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::ReactionList { sel: 15, .. })
        ));
        press(&mut app, K::Home, M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::ReactionList { sel: 0, .. })
        ));
        app.popup = Some(Popup::Switcher {
            input: TextInput::with_text("chat"),
            sel: 0,
            forward: None,
        });
        press(&mut app, K::PageDown, M::NONE);
        assert!(matches!(app.popup, Some(Popup::Switcher { sel: 4, .. })));
        press(&mut app, K::End, M::NONE);
        assert!(
            matches!(app.popup, Some(Popup::Switcher { sel: 4, .. })),
            "End belongs to the query input"
        );
        for _ in 0..20 {
            press(&mut app, K::PageDown, M::NONE);
        }
        let n = app.switcher_results("chat").len();
        assert!(matches!(app.popup, Some(Popup::Switcher { sel, .. }) if sel == n - 1));
        app.popup = Some(Popup::Inbox { sel: 0 });
        press(&mut app, K::PageDown, M::NONE);
        assert!(matches!(app.popup, Some(Popup::Inbox { sel: 0 })));
    }

    fn count(log: &td::RequestLog, what: &str) -> usize {
        log.lock().unwrap().iter().filter(|e| *e == what).count()
    }

    fn ghost() -> Settings {
        let mut s = Settings::default();
        s.privacy.ghost_mode = true;
        s
    }

    fn no_read() -> Settings {
        let mut s = Settings::default();
        s.privacy.dont_mark_read = true;
        s
    }

    fn browse(app: &mut App) {
        press(app, K::Char('k'), M::NONE);
        press(app, K::Char('k'), M::NONE);
        press(app, K::Char('j'), M::NONE);
        press(app, K::Esc, M::NONE);
        press(app, K::Char('G'), M::NONE);
    }

    fn type_text(app: &mut App) {
        press(app, K::Char('i'), M::NONE);
        for c in "hello".chars() {
            press(app, K::Char(c), M::NONE);
        }
        press(app, K::Esc, M::NONE);
    }

    #[test]
    fn default_mode_reads_types_and_goes_online() {
        let (mut app, log) = fake_app(Settings::default());
        app.apply_online();
        assert_eq!(count(&log, "go online"), 1);
        open_with_history(&mut app, 10);
        browse(&mut app);
        assert!(count(&log, "mark read") >= 1);
        type_text(&mut app);
        assert_eq!(count(&log, "chat action"), 1);
        app.handle(Event::Input(TermEvent::FocusLost));
        assert_eq!(count(&log, "go offline"), 1);
        app.handle(Event::Input(TermEvent::FocusGained));
        assert_eq!(count(&log, "go online"), 2);
    }

    #[test]
    fn dont_mark_read_never_reads_until_asked() {
        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        browse(&mut app);
        app.on_update(Update::ChatReadInbox(
            tdlib_rs::types::UpdateChatReadInbox {
                chat_id: 10,
                last_read_inbox_message_id: 0,
                unread_count: 5,
            },
        ));
        assert_eq!(count(&log, "mark read"), 0);
        assert_eq!(count(&log, "read mentions"), 0);
        type_text(&mut app);
        assert_eq!(count(&log, "chat action"), 1);
        app.apply_online();
        assert_eq!(count(&log, "go online"), 1);
        leader(&mut app, 'm');
        assert_eq!(count(&log, "mark read"), 1);
        assert_eq!(count(&log, "read mentions"), 1);
        app.focus = Pane::Chats;
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "mark read"), 1);
        app.focus = Pane::Chats;
        leader(&mut app, 'a');
        press(&mut app, K::Char('m'), M::NONE);
        assert_eq!(count(&log, "mark read"), 2);
    }

    #[test]
    fn ghost_mode_blocks_reads_typing_and_online() {
        let (mut app, log) = fake_app(ghost());
        app.apply_online();
        app.handle(Event::Input(TermEvent::FocusGained));
        assert_eq!(count(&log, "go online"), 0);
        assert_eq!(count(&log, "go offline"), 1);
        open_with_history(&mut app, 10);
        browse(&mut app);
        type_text(&mut app);
        assert_eq!(count(&log, "mark read"), 0);
        assert_eq!(count(&log, "chat action"), 0);
        leader(&mut app, 'm');
        assert_eq!(count(&log, "mark read"), 1);
    }

    #[test]
    fn options_switch_presets_live_write_and_hot_reload_theme_file() {
        assert_eq!(Settings::default().appearance.theme, ThemeChoice::Terminal);
        let mut custom = Settings::default();
        custom.appearance.theme = ThemeChoice::Custom;
        let (mut app, _log) = fake_app(custom);
        let dir = std::env::temp_dir().join(format!("gramtty-app-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("theme.toml");
        app.theme_path = Some(path.clone());
        leader(&mut app, 'o');
        assert_eq!(app.option_value(THEME_OPTION), "custom (theme.toml)");
        assert_eq!(*app.frame_theme(), Theme::default());
        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Terminal);
        assert!(
            std::mem::take(&mut app.palette_query),
            "re-selecting asks the terminal again"
        );
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Custom);
        press(&mut app, K::Char('l'), M::NONE);
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Default);
        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.option_value(THEME_OPTION), "high-contrast");
        assert_eq!(*app.frame_theme(), Theme::preset(ThemeChoice::HighContrast));
        assert_eq!(app.border_style(), BorderStyle::Thick);
        assert!(
            app.settings
                .to_toml()
                .unwrap()
                .contains("theme = \"high-contrast\"")
        );
        press(&mut app, K::Char('6'), M::NONE);
        assert_eq!(
            app.settings.appearance.border_style,
            Some(BorderStyle::Double)
        );
        assert_eq!(app.frame_theme().layout.border_style, BorderStyle::Double);
        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Light);
        assert_eq!(app.settings.appearance.border_style, None);
        assert_eq!(app.message_layout(), MessageLayout::Bubbles);
        assert_eq!(*app.frame_theme(), Theme::preset(ThemeChoice::Light));

        press(&mut app, K::Char('8'), M::NONE);
        assert!(matches!(&app.status, Some((s, false)) if s.contains("wrote")));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("base = \"light\""));
        assert_eq!(
            Theme::parse(&written, &mut Vec::new()).unwrap(),
            Theme::preset(ThemeChoice::Light)
        );
        press(&mut app, K::Char('8'), M::NONE);
        assert!(matches!(&app.status, Some((s, true)) if s.contains("already exists")));

        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Mono);
        std::fs::write(&path, "[layout]\nmessage_gutter = 99\n").unwrap();
        app.handle(Event::ThemeFile(Ok(())));
        assert_eq!(*app.frame_theme(), Theme::preset(ThemeChoice::Mono));
        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Mono);
        assert!(matches!(&app.status, Some((s, true)) if s.contains("message_gutter")));
        std::fs::write(&path, &written).unwrap();
        press(&mut app, K::Char('7'), M::NONE);
        assert_eq!(app.settings.appearance.theme, ThemeChoice::Custom);
        assert_eq!(*app.frame_theme(), Theme::preset(ThemeChoice::Light));

        std::fs::write(&path, "[colors]\nlink = \"bluish\"\n").unwrap();
        app.handle(Event::ThemeFile(Ok(())));
        assert!(
            matches!(&app.status, Some((s, true)) if s.contains("keeping the old one") && s.contains("colors.link"))
        );
        assert_eq!(*app.frame_theme(), Theme::preset(ThemeChoice::Light));
        std::fs::write(&path, "base = \"mono\"\n[glyphs]\ntick_read = \"RR\"\n").unwrap();
        app.handle(Event::ThemeFile(Ok(())));
        assert!(matches!(&app.status, Some((s, false)) if s.contains("reloaded")));
        assert_eq!(app.frame_theme().glyphs.tick_read, "RR");
        assert_eq!(app.frame_theme().layout.border_style, BorderStyle::Plain);
        std::fs::remove_file(&path).unwrap();
        app.handle(Event::ThemeFile(Ok(())));
        assert_eq!(*app.frame_theme(), Theme::default());
        app.handle(Event::ThemeFile(Err("watch died".to_owned())));
        assert!(matches!(&app.status, Some((s, true)) if s == "watch died"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn options_popup_toggles_ghost_and_applies_online() {
        let (mut app, log) = fake_app(Settings::default());
        app.apply_online();
        leader(&mut app, 'o');
        assert!(matches!(app.popup, Some(Popup::Options { .. })));
        press(&mut app, K::Char('3'), M::NONE);
        assert!(app.settings.privacy.ghost_mode);
        assert!(!app.settings.auto_read());
        assert_eq!(count(&log, "go offline"), 1);
        press(&mut app, K::Char('j'), M::NONE);
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(app.settings.privacy.dont_mark_read);
        press(&mut app, K::Char('3'), M::NONE);
        assert!(!app.settings.privacy.ghost_mode);
        assert_eq!(count(&log, "go online"), 2);
        assert!(!app.settings.auto_read());
        assert_eq!(app.message_layout(), MessageLayout::Bubbles);
        assert_eq!(app.option_value(LAYOUT_OPTION), "bubbles");
        for name in [
            "classic", "cards", "compact", "minimal", "split", "bubbles", "classic",
        ] {
            press(&mut app, K::Char('m'), M::NONE);
            assert_eq!(app.option_value(LAYOUT_OPTION), name);
            assert_eq!(app.frame_theme().layout.message_layout.name(), name);
        }
        press(&mut app, K::Char('m'), M::NONE);
        assert_eq!(app.settings.appearance.layout, Some(MessageLayout::Cards));
        assert!(
            app.settings
                .to_toml()
                .unwrap()
                .contains("layout = \"cards\"")
        );
        press(&mut app, K::Char('j'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::Options { sel }) if sel == AVATAR_OPTION));
        assert_eq!(app.option_value(AVATAR_OPTION), "square");
        assert_eq!(
            app.frame_theme().layout.avatar_shape,
            crate::theme::AvatarShape::Square
        );
        for (key, name) in [
            (K::Char('l'), "rounded"),
            (K::Right, "circle"),
            (K::Char('l'), "square"),
            (K::Char('h'), "circle"),
            (K::Left, "rounded"),
            (K::Enter, "circle"),
        ] {
            press(&mut app, key, M::NONE);
            assert_eq!(app.option_value(AVATAR_OPTION), name);
            assert_eq!(app.frame_theme().layout.avatar_shape.name(), name);
        }
        press(&mut app, K::Char('a'), M::NONE);
        assert_eq!(app.option_value(AVATAR_OPTION), "square");
        assert!(matches!(app.popup, Some(Popup::Options { sel }) if sel == AVATAR_OPTION));
        press(&mut app, K::Char('l'), M::NONE);
        assert!(
            app.settings
                .to_toml()
                .unwrap()
                .contains("avatar_shape = \"rounded\"")
        );
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.option_value(LAYOUT_OPTION), "classic");
        press(&mut app, K::Char('l'), M::NONE);
        assert_eq!(app.option_value(LAYOUT_OPTION), "cards");
        press(&mut app, K::Char('j'), M::NONE);
        press(&mut app, K::Char('j'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::Options { sel }) if sel == WRITE_THEME_OPTION));
        press(&mut app, K::Char('l'), M::NONE);
        assert!(
            app.status
                .as_ref()
                .is_none_or(|(s, _)| !s.contains("wrote"))
        );
        assert_eq!(app.option_value(BORDER_OPTION), "plain");
        for name in ["thick", "double", "rounded", "plain"] {
            press(&mut app, K::Char('6'), M::NONE);
            assert_eq!(app.option_value(BORDER_OPTION), name);
        }
        press(&mut app, K::Char('6'), M::NONE);
        assert_eq!(app.border_style(), crate::config::BorderStyle::Thick);
        assert_eq!(app.message_layout(), MessageLayout::Cards);
        press(&mut app, K::Char('4'), M::NONE);
        assert!(!app.settings.notifications.sound);
        assert_eq!(app.option_value(SOUND_OPTION), "[ ]");
        press(&mut app, K::Char('5'), M::NONE);
        assert_eq!(app.settings.volume(), 80);
        press(&mut app, K::Char('h'), M::NONE);
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.option_value(VOLUME_OPTION), "60%");
        for _ in 0..8 {
            press(&mut app, K::Right, M::NONE);
        }
        assert_eq!(app.settings.volume(), 100);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.settings.volume(), 0);
        press(&mut app, K::Left, M::NONE);
        assert_eq!(app.settings.volume(), 0);
        press(&mut app, K::Char('k'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::Options { sel }) if sel == SOUND_OPTION));
        press(&mut app, K::Char('l'), M::NONE);
        assert!(app.settings.notifications.sound);
        press(&mut app, K::Char('l'), M::NONE);
        assert!(app.settings.notifications.sound, "l means on, not toggle");
        press(&mut app, K::Left, M::NONE);
        assert!(!app.settings.notifications.sound);
        press(&mut app, K::Char('h'), M::NONE);
        assert!(!app.settings.notifications.sound);
        press(&mut app, K::Char(' '), M::NONE);
        assert!(app.settings.notifications.sound);
        press(&mut app, K::Esc, M::NONE);
        assert!(app.popup.is_none());
    }

    #[test]
    fn composer_stays_focused_after_send_and_enter_opens_menu() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        press(&mut app, K::Char('i'), M::NONE);
        for c in "hey".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "send"), 1);
        assert!(app.composer.active);
        assert!(app.composer.input.text().is_empty());
        press(&mut app, K::Char('x'), M::NONE);
        assert_eq!(app.composer.input.text(), "x");
        press(&mut app, K::Esc, M::NONE);
        assert!(!app.composer.active);
        let h = app.histories.get_mut(&10).unwrap();
        h.get_mut(400).unwrap().attachments = vec![Attachment::new(
            MediaKind::Photo,
            FileRef {
                id: 9,
                ..FileRef::default()
            },
        )];
        press(&mut app, K::Char('k'), M::NONE);
        assert_eq!(app.selected_message().map(|m| m.id), Some(400));
        press(&mut app, K::Enter, M::NONE);
        assert!(matches!(app.popup, Some(Popup::Menu { .. })));
        assert!(!matches!(app.popup, Some(Popup::Viewer { .. })));
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(matches!(app.popup, Some(Popup::Menu { .. })));
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char('p'), M::NONE);
        assert!(app.popup.is_none());
    }

    #[test]
    fn pinned_bar_jump_and_read_outbox_updates() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        assert_eq!(count(&log, "load pinned"), 1);
        press(&mut app, K::Char('g'), M::NONE);
        press(&mut app, K::Char('p'), M::NONE);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, _)| t.contains("no pinned"))
        );
        let pinned = app.histories[&10].get(200).unwrap().clone();
        app.handle(Event::Pinned {
            chat_id: 10,
            result: Ok(Some(pinned)),
        });
        press(&mut app, K::Char('g'), M::NONE);
        press(&mut app, K::Char('p'), M::NONE);
        assert_eq!(app.selected_message().map(|m| m.id), Some(200));
        app.handle(Event::Pinned {
            chat_id: 10,
            result: Ok(None),
        });
        assert!(!app.pinned_msg.contains_key(&10));
        app.on_update(Update::ChatReadOutbox(
            tdlib_rs::types::UpdateChatReadOutbox {
                chat_id: 10,
                last_read_outbox_message_id: 300,
            },
        ));
        assert_eq!(app.chats[&10].last_read_outbox, 300);
    }

    #[test]
    fn notifications_carry_avatar_and_sound() {
        let (mut app, log) = fake_app(Settings::default());
        let entries = |log: &td::RequestLog, prefix: &str| -> Vec<String> {
            log.lock()
                .unwrap()
                .iter()
                .filter(|e| e.starts_with(prefix))
                .cloned()
                .collect()
        };
        app.on_incoming(&msg(20, 1, 2));
        assert_eq!(entries(&log, "notify icon"), ["notify icon None"]);
        assert_eq!(
            entries(&log, "sound "),
            [format!(
                "sound canberra-gtk-play -f {} -V -3.1",
                crate::config::cache_dir()
                    .unwrap()
                    .join(notify::DEFAULT_SOUND_NAME)
                    .display()
            )]
        );
        app.user_meta.entry(2).or_default().photo = Some(FileRef {
            id: 77,
            ..FileRef::default()
        });
        app.on_incoming(&msg(20, 2, 2));
        assert_eq!(
            entries(&log, "notify icon").last().unwrap(),
            "notify icon Download(77)"
        );
        app.media.on_file(FileRef {
            id: 77,
            path: "/p/alice.jpg".to_owned(),
            done: true,
            ..FileRef::default()
        });
        app.on_incoming(&msg(20, 3, 2));
        assert_eq!(
            entries(&log, "notify icon").last().unwrap(),
            "notify icon Path(\"/p/alice.jpg\")"
        );
        app.chats.get_mut(&10).unwrap().photo = Some(FileRef {
            id: 88,
            ..FileRef::default()
        });
        app.on_incoming(&msg(10, 4, 2));
        assert_eq!(
            entries(&log, "notify icon").last().unwrap(),
            "notify icon Download(88)"
        );
        assert_eq!(entries(&log, "sound ").len(), 4);
        app.on_incoming(&msg(30, 5, 2));
        assert_eq!(entries(&log, "sound ").len(), 4);
        app.settings.notifications.desktop = false;
        app.settings.notifications.volume = 100;
        app.on_incoming(&msg(20, 6, 2));
        assert_eq!(entries(&log, "notify icon").len(), 4);
        assert_eq!(
            *entries(&log, "sound ").last().unwrap(),
            format!(
                "sound canberra-gtk-play -f {} -V 0.0",
                crate::config::cache_dir()
                    .unwrap()
                    .join(notify::DEFAULT_SOUND_NAME)
                    .display()
            )
        );
        app.settings.notifications.sound = false;
        app.on_incoming(&msg(20, 7, 2));
        assert_eq!(entries(&log, "sound ").len(), 5);
        app.settings.notifications.sound = true;
        app.sound_env = Some(notify::SoundEnv::default());
        app.on_incoming(&msg(20, 8, 2));
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("Space o"))
        );
        app.sound_env = Some(notify::SoundEnv {
            pw_play: true,
            ..notify::SoundEnv::default()
        });
        app.on_incoming(&msg(20, 9, 2));
        assert_eq!(entries(&log, "sound ").len(), 5);
        app.toggle_option(SOUND_OPTION);
        app.toggle_option(SOUND_OPTION);
        app.on_incoming(&msg(20, 10, 2));
        assert!(
            entries(&log, "sound ")
                .last()
                .unwrap()
                .starts_with("sound pw-play --volume 1.00 --channels 2 --channel-map FL,FR /")
        );
        app.handle(Event::SoundFailed("pw-play failed".to_owned()));
        app.on_incoming(&msg(20, 11, 2));
        assert_eq!(entries(&log, "sound ").len(), 6);
    }

    #[test]
    fn notifications_respect_mute_mentions_and_viewing() {
        let (mut app, log) = fake_app(Settings::default());
        app.on_incoming(&msg(10, 1, 2));
        assert_eq!(count(&log, "notify"), 1);
        app.on_incoming(&msg(30, 2, 2));
        assert_eq!(count(&log, "notify"), 1);
        let mut mention = msg(30, 3, 2);
        mention.mention = true;
        app.on_incoming(&mention);
        assert_eq!(count(&log, "notify"), 2);
        app.on_incoming(&msg(10, 4, 1));
        let mut old = msg(10, 5, 2);
        old.date -= 3600;
        app.on_incoming(&old);
        assert_eq!(count(&log, "notify"), 2);
        open_with_history(&mut app, 10);
        app.on_incoming(&msg(10, 500, 2));
        assert_eq!(count(&log, "notify"), 2);
        app.handle(Event::Input(TermEvent::FocusLost));
        app.on_incoming(&msg(10, 510, 2));
        assert_eq!(count(&log, "notify"), 3);
        app.settings.notifications.desktop = false;
        app.on_incoming(&msg(20, 6, 2));
        assert_eq!(count(&log, "notify"), 3);
        let ids: Vec<i64> = app.inbox.iter().map(|i| i.msg_id).collect();
        assert_eq!(ids, [6, 510, 500, 3, 1]);
        app.chats.get_mut(&30).unwrap().use_default_mute = true;
        assert!(!app.is_muted(30));
        app.scope_muted[1] = true;
        assert!(app.is_muted(30));
        app.handle(Event::NotifyFailed("no notify-send".to_owned()));
        app.settings.notifications.desktop = true;
        app.on_incoming(&msg(20, 7, 2));
        assert_eq!(count(&log, "notify"), 3);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("notify-send"))
        );
    }

    #[test]
    fn inbox_and_search_jump_to_messages() {
        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        app.inbox.push_front(InboxItem {
            chat_id: 10,
            msg_id: 200,
            sender: Sender::User(2),
            preview: "hi".to_owned(),
            date: unix_now(),
            mention: false,
            topic: None,
        });
        leader(&mut app, 'n');
        press(&mut app, K::Enter, M::NONE);
        let h = app.histories.get(&10).unwrap();
        assert_eq!(app.msg_sel.map(|i| h.msgs()[i].id), Some(200));
        app.jump_to(10, 5);
        assert_eq!(count(&log, "load history"), 1);
        assert_eq!(count(&log, "load around"), 1);
        app.handle(Event::HistoryAround {
            chat_id: 10,
            target: 5,
            result: Ok(vec![
                msg(10, 10, 2),
                msg(10, 7, 2),
                msg(10, 5, 2),
                msg(10, 3, 2),
            ]),
        });
        let h = app.histories.get(&10).unwrap();
        assert_eq!(app.msg_sel.map(|i| h.msgs()[i].id), Some(5));
        app.jump_to(10, 4);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("deleted"))
        );
        app.focus = Pane::Messages;
        press(&mut app, K::Char('/'), M::NONE);
        assert!(matches!(&app.popup, Some(Popup::Search(s)) if s.chat_id == Some(10)));
        for c in "text".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "search"), 1);
        let token = match &app.popup {
            Some(Popup::Search(s)) => s.token,
            _ => panic!("search closed"),
        };
        app.handle(Event::Search {
            token: token + 7,
            result: Ok(vec![msg(10, 100, 2)]),
        });
        assert!(matches!(&app.popup, Some(Popup::Search(s)) if s.results.is_empty()));
        app.handle(Event::Search {
            token,
            result: Ok(vec![msg(10, 300, 2), msg(10, 100, 2)]),
        });
        press(&mut app, K::Down, M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none());
        let h = app.histories.get(&10).unwrap();
        assert_eq!(app.msg_sel.map(|i| h.msgs()[i].id), Some(100));
        assert_eq!(count(&log, "mark read"), 0);
    }

    #[test]
    fn message_actions_call_the_right_requests() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        let h = app.histories.get_mut(&10).unwrap();
        let m = h.get_mut(400).unwrap();
        m.reply_to = Some(100);
        m.reactions = vec![model::Reaction {
            label: "👍".to_owned(),
            emoji: Some("👍".to_owned()),
            custom: 0,
            count: 2,
            chosen: true,
        }];
        m.keyboard = Some(Keyboard {
            inline: true,
            rows: vec![vec![
                bots::Button {
                    text: "Go".to_owned(),
                    action: ButtonAction::Callback("Zw==".to_owned()),
                },
                bots::Button {
                    text: "Site".to_owned(),
                    action: ButtonAction::Url("https://example.org".to_owned()),
                },
            ]],
            placeholder: String::new(),
        });
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        let Some(Popup::Menu { items, .. }) = &app.popup else {
            panic!("no menu");
        };
        let keys: String = items.iter().map(|i| i.key).collect();
        for k in ['y', 'r', 'f', 'P', 'p', 'g', 'u', 'b'] {
            assert!(keys.contains(k), "{keys}");
        }
        press(&mut app, K::Char('b'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::Buttons(_))));
        press(&mut app, K::Char('1'), M::NONE);
        assert_eq!(count(&log, "callback"), 1);
        app.handle(Event::CallbackAnswer(Ok(td::Answer {
            text: "Done!".to_owned(),
            alert: false,
            url: String::new(),
        })));
        assert_eq!(app.status.as_ref().map(|s| s.0.as_str()), Some("🤖 Done!"));
        leader(&mut app, 'k');
        press(&mut app, K::Char('2'), M::NONE);
        assert!(matches!(&app.popup, Some(Popup::ConfirmUrl(u)) if u == "https://example.org"));
        press(&mut app, K::Char('n'), M::NONE);
        assert!(app.popup.is_none());
        press(&mut app, K::Char('r'), M::NONE);
        assert_eq!(count(&log, "available reactions"), 1);
        app.handle(Event::AvailableReactions {
            chat_id: 10,
            message_id: 400,
            result: Ok(td::ReactionChoices {
                emojis: vec!["❤".to_owned(), "👍".to_owned()],
                note: None,
            }),
        });
        press(&mut app, K::Char('l'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "react"), 1);
        press(&mut app, K::Char('P'), M::NONE);
        assert_eq!(count(&log, "pin"), 1);
        press(&mut app, K::Char('f'), M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::Switcher {
                forward: Some(_),
                ..
            })
        ));
        for c in "chat 20".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "forward"), 1);
        assert_eq!(app.open_chat, Some(10));
        leader(&mut app, 'a');
        press(&mut app, K::Char('g'), M::NONE);
        let h = app.histories.get(&10).unwrap();
        assert_eq!(app.msg_sel.map(|i| h.msgs()[i].id), Some(100));
        leader(&mut app, 'a');
        press(&mut app, K::Char('p'), M::NONE);
        assert!(
            matches!(&app.popup, Some(Popup::Profile(p)) if p.target == ProfileTarget::User(2))
        );
        assert_eq!(count(&log, "load profile"), 1);
    }

    #[test]
    fn bot_commands_complete_in_composer() {
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 20);
        let cmd = |n: &str| Command {
            name: n.to_owned(),
            description: String::new(),
            bot: "abot".to_owned(),
        };
        app.handle(Event::BotCommands {
            chat_id: 20,
            result: Ok(vec![cmd("start"), cmd("settings"), cmd("help")]),
        });
        press(&mut app, K::Char('i'), M::NONE);
        press(&mut app, K::Char('/'), M::NONE);
        press(&mut app, K::Char('s'), M::NONE);
        assert_eq!(app.command_suggestions().len(), 2);
        press(&mut app, K::Down, M::NONE);
        press(&mut app, K::Tab, M::NONE);
        assert_eq!(app.composer.input.text(), "/start ");
        assert!(app.command_suggestions().is_empty());
    }

    #[test]
    fn presence_and_typing_feed_subtitles() {
        let (mut app, _log) = fake_app(Settings::default());
        app.user_meta.insert(
            2,
            UserMeta {
                status: presence::Status::Online {
                    expires: unix_now() + 60,
                },
                ..UserMeta::default()
            },
        );
        assert_eq!(app.chat_subtitle(20).as_deref(), Some("online"));
        assert!(app.is_online(2));
        app.typing.insert(
            20,
            vec![presence::Typing::new(
                Sender::User(2),
                Activity::Typing,
                None,
            )],
        );
        assert_eq!(app.chat_subtitle(20).as_deref(), Some("online"));
        assert_eq!(
            app.typing_line(20, None).as_deref(),
            Some("Alice is typing…")
        );
        app.typing.insert(
            10,
            vec![presence::Typing::new(
                Sender::User(2),
                Activity::RecordingVoice,
                None,
            )],
        );
        assert_eq!(app.chat_subtitle(10), None);
        assert_eq!(
            app.typing_line(10, None).as_deref(),
            Some("Alice is recording a voice message…")
        );
        app.on_update(Update::NewMessage(tdlib_rs::types::UpdateNewMessage {
            message: serde_json::from_value(serde_json::json!({
                "@type": "message", "id": 99, "chat_id": 10,
                "sender_id": {"@type": "messageSenderUser", "user_id": 2},
                "is_outgoing": false, "is_pinned": false, "is_from_offline": false,
                "can_be_saved": true, "has_timestamped_media": false, "is_channel_post": false,
                "is_paid_star_suggested_post": false, "is_paid_ton_suggested_post": false,
                "contains_unread_mention": false, "date": 1, "edit_date": 0,
                "unread_reactions": [], "self_destruct_in": 0.0, "auto_delete_in": 0.0,
                "via_bot_user_id": 0, "sender_business_bot_user_id": 0, "sender_boost_count": 0,
                "paid_message_star_count": 0, "author_signature": "", "media_album_id": "0",
                "effect_id": "0", "summary_language_code": "",
                "content": {"@type": "messageText", "text": {"@type": "formattedText", "text": "hi", "entities": []}}
            }))
            .expect("message json"),
        }));
        assert!(!app.typing.contains_key(&10));
        app.user_meta.get_mut(&2).unwrap().status = presence::Status::LastWeek;
        app.typing.clear();
        assert_eq!(
            app.chat_subtitle(20).as_deref(),
            Some("last seen within a week")
        );
        assert_eq!(app.chat_subtitle(10), None);
    }

    fn chat_action(
        app: &mut App,
        user: i64,
        topic: Option<i32>,
        action: tdlib_rs::enums::ChatAction,
    ) {
        use tdlib_rs::{enums, types};
        app.on_update(Update::ChatAction(types::UpdateChatAction {
            chat_id: 10,
            topic_id: topic.map(|forum_topic_id| {
                enums::MessageTopic::Forum(types::MessageTopicForum { forum_topic_id })
            }),
            sender_id: enums::MessageSender::User(types::MessageSenderUser { user_id: user }),
            action,
        }));
    }

    #[test]
    fn chat_actions_feed_the_typing_line_per_topic_and_expire() {
        use tdlib_rs::enums::ChatAction as A;
        let (mut app, _log) = fake_app(ghost());
        app.lang = format::Lang::Ru;
        app.users.insert(3, "ильнур Х".to_owned());
        app.users.insert(4, "Вася".to_owned());
        app.users.insert(2, "рустем Г".to_owned());
        chat_action(&mut app, 2, Some(7), A::Typing);
        assert_eq!(
            app.typing_line(10, None).as_deref(),
            Some("рустем печатает…")
        );
        assert_eq!(
            app.typing_line(10, Some(7)).as_deref(),
            Some("рустем печатает…")
        );
        assert_eq!(app.typing_line(10, Some(8)), None);
        chat_action(&mut app, 3, Some(7), A::Typing);
        assert_eq!(
            app.typing_line(10, Some(7)).as_deref(),
            Some("рустем, ильнур печатают…")
        );
        chat_action(&mut app, 4, Some(7), A::Typing);
        assert_eq!(
            app.typing_line(10, Some(7)).as_deref(),
            Some("рустем и ещё 2 печатают…")
        );
        chat_action(&mut app, 3, Some(7), A::Cancel);
        chat_action(&mut app, 4, Some(7), A::Cancel);
        chat_action(&mut app, 2, Some(7), A::RecordingVoiceNote);
        assert_eq!(
            app.typing_line(10, Some(7)).as_deref(),
            Some("рустем записывает голосовое…")
        );
        for (action, text) in [
            (A::RecordingVideo, "записывает видео"),
            (A::ChoosingSticker, "выбирает стикер"),
        ] {
            chat_action(&mut app, 2, Some(7), action);
            assert_eq!(
                app.typing_line(10, Some(7)),
                Some(format!("рустем {text}…"))
            );
        }
        chat_action(&mut app, 2, Some(7), A::Cancel);
        assert!(!app.typing.contains_key(&10));
        chat_action(&mut app, 2, None, A::Typing);
        let since = app.typing[&10][0].since;
        app.prune_typing(since + std::time::Duration::from_millis(5900));
        assert!(app.typing.contains_key(&10));
        assert!(app.tick_interval().is_some(), "redraws to expire the line");
        app.prune_typing(since + presence::TYPING_TIMEOUT);
        assert!(
            !app.typing.contains_key(&10),
            "lost cancel expires after 6 s"
        );
        assert_eq!(app.tick_interval(), None);
        chat_action(&mut app, 1, None, A::Typing);
        assert!(!app.typing.contains_key(&10), "own actions are not shown");
    }

    fn history(app: &mut App, chat: i64, ids: std::ops::RangeInclusive<i64>) {
        app.handle(Event::History {
            chat_id: chat,
            msgs: ids.rev().map(|i| msg(chat, i * 10, 2)).collect(),
            initial: true,
            epoch: 0,
        });
    }

    fn set_unread(app: &mut App, chat: i64, unread: i32, last_read: i64) {
        let c = app.chats.get_mut(&chat).unwrap();
        c.unread = unread;
        c.last_read_inbox = last_read;
    }

    #[test]
    fn next_matching_wraps_and_skips_current() {
        let order = [30, 20, 10];
        let all = |_: i64| true;
        assert_eq!(next_matching(&order, Some(20), false, all), Some(10));
        assert_eq!(next_matching(&order, Some(10), false, all), Some(30));
        assert_eq!(next_matching(&order, Some(10), true, all), Some(10));
        assert_eq!(next_matching(&order, None, false, all), Some(30));
        assert_eq!(
            next_matching(&order, Some(99), false, |id| id == 10),
            Some(10)
        );
        assert_eq!(next_matching(&order, Some(20), false, |id| id == 20), None);
        assert_eq!(
            next_matching(&order, Some(20), true, |id| id == 20),
            Some(20)
        );
        assert_eq!(next_matching(&[], None, true, all), None);
    }

    #[test]
    fn next_unread_follows_list_order_wraps_and_skips_muted() {
        let (mut app, log) = fake_app(Settings::default());
        app.enter_chat(20);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(10));
        assert_eq!(app.visible_chats()[app.chat_sel], 10);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(20), "muted 30 is skipped on wrap");
        app.settings.navigation.skip_muted = false;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(10));
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(30));
        app.settings.navigation.skip_muted = true;
        set_unread(&mut app, 10, 0, 0);
        set_unread(&mut app, 20, 0, 0);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(30));
        assert!(matches!(&app.status, Some((s, false)) if s == "no unread chats"));
        app.enter_chat(10);
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(10));
        assert!(
            matches!(&app.status, Some((s, false)) if s == "no unread chats (1 muted skipped)")
        );
        assert_eq!(count(&log, "find mention"), 0);
    }

    #[test]
    fn composer_ctrl_n_opens_next_unread_unless_suggesting_commands() {
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 20);
        press(&mut app, K::Char('i'), M::NONE);
        press(&mut app, K::Char('n'), M::CONTROL);
        assert_eq!(app.open_chat, Some(10));
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 20);
        app.bot_commands.insert(
            20,
            vec![
                Command {
                    name: "start".to_owned(),
                    description: String::new(),
                    bot: "b".to_owned(),
                },
                Command {
                    name: "stop".to_owned(),
                    description: String::new(),
                    bot: "b".to_owned(),
                },
            ],
        );
        press(&mut app, K::Char('i'), M::NONE);
        press(&mut app, K::Char('/'), M::NONE);
        press(&mut app, K::Char('n'), M::CONTROL);
        assert_eq!(app.open_chat, Some(20));
        assert_eq!(app.cmd_sel, 1);
    }

    #[test]
    fn next_mention_searches_the_right_chat_and_jumps_to_the_message() {
        let (mut app, log) = fake_app(Settings::default());
        app.chats.get_mut(&30).unwrap().mentions = 0;
        app.enter_chat(10);
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('m'), M::NONE);
        assert_eq!(count(&log, "find mention"), 1);
        app.chats.get_mut(&10).unwrap().mentions = 0;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('m'), M::NONE);
        assert_eq!(count(&log, "find mention"), 2);
        app.handle(Event::Mention {
            chat_id: 20,
            result: Ok(Some((150, None))),
        });
        assert_eq!(app.open_chat, Some(20));
        assert_eq!(app.focus, Pane::Messages);
        history(&mut app, 20, 1..=40);
        assert_eq!(app.selected_message().map(|m| m.id), Some(150));
        assert!(!app.composer.active);
        app.chats.get_mut(&20).unwrap().mentions = 0;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('m'), M::NONE);
        assert_eq!(count(&log, "find mention"), 2);
        assert!(matches!(&app.status, Some((s, false)) if s == "no unread mentions"));
        app.handle(Event::Mention {
            chat_id: 20,
            result: Ok(None),
        });
        assert!(matches!(&app.status, Some((s, false)) if s.contains("no unread mentions")));
        app.handle(Event::Mention {
            chat_id: 20,
            result: Err("boom".to_owned()),
        });
        assert!(matches!(&app.status, Some((s, true)) if s.contains("boom")));
    }

    #[test]
    fn smart_focus_opens_composer_when_unread_fit_on_screen() {
        let (mut app, log) = fake_app(Settings::default());
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        assert!(!app.composer.active);
        assert_eq!(count(&log, "mark read"), 0);
        history(&mut app, 20, 1..=40);
        assert!(app.composer.active);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(app.msg_sel, None);
        assert_eq!(count(&log, "mark read"), 1);
        press(&mut app, K::Esc, M::NONE);
        assert!(!app.composer.active);
        assert_eq!(app.focus, Pane::Messages);
        set_unread(&mut app, 10, 0, 0);
        app.enter_chat(10);
        assert!(
            app.composer.active,
            "no unread opens the composer right away"
        );
    }

    #[test]
    fn smart_focus_selects_first_unread_when_they_exceed_a_screen() {
        let (mut app, log) = fake_app(Settings::default());
        set_unread(&mut app, 20, 35, 50);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        assert!(!app.composer.active);
        assert_eq!(app.focus, Pane::Messages);
        assert_eq!(app.selected_message().map(|m| m.id), Some(60));
        assert_eq!(app.msg_top, 5);
        assert_eq!(count(&log, "mark read"), 1);
        let lines = crate::ui::lines_from(&mut app, 20, 5);
        assert!(lines > app.msg_view_height, "{lines}");
        assert!(crate::ui::lines_from(&mut app, 20, 38) <= app.msg_view_height);
    }

    #[test]
    fn smart_focus_loads_older_until_the_first_unread_is_known() {
        let (mut app, log) = fake_app(Settings::default());
        set_unread(&mut app, 20, 99, 0);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        assert_eq!(count(&log, "load history"), 2);
        assert!(app.msg_sel.is_none());
        app.handle(Event::History {
            chat_id: 20,
            msgs: Vec::new(),
            initial: false,
            epoch: 0,
        });
        assert_eq!(app.selected_message().map(|m| m.id), Some(10));
        assert!(!app.composer.active);
    }

    #[test]
    fn open_focus_modes_and_key_during_load_cancels_decision() {
        let mut s = Settings::default();
        s.navigation.open_chat_focus = OpenFocus::Composer;
        let (mut app, _log) = fake_app(s);
        set_unread(&mut app, 20, 35, 50);
        app.enter_chat(20);
        assert!(app.composer.active);
        let mut s = Settings::default();
        s.navigation.open_chat_focus = OpenFocus::Messages;
        let (mut app, _log) = fake_app(s);
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        assert!(!app.composer.active);
        assert_eq!(app.selected_message().map(|m| m.id), Some(390));
        let (mut app, _log) = fake_app(Settings::default());
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        press(&mut app, K::Char('2'), M::NONE);
        history(&mut app, 20, 1..=40);
        assert!(!app.composer.active);
        assert_eq!(app.focus, Pane::Chats);
    }

    #[test]
    fn enter_in_chat_list_applies_open_focus() {
        let (mut app, _log) = fake_app(Settings::default());
        set_unread(&mut app, 30, 2, 380);
        app.focus = Pane::Chats;
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.open_chat, Some(30));
        history(&mut app, 30, 1..=40);
        assert!(app.composer.active);
    }

    #[test]
    fn options_toggle_open_focus_and_after_send() {
        let (mut app, _log) = fake_app(Settings::default());
        leader(&mut app, 'o');
        assert_eq!(app.option_value(OPEN_FOCUS_OPTION), "smart");
        press(&mut app, K::Char('9'), M::NONE);
        assert_eq!(app.settings.navigation.open_chat_focus, OpenFocus::Composer);
        assert_eq!(app.option_value(OPEN_FOCUS_OPTION), "composer");
        press(&mut app, K::Char('9'), M::NONE);
        press(&mut app, K::Char('9'), M::NONE);
        assert_eq!(app.option_value(OPEN_FOCUS_OPTION), "smart");
        assert_eq!(app.option_value(AFTER_SEND_OPTION), "[ ]");
        press(&mut app, K::Char('0'), M::NONE);
        assert!(app.settings.navigation.after_send_next_unread);
        assert_eq!(app.option_value(AFTER_SEND_OPTION), "[x]");
        assert!(matches!(app.popup, Some(Popup::Options { sel: 9 })));
    }

    #[test]
    fn after_send_opens_next_unread_only_when_chat_is_read() {
        let mut s = Settings::default();
        s.navigation.after_send_next_unread = true;
        let (mut app, log) = fake_app(s);
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        assert!(app.composer.active);
        press(&mut app, K::Char('h'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "send"), 1);
        assert_eq!(app.open_chat, Some(10));

        let mut s = no_read();
        s.navigation.after_send_next_unread = true;
        let (mut app, _log) = fake_app(s);
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        press(&mut app, K::Char('h'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.open_chat, Some(20), "unread remain when reads are off");

        let (mut app, _log) = fake_app(Settings::default());
        set_unread(&mut app, 20, 2, 380);
        app.enter_chat(20);
        history(&mut app, 20, 1..=40);
        press(&mut app, K::Char('h'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.open_chat, Some(20), "option is off by default");
    }

    fn logged(log: &td::RequestLog, prefix: &str) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| e.starts_with(prefix) && e.len() > prefix.len())
            .cloned()
            .collect()
    }

    fn tmsg(chat_id: i64, id: i64, sender: i64, topic: Option<i32>) -> Msg {
        Msg {
            topic,
            ..msg(chat_id, id, sender)
        }
    }

    fn ftopic(id: i32, order: i64, unread: i32) -> model::Topic {
        model::Topic {
            id,
            name: format!("topic {id}"),
            order,
            unread,
            use_default_mute: true,
            last_message: Some(tmsg(40, i64::from(id) * 1000, 2, Some(id))),
            ..model::Topic::default()
        }
    }

    fn forum_app(settings: Settings) -> (App, td::RequestLog) {
        let (mut app, log) = fake_app(settings);
        let mut c = chat(40, ChatKind::Supergroup, false);
        c.forum = true;
        c.group_id = 400;
        app.chats.insert(40, c);
        (app, log)
    }

    fn feed_topics(app: &mut App, topics: Vec<model::Topic>, next: Option<model::TopicCursor>) {
        app.handle(Event::Topics {
            chat_id: 40,
            result: Ok(td::TopicPage {
                total: topics.len() as i32 + i32::from(next.is_some()),
                topics,
                next,
            }),
        });
    }

    fn standard_topics() -> Vec<model::Topic> {
        let mut general = ftopic(1, 100, 0);
        general.general = true;
        general.name = "General".to_owned();
        let mut pinned = ftopic(6, 1, 0);
        pinned.pinned = true;
        let mut closed = ftopic(7, 40, 1);
        closed.closed = true;
        vec![general, ftopic(5, 50, 2), pinned, closed]
    }

    fn update(v: serde_json::Value) -> Update {
        serde_json::from_value(v).expect("valid update JSON")
    }

    fn new_message(chat_id: i64, id: i64, topic: i32) -> Update {
        update(serde_json::json!({
            "@type": "updateNewMessage",
            "message": {
                "@type": "message", "id": id, "chat_id": chat_id,
                "sender_id": {"@type": "messageSenderUser", "user_id": 2},
                "is_outgoing": false, "is_pinned": false, "is_from_offline": false,
                "can_be_saved": true, "has_timestamped_media": false, "is_channel_post": false,
                "is_paid_star_suggested_post": false, "is_paid_ton_suggested_post": false,
                "contains_unread_mention": false, "date": unix_now(), "edit_date": 0,
                "unread_reactions": [], "self_destruct_in": 0.0, "auto_delete_in": 0.0,
                "via_bot_user_id": 0, "sender_business_bot_user_id": 0, "sender_boost_count": 0,
                "paid_message_star_count": 0, "author_signature": "", "media_album_id": "0",
                "effect_id": "0", "summary_language_code": "",
                "topic_id": {"@type": "messageTopicForum", "forum_topic_id": topic},
                "content": {"@type": "messageText",
                            "text": {"@type": "formattedText", "text": "new", "entities": []}},
            },
        }))
    }

    #[test]
    fn forum_expands_into_topics_paginates_and_opens_a_topic() {
        let (mut app, log) = forum_app(Settings::default());
        app.focus = Pane::Chats;
        app.refresh_lists();
        assert_eq!(app.selected_row(), Some(Row::Chat(40)));
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.expanded, Some(40));
        assert_eq!(app.open_chat, Some(40));
        assert_eq!(app.open_topic, None);
        assert_eq!(app.focus, Pane::Chats, "the topic list is in the chat pane");
        assert_eq!(logged(&log, "topics 40"), vec!["topics 40 None"]);
        assert_eq!(app.chat_rows()[..2], [Row::Chat(40), Row::Loading(40)]);
        let cursor = model::TopicCursor {
            date: 9,
            message_id: 7000,
            topic_id: 7,
        };
        feed_topics(&mut app, standard_topics(), Some(cursor));
        assert_eq!(
            app.chat_rows()[..6],
            [
                Row::Chat(40),
                Row::Topic(40, 6),
                Row::Topic(40, 1),
                Row::Topic(40, 5),
                Row::Topic(40, 7),
                Row::Chat(30),
            ],
            "pinned first, then by order"
        );
        assert_eq!(
            app.selected_row(),
            Some(Row::Topic(40, 5)),
            "first unread topic"
        );
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(logged(&log, "topics 40").len(), 2, "next page near the end");
        assert!(logged(&log, "topics 40")[1].contains("topic_id: 7"));
        assert!(app.chat_rows().contains(&Row::Loading(40)));
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(
            logged(&log, "topics 40").len(),
            2,
            "no duplicate page request"
        );
        feed_topics(&mut app, vec![ftopic(8, 10, 0)], None);
        assert!(app.chat_rows().contains(&Row::Topic(40, 8)));
        assert!(!app.chat_rows().contains(&Row::Loading(40)));

        app.select_row(Row::Topic(40, 5));
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.open_topic, Some(5));
        assert_eq!(app.view_title(), "chat 40 › topic 5");
        assert_eq!(
            logged(&log, "history 40"),
            vec!["history 40 topic None", "history 40 topic Some(5)"]
        );
        app.handle(Event::History {
            chat_id: 40,
            msgs: vec![tmsg(40, 4000, 2, Some(5)), tmsg(40, 5000, 2, Some(5))],
            initial: true,
            epoch: 0,
        });
        assert!(
            app.histories[&40].is_empty(),
            "stale chat-wide page is dropped"
        );
        app.handle(Event::History {
            chat_id: 40,
            msgs: vec![tmsg(40, 5000, 2, Some(5)), tmsg(40, 4000, 2, Some(5))],
            initial: true,
            epoch: 1,
        });
        app.handle(Event::History {
            chat_id: 40,
            msgs: Vec::new(),
            initial: true,
            epoch: 1,
        });
        assert_eq!(history_ids(&app, 40), vec![4000, 5000]);
        assert_eq!(
            logged(&log, "view 40"),
            vec!["view 40 [5000] topic Some(5)"]
        );
        assert_eq!(
            app.topic(40, 5).unwrap().unread,
            0,
            "read locally right away"
        );
        assert!(app.composer.active);
        for c in "hi".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(
            logged(&log, "send 40"),
            vec!["send 40 topic Some(5) reply None edit None"]
        );
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Char('R'), M::NONE);
        press(&mut app, K::Char('x'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(
            logged(&log, "send 40")[1],
            "send 40 topic Some(5) reply Some(5000) edit None"
        );

        app.on_update(new_message(40, 6500, 6));
        assert_eq!(
            history_ids(&app, 40),
            vec![4000, 5000],
            "other topic stays out"
        );
        assert_eq!(app.topic(40, 6).unwrap().unread, 1);
        app.on_update(new_message(40, 6600, 5));
        assert_eq!(history_ids(&app, 40), vec![4000, 5000, 6600]);
        assert_eq!(
            app.chat_rows()[1],
            Row::Topic(40, 6),
            "pinned topic stays on top"
        );
        assert_eq!(
            app.chat_rows()[2],
            Row::Topic(40, 5),
            "active topic moves up"
        );

        app.enter_topic(40, 1);
        assert_eq!(app.open_topic, Some(1));
        press(&mut app, K::Char('i'), M::NONE);
        press(&mut app, K::Char('g'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(
            logged(&log, "send 40").last().unwrap(),
            "send 40 topic None reply None edit None",
            "General gets messages without a topic"
        );
        press(&mut app, K::Esc, M::NONE);
        app.focus = Pane::Chats;
        app.select_row(Row::Chat(40));
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(app.expanded, None, "Enter on an open forum folds it");
    }

    #[test]
    fn topics_respect_dont_mark_read_and_ghost_but_read_on_request() {
        for settings in [no_read(), ghost()] {
            let (mut app, log) = forum_app(settings);
            app.expand(40);
            let mut topics = standard_topics();
            topics[1].mentions = 1;
            feed_topics(&mut app, topics, None);
            app.enter_topic(40, 5);
            app.handle(Event::History {
                chat_id: 40,
                msgs: (1..=40).map(|i| tmsg(40, i * 10, 2, Some(5))).collect(),
                initial: true,
                epoch: 0,
            });
            browse(&mut app);
            type_text(&mut app);
            assert_eq!(count(&log, "mark read"), 0);
            leader(&mut app, 'm');
            assert_eq!(logged(&log, "view 40"), vec!["view 40 [400] topic Some(5)"]);
            assert_eq!(
                logged(&log, "read topic mentions"),
                vec!["read topic mentions 40 5"]
            );
            app.focus = Pane::Chats;
            app.select_row(Row::Topic(40, 7));
            leader(&mut app, 'a');
            press(&mut app, K::Char('m'), M::NONE);
            assert_eq!(
                logged(&log, "view 40").last().unwrap(),
                "view 40 [7000] topic Some(7)"
            );
        }
        let (mut app, log) = forum_app(Settings::default());
        app.enter_forum(40);
        app.handle(Event::History {
            chat_id: 40,
            msgs: (1..=5).map(|i| tmsg(40, i * 10, 2, Some(5))).collect(),
            initial: true,
            epoch: 0,
        });
        browse(&mut app);
        assert_eq!(
            count(&log, "mark read"),
            0,
            "the mixed forum view never reads"
        );
    }

    #[test]
    fn next_unread_walks_topics_then_chats_and_mentions_open_their_topic() {
        let (mut app, log) = forum_app(Settings::default());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.enter_topic(40, 5);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!((app.open_chat, app.open_topic), (Some(40), Some(7)));
        if let Some(list) = app.topics.get_mut(&40) {
            list.mark_read_up_to(5, 99_999);
            list.mark_read_up_to(7, 99_999);
        }
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(20), "no unread topics left, next chat");
        assert_eq!(app.open_topic, None);

        let (mut app, log2) = forum_app(Settings::default());
        app.chats.get_mut(&20).unwrap().unread = 0;
        app.chats.get_mut(&30).unwrap().unread = 0;
        app.enter_chat(10);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.open_chat, Some(40));
        assert_eq!(logged(&log2, "topics 40"), vec!["topics 40 None"]);
        feed_topics(&mut app, standard_topics(), None);
        assert_eq!(
            app.open_topic,
            Some(5),
            "first unread topic opens when topics arrive"
        );

        app.handle(Event::Mention {
            chat_id: 40,
            result: Ok(Some((7000, Some(7)))),
        });
        assert_eq!(app.open_topic, Some(7));
        assert_eq!(
            logged(&log2, "around 40").last().unwrap(),
            "around 40 7000 topic Some(7)"
        );
        drop(log);
    }

    #[test]
    fn topic_menu_is_gated_by_rights_and_calls_topic_requests() {
        let keys = |app: &App| -> String {
            match &app.popup {
                Some(Popup::Menu { items, .. }) => items.iter().map(|i| i.key).collect(),
                _ => String::new(),
            }
        };
        let (mut app, log) = forum_app(Settings::default());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.focus = Pane::Chats;
        app.select_row(Row::Topic(40, 5));
        leader(&mut app, 'a');
        assert_eq!(keys(&app), "omMy", "plain member");
        press(&mut app, K::Esc, M::NONE);
        app.chats.get_mut(&40).unwrap().perms.create_topics = true;
        leader(&mut app, 'a');
        assert_eq!(keys(&app), "omMyn", "members may create topics");
        press(&mut app, K::Esc, M::NONE);
        app.groups.insert(
            400,
            model::GroupMeta {
                forum: true,
                status: model::Membership::Admin(model::Rights {
                    manage_topics: true,
                    ..model::Rights::default()
                }),
            },
        );
        leader(&mut app, 'a');
        assert_eq!(keys(&app), "omMPcyn");
        press(&mut app, K::Char('M'), M::NONE);
        assert_eq!(logged(&log, "mute topic"), vec!["mute topic 40 5 true"]);
        assert!(app.topic_muted(40, 5));
        leader(&mut app, 'a');
        let label = match &app.popup {
            Some(Popup::Menu { items, .. }) => items[2].label,
            _ => "",
        };
        assert_eq!(label, "Unmute topic");
        press(&mut app, K::Char('P'), M::NONE);
        assert_eq!(logged(&log, "pin topic"), vec!["pin topic 40 5 true"]);
        leader(&mut app, 'a');
        press(&mut app, K::Char('c'), M::NONE);
        assert_eq!(logged(&log, "close topic"), vec!["close topic 40 5 true"]);
        leader(&mut app, 'a');
        press(&mut app, K::Char('y'), M::NONE);
        assert_eq!(logged(&log, "topic link"), vec!["topic link 40 5"]);
        app.select_row(Row::Topic(40, 1));
        leader(&mut app, 'a');
        assert_eq!(keys(&app), "omMPhyn", "General hides instead of closing");
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(logged(&log, "hide general"), vec!["hide general 40 true"]);
        leader(&mut app, 'a');
        press(&mut app, K::Char('n'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::NewTopic { .. })));
        press(&mut app, K::Enter, M::NONE);
        assert!(
            matches!(app.popup, Some(Popup::NewTopic { .. })),
            "empty name"
        );
        for c in "Ideas".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(logged(&log, "create topic"), vec!["create topic 40 Ideas"]);
        app.handle(Event::TopicCreated {
            chat_id: 40,
            result: Ok(model::Topic {
                id: 9,
                name: "Ideas".to_owned(),
                ..model::Topic::default()
            }),
        });
        assert_eq!(app.open_topic, Some(9));
        assert_eq!(logged(&log, "refresh topic"), vec!["refresh topic 40 9"]);

        app.on_update(update(serde_json::json!({
            "@type": "updateForumTopic", "chat_id": 40, "forum_topic_id": 7, "is_pinned": true,
            "last_read_inbox_message_id": 7000, "last_read_outbox_message_id": 0,
            "unread_mention_count": 0, "unread_reaction_count": 0,
            "notification_settings": {
                "@type": "chatNotificationSettings", "use_default_mute_for": true, "mute_for": 0,
                "use_default_sound": true, "sound_id": "0", "use_default_show_preview": true,
                "show_preview": true, "use_default_mute_stories": true, "mute_stories": false,
                "use_default_story_sound": true, "story_sound_id": "0",
                "use_default_show_story_poster": true, "show_story_poster": true,
                "use_default_disable_pinned_message_notifications": true,
                "disable_pinned_message_notifications": false,
                "use_default_disable_mention_notifications": true,
                "disable_mention_notifications": false,
            },
        })));
        let t7 = app.topic(40, 7).unwrap();
        assert!(t7.pinned);
        assert_eq!(t7.unread, 0, "read up to the last message");
        assert_eq!(
            app.chat_rows()[1..3],
            [Row::Topic(40, 7), Row::Topic(40, 6)]
        );
    }

    #[test]
    fn supergroup_updates_mark_forums_and_rights() {
        let (mut app, _log) = fake_app(Settings::default());
        app.chats.get_mut(&10).unwrap().group_id = 100;
        app.on_update(update(serde_json::json!({
            "@type": "updateSupergroup",
            "supergroup": {
                "@type": "supergroup", "id": 100, "date": 0,
                "status": {"@type": "chatMemberStatusCreator", "custom_title": "", "is_anonymous": false, "is_member": true},
                "member_count": 3, "boost_level": 0, "has_automatic_translation": false,
                "has_linked_chat": false, "has_location": false, "sign_messages": false,
                "show_message_sender": false, "join_to_send_messages": false, "join_by_request": false,
                "is_slow_mode_enabled": false, "is_channel": false, "is_broadcast_group": false,
                "is_forum": true, "is_direct_messages_group": false,
                "is_administered_direct_messages_group": false, "has_direct_messages_group": false,
                "has_forum_tabs": false, "paid_message_star_count": 0,
            },
        })));
        assert!(app.is_forum(10));
        assert_eq!(app.rights(10), model::Rights::ALL);
        assert!(app.rights(20).change_info, "private chats may set a timer");
    }

    fn secret_app(settings: Settings, state: model::SecretState) -> (App, td::RequestLog) {
        let (mut app, log) = fake_app(settings);
        let mut c = chat(50, ChatKind::Secret(2), false);
        c.secret_id = 7;
        app.chats.insert(50, c);
        app.secret_chats.insert(
            7,
            model::SecretChat {
                id: 7,
                user_id: 2,
                state,
                outbound: true,
            },
        );
        (app, log)
    }

    #[test]
    fn secret_chats_wait_until_ready_then_send_like_normal_chats() {
        let (mut app, log) = secret_app(Settings::default(), model::SecretState::Pending);
        open_with_history(&mut app, 50);
        assert!(
            app.chat_subtitle(50)
                .unwrap()
                .starts_with("secret chat: waiting")
        );
        type_text(&mut app);
        press(&mut app, K::Char('i'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(count(&log, "send"), 0);
        assert!(matches!(&app.status, Some((s, true)) if s.contains("not ready")));
        app.on_update(update(serde_json::json!({
            "@type": "updateSecretChat",
            "secret_chat": {"@type": "secretChat", "id": 7, "user_id": 2,
                            "state": {"@type": "secretChatStateReady"},
                            "is_outbound": true, "key_hash": "", "layer": 144},
        })));
        assert_eq!(
            app.secret_chat(50).unwrap().state,
            model::SecretState::Ready
        );
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(
            logged(&log, "send 50"),
            vec!["send 50 topic None reply None edit None"]
        );
        assert!(
            count(&log, "mark read") >= 1,
            "secret chats read like any other chat"
        );

        app.on_update(update(serde_json::json!({
            "@type": "updateChatMessageAutoDeleteTime", "chat_id": 50, "message_auto_delete_time": 30,
        })));
        assert!(app.chat_subtitle(50).unwrap().ends_with("30s"));
        press(&mut app, K::Esc, M::NONE);
        app.focus = Pane::Chats;
        app.refresh_lists();
        app.select_row(Row::Chat(50));
        leader(&mut app, 'a');
        let keys: String = match &app.popup {
            Some(Popup::Menu { items, .. }) => items.iter().map(|i| i.key).collect(),
            _ => String::new(),
        };
        assert_eq!(keys, "ompP/RtxD");
        press(&mut app, K::Char('t'), M::NONE);
        assert!(
            matches!(app.popup, Some(Popup::Ttl { sel: 2, .. })),
            "current value selected"
        );
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(logged(&log, "auto delete"), vec!["auto delete 50 5"]);

        leader(&mut app, 'a');
        press(&mut app, K::Char('x'), M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::ConfirmSecret { delete: false, .. })
        ));
        press(&mut app, K::Char('n'), M::NONE);
        assert!(app.popup.is_none());
        assert!(logged(&log, "secret close").is_empty());
        leader(&mut app, 'a');
        press(&mut app, K::Char('x'), M::NONE);
        press(&mut app, K::Char('y'), M::NONE);
        assert_eq!(logged(&log, "secret close"), vec!["secret close 7"]);

        app.secret_chats.get_mut(&7).unwrap().state = model::SecretState::Closed;
        leader(&mut app, 'a');
        let keys: String = match &app.popup {
            Some(Popup::Menu { items, .. }) => items.iter().map(|i| i.key).collect(),
            _ => String::new(),
        };
        assert_eq!(keys, "ompP/RtD", "closed chats can only be deleted");
        press(&mut app, K::Char('D'), M::NONE);
        press(&mut app, K::Char('y'), M::NONE);
        assert_eq!(
            logged(&log, "secret delete"),
            vec!["secret delete 50 7 false"]
        );
        assert_eq!(app.open_chat, None);
    }

    #[test]
    fn ghost_mode_never_reads_secret_chats() {
        let (mut app, log) = secret_app(ghost(), model::SecretState::Ready);
        open_with_history(&mut app, 50);
        browse(&mut app);
        type_text(&mut app);
        assert_eq!(count(&log, "mark read"), 0);
        assert_eq!(count(&log, "chat action"), 0);
    }

    #[test]
    fn profile_starts_a_secret_chat_with_people_only() {
        let (mut app, log) = fake_app(Settings::default());
        app.open_profile(ProfileTarget::User(2));
        if let Some(Popup::Profile(p)) = &mut app.popup {
            p.data = Some(Profile {
                user_id: Some(2),
                ..Profile::default()
            });
        }
        press(&mut app, K::Char('S'), M::NONE);
        assert_eq!(logged(&log, "secret new"), vec!["secret new 2"]);
        let mut c = chat(50, ChatKind::Secret(2), false);
        c.secret_id = 7;
        app.chats.insert(50, c);
        app.handle(Event::SecretCreated(Ok(50)));
        assert!(app.popup.is_none());
        assert_eq!(app.open_chat, Some(50));
        assert!(matches!(&app.status, Some((s, false)) if s.contains("only on this device")));

        app.user_meta.entry(3).or_default().bot = true;
        app.open_profile(ProfileTarget::User(3));
        if let Some(Popup::Profile(p)) = &mut app.popup {
            p.data = Some(Profile {
                user_id: Some(3),
                ..Profile::default()
            });
        }
        press(&mut app, K::Char('S'), M::NONE);
        assert_eq!(
            logged(&log, "secret new").len(),
            1,
            "no secret chats with bots"
        );
    }

    fn private_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gramtty-5b-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn keeping() -> Settings {
        let mut s = Settings::default();
        s.privacy.keep_deleted = true;
        s
    }

    fn saveable(app: &mut App, chat: i64, ids: &[i64]) {
        let h = app.histories.get_mut(&chat).unwrap();
        for id in ids {
            h.get_mut(*id).unwrap().meta.can_be_saved = true;
        }
    }

    fn delete_update(chat: i64, ids: &[i64]) -> Update {
        update(serde_json::json!({
            "@type": "updateDeleteMessages", "chat_id": chat, "message_ids": ids,
            "is_permanent": true, "from_cache": false,
        }))
    }

    fn content_update(chat: i64, id: i64, text: &str) -> Update {
        update(serde_json::json!({
            "@type": "updateMessageContent", "chat_id": chat, "message_id": id,
            "new_content": {"@type": "messageText",
                "text": {"@type": "formattedText", "text": text, "entities": []}},
        }))
    }

    fn message_update(chat: i64, id: i64, content: serde_json::Value) -> Update {
        update(serde_json::json!({
            "@type": "updateNewMessage",
            "message": {
                "@type": "message", "id": id, "chat_id": chat,
                "sender_id": {"@type": "messageSenderUser", "user_id": 2},
                "is_outgoing": false, "is_pinned": false, "is_from_offline": false,
                "can_be_saved": true, "has_timestamped_media": false, "is_channel_post": false,
                "is_paid_star_suggested_post": false, "is_paid_ton_suggested_post": false,
                "contains_unread_mention": false, "date": unix_now(), "edit_date": 0,
                "unread_reactions": [], "self_destruct_in": 0.0, "auto_delete_in": 0.0,
                "via_bot_user_id": 0, "sender_business_bot_user_id": 0, "sender_boost_count": 0,
                "paid_message_star_count": 0, "author_signature": "", "media_album_id": "0",
                "effect_id": "0", "summary_language_code": "",
                "content": content,
            }
        }))
    }

    #[test]
    fn deleted_messages_stay_in_place_persist_per_account_and_skip_protected() {
        let dir = private_dir("deleted");
        let path = dir.join("archive").join("1.jsonl");
        let (mut app, _log) = fake_app(keeping());
        app.data_dir = dir.clone();
        app.on_update(update(serde_json::json!({
            "@type": "updateOption", "name": "my_id",
            "value": {"@type": "optionValueInteger", "value": "1"},
        })));
        assert_eq!(app.archive.path(), Some(path.as_path()));
        open_with_history(&mut app, 10);
        saveable(&mut app, 10, &[100, 110]);
        app.on_update(delete_update(10, &[100, 110, 120]));
        let h = &app.histories[&10];
        assert!(h.get(100).is_some_and(|m| m.deleted));
        assert!(h.get(110).is_some_and(|m| m.deleted));
        assert!(h.get(120).is_none(), "can_be_saved=false is not kept");
        let shown = crate::ui::display_msgs(&app, app.histories[&10].msgs());
        let marked = shown.iter().find(|m| m.id == 100).unwrap();
        assert!(marked.body.text.starts_with(crate::ui::DELETED_MARK));
        assert!(
            marked
                .body
                .entities
                .iter()
                .any(|e| e.kind == model::EntityKind::Strike)
        );
        app.msg_sel = app.histories[&10].index_of(100);
        app.focus = Pane::Messages;
        press(&mut app, K::Enter, M::NONE);
        let Some(Popup::Menu { items, .. }) = &app.popup else {
            panic!("menu expected");
        };
        assert!(items.iter().all(|i| i.action != MenuAction::Delete));
        assert!(items.iter().any(|i| i.action == MenuAction::Details));
        app.popup = None;

        let (mut fresh, _log) = fake_app(keeping());
        fresh.data_dir = dir.clone();
        fresh.account_archive(1);
        open_with_history(&mut fresh, 10);
        let raw: Vec<Msg> = (1..=40)
            .rev()
            .filter(|i| ![10, 11, 12].contains(i))
            .map(|i| msg(10, i * 10, 2))
            .collect();
        let epoch = fresh.histories[&10].epoch + 1;
        fresh.reset_history(10, None, epoch);
        fresh.handle(Event::History {
            chat_id: 10,
            msgs: raw,
            initial: true,
            epoch,
        });
        let h = &fresh.histories[&10];
        assert!(
            h.get(100)
                .is_some_and(|m| m.deleted && m.body.text == "text 100")
        );
        assert!(h.get(110).is_some_and(|m| m.deleted));
        assert!(h.get(120).is_none());

        fresh.account_archive(2);
        assert_eq!(fresh.archive.len(), 0, "another account sees nothing");
        fresh.account_archive(1);
        assert_eq!(fresh.archive.len(), 2);

        app.chats.get_mut(&30).unwrap().protected = true;
        open_with_history(&mut app, 30);
        saveable(&mut app, 30, &[100]);
        app.on_update(delete_update(30, &[100]));
        assert!(app.histories[&30].get(100).is_none());
        assert_eq!(app.archive.len(), 2, "protected chats are never stored");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("\"chat\":30"), "{raw}");

        let mut off = keeping();
        off.privacy.keep_deleted = false;
        app.settings = off;
        open_with_history(&mut app, 20);
        saveable(&mut app, 20, &[100]);
        app.on_update(delete_update(20, &[100]));
        assert!(app.histories[&20].get(100).is_none());
        assert_eq!(app.archive.len(), 2);

        app.settings = keeping();
        open_with_history(&mut app, 10);
        app.popup = Some(Popup::Options { sel: 0 });
        assert_eq!(app.option_value(CLEAR_DELETED_OPTION), "2 saved");
        press(&mut app, K::Char('c'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::ConfirmClearDeleted)));
        press(&mut app, K::Char('y'), M::NONE);
        assert_eq!(app.archive.len(), 0);
        assert!(!path.exists());
        assert!(matches!(app.popup, Some(Popup::Options { sel }) if sel == CLEAR_DELETED_OPTION));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_keep_previous_versions_and_show_edit_history() {
        let (mut app, _log) = fake_app(keeping());
        open_with_history(&mut app, 10);
        saveable(&mut app, 10, &[100]);
        app.on_update(content_update(10, 100, "text 100"));
        assert!(
            !app.archive.has_edits(10, 100),
            "unchanged text is not a version"
        );
        app.on_update(content_update(10, 100, "changed"));
        app.on_update(update(serde_json::json!({
            "@type": "updateMessageEdited", "chat_id": 10, "message_id": 100,
            "edit_date": 1_700_000_100, "reply_markup": null,
        })));
        app.on_update(content_update(10, 100, "changed again"));
        app.on_update(content_update(10, 120, "not saveable"));
        assert!(!app.archive.has_edits(10, 120));
        let versions = app.edit_versions(10, 100);
        let texts: Vec<&str> = versions.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["text 100", "changed", "changed again"]);
        assert!(versions[0].0.starts_with("sent "));
        assert_eq!(
            versions[1].0,
            format!("edited {}", format::stamp(1_700_000_100))
        );
        app.msg_sel = app.histories[&10].index_of(100);
        app.focus = Pane::Messages;
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('h'), M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::EditHistory {
                chat_id: 10,
                msg_id: 100,
                ..
            })
        ));
        let rows = app.details_rows(10, 100).unwrap();
        assert!(rows.iter().any(|(k, v)| k == "Saved versions" && v == "2"));
        app.settings.privacy.keep_deleted = false;
        app.on_update(content_update(10, 100, "third"));
        assert_eq!(app.edit_versions(10, 100).len(), 3);
    }

    #[test]
    fn details_popup_loads_flags_and_sender_search_jumps() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.msg_sel = app.histories[&10].index_of(200);
        app.focus = Pane::Messages;
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('i'), M::NONE);
        assert!(matches!(
            app.popup,
            Some(Popup::Details {
                msg_id: 200,
                flags: None,
                ..
            })
        ));
        assert_eq!(count(&log, "message properties"), 1);
        app.handle(Event::Properties {
            chat_id: 10,
            message_id: 200,
            result: Ok(td::Flags {
                can_be_saved: true,
                can_be_forwarded: false,
                can_be_copied: true,
            }),
        });
        let rows = app.details_rows(10, 200).unwrap();
        let get = |k: &str| {
            rows.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("Message ID").as_deref(), Some("200"));
        assert_eq!(get("Sender ID").as_deref(), Some("2 (user)"));
        assert_eq!(get("Can be forwarded").as_deref(), Some("no"));
        assert_eq!(get("Sent").map(|s| s.len()), Some(19));
        press(&mut app, K::Esc, M::NONE);
        assert!(app.popup.is_none());

        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('a'), M::NONE);
        let Some(Popup::Search(sv)) = &app.popup else {
            panic!("search expected");
        };
        assert_eq!(sv.sender, Some(Sender::User(2)));
        assert!(sv.loading);
        let token = sv.token;
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e == "search Some(10) sender Some(User(2))")
        );
        app.handle(Event::Search {
            token,
            result: Ok(vec![msg(10, 5, 2), msg(10, 300, 2)]),
        });
        press(&mut app, K::Char('j'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none());
        let sel = app.msg_sel.unwrap();
        assert_eq!(app.histories[&10].msgs()[sel].id, 300);

        app.popup = Some(Popup::Profile(ProfileView {
            target: ProfileTarget::User(2),
            data: None,
            error: None,
            scroll: 0,
        }));
        press(&mut app, K::Char('a'), M::NONE);
        assert!(
            matches!(&app.popup, Some(Popup::Search(sv)) if sv.sender == Some(Sender::User(2)) && sv.chat_id == Some(10))
        );
    }

    #[test]
    fn blocked_senders_collapse_in_groups_until_enter() {
        let mut s = Settings::default();
        s.privacy.hide_blocked = true;
        let (mut app, log) = fake_app(Settings::default());
        app.popup = Some(Popup::Options {
            sel: HIDE_BLOCKED_OPTION,
        });
        press(&mut app, K::Char('b'), M::NONE);
        assert!(app.settings.privacy.hide_blocked);
        assert_eq!(count(&log, "blocked senders"), 1);
        press(&mut app, K::Char('b'), M::NONE);
        press(&mut app, K::Char('b'), M::NONE);
        assert_eq!(count(&log, "blocked senders"), 1, "loaded once");
        app.popup = None;
        app.handle(Event::Blocked(Ok(vec![Sender::User(2)])));
        open_with_history(&mut app, 10);
        let m = app.histories[&10].get(200).unwrap().clone();
        assert!(app.hidden_blocked(&m));
        let mut mine = m.clone();
        mine.sender = Sender::User(3);
        assert!(!app.hidden_blocked(&mine));
        let shown = crate::ui::display_msgs(&app, app.histories[&10].msgs());
        let row = shown.iter().find(|x| x.id == 200).unwrap();
        assert_eq!(row.body.text, crate::ui::BLOCKED_PLACEHOLDER);
        assert!(row.attachments.is_empty() && row.reactions.is_empty());
        app.msg_sel = app.histories[&10].index_of(200);
        app.focus = Pane::Messages;
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none(), "first Enter reveals");
        assert!(!app.hidden_blocked(&m));
        press(&mut app, K::Enter, M::NONE);
        assert!(matches!(app.popup, Some(Popup::Menu { .. })));
        app.popup = None;
        let other = app.histories[&10].get(210).unwrap().clone();
        assert!(app.hidden_blocked(&other));
        let mut private = other.clone();
        private.chat_id = 20;
        assert!(
            !app.hidden_blocked(&private),
            "private chats are not collapsed"
        );
        app.on_update(update(serde_json::json!({
            "@type": "updateChatBlockList", "chat_id": 20, "block_list": null,
        })));
        assert!(!app.hidden_blocked(&other));
        app.on_update(update(serde_json::json!({
            "@type": "updateChatBlockList", "chat_id": 20,
            "block_list": {"@type": "blockListMain"},
        })));
        assert!(app.hidden_blocked(&other));
        app.settings = s;
        app.settings.privacy.hide_blocked = false;
        assert!(!app.hidden_blocked(&other));
    }

    #[test]
    fn hidden_stories_are_dropped_from_chats() {
        let story = serde_json::json!({
            "@type": "messageStory", "story_poster_chat_id": 20, "story_id": 7,
            "via_mention": true,
        });
        let text = serde_json::json!({"@type": "messageText",
            "text": {"@type": "formattedText", "text": "hi", "entities": []}});
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.on_update(message_update(10, 500, story.clone()));
        assert!(app.histories[&10].get(500).is_some_and(|m| m.meta.story));
        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('t'), M::NONE);
        assert!(app.settings.privacy.hide_stories);
        app.on_update(message_update(10, 510, story.clone()));
        app.on_update(message_update(10, 520, text));
        assert!(app.histories[&10].get(510).is_none());
        assert!(app.histories[&10].get(520).is_some());
        let mut page: Vec<Msg> = (1..=5).rev().map(|i| msg(10, i, 2)).collect();
        page[0].meta.story = true;
        let epoch = app.histories[&10].epoch;
        app.histories.get_mut(&10).unwrap().loading_older = true;
        app.handle(Event::History {
            chat_id: 10,
            msgs: page,
            initial: false,
            epoch,
        });
        assert!(app.histories[&10].get(5).is_none());
        assert!(app.histories[&10].get(4).is_some());
    }

    #[test]
    fn ids_seconds_and_exact_number_options() {
        let (mut app, _log) = fake_app(Settings::default());
        app.popup = Some(Popup::Options { sel: 0 });
        assert!(!app.frame_theme().layout.seconds);
        press(&mut app, K::Char('s'), M::NONE);
        assert!(app.frame_theme().layout.seconds);
        assert_eq!(app.option_value(SECONDS_OPTION), "[x]");
        assert!(app.settings.to_toml().unwrap().contains("seconds = true"));
        assert_eq!(app.count(1234), "1.2K");
        press(&mut app, K::Char('n'), M::NONE);
        assert_eq!(app.count(1234), "1234");
        assert!(!app.settings.appearance.show_ids);
        press(&mut app, K::Char('i'), M::NONE);
        assert!(app.settings.appearance.show_ids);
        let restored = Settings::parse(&app.settings.to_toml().unwrap(), &mut Vec::new()).unwrap();
        assert_eq!(restored, app.settings);
    }

    fn select(app: &mut App, chat: i64, id: i64) {
        app.msg_sel = app.histories[&chat].index_of(id);
        app.focus = Pane::Messages;
    }

    fn menu_keys(app: &App) -> String {
        match &app.popup {
            Some(Popup::Menu { items, .. }) => items.iter().map(|i| i.key).collect(),
            _ => panic!("no menu"),
        }
    }

    #[test]
    fn copy_as_quote_and_reply_with_a_quoted_fragment() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        let keys = menu_keys(&app);
        for k in ['Y', 'Q', 't', 'S'] {
            assert!(keys.contains(k), "{keys}");
        }
        assert!(!keys.contains('C'), "{keys}");
        press(&mut app, K::Char('Y'), M::NONE);
        let copied = logged(&log, "copy ");
        assert_eq!(copied.len(), 1);
        assert!(
            copied[0].starts_with("copy > text 200\n— Alice Smith, "),
            "{copied:?}"
        );

        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('Q'), M::NONE);
        assert!(
            matches!(&app.popup, Some(Popup::QuotePick { msg_id: 200, input, .. }) if input.text() == "text 200")
        );
        press(&mut app, K::Char('x'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(matches!(app.popup, Some(Popup::QuotePick { .. })));
        assert!(app.status.as_ref().is_some_and(|s| s.1));
        press(&mut app, K::Backspace, M::NONE);
        press(&mut app, K::Home, M::NONE);
        for _ in 0..5 {
            press(&mut app, K::Delete, M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none());
        assert!(app.composer.active);
        assert_eq!(app.composer.reply_to, Some(200));
        assert_eq!(app.composer.quote, Some(("200".to_owned(), 5)));
        for c in "agreed".chars() {
            press(&mut app, K::Char(c), M::NONE);
        }
        press(&mut app, K::Enter, M::NONE);
        assert_eq!(logged(&log, "quote "), vec!["quote 5 200".to_owned()]);
        assert_eq!(app.composer.quote, None);

        app.composer.active = false;
        select(&mut app, 10, 300);
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('Q'), M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(app.composer.quote.is_some());
        press(&mut app, K::Esc, M::NONE);
        assert_eq!(app.composer.quote, None);
        assert_eq!(app.composer.reply_to, None);
    }

    #[test]
    fn translate_uses_telegram_only() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('t'), M::NONE);
        assert_eq!(
            logged(&log, "translate "),
            vec!["translate 10 200 ru".to_owned()]
        );
        app.handle(Event::Translated {
            chat_id: 10,
            message_id: 200,
            result: Ok("текст 200".to_owned()),
        });
        assert!(matches!(
            &app.popup,
            Some(Popup::Translate { result: Some(Ok(t)), .. }) if t == "текст 200"
        ));
        press(&mut app, K::Char('y'), M::NONE);
        assert!(logged(&log, "copy ").contains(&"copy текст 200".to_owned()));
        press(&mut app, K::Esc, M::NONE);

        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('t'), M::NONE);
        app.handle(Event::Translated {
            chat_id: 10,
            message_id: 200,
            result: Err("PREMIUM_ACCOUNT_REQUIRED (400)".to_owned()),
        });
        assert!(matches!(
            &app.popup,
            Some(Popup::Translate { result: Some(Err(e)), .. }) if e.contains("Telegram Premium") && e.contains("PREMIUM_ACCOUNT_REQUIRED")
        ));
        assert_eq!(logged(&log, "translate ").len(), 2);
        press(&mut app, K::Esc, M::NONE);

        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('g'), M::NONE);
        assert_eq!(app.settings.chats.translate_to, "en");
        assert_eq!(app.option_value(TRANSLATE_OPTION), "en");
        press(&mut app, K::Char('h'), M::NONE);
        assert_eq!(app.settings.chats.translate_to, "ru");
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('t'), M::NONE);
        assert_eq!(
            logged(&log, "translate ").last(),
            Some(&"translate 10 200 ru".to_owned())
        );
        app.handle(Event::Translated {
            chat_id: 10,
            message_id: 999,
            result: Ok("stale".to_owned()),
        });
        assert!(matches!(
            app.popup,
            Some(Popup::Translate { result: None, .. })
        ));
        app.handle(Event::Translated {
            chat_id: 10,
            message_id: 200,
            result: Ok("перевод".to_owned()),
        });
        assert!(matches!(
            &app.popup,
            Some(Popup::Translate { result: Some(Ok(t)), .. }) if t == "перевод"
        ));
    }

    #[test]
    fn folder_counters_follow_options_and_all_chats_can_be_hidden() {
        let (mut app, _log) = fake_app(Settings::default());
        app.folders = vec![(3, "Work".to_owned())];
        app.handle(Event::Update(
            0,
            Box::new(update(serde_json::json!({
                "@type": "updateUnreadChatCount",
                "chat_list": {"@type": "chatListMain"},
                "total_count": 50,
                "unread_count": 1234,
                "unread_unmuted_count": 7,
                "marked_as_unread_count": 0,
                "marked_as_unread_unmuted_count": 0
            }))),
        ));
        app.handle(Event::Update(
            0,
            Box::new(update(serde_json::json!({
                "@type": "updateUnreadMessageCount",
                "chat_list": {"@type": "chatListMain"},
                "unread_count": 4321,
                "unread_unmuted_count": 12
            }))),
        ));
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), Some("1.2K"));
        app.settings.appearance.exact_numbers = true;
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), Some("1234"));
        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), Some("7"));
        press(&mut app, K::Char('f'), M::NONE);
        assert_eq!(
            app.option_value(FOLDER_COUNTER_OPTION),
            "unread messages".to_owned()
        );
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), Some("12"));
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), Some("4321"));
        for id in [20, 30] {
            app.chats.get_mut(&id).unwrap().positions.push(Position {
                list: ListId::Folder(3),
                order: id,
                pinned: false,
            });
        }
        app.chats.get_mut(&20).unwrap().unread = 5;
        assert_eq!(app.folder_badge(ListId::Folder(3)).as_deref(), Some("7"));
        press(&mut app, K::Char('u'), M::NONE);
        assert_eq!(app.folder_badge(ListId::Folder(3)).as_deref(), Some("5"));
        press(&mut app, K::Char('f'), M::NONE);
        assert_eq!(app.folder_badge(ListId::Folder(3)).as_deref(), None);
        assert_eq!(app.folder_badge(ListId::Main).as_deref(), None);
        press(&mut app, K::Char('f'), M::NONE);
        assert_eq!(app.folder_badge(ListId::Folder(3)).as_deref(), Some("1"));

        app.folder_sel = 1;
        assert_eq!(app.current_list(), ListId::Folder(3));
        press(&mut app, K::Char('x'), M::NONE);
        let lists: Vec<ListId> = app.folder_entries().iter().map(|(l, _)| *l).collect();
        assert_eq!(lists, vec![ListId::Folder(3), ListId::Archive]);
        assert_eq!(app.current_list(), ListId::Folder(3));
        app.folders.clear();
        assert_eq!(app.folder_entries()[0].0, ListId::Main);
        let restored = Settings::parse(&app.settings.to_toml().unwrap(), &mut Vec::new()).unwrap();
        assert_eq!(restored, app.settings);
    }

    #[test]
    fn hiding_all_chats_keeps_the_selected_folder_when_folders_arrive() {
        let mut settings = Settings::default();
        settings.appearance.hide_all_chats = true;
        let (mut app, log) = fake_app(settings);
        assert_eq!(app.current_list(), ListId::Main);
        app.handle(Event::Update(
            0,
            Box::new(update(serde_json::json!({
                "@type": "updateChatFolders",
                "chat_folders": [{
                    "id": 4,
                    "name": {"text": {"text": "Friends", "entities": []}, "animate_custom_emoji": false},
                    "icon": {"name": "Custom"},
                    "color_id": -1,
                    "is_shareable": false,
                    "has_my_invite_links": false
                }],
                "main_chat_list_position": 0,
                "are_tags_enabled": false
            }))),
        ));
        assert_eq!(app.current_list(), ListId::Folder(4));
        assert_eq!(count(&log, "load chats"), 1);
    }

    fn admin_app() -> (App, td::RequestLog) {
        let (mut app, log) = fake_app(Settings::default());
        app.chats.get_mut(&10).unwrap().group_id = 77;
        app.groups.insert(
            77,
            model::GroupMeta {
                forum: false,
                status: model::Membership::Admin(model::Rights::default()),
            },
        );
        app.focus = Pane::Chats;
        app.refresh_lists();
        (app, log)
    }

    fn select_chat(app: &mut App, chat: i64) {
        app.focus = Pane::Chats;
        app.refresh_lists();
        app.chat_sel = app
            .chat_rows()
            .iter()
            .position(|r| r.chat() == chat)
            .unwrap();
    }

    #[test]
    fn admins_browse_members_with_filters_and_search() {
        let (mut app, log) = admin_app();
        select_chat(&mut app, 30);
        leader(&mut app, 'a');
        assert!(!menu_keys(&app).contains('M'));
        press(&mut app, K::Esc, M::NONE);
        select_chat(&mut app, 10);
        leader(&mut app, 'a');
        let keys = menu_keys(&app);
        assert!(keys.contains('M') && keys.contains('L'), "{keys}");
        press(&mut app, K::Char('M'), M::NONE);
        assert_eq!(
            logged(&log, "members "),
            vec!["members 77 recent \"\"".to_owned()]
        );
        let Some(Popup::Members(mv)) = &app.popup else {
            panic!("members expected");
        };
        let first = mv.token;
        press(&mut app, K::Char('a'), M::NONE);
        assert_eq!(
            logged(&log, "members ").last().unwrap(),
            "members 77 recent \"a\""
        );
        let member = |id: i64| crate::admin::Member {
            sender: Sender::User(id),
            status: "member".to_owned(),
            joined: 0,
        };
        app.handle(Event::Members {
            token: first,
            result: Ok(crate::admin::MemberPage {
                total: 1,
                members: vec![member(5)],
            }),
        });
        assert!(matches!(&app.popup, Some(Popup::Members(mv)) if mv.page.is_none()));
        let Some(Popup::Members(mv)) = &app.popup else {
            panic!();
        };
        let token = mv.token;
        app.handle(Event::Members {
            token,
            result: Ok(crate::admin::MemberPage {
                total: 2,
                members: vec![member(2), member(5)],
            }),
        });
        press(&mut app, K::Tab, M::NONE);
        assert_eq!(
            logged(&log, "members ").last().unwrap(),
            "members 77 admins \"a\""
        );
        press(&mut app, K::BackTab, M::NONE);
        let Some(Popup::Members(mv)) = &app.popup else {
            panic!();
        };
        let token = mv.token;
        app.handle(Event::Members {
            token,
            result: Ok(crate::admin::MemberPage {
                total: 2,
                members: vec![member(2), member(5)],
            }),
        });
        press(&mut app, K::Down, M::NONE);
        press(&mut app, K::Enter, M::NONE);
        assert!(
            matches!(&app.popup, Some(Popup::Profile(p)) if p.target == ProfileTarget::User(5))
        );
    }

    #[test]
    fn admins_read_the_recent_actions_log_page_by_page() {
        let (mut app, log) = admin_app();
        select_chat(&mut app, 10);
        leader(&mut app, 'a');
        press(&mut app, K::Char('L'), M::NONE);
        assert_eq!(
            logged(&log, "event log "),
            vec!["event log 10 from 0".to_owned()]
        );
        let entry = |id: i64| crate::admin::LogEntry {
            id,
            date: 1_700_000_000,
            actor: Sender::User(2),
            action: "invited".to_owned(),
            target: Some(Sender::User(id)),
            detail: String::new(),
        };
        app.handle(Event::ActionLog {
            chat_id: 10,
            from: 0,
            result: Ok((1..=crate::admin::LOG_PAGE as i64)
                .rev()
                .map(entry)
                .collect()),
        });
        for _ in 0..crate::admin::LOG_PAGE - 2 {
            press(&mut app, K::Char('j'), M::NONE);
        }
        assert_eq!(logged(&log, "event log ").len(), 1);
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(
            logged(&log, "event log ").last().unwrap(),
            "event log 10 from 1"
        );
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(logged(&log, "event log ").len(), 2);
        app.handle(Event::ActionLog {
            chat_id: 10,
            from: 1,
            result: Ok(vec![entry(0)]),
        });
        let Some(Popup::EventLog(lv)) = &app.popup else {
            panic!();
        };
        assert!(lv.done);
        assert_eq!(lv.entries.len(), crate::admin::LOG_PAGE + 1);
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(logged(&log, "event log ").len(), 2);
        press(&mut app, K::Enter, M::NONE);
        assert!(
            matches!(&app.popup, Some(Popup::Profile(p)) if p.target == ProfileTarget::User(0))
        );

        app.groups.insert(
            77,
            model::GroupMeta {
                forum: false,
                status: model::Membership::Member,
            },
        );
        app.popup = None;
        select_chat(&mut app, 10);
        leader(&mut app, 'a');
        assert!(!menu_keys(&app).contains('L'));
    }

    #[test]
    fn copy_photo_downloads_first_then_copies_png() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        let file = FileRef {
            id: 9,
            size: 100,
            ..FileRef::default()
        };
        app.histories
            .get_mut(&10)
            .unwrap()
            .get_mut(200)
            .unwrap()
            .attachments = vec![Attachment::new(MediaKind::Photo, file.clone())];
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        assert!(menu_keys(&app).contains('C'));
        press(&mut app, K::Char('C'), M::NONE);
        assert!(logged(&log, "copy image").is_empty());
        app.handle(Event::File(FileRef {
            path: "/nonexistent/photo.jpg".to_owned(),
            done: true,
            ..file
        }));
        assert_eq!(
            logged(&log, "copy image "),
            vec!["copy image /nonexistent/photo.jpg".to_owned()]
        );
        app.handle(Event::ImageCopied(Err("no wl-copy".to_owned())));
        assert_eq!(
            app.status,
            Some(("copy photo: no wl-copy".to_owned(), true))
        );
        app.handle(Event::ImageCopied(Ok(())));
        assert_eq!(
            app.status,
            Some(("photo copied to clipboard".to_owned(), false))
        );
    }

    #[test]
    fn photos_attach_as_files_when_the_default_says_original() {
        let dir = private_dir("photos-as-files");
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("shot.png");
        let doc = dir.join("notes.txt");
        std::fs::write(&photo, b"png").unwrap();
        std::fs::write(&doc, b"txt").unwrap();
        let (mut app, _log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        app.attach_paths(vec![photo.clone()]);
        assert!(!app.composer.attachments[0].as_file);
        app.composer.attachments.clear();
        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('p'), M::NONE);
        assert_eq!(app.option_value(PHOTOS_AS_FILES_OPTION), "[x]");
        app.popup = None;
        app.attach_paths(vec![photo, doc]);
        assert_eq!(app.composer.attachments[0].kind, upload::Kind::Photo);
        assert!(app.composer.attachments[0].as_file);
        assert!(!app.composer.attachments[1].as_file);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_forwards_to_saved_messages_or_the_chosen_chat() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('S'), M::NONE);
        assert_eq!(
            logged(&log, "forward "),
            vec!["forward 10 [200] -> 1".to_owned()]
        );
        assert_eq!(
            app.status,
            Some(("saved to Saved Messages".to_owned(), false))
        );
        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('v'), M::NONE);
        assert_eq!(app.settings.chats.save_to_chat, 10);
        assert_eq!(app.option_value(SAVE_TARGET_OPTION), "chat 10");
        press(&mut app, K::Esc, M::NONE);
        app.keymap = Keymap::from_toml("[keymap]\nSaveMessage = \"w\"\n", &mut Vec::new()).unwrap();
        select(&mut app, 10, 300);
        press(&mut app, K::Char('w'), M::NONE);
        assert_eq!(
            logged(&log, "forward ").last().unwrap(),
            "forward 10 [300] -> 10"
        );
        app.popup = Some(Popup::Options { sel: 0 });
        press(&mut app, K::Char('v'), M::NONE);
        assert_eq!(app.settings.chats.save_to_chat, 0);
    }

    #[test]
    fn logging_out_clears_every_per_account_collection_and_the_image_cache() {
        let (mut app, _log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        app.connection = Some("Ready");
        app.folders.push((1, "Work".to_owned()));
        app.list_cache.insert(ListId::Main, vec![10]);
        app.loaded_lists.push(ListId::Main);
        app.last_viewed.insert(10, 400);
        app.composer.input.insert_str("draft");
        app.composer.reply_to = Some(400);
        app.popup = Some(Popup::ConfirmUrl("https://example.org".to_owned()));
        app.user_meta.insert(2, crate::model::UserMeta::default());
        app.typing.insert(10, Vec::new());
        app.online_sent = Some(true);
        app.typing_sent = Some((10, Instant::now()));
        app.bot_commands.insert(10, Vec::new());
        app.cmd_sel = 3;
        app.reply_kb_ids.insert(10, 1);
        app.reply_kb.insert(10, None);
        app.inbox.push_back(InboxItem {
            chat_id: 10,
            msg_id: 400,
            sender: Sender::User(2),
            preview: "hi".to_owned(),
            date: 0,
            mention: false,
            topic: None,
        });
        app.pending_jump = Some((10, 5));
        app.jump_inflight = Some((10, 5));
        app.unread_counts.insert(ListId::Main, 1);
        app.unread_unmuted.insert(ListId::Main, 1);
        app.unread_messages.insert(ListId::Main, (1, 1));
        app.scope_muted = [true; 3];
        app.pinned_msg.insert(10, msg(10, 400, 2));
        app.pending_open = Some(PendingOpen {
            chat_id: 10,
            last_read: 0,
            unread: 1,
        });
        app.groups.insert(10, crate::model::GroupMeta::default());
        app.secret_chats.insert(
            7,
            crate::model::SecretChat {
                id: 7,
                user_id: 2,
                state: crate::model::SecretState::Ready,
                outbound: true,
            },
        );
        app.topics.insert(10, crate::model::TopicList::default());
        app.expanded = Some(10);
        app.open_topic = Some(1);
        app.topic_select = Some(10);
        app.topic_unread = Some(10);
        app.archive.record_deleted(&msg(10, 400, 2), 1).unwrap();
        app.blocked.insert(Sender::User(2));
        app.blocked_loaded = true;
        app.revealed.insert((10, 400));
        let key = crate::media::Key {
            src: crate::media::Src::File(5),
            cols: 4,
            rows: 2,
            shape: None,
            animate: false,
        };
        app.media.insert_ready(
            key,
            crate::media::Frames {
                frames: Vec::new(),
                bytes: 10,
            },
            0,
        );
        app.media.on_file(FileRef {
            id: 5,
            ..FileRef::default()
        });
        app.media.want_download(6, 1);
        app.media.download_failed(8, "gone".to_owned());
        let before = app.per_account_leftovers();
        for name in [
            "connection",
            "my_id",
            "users",
            "chats",
            "folders",
            "list_cache",
            "loaded_lists",
            "open_chat",
            "histories",
            "last_viewed",
            "composer",
            "reply_to",
            "popup",
            "media",
            "user_meta",
            "typing",
            "online_sent",
            "typing_sent",
            "bot_commands",
            "cmd_sel",
            "reply_kb_ids",
            "reply_kb",
            "inbox",
            "pending_jump",
            "jump_inflight",
            "unread_counts",
            "unread_unmuted",
            "unread_messages",
            "scope_muted",
            "pinned_msg",
            "pending_open",
            "groups",
            "secret_chats",
            "topics",
            "expanded",
            "open_topic",
            "topic_select",
            "topic_unread",
            "archive",
            "blocked",
            "blocked_loaded",
            "revealed",
        ] {
            assert!(
                before.contains(&name),
                "{name} was not filled before the reset"
            );
        }
        app.reset_session();
        assert_eq!(app.per_account_leftovers(), Vec::<&str>::new());
        assert_eq!(app.media.stats(), (0, 0));
    }

    #[test]
    fn image_decoded_for_the_previous_session_is_dropped_after_reset() {
        let (mut app, _log) = fake_app(no_read());
        let key = crate::media::Key {
            src: crate::media::Src::File(5),
            cols: 4,
            rows: 2,
            shape: None,
            animate: false,
        };
        let input = crate::media::Input::Bytes(std::sync::Arc::from(&b"x"[..]));
        assert!(matches!(
            app.media.look(key, input.clone(), false),
            crate::media::Look::Pending
        ));
        app.reset_session();
        app.media.insert_pending(key);
        app.media.on_done(crate::media::Done {
            key,
            epoch: 0,
            result: Ok(crate::media::Frames {
                frames: Vec::new(),
                bytes: 10,
            }),
        });
        assert!(matches!(
            app.media.look(key, input, false),
            crate::media::Look::Pending
        ));
    }

    #[test]
    fn translate_is_not_offered_in_secret_or_protected_chats() {
        let (mut app, log) = fake_app(Settings::default());
        app.chats.insert(40, chat(40, ChatKind::Secret(7), false));
        let mut protected = chat(50, ChatKind::Supergroup, false);
        protected.protected = true;
        app.chats.insert(50, protected);
        open_with_history(&mut app, 10);
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        assert!(menu_keys(&app).contains('t'));
        app.popup = None;
        for id in [40, 50] {
            open_with_history(&mut app, id);
            select(&mut app, id, 200);
            press(&mut app, K::Enter, M::NONE);
            let keys = menu_keys(&app);
            assert!(keys.contains('Y'), "{keys}");
            assert!(!keys.contains('t'), "chat {id}: {keys}");
            press(&mut app, K::Char('t'), M::NONE);
            app.popup = None;
            app.message_action(MenuAction::Translate);
            assert!(!matches!(app.popup, Some(Popup::Translate { .. })));
            assert!(
                app.status
                    .as_ref()
                    .is_some_and(|(s, err)| *err && s.contains("translation is off"))
            );
        }
        assert!(logged(&log, "translate ").is_empty());
    }

    #[test]
    fn hidden_message_link_asks_before_opening() {
        let (mut app, _log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        let m = app.histories.get_mut(&10).unwrap().get_mut(200).unwrap();
        m.body = crate::model::RichText {
            text: "see docs".to_owned(),
            entities: vec![crate::model::Entity {
                offset: 4,
                length: 4,
                kind: crate::model::EntityKind::Link(Some("https://evil.example/x".to_owned())),
            }],
        };
        select(&mut app, 10, 200);
        press(&mut app, K::Enter, M::NONE);
        press(&mut app, K::Char('o'), M::NONE);
        assert!(matches!(&app.popup, Some(Popup::ConfirmUrl(u)) if u == "https://evil.example/x"));
        press(&mut app, K::Char('n'), M::NONE);
        assert!(app.popup.is_none());
    }

    fn unread_reactions(chat_id: i64, id: i64, n: usize, left: i32) -> Update {
        let one = serde_json::json!({
            "@type": "unreadReaction",
            "type": {"@type": "reactionTypeEmoji", "emoji": "❤"},
            "sender_id": {"@type": "messageSenderUser", "user_id": 2},
            "is_big": false,
        });
        update(serde_json::json!({
            "@type": "updateMessageUnreadReactions", "chat_id": chat_id, "message_id": id,
            "unread_reactions": vec![one; n], "unread_reaction_count": left,
        }))
    }

    fn forum_topic_counts(topic: i32, mentions: i32, reactions: i32) -> Update {
        update(serde_json::json!({
            "@type": "updateForumTopic", "chat_id": 40, "forum_topic_id": topic, "is_pinned": false,
            "last_read_inbox_message_id": 0, "last_read_outbox_message_id": 0,
            "unread_mention_count": mentions, "unread_reaction_count": reactions,
            "notification_settings": {
                "@type": "chatNotificationSettings", "use_default_mute_for": true, "mute_for": 0,
                "use_default_sound": true, "sound_id": "0", "use_default_show_preview": true,
                "show_preview": true, "use_default_mute_stories": true, "mute_stories": false,
                "use_default_story_sound": true, "story_sound_id": "0",
                "use_default_show_story_poster": true, "show_story_poster": true,
                "use_default_disable_pinned_message_notifications": true,
                "disable_pinned_message_notifications": false,
                "use_default_disable_mention_notifications": true,
                "disable_mention_notifications": false,
            },
        }))
    }

    #[test]
    fn unread_mention_and_reaction_counts_follow_tdlib_updates() {
        let (mut app, _log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        assert_eq!(app.unread_counts(10, None), (1, 0));
        app.on_update(update(serde_json::json!({
            "@type": "updateChatUnreadReactionCount", "chat_id": 10, "unread_reaction_count": 3,
        })));
        assert_eq!(app.unread_counts(10, None), (1, 3));
        app.on_update(unread_reactions(10, 200, 2, 4));
        assert!(app.histories[&10].get(200).unwrap().unread_reaction);
        assert_eq!(app.unread_counts(10, None), (1, 4));
        app.on_update(unread_reactions(10, 200, 0, 3));
        assert!(!app.histories[&10].get(200).unwrap().unread_reaction);
        assert_eq!(app.unread_counts(10, None), (1, 3));
        app.histories
            .get_mut(&10)
            .unwrap()
            .get_mut(100)
            .unwrap()
            .mention = true;
        app.on_update(update(serde_json::json!({
            "@type": "updateMessageMentionRead", "chat_id": 10, "message_id": 100,
            "unread_mention_count": 0,
        })));
        assert!(!app.histories[&10].get(100).unwrap().mention);
        assert_eq!(app.unread_counts(10, None), (0, 3));
        app.on_update(update(serde_json::json!({
            "@type": "updateChatUnreadMentionCount", "chat_id": 10, "unread_mention_count": 5,
        })));
        assert_eq!(app.unread_counts(10, None), (5, 3));

        let (mut app, _log) = forum_app(no_read());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.on_update(forum_topic_counts(5, 2, 6));
        assert_eq!(app.unread_counts(40, Some(5)), (2, 6));
        assert_eq!(
            app.unread_counts(40, None),
            (1, 0),
            "chat counters stay separate"
        );
        app.on_update(forum_topic_counts(5, 0, 0));
        assert_eq!(app.unread_counts(40, Some(5)), (0, 0));
    }

    #[test]
    fn next_reaction_prefers_the_open_chat_then_the_list_and_reads_what_it_shows() {
        let (mut app, log) = fake_app(Settings::default());
        open_with_history(&mut app, 10);
        set_unread(&mut app, 10, 0, 400);
        press(&mut app, K::Esc, M::NONE);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('r'), M::NONE);
        assert_eq!(count(&log, "find reaction"), 0);
        assert!(matches!(&app.status, Some((s, false)) if s == "no unread reactions"));
        app.chats.get_mut(&30).unwrap().reactions = 1;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('r'), M::NONE);
        assert_eq!(
            logged(&log, "unread search "),
            vec!["unread search 30 topic None"]
        );
        app.chats.get_mut(&10).unwrap().reactions = 2;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('r'), M::NONE);
        assert_eq!(
            logged(&log, "unread search ").last().unwrap(),
            "unread search 10 topic None",
            "the open chat goes first"
        );
        assert_eq!(count(&log, "find reaction"), 2);
        assert_eq!(count(&log, "find mention"), 0);

        app.histories
            .get_mut(&10)
            .unwrap()
            .get_mut(200)
            .unwrap()
            .unread_reaction = true;
        let views = logged(&log, "view 10").len();
        app.handle(Event::Reaction {
            chat_id: 10,
            result: Ok(Some((200, None))),
        });
        assert_eq!(app.selected_message().map(|m| m.id), Some(200));
        assert_eq!(
            logged(&log, "view 10")[views..],
            ["view 10 [200] topic None"]
        );
        assert!(!app.histories[&10].get(200).unwrap().unread_reaction);
        press(&mut app, K::Char('k'), M::NONE);
        press(&mut app, K::Char('j'), M::NONE);
        assert_eq!(
            logged(&log, "view 10").len(),
            views + 1,
            "a message is viewed for its reaction only once"
        );

        app.histories
            .get_mut(&10)
            .unwrap()
            .get_mut(100)
            .unwrap()
            .mention = true;
        app.handle(Event::Mention {
            chat_id: 10,
            result: Ok(Some((100, None))),
        });
        assert_eq!(
            logged(&log, "view 10").last().unwrap(),
            "view 10 [100] topic None"
        );
        assert!(!app.histories[&10].get(100).unwrap().mention);

        app.handle(Event::Reaction {
            chat_id: 10,
            result: Ok(None),
        });
        assert!(matches!(&app.status, Some((s, false)) if s.contains("no unread reactions")));
        app.handle(Event::Reaction {
            chat_id: 10,
            result: Err("boom".to_owned()),
        });
        assert!(matches!(&app.status, Some((s, true)) if s.contains("find reaction: boom")));
    }

    #[test]
    fn unread_jumps_search_the_open_topic_before_the_whole_forum() {
        let (mut app, log) = forum_app(Settings::default());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.enter_topic(40, 5);
        app.chats.get_mut(&40).unwrap().reactions = 3;
        app.on_update(forum_topic_counts(5, 1, 2));
        press(&mut app, K::Esc, M::NONE);
        app.focus = Pane::Messages;
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('r'), M::NONE);
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('m'), M::NONE);
        app.on_update(forum_topic_counts(5, 0, 0));
        press(&mut app, K::Char(']'), M::NONE);
        press(&mut app, K::Char('r'), M::NONE);
        assert_eq!(
            logged(&log, "unread search "),
            vec![
                "unread search 40 topic Some(5)",
                "unread search 40 topic Some(5)",
                "unread search 40 topic None",
            ]
        );
        assert_eq!(count(&log, "find reaction"), 2);
        assert_eq!(count(&log, "find mention"), 1);
    }

    #[test]
    fn privacy_modes_never_read_reactions_or_mentions_on_view() {
        for settings in [no_read(), ghost()] {
            let (mut app, log) = fake_app(settings);
            open_with_history(&mut app, 10);
            set_unread(&mut app, 10, 0, 400);
            app.chats.get_mut(&10).unwrap().reactions = 1;
            let h = app.histories.get_mut(&10).unwrap();
            h.get_mut(200).unwrap().unread_reaction = true;
            h.get_mut(100).unwrap().mention = true;
            press(&mut app, K::Esc, M::NONE);
            press(&mut app, K::Char(']'), M::NONE);
            press(&mut app, K::Char('r'), M::NONE);
            assert_eq!(count(&log, "find reaction"), 1, "jumping still works");
            app.handle(Event::Reaction {
                chat_id: 10,
                result: Ok(Some((200, None))),
            });
            app.handle(Event::Mention {
                chat_id: 10,
                result: Ok(Some((100, None))),
            });
            assert_eq!(app.selected_message().map(|m| m.id), Some(100));
            assert_eq!(count(&log, "mark read"), 0);
            assert!(logged(&log, "view ").is_empty());
            let h = &app.histories[&10];
            assert!(h.get(200).unwrap().unread_reaction && h.get(100).unwrap().mention);
            assert_eq!(app.unread_counts(10, None), (1, 1));
        }
    }

    #[test]
    fn menus_read_all_mentions_and_reactions_on_request_even_in_privacy_modes() {
        let (mut app, log) = fake_app(no_read());
        open_with_history(&mut app, 10);
        app.histories
            .get_mut(&10)
            .unwrap()
            .get_mut(200)
            .unwrap()
            .unread_reaction = true;
        app.chats.get_mut(&10).unwrap().reactions = 2;
        app.chats.get_mut(&20).unwrap().mentions = 0;
        app.focus = Pane::Chats;
        app.refresh_lists();
        app.select_row(Row::Chat(20));
        leader(&mut app, 'a');
        assert!(!menu_keys(&app).contains('R'), "nothing unread, no action");
        press(&mut app, K::Esc, M::NONE);
        app.select_row(Row::Chat(10));
        leader(&mut app, 'a');
        assert!(menu_keys(&app).contains('R'));
        press(&mut app, K::Char('R'), M::NONE);
        assert_eq!(
            logged(&log, "read all "),
            ["read all mentions 10", "read all reactions 10"]
        );
        assert_eq!(count(&log, "mark read"), 0);
        assert!(!app.histories[&10].get(200).unwrap().unread_reaction);
        assert!(matches!(&app.status, Some((s, false)) if s.contains("marked as read")));

        let (mut app, log) = forum_app(ghost());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.on_update(forum_topic_counts(5, 0, 4));
        app.focus = Pane::Chats;
        app.select_row(Row::Topic(40, 5));
        leader(&mut app, 'a');
        assert!(menu_keys(&app).contains('R'));
        press(&mut app, K::Char('R'), M::NONE);
        assert_eq!(logged(&log, "read topic "), ["read topic reactions 40 5"]);
        app.select_row(Row::Topic(40, 7));
        leader(&mut app, 'a');
        assert!(!menu_keys(&app).contains('R'));
    }

    #[test]
    fn mark_as_read_also_clears_reactions() {
        let (mut app, log) = fake_app(Settings::default());
        app.chats.get_mut(&20).unwrap().reactions = 1;
        app.chats.get_mut(&20).unwrap().mentions = 0;
        app.mark_chat_read(20);
        assert_eq!(logged(&log, "read all "), ["read all reactions 20"]);
        let (mut app, log) = forum_app(Settings::default());
        app.expand(40);
        feed_topics(&mut app, standard_topics(), None);
        app.on_update(forum_topic_counts(6, 0, 1));
        app.mark_chat_read(40);
        assert_eq!(logged(&log, "read topic "), ["read topic reactions 40 6"]);
    }
}
