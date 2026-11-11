use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub const fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        normalize(ev.code, ev.modifiers)
    }

    pub fn label(&self) -> String {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::BackTab => "S-Tab".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PgUp".to_owned(),
            KeyCode::PageDown => "PgDn".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        let mut prefix = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            prefix.push_str("C-");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            prefix.push_str("A-");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            prefix.push_str("S-");
        }
        if prefix.is_empty() {
            name
        } else {
            format!("<{prefix}{name}>")
        }
    }
}

const LAYOUT_RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";
const LAYOUT_EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const LAYOUT_RU_SHIFT: &str = "ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮЁ";
const LAYOUT_EN_SHIFT: &str = "QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>~";

pub fn latin_key(c: char) -> Option<char> {
    [(LAYOUT_RU, LAYOUT_EN), (LAYOUT_RU_SHIFT, LAYOUT_EN_SHIFT)]
        .into_iter()
        .find_map(|(ru, en)| en.chars().nth(ru.chars().position(|r| r == c)?))
}

fn normalize(code: KeyCode, mods: KeyModifiers) -> Chord {
    let mut mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    let code = match code {
        KeyCode::BackTab => {
            mods.remove(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => {
            mods.remove(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        KeyCode::Char(c) => {
            mods.remove(KeyModifiers::SHIFT);
            if mods.contains(KeyModifiers::CONTROL) {
                KeyCode::Char(c.to_ascii_lowercase())
            } else {
                KeyCode::Char(c)
            }
        }
        other => other,
    };
    Chord::new(code, mods)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    StartComposer,
    OpenPaneFilter,
    ClosePopup,
    FocusFolderPane,
    FocusChatPane,
    FocusMessagePane,
    FocusInfoPane,
    SelectNext,
    SelectPrevious,
    CycleFocusNext,
    CycleFocusPrevious,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    ScrollViewportDown,
    ScrollViewportUp,
    JumpTop,
    JumpBottom,
    Quit,
    ReplyMessage,
    DeleteMessage,
    EditMessage,
    OpenMessageUrl,
    PlayMedia,
    ViewAttachment,
    ToggleFolderPane,
    ToggleChatPane,
    ToggleInfoPane,
    OpenPaneActions,
    RefreshScreen,
    ChatSwitcher,
    LogOut,
    AttachFile,
    RecordVoice,
    OpenStickers,
    CopyMessage,
    ReactMessage,
    ForwardMessage,
    PinMessage,
    GoToReply,
    ShowMessageProfile,
    ShowReactionUsers,
    OpenProfile,
    ShowPinned,
    SearchMessages,
    OpenInbox,
    MarkChatRead,
    OpenBotKeyboard,
    OpenOptions,
    JumpToPinned,
    OpenNextUnread,
    JumpToNextMention,
    JumpToNextReaction,
    CopyQuote,
    TranslateMessage,
    SaveMessage,
    MarkMessage,
    SelectText,
    OpenSharedMedia,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Navigate,
    Compose,
    Message,
    Panes,
    Account,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Navigate => "Navigate",
            Group::Compose => "Compose",
            Group::Message => "Message",
            Group::Panes => "Panes",
            Group::Account => "Account",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub key: String,
    pub desc: String,
    pub action: Action,
}

impl Action {
    pub fn group(self) -> (Group, u8) {
        match self {
            Action::ChatSwitcher => (Group::Navigate, 0),
            Action::OpenPaneActions => (Group::Navigate, 1),
            Action::OpenPaneFilter => (Group::Navigate, 2),
            Action::FocusFolderPane => (Group::Navigate, 3),
            Action::FocusChatPane => (Group::Navigate, 4),
            Action::FocusMessagePane => (Group::Navigate, 5),
            Action::FocusInfoPane => (Group::Navigate, 6),
            Action::CycleFocusNext => (Group::Navigate, 7),
            Action::CycleFocusPrevious => (Group::Navigate, 8),
            Action::SelectNext => (Group::Navigate, 9),
            Action::SelectPrevious => (Group::Navigate, 10),
            Action::HalfPageDown => (Group::Navigate, 11),
            Action::HalfPageUp => (Group::Navigate, 12),
            Action::PageDown => (Group::Navigate, 26),
            Action::PageUp => (Group::Navigate, 27),
            Action::ScrollViewportDown => (Group::Navigate, 13),
            Action::ScrollViewportUp => (Group::Navigate, 14),
            Action::JumpTop => (Group::Navigate, 15),
            Action::JumpBottom => (Group::Navigate, 16),
            Action::ClosePopup => (Group::Navigate, 17),
            Action::StartComposer => (Group::Compose, 0),
            Action::AttachFile => (Group::Compose, 1),
            Action::RecordVoice => (Group::Compose, 2),
            Action::OpenStickers => (Group::Compose, 3),
            Action::ReplyMessage => (Group::Message, 0),
            Action::EditMessage => (Group::Message, 1),
            Action::PlayMedia => (Group::Message, 2),
            Action::ViewAttachment => (Group::Message, 3),
            Action::OpenMessageUrl => (Group::Message, 4),
            Action::DeleteMessage => (Group::Message, 5),
            Action::ToggleFolderPane => (Group::Panes, 0),
            Action::ToggleChatPane => (Group::Panes, 1),
            Action::ToggleInfoPane => (Group::Panes, 2),
            Action::RefreshScreen => (Group::Panes, 3),
            Action::Quit => (Group::Account, 1),
            Action::LogOut => (Group::Account, 2),
            Action::CopyMessage => (Group::Message, 6),
            Action::ReactMessage => (Group::Message, 7),
            Action::ForwardMessage => (Group::Message, 8),
            Action::PinMessage => (Group::Message, 9),
            Action::GoToReply => (Group::Message, 10),
            Action::ShowMessageProfile => (Group::Message, 11),
            Action::ShowReactionUsers => (Group::Message, 12),
            Action::OpenProfile => (Group::Navigate, 18),
            Action::ShowPinned => (Group::Navigate, 19),
            Action::SearchMessages => (Group::Navigate, 20),
            Action::OpenInbox => (Group::Navigate, 21),
            Action::MarkChatRead => (Group::Navigate, 22),
            Action::OpenBotKeyboard => (Group::Compose, 4),
            Action::OpenOptions => (Group::Account, 0),
            Action::JumpToPinned => (Group::Navigate, 23),
            Action::OpenNextUnread => (Group::Navigate, 24),
            Action::JumpToNextMention => (Group::Navigate, 25),
            Action::JumpToNextReaction => (Group::Navigate, 28),
            Action::CopyQuote => (Group::Message, 13),
            Action::TranslateMessage => (Group::Message, 14),
            Action::SaveMessage => (Group::Message, 15),
            Action::MarkMessage => (Group::Message, 16),
            Action::SelectText => (Group::Message, 17),
            Action::OpenSharedMedia => (Group::Navigate, 29),
        }
    }

    pub fn destructive(self) -> bool {
        matches!(self, Action::LogOut | Action::DeleteMessage)
    }

    pub const ALL: [Action; 60] = [
        Action::StartComposer,
        Action::OpenPaneFilter,
        Action::ClosePopup,
        Action::FocusFolderPane,
        Action::FocusChatPane,
        Action::FocusMessagePane,
        Action::FocusInfoPane,
        Action::SelectNext,
        Action::SelectPrevious,
        Action::CycleFocusNext,
        Action::CycleFocusPrevious,
        Action::HalfPageDown,
        Action::HalfPageUp,
        Action::PageDown,
        Action::PageUp,
        Action::ScrollViewportDown,
        Action::ScrollViewportUp,
        Action::JumpTop,
        Action::JumpBottom,
        Action::Quit,
        Action::ReplyMessage,
        Action::DeleteMessage,
        Action::EditMessage,
        Action::OpenMessageUrl,
        Action::PlayMedia,
        Action::ViewAttachment,
        Action::ToggleFolderPane,
        Action::ToggleChatPane,
        Action::ToggleInfoPane,
        Action::OpenPaneActions,
        Action::RefreshScreen,
        Action::ChatSwitcher,
        Action::LogOut,
        Action::AttachFile,
        Action::RecordVoice,
        Action::OpenStickers,
        Action::CopyMessage,
        Action::ReactMessage,
        Action::ForwardMessage,
        Action::PinMessage,
        Action::GoToReply,
        Action::ShowMessageProfile,
        Action::ShowReactionUsers,
        Action::OpenProfile,
        Action::ShowPinned,
        Action::SearchMessages,
        Action::OpenInbox,
        Action::MarkChatRead,
        Action::OpenBotKeyboard,
        Action::OpenOptions,
        Action::JumpToPinned,
        Action::OpenNextUnread,
        Action::JumpToNextMention,
        Action::JumpToNextReaction,
        Action::CopyQuote,
        Action::TranslateMessage,
        Action::SaveMessage,
        Action::MarkMessage,
        Action::SelectText,
        Action::OpenSharedMedia,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Action::StartComposer => "StartComposer",
            Action::OpenPaneFilter => "OpenPaneFilter",
            Action::ClosePopup => "ClosePopup",
            Action::FocusFolderPane => "FocusFolderPane",
            Action::FocusChatPane => "FocusChatPane",
            Action::FocusMessagePane => "FocusMessagePane",
            Action::FocusInfoPane => "FocusInfoPane",
            Action::SelectNext => "SelectNext",
            Action::SelectPrevious => "SelectPrevious",
            Action::CycleFocusNext => "CycleFocusNext",
            Action::CycleFocusPrevious => "CycleFocusPrevious",
            Action::HalfPageDown => "HalfPageDown",
            Action::HalfPageUp => "HalfPageUp",
            Action::PageDown => "PageDown",
            Action::PageUp => "PageUp",
            Action::ScrollViewportDown => "ScrollViewportDown",
            Action::ScrollViewportUp => "ScrollViewportUp",
            Action::JumpTop => "JumpTop",
            Action::JumpBottom => "JumpBottom",
            Action::Quit => "Quit",
            Action::ReplyMessage => "ReplyMessage",
            Action::DeleteMessage => "DeleteMessage",
            Action::EditMessage => "EditMessage",
            Action::OpenMessageUrl => "OpenMessageUrl",
            Action::PlayMedia => "PlayMedia",
            Action::ViewAttachment => "ViewMessageAttachment",
            Action::ToggleFolderPane => "ToggleFolderPane",
            Action::ToggleChatPane => "ToggleChatPane",
            Action::ToggleInfoPane => "ToggleInfoPane",
            Action::OpenPaneActions => "OpenPaneActions",
            Action::RefreshScreen => "RefreshScreen",
            Action::ChatSwitcher => "ChatSwitcher",
            Action::LogOut => "LogOut",
            Action::AttachFile => "AttachFile",
            Action::RecordVoice => "RecordVoiceMessage",
            Action::OpenStickers => "OpenStickerPicker",
            Action::CopyMessage => "CopyMessage",
            Action::ReactMessage => "ReactMessage",
            Action::ForwardMessage => "ForwardMessage",
            Action::PinMessage => "PinMessage",
            Action::GoToReply => "GoToReferencedMessage",
            Action::ShowMessageProfile => "ShowMessageProfile",
            Action::ShowReactionUsers => "ShowReactionUsers",
            Action::OpenProfile => "OpenProfile",
            Action::ShowPinned => "ShowPinnedMessages",
            Action::SearchMessages => "SearchMessages",
            Action::OpenInbox => "OpenNotificationInbox",
            Action::MarkChatRead => "MarkChatRead",
            Action::OpenBotKeyboard => "OpenBotKeyboard",
            Action::OpenOptions => "OpenOptions",
            Action::JumpToPinned => "JumpToPinnedMessage",
            Action::OpenNextUnread => "OpenNextUnread",
            Action::JumpToNextMention => "JumpToNextMention",
            Action::JumpToNextReaction => "JumpToNextReaction",
            Action::CopyQuote => "CopyQuote",
            Action::TranslateMessage => "TranslateMessage",
            Action::SaveMessage => "SaveMessage",
            Action::MarkMessage => "MarkMessage",
            Action::SelectText => "SelectMessageText",
            Action::OpenSharedMedia => "OpenSharedMedia",
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::StartComposer => "compose",
            Action::OpenPaneFilter => "filter",
            Action::ClosePopup => "close popup",
            Action::FocusFolderPane => "folders",
            Action::FocusChatPane => "chats",
            Action::FocusMessagePane => "messages",
            Action::FocusInfoPane => "chat info",
            Action::SelectNext => "next",
            Action::SelectPrevious => "previous",
            Action::CycleFocusNext => "next pane",
            Action::CycleFocusPrevious => "previous pane",
            Action::HalfPageDown => "half page down",
            Action::HalfPageUp => "half page up",
            Action::PageDown => "page down",
            Action::PageUp => "page up",
            Action::ScrollViewportDown => "scroll down",
            Action::ScrollViewportUp => "scroll up",
            Action::JumpTop => "top",
            Action::JumpBottom => "bottom",
            Action::Quit => "quit",
            Action::ReplyMessage => "reply",
            Action::DeleteMessage => "delete",
            Action::EditMessage => "edit",
            Action::OpenMessageUrl => "open url",
            Action::PlayMedia => "play / open media",
            Action::ViewAttachment => "view attachment",
            Action::ToggleFolderPane => "toggle folders",
            Action::ToggleChatPane => "toggle chats",
            Action::ToggleInfoPane => "toggle chat info",
            Action::OpenPaneActions => "actions",
            Action::RefreshScreen => "redraw",
            Action::ChatSwitcher => "find chat",
            Action::LogOut => "log out",
            Action::AttachFile => "attach file",
            Action::RecordVoice => "record voice message",
            Action::OpenStickers => "stickers & GIFs",
            Action::CopyMessage => "copy text",
            Action::ReactMessage => "react",
            Action::ForwardMessage => "forward",
            Action::PinMessage => "pin / unpin",
            Action::GoToReply => "go to replied message",
            Action::ShowMessageProfile => "sender profile",
            Action::ShowReactionUsers => "who reacted",
            Action::OpenProfile => "profile",
            Action::ShowPinned => "pinned messages",
            Action::SearchMessages => "search all chats",
            Action::OpenInbox => "notification inbox",
            Action::MarkChatRead => "mark chat read",
            Action::OpenBotKeyboard => "bot buttons",
            Action::OpenOptions => "options",
            Action::JumpToPinned => "jump to pinned message",
            Action::OpenNextUnread => "next unread chat",
            Action::JumpToNextMention => "next unread mention or reply",
            Action::JumpToNextReaction => "next unread reaction",
            Action::CopyQuote => "copy as quote",
            Action::TranslateMessage => "translate message",
            Action::SaveMessage => "save to Saved Messages",
            Action::MarkMessage => "mark / unmark message",
            Action::SelectText => "select text to copy",
            Action::OpenSharedMedia => "shared media",
        }
    }

    fn default_keys(self) -> &'static [&'static str] {
        match self {
            Action::StartComposer => &["i"],
            Action::OpenPaneFilter => &["/"],
            Action::ClosePopup => &["q"],
            Action::FocusFolderPane => &["1"],
            Action::FocusChatPane => &["2"],
            Action::FocusMessagePane => &["3"],
            Action::FocusInfoPane => &["4"],
            Action::SelectNext => &["j", "down"],
            Action::SelectPrevious => &["k", "up"],
            Action::CycleFocusNext => &["tab", "l", "right"],
            Action::CycleFocusPrevious => &["<S-tab>", "h", "left"],
            Action::HalfPageDown => &["<C-d>"],
            Action::HalfPageUp => &["<C-u>"],
            Action::PageDown => &["pagedown"],
            Action::PageUp => &["pageup"],
            Action::ScrollViewportDown => &["J"],
            Action::ScrollViewportUp => &["K"],
            Action::JumpTop => &["gg", "home"],
            Action::JumpBottom => &["G", "end"],
            Action::Quit => &["q"],
            Action::ReplyMessage => &["R"],
            Action::DeleteMessage => &["d"],
            Action::EditMessage => &["e"],
            Action::OpenMessageUrl => &["o"],
            Action::PlayMedia => &["x"],
            Action::ViewAttachment => &["v"],
            Action::ToggleFolderPane => &["<leader>1"],
            Action::ToggleChatPane => &["<leader>2"],
            Action::ToggleInfoPane => &["<leader>4"],
            Action::OpenPaneActions => &["<leader>a"],
            Action::RefreshScreen => &["<leader>r"],
            Action::ChatSwitcher => &["<leader><leader>"],
            Action::LogOut => &["<leader>L"],
            Action::AttachFile => &["<leader>u"],
            Action::RecordVoice => &["<leader>v"],
            Action::OpenStickers => &["<leader>s"],
            Action::CopyMessage => &["y"],
            Action::ReactMessage => &["r"],
            Action::ForwardMessage => &["f"],
            Action::PinMessage => &["P"],
            Action::GoToReply => &[],
            Action::ShowMessageProfile => &[],
            Action::ShowReactionUsers => &[],
            Action::OpenProfile => &["<leader>p"],
            Action::ShowPinned => &["<leader>P"],
            Action::SearchMessages => &["<leader>/"],
            Action::OpenInbox => &["<leader>n"],
            Action::MarkChatRead => &["<leader>m"],
            Action::OpenBotKeyboard => &["<leader>k"],
            Action::OpenOptions => &["<leader>o"],
            Action::JumpToPinned => &["gp"],
            Action::OpenNextUnread => &["]u"],
            Action::JumpToNextMention => &["]m"],
            Action::JumpToNextReaction => &["]r"],
            Action::CopyQuote | Action::TranslateMessage | Action::SaveMessage => &[],
            Action::MarkMessage => &["m"],
            Action::SelectText => &["V"],
            Action::OpenSharedMedia => &["<leader>g"],
        }
    }
}

fn named_key(name: &str) -> Option<KeyCode> {
    Some(match name.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "enter" | "cr" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "lt" => KeyCode::Char('<'),
        _ => {
            let rest = name.strip_prefix(['f', 'F'])?;
            KeyCode::F(rest.parse().ok().filter(|n| (1..=24).contains(n))?)
        }
    })
}

fn parse_angle(inner: &str) -> Result<Chord, String> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = inner;
    loop {
        let lower = rest.get(..2).map(str::to_ascii_lowercase);
        match lower.as_deref() {
            Some("c-") => mods |= KeyModifiers::CONTROL,
            Some("a-") | Some("m-") => mods |= KeyModifiers::ALT,
            Some("s-") => mods |= KeyModifiers::SHIFT,
            _ => break,
        }
        rest = &rest[2..];
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => named_key(rest).ok_or_else(|| format!("unknown key <{inner}>"))?,
    };
    Ok(normalize(code, mods))
}

