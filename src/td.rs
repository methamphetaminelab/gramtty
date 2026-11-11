use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::Path;

use tdlib_rs::enums::{
    self, ChatList, MessageContent, MessageSender, StickerFormat, TextEntityType, ThumbnailFormat,
    Update,
};
use tdlib_rs::types;
use tokio::sync::mpsc::UnboundedSender;

use crate::bots::{Button, ButtonAction, Command, Keyboard};
use crate::format::duration;
use crate::model::{
    self, Attachment, ChatInfo, ChatKind, Decode, Entity, EntityKind, FileRef, ListId, MediaKind,
    Membership, Msg, MsgMeta, Origin, Position, Profile, ProfileTarget, Reaction, RichText, Rights,
    SecretChat, SecretState, SendState, Sender, Topic, TopicCursor, Visual,
};
use crate::presence::{Activity, Status};
use crate::stickers::{Data as StickerData, Item, ItemKind};
use crate::upload::{self, Kind as UploadKind, Pending, ToolResult};

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
        epoch: u64,
    },
    HistoryFailed {
        chat_id: i64,
        epoch: u64,
        error: String,
    },
    NewerHistory {
        chat_id: i64,
        msgs: Vec<Msg>,
        epoch: u64,
    },
    HistoryAround {
        chat_id: i64,
        target: i64,
        result: Result<Vec<Msg>, String>,
    },
    EditText {
        chat_id: i64,
        message_id: i64,
        text: String,
    },
    AuthError(String),
    Error(String),
    File(FileRef),
    DownloadFailed {
        file_id: i32,
        error: String,
    },
    Image(crate::media::Done),
    Anim(crate::anim::Done),
    Video,
    Player {
        token: u64,
        result: Result<(), String>,
    },
    Saved(Result<String, String>),
    Notice(String),
    Clipboard(Result<crate::upload::Clip, String>),
    DirListing {
        dir: std::path::PathBuf,
        result: Result<Vec<crate::upload::DirEntry>, String>,
    },
    Voice(Result<Option<crate::voice::VoiceNote>, String>),
    Stickers(crate::stickers::Data),
    StickersFailed(String),
    NotifyFailed(String),
    Profile(ProfileTarget, Result<Profile, String>),
    BotCommands {
        chat_id: i64,
        result: Result<Vec<Command>, String>,
    },
    CallbackAnswer(Result<Answer, String>),
    AvailableReactions {
        chat_id: i64,
        message_id: i64,
        result: Result<ReactionChoices, String>,
    },
    ReactionList {
        chat_id: i64,
        message_id: i64,
        result: Result<Vec<(Sender, String)>, String>,
    },
    CustomEmoji(Result<std::collections::HashMap<i64, String>, String>),
    Search {
        token: u64,
        result: Result<Vec<Msg>, String>,
    },
    MediaCounts {
        chat_id: i64,
        counts: Vec<(crate::model::MediaFilter, Option<i32>)>,
    },
    Fetched(Msg),
    Properties {
        chat_id: i64,
        message_id: i64,
        result: Result<Flags, String>,
    },
    Blocked(Result<Vec<Sender>, String>),
    Mention {
        chat_id: i64,
        result: Result<Option<(i64, Option<i32>)>, String>,
    },
    Reaction {
        chat_id: i64,
        result: Result<Option<(i64, Option<i32>)>, String>,
    },
    Topics {
        chat_id: i64,
        result: Result<TopicPage, String>,
    },
    TopicRefreshed {
        chat_id: i64,
        result: Result<Topic, String>,
    },
    TopicCreated {
        chat_id: i64,
        result: Result<Topic, String>,
    },
    TopicLink(Result<String, String>),
    SecretCreated(Result<i64, String>),
    Pinned {
        chat_id: i64,
        result: Result<Option<Msg>, String>,
    },
    SoundFailed(String),
    Translated {
        chat_id: i64,
        message_id: i64,
        result: Result<String, String>,
    },
    Members {
        token: u64,
        result: Result<crate::admin::MemberPage, String>,
    },
    ActionLog {
        chat_id: i64,
        from: i64,
        result: Result<Vec<crate::admin::LogEntry>, String>,
    },
    ImageCopied(Result<(), String>),
    SettingsSaved(Result<(), String>),
    ThemeFile(Result<(), String>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub alert: bool,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub can_be_saved: bool,
    pub can_be_forwarded: bool,
    pub can_be_copied: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReactionChoices {
    pub emojis: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicPage {
    pub topics: Vec<Topic>,
    pub next: Option<TopicCursor>,
    pub total: i32,
}

pub type RequestLog = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

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
    log: Option<RequestLog>,
}

impl Td {
    pub fn new(tx: UnboundedSender<Event>) -> Self {
        Self {
            client_id: tdlib_rs::create_client(),
            tx,
            log: None,
        }
    }

    #[cfg(test)]
    pub fn recording(tx: UnboundedSender<Event>) -> (Self, RequestLog) {
        let log = RequestLog::default();
        (
            Self {
                client_id: 0,
                tx,
                log: Some(log.clone()),
            },
            log,
        )
    }

    fn fake(&self, what: &str) -> bool {
        match &self.log {
            Some(log) => {
                log.lock().expect("request log").push(what.to_owned());
                true
            }
            None => false,
        }
    }

    pub fn recreate(&mut self) {
        self.client_id = tdlib_rs::create_client();
    }

    fn spawn<F>(&self, context: &'static str, auth: bool, fut: F)
    where
        F: Future<Output = Result<(), types::Error>> + Send + 'static,
    {
        if self.fake(context) {
            return;
        }
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
        if self.fake("load chats") {
            return;
        }
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

    pub fn load_history(
        &self,
        chat_id: i64,
        from_message_id: i64,
        initial: bool,
        epoch: u64,
        topic: Option<i32>,
    ) {
        self.note(format!("history {chat_id} topic {topic:?}"));
        if self.fake("load history") {
            return;
        }
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ev = match history_page(chat_id, from_message_id, 0, 50, topic, c).await {
                Ok(msgs) => Event::History {
                    chat_id,
                    msgs,
                    initial,
                    epoch,
                },
                Err(error) => Event::HistoryFailed {
                    chat_id,
                    epoch,
                    error,
                },
            };
            let _ = tx.send(ev);
        });
    }

    pub fn load_newer(&self, chat_id: i64, from_message_id: i64, epoch: u64, topic: Option<i32>) {
        let c = self.client_id;
        self.task("load newer", async move {
            match history_page(chat_id, from_message_id, -49, 50, topic, c).await {
                Ok(msgs) => Event::NewerHistory {
                    chat_id,
                    msgs,
                    epoch,
                },
                Err(e) => Event::Error(format!("load newer messages: {e}")),
            }
        });
    }

    pub fn load_around(&self, chat_id: i64, target: i64, topic: Option<i32>) {
        self.note(format!("around {chat_id} {target} topic {topic:?}"));
        let c = self.client_id;
        self.task("load around", async move {
            let result = match tdlib_rs::functions::get_message(chat_id, target, c).await {
                Err(e) if e.code == 404 => Err("that message was deleted".to_owned()),
                Err(e) => Err(format!("cannot open that message: {}", friendly_error(&e))),
                Ok(enums::Message::Message(m)) => {
                    around_window(convert_message(m), || {
                        history_page(chat_id, target, AROUND_OFFSET, AROUND_LIMIT, topic, c)
                    })
                    .await
                }
            };
            Event::HistoryAround {
                chat_id,
                target,
                result,
            }
        });
    }

    pub fn view_messages(&self, chat_id: i64, ids: Vec<i64>, topic: Option<i32>) {
        self.note(format!("view {chat_id} {ids:?} topic {topic:?}"));
        let c = self.client_id;
        let source = topic.map(|_| enums::MessageSource::ForumTopicHistory);
        self.spawn("mark read", false, async move {
            tdlib_rs::functions::view_messages(chat_id, ids, source, true, c).await
        });
    }

    pub fn send_text(
        &self,
        chat_id: i64,
        text: String,
        reply_to: Option<i64>,
        edit: Option<i64>,
        topic: Option<i32>,
        quote: Option<(String, i32)>,
    ) {
        self.note(format!(
            "send {chat_id} topic {topic:?} reply {reply_to:?} edit {edit:?}"
        ));
        if let Some((q, pos)) = &quote {
            self.note(format!("quote {pos} {q}"));
        }
        if self.fake("send") {
            return;
        }
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
                                quote: quote.map(|(text, position)| types::InputTextQuote {
                                    text: types::FormattedText {
                                        text,
                                        entities: Vec::new(),
                                    },
                                    position,
                                }),
                                checklist_task_id: 0,
                            })
                        });
                        tdlib_rs::functions::send_message(
                            chat_id,
                            message_topic(topic),
                            reply,
                            None,
                            content,
                            c,
                        )
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
        if self.fake("fetch edit text") {
            return;
        }
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

    pub fn delete_messages(&self, chat_id: i64, message_ids: Vec<i64>, revoke: bool) {
        let c = self.client_id;
        self.spawn("delete", false, async move {
            tdlib_rs::functions::delete_messages(chat_id, message_ids, revoke, c).await
        });
    }

    pub fn sender(&self) -> UnboundedSender<Event> {
        self.tx.clone()
    }

    pub fn download_file(&self, file_id: i32, priority: i32) {
        if self.fake("download") {
            return;
        }
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ev =
                match tdlib_rs::functions::download_file(file_id, priority, 0, 0, false, c).await {
                    Ok(enums::File::File(f)) => Event::File(convert_file(&f)),
                    Err(e) => Event::DownloadFailed {
                        file_id,
                        error: friendly_error(&e),
                    },
                };
            let _ = tx.send(ev);
        });
    }
}

async fn oldest_found(
    chat_id: i64,
    topic: Option<i32>,
    filter: enums::SearchMessagesFilter,
    c: i32,
) -> Result<Option<(i64, Option<i32>)>, String> {
    tdlib_rs::functions::search_chat_messages(
        chat_id,
        message_topic(topic),
        String::new(),
        None,
        0,
        0,
        100,
        Some(filter),
        c,
    )
    .await
    .map(|enums::FoundChatMessages::FoundChatMessages(f)| {
        f.messages
            .iter()
            .min_by_key(|m| m.id)
            .map(|m| (m.id, msg_topic(&m.topic_id)))
    })
    .map_err(|e| friendly_error(&e))
}

fn message_topic(topic: Option<i32>) -> Option<enums::MessageTopic> {
    topic.map(|forum_topic_id| {
        enums::MessageTopic::Forum(types::MessageTopicForum { forum_topic_id })
    })
}

fn reply_to_message(reply_to: Option<i64>) -> Option<enums::InputMessageReplyTo> {
    reply_to.map(|message_id| {
        enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
            message_id,
            quote: None,
            checklist_task_id: 0,
        })
    })
}

fn local_file(path: &std::path::Path) -> enums::InputFile {
    enums::InputFile::Local(types::InputFileLocal {
        path: path.to_string_lossy().into_owned(),
    })
}

async fn parse_caption(text: String, c: i32) -> Result<types::FormattedText, types::Error> {
    let enums::FormattedText::FormattedText(f) = tdlib_rs::functions::parse_markdown(
        types::FormattedText {
            text,
            entities: Vec::new(),
        },
        c,
    )
    .await?;
    Ok(f)
}

async fn probe_or_note(p: &Pending, notes: &mut Vec<String>) -> upload::Probe {
    match upload::probe(&p.path).await {
        ToolResult::Ok(v) => v,
        ToolResult::Missing => {
            notes.push("ffprobe not found; sent without duration".to_owned());
            upload::Probe::default()
        }
        ToolResult::Failed(e) => {
            notes.push(format!("{}: {e}", p.name));
            upload::Probe::default()
        }
    }
}

async fn build_content(
    p: &Pending,
    caption: Option<types::FormattedText>,
    uploads: &std::path::Path,
    notes: &mut Vec<String>,
) -> enums::InputMessageContent {
    use enums::InputMessageContent as I;
    let file = local_file(&p.path);
    match p.send_kind() {
        UploadKind::Photo => {
            let (w, h) = image::image_dimensions(&p.path)
                .map(|(w, h)| (w as i32, h as i32))
                .unwrap_or((0, 0));
            I::InputMessagePhoto(types::InputMessagePhoto {
                photo: file,
                thumbnail: None,
                added_sticker_file_ids: Vec::new(),
                width: w,
                height: h,
                caption,
                show_caption_above_media: false,
                self_destruct_type: None,
                has_spoiler: false,
            })
        }
        UploadKind::Video => {
            let info = probe_or_note(p, notes).await;
            let thumbnail = match upload::thumbnail(&p.path, uploads).await {
                ToolResult::Ok(t) => {
                    let (w, h) = image::image_dimensions(&t)
                        .map(|(w, h)| (w as i32, h as i32))
                        .unwrap_or((0, 0));
                    Some(types::InputThumbnail {
                        thumbnail: local_file(&t),
                        width: w,
                        height: h,
                    })
                }
                ToolResult::Missing => {
                    notes.push("ffmpeg not found; video sent without thumbnail".to_owned());
                    None
                }
                ToolResult::Failed(e) => {
                    notes.push(format!("thumbnail for {}: {e}", p.name));
                    None
                }
            };
            I::InputMessageVideo(types::InputMessageVideo {
                video: file,
                thumbnail,
                cover: None,
                start_timestamp: 0,
                added_sticker_file_ids: Vec::new(),
                duration: info.duration,
                width: info.width,
                height: info.height,
                supports_streaming: true,
                caption,
                show_caption_above_media: false,
                self_destruct_type: None,
                has_spoiler: false,
            })
        }
        UploadKind::Animation => {
            let (w, h) = image::image_dimensions(&p.path)
                .map(|(w, h)| (w as i32, h as i32))
                .unwrap_or((0, 0));
            I::InputMessageAnimation(types::InputMessageAnimation {
                animation: file,
                thumbnail: None,
                added_sticker_file_ids: Vec::new(),
                duration: 0,
                width: w,
                height: h,
                caption,
                show_caption_above_media: false,
                has_spoiler: false,
            })
        }
        UploadKind::Audio => {
            let info = probe_or_note(p, notes).await;
            I::InputMessageAudio(types::InputMessageAudio {
                audio: file,
                album_cover_thumbnail: None,
                duration: info.duration,
                title: info.title,
                performer: info.performer,
                caption,
            })
        }
        UploadKind::Document => I::InputMessageDocument(types::InputMessageDocument {
            document: file,
            thumbnail: None,
            disable_content_type_detection: p.as_file,
            caption,
        }),
    }
}

