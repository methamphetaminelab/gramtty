use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::Path;

use tdlib_rs::enums::{self, ChatList, MessageContent, MessageSender, TextEntityType, Update};
use tdlib_rs::types;
use tokio::sync::mpsc::UnboundedSender;

use crate::format::duration;
use crate::model::{
    ChatInfo, ChatKind, Entity, EntityKind, ListId, Msg, Position, RichText, SendState, Sender,
};

unsafe extern "C" {
    fn td_execute(request: *const c_char) -> *const c_char;
}

pub enum Event {
    Input(crossterm::event::Event),
    Update(i32, Box<Update>),
    History {
        chat_id: i64,
        msgs: Vec<Msg>,
        initial: bool,
    },
    EditText {
        chat_id: i64,
        message_id: i64,
        text: String,
    },
    AuthError(String),
    Error(String),
}

fn execute(request: &serde_json::Value) -> Result<(), String> {
    let raw = CString::new(request.to_string()).map_err(|e| e.to_string())?;
    let reply = unsafe {
        let ptr = td_execute(raw.as_ptr());
        if ptr.is_null() {
            return Err("td_execute returned null".to_owned());
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    let reply: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    if reply["@type"] == "error" {
        return Err(reply["message"]
            .as_str()
            .unwrap_or("unknown error")
            .to_owned());
    }
    Ok(())
}

pub fn configure_logging(log_file: &Path) -> Result<(), String> {
    execute(&serde_json::json!({
        "@type": "setLogVerbosityLevel",
        "new_verbosity_level": 1,
    }))?;
    execute(&serde_json::json!({
        "@type": "setLogStream",
        "log_stream": {
            "@type": "logStreamFile",
            "path": log_file.to_string_lossy(),
            "max_file_size": 10_485_760,
            "redirect_stderr": false,
        },
    }))
    .map_err(|e| format!("cannot set TDLib log file {}: {e}", log_file.display()))
}

pub fn spawn_receiver(tx: UnboundedSender<Event>) {
    std::thread::Builder::new()
        .name("tdlib-receive".to_owned())
        .spawn(move || {
            while !tx.is_closed() {
                if let Some((update, client_id)) = tdlib_rs::receive()
                    && tx.send(Event::Update(client_id, Box::new(update))).is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn TDLib receive thread");
}

pub fn friendly_error(err: &types::Error) -> String {
    let hint = match err.message.as_str() {
        "PHONE_NUMBER_INVALID" => {
            "invalid phone number, use international format like +79991234567"
        }
        "PHONE_NUMBER_BANNED" => "this phone number is banned by Telegram",
        "PHONE_CODE_INVALID" => "wrong code",
        "PHONE_CODE_EXPIRED" => "the code expired, restart login",
        "PASSWORD_HASH_INVALID" => "wrong password",
        "API_ID_INVALID" | "API_ID_PUBLISHED_FLOOD" => "api_id/api_hash rejected by Telegram",
        "AUTH_KEY_UNREGISTERED" => "session was terminated, log in again",
        "MESSAGE_NOT_MODIFIED" => "message not modified",
        "MESSAGE_ID_INVALID" => "message can't be changed",
        m if m.starts_with("Too Many Requests") || m.starts_with("FLOOD_WAIT") => {
            return format!("too many attempts, wait and retry ({m})");
        }
        _ => return format!("{} ({})", err.message, err.code),
    };
    hint.to_owned()
}

#[derive(Clone)]
pub struct Td {
    pub client_id: i32,
    tx: UnboundedSender<Event>,
}

impl Td {
    pub fn new(tx: UnboundedSender<Event>) -> Self {
        Self {
            client_id: tdlib_rs::create_client(),
            tx,
        }
    }

    pub fn recreate(&mut self) {
        self.client_id = tdlib_rs::create_client();
    }

    fn spawn<F>(&self, context: &'static str, auth: bool, fut: F)
    where
        F: Future<Output = Result<(), types::Error>> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(e) = fut.await {
                let text = friendly_error(&e);
                let ev = if auth {
                    Event::AuthError(text)
                } else {
                    Event::Error(format!("{context}: {text}"))
                };
                let _ = tx.send(ev);
            }
        });
    }

    pub fn start(&self) {
        let c = self.client_id;
        self.spawn("get option", false, async move {
            tdlib_rs::functions::get_option("version".to_owned(), c)
                .await
                .map(|_| ())
        });
    }

    pub fn set_parameters(&self, db_dir: String, files_dir: String, api_id: i32, api_hash: String) {
        let c = self.client_id;
        self.spawn("tdlib parameters", true, async move {
            tdlib_rs::functions::set_tdlib_parameters(
                false,
                db_dir,
                files_dir,
                String::new(),
                true,
                true,
                true,
                true,
                api_id,
                api_hash,
                "en".to_owned(),
                crate::config::APP_NAME.to_owned(),
                String::new(),
                env!("CARGO_PKG_VERSION").to_owned(),
                c,
            )
            .await
        });
    }

    pub fn request_qr(&self) {
        let c = self.client_id;
        self.spawn("qr", true, async move {
            tdlib_rs::functions::request_qr_code_authentication(Vec::new(), c).await
        });
    }

    pub fn submit_phone(&self, phone: String) {
        let c = self.client_id;
        self.spawn("phone", true, async move {
            tdlib_rs::functions::set_authentication_phone_number(phone, None, c).await
        });
    }

    pub fn submit_code(&self, code: String) {
        let c = self.client_id;
        self.spawn("code", true, async move {
            tdlib_rs::functions::check_authentication_code(code, c).await
        });
    }

    pub fn submit_password(&self, password: String) {
        let c = self.client_id;
        self.spawn("password", true, async move {
            tdlib_rs::functions::check_authentication_password(password, c).await
        });
    }

    pub fn submit_email(&self, email: String) {
        let c = self.client_id;
        self.spawn("email", true, async move {
            tdlib_rs::functions::set_authentication_email_address(email, c).await
        });
    }

    pub fn submit_email_code(&self, code: String) {
        let c = self.client_id;
        self.spawn("email code", true, async move {
            tdlib_rs::functions::check_authentication_email_code(
                enums::EmailAddressAuthentication::Code(types::EmailAddressAuthenticationCode {
                    code,
                }),
                c,
            )
            .await
        });
    }

    pub fn log_out(&self) {
        let c = self.client_id;
        self.spawn("log out", true, async move {
            tdlib_rs::functions::log_out(c).await
        });
    }

    pub fn close(&self) {
        let c = self.client_id;
        self.spawn("close", false, async move {
            tdlib_rs::functions::close(c).await
        });
    }

    pub fn load_chats(&self, list: ListId) {
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(e) = tdlib_rs::functions::load_chats(Some(to_chat_list(list)), 100, c).await
                && e.code != 404
            {
                let _ = tx.send(Event::Error(format!("load chats: {}", friendly_error(&e))));
            }
        });
    }

    pub fn open_chat(&self, chat_id: i64) {
        let c = self.client_id;
        self.spawn("open chat", false, async move {
            tdlib_rs::functions::open_chat(chat_id, c).await
        });
    }

    pub fn close_chat(&self, chat_id: i64) {
        let c = self.client_id;
        self.spawn("close chat", false, async move {
            tdlib_rs::functions::close_chat(chat_id, c).await
        });
    }

    pub fn load_history(&self, chat_id: i64, from_message_id: i64, initial: bool) {
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result =
                tdlib_rs::functions::get_chat_history(chat_id, from_message_id, 0, 50, false, c)
                    .await;
            let ev = match result {
                Ok(enums::Messages::Messages(m)) => Event::History {
                    chat_id,
                    msgs: m
                        .messages
                        .into_iter()
                        .flatten()
                        .map(convert_message)
                        .collect(),
                    initial,
                },
                Err(e) => Event::Error(format!("load history: {}", friendly_error(&e))),
            };
            let _ = tx.send(ev);
        });
    }

    pub fn view_messages(&self, chat_id: i64, ids: Vec<i64>) {
        let c = self.client_id;
        self.spawn("mark read", false, async move {
            tdlib_rs::functions::view_messages(chat_id, ids, None, true, c).await
        });
    }

    pub fn send_text(&self, chat_id: i64, text: String, reply_to: Option<i64>, edit: Option<i64>) {
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = async {
                let enums::FormattedText::FormattedText(formatted) =
                    tdlib_rs::functions::parse_markdown(
                        types::FormattedText {
                            text,
                            entities: Vec::new(),
                        },
                        c,
                    )
                    .await?;
                let content =
                    enums::InputMessageContent::InputMessageText(types::InputMessageText {
                        text: formatted,
                        link_preview_options: None,
                        clear_draft: true,
                    });
                match edit {
                    Some(message_id) => {
                        tdlib_rs::functions::edit_message_text(chat_id, message_id, content, c)
                            .await
                    }
                    None => {
                        let reply = reply_to.map(|message_id| {
                            enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
                                message_id,
                                quote: None,
                                checklist_task_id: 0,
                            })
                        });
                        tdlib_rs::functions::send_message(chat_id, None, reply, None, content, c)
                            .await
                    }
                }
            }
            .await;
            if let Err(e) = result {
                let what = if edit.is_some() { "edit" } else { "send" };
                let _ = tx.send(Event::Error(format!("{what}: {}", friendly_error(&e))));
            }
        });
    }

    pub fn fetch_edit_text(&self, chat_id: i64, message_id: i64, body: RichText) {
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let formatted = types::FormattedText {
                text: body.text,
                entities: body.entities.iter().filter_map(to_td_entity).collect(),
            };
            let ev = match tdlib_rs::functions::get_markdown_text(formatted, c).await {
                Ok(enums::FormattedText::FormattedText(md)) => Event::EditText {
                    chat_id,
                    message_id,
                    text: md.text,
                },
                Err(e) => Event::Error(format!("edit: {}", friendly_error(&e))),
            };
            let _ = tx.send(ev);
        });
    }

    pub fn delete_message(&self, chat_id: i64, message_id: i64, revoke: bool) {
        let c = self.client_id;
        self.spawn("delete", false, async move {
            tdlib_rs::functions::delete_messages(chat_id, vec![message_id], revoke, c).await
        });
    }
}

