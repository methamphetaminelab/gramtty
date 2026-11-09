use crate::format::Lang;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Unknown,
    Online {
        expires: i64,
    },
    Offline {
        was_online: i64,
    },
    Recently,
    LastWeek,
    LastMonth,
}

impl Status {
    pub fn is_online(self, now: i64) -> bool {
        matches!(self, Status::Online { expires } if expires > now)
    }
}

pub fn status_text_in(lang: Lang, status: Status, bot: bool, now: i64) -> String {
    match lang {
        Lang::Ru => status_ru(status, bot, now),
        Lang::En => status_en(status, bot, now),
    }
}

fn status_en(status: Status, bot: bool, now: i64) -> String {
    if bot {
        return "bot".to_owned();
    }
    match status {
        Status::Online { expires } if expires > now => "online".to_owned(),
        Status::Online { expires } => last_seen(expires, now),
        Status::Offline { was_online } => last_seen(was_online, now),
        Status::Recently => "last seen recently".to_owned(),
        Status::LastWeek => "last seen within a week".to_owned(),
        Status::LastMonth => "last seen within a month".to_owned(),
        Status::Unknown => "last seen a long time ago".to_owned(),
    }
}

fn status_ru(status: Status, bot: bool, now: i64) -> String {
    if bot {
        return "бот".to_owned();
    }
    let seen = |at: i64| {
        let ago = (now - at).max(0);
        if ago < 60 {
            "был(а) только что".to_owned()
        } else if ago < 3600 {
            format!("был(а) {} мин. назад", ago / 60)
        } else if ago < 86_400 {
            format!("был(а) {} ч. назад", ago / 3600)
        } else {
            format!("был(а) {}", short_date(at))
        }
    };
    match status {
        Status::Online { expires } if expires > now => "в сети".to_owned(),
        Status::Online { expires } => seen(expires),
        Status::Offline { was_online } => seen(was_online),
        Status::Recently => "был(а) недавно".to_owned(),
        Status::LastWeek => "был(а) на этой неделе".to_owned(),
        Status::LastMonth => "был(а) в этом месяце".to_owned(),
        Status::Unknown => "был(а) давно".to_owned(),
    }
}

fn short_date(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y")
                .to_string()
        })
        .unwrap_or_default()
}

fn last_seen(at: i64, now: i64) -> String {
    let ago = (now - at).max(0);
    if ago < 60 {
        "last seen just now".to_owned()
    } else if ago < 3600 {
        format!("last seen {} min ago", ago / 60)
    } else if ago < 86_400 {
        let h = ago / 3600;
        format!("last seen {h} hour{} ago", if h == 1 { "" } else { "s" })
    } else {
        format!("last seen {}", short_date(at))
    }
}

pub const TYPING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typing {
    pub sender: crate::model::Sender,
    pub activity: Activity,
    pub topic: Option<i32>,
    pub since: std::time::Instant,
}

impl Typing {
    pub fn new(sender: crate::model::Sender, activity: Activity, topic: Option<i32>) -> Self {
        Self {
            sender,
            activity,
            topic,
            since: std::time::Instant::now(),
        }
    }

    pub fn expired(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.since) >= TYPING_TIMEOUT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Typing,
    RecordingVoice,
    RecordingVideo,
    RecordingVideoNote,
    UploadingPhoto,
    UploadingVideo,
    UploadingFile,
    ChoosingSticker,
    Other,
}

impl Activity {
    fn verb(self) -> &'static str {
        match self {
            Activity::Typing => "typing",
            Activity::RecordingVoice => "recording a voice message",
            Activity::RecordingVideo => "recording a video",
            Activity::RecordingVideoNote => "recording a video message",
            Activity::UploadingPhoto => "sending a photo",
            Activity::UploadingVideo => "sending a video",
            Activity::UploadingFile => "sending a file",
            Activity::ChoosingSticker => "choosing a sticker",
            Activity::Other => "doing something",
        }
    }
}

impl Activity {
    fn verb_ru(self, plural: bool) -> &'static str {
        match (self, plural) {
            (Activity::Typing, false) => "печатает",
            (Activity::Typing, true) => "печатают",
            (Activity::RecordingVoice, false) => "записывает голосовое",
            (Activity::RecordingVoice, true) => "записывают голосовые",
            (Activity::RecordingVideo, false) => "записывает видео",
            (Activity::RecordingVideo, true) => "записывают видео",
            (Activity::RecordingVideoNote, false) => "записывает видеосообщение",
            (Activity::RecordingVideoNote, true) => "записывают видеосообщения",
            (Activity::UploadingPhoto, false) => "отправляет фото",
            (Activity::UploadingPhoto, true) => "отправляют фото",
            (Activity::UploadingVideo, false) => "отправляет видео",
            (Activity::UploadingVideo, true) => "отправляют видео",
            (Activity::UploadingFile, false) => "отправляет файл",
            (Activity::UploadingFile, true) => "отправляют файлы",
            (Activity::ChoosingSticker, false) => "выбирает стикер",
            (Activity::ChoosingSticker, true) => "выбирают стикеры",
            (Activity::Other, false) => "что-то делает",
            (Activity::Other, true) => "что-то делают",
        }
    }
}