pub fn sticker_item(st: &types::Sticker) -> Item {
    let mut att = sticker_attachment(st);
    if let Some(t) = thumb_visual(&st.thumbnail).filter(|t| t.decode != Decode::Video) {
        att.preview = Some(t);
    }
    Item {
        kind: ItemKind::Sticker,
        file_id: st.sticker.id,
        emoji: st.emoji.clone(),
        width: st.width,
        height: st.height,
        duration: 0,
        att,
    }
}

pub fn animation_item(a: &types::Animation) -> Item {
    let att = video_like(
        MediaKind::Animation,
        &a.animation,
        a.width,
        a.height,
        a.duration,
        &a.file_name,
        &a.mime_type,
        &a.minithumbnail,
        &a.thumbnail,
    );
    Item {
        kind: ItemKind::Animation,
        file_id: a.animation.id,
        emoji: String::new(),
        width: a.width,
        height: a.height,
        duration: a.duration,
        att,
    }
}

const STICKER_SEARCH_EMOJIS: usize = 24;
const KEYWORD_MIN_CHARS: usize = 3;

async fn keyword_emojis(word: &str, langs: &[String], c: i32) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut len = chars.len();
    while len >= KEYWORD_MIN_CHARS.min(chars.len()) && len > 0 {
        let prefix: String = chars[..len].iter().collect();
        if let Ok(enums::EmojiKeywords::EmojiKeywords(k)) =
            tdlib_rs::functions::search_emojis(prefix, langs.to_vec(), c).await
            && !k.emoji_keywords.is_empty()
        {
            let mut out: Vec<String> = Vec::new();
            for e in k.emoji_keywords {
                if !out.contains(&e.emoji) {
                    out.push(e.emoji);
                }
            }
            return out;
        }
        len -= 1;
    }
    Vec::new()
}

async fn search_gifs(query: String, chat_id: i64, c: i32) -> Result<Vec<Item>, types::Error> {
    let bot = match tdlib_rs::functions::get_option("animation_search_bot_username".to_owned(), c)
        .await?
    {
        enums::OptionValue::String(s) => s.value,
        _ => "gif".to_owned(),
    };
    let enums::Chat::Chat(chat) = tdlib_rs::functions::search_public_chat(bot, c).await?;
    let enums::ChatType::Private(p) = chat.r#type else {
        return Ok(Vec::new());
    };
    let enums::InlineQueryResults::InlineQueryResults(found) =
        tdlib_rs::functions::get_inline_query_results(
            p.user_id,
            chat_id,
            None,
            query,
            String::new(),
            c,
        )
        .await?;
    Ok(found
        .results
        .iter()
        .filter_map(|r| match r {
            enums::InlineQueryResult::Animation(a) => Some(animation_item(&a.animation)),
            _ => None,
        })
        .collect())
}

fn stickers_of(s: enums::Stickers) -> Vec<Item> {
    let enums::Stickers::Stickers(s) = s;
    s.stickers.iter().map(sticker_item).collect()
}

impl Td {
    pub fn send_uploads(
        &self,
        chat_id: i64,
        items: Vec<Pending>,
        text: String,
        reply_to: Option<i64>,
        uploads: std::path::PathBuf,
        topic: Option<i32>,
    ) {
        self.note(format!(
            "send uploads {chat_id} topic {topic:?} reply {reply_to:?}"
        ));
        if self.fake("send uploads") {
            return;
        }
        let c = self.client_id;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut notes = Vec::new();
            let result: Result<(), types::Error> = async {
                let long = text.chars().count() > upload::CAPTION_MAX;
                let mut caption = if text.trim().is_empty() || long {
                    None
                } else {
                    Some(parse_caption(text.clone(), c).await?)
                };
                let mut reply = reply_to;
                for group in upload::plan(&items) {
                    let mut contents = Vec::with_capacity(group.len());
                    for i in group {
                        contents.push(
                            build_content(&items[i], caption.take(), &uploads, &mut notes).await,
                        );
                    }
                    let r = reply_to_message(reply.take());
                    if contents.len() == 1 {
                        let content = contents.pop().expect("one content");
                        tdlib_rs::functions::send_message(
                            chat_id,
                            message_topic(topic),
                            r,
                            None,
                            content,
                            c,
                        )
                        .await?;
                    } else {
                        tdlib_rs::functions::send_message_album(
                            chat_id,
                            message_topic(topic),
                            r,
                            None,
                            contents,
                            c,
                        )
                        .await?;
                    }
                }
                if long {
                    let formatted = parse_caption(text, c).await?;
                    let content =
                        enums::InputMessageContent::InputMessageText(types::InputMessageText {
                            text: formatted,
                            link_preview_options: None,
                            clear_draft: true,
                        });
                    tdlib_rs::functions::send_message(
                        chat_id,
                        message_topic(topic),
                        None,
                        None,
                        content,
                        c,
                    )
                    .await?;
                }
                Ok(())
            }
            .await;
            notes.dedup();
            if !notes.is_empty() {
                let _ = tx.send(Event::Notice(notes.join("; ")));
            }
            if let Err(e) = result {
                let _ = tx.send(Event::Error(format!("send: {}", friendly_error(&e))));
            }
        });
    }

    pub fn send_voice(
        &self,
        chat_id: i64,
        note: crate::voice::VoiceNote,
        reply_to: Option<i64>,
        topic: Option<i32>,
    ) {
        use base64::Engine;
        self.note(format!(
            "send voice {chat_id} topic {topic:?} reply {reply_to:?}"
        ));
        let c = self.client_id;
        let waveform = base64::engine::general_purpose::STANDARD
            .encode(crate::voice::pack_waveform(&note.waveform));
        self.spawn("send voice", false, async move {
            let content =
                enums::InputMessageContent::InputMessageVoiceNote(types::InputMessageVoiceNote {
                    voice_note: local_file(&note.path),
                    duration: note.duration,
                    waveform,
                    caption: None,
                    self_destruct_type: None,
                });
            tdlib_rs::functions::send_message(
                chat_id,
                message_topic(topic),
                reply_to_message(reply_to),
                None,
                content,
                c,
            )
            .await
            .map(|_| ())
        });
    }

    pub fn send_item(&self, chat_id: i64, item: Item, reply_to: Option<i64>, topic: Option<i32>) {
        self.note(format!(
            "send sticker {chat_id} topic {topic:?} reply {reply_to:?}"
        ));
        let c = self.client_id;
        self.spawn("send sticker", false, async move {
            let file = enums::InputFile::Id(types::InputFileId { id: item.file_id });
            let content = match item.kind {
                ItemKind::Sticker => {
                    enums::InputMessageContent::InputMessageSticker(types::InputMessageSticker {
                        sticker: file,
                        thumbnail: None,
                        width: item.width,
                        height: item.height,
                        emoji: item.emoji,
                    })
                }
                ItemKind::Animation => enums::InputMessageContent::InputMessageAnimation(
                    types::InputMessageAnimation {
                        animation: file,
                        thumbnail: None,
                        added_sticker_file_ids: Vec::new(),
                        duration: item.duration,
                        width: item.width,
                        height: item.height,
                        caption: None,
                        show_caption_above_media: false,
                        has_spoiler: false,
                    },
                ),
            };
            tdlib_rs::functions::send_message(
                chat_id,
                message_topic(topic),
                reply_to_message(reply_to),
                None,
                content,
                c,
            )
            .await
            .map(|_| ())
        });
    }

    fn sticker_task<F>(&self, fut: F)
    where
        F: Future<Output = Result<StickerData, types::Error>> + Send + 'static,
    {
        if self.fake("stickers") {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ev = match fut.await {
                Ok(d) => Event::Stickers(d),
                Err(e) => Event::StickersFailed(friendly_error(&e)),
            };
            let _ = tx.send(ev);
        });
    }

    pub fn load_stickers(&self) {
        let c = self.client_id;
        self.sticker_task(async move {
            Ok(StickerData::Recent(stickers_of(
                tdlib_rs::functions::get_recent_stickers(false, c).await?,
            )))
        });
        self.sticker_task(async move {
            Ok(StickerData::Favorite(stickers_of(
                tdlib_rs::functions::get_favorite_stickers(c).await?,
            )))
        });
        self.sticker_task(async move {
            let enums::Animations::Animations(a) =
                tdlib_rs::functions::get_saved_animations(c).await?;
            Ok(StickerData::Gifs(
                a.animations.iter().map(animation_item).collect(),
            ))
        });
        self.sticker_task(async move {
            let enums::StickerSets::StickerSets(s) =
                tdlib_rs::functions::get_installed_sticker_sets(enums::StickerType::Regular, c)
                    .await?;
            Ok(StickerData::Sets(
                s.sets.into_iter().map(|s| (s.id, s.title)).collect(),
            ))
        });
    }

    pub fn load_sticker_set(&self, set_id: i64) {
        let c = self.client_id;
        self.sticker_task(async move {
            let enums::StickerSet::StickerSet(s) =
                tdlib_rs::functions::get_sticker_set(set_id, c).await?;
            Ok(StickerData::Set(
                set_id,
                s.stickers.iter().map(sticker_item).collect(),
            ))
        });
    }

    pub fn search_stickers(&self, query: String, chat_id: i64, gifs: bool) {
        let c = self.client_id;
        self.sticker_task(async move {
            if gifs {
                return Ok(StickerData::Search(
                    query.clone(),
                    search_gifs(query, chat_id, c).await?,
                ));
            }
            let langs = vec!["ru".to_owned(), "en".to_owned()];
            let mut emojis: Vec<String> = Vec::new();
            for word in query.split_whitespace() {
                for e in keyword_emojis(word, &langs, c).await {
                    if !emojis.contains(&e) {
                        emojis.push(e);
                    }
                }
            }
            let mut items = stickers_of(
                tdlib_rs::functions::get_stickers(
                    enums::StickerType::Regular,
                    query.clone(),
                    100,
                    chat_id,
                    c,
                )
                .await?,
            );
            if !emojis.is_empty() {
                emojis.truncate(STICKER_SEARCH_EMOJIS);
                let joined = emojis.join(" ");
                if let Ok(found) = tdlib_rs::functions::get_stickers(
                    enums::StickerType::Regular,
                    joined.clone(),
                    200,
                    chat_id,
                    c,
                )
                .await
                {
                    items.extend(stickers_of(found));
                }
                if let Ok(found) = tdlib_rs::functions::search_stickers(
                    enums::StickerType::Regular,
                    joined,
                    query.clone(),
                    langs,
                    0,
                    100,
                    c,
                )
                .await
                {
                    items.extend(stickers_of(found));
                }
            }
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.file_id));
            Ok(StickerData::Search(query, items))
        });
    }
}

impl Td {
    fn task<F>(&self, context: &'static str, fut: F)
    where
        F: Future<Output = Event> + Send + 'static,
    {
        if self.fake(context) {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(fut.await);
        });
    }

    pub fn set_online(&self, online: bool) {
        let c = self.client_id;
        let context = if online { "go online" } else { "go offline" };
        self.spawn(context, false, async move {
            tdlib_rs::functions::set_option(
                "online".to_owned(),
                Some(enums::OptionValue::Boolean(types::OptionValueBoolean {
                    value: online,
                })),
                c,
            )
            .await
        });
    }

    pub fn send_chat_action(&self, chat_id: i64, action: enums::ChatAction, topic: Option<i32>) {
        let c = self.client_id;
        self.spawn("chat action", false, async move {
            tdlib_rs::functions::send_chat_action(chat_id, message_topic(topic), Some(action), c)
                .await
        });
    }

    fn note(&self, entry: String) {
        if let Some(log) = &self.log {
            log.lock().expect("request log").push(entry);
        }
    }

