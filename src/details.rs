use crate::format;
use crate::model::{MediaKind, Msg, Origin, Sender};
use crate::td::Flags;

pub struct Ctx {
    pub sender: String,
    pub chat: String,
    pub origin: Option<String>,
    pub reply: Option<String>,
    pub flags: Option<Result<Flags, String>>,
    pub protected: bool,
    pub exact: bool,
    pub edits: usize,
}

fn yes(v: bool) -> String {
    if v { "yes" } else { "no" }.to_owned()
}

pub fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Video => "video",
        MediaKind::Animation => "GIF",
        MediaKind::VideoNote => "video message",
        MediaKind::Voice => "voice message",
        MediaKind::Audio => "audio",
        MediaKind::Sticker => "sticker",
        MediaKind::Document => "file",
    }
}

pub fn sender_id(s: Sender) -> String {
    match s {
        Sender::User(id) => format!("{id} (user)"),
        Sender::Chat(id) => format!("{id} (chat)"),
    }
}

pub fn lines(m: &Msg, c: &Ctx) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, v: String| out.push((k.to_owned(), v));
    put("Message ID", m.id.to_string());
    put("Chat", format!("{} · {}", m.chat_id, c.chat));
    put("Sender", c.sender.clone());
    put("Sender ID", sender_id(m.sender));
    put("Sent", format::stamp(m.date));
    if m.meta.edit_date > 0 {
        put("Edited", format::stamp(m.meta.edit_date));
    } else if m.edited {
        put("Edited", "yes".to_owned());
    }
    if c.edits > 0 {
        put("Saved versions", c.edits.to_string());
    }
    if m.deleted {
        put("Deleted", "yes, kept only on this device".to_owned());
    }
    if let Some(t) = m.topic {
        put("Topic ID", t.to_string());
    }
    if let Some(o) = &m.forward {
        let id = match o {
            Origin::Sender(s) => format!(" · {}", sender_id(*s)),
            Origin::Name(_) => String::new(),
        };
        put(
            "Forwarded from",
            format!("{}{id}", c.origin.clone().unwrap_or_default()),
        );
        if m.meta.forward_date > 0 {
            put("Original date", format::stamp(m.meta.forward_date));
        }
    }
    if let Some(r) = m.reply_to {
        let mut v = format!("message {r}");
        if let Some(p) = &c.reply {
            v.push_str(&format!(" · {p}"));
        }
        put("Reply to", v);
    } else if m.meta.reply_chat != 0 {
        put("Reply to", format!("message in chat {}", m.meta.reply_chat));
    }
    if m.meta.via_bot != 0 {
        put("Via bot", m.meta.via_bot.to_string());
    }
    if !m.meta.signature.is_empty() {
        put("Signature", m.meta.signature.clone());
    }
    if m.meta.channel_post || m.meta.views > 0 {
        put("Views", format::count(i64::from(m.meta.views), c.exact));
        put(
            "Forwards",
            format::count(i64::from(m.meta.forwards), c.exact),
        );
    }
    if m.meta.replies > 0 {
        put("Replies", format::count(i64::from(m.meta.replies), c.exact));
    }
    if m.ttl != 0 {
        put("Self-destruct", format::ttl(m.ttl));
    }
    if let Some(label) = &m.media {
        put("Content", format::one_line(label));
    }
    let many = m.attachments.len() > 1;
    for (i, a) in m.attachments.iter().enumerate() {
        let head = if many {
            format!("Media {}", i + 1)
        } else {
            "Media".to_owned()
        };
        put(&head, kind_name(a.kind).to_owned());
        if !a.name.is_empty() {
            put("  name", a.name.clone());
        }
        if a.file.size > 0 {
            put(
                "  size",
                format!("{} ({} bytes)", format::file_size(a.file.size), a.file.size),
            );
        }
        if a.width > 0 && a.height > 0 {
            put("  dimensions", format!("{}×{}", a.width, a.height));
        }
        if a.duration > 0 {
            put("  duration", format::duration(a.duration));
        }
        if !a.mime.is_empty() {
            put("  type", a.mime.clone());
        }
        put("  file ID", a.file.id.to_string());
    }
    put("Protected chat", yes(c.protected));
    match &c.flags {
        Some(Ok(f)) => {
            put("Can be saved", yes(f.can_be_saved));
            put("Can be forwarded", yes(f.can_be_forwarded));
            put("Can be copied", yes(f.can_be_copied));
        }
        Some(Err(e)) => {
            put("Can be saved", yes(m.meta.can_be_saved));
            put("Can be forwarded", format!("unknown: {e}"));
        }
        None if m.deleted => put("Can be saved", "no longer on the server".to_owned()),
        None => {
            put("Can be saved", yes(m.meta.can_be_saved));
            put("Can be forwarded", "checking...".to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, FileRef, MsgMeta, RichText, SendState};

    fn base() -> Msg {
        Msg {
            id: 4242,
            chat_id: -100777,
            sender: Sender::User(55),
            date: 1_700_000_005,
            edited: true,
            outgoing: false,
            state: SendState::Sent,
            reply_to: Some(4200),
            album: 0,
            media: None,
            attachments: Vec::new(),
            body: RichText::plain("hello"),
            pinned: false,
            mention: false,
            unread_reaction: false,
            reactions: Vec::new(),
            keyboard: None,
            forward: Some(Origin::Sender(Sender::Chat(-100999))),
            topic: None,
            ttl: 0,
            deleted: false,
            meta: MsgMeta {
                edit_date: 1_700_000_065,
                forward_date: 1_690_000_000,
                views: 15_234,
                forwards: 1_200,
                can_be_saved: true,
                channel_post: true,
                ..MsgMeta::default()
            },
        }
    }

    fn ctx(exact: bool, flags: Option<Result<Flags, String>>) -> Ctx {
        Ctx {
            sender: "Alice".to_owned(),
            chat: "News".to_owned(),
            origin: Some("Source channel".to_owned()),
            reply: Some("Bob: earlier".to_owned()),
            flags,
            protected: false,
            exact,
            edits: 2,
        }
    }

    fn get<'a>(rows: &'a [(String, String)], key: &str) -> &'a str {
        rows.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("{key} missing in {rows:?}"))
    }

    #[test]
    fn details_show_ids_exact_dates_origin_counts_and_flags() {
        let mut m = base();
        let mut a = Attachment::new(
            MediaKind::Video,
            FileRef {
                id: 9,
                size: 3_145_728,
                ..FileRef::default()
            },
        );
        a.width = 1280;
        a.height = 720;
        a.duration = 75;
        a.mime = "video/mp4".to_owned();
        a.name = "clip.mp4".to_owned();
        m.attachments.push(a);
        let flags = Flags {
            can_be_saved: false,
            can_be_forwarded: true,
            can_be_copied: false,
        };
        let rows = lines(&m, &ctx(false, Some(Ok(flags))));
        assert_eq!(get(&rows, "Message ID"), "4242");
        assert_eq!(get(&rows, "Chat"), "-100777 · News");
        assert_eq!(get(&rows, "Sender ID"), "55 (user)");
        assert_eq!(get(&rows, "Sent"), format::stamp(1_700_000_005));
        assert_eq!(get(&rows, "Sent").len(), 19);
        assert_eq!(get(&rows, "Edited"), format::stamp(1_700_000_065));
        assert_eq!(
            get(&rows, "Forwarded from"),
            "Source channel · -100999 (chat)"
        );
        assert_eq!(get(&rows, "Original date"), format::stamp(1_690_000_000));
        assert_eq!(get(&rows, "Reply to"), "message 4200 · Bob: earlier");
        assert_eq!(get(&rows, "Views"), "15.2K");
        assert_eq!(get(&rows, "Forwards"), "1.2K");
        assert_eq!(get(&rows, "Saved versions"), "2");
        assert_eq!(get(&rows, "Media"), "video");
        assert_eq!(get(&rows, "  size"), "3.0 MB (3145728 bytes)");
        assert_eq!(get(&rows, "  dimensions"), "1280×720");
        assert_eq!(get(&rows, "  duration"), "1:15");
        assert_eq!(get(&rows, "  type"), "video/mp4");
        assert_eq!(get(&rows, "Can be saved"), "no");
        assert_eq!(get(&rows, "Can be forwarded"), "yes");
        let exact = lines(&m, &ctx(true, None));
        assert_eq!(get(&exact, "Views"), "15234");
        assert_eq!(get(&exact, "Can be forwarded"), "checking...");
        assert_eq!(get(&exact, "Can be saved"), "yes");
    }

    #[test]
    fn plain_message_details_skip_channel_rows() {
        let mut m = base();
        m.meta = MsgMeta::default();
        m.forward = None;
        m.reply_to = None;
        m.edited = false;
        m.deleted = true;
        let rows = lines(&m, &ctx(false, None));
        assert!(!rows.iter().any(|(k, _)| k == "Views" || k == "Edited"));
        assert!(!rows.iter().any(|(k, _)| k == "Reply to"));
        assert_eq!(get(&rows, "Deleted"), "yes, kept only on this device");
        assert_eq!(get(&rows, "Can be saved"), "no longer on the server");
    }
}