pub fn typing_text_in(lang: Lang, entries: &[(String, Activity)], private: bool) -> Option<String> {
    if lang == Lang::En {
        return typing_text(entries, private);
    }
    let (first, activity) = entries.first()?;
    if private {
        return Some(format!("{}…", activity.verb_ru(false)));
    }
    let same = entries.iter().all(|(_, a)| a == activity);
    let act = if same { *activity } else { Activity::Typing };
    Some(match entries.len() {
        1 => format!("{first} {}…", act.verb_ru(false)),
        2 => format!("{first}, {} {}…", entries[1].0, act.verb_ru(true)),
        n => format!("{first} и ещё {} {}…", n - 1, act.verb_ru(true)),
    })
}

pub fn typing_text(entries: &[(String, Activity)], private: bool) -> Option<String> {
    let (first, activity) = entries.first()?;
    if private {
        return Some(format!("{}…", activity.verb()));
    }
    let same = entries.iter().all(|(_, a)| a == activity);
    let verb = if same { activity.verb() } else { "typing" };
    Some(match entries.len() {
        1 => format!("{first} is {verb}…"),
        2 => format!("{first} and {} are {verb}…", entries[1].0),
        n => format!("{first} and {} others are {verb}…", n - 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn russian_status_and_typing() {
        let t = |s| status_text_in(Lang::Ru, s, false, NOW);
        assert_eq!(t(Status::Online { expires: NOW + 30 }), "в сети");
        assert_eq!(
            t(Status::Offline {
                was_online: NOW - 5 * 60
            }),
            "был(а) 5 мин. назад"
        );
        assert_eq!(t(Status::Recently), "был(а) недавно");
        let a = ("Аня".to_owned(), Activity::Typing);
        let b = ("Боря".to_owned(), Activity::Typing);
        assert_eq!(
            typing_text_in(Lang::Ru, std::slice::from_ref(&a), true).as_deref(),
            Some("печатает…")
        );
        assert_eq!(
            typing_text_in(Lang::Ru, &[a, b], false).as_deref(),
            Some("Аня, Боря печатают…")
        );
    }

    #[test]
    fn formats_user_status() {
        let t = |s| status_text_in(Lang::En, s, false, NOW);
        assert_eq!(t(Status::Online { expires: NOW + 30 }), "online");
        assert_eq!(
            t(Status::Online { expires: NOW - 10 }),
            "last seen just now"
        );
        assert_eq!(
            t(Status::Offline {
                was_online: NOW - 5 * 60 - 7
            }),
            "last seen 5 min ago"
        );
        assert_eq!(
            t(Status::Offline {
                was_online: NOW - 3600
            }),
            "last seen 1 hour ago"
        );
        assert_eq!(
            t(Status::Offline {
                was_online: NOW - 5 * 3600
            }),
            "last seen 5 hours ago"
        );
        assert!(
            t(Status::Offline {
                was_online: NOW - 3 * 86_400
            })
            .starts_with("last seen ")
        );
        assert!(
            t(Status::Offline {
                was_online: NOW - 3 * 86_400
            })
            .contains('.')
        );
        assert_eq!(
            t(Status::Offline {
                was_online: NOW + 9
            }),
            "last seen just now"
        );
        assert_eq!(t(Status::Recently), "last seen recently");
        assert_eq!(t(Status::LastWeek), "last seen within a week");
        assert_eq!(t(Status::LastMonth), "last seen within a month");
        assert_eq!(t(Status::Unknown), "last seen a long time ago");
        assert_eq!(status_text_in(Lang::En, Status::Recently, true, NOW), "bot");
        assert!(Status::Online { expires: NOW + 1 }.is_online(NOW));
        assert!(!Status::Online { expires: NOW }.is_online(NOW));
        assert!(!Status::Recently.is_online(NOW));
    }

    #[test]
    fn formats_typing_line() {
        let e = |names: &[(&str, Activity)]| -> Vec<(String, Activity)> {
            names.iter().map(|(n, a)| ((*n).to_owned(), *a)).collect()
        };
        assert_eq!(typing_text(&[], false), None);
        assert_eq!(
            typing_text(&e(&[("Alice", Activity::Typing)]), true).unwrap(),
            "typing…"
        );
        assert_eq!(
            typing_text(&e(&[("Alice", Activity::RecordingVoice)]), true).unwrap(),
            "recording a voice message…"
        );
        assert_eq!(
            typing_text(&e(&[("Alice", Activity::Typing)]), false).unwrap(),
            "Alice is typing…"
        );
        assert_eq!(
            typing_text(
                &e(&[("Alice", Activity::Typing), ("Bob", Activity::Typing)]),
                false
            )
            .unwrap(),
            "Alice and Bob are typing…"
        );
        assert_eq!(
            typing_text(
                &e(&[
                    ("Alice", Activity::UploadingPhoto),
                    ("Bob", Activity::Typing),
                    ("Eve", Activity::Typing)
                ]),
                false
            )
            .unwrap(),
            "Alice and 2 others are typing…"
        );
    }
}