    pub fn notify(&self, title: String, body: String, icon: crate::notify::Icon) {
        if self.fake("notify") {
            self.note(format!("notify icon {icon:?}"));
            return;
        }
        let tx = self.tx.clone();
        let c = self.client_id;
        match icon {
            crate::notify::Icon::None => crate::notify::send(title, body, None, tx),
            crate::notify::Icon::Path(p) => crate::notify::send(title, body, Some(p), tx),
            crate::notify::Icon::Download(id) => {
                tokio::spawn(async move {
                    let fetched = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        tdlib_rs::functions::download_file(id, 16, 0, 0, true, c),
                    )
                    .await;
                    let path = match fetched {
                        Ok(Ok(enums::File::File(f))) => convert_file(&f).local().map(str::to_owned),
                        _ => None,
                    };
                    crate::notify::send(title, body, path, tx);
                });
            }
        }
    }

    pub fn play_sound(&self, cmd: crate::notify::SoundCommand, file: std::path::PathBuf) {
        if self.fake("sound") {
            self.note(format!("sound {} {}", cmd.program, cmd.args.join(" ")));
            return;
        }
        crate::notify::play(cmd, file, self.tx.clone());
    }

    pub fn load_pinned(&self, chat_id: i64) {
        let c = self.client_id;
        self.task("load pinned", async move {
            let result = match tdlib_rs::functions::get_chat_pinned_message(chat_id, c).await {
                Ok(enums::Message::Message(m)) => Ok(Some(convert_message(m))),
                Err(e) if e.code == 404 => Ok(None),
                Err(e) => Err(friendly_error(&e)),
            };
            Event::Pinned { chat_id, result }
        });
    }

    pub fn read_all_mentions(&self, chat_id: i64) {
        self.note(format!("read all mentions {chat_id}"));
        let c = self.client_id;
        self.spawn("read mentions", false, async move {
            tdlib_rs::functions::read_all_chat_mentions(chat_id, c).await
        });
    }

    pub fn load_profile(&self, target: ProfileTarget) {
        let c = self.client_id;
        self.task("load profile", async move {
            let result = fetch_profile(target, c)
                .await
                .map_err(|e| friendly_error(&e));
            Event::Profile(target, result)
        });
    }

    pub fn load_bot_commands(&self, chat_id: i64) {
        let c = self.client_id;
        self.task("bot commands", async move {
            let result = fetch_commands(chat_id, c)
                .await
                .map_err(|e| friendly_error(&e));
            Event::BotCommands { chat_id, result }
        });
    }

    pub fn press_callback(&self, chat_id: i64, message_id: i64, data: String) {
        let c = self.client_id;
        self.task("callback", async move {
            let payload =
                enums::CallbackQueryPayload::Data(types::CallbackQueryPayloadData { data });
            let result =
                tdlib_rs::functions::get_callback_query_answer(chat_id, message_id, payload, c)
                    .await
                    .map(
                        |enums::CallbackQueryAnswer::CallbackQueryAnswer(a)| Answer {
                            text: a.text,
                            alert: a.show_alert,
                            url: a.url,
                        },
                    )
                    .map_err(|e| friendly_error(&e));
            Event::CallbackAnswer(result)
        });
    }

    pub fn load_available_reactions(&self, chat_id: i64, message_id: i64) {
        let c = self.client_id;
        self.task("available reactions", async move {
            let result =
                tdlib_rs::functions::get_message_available_reactions(chat_id, message_id, 8, c)
                    .await
                    .map(|enums::AvailableReactions::AvailableReactions(a)| reaction_choices(&a))
                    .map_err(|e| friendly_error(&e));
            Event::AvailableReactions {
                chat_id,
                message_id,
                result,
            }
        });
    }

    pub fn load_custom_emoji(&self, ids: Vec<i64>) {
        let c = self.client_id;
        self.task("custom emoji", async move {
            Event::CustomEmoji(custom_emoji_names(ids, c).await)
        });
    }

    pub fn set_reaction(&self, chat_id: i64, message_id: i64, emoji: String, add: bool) {
        let c = self.client_id;
        self.spawn("react", false, async move {
            let r = enums::ReactionType::Emoji(types::ReactionTypeEmoji { emoji });
            if add {
                tdlib_rs::functions::add_message_reaction(chat_id, message_id, r, false, true, c)
                    .await
            } else {
                tdlib_rs::functions::remove_message_reaction(chat_id, message_id, r, c).await
            }
        });
    }

    pub fn load_viewers(&self, chat_id: i64, message_id: i64) {
        let c = self.client_id;
        self.task("viewers", async move {
            let result = tdlib_rs::functions::get_message_viewers(chat_id, message_id, c)
                .await
                .map(|enums::MessageViewers::MessageViewers(v)| {
                    v.viewers
                        .iter()
                        .map(|v| {
                            let at = chrono::DateTime::from_timestamp(i64::from(v.view_date), 0)
                                .map(|d| {
                                    d.with_timezone(&chrono::Local)
                                        .format("%d.%m %H:%M")
                                        .to_string()
                                })
                                .unwrap_or_default();
                            (Sender::User(v.user_id), at)
                        })
                        .collect()
                })
                .map_err(|e| friendly_error(&e));
            Event::ReactionList {
                chat_id,
                message_id,
                result,
            }
        });
    }

    pub fn load_reaction_list(&self, chat_id: i64, message_id: i64) {
        let c = self.client_id;
        self.task("reaction list", async move {
            let result = tdlib_rs::functions::get_message_added_reactions(
                chat_id,
                message_id,
                None,
                String::new(),
                100,
                c,
            )
            .await
            .map(|enums::AddedReactions::AddedReactions(a)| {
                a.reactions
                    .iter()
                    .map(|r| {
                        let label = match &r.r#type {
                            enums::ReactionType::Emoji(e) => e.emoji.clone(),
                            enums::ReactionType::CustomEmoji(_) => {
                                crate::format::EMOJI_PLACEHOLDER.to_owned()
                            }
                            enums::ReactionType::Paid => "⭐".to_owned(),
                        };
                        (convert_sender(&r.sender_id), label)
                    })
                    .collect()
            })
            .map_err(|e| friendly_error(&e));
            Event::ReactionList {
                chat_id,
                message_id,
                result,
            }
        });
    }

    pub fn pin_message(&self, chat_id: i64, message_id: i64, pin: bool) {
        let c = self.client_id;
        self.spawn(if pin { "pin" } else { "unpin" }, false, async move {
            if pin {
                tdlib_rs::functions::pin_chat_message(chat_id, message_id, true, false, c).await
            } else {
                tdlib_rs::functions::unpin_chat_message(chat_id, message_id, c).await
            }
        });
    }

    pub fn resend(&self, chat_id: i64, message_ids: Vec<i64>) {
        self.note(format!("resend {chat_id} {message_ids:?}"));
        let c = self.client_id;
        self.spawn("resend", false, async move {
            tdlib_rs::functions::resend_messages(chat_id, message_ids, None, 0, c)
                .await
                .map(|_| ())
        });
    }

    pub fn forward(&self, to: i64, from: i64, message_ids: Vec<i64>, copy: bool) {
        self.note(format!(
            "forward {from} {message_ids:?} -> {to}{}",
            if copy { " copy" } else { "" }
        ));
        let c = self.client_id;
        self.spawn("forward", false, async move {
            tdlib_rs::functions::forward_messages(
                to,
                None,
                from,
                message_ids,
                None,
                copy,
                false,
                c,
            )
            .await
            .map(|_| ())
        });
    }

    pub fn search(
        &self,
        token: u64,
        chat_id: Option<i64>,
        query: String,
        pinned: bool,
        sender: Option<Sender>,
        media: Option<crate::model::MediaFilter>,
    ) {
        self.note(format!("search {chat_id:?} sender {sender:?}"));
        let c = self.client_id;
        self.task("search", async move {
            let result = match chat_id {
                Some(chat_id) => {
                    let filter = media
                        .map(media_filter)
                        .or(pinned.then_some(enums::SearchMessagesFilter::Pinned));
                    let sender_id = sender.map(input_sender);
                    tdlib_rs::functions::search_chat_messages(
                        chat_id, None, query, sender_id, 0, 0, 100, filter, c,
                    )
                    .await
                    .map(|enums::FoundChatMessages::FoundChatMessages(f)| f.messages)
                }
                None => tdlib_rs::functions::search_messages(
                    None,
                    query,
                    String::new(),
                    100,
                    None,
                    None,
                    0,
                    0,
                    c,
                )
                .await
                .map(|enums::FoundMessages::FoundMessages(f)| f.messages),
            };
            Event::Search {
                token,
                result: result
                    .map(|m| m.into_iter().map(convert_message).collect())
                    .map_err(|e| friendly_error(&e)),
            }
        });
    }

    pub fn media_counts(&self, chat_id: i64) {
        self.note(format!("media counts {chat_id}"));
        let c = self.client_id;
        self.task("media counts", async move {
            let mut counts = Vec::new();
            for f in crate::model::MediaFilter::ALL {
                let n = tdlib_rs::functions::get_chat_message_count(
                    chat_id,
                    None,
                    media_filter(f),
                    false,
                    c,
                )
                .await
                .ok()
                .map(|enums::Count::Count(n)| n.count);
                counts.push((f, n));
            }
            Event::MediaCounts { chat_id, counts }
        });
    }

    pub fn find_unread_mention(&self, chat_id: i64, topic: Option<i32>) {
        self.note(format!("unread search {chat_id} topic {topic:?}"));
        let c = self.client_id;
        self.task("find mention", async move {
            let filter = enums::SearchMessagesFilter::UnreadMention;
            let result = oldest_found(chat_id, topic, filter, c).await;
            Event::Mention { chat_id, result }
        });
    }

    pub fn find_unread_reaction(&self, chat_id: i64, topic: Option<i32>) {
        self.note(format!("unread search {chat_id} topic {topic:?}"));
        let c = self.client_id;
        self.task("find reaction", async move {
            let filter = enums::SearchMessagesFilter::UnreadReaction;
            let result = oldest_found(chat_id, topic, filter, c).await;
            Event::Reaction { chat_id, result }
        });
    }

    pub fn read_all_reactions(&self, chat_id: i64) {
        self.note(format!("read all reactions {chat_id}"));
        let c = self.client_id;
        self.spawn("read reactions", false, async move {
            tdlib_rs::functions::read_all_chat_reactions(chat_id, c).await
        });
    }

    pub fn read_topic_reactions(&self, chat_id: i64, topic: i32) {
        self.note(format!("read topic reactions {chat_id} {topic}"));
        let c = self.client_id;
        self.spawn("read reactions", false, async move {
            tdlib_rs::functions::read_all_forum_topic_reactions(chat_id, topic, c).await
        });
    }

    pub fn load_properties(&self, chat_id: i64, message_id: i64) {
        let c = self.client_id;
        self.task("message properties", async move {
            let result = tdlib_rs::functions::get_message_properties(chat_id, message_id, c)
                .await
                .map(|enums::MessageProperties::MessageProperties(p)| Flags {
                    can_be_saved: p.can_be_saved,
                    can_be_forwarded: p.can_be_forwarded,
                    can_be_copied: p.can_be_copied,
                })
                .map_err(|e| friendly_error(&e));
            Event::Properties {
                chat_id,
                message_id,
                result,
            }
        });
    }

    pub fn load_blocked(&self) {
        let c = self.client_id;
        self.task("blocked senders", async move {
            let mut all = Vec::new();
            loop {
                let page = tdlib_rs::functions::get_blocked_message_senders(
                    enums::BlockList::Main,
                    all.len() as i32,
                    100,
                    c,
                )
                .await;
                match page {
                    Ok(enums::MessageSenders::MessageSenders(p)) => {
                        let got = p.senders.len();
                        all.extend(p.senders.iter().map(convert_sender));
                        if got == 0 || all.len() as i32 >= p.total_count {
                            break Event::Blocked(Ok(all));
                        }
                    }
                    Err(e) => break Event::Blocked(Err(friendly_error(&e))),
                }
            }
        });
    }

    pub fn translate(&self, chat_id: i64, message_id: i64, lang: String) {
        self.note(format!("translate {chat_id} {message_id} {lang}"));
        let c = self.client_id;
        self.task("translate", async move {
            let result = tdlib_rs::functions::translate_message_text(chat_id, message_id, lang, c)
                .await
                .map(|enums::FormattedText::FormattedText(f)| f.text)
                .map_err(|e| friendly_error(&e));
            Event::Translated {
                chat_id,
                message_id,
                result,
            }
        });
    }

    pub fn load_members(
        &self,
        token: u64,
        supergroup_id: i64,
        filter: crate::admin::MemberFilter,
        query: String,
    ) {
        self.note(format!(
            "members {supergroup_id} {} {query:?}",
            filter.name()
        ));
        let c = self.client_id;
        self.task("members", async move {
            let result = tdlib_rs::functions::get_supergroup_members(
                supergroup_id,
                Some(filter.to_td(&query)),
                0,
                200,
                c,
            )
            .await
            .map(
                |enums::ChatMembers::ChatMembers(m)| crate::admin::MemberPage {
                    total: m.total_count,
                    members: m
                        .members
                        .into_iter()
                        .map(crate::admin::convert_member)
                        .collect(),
                },
            )
            .map_err(|e| friendly_error(&e));
            Event::Members { token, result }
        });
    }

    pub fn load_event_log(&self, chat_id: i64, from: i64) {
        self.note(format!("event log {chat_id} from {from}"));
        let c = self.client_id;
        self.task("event log", async move {
            let result = tdlib_rs::functions::get_chat_event_log(
                chat_id,
                String::new(),
                from,
                crate::admin::LOG_PAGE as i32,
                None,
                Vec::new(),
                c,
            )
            .await
            .map(|enums::ChatEvents::ChatEvents(e)| {
                e.events
                    .into_iter()
                    .map(crate::admin::convert_event)
                    .collect()
            })
            .map_err(|e| friendly_error(&e));
            Event::ActionLog {
                chat_id,
                from,
                result,
            }
        });
    }

    pub fn copy_text(&self, text: String) {
        self.note(format!("copy {text}"));
        self.task("clipboard", async move {
            match upload::copy_text(text).await {
                Ok(()) => Event::Notice("copied to clipboard".to_owned()),
                Err(e) => Event::Error(e),
            }
        });
    }

    pub fn copy_image(&self, path: std::path::PathBuf) {
        self.note(format!("copy image {}", path.display()));
        self.task("clipboard image", async move {
            Event::ImageCopied(upload::copy_image(path).await)
        });
    }

    pub fn fetch_message(&self, chat_id: i64, message_id: i64) {
        let c = self.client_id;
        self.task("get message", async move {
            match tdlib_rs::functions::get_message(chat_id, message_id, c).await {
                Ok(enums::Message::Message(m)) => Event::Fetched(convert_message(m)),
                Err(e) => Event::Error(format!("load keyboard: {}", friendly_error(&e))),
            }
        });
    }
}