pub fn parse_sequence(spec: &str, leader: Chord) -> Result<Vec<Chord>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty key sequence".to_owned());
    }
    if spec.chars().count() > 1
        && !spec.contains('<')
        && let Some(code) = named_key(spec)
    {
        return Ok(vec![normalize(code, KeyModifiers::NONE)]);
    }
    let mut out = Vec::new();
    let mut rest = spec;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && end > 1
        {
            let inner = &rest[1..end];
            if inner.eq_ignore_ascii_case("leader") {
                out.push(leader);
            } else {
                out.push(parse_angle(inner)?);
            }
            rest = &rest[end + 1..];
            continue;
        }
        out.push(normalize(KeyCode::Char(c), KeyModifiers::NONE));
        rest = &rest[c.len_utf8()..];
    }
    Ok(out)
}

const RESERVED: [Chord; 7] = [
    Chord::new(KeyCode::Enter, KeyModifiers::NONE),
    Chord::new(KeyCode::Esc, KeyModifiers::NONE),
    Chord::new(KeyCode::Backspace, KeyModifiers::NONE),
    Chord::new(KeyCode::Delete, KeyModifiers::NONE),
    Chord::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    Chord::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
    Chord::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
];

#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    Action(Action),
    Pending,
    Unbound,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    pub leader: Chord,
    bindings: Vec<(Vec<Chord>, Action)>,
}