pub fn to_chat_list(list: ListId) -> ChatList {
    match list {
        ListId::Main => ChatList::Main,
        ListId::Archive => ChatList::Archive,
        ListId::Folder(id) => ChatList::Folder(types::ChatListFolder { chat_folder_id: id }),
    }
}

pub fn from_chat_list(list: &ChatList) -> ListId {
    match list {
        ChatList::Main => ListId::Main,
        ChatList::Archive => ListId::Archive,
        ChatList::Folder(f) => ListId::Folder(f.chat_folder_id),
    }
}

pub fn convert_position(p: &types::ChatPosition) -> Position {
    Position {
        list: from_chat_list(&p.list),
        order: p.order,
        pinned: p.is_pinned,
    }
}

pub fn convert_chat(chat: types::Chat) -> ChatInfo {
    let kind = match &chat.r#type {
        enums::ChatType::Private(p) => ChatKind::Private(p.user_id),
        enums::ChatType::BasicGroup(_) => ChatKind::Group,
        enums::ChatType::Supergroup(s) if s.is_channel => ChatKind::Channel,
        enums::ChatType::Supergroup(_) => ChatKind::Supergroup,
        enums::ChatType::Secret(s) => ChatKind::Secret(s.user_id),
    };
    ChatInfo {
        id: chat.id,
        title: chat.title,
        kind,
        unread: chat.unread_count,
        mentions: chat.unread_mention_count,
        last_read_inbox: chat.last_read_inbox_message_id,
        muted: !chat.notification_settings.use_default_mute_for
            && chat.notification_settings.mute_for > 0,
        positions: chat
            .positions
            .iter()
            .filter(|p| p.order != 0)
            .map(convert_position)
            .collect(),
        last_message: chat.last_message.map(convert_message),
    }
}