impl Td {
    pub fn load_topics(&self, chat_id: i64, cursor: Option<TopicCursor>) {
        self.note(format!("topics {chat_id} {cursor:?}"));
        let c = self.client_id;
        let at = cursor.unwrap_or_default();
        self.task("load topics", async move {
            let result = match tdlib_rs::functions::get_forum_topics(
                chat_id,
                String::new(),
                at.date,
                at.message_id,
                at.topic_id,
                model::TOPIC_PAGE,
                c,
            )
            .await
            {
                Ok(enums::ForumTopics::ForumTopics(f)) => {
                    let next = (f.next_offset_forum_topic_id != 0).then_some(TopicCursor {
                        date: f.next_offset_date,
                        message_id: f.next_offset_message_id,
                        topic_id: f.next_offset_forum_topic_id,
                    });
                    Ok(TopicPage {
                        topics: topics_with_icons(f.topics, c).await,
                        next,
                        total: f.total_count,
                    })
                }
                Err(e) => Err(friendly_error(&e)),
            };
            Event::Topics { chat_id, result }
        });
    }

    pub fn refresh_topic(&self, chat_id: i64, topic: i32) {
        self.note(format!("refresh topic {chat_id} {topic}"));
        let c = self.client_id;
        self.task("refresh topic", async move {
            let result = match tdlib_rs::functions::get_forum_topic(chat_id, topic, c).await {
                Ok(enums::ForumTopic::ForumTopic(t)) => topics_with_icons(vec![t], c)
                    .await
                    .pop()
                    .ok_or_else(|| "empty topic".to_owned()),
                Err(e) => Err(friendly_error(&e)),
            };
            Event::TopicRefreshed { chat_id, result }
        });
    }

    pub fn set_topic_muted(&self, chat_id: i64, topic: i32, mute: bool) {
        self.note(format!("mute topic {chat_id} {topic} {mute}"));
        let c = self.client_id;
        self.spawn("topic notifications", false, async move {
            tdlib_rs::functions::set_forum_topic_notification_settings(
                chat_id,
                topic,
                topic_settings(mute),
                c,
            )
            .await
        });
    }

    pub fn pin_topic(&self, chat_id: i64, topic: i32, pin: bool) {
        self.note(format!("pin topic {chat_id} {topic} {pin}"));
        let c = self.client_id;
        self.spawn("pin topic", false, async move {
            tdlib_rs::functions::toggle_forum_topic_is_pinned(chat_id, topic, pin, c).await
        });
    }

    pub fn close_topic(&self, chat_id: i64, topic: i32, close: bool) {
        self.note(format!("close topic {chat_id} {topic} {close}"));
        let c = self.client_id;
        self.spawn("close topic", false, async move {
            tdlib_rs::functions::toggle_forum_topic_is_closed(chat_id, topic, close, c).await
        });
    }

    pub fn hide_general(&self, chat_id: i64, hide: bool) {
        self.note(format!("hide general {chat_id} {hide}"));
        let c = self.client_id;
        self.spawn("hide General", false, async move {
            tdlib_rs::functions::toggle_general_forum_topic_is_hidden(chat_id, hide, c).await
        });
    }

    pub fn read_topic_mentions(&self, chat_id: i64, topic: i32) {
        self.note(format!("read topic mentions {chat_id} {topic}"));
        let c = self.client_id;
        self.spawn("read mentions", false, async move {
            tdlib_rs::functions::read_all_forum_topic_mentions(chat_id, topic, c).await
        });
    }

    pub fn topic_link(&self, chat_id: i64, topic: i32) {
        self.note(format!("topic link {chat_id} {topic}"));
        let c = self.client_id;
        self.task("topic link", async move {
            Event::TopicLink(
                match tdlib_rs::functions::get_forum_topic_link(chat_id, topic, c).await {
                    Ok(enums::MessageLink::MessageLink(l)) => Ok(l.link),
                    Err(e) => Err(friendly_error(&e)),
                },
            )
        });
    }

    pub fn create_topic(&self, chat_id: i64, name: String) {
        self.note(format!("create topic {chat_id} {name}"));
        let c = self.client_id;
        self.task("create topic", async move {
            let icon = types::ForumTopicIcon {
                color: TOPIC_ICON_COLOR,
                custom_emoji_id: 0,
            };
            let result = match tdlib_rs::functions::create_forum_topic(
                chat_id, name, false, icon, c,
            )
            .await
            {
                Ok(enums::ForumTopicInfo::ForumTopicInfo(i)) => Ok(convert_topic_info(i)),
                Err(e) => Err(friendly_error(&e)),
            };
            Event::TopicCreated { chat_id, result }
        });
    }

    pub fn create_secret_chat(&self, user_id: i64) {
        self.note(format!("secret new {user_id}"));
        let c = self.client_id;
        self.task("new secret chat", async move {
            Event::SecretCreated(
                match tdlib_rs::functions::create_new_secret_chat(user_id, c).await {
                    Ok(enums::Chat::Chat(chat)) => Ok(chat.id),
                    Err(e) => Err(friendly_error(&e)),
                },
            )
        });
    }

    pub fn close_secret_chat(&self, secret_id: i32) {
        self.note(format!("secret close {secret_id}"));
        let c = self.client_id;
        self.spawn("close secret chat", false, async move {
            tdlib_rs::functions::close_secret_chat(secret_id, c).await
        });
    }

    pub fn delete_secret_chat(&self, chat_id: i64, secret_id: i32, open: bool) {
        self.note(format!("secret delete {chat_id} {secret_id} {open}"));
        let c = self.client_id;
        self.spawn("delete secret chat", false, async move {
            if open {
                tdlib_rs::functions::close_secret_chat(secret_id, c).await?;
            }
            tdlib_rs::functions::delete_chat_history(chat_id, true, false, c).await
        });
    }

    pub fn set_auto_delete(&self, chat_id: i64, secs: i32) {
        self.note(format!("auto delete {chat_id} {secs}"));
        let c = self.client_id;
        self.spawn("auto-delete timer", false, async move {
            tdlib_rs::functions::set_chat_message_auto_delete_time(chat_id, secs, c).await
        });
    }
}

pub const AROUND_OFFSET: i32 = -20;
pub const AROUND_LIMIT: i32 = 40;
pub const AROUND_ATTEMPTS: usize = 3;

async fn history_page(
    chat_id: i64,
    from_message_id: i64,
    offset: i32,
    limit: i32,
    topic: Option<i32>,
    client_id: i32,
) -> Result<Vec<Msg>, String> {
    let page = match topic {
        Some(t) => {
            tdlib_rs::functions::get_forum_topic_history(
                chat_id,
                t,
                from_message_id,
                offset,
                limit,
                client_id,
            )
            .await
        }
        None => {
            tdlib_rs::functions::get_chat_history(
                chat_id,
                from_message_id,
                offset,
                limit,
                false,
                client_id,
            )
            .await
        }
    };
    match page {
        Ok(enums::Messages::Messages(m)) => Ok(m
            .messages
            .into_iter()
            .flatten()
            .map(convert_message)
            .collect()),
        Err(e) => Err(friendly_error(&e)),
    }
}

pub async fn around_window<F, Fut>(target: Msg, mut fetch: F) -> Result<Vec<Msg>, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<Msg>, String>>,
{
    let mut out: Vec<Msg> = Vec::new();
    for _ in 0..AROUND_ATTEMPTS {
        for m in fetch().await? {
            if !out.iter().any(|o| o.id == m.id) {
                out.push(m);
            }
        }
        if out.iter().any(|m| m.id == target.id) {
            break;
        }
    }
    if !out.iter().any(|m| m.id == target.id) {
        out.push(target);
    }
    out.sort_by_key(|m| m.id);
    Ok(out)
}

fn reaction_choices(a: &types::AvailableReactions) -> ReactionChoices {
    let mut emojis: Vec<String> = Vec::new();
    for r in a
        .top_reactions
        .iter()
        .chain(&a.recent_reactions)
        .chain(&a.popular_reactions)
    {
        if r.needs_premium {
            continue;
        }
        if let enums::ReactionType::Emoji(e) = &r.r#type
            && !emojis.contains(&e.emoji)
        {
            emojis.push(e.emoji.clone());
        }
    }
    let note = a.unavailability_reason.as_ref().map(|r| match r {
        enums::ReactionUnavailabilityReason::AnonymousAdministrator => {
            "anonymous administrators can't react".to_owned()
        }
        enums::ReactionUnavailabilityReason::Guest => "join the chat to react".to_owned(),
    });
    ReactionChoices { emojis, note }
}

fn usernames(u: &Option<types::Usernames>) -> Vec<String> {
    u.as_ref()
        .map(|n| n.active_usernames.clone())
        .unwrap_or_default()
}

fn tme(names: &[String]) -> Vec<String> {
    names.iter().map(|n| format!("https://t.me/{n}")).collect()
}

fn bot_commands(list: &[types::BotCommand], bot: &str) -> Vec<Command> {
    list.iter()
        .map(|c| Command {
            name: c.command.clone(),
            description: c.description.clone(),
            bot: bot.to_owned(),
        })
        .collect()
}

async fn user_profile(user_id: i64, c: i32) -> Result<Profile, types::Error> {
    let enums::User::User(u) = tdlib_rs::functions::get_user(user_id, c).await?;
    let enums::UserFullInfo::UserFullInfo(full) =
        tdlib_rs::functions::get_user_full_info(user_id, c).await?;
    let names = usernames(&u.usernames);
    let bot = full.bot_info.as_ref();
    let bio = match (&full.bio, bot) {
        (Some(b), _) if !b.text.is_empty() => b.text.clone(),
        (_, Some(info)) if !info.description.is_empty() => info.description.clone(),
        (_, Some(info)) => info.short_description.clone(),
        _ => String::new(),
    };
    let kind = match u.r#type {
        enums::UserType::Bot(_) => "bot",
        enums::UserType::Deleted => "deleted account",
        _ => "user",
    };
    Ok(Profile {
        title: user_name(&u),
        kind: kind.to_owned(),
        id: u.id,
        user_id: Some(u.id),
        links: tme(&names),
        phone: if u.phone_number.is_empty() {
            String::new()
        } else {
            format!("+{}", u.phone_number)
        },
        commands: bot
            .map(|b| bot_commands(&b.commands, names.first().map_or("", String::as_str)))
            .unwrap_or_default(),
        usernames: names,
        bio,
        common_chats: Some(full.group_in_common_count),
        members: None,
    })
}

async fn fetch_profile(target: ProfileTarget, c: i32) -> Result<Profile, types::Error> {
    let chat_id = match target {
        ProfileTarget::User(id) => return user_profile(id, c).await,
        ProfileTarget::Chat(id) => id,
    };
    let enums::Chat::Chat(chat) = tdlib_rs::functions::get_chat(chat_id, c).await?;
    match chat.r#type {
        enums::ChatType::Private(p) => user_profile(p.user_id, c).await,
        enums::ChatType::Secret(s) => {
            let mut p = user_profile(s.user_id, c).await?;
            p.kind = "secret chat".to_owned();
            Ok(p)
        }
        enums::ChatType::BasicGroup(g) => {
            let enums::BasicGroup::BasicGroup(group) =
                tdlib_rs::functions::get_basic_group(g.basic_group_id, c).await?;
            let enums::BasicGroupFullInfo::BasicGroupFullInfo(full) =
                tdlib_rs::functions::get_basic_group_full_info(g.basic_group_id, c).await?;
            Ok(Profile {
                title: chat.title,
                kind: "group".to_owned(),
                id: chat.id,
                bio: full.description,
                members: Some(group.member_count),
                links: full
                    .invite_link
                    .map(|l| l.invite_link)
                    .into_iter()
                    .collect(),
                ..Profile::default()
            })
        }
        enums::ChatType::Supergroup(s) => {
            let enums::Supergroup::Supergroup(group) =
                tdlib_rs::functions::get_supergroup(s.supergroup_id, c).await?;
            let enums::SupergroupFullInfo::SupergroupFullInfo(full) =
                tdlib_rs::functions::get_supergroup_full_info(s.supergroup_id, c).await?;
            let names = usernames(&group.usernames);
            let mut links = tme(&names);
            links.extend(full.invite_link.map(|l| l.invite_link));
            Ok(Profile {
                title: chat.title,
                kind: if group.is_channel {
                    "channel"
                } else {
                    "supergroup"
                }
                .to_owned(),
                id: chat.id,
                usernames: names,
                bio: full.description,
                members: Some(full.member_count.max(group.member_count)),
                links,
                ..Profile::default()
            })
        }
    }
}

async fn group_commands(
    list: Vec<types::BotCommands>,
    c: i32,
) -> Result<Vec<Command>, types::Error> {
    let mut out = Vec::new();
    for b in list {
        let enums::User::User(u) = tdlib_rs::functions::get_user(b.bot_user_id, c).await?;
        let names = usernames(&u.usernames);
        out.extend(bot_commands(
            &b.commands,
            names.first().map_or("", String::as_str),
        ));
    }
    Ok(out)
}

