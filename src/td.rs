use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::Path;

use tdlib_rs::enums::{
    self, ChatList, MessageContent, MessageSender, StickerFormat, TextEntityType, ThumbnailFormat,
    Update,
};
use tdlib_rs::types;
use tokio::sync::mpsc::UnboundedSender;

use crate::format::duration;
use crate::model::{
    Attachment, ChatInfo, ChatKind, Decode, Entity, EntityKind, FileRef, ListId, MediaKind, Msg,
    Position, RichText, SendState, Sender, Visual,
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
    File(FileRef),
    DownloadFailed {
        file_id: i32,
        error: String,
    },
    Image(crate::media::Done),
    Player {
        token: u64,
        result: Result<(), String>,
    },
    Saved(Result<String, String>),
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

pub fn convert_sender(s: &MessageSender) -> Sender {
    match s {
        MessageSender::User(u) => Sender::User(u.user_id),
        MessageSender::Chat(c) => Sender::Chat(c.chat_id),
    }
}

pub fn convert_message(m: types::Message) -> Msg {
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
}