fn convert_entity(e: &types::TextEntity) -> Option<Entity> {
    let kind = match &e.r#type {
        TextEntityType::Bold => EntityKind::Bold,
        TextEntityType::Italic => EntityKind::Italic,
        TextEntityType::Underline => EntityKind::Underline,
        TextEntityType::Strikethrough => EntityKind::Strike,
        TextEntityType::Spoiler => EntityKind::Spoiler,
        TextEntityType::Code => EntityKind::Code,
        TextEntityType::Pre | TextEntityType::PreCode(_) => EntityKind::Pre,
        TextEntityType::Url => EntityKind::Link(None),
        TextEntityType::EmailAddress => EntityKind::Link(None),
        TextEntityType::TextUrl(u) => EntityKind::Link(Some(u.url.clone())),
        TextEntityType::Mention
        | TextEntityType::MentionName(_)
        | TextEntityType::Hashtag
        | TextEntityType::Cashtag
        | TextEntityType::BotCommand => EntityKind::Mention,
        TextEntityType::BlockQuote | TextEntityType::ExpandableBlockQuote => EntityKind::Quote,
        _ => return None,
    };
    Some(Entity {
        offset: usize::try_from(e.offset).ok()?,
        length: usize::try_from(e.length).ok()?,
        kind,
    })
}