async fn fetch_commands(chat_id: i64, c: i32) -> Result<Vec<Command>, types::Error> {
    let enums::Chat::Chat(chat) = tdlib_rs::functions::get_chat(chat_id, c).await?;
    match chat.r#type {
        enums::ChatType::Private(p) => {
            let enums::User::User(u) = tdlib_rs::functions::get_user(p.user_id, c).await?;
            if !matches!(u.r#type, enums::UserType::Bot(_)) {
                return Ok(Vec::new());
            }
            let enums::UserFullInfo::UserFullInfo(full) =
                tdlib_rs::functions::get_user_full_info(p.user_id, c).await?;
            let names = usernames(&u.usernames);
            Ok(full
                .bot_info
                .map(|b| bot_commands(&b.commands, names.first().map_or("", String::as_str)))
                .unwrap_or_default())
        }
        enums::ChatType::BasicGroup(g) => {
            let enums::BasicGroupFullInfo::BasicGroupFullInfo(full) =
                tdlib_rs::functions::get_basic_group_full_info(g.basic_group_id, c).await?;
            group_commands(full.bot_commands, c).await
        }
        enums::ChatType::Supergroup(s) if !s.is_channel => {
            let enums::SupergroupFullInfo::SupergroupFullInfo(full) =
                tdlib_rs::functions::get_supergroup_full_info(s.supergroup_id, c).await?;
            group_commands(full.bot_commands, c).await
        }
        _ => Ok(Vec::new()),
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
    let (group_id, secret_id) = match &chat.r#type {
        enums::ChatType::Supergroup(s) => (s.supergroup_id, 0),
        enums::ChatType::Secret(s) => (0, s.secret_chat_id),
        _ => (0, 0),
    };
    ChatInfo {
        group_id,
        secret_id,
        forum: false,
        auto_delete: chat.message_auto_delete_time,
        perms: convert_perms(&chat.permissions),
        protected: chat.has_protected_content,
        id: chat.id,
        title: chat.title,
        kind,
        unread: chat.unread_count,
        mentions: chat.unread_mention_count,
        reactions: chat.unread_reaction_count,
        last_read_inbox: chat.last_read_inbox_message_id,
        last_read_outbox: chat.last_read_outbox_message_id,
        photo: chat.photo.as_ref().map(|p| convert_file(&p.small)),
        muted: chat.notification_settings.mute_for > 0,
        use_default_mute: chat.notification_settings.use_default_mute_for,
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

pub struct Content {
    pub media: Option<String>,
    pub body: RichText,
    pub attachments: Vec<Attachment>,
}

impl Content {
    fn label(label: impl Into<String>) -> Self {
        Self {
            media: Some(label.into()),
            body: RichText::default(),
            attachments: Vec::new(),
        }
    }

    fn captioned(label: impl Into<String>, caption: types::FormattedText, att: Attachment) -> Self {
        Self {
            media: Some(label.into()),
            body: convert_text(caption),
            attachments: vec![att],
        }
    }
}

pub fn convert_file(f: &types::File) -> FileRef {
    FileRef {
        id: f.id,
        size: if f.size > 0 { f.size } else { f.expected_size },
        path: f.local.path.clone(),
        done: f.local.is_downloading_completed,
        active: f.local.is_downloading_active,
        downloaded: f.local.downloaded_size,
        uploading: f.remote.is_uploading_active,
        uploaded: f.remote.uploaded_size,
    }
}

fn b64(data: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap_or_default()
}

fn mini(m: &Option<types::Minithumbnail>) -> Option<std::sync::Arc<[u8]>> {
    let m = m.as_ref()?;
    let bytes = b64(&m.data);
    (!bytes.is_empty()).then(|| bytes.into())
}

fn thumb_visual(t: &Option<types::Thumbnail>) -> Option<Visual> {
    let t = t.as_ref()?;
    let decode = match t.format {
        ThumbnailFormat::Jpeg
        | ThumbnailFormat::Png
        | ThumbnailFormat::Webp
        | ThumbnailFormat::Gif => Decode::Raster,
        ThumbnailFormat::Tgs => Decode::Tgs,
        ThumbnailFormat::Mpeg4 | ThumbnailFormat::Webm => Decode::Video,
    };
    Some(Visual {
        file: convert_file(&t.file),
        decode,
        width: t.width,
        height: t.height,
    })
}

const INLINE_PHOTO_PX: i32 = 640;
const SMALL_VIDEO_BYTES: i64 = 4 * 1024 * 1024;

pub fn photo_attachment(p: &types::Photo) -> Option<Attachment> {
    let mut sizes: Vec<&types::PhotoSize> = p.sizes.iter().collect();
    sizes.sort_by_key(|s| i64::from(s.width) * i64::from(s.height));
    let largest = *sizes.last()?;
    let inline = sizes
        .iter()
        .find(|s| s.width.max(s.height) >= INLINE_PHOTO_PX)
        .copied()
        .unwrap_or(largest);
    let visual = |s: &types::PhotoSize| Visual {
        file: convert_file(&s.photo),
        decode: Decode::Raster,
        width: s.width,
        height: s.height,
    };
    let mut att = Attachment::new(MediaKind::Photo, convert_file(&largest.photo));
    att.width = largest.width;
    att.height = largest.height;
    att.preview = Some(visual(inline));
    att.full = Some(visual(largest));
    att.mini = mini(&p.minithumbnail);
    att.name = "photo.jpg".to_owned();
    Some(att)
}

#[allow(clippy::too_many_arguments)]
fn video_like(
    kind: MediaKind,
    file: &types::File,
    width: i32,
    height: i32,
    duration: i32,
    name: &str,
    mime: &str,
    minithumb: &Option<types::Minithumbnail>,
    thumbnail: &Option<types::Thumbnail>,
) -> Attachment {
    let mut att = Attachment::new(kind, convert_file(file));
    att.width = width;
    att.height = height;
    att.duration = duration;
    att.name = name.to_owned();
    att.mime = mime.to_owned();
    att.mini = mini(minithumb);
    let thumb = thumb_visual(thumbnail).filter(|t| t.decode == Decode::Raster);
    att.preview = thumb.or_else(|| {
        (att.file.size > 0 && att.file.size <= SMALL_VIDEO_BYTES).then(|| Visual {
            file: att.file.clone(),
            decode: Decode::Video,
            width,
            height,
        })
    });
    att.full = att.preview.clone();
    att
}

pub fn sticker_attachment(st: &types::Sticker) -> Attachment {
    let decode = match st.format {
        StickerFormat::Webp => Decode::Raster,
        StickerFormat::Tgs => Decode::Tgs,
        StickerFormat::Webm => Decode::Video,
    };
    let mut att = Attachment::new(MediaKind::Sticker, convert_file(&st.sticker));
    att.width = st.width;
    att.height = st.height;
    att.name = st.emoji.clone();
    let main = Visual {
        file: att.file.clone(),
        decode,
        width: st.width,
        height: st.height,
    };
    att.preview = match decode {
        Decode::Video => thumb_visual(&st.thumbnail)
            .filter(|t| t.decode == Decode::Raster)
            .or(Some(main.clone())),
        _ => Some(main.clone()),
    };
    att.full = Some(main);
    att
}

pub fn convert_content(content: MessageContent) -> Content {
    use MessageContent as C;
    match content {
        C::MessageText(t) => Content {
            media: None,
            body: convert_text(t.text),
            attachments: Vec::new(),
        },
        C::MessagePhoto(p) => match photo_attachment(&p.photo) {
            Some(att) => Content::captioned("[photo]", p.caption, att),
            None => Content {
                media: Some("[photo]".to_owned()),
                body: convert_text(p.caption),
                attachments: Vec::new(),
            },
        },
        C::MessageVideo(v) => {
            let x = &v.video;
            let att = video_like(
                MediaKind::Video,
                &x.video,
                x.width,
                x.height,
                x.duration,
                &x.file_name,
                &x.mime_type,
                &x.minithumbnail,
                &x.thumbnail,
            );
            Content::captioned(format!("[video {}]", duration(x.duration)), v.caption, att)
        }
        C::MessageAnimation(a) => {
            let x = &a.animation;
            let att = video_like(
                MediaKind::Animation,
                &x.animation,
                x.width,
                x.height,
                x.duration,
                &x.file_name,
                &x.mime_type,
                &x.minithumbnail,
                &x.thumbnail,
            );
            Content::captioned("[GIF]", a.caption, att)
        }
        C::MessageVideoNote(v) => {
            let x = &v.video_note;
            let mut att = video_like(
                MediaKind::VideoNote,
                &x.video,
                x.length,
                x.length,
                x.duration,
                "video_note.mp4",
                "video/mp4",
                &x.minithumbnail,
                &x.thumbnail,
            );
            if att.preview.is_none() {
                att.preview = Some(Visual {
                    file: att.file.clone(),
                    decode: Decode::Video,
                    width: x.length,
                    height: x.length,
                });
                att.full = att.preview.clone();
            }
            Content::captioned(
                format!("[video message {}]", duration(x.duration)),
                types::FormattedText::default(),
                att,
            )
        }
        C::MessageVoiceNote(v) => {
            let x = &v.voice_note;
            let mut att = Attachment::new(MediaKind::Voice, convert_file(&x.voice));
            att.duration = x.duration;
            att.mime = x.mime_type.clone();
            att.name = "voice.ogg".to_owned();
            att.waveform = crate::model::decode_waveform(&b64(&x.waveform));
            Content::captioned(format!("[voice {}]", duration(x.duration)), v.caption, att)
        }
        C::MessageAudio(a) => {
            let x = &a.audio;
            let title = match (x.performer.is_empty(), x.title.is_empty()) {
                (false, false) => format!("{} – {}", x.performer, x.title),
                (true, false) => x.title.clone(),
                _ => x.file_name.clone(),
            };
            let mut att = Attachment::new(MediaKind::Audio, convert_file(&x.audio));
            att.duration = x.duration;
            att.mime = x.mime_type.clone();
            att.name = if x.file_name.is_empty() {
                format!("{title}.mp3")
            } else {
                x.file_name.clone()
            };
            Content::captioned(format!("♪ {title}"), a.caption, att)
        }
        C::MessageDocument(d) => {
            let x = &d.document;
            let mut att = Attachment::new(MediaKind::Document, convert_file(&x.document));
            att.name = x.file_name.clone();
            att.mime = x.mime_type.clone();
            att.mini = mini(&x.minithumbnail);
            att.preview = thumb_visual(&x.thumbnail).filter(|t| t.decode == Decode::Raster);
            if let Some(t) = &att.preview {
                att.width = t.width;
                att.height = t.height;
            }
            att.full = att.preview.clone();
            Content::captioned(format!("[file {}]", x.file_name), d.caption, att)
        }
        C::MessageSticker(s) => Content {
            media: Some(format!("[sticker {}]", s.sticker.emoji)),
            body: RichText::default(),
            attachments: vec![sticker_attachment(&s.sticker)],
        },
        C::MessageAnimatedEmoji(e) => Content {
            media: None,
            body: RichText::plain(e.emoji),
            attachments: Vec::new(),
        },
        C::MessageLocation(l) => Content::label(format!(
            "📍 {}location {:.5}, {:.5}",
            if l.live_period > 0 { "live " } else { "" },
            l.location.latitude,
            l.location.longitude
        )),
        C::MessageVenue(v) => Content::label(format!(
            "📍 {} · {} ({:.5}, {:.5})",
            v.venue.title, v.venue.address, v.venue.location.latitude, v.venue.location.longitude
        )),
        C::MessageContact(c) => Content::label(format!(
            "👤 {} {} · {}",
            c.contact.first_name, c.contact.last_name, c.contact.phone_number
        )),
        C::MessagePoll(p) => {
            let poll = &p.poll;
            let mut text = format!(
                "📊 {}{}",
                poll.question.text,
                if poll.is_closed { " (closed)" } else { "" }
            );
            for o in &poll.options {
                let mark = if o.is_chosen { '●' } else { '○' };
                text.push_str(&format!(
                    "\n  {mark} {} — {}%",
                    o.text.text, o.vote_percentage
                ));
            }
            text.push_str(&format!("\n  {} votes", poll.total_voter_count));
            Content::label(text)
        }
        C::MessageDice(d) => Content::label(format!("{} rolled {}", d.emoji, d.value)),
        C::MessageGame(g) => Content::label(format!("🎮 game: {}", g.game.title)),
        C::MessageCall(c) => Content::label(format!("📞 call {}", duration(c.duration))),
        C::MessagePinMessage(_) => Content::label("pinned a message"),
        C::MessageChatAddMembers(m) => Content::label(if m.member_user_ids.len() == 1 {
            "added a member".to_owned()
        } else {
            format!("added {} members", m.member_user_ids.len())
        }),
        C::MessageChatJoinByLink | C::MessageChatJoinByRequest => Content::label("joined the chat"),
        C::MessageChatDeleteMember(_) => Content::label("left the chat"),
        C::MessageChatChangeTitle(t) => Content::label(format!("changed title to «{}»", t.title)),
        C::MessageChatChangePhoto(_) => Content::label("changed the chat photo"),
        C::MessageChatDeletePhoto => Content::label("removed the chat photo"),
        C::MessageChatSetBackground(_) => Content::label("changed the chat background"),
        C::MessageBasicGroupChatCreate(_) | C::MessageSupergroupChatCreate(_) => {
            Content::label("created the group")
        }
        C::MessageForumTopicCreated(t) => Content::label(format!("created topic «{}»", t.name)),
        C::MessageContactRegistered => Content::label("joined Telegram"),
        C::MessageScreenshotTaken => Content::label("took a screenshot"),
        C::MessageExpiredPhoto => Content::label("[expired photo]"),
        C::MessageExpiredVideo => Content::label("[expired video]"),
        C::MessageExpiredVideoNote => Content::label("[expired video message]"),
        C::MessageExpiredVoiceNote => Content::label("[expired voice message]"),
        C::MessageUnsupported => Content::label("[message not supported, open an official app]"),
        other => {
            let name = format!("{other:?}");
            let name = name
                .split(['(', ' ', '{'])
                .next()
                .unwrap_or("message")
                .trim_start_matches("Message");
            Content::label(format!("[{}]", split_camel(name)))
        }
    }
}

pub fn split_camel(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

pub fn input_sender(s: Sender) -> MessageSender {
    match s {
        Sender::User(user_id) => MessageSender::User(types::MessageSenderUser { user_id }),
        Sender::Chat(chat_id) => MessageSender::Chat(types::MessageSenderChat { chat_id }),
    }
}

pub fn convert_sender(s: &MessageSender) -> Sender {
    match s {
        MessageSender::User(u) => Sender::User(u.user_id),
        MessageSender::Chat(c) => Sender::Chat(c.chat_id),
    }
}

pub fn convert_message(m: types::Message) -> Msg {
    let story = matches!(m.content, enums::MessageContent::MessageStory(_));
    let info = m.interaction_info.as_ref();
    let meta = MsgMeta {
        edit_date: i64::from(m.edit_date),
        forward_date: m.forward_info.as_ref().map_or(0, |f| i64::from(f.date)),
        views: info.map_or(0, |i| i.view_count),
        forwards: info.map_or(0, |i| i.forward_count),
        replies: info
            .and_then(|i| i.reply_info.as_ref())
            .map_or(0, |r| r.reply_count),
        can_be_saved: m.can_be_saved,
        channel_post: m.is_channel_post,
        reply_chat: match &m.reply_to {
            Some(enums::MessageReplyTo::Message(r)) if r.chat_id != m.chat_id => r.chat_id,
            _ => 0,
        },
        via_bot: m.via_bot_user_id,
        signature: m.author_signature.clone(),
        story,
    };
    let content = convert_content(m.content);
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
        album: m.media_album_id,
        media: content.media,
        attachments: content.attachments,
        body: content.body,
        pinned: m.is_pinned,
        mention: m.contains_unread_mention,
        unread_reaction: !m.unread_reactions.is_empty(),
        reactions: convert_reactions(m.interaction_info.as_ref()),
        keyboard: convert_markup(m.reply_markup),
        forward: m.forward_info.map(|f| convert_origin(f.origin)),
        topic: msg_topic(&m.topic_id),
        ttl: match m.self_destruct_type {
            Some(enums::MessageSelfDestructType::Timer(t)) => t.self_destruct_time,
            Some(enums::MessageSelfDestructType::Immediately) => -1,
            None => 0,
        },
        deleted: false,
        meta,
    }
}

pub fn msg_topic(t: &Option<enums::MessageTopic>) -> Option<i32> {
    match t {
        Some(enums::MessageTopic::Forum(f)) => Some(f.forum_topic_id),
        _ => None,
    }
}

pub fn convert_perms(p: &types::ChatPermissions) -> Rights {
    Rights {
        manage_topics: false,
        create_topics: p.can_create_topics,
        pin: p.can_pin_messages,
        change_info: p.can_change_info,
    }
}

pub fn convert_membership(s: &enums::ChatMemberStatus) -> Membership {
    match s {
        enums::ChatMemberStatus::Creator(_) => Membership::Creator,
        enums::ChatMemberStatus::Administrator(a) => Membership::Admin(Rights {
            manage_topics: a.rights.can_manage_topics,
            create_topics: a.rights.can_manage_topics,
            pin: a.rights.can_pin_messages,
            change_info: a.rights.can_change_info,
        }),
        enums::ChatMemberStatus::Member(_) => Membership::Member,
        enums::ChatMemberStatus::Restricted(r) if r.is_member => {
            Membership::Restricted(convert_perms(&r.permissions))
        }
        _ => Membership::Left,
    }
}

pub fn convert_secret(s: &types::SecretChat) -> SecretChat {
    SecretChat {
        id: s.id,
        user_id: s.user_id,
        state: match s.state {
            enums::SecretChatState::Pending => SecretState::Pending,
            enums::SecretChatState::Ready => SecretState::Ready,
            enums::SecretChatState::Closed => SecretState::Closed,
        },
        outbound: s.is_outbound,
    }
}

pub fn convert_topic_info(i: types::ForumTopicInfo) -> Topic {
    Topic {
        id: i.forum_topic_id,
        name: i.name,
        icon: String::new(),
        color: i.icon.color,
        general: i.is_general,
        closed: i.is_closed,
        hidden: i.is_hidden,
        outgoing: i.is_outgoing,
        ..Topic::default()
    }
}

pub fn convert_topic(t: types::ForumTopic) -> Topic {
    let info = t.info;
    let muted = t.notification_settings.mute_for > 0;
    let use_default_mute = t.notification_settings.use_default_mute_for;
    Topic {
        pinned: t.is_pinned,
        order: t.order,
        unread: t.unread_count,
        mentions: t.unread_mention_count,
        reactions: t.unread_reaction_count,
        last_read_inbox: t.last_read_inbox_message_id,
        last_read_outbox: t.last_read_outbox_message_id,
        muted,
        use_default_mute,
        last_message: t.last_message.map(convert_message),
        ..convert_topic_info(info)
    }
}

fn topic_icon_ids(t: &types::ForumTopic) -> i64 {
    t.info.icon.custom_emoji_id
}

async fn emoji_for(ids: Vec<i64>, c: i32) -> std::collections::HashMap<i64, String> {
    custom_emoji_names(ids, c).await.unwrap_or_default()
}

async fn custom_emoji_names(
    ids: Vec<i64>,
    c: i32,
) -> Result<std::collections::HashMap<i64, String>, String> {
    let mut out = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let enums::Stickers::Stickers(list) = tdlib_rs::functions::get_custom_emoji_stickers(ids, c)
        .await
        .map_err(|e| friendly_error(&e))?;
    for st in list.stickers {
        if let enums::StickerFullType::CustomEmoji(e) = &st.full_type {
            out.insert(e.custom_emoji_id, st.emoji.clone());
        }
    }
    Ok(out)
}

async fn topics_with_icons(raw: Vec<types::ForumTopic>, c: i32) -> Vec<Topic> {
    let ids: Vec<i64> = raw
        .iter()
        .map(topic_icon_ids)
        .filter(|id| *id != 0)
        .collect();
    let emoji = emoji_for(ids, c).await;
    raw.into_iter()
        .map(|t| {
            let id = topic_icon_ids(&t);
            let mut topic = convert_topic(t);
            if let Some(e) = emoji.get(&id) {
                topic.icon = e.clone();
            }
            topic
        })
        .collect()
}

fn topic_settings(mute: bool) -> types::ChatNotificationSettings {
    types::ChatNotificationSettings {
        use_default_mute_for: false,
        mute_for: if mute { MUTE_FOREVER } else { 0 },
        use_default_sound: true,
        sound_id: 0,
        use_default_show_preview: true,
        show_preview: true,
        use_default_mute_stories: true,
        mute_stories: false,
        use_default_story_sound: true,
        story_sound_id: 0,
        use_default_show_story_poster: true,
        show_story_poster: true,
        use_default_disable_pinned_message_notifications: true,
        disable_pinned_message_notifications: false,
        use_default_disable_mention_notifications: true,
        disable_mention_notifications: false,
    }
}

const MUTE_FOREVER: i32 = 367 * 86_400;
pub const TOPIC_ICON_COLOR: i32 = 0x6F_B9_F0;

pub fn convert_origin(o: enums::MessageOrigin) -> Origin {
    match o {
        enums::MessageOrigin::User(u) => Origin::Sender(Sender::User(u.sender_user_id)),
        enums::MessageOrigin::HiddenUser(u) => Origin::Name(u.sender_name),
        enums::MessageOrigin::Chat(c) => Origin::Sender(Sender::Chat(c.sender_chat_id)),
        enums::MessageOrigin::Channel(c) => Origin::Sender(Sender::Chat(c.chat_id)),
    }
}

pub fn convert_reactions(info: Option<&types::MessageInteractionInfo>) -> Vec<Reaction> {
    let Some(r) = info.and_then(|i| i.reactions.as_ref()) else {
        return Vec::new();
    };
    r.reactions
        .iter()
        .map(|x| {
            let (label, emoji, custom) = match &x.r#type {
                enums::ReactionType::Emoji(e) => (e.emoji.clone(), Some(e.emoji.clone()), 0),
                enums::ReactionType::CustomEmoji(c) => (
                    crate::format::EMOJI_PLACEHOLDER.to_owned(),
                    None,
                    c.custom_emoji_id,
                ),
                enums::ReactionType::Paid => ("⭐".to_owned(), None, 0),
            };
            Reaction {
                label,
                emoji,
                custom,
                count: x.total_count,
                chosen: x.is_chosen,
            }
        })
        .collect()
}