impl Default for Keymap {
    fn default() -> Self {
        let leader = Chord::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let mut map = Keymap {
            leader,
            bindings: Vec::new(),
        };
        for action in Action::ALL {
            for spec in action.default_keys() {
                let seq = parse_sequence(spec, leader).expect("default keymap parses");
                map.bindings.push((seq, action));
            }
        }
        map
    }
}

impl Keymap {
    pub fn from_toml(raw: &str, warnings: &mut Vec<String>) -> Result<Keymap, String> {
        let root: toml::Table = toml::from_str(raw).map_err(|e| e.message().to_owned())?;
        let mut map = Keymap::default();
        let Some(table) = root.get("keymap") else {
            return Ok(map);
        };
        let table = table
            .as_table()
            .ok_or_else(|| "[keymap] must be a table".to_owned())?;
        if let Some(leader) = table.get("leader") {
            match leader.as_str().map(|s| parse_sequence(s, map.leader)) {
                Some(Ok(seq)) if seq.len() == 1 && !RESERVED.contains(&seq[0]) => {
                    let old = map.leader;
                    map.leader = seq[0];
                    for (keys, _) in &mut map.bindings {
                        for chord in keys.iter_mut() {
                            if *chord == old {
                                *chord = seq[0];
                            }
                        }
                    }
                }
                _ => warnings.push("keymap.leader: expected a single non-reserved key".to_owned()),
            }
        }
        for (name, value) in table {
            if name == "leader"
                || value.is_table() && !value.as_table().is_some_and(|t| t.contains_key("keys"))
            {
                continue;
            }
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("keymap.{name}: unknown action"));
                continue;
            };
            let specs: Vec<String> = match value {
                toml::Value::Boolean(false) => Vec::new(),
                toml::Value::String(s) if s.is_empty() => Vec::new(),
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Table(t) => match t.get("keys") {
                    Some(toml::Value::String(s)) => vec![s.clone()],
                    Some(toml::Value::Array(items)) => items
                        .iter()
                        .filter_map(|i| i.as_str().map(str::to_owned))
                        .collect(),
                    _ => {
                        warnings.push(format!("keymap.{name}: keys must be a string or list"));
                        continue;
                    }
                },
                _ => {
                    warnings.push(format!("keymap.{name}: expected string, table or false"));
                    continue;
                }
            };
            let mut parsed = Vec::new();
            for spec in &specs {
                match parse_sequence(spec, map.leader) {
                    Ok(seq) if RESERVED.contains(&seq[0]) => {
                        warnings.push(format!("keymap.{name}: {spec} uses a reserved key"))
                    }
                    Ok(seq) => parsed.push(seq),
                    Err(e) => warnings.push(format!("keymap.{name}: {e}")),
                }
            }
            map.bindings.retain(|(_, a)| *a != action);
            for seq in parsed {
                map.bindings.push((seq, action));
            }
        }
        Ok(map)
    }

    pub fn feed(&self, pending: &[Chord], chord: Chord) -> Feed {
        let mut seq = pending.to_vec();
        seq.push(chord);
        let mut prefix = false;
        for (keys, action) in &self.bindings {
            if *keys == seq {
                return Feed::Action(*action);
            }
            if keys.len() > seq.len() && keys[..seq.len()] == seq[..] {
                prefix = true;
            }
        }
        if prefix { Feed::Pending } else { Feed::Unbound }
    }

    pub fn layout_fallback(&self, pending: &[Chord], chord: Chord) -> Chord {
        let KeyCode::Char(c) = chord.code else {
            return chord;
        };
        match latin_key(c) {
            Some(l) if self.feed(pending, chord) == Feed::Unbound => {
                Chord::new(KeyCode::Char(l), chord.mods)
            }
            _ => chord,
        }
    }

    pub fn actions_for(&self, chord: Chord) -> Vec<Action> {
        self.bindings
            .iter()
            .filter(|(k, _)| k.len() == 1 && k[0] == chord)
            .map(|(_, a)| *a)
            .collect()
    }

    pub fn continuations(&self, pending: &[Chord]) -> Vec<Hint> {
        let mut out: Vec<Hint> = Vec::new();
        for (keys, action) in &self.bindings {
            if keys.len() > pending.len() && keys[..pending.len()] == pending[..] {
                let rest = &keys[pending.len()..];
                let label: String = rest.iter().map(Chord::label).collect();
                let desc = if rest.len() == 1 {
                    action.description().to_owned()
                } else {
                    format!("+{}", action.description())
                };
                if !out.iter().any(|h| h.key == label) {
                    out.push(Hint {
                        key: label,
                        desc,
                        action: *action,
                    });
                }
            }
        }
        out
    }

    pub fn grouped_continuations(&self, pending: &[Chord]) -> Vec<(Group, Vec<Hint>)> {
        let mut hints = self.continuations(pending);
        hints.sort_by_key(|h| h.action.group());
        let mut out: Vec<(Group, Vec<Hint>)> = Vec::new();
        for h in hints {
            let g = h.action.group().0;
            match out.last_mut() {
                Some((last, items)) if *last == g => items.push(h),
                _ => out.push((g, vec![h])),
            }
        }
        out
    }

    pub fn label_for(&self, action: Action) -> Option<String> {
        self.bindings
            .iter()
            .find(|(_, a)| *a == action)
            .map(|(keys, _)| {
                keys.iter()
                    .map(|c| {
                        if *c == self.leader {
                            "<leader>".to_owned()
                        } else {
                            c.label()
                        }
                    })
                    .collect()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> Chord {
        Chord::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn space() -> Chord {
        key(' ')
    }

    #[test]
    fn parses_plain_and_compact_sequences() {
        assert_eq!(parse_sequence("i", space()).unwrap(), vec![key('i')]);
        assert_eq!(
            parse_sequence("gg", space()).unwrap(),
            vec![key('g'), key('g')]
        );
        assert_eq!(parse_sequence("G", space()).unwrap(), vec![key('G')]);
    }

    #[test]
    fn parses_named_keys_and_modifiers() {
        assert_eq!(
            parse_sequence("tab", space()).unwrap(),
            vec![Chord::new(KeyCode::Tab, KeyModifiers::NONE)]
        );
        assert_eq!(
            parse_sequence("<S-tab>", space()).unwrap(),
            vec![Chord::new(KeyCode::BackTab, KeyModifiers::NONE)]
        );
        assert_eq!(
            parse_sequence("<C-d>", space()).unwrap(),
            vec![Chord::new(KeyCode::Char('d'), KeyModifiers::CONTROL)]
        );
        assert_eq!(
            parse_sequence("<A-left>", space()).unwrap(),
            vec![Chord::new(KeyCode::Left, KeyModifiers::ALT)]
        );
        assert!(parse_sequence("<C-nope>", space()).is_err());
        assert!(parse_sequence("", space()).is_err());
    }

    #[test]
    fn parses_leader_sequences() {
        assert_eq!(
            parse_sequence("<leader><leader>", space()).unwrap(),
            vec![space(), space()]
        );
        assert_eq!(
            parse_sequence("<leader>1", space()).unwrap(),
            vec![space(), key('1')]
        );
    }

    #[test]
    fn shift_is_folded_into_chars() {
        let ev = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(Chord::from_event(&ev), key('G'));
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            Chord::from_event(&ev),
            Chord::new(KeyCode::BackTab, KeyModifiers::NONE)
        );
    }

    #[test]
    fn bracket_sequences_reach_unread_and_mentions() {
        let map = Keymap::default();
        assert_eq!(map.feed(&[], key(']')), Feed::Pending);
        assert_eq!(
            map.feed(&[key(']')], key('u')),
            Feed::Action(Action::OpenNextUnread)
        );
        assert_eq!(
            map.feed(&[key(']')], key('m')),
            Feed::Action(Action::JumpToNextMention)
        );
        assert_eq!(
            map.feed(&[key(']')], key('r')),
            Feed::Action(Action::JumpToNextReaction)
        );
        assert_eq!(map.label_for(Action::OpenNextUnread).as_deref(), Some("]u"));
        assert_eq!(
            map.label_for(Action::JumpToNextReaction).as_deref(),
            Some("]r")
        );
    }

    #[test]
    fn feed_resolves_sequences() {
        let map = Keymap::default();
        assert_eq!(map.feed(&[], key('g')), Feed::Pending);
        assert_eq!(
            map.feed(&[key('g')], key('g')),
            Feed::Action(Action::JumpTop)
        );
        assert_eq!(map.feed(&[], key('G')), Feed::Action(Action::JumpBottom));
        assert_eq!(map.feed(&[], space()), Feed::Pending);
        assert_eq!(
            map.feed(&[space()], space()),
            Feed::Action(Action::ChatSwitcher)
        );
        assert_eq!(
            map.feed(&[space()], key('4')),
            Feed::Action(Action::ToggleInfoPane)
        );
        assert_eq!(map.feed(&[], key('z')), Feed::Unbound);
        assert_eq!(
            map.feed(&[], Chord::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Feed::Action(Action::HalfPageDown)
        );
    }

    #[test]
    fn q_is_bound_to_close_and_quit() {
        let map = Keymap::default();
        let actions = map.actions_for(key('q'));
        assert!(actions.contains(&Action::ClosePopup));
        assert!(actions.contains(&Action::Quit));
    }

    #[test]
    fn toml_overrides_and_disables() {
        let raw = r#"
[keymap]
StartComposer = { keys = ["c", "<leader>e"] }
DeleteMessage = false
ReplyMessage = "<C-c>"
Bogus = "x"
"#;
        let mut warnings = Vec::new();
        let map = Keymap::from_toml(raw, &mut warnings).unwrap();
        assert_eq!(map.feed(&[], key('c')), Feed::Action(Action::StartComposer));
        assert_eq!(
            map.feed(&[space()], key('e')),
            Feed::Action(Action::StartComposer)
        );
        assert_eq!(map.feed(&[], key('i')), Feed::Unbound);
        assert_eq!(map.feed(&[], key('d')), Feed::Unbound);
        assert_eq!(map.feed(&[], key('R')), Feed::Unbound);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn custom_leader_rewrites_defaults() {
        let raw = "[keymap]\nleader = \",\"\n";
        let mut warnings = Vec::new();
        let map = Keymap::from_toml(raw, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            map.feed(&[key(',')], key(',')),
            Feed::Action(Action::ChatSwitcher)
        );
        assert_eq!(map.feed(&[], space()), Feed::Unbound);
    }

    #[test]
    fn leader_l_logs_out() {
        let map = Keymap::default();
        assert_eq!(map.feed(&[space()], key('L')), Feed::Action(Action::LogOut));
        assert!(
            map.continuations(&[space()])
                .iter()
                .any(|h| h.key == "L" && h.desc == "log out")
        );
    }

    #[test]
    fn continuations_list_leader_menu() {
        let map = Keymap::default();
        let items = map.continuations(&[space()]);
        assert!(
            items
                .iter()
                .any(|h| h.key == "1" && h.desc == "toggle folders")
        );
        assert!(
            items
                .iter()
                .any(|h| h.key == "Space" && h.desc == "find chat")
        );
        assert_eq!(
            map.label_for(Action::ChatSwitcher).unwrap(),
            "<leader><leader>"
        );
    }

    #[test]
    fn leader_hints_are_grouped_in_order_with_log_out_last() {
        let map = Keymap::default();
        let groups = map.grouped_continuations(&[space()]);
        let titles: Vec<&str> = groups.iter().map(|(g, _)| g.title()).collect();
        assert_eq!(titles, ["Navigate", "Compose", "Panes", "Account"]);
        let keys: Vec<Vec<&str>> = groups
            .iter()
            .map(|(_, h)| h.iter().map(|h| h.key.as_str()).collect())
            .collect();
        assert_eq!(
            keys,
            [
                vec!["Space", "a", "p", "P", "/", "n", "m", "g"],
                vec!["u", "v", "s", "k"],
                vec!["1", "2", "4", "r"],
                vec!["o", "L"]
            ]
        );
        assert!(
            groups
                .last()
                .unwrap()
                .1
                .last()
                .unwrap()
                .action
                .destructive()
        );
        let total: usize = groups.iter().map(|(_, h)| h.len()).sum();
        assert_eq!(total, map.continuations(&[space()]).len());
    }
}