fn to_td_entity(e: &Entity) -> Option<types::TextEntity> {
    let r#type = match &e.kind {
        EntityKind::Bold => TextEntityType::Bold,
        EntityKind::Italic => TextEntityType::Italic,
        EntityKind::Underline => TextEntityType::Underline,
        EntityKind::Strike => TextEntityType::Strikethrough,
        EntityKind::Spoiler => TextEntityType::Spoiler,
        EntityKind::Code => TextEntityType::Code,
        EntityKind::Pre => TextEntityType::Pre,
        EntityKind::Link(Some(url)) => {
            TextEntityType::TextUrl(types::TextEntityTypeTextUrl { url: url.clone() })
        }
        EntityKind::Quote => TextEntityType::BlockQuote,
        EntityKind::Link(None) | EntityKind::Mention => return None,
    };
    Some(types::TextEntity {
        offset: i32::try_from(e.offset).ok()?,
        length: i32::try_from(e.length).ok()?,
        r#type,
    })
}

pub fn convert_text(t: types::FormattedText) -> RichText {
    RichText {
        entities: t.entities.iter().filter_map(convert_entity).collect(),
        text: t.text,
    }
}

fn with_caption(label: String, caption: types::FormattedText) -> (Option<String>, RichText) {
    (Some(label), convert_text(caption))
}