pub fn convert_markup(markup: Option<enums::ReplyMarkup>) -> Option<Keyboard> {
    use enums::InlineKeyboardButtonType as I;
    use enums::KeyboardButtonType as K;
    match markup? {
        enums::ReplyMarkup::InlineKeyboard(k) => Some(Keyboard {
            inline: true,
            placeholder: String::new(),
            rows: k
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|b| Button {
                            text: b.text,
                            action: match b.r#type {
                                I::Callback(c) => ButtonAction::Callback(c.data),
                                I::Url(u) => ButtonAction::Url(u.url),
                                I::LoginUrl(u) => ButtonAction::Url(u.url),
                                I::WebApp(u) => ButtonAction::Url(u.url),
                                I::User(u) => ButtonAction::User(u.user_id),
                                I::CopyText(t) => ButtonAction::CopyText(t.text),
                                I::CallbackWithPassword(_) => {
                                    ButtonAction::Unsupported("password-protected buttons")
                                }
                                I::CallbackGame => ButtonAction::Unsupported("games"),
                                I::SwitchInline(_) => ButtonAction::Unsupported("inline mode"),
                                I::Buy => ButtonAction::Unsupported("payments"),
                            },
                        })
                        .collect()
                })
                .collect(),
        }),
        enums::ReplyMarkup::ShowKeyboard(k) => Some(Keyboard {
            inline: false,
            placeholder: k.input_field_placeholder,
            rows: k
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|b| Button {
                            text: b.text,
                            action: match b.r#type {
                                K::Text => ButtonAction::Text,
                                K::WebApp(u) => ButtonAction::Url(u.url),
                                K::RequestPhoneNumber => {
                                    ButtonAction::Unsupported("sharing your phone number")
                                }
                                K::RequestLocation => {
                                    ButtonAction::Unsupported("sharing your location")
                                }
                                K::RequestPoll(_) => ButtonAction::Unsupported("creating polls"),
                                K::RequestUsers(_) | K::RequestChat(_) => {
                                    ButtonAction::Unsupported("sharing chats")
                                }
                            },
                        })
                        .collect()
                })
                .collect(),
        }),
        enums::ReplyMarkup::RemoveKeyboard(_) | enums::ReplyMarkup::ForceReply(_) => None,
    }
}

pub fn convert_status(s: &enums::UserStatus) -> Status {
    match s {
        enums::UserStatus::Empty => Status::Unknown,
        enums::UserStatus::Online(o) => Status::Online {
            expires: i64::from(o.expires),
        },
        enums::UserStatus::Offline(o) => Status::Offline {
            was_online: i64::from(o.was_online),
        },
        enums::UserStatus::Recently(_) => Status::Recently,
        enums::UserStatus::LastWeek(_) => Status::LastWeek,
        enums::UserStatus::LastMonth(_) => Status::LastMonth,
    }
}

pub fn convert_activity(a: &enums::ChatAction) -> Option<Activity> {
    use enums::ChatAction as A;
    Some(match a {
        A::Cancel => return None,
        A::Typing => Activity::Typing,
        A::RecordingVoiceNote | A::UploadingVoiceNote(_) => Activity::RecordingVoice,
        A::RecordingVideo => Activity::RecordingVideo,
        A::RecordingVideoNote | A::UploadingVideoNote(_) => Activity::RecordingVideoNote,
        A::UploadingPhoto(_) => Activity::UploadingPhoto,
        A::UploadingVideo(_) => Activity::UploadingVideo,
        A::UploadingDocument(_) => Activity::UploadingFile,
        A::ChoosingSticker => Activity::ChoosingSticker,
        _ => Activity::Other,
    })
}