pub fn convert_content(content: MessageContent) -> (Option<String>, RichText) {
    use MessageContent as C;
    match content {
        C::MessageText(t) => (None, convert_text(t.text)),
        C::MessagePhoto(p) => with_caption("[photo]".to_owned(), p.caption),
        C::MessageVideo(v) => {
            with_caption(format!("[video {}]", duration(v.video.duration)), v.caption)
        }
        C::MessageAnimation(a) => with_caption("[GIF]".to_owned(), a.caption),
        C::MessageDocument(d) => {
            with_caption(format!("[file {}]", d.document.file_name), d.caption)
        }
        C::MessageAudio(a) => {
            let name = match (a.audio.performer.is_empty(), a.audio.title.is_empty()) {
                (false, false) => format!("{} – {}", a.audio.performer, a.audio.title),
                (true, false) => a.audio.title.clone(),
                _ => a.audio.file_name.clone(),
            };
            with_caption(
                format!("[audio {name} {}]", duration(a.audio.duration)),
                a.caption,
            )
        }
        C::MessageVoiceNote(v) => with_caption(
            format!("[voice {}]", duration(v.voice_note.duration)),
            v.caption,
        ),
        C::MessageVideoNote(v) => (
            Some(format!("[video note {}]", duration(v.video_note.duration))),
            RichText::default(),
        ),
        C::MessageSticker(s) => (
            Some(format!("[sticker {}]", s.sticker.emoji)),
            RichText::default(),
        ),
        C::MessageAnimatedEmoji(e) => (None, RichText::plain(e.emoji)),
        C::MessageLocation(_) => (Some("[location]".to_owned()), RichText::default()),
        C::MessageVenue(_) => (Some("[venue]".to_owned()), RichText::default()),
        C::MessageContact(c) => (
            Some(format!(
                "[contact {} {}]",
                c.contact.first_name, c.contact.phone_number
            )),
            RichText::default(),
        ),
        C::MessagePoll(p) => (
            Some(format!("[poll: {}]", p.poll.question.text)),
            RichText::default(),
        ),
        C::MessageDice(d) => (Some(format!("[dice {}]", d.emoji)), RichText::default()),
        C::MessageCall(c) => (
            Some(format!("[call {}]", duration(c.duration))),
            RichText::default(),
        ),
        C::MessagePinMessage(_) => (Some("[pinned a message]".to_owned()), RichText::default()),
        C::MessageChatAddMembers(_) => (Some("[added members]".to_owned()), RichText::default()),
        C::MessageChatJoinByLink | C::MessageChatJoinByRequest => {
            (Some("[joined the chat]".to_owned()), RichText::default())
        }
        C::MessageChatDeleteMember(_) => (Some("[left the chat]".to_owned()), RichText::default()),
        C::MessageChatChangeTitle(t) => (
            Some(format!("[changed title to {}]", t.title)),
            RichText::default(),
        ),
        C::MessageChatChangePhoto(_) => (Some("[changed photo]".to_owned()), RichText::default()),
        C::MessageExpiredPhoto => (Some("[expired photo]".to_owned()), RichText::default()),
        C::MessageExpiredVideo => (Some("[expired video]".to_owned()), RichText::default()),
        C::MessageExpiredVideoNote => {
            (Some("[expired video note]".to_owned()), RichText::default())
        }
        C::MessageExpiredVoiceNote => (Some("[expired voice]".to_owned()), RichText::default()),
        C::MessageScreenshotTaken => (Some("[took a screenshot]".to_owned()), RichText::default()),
        other => {
            let name = format!("{other:?}");
            let name = name
                .split('(')
                .next()
                .unwrap_or("message")
                .trim_start_matches("Message");
            (Some(format!("[{name}]")), RichText::default())
        }
    }
}

pub fn convert_sender(s: &MessageSender) -> Sender {
    match s {
        MessageSender::User(u) => Sender::User(u.user_id),
        MessageSender::Chat(c) => Sender::Chat(c.chat_id),
    }
}

pub fn convert_message(m: types::Message) -> Msg {
    let (media, body) = convert_content(m.content);
    let state = match m.sending_state {
        None => SendState::Sent,
        Some(enums::MessageSendingState::Pending(_)) => SendState::Pending,
        Some(enums::MessageSendingState::Failed(f)) => SendState::Failed(f.error.message.clone()),
    };
    Msg {
        id: m.id,
        chat_id: m.chat_id,
        sender: convert_sender(&m.sender_id),
        date: i64::from(m.date),
        edited: m.edit_date > 0,
        outgoing: m.is_outgoing,
        state,
        reply_to: match m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) if r.chat_id == 0 || r.chat_id == m.chat_id => {
                Some(r.message_id)
            }
            _ => None,
        },
        media,
        body,
    }
}

pub fn user_name(u: &types::User) -> String {
    let name = format!("{} {}", u.first_name, u.last_name)
        .trim()
        .to_owned();
    if name.is_empty() {
        "Deleted Account".to_owned()
    } else {
        name
    }
}