pub fn user_meta(u: &types::User) -> crate::model::UserMeta {
    crate::model::UserMeta {
        status: convert_status(&u.status),
        bot: matches!(u.r#type, enums::UserType::Bot(_)),
        usernames: u
            .usernames
            .as_ref()
            .map(|n| n.active_usernames.clone())
            .unwrap_or_default(),
        photo: u.profile_photo.as_ref().map(|p| convert_file(&p.small)),
    }
}

fn media_filter(f: crate::model::MediaFilter) -> enums::SearchMessagesFilter {
    use crate::model::MediaFilter as M;
    match f {
        M::Photos => enums::SearchMessagesFilter::Photo,
        M::Videos => enums::SearchMessagesFilter::Video,
        M::Files => enums::SearchMessagesFilter::Document,
        M::Links => enums::SearchMessagesFilter::Url,
        M::Voice => enums::SearchMessagesFilter::VoiceNote,
        M::Gifs => enums::SearchMessagesFilter::Animation,
        M::Music => enums::SearchMessagesFilter::Audio,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn plain(id: i64) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: Sender::User(2),
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

    type Page = std::future::Ready<Result<Vec<Msg>, String>>;

    fn scripted(
        pages: Vec<Result<Vec<i64>, String>>,
    ) -> (std::rc::Rc<std::cell::Cell<usize>>, impl FnMut() -> Page) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = calls.clone();
        let mut pages = pages.into_iter();
        let fetch = move || {
            seen.set(seen.get() + 1);
            let page = pages
                .next()
                .expect("no more scripted pages")
                .map(|ids| ids.into_iter().rev().map(plain).collect());
            std::future::ready(page)
        };
        (calls, fetch)
    }

    #[test]
    fn around_window_retries_until_the_target_is_in_the_page() {
        let (calls, fetch) = scripted(vec![
            Ok(vec![]),
            Ok((90..=99).collect()),
            Ok((80..=119).collect()),
        ]);
        let got = futures::executor::block_on(around_window(plain(100), fetch)).unwrap();
        assert_eq!(calls.get(), 3);
        let ids: Vec<i64> = got.iter().map(|m| m.id).collect();
        assert_eq!(ids, (80..=119).collect::<Vec<_>>());
        let (calls, fetch) = scripted(vec![Ok((95..=130).collect())]);
        let got = futures::executor::block_on(around_window(plain(100), fetch)).unwrap();
        assert_eq!(calls.get(), 1, "stops as soon as the target arrives");
        assert_eq!(got.first().map(|m| m.id), Some(95));
    }

    #[test]
    fn around_window_keeps_the_confirmed_target_when_pages_miss_it() {
        let (calls, fetch) = scripted(vec![Ok(vec![1, 2]), Ok(vec![2, 3]), Ok(vec![])]);
        let got = futures::executor::block_on(around_window(plain(100), fetch)).unwrap();
        assert_eq!(calls.get(), AROUND_ATTEMPTS);
        let ids: Vec<i64> = got.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 100]);
        let (_, fetch) = scripted(vec![Ok(vec![1]), Err("flood wait".to_owned())]);
        assert_eq!(
            futures::executor::block_on(around_window(plain(100), fetch)),
            Err("flood wait".to_owned())
        );
    }

    fn file(id: i32, size: i64, done: bool) -> Value {
        json!({
            "@type": "file", "id": id, "size": size, "expected_size": size,
            "local": {
                "@type": "localFile",
                "path": if done { format!("/tmp/f{id}") } else { String::new() },
                "can_be_downloaded": true, "can_be_deleted": false,
                "is_downloading_active": false, "is_downloading_completed": done,
                "download_offset": 0, "downloaded_prefix_size": 0,
                "downloaded_size": if done { size } else { 0 }
            },
            "remote": {
                "@type": "remoteFile", "id": "r", "unique_id": "u",
                "is_uploading_active": false, "is_uploading_completed": true, "uploaded_size": size
            }
        })
    }

    fn thumb(format: &str, id: i32) -> Value {
        json!({"@type": "thumbnail", "format": {"@type": format}, "width": 320, "height": 180, "file": file(id, 5000, false)})
    }

    fn caption(text: &str) -> Value {
        json!({"@type": "formattedText", "text": text, "entities": []})
    }

    fn content(v: Value) -> Content {
        convert_content(serde_json::from_value(v).expect("valid MessageContent JSON"))
    }

    #[test]
    fn photo_picks_inline_and_full_sizes() {
        let size = |t: &str, id: i32, w: i32, h: i32| json!({"@type": "photoSize", "type": t, "photo": file(id, i64::from(w) * 10, false), "width": w, "height": h, "progressive_sizes": []});
        let c = content(json!({
            "@type": "messagePhoto",
            "photo": {
                "@type": "photo", "has_stickers": false,
                "minithumbnail": {"@type": "minithumbnail", "width": 40, "height": 30, "data": "AQID"},
                "sizes": [size("m", 1, 320, 240), size("y", 3, 1280, 960), size("x", 2, 800, 600)]
            },
            "caption": caption("look"),
            "show_caption_above_media": false, "has_spoiler": false, "is_secret": false
        }));
        assert_eq!(c.body.text, "look");
        let a = &c.attachments[0];
        assert_eq!(a.kind, MediaKind::Photo);
        assert_eq!(a.preview.as_ref().unwrap().file.id, 2);
        assert_eq!(a.full.as_ref().unwrap().file.id, 3);
        assert_eq!(a.file.id, 3);
        assert_eq!((a.width, a.height), (1280, 960));
        assert_eq!(a.mini.as_deref(), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn video_uses_image_thumbnail_or_small_file() {
        let video = |thumbnail: Value, size: i64| {
            json!({
                "@type": "messageVideo",
                "video": {
                    "@type": "video", "duration": 83, "width": 1280, "height": 720,
                    "file_name": "clip.mp4", "mime_type": "video/mp4", "has_stickers": false,
                    "supports_streaming": true, "minithumbnail": null, "thumbnail": thumbnail,
                    "video": file(9, size, false)
                },
                "alternative_videos": [], "storyboards": [], "cover": null, "start_timestamp": 0,
                "caption": caption(""), "show_caption_above_media": false,
                "has_spoiler": false, "is_secret": false
            })
        };
        let c = content(video(thumb("thumbnailFormatJpeg", 10), 50_000_000));
        let a = &c.attachments[0];
        assert_eq!(a.kind, MediaKind::Video);
        assert_eq!(a.duration, 83);
        assert_eq!(a.preview.as_ref().unwrap().file.id, 10);
        assert_eq!(a.preview.as_ref().unwrap().decode, Decode::Raster);
        assert_eq!(c.media.as_deref(), Some("[video 1:23]"));
        let big = content(video(thumb("thumbnailFormatMpeg4", 10), 50_000_000));
        assert!(big.attachments[0].preview.is_none());
        let small = content(video(Value::Null, 300_000));
        let p = small.attachments[0].preview.as_ref().unwrap();
        assert_eq!((p.file.id, p.decode), (9, Decode::Video));
    }

    #[test]
    fn stickers_map_formats() {
        let sticker = |format: &str, thumbnail: Value| {
            json!({
                "@type": "messageSticker", "is_premium": false,
                "sticker": {
                    "@type": "sticker", "id": "1", "set_id": "2", "width": 512, "height": 512,
                    "emoji": "😀", "format": {"@type": format},
                    "full_type": {"@type": "stickerFullTypeRegular", "premium_animation": null},
                    "thumbnail": thumbnail, "sticker": file(5, 30_000, true)
                }
            })
        };
        let tgs = content(sticker("stickerFormatTgs", Value::Null));
        let a = &tgs.attachments[0];
        assert_eq!(a.kind, MediaKind::Sticker);
        assert_eq!(a.preview.as_ref().unwrap().decode, Decode::Tgs);
        assert_eq!(a.file.local(), Some("/tmp/f5"));
        assert_eq!(a.name, "😀");
        let webm = content(sticker(
            "stickerFormatWebm",
            thumb("thumbnailFormatWebp", 6),
        ));
        let a = &webm.attachments[0];
        assert_eq!(a.preview.as_ref().unwrap().file.id, 6);
        assert_eq!(a.full.as_ref().unwrap().decode, Decode::Video);
        let webm_bare = content(sticker("stickerFormatWebm", Value::Null));
        assert_eq!(
            webm_bare.attachments[0].preview.as_ref().unwrap().decode,
            Decode::Video
        );
        let webp = content(sticker("stickerFormatWebp", Value::Null));
        assert_eq!(
            webp.attachments[0].preview.as_ref().unwrap().decode,
            Decode::Raster
        );
    }

    #[test]
    fn voice_document_and_note() {
        let voice = content(json!({
            "@type": "messageVoiceNote",
            "voice_note": {
                "@type": "voiceNote", "duration": 7, "waveform": "/wE=", "mime_type": "audio/ogg",
                "speech_recognition_result": null, "voice": file(1, 9000, false)
            },
            "caption": caption(""), "is_listened": false
        }));
        let a = &voice.attachments[0];
        assert_eq!(a.kind, MediaKind::Voice);
        assert!(!a.is_visual());
        assert_eq!(a.waveform, vec![31, 15, 0]);
        let doc = content(json!({
            "@type": "messageDocument",
            "document": {
                "@type": "document", "file_name": "report.pdf", "mime_type": "application/pdf",
                "minithumbnail": null, "thumbnail": null, "document": file(4, 123_456, false)
            },
            "caption": caption("q3")
        }));
        let a = &doc.attachments[0];
        assert_eq!(
            (a.kind, a.name.as_str()),
            (MediaKind::Document, "report.pdf")
        );
        assert!(!a.viewable());
        assert_eq!(doc.body.text, "q3");
        let note = content(json!({
            "@type": "messageVideoNote",
            "video_note": {
                "@type": "videoNote", "duration": 12, "waveform": "", "length": 384,
                "minithumbnail": null, "thumbnail": thumb("thumbnailFormatJpeg", 8),
                "speech_recognition_result": null, "video": file(7, 900_000, false)
            },
            "is_viewed": false, "is_secret": false
        }));
        let a = &note.attachments[0];
        assert_eq!(a.kind, MediaKind::VideoNote);
        assert_eq!((a.width, a.height), (384, 384));
        assert_eq!(a.preview.as_ref().unwrap().file.id, 8);
    }

    #[test]
    fn converts_keyboards_reactions_and_status() {
        let style = json!({"@type": "buttonStyleDefault"});
        let ib = |text: &str, t: Value| json!({"@type": "inlineKeyboardButton", "text": text, "icon_custom_emoji_id": "0", "style": style.clone(), "type": t});
        let inline: enums::ReplyMarkup = serde_json::from_value(json!({
            "@type": "replyMarkupInlineKeyboard",
            "rows": [
                [ib("Yes", json!({"@type": "inlineKeyboardButtonTypeCallback", "data": "eWVz"})),
                 ib("Site", json!({"@type": "inlineKeyboardButtonTypeUrl", "url": "https://x.org"}))],
                [ib("Pay", json!({"@type": "inlineKeyboardButtonTypeBuy"})),
                 ib("Copy", json!({"@type": "inlineKeyboardButtonTypeCopyText", "text": "code"}))]
            ]
        }))
        .expect("inline markup json");
        let kb = convert_markup(Some(inline)).unwrap();
        assert!(kb.inline);
        assert_eq!(kb.buttons().len(), 4);
        assert_eq!(kb.rows[0][0].action, ButtonAction::Callback("eWVz".into()));
        assert_eq!(
            kb.rows[0][1].action,
            ButtonAction::Url("https://x.org".into())
        );
        assert_eq!(kb.rows[1][0].action, ButtonAction::Unsupported("payments"));
        assert_eq!(kb.rows[1][1].action, ButtonAction::CopyText("code".into()));
        let kbtn = |text: &str, t: &str| json!({"@type": "keyboardButton", "text": text, "icon_custom_emoji_id": "0", "style": style.clone(), "type": {"@type": t}});
        let reply: enums::ReplyMarkup = serde_json::from_value(json!({
            "@type": "replyMarkupShowKeyboard",
            "rows": [[kbtn("Hi", "keyboardButtonTypeText"), kbtn("Phone", "keyboardButtonTypeRequestPhoneNumber")]],
            "is_persistent": false, "resize_keyboard": true, "one_time": false,
            "is_personal": false, "input_field_placeholder": "pick"
        }))
        .expect("reply markup json");
        let kb = convert_markup(Some(reply)).unwrap();
        assert!(!kb.inline);
        assert_eq!(kb.placeholder, "pick");
        assert_eq!(kb.rows[0][0].action, ButtonAction::Text);
        assert!(matches!(kb.rows[0][1].action, ButtonAction::Unsupported(_)));
        let remove: enums::ReplyMarkup = serde_json::from_value(
            json!({"@type": "replyMarkupRemoveKeyboard", "is_personal": false}),
        )
        .expect("remove json");
        assert!(convert_markup(Some(remove)).is_none());
        assert!(convert_markup(None).is_none());
        let info: types::MessageInteractionInfo = serde_json::from_value(json!({
            "@type": "messageInteractionInfo", "view_count": 0, "forward_count": 0, "reply_info": null,
            "reactions": {
                "@type": "messageReactions", "are_tags": false, "paid_reactors": [],
                "can_get_added_reactions": true,
                "reactions": [
                    {"@type": "messageReaction", "type": {"@type": "reactionTypeEmoji", "emoji": "👍"},
                     "total_count": 4, "is_chosen": true, "used_sender_id": null, "recent_sender_ids": []},
                    {"@type": "messageReaction", "type": {"@type": "reactionTypeCustomEmoji", "custom_emoji_id": "5"},
                     "total_count": 1, "is_chosen": false, "used_sender_id": null, "recent_sender_ids": []},
                    {"@type": "messageReaction", "type": {"@type": "reactionTypePaid"},
                     "total_count": 2, "is_chosen": false, "used_sender_id": null, "recent_sender_ids": []}
                ]
            }
        }))
        .expect("interaction json");
        let r = convert_reactions(Some(&info));
        assert_eq!(r.len(), 3);
        assert_eq!(
            (r[0].label.as_str(), r[0].count, r[0].chosen),
            ("👍", 4, true)
        );
        assert_eq!(r[0].emoji.as_deref(), Some("👍"));
        assert_eq!((r[1].label.as_str(), r[1].emoji.as_deref()), ("•", None));
        assert_eq!((r[0].custom, r[1].custom, r[2].custom), (0, 5, 0));
        assert_eq!(r[2].label, "⭐");
        assert!(convert_reactions(None).is_empty());
        let st = |v: Value| convert_status(&serde_json::from_value(v).expect("status json"));
        assert_eq!(
            st(json!({"@type": "userStatusOnline", "expires": 50})),
            Status::Online { expires: 50 }
        );
        assert_eq!(
            st(json!({"@type": "userStatusOffline", "was_online": 40})),
            Status::Offline { was_online: 40 }
        );
        assert_eq!(
            st(json!({"@type": "userStatusRecently", "by_my_privacy_settings": false})),
            Status::Recently
        );
        assert_eq!(st(json!({"@type": "userStatusEmpty"})), Status::Unknown);
        let act = |t: &str| {
            convert_activity(&serde_json::from_value(json!({"@type": t})).expect("action json"))
        };
        assert_eq!(act("chatActionTyping"), Some(Activity::Typing));
        assert_eq!(
            act("chatActionRecordingVoiceNote"),
            Some(Activity::RecordingVoice)
        );
        assert_eq!(act("chatActionCancel"), None);
    }

    #[test]
    fn reaction_choices_skip_premium_and_duplicates() {
        let a: types::AvailableReactions = serde_json::from_value(json!({
            "@type": "availableReactions",
            "top_reactions": [
                {"@type": "availableReaction", "type": {"@type": "reactionTypeEmoji", "emoji": "👍"}, "needs_premium": false},
                {"@type": "availableReaction", "type": {"@type": "reactionTypeEmoji", "emoji": "🦄"}, "needs_premium": true}
            ],
            "recent_reactions": [
                {"@type": "availableReaction", "type": {"@type": "reactionTypeEmoji", "emoji": "👍"}, "needs_premium": false},
                {"@type": "availableReaction", "type": {"@type": "reactionTypeCustomEmoji", "custom_emoji_id": "9"}, "needs_premium": false}
            ],
            "popular_reactions": [
                {"@type": "availableReaction", "type": {"@type": "reactionTypeEmoji", "emoji": "🔥"}, "needs_premium": false}
            ],
            "allow_custom_emoji": false, "are_tags": false, "unavailability_reason": null
        }))
        .expect("available reactions json");
        let c = reaction_choices(&a);
        assert_eq!(c.emojis, ["👍", "🔥"]);
        assert_eq!(c.note, None);
    }

    #[test]
    fn other_content_is_one_readable_line() {
        let loc = content(json!({
            "@type": "messageLocation",
            "location": {"@type": "location", "latitude": 55.75, "longitude": 37.62, "horizontal_accuracy": 0.0},
            "live_period": 0, "expires_in": 0, "heading": 0, "proximity_alert_radius": 0
        }));
        assert_eq!(loc.media.as_deref(), Some("📍 location 55.75000, 37.62000"));
        let pin = content(json!({"@type": "messagePinMessage", "message_id": 5}));
        assert_eq!(pin.media.as_deref(), Some("pinned a message"));
        let join = content(json!({"@type": "messageChatJoinByLink"}));
        assert_eq!(join.media.as_deref(), Some("joined the chat"));
        let dice = content(json!({
            "@type": "messageDice", "initial_state": null, "final_state": null,
            "emoji": "🎲", "value": 4, "success_animation_frame_number": 0
        }));
        assert_eq!(dice.media.as_deref(), Some("🎲 rolled 4"));
        let theme = content(json!({"@type": "messageScreenshotTaken"}));
        assert_eq!(theme.media.as_deref(), Some("took a screenshot"));
        assert_eq!(split_camel("ChatSetTheme"), "chat set theme");
        assert!(loc.attachments.is_empty());
    }

    fn message_json(id: i64, topic: Value, destruct: Value) -> Value {
        json!({
            "@type": "message",
            "id": id,
            "sender_id": {"@type": "messageSenderUser", "user_id": 2},
            "chat_id": -100,
            "is_outgoing": false,
            "date": 1_700_000_000,
            "topic_id": topic,
            "self_destruct_type": destruct,
            "content": {"@type": "messageText", "text": caption("hi")},
            "is_pinned": false, "is_from_offline": false, "can_be_saved": true,
            "has_timestamped_media": false, "is_channel_post": false,
            "is_paid_star_suggested_post": false, "is_paid_ton_suggested_post": false,
            "contains_unread_mention": false, "edit_date": 0, "unread_reactions": [],
            "self_destruct_in": 0.0, "auto_delete_in": 0.0, "via_bot_user_id": 0,
            "sender_business_bot_user_id": 0, "sender_boost_count": 0,
            "paid_message_star_count": 0, "author_signature": "", "media_album_id": "0",
            "effect_id": "0", "summary_language_code": "",
        })
    }

    fn notification_json(mute_for: i32) -> Value {
        json!({
            "@type": "chatNotificationSettings", "use_default_mute_for": false, "mute_for": mute_for,
            "use_default_sound": true, "sound_id": "0", "use_default_show_preview": true,
            "show_preview": true, "use_default_mute_stories": true, "mute_stories": false,
            "use_default_story_sound": true, "story_sound_id": "0",
            "use_default_show_story_poster": true, "show_story_poster": true,
            "use_default_disable_pinned_message_notifications": true,
            "disable_pinned_message_notifications": false,
            "use_default_disable_mention_notifications": true,
            "disable_mention_notifications": false,
        })
    }

    #[test]
    fn history_with_chat_background_without_background_object_loads() {
        let mut bg = message_json(9, Value::Null, Value::Null);
        bg["content"] = json!({
            "@type": "messageChatSetBackground",
            "old_background_message_id": 0,
            "background": {"@type": "chatBackground", "dark_theme_dimming": 0},
            "only_for_self": false
        });
        let page: enums::Messages = serde_json::from_value(json!({
            "@type": "messages",
            "total_count": 2,
            "messages": [bg, message_json(8, Value::Null, Value::Null)]
        }))
        .expect("messages JSON with a background-less chatBackground");
        let enums::Messages::Messages(page) = page;
        let msgs: Vec<Msg> = page
            .messages
            .into_iter()
            .flatten()
            .map(convert_message)
            .collect();
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0].media.as_deref(),
            Some("changed the chat background")
        );
        assert_eq!(msgs[1].body.text, "hi");
    }

    #[test]
    fn history_with_gift_without_background_loads() {
        let file = json!({
            "@type": "file", "id": 1, "size": 0, "expected_size": 0,
            "local": {"@type": "localFile", "path": "", "can_be_downloaded": true,
                "can_be_deleted": false, "is_downloading_active": false,
                "is_downloading_completed": false, "download_offset": 0,
                "downloaded_prefix_size": 0, "downloaded_size": 0},
            "remote": {"@type": "remoteFile", "id": "", "unique_id": "",
                "is_uploading_active": false, "is_uploading_completed": true, "uploaded_size": 0},
        });
        let sender = json!({"@type": "messageSenderUser", "user_id": 7});
        let mut gift = message_json(9, Value::Null, Value::Null);
        gift["content"] = json!({
            "@type": "messageGift",
            "gift": {
                "@type": "gift", "id": "1", "publisher_chat_id": 0,
                "sticker": {"@type": "sticker", "id": "2", "set_id": "3", "width": 512,
                    "height": 512, "emoji": "🎁", "format": {"@type": "stickerFormatTgs"},
                    "full_type": {"@type": "stickerFullTypeRegular"}, "sticker": file},
                "star_count": 15, "default_sell_star_count": 13, "upgrade_star_count": 0,
                "upgrade_variant_count": 0, "has_colors": false, "is_for_birthday": false,
                "is_premium": false, "next_send_date": 0, "first_send_date": 0,
                "last_send_date": 0
            },
            "sender_id": sender, "receiver_id": sender, "received_gift_id": "",
            "text": {"@type": "formattedText", "text": "", "entities": []},
            "unique_gift_number": 0, "sell_star_count": 13, "prepaid_upgrade_star_count": 0,
            "is_upgrade_separate": false, "is_from_auction": false, "is_private": false,
            "is_saved": false, "is_prepaid_upgrade": false, "can_be_upgraded": false,
            "was_converted": false, "was_upgraded": false, "was_refunded": false,
            "upgraded_received_gift_id": "", "prepaid_upgrade_hash": ""
        });
        let page: enums::Messages = serde_json::from_value(json!({
            "@type": "messages",
            "total_count": 2,
            "messages": [gift, message_json(8, Value::Null, Value::Null)]
        }))
        .expect("messages JSON with a gift that has no background");
        let enums::Messages::Messages(page) = page;
        let msgs: Vec<Msg> = page
            .messages
            .into_iter()
            .flatten()
            .map(convert_message)
            .collect();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].body.text, "hi");
    }

    #[test]
    fn messages_carry_forum_topic_and_self_destruct_timer() {
        let m: types::Message = serde_json::from_value(message_json(
            5,
            json!({"@type": "messageTopicForum", "forum_topic_id": 7}),
            json!({"@type": "messageSelfDestructTypeTimer", "self_destruct_time": 30}),
        ))
        .expect("message JSON");
        let msg = convert_message(m);
        assert_eq!(msg.topic, Some(7));
        assert_eq!(msg.ttl, 30);
        let once: types::Message = serde_json::from_value(message_json(
            6,
            json!({"@type": "messageTopicThread", "message_thread_id": 3}),
            json!({"@type": "messageSelfDestructTypeImmediately"}),
        ))
        .expect("message JSON");
        let once = convert_message(once);
        assert_eq!(once.topic, None);
        assert_eq!(once.ttl, -1);
        let plain: types::Message =
            serde_json::from_value(message_json(7, Value::Null, Value::Null)).expect("JSON");
        let plain = convert_message(plain);
        assert_eq!((plain.topic, plain.ttl), (None, 0));
    }

    #[test]
    fn forum_topics_secret_chats_and_rights_convert() {
        let t: types::ForumTopic = serde_json::from_value(json!({
            "@type": "forumTopic",
            "info": {"@type": "forumTopicInfo", "chat_id": -100, "forum_topic_id": 9, "name": "Releases",
                     "icon": {"@type": "forumTopicIcon", "color": 0x6FB9F0, "custom_emoji_id": "0"},
                     "creation_date": 1, "creator_id": {"@type": "messageSenderUser", "user_id": 1},
                     "is_general": false, "is_outgoing": true, "is_closed": true, "is_hidden": false, "is_name_implicit": false},
            "order": "12345",
            "is_pinned": true,
            "unread_count": 4,
            "last_read_inbox_message_id": 10,
            "last_read_outbox_message_id": 11,
            "unread_mention_count": 1,
            "unread_reaction_count": 0,
            "notification_settings": notification_json(100),
        }))
        .expect("topic JSON");
        let t = convert_topic(t);
        assert_eq!((t.id, t.name.as_str(), t.order), (9, "Releases", 12345));
        assert!(t.pinned && t.closed && t.outgoing && t.muted && !t.use_default_mute);
        assert_eq!((t.unread, t.mentions, t.last_read_inbox), (4, 1, 10));
        assert_eq!(t.color, 0x6FB9F0);

        let sc: types::SecretChat = serde_json::from_value(json!({
            "@type": "secretChat", "id": 3, "user_id": 2,
            "state": {"@type": "secretChatStatePending"}, "is_outbound": true, "key_hash": "", "layer": 144,
        }))
        .expect("secret chat JSON");
        assert_eq!(
            convert_secret(&sc),
            SecretChat {
                id: 3,
                user_id: 2,
                state: SecretState::Pending,
                outbound: true
            }
        );

        let admin: enums::ChatMemberStatus = serde_json::from_value(json!({
            "@type": "chatMemberStatusAdministrator", "custom_title": "", "can_be_edited": false,
            "rights": {"@type": "chatAdministratorRights", "can_manage_chat": false, "can_change_info": true, "can_post_messages": false, "can_edit_messages": false, "can_delete_messages": false, "can_invite_users": false, "can_restrict_members": false, "can_pin_messages": false, "can_manage_topics": true, "can_promote_members": false, "can_manage_video_chats": false, "can_post_stories": false, "can_edit_stories": false, "can_delete_stories": false, "can_manage_direct_messages": false, "is_anonymous": false},
        }))
        .expect("admin JSON");
        assert_eq!(
            convert_membership(&admin),
            Membership::Admin(Rights {
                manage_topics: true,
                create_topics: true,
                pin: false,
                change_info: true
            })
        );
        let left: enums::ChatMemberStatus = serde_json::from_value(json!({
            "@type": "chatMemberStatusRestricted", "is_member": false, "restricted_until_date": 0,
            "permissions": {"@type": "chatPermissions", "can_send_basic_messages": false, "can_send_audios": false, "can_send_documents": false, "can_send_photos": false, "can_send_videos": false, "can_send_video_notes": false, "can_send_voice_notes": false, "can_send_polls": false, "can_send_other_messages": false, "can_add_link_previews": false, "can_change_info": false, "can_invite_users": false, "can_pin_messages": false, "can_create_topics": false},
        }))
        .expect("restricted JSON");
        assert_eq!(convert_membership(&left), Membership::Left);
        assert_eq!(
            message_topic(Some(4)),
            Some(enums::MessageTopic::Forum(types::MessageTopicForum {
                forum_topic_id: 4
            }))
        );
        assert_eq!(message_topic(None), None);
    }
}
