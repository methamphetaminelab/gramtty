use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Auth};
use crate::config::ApiCredentials;
use crate::keymap::Keymap;
use crate::model::{
    Attachment, ChatInfo, ChatKind, Decode, Draft, Entity, EntityKind, FileRef, ListId, MediaKind,
    Msg, Origin, Position, Reaction, RichText, SendState, Sender, UserMeta, Visual,
};
use crate::presence::{Activity, Status, Typing};
use crate::td::Event;
use crate::termcolors::TermPalette;

const W: u16 = 110;
const H: u16 = 30;
const ME: i64 = 1;
const MAYA: i64 = 2;
const TOBIAS: i64 = 3;
const CLARA: i64 = 4;
const ETHAN: i64 = 5;
const PRIYA: i64 = 6;
const JONAS: i64 = 7;
const BOT: i64 = 8;

const HIKERS: i64 = 100;
const MAYA_CHAT: i64 = 101;
const LANTERN: i64 = 102;
const WEATHER: i64 = 103;
const CLARA_CHAT: i64 = 104;
const BOT_CHAT: i64 = 105;
const SAVED: i64 = 106;
const ETHAN_CHAT: i64 = 107;
const BOOKS: i64 = 108;
const TOBIAS_CHAT: i64 = 109;
const PATTERNS: i64 = 110;
const PRIYA_CHAT: i64 = 111;

const PERSONAL: i32 = 2;
const PROJECTS: i32 = 3;

const PALETTE: TermPalette = TermPalette {
    fg: (192, 202, 245),
    bg: (26, 27, 38),
    ansi: [
        (21, 22, 30),
        (247, 118, 142),
        (158, 206, 106),
        (224, 175, 104),
        (122, 162, 247),
        (187, 154, 247),
        (125, 207, 255),
        (169, 177, 214),
        (86, 95, 137),
        (255, 137, 157),
        (185, 242, 124),
        (255, 199, 119),
        (137, 180, 250),
        (199, 169, 255),
        (180, 249, 248),
        (192, 202, 245),
    ],
};

const REPLY: &str = "What a view! I will bring the stove and hot tea";
const PICKUP: &str = "Leaving at 7:30 sharp, I will pick everyone up";
const DRAGGED: &str = "Perfect, then we are all set!";
const EMOJIS: [&str; 16] = [
    "👍", "❤", "🔥", "🎉", "😍", "👏", "😁", "🤔", "🙏", "👌", "😢", "🤩", "💯", "⚡", "🏆", "😎",
];
const RHYTHM: [u64; 11] = [90, 60, 120, 60, 90, 60, 150, 90, 60, 60, 120];

const LANDSCAPE: i32 = 1;
const ALBUM: [i32; 3] = [2, 3, 4];
const STICKER: i32 = 5;
const CLIP: i32 = 6;
const AVATAR_BASE: i32 = 8;

const ART: [(i32, &str, u32, u32); 6] = [
    (LANDSCAPE, "mountains", 380, 260),
    (ALBUM[0], "forest", 220, 280),
    (ALBUM[1], "lake", 220, 280),
    (ALBUM[2], "camp", 220, 280),
    (STICKER, "sticker", 192, 192),
    (CLIP, "clip", 380, 220),
];

const FACES: [(i64, &str, usize); 7] = [
    (MAYA, "ML", 3),
    (TOBIAS, "TR", 4),
    (CLARA, "CM", 5),
    (ETHAN, "EB", 6),
    (PRIYA, "PN", 1),
    (JONAS, "JW", 2),
    (BOT, "TB", 8),
];

static KEYS: OnceLock<PathBuf> = OnceLock::new();

fn key_path(id: i32) -> String {
    KEYS.get()
        .map(|dir| dir.join(format!("{id}.png")).to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn write_key(id: i32, w: u32, h: u32) {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([
            id as u8,
            (x * 255 / (w - 1)) as u8,
            (y * 255 / (h - 1)) as u8,
            255,
        ])
    });
    img.save(key_path(id)).unwrap();
}

fn write_meta(out: &std::path::Path) {
    let mut art = serde_json::Map::new();
    for (id, name, w, h) in ART {
        write_key(id, w, h);
        art.insert(id.to_string(), serde_json::json!({ "name": name }));
    }
    for (user, label, tint) in FACES {
        let id = AVATAR_BASE + user as i32;
        write_key(id, 64, 64);
        art.insert(
            id.to_string(),
            serde_json::json!({
                "name": "avatar",
                "label": label,
                "tint": plain_hex(PALETTE.ansi[tint]),
            }),
        );
    }
    let meta = serde_json::json!({
        "fg": plain_hex(PALETTE.fg),
        "bg": plain_hex(PALETTE.bg),
        "dots": ([1, 3, 2].map(|i| plain_hex(PALETTE.ansi[i]))),
        "accent": plain_hex(PALETTE.ansi[4]),
        "art": art,
    });
    std::fs::write(out.join("meta.json"), meta.to_string()).unwrap();
}

struct Demo {
    app: App,
    rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    out: PathBuf,
    count: usize,
    now: i64,
    pointer: Option<(u16, u16)>,
    caption: &'static str,
    typed: usize,
    order: i64,
    buf: Buffer,
}

fn hex(c: Color, fallback: crate::termcolors::Rgb) -> String {
    plain_hex(crate::theme::rgb_of(Some(c), fallback, &PALETTE))
}

fn plain_hex((r, g, b): crate::termcolors::Rgb) -> String {
    format!("{r:02x}{g:02x}{b:02x}")
}

fn cells(buf: &Buffer) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    for y in 0..buf.area.height {
        let mut row = Vec::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let symbol = if skip > 0 { "" } else { c.symbol() };
            skip = if skip > 0 {
                skip - 1
            } else {
                symbol.width().saturating_sub(1)
            };
            let flags: String = [
                (Modifier::BOLD, 'b'),
                (Modifier::ITALIC, 'i'),
                (Modifier::UNDERLINED, 'u'),
                (Modifier::REVERSED, 'r'),
                (Modifier::DIM, 'd'),
                (Modifier::CROSSED_OUT, 's'),
            ]
            .iter()
            .filter(|(m, _)| c.modifier.contains(*m))
            .map(|(_, f)| *f)
            .collect();
            row.push(serde_json::json!([
                symbol,
                hex(c.fg, PALETTE.fg),
                hex(c.bg, PALETTE.bg),
                flags
            ]));
        }
        rows.push(serde_json::Value::Array(row));
    }
    rows
}

impl Demo {
    fn start(out: PathBuf, now: i64) -> Self {
        let (app, rx) = account(now);
        Demo {
            app,
            rx,
            out,
            count: 0,
            now,
            pointer: None,
            caption: "",
            typed: 0,
            order: 1500,
            buf: Buffer::empty(ratatui::layout::Rect::new(0, 0, W, H)),
        }
    }

    fn draw(&mut self) -> Option<[u16; 2]> {
        let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
        term.draw(|f| crate::ui::draw(f, &mut self.app)).unwrap();
        let backend = term.backend();
        self.buf = backend.buffer().clone();
        backend.cursor_visible().then(|| {
            let p = backend.cursor_position();
            [p.x, p.y]
        })
    }

    fn settle(&mut self) -> Option<[u16; 2]> {
        loop {
            let cursor = self.draw();
            let work = self.app.media.has_work();
            self.app.after_draw();
            if !work {
                return cursor;
            }
            let mut patience = Duration::from_secs(5);
            let mut idle = Instant::now();
            while idle.elapsed() < patience {
                match self.rx.try_recv() {
                    Ok(ev) => {
                        self.app.handle(ev);
                        idle = Instant::now();
                        patience = Duration::from_millis(350);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        }
    }

    fn step(&mut self, ms: u64, extra: serde_json::Value) {
        let cursor = self.settle();
        let mut frame = serde_json::json!({
            "ms": ms,
            "cursor": cursor,
            "pointer": self.pointer.map(|(x, y)| [x, y]),
            "caption": self.caption,
            "rows": cells(&self.buf),
        });
        if let (Some(frame), Some(extra)) = (frame.as_object_mut(), extra.as_object()) {
            frame.extend(extra.clone());
        }
        let path = self.out.join(format!("frame-{:04}.json", self.count));
        std::fs::write(path, frame.to_string()).unwrap();
        self.count += 1;
    }

    fn shot(&mut self, ms: u64) {
        self.step(ms, serde_json::json!({}));
    }

    fn pop(&mut self, ms: u64) {
        self.step(ms, serde_json::json!({ "enter": "pop" }));
    }

    fn fade(&mut self, ms: u64) {
        self.step(ms, serde_json::json!({ "enter": "fade" }));
    }

    fn glide(&mut self, x: u16, y: u16, ms: u64) {
        self.pointer = Some((x, y));
        self.step(ms, serde_json::json!({ "glide": "ease" }));
    }

    fn hide_pointer(&mut self) {
        self.pointer = None;
    }

    fn press(&mut self, code: KeyCode) {
        self.app.handle(Event::Input(TermEvent::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))));
        self.feed();
    }

    fn keys(&mut self, code: KeyCode, times: usize, ms: u64) {
        for _ in 0..times {
            self.press(code);
            self.shot(ms);
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c));
            self.settle_search();
            let pause = RHYTHM[self.typed % RHYTHM.len()] + if c == ' ' { 30 } else { 0 };
            self.typed += 1;
            self.step(pause, serde_json::json!({ "typing": true }));
        }
    }

    fn mouse(&mut self, kind: MouseEventKind) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        self.app.handle(Event::Input(TermEvent::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })));
        self.feed();
    }

    fn click(&mut self, ms: u64) {
        self.mouse(MouseEventKind::Down(MouseButton::Left));
        self.mouse(MouseEventKind::Up(MouseButton::Left));
        self.step(ms, serde_json::json!({ "click": true }));
    }

    fn wheel(&mut self, up: bool, limit: usize, ms: u64) -> usize {
        for done in 0..limit {
            let before = self.buf.clone();
            self.mouse(if up {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            });
            self.settle();
            if self.buf == before {
                return done;
            }
            self.shot(ms);
        }
        limit
    }

    fn drag_to(&mut self, x: u16, ms: u64) {
        let Some((from, y)) = self.pointer else {
            return;
        };
        self.mouse(MouseEventKind::Down(MouseButton::Left));
        for at in from + 1..=x {
            self.pointer = Some((at, y));
            self.mouse(MouseEventKind::Drag(MouseButton::Left));
            self.step(ms, serde_json::json!({ "glide": "line" }));
        }
    }

    fn feed(&mut self) {
        let Some(chat) = self.app.open_chat else {
            return;
        };
        let Some(h) = self.app.histories.get(&chat) else {
            return;
        };
        if !h.is_empty() || !h.loading_older {
            return;
        }
        let epoch = h.epoch;
        let last = self.app.chats[&chat]
            .last_message
            .as_ref()
            .map_or(0, |m| m.id);
        let mut msgs = story(chat, self.now);
        msgs.retain(|m| m.id <= last);
        msgs.reverse();
        self.app.handle(Event::History {
            chat_id: chat,
            msgs,
            initial: true,
            epoch,
        });
        if let Some(h) = self.app.histories.get_mut(&chat) {
            h.reached_start = true;
        }
        if chat == HIKERS {
            let pinned = story(HIKERS, self.now).remove(0);
            self.app.pinned_msg.insert(HIKERS, pinned);
        }
    }

    fn settle_search(&mut self) {
        let now = self.now;
        let Some(crate::app::Popup::Find(v)) = &mut self.app.popup else {
            return;
        };
        let query = v.query().to_lowercase();
        v.due = None;
        v.sent = Some(query.clone());
        v.peers = Some(crate::td::Peers::default());
        v.loading = false;
        v.msgs = if query.is_empty() {
            Vec::new()
        } else {
            [HIKERS, MAYA_CHAT, LANTERN, WEATHER, BOOKS]
                .into_iter()
                .flat_map(|chat| story(chat, now))
                .filter(|m| m.body.text.to_lowercase().contains(&query))
                .collect()
        };
        v.msgs.sort_by_key(|m| std::cmp::Reverse(m.date));
        v.total = v.msgs.len() as i32;
    }

    fn read(&mut self, chat: i64) {
        let c = self.app.chats.get_mut(&chat).unwrap();
        c.unread = 0;
        c.mentions = 0;
        c.last_read_inbox = c.last_message.as_ref().map_or(0, |m| m.id);
    }

    fn arrive(&mut self, chat: i64, id: i64) {
        let msg = story(chat, self.now)
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        self.order += 1;
        let order = self.order;
        let open = self.app.open_chat == Some(chat);
        let c = self.app.chats.get_mut(&chat).unwrap();
        for p in &mut c.positions {
            p.order = order;
        }
        if !msg.outgoing && !open {
            c.unread += 1;
        }
        c.last_message = Some(msg.clone());
        if open && let Some(h) = self.app.histories.get_mut(&chat) {
            h.upsert(msg);
        }
        self.app.typing.remove(&chat);
        self.app.refresh_lists();
    }

    fn typing(&mut self, chat: i64, user: i64, activity: Activity) {
        self.app
            .typing
            .insert(chat, vec![Typing::new(Sender::User(user), activity, None)]);
    }

    fn find(&self, needle: &str) -> (u16, u16) {
        self.find_from(needle, 0)
    }

    fn find_from(&self, needle: &str, left: u16) -> (u16, u16) {
        let want: Vec<String> = needle.chars().map(String::from).collect();
        for y in 0..H {
            for x in left..W {
                let hit = want.iter().enumerate().all(|(i, ch)| {
                    let xi = x + i as u16;
                    xi < W && self.buf[(xi, y)].symbol() == ch
                });
                if hit {
                    return (x, y);
                }
            }
        }
        panic!("{needle} is not on screen");
    }
}

fn clock() -> i64 {
    chrono::Local::now()
        .date_naive()
        .and_hms_opt(18, 42, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map_or_else(crate::app::unix_now, |t| t.timestamp())
}

fn text(id: i64, chat: i64, sender: i64, date: i64, body: &str) -> Msg {
    Msg {
        id,
        chat_id: chat,
        sender: Sender::User(sender),
        date,
        edited: false,
        outgoing: sender == ME,
        state: SendState::Sent,
        reply_to: None,
        album: 0,
        media: None,
        attachments: Vec::new(),
        body: RichText::plain(body),
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

fn post(id: i64, chat: i64, date: i64, body: &str) -> Msg {
    let mut m = text(id, chat, 0, date, body);
    m.sender = Sender::Chat(chat);
    m.meta.channel_post = true;
    m
}

fn reaction(emoji: &str, count: i32, chosen: bool) -> Reaction {
    Reaction {
        label: emoji.to_owned(),
        emoji: Some(emoji.to_owned()),
        custom: 0,
        count,
        chosen,
    }
}

fn mark(m: &mut Msg, needle: &str, kind: EntityKind) {
    let at = m.body.text.find(needle).unwrap();
    m.body.entities.push(Entity {
        offset: m.body.text[..at].encode_utf16().count(),
        length: needle.encode_utf16().count(),
        kind,
    });
}

fn picture(kind: MediaKind, id: i32, width: i32, height: i32) -> Attachment {
    let stored = FileRef {
        id,
        size: 184_320,
        path: key_path(id),
        done: true,
        active: false,
        downloaded: 184_320,
        uploading: false,
        uploaded: 0,
    };
    let mut a = Attachment::new(kind, stored.clone());
    a.width = width;
    a.height = height;
    a.preview = Some(Visual {
        file: stored,
        decode: Decode::Raster,
        width,
        height,
    });
    a.full = a.preview.clone();
    a
}

fn media(id: i64, chat: i64, sender: i64, date: i64, body: &str, att: Attachment) -> Msg {
    let mut m = text(id, chat, sender, date, body);
    m.attachments = vec![att];
    m
}

fn file(id: i32, size: i64) -> FileRef {
    FileRef {
        id,
        size,
        path: String::new(),
        done: false,
        active: false,
        downloaded: 0,
        uploading: false,
        uploaded: 0,
    }
}

fn story(chat: i64, now: i64) -> Vec<Msg> {
    let at = |minutes: i64| now - minutes * 60;
    match chat {
        HIKERS => {
            let mut plan = text(
                1,
                chat,
                MAYA,
                at(1560),
                "Plan for Saturday: Ridgeline loop, 14 km. We meet at the north trailhead at 8:00, back by five.",
            );
            plan.pinned = true;
            mark(&mut plan, "Ridgeline loop, 14 km", EntityKind::Bold);
            plan.reactions = vec![reaction("👍", 4, true), reaction("🔥", 2, false)];
            let mut gpx = text(2, chat, MAYA, at(1558), "Here is the track");
            let mut att = Attachment::new(MediaKind::Document, file(31, 48_312));
            att.name = "ridgeline-loop.gpx".to_owned();
            att.mime = "application/gpx+xml".to_owned();
            gpx.attachments = vec![att];
            gpx.media = Some("ridgeline-loop.gpx".to_owned());
            let mut album: Vec<Msg> = ALBUM
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    let caption = if i == 0 {
                        "Scouting trip last week: the forest, the lake and our camp spot"
                    } else {
                        ""
                    };
                    let mut m = media(
                        3 + i as i64,
                        chat,
                        PRIYA,
                        at(1500),
                        caption,
                        picture(MediaKind::Photo, *id, 1100, 1400),
                    );
                    m.album = 77;
                    m
                })
                .collect();
            album[0].reactions = vec![reaction("😍", 3, true)];
            let mut forecast = post(
                6,
                chat,
                at(190),
                "Saturday on the ridge: clear until noon, wind 20 km/h from the west, light showers after four.",
            );
            forecast.sender = Sender::User(TOBIAS);
            forecast.meta.channel_post = false;
            forecast.forward = Some(Origin::Sender(Sender::Chat(WEATHER)));
            forecast.meta.forward_date = at(260);
            let mut early = text(
                7,
                chat,
                TOBIAS,
                at(189),
                "So we should be off the ridge before the rain. Starting early was the right call.",
            );
            early.edited = true;
            early.meta.edit_date = at(187);
            let mut map = text(
                8,
                chat,
                CLARA,
                at(120),
                "I checked the trail report: https://example.org/trails/ridgeline. The bridge at km 9 is open again.",
            );
            mark(
                &mut map,
                "https://example.org/trails/ridgeline",
                EntityKind::Link(None),
            );
            map.reactions = vec![reaction("🎉", 3, false)];
            let mut voice = text(9, chat, PRIYA, at(64), "");
            let mut note = Attachment::new(MediaKind::Voice, file(32, 96_000));
            note.duration = 23;
            note.waveform = (0..64u32)
                .map(|i| ((i * 37 % 29) + (i % 7) * 3) as u8)
                .collect();
            voice.attachments = vec![note];
            voice.media = Some("voice message".to_owned());
            let mut soup = text(
                10,
                chat,
                MAYA,
                at(41),
                "I will make sandwiches. Someone take a stove and we get hot tea at the top.",
            );
            soup.reactions = vec![reaction("❤", 2, false)];
            let seats = text(11, chat, ME, at(38), "Count me in, one seat please!");
            let pickup = text(12, chat, PRIYA, at(2), PICKUP);
            let view = media(
                13,
                chat,
                CLARA,
                at(1),
                "The view from the saddle. Tomorrow we are up there!",
                picture(MediaKind::Photo, LANDSCAPE, 1520, 1040),
            );
            let mut wave = media(
                14,
                chat,
                TOBIAS,
                at(0),
                "",
                picture(MediaKind::Sticker, STICKER, 512, 512),
            );
            wave.attachments[0].name = "👋".to_owned();
            wave.media = Some("👋 Sticker".to_owned());
            let mut mine = text(15, chat, ME, now, REPLY);
            mine.reply_to = Some(13);
            let mut set = text(16, chat, MAYA, now + 60, DRAGGED);
            set.reactions = vec![reaction("👍", 1, true)];
            let mut all = vec![plan, gpx];
            all.extend(album);
            all.extend([
                forecast, early, map, voice, soup, seats, pickup, view, wave, mine, set,
            ]);
            all
        }
        MAYA_CHAT => {
            let mut review = text(
                3,
                chat,
                MAYA,
                at(95),
                "I read your patch for lanternfish. The parser is much cleaner now.",
            );
            mark(&mut review, "lanternfish", EntityKind::Italic);
            let mut nit = text(
                4,
                chat,
                MAYA,
                at(94),
                "One thing: parse_header still allocates on every call. Could it borrow instead?",
            );
            mark(&mut nit, "parse_header", EntityKind::Code);
            let mut fixed = text(
                5,
                chat,
                ME,
                at(71),
                "Good catch. Switched it to a borrowed slice, the benchmark went from 41 ms to 28 ms.",
            );
            fixed.reactions = vec![reaction("🔥", 1, false)];
            let mut merge = text(6, chat, MAYA, at(66), "Nice! Then it is ready to merge.");
            merge.reply_to = Some(5);
            let release = text(
                7,
                chat,
                ME,
                at(30),
                "Merging tonight. I will tag 0.4.0 after the hike on Saturday.",
            );
            let hike = text(
                8,
                chat,
                MAYA,
                at(26),
                "Deal. Speaking of which, did you see the plan in Weekend Hikers?",
            );
            vec![
                text(
                    1,
                    chat,
                    ME,
                    at(1500),
                    "Pushed the parser rewrite, have a look when you can.",
                ),
                text(2, chat, MAYA, at(1490), "On it tomorrow morning ☕"),
                review,
                nit,
                fixed,
                merge,
                release,
                hike,
            ]
        }
        LANTERN => {
            let mut ping = text(
                7,
                chat,
                JONAS,
                at(17),
                "Can you review the docs change before the release? It touches the trail of examples in the README.",
            );
            ping.mention = true;
            let mut docs = text(
                8,
                chat,
                JONAS,
                at(9),
                "Docs preview: https://example.org/lanternfish/guide and the changelog: https://example.org/lanternfish/changes. Maya Lindqvist wrote the new guide.",
            );
            mark(
                &mut docs,
                "https://example.org/lanternfish/guide",
                EntityKind::Link(None),
            );
            mark(
                &mut docs,
                "https://example.org/lanternfish/changes",
                EntityKind::Link(None),
            );
            mark(&mut docs, "Maya Lindqvist", EntityKind::MentionName(MAYA));
            let mut bench = text(
                2,
                chat,
                MAYA,
                at(400),
                "Benchmarks for the new parser: 28 ms on the large fixture, down from 41 ms.",
            );
            bench.reactions = vec![reaction("🔥", 3, true), reaction("👍", 2, false)];
            vec![
                text(
                    1,
                    chat,
                    JONAS,
                    at(460),
                    "Release plan for lanternfish 0.4.0: parser rewrite, ARM builds, new docs.",
                ),
                bench,
                text(
                    3,
                    chat,
                    JONAS,
                    at(300),
                    "CI is green again after the cache fix.",
                ),
                text(
                    4,
                    chat,
                    MAYA,
                    at(240),
                    "Parser rewrite is approved, merging tonight.",
                ),
                media(
                    5,
                    chat,
                    ETHAN,
                    at(90),
                    "Built it for ARM, all tests pass. Here is the benchmark run.",
                    picture(MediaKind::Animation, CLIP, 1520, 880),
                ),
                text(
                    6,
                    chat,
                    MAYA,
                    at(40),
                    "Then lanternfish 0.4.0 goes out right after the weekend.",
                ),
                ping,
                docs,
            ]
        }
        WEATHER => vec![
            post(
                1,
                chat,
                at(1700),
                "Friday: fog in the valleys until ten, dry on every marked trail.",
            ),
            post(
                2,
                chat,
                at(260),
                "Saturday on the ridge: clear until noon, wind 20 km/h from the west, light showers after four.",
            ),
            post(
                3,
                chat,
                at(45),
                "Sunday outlook: steady rain, trail crews advise staying below the tree line.",
            ),
        ],
        CLARA_CHAT => vec![
            text(
                1,
                chat,
                CLARA,
                at(420),
                "Hey! Are you coming with us on Saturday?",
            ),
            text(
                2,
                chat,
                ME,
                at(410),
                "Sure. I'll bring a thermos and the stove.",
            ),
            text(
                3,
                chat,
                CLARA,
                at(400),
                "Great, then I'll take the map and the first aid kit.",
            ),
        ],
        BOT_CHAT => vec![
            text(1, chat, ME, at(600), "/forecast ridgeline"),
            text(
                2,
                chat,
                BOT,
                at(600),
                "Ridgeline, Saturday: 9°C at the trailhead, 3°C on the summit, sunrise 7:12.",
            ),
        ],
        SAVED => vec![
            text(
                1,
                chat,
                ME,
                at(3000),
                "Packing list: stove, gas, mug, headlamp, spare socks",
            ),
            text(2, chat, ME, at(900), "Release checklist for 0.4.0"),
        ],
        ETHAN_CHAT => vec![
            text(1, chat, ME, at(700), "How's the ARM build going?"),
            text(2, chat, ETHAN, at(210), "Done, I'll send the logs tonight"),
        ],
        BOOKS => vec![
            text(
                1,
                chat,
                PRIYA,
                at(2000),
                "November pick: a novel about a lighthouse keeper. 320 pages, easy trail of chapters.",
            ),
            text(
                2,
                chat,
                TOBIAS,
                at(1300),
                "Chapter six changed my mind about the keeper.",
            ),
            text(3, chat, PRIYA, at(330), "We meet on the 21st, my place."),
        ],
        TOBIAS_CHAT => {
            let mut voice = text(2, chat, TOBIAS, at(500), "");
            let mut note = Attachment::new(MediaKind::Voice, file(13, 51_000));
            note.duration = 12;
            voice.attachments = vec![note];
            voice.media = Some("voice message".to_owned());
            vec![
                text(1, chat, ME, at(520), "Do you still have my headlamp?"),
                voice,
            ]
        }
        PATTERNS => vec![post(
            1,
            chat,
            at(1450),
            "New post: borrowing instead of cloning in hot loops, with benchmarks.",
        )],
        PRIYA_CHAT => vec![
            text(1, chat, PRIYA, at(2890), "Thanks for the book!"),
            text(2, chat, ME, at(2880), "Enjoy, the ending is worth it"),
        ],
        _ => Vec::new(),
    }
}

fn chat(id: i64, title: &str, kind: ChatKind, order: i64, lists: &[ListId], now: i64) -> ChatInfo {
    let mut msgs = story(id, now);
    match id {
        HIKERS => msgs.truncate(11),
        LANTERN => msgs.truncate(7),
        _ => {}
    }
    let last_id = msgs.last().map_or(0, |m| m.id);
    ChatInfo {
        id,
        title: title.to_owned(),
        kind,
        unread: 0,
        mentions: 0,
        reactions: 0,
        last_read_inbox: last_id,
        last_read_outbox: last_id,
        photo: None,
        muted: false,
        use_default_mute: false,
        positions: lists
            .iter()
            .map(|list| Position {
                list: *list,
                order,
                pinned: false,
            })
            .collect(),
        last_message: msgs.last().cloned(),
        group_id: 0,
        secret_id: 0,
        forum: false,
        auto_delete: 0,
        perms: crate::model::Rights::default(),
        protected: false,
        reactions_off: false,
    }
}

fn account(now: i64) -> (App, tokio::sync::mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let creds = ApiCredentials {
        api_id: 1,
        api_hash: "x".to_owned(),
    };
    let mut app = App::new(
        crate::td::Td::recording(tx).0,
        creds,
        std::env::temp_dir(),
        Keymap::default(),
        ratatui_image::picker::Picker::halfblocks(),
    );
    app.auth = Auth::Ready;
    app.lang = crate::format::Lang::En;
    app.my_id = Some(ME);
    for (id, name) in [
        (ME, "Alex Carter"),
        (MAYA, "Maya Lindqvist"),
        (TOBIAS, "Tobias Reed"),
        (CLARA, "Clara Mendes"),
        (ETHAN, "Ethan Brooks"),
        (PRIYA, "Priya Nair"),
        (JONAS, "Jonas Weber"),
        (BOT, "Trailhead Bot"),
    ] {
        app.users.insert(id, name.to_owned());
        let status = match id {
            MAYA | PRIYA => Status::Online {
                expires: now + 86_400,
            },
            _ => Status::Unknown,
        };
        app.user_meta.insert(
            id,
            UserMeta {
                status,
                bot: id == BOT,
                usernames: Vec::new(),
                photo: FACES
                    .iter()
                    .any(|(user, ..)| *user == id)
                    .then(|| picture(MediaKind::Photo, AVATAR_BASE + id as i32, 64, 64).file),
            },
        );
    }
    app.folders = vec![
        (PERSONAL, "Personal".to_owned()),
        (PROJECTS, "Projects".to_owned()),
    ];
    let main = ListId::Main;
    let personal = ListId::Folder(PERSONAL);
    let projects = ListId::Folder(PROJECTS);
    let rows: [(i64, &str, ChatKind, &[ListId]); 12] = [
        (
            MAYA_CHAT,
            "Maya Lindqvist",
            ChatKind::Private(MAYA),
            &[main, personal],
        ),
        (HIKERS, "Weekend Hikers", ChatKind::Group, &[main, personal]),
        (
            LANTERN,
            "lanternfish dev",
            ChatKind::Supergroup,
            &[main, projects],
        ),
        (WEATHER, "Ridgeline Weather", ChatKind::Channel, &[main]),
        (
            CLARA_CHAT,
            "Clara Mendes",
            ChatKind::Private(CLARA),
            &[main, personal],
        ),
        (
            ETHAN_CHAT,
            "Ethan Brooks",
            ChatKind::Private(ETHAN),
            &[main, projects],
        ),
        (BOOKS, "Book Club", ChatKind::Group, &[main, personal]),
        (
            TOBIAS_CHAT,
            "Tobias Reed",
            ChatKind::Private(TOBIAS),
            &[main, personal],
        ),
        (BOT_CHAT, "Trailhead Bot", ChatKind::Private(BOT), &[main]),
        (SAVED, "Saved Messages", ChatKind::Private(ME), &[main]),
        (
            PATTERNS,
            "Rust Patterns",
            ChatKind::Channel,
            &[main, projects],
        ),
        (
            PRIYA_CHAT,
            "Priya Nair",
            ChatKind::Private(PRIYA),
            &[main, personal],
        ),
    ];
    for (i, (id, title, kind, lists)) in rows.into_iter().enumerate() {
        app.chats
            .insert(id, chat(id, title, kind, 1000 - i as i64, lists, now));
    }
    let set =
        |app: &mut App, id: i64, f: &dyn Fn(&mut ChatInfo)| f(app.chats.get_mut(&id).unwrap());
    set(&mut app, HIKERS, &|c| {
        c.last_read_inbox = 11;
    });
    set(&mut app, LANTERN, &|c| {
        c.unread = 3;
        c.mentions = 1;
        c.last_read_inbox = 4;
    });
    set(&mut app, WEATHER, &|c| {
        c.unread = 3;
        c.muted = true;
        c.last_read_inbox = 0;
    });
    set(&mut app, BOOKS, &|c| {
        c.unread = 2;
        c.last_read_inbox = 1;
    });
    set(&mut app, MAYA_CHAT, &|c| {
        for p in &mut c.positions {
            p.order = 2000;
        }
        c.positions[0].pinned = true;
    });
    app.drafts.insert(
        (CLARA_CHAT, None),
        Draft {
            text: "I'll pack spare gloves too".to_owned(),
            ..Draft::default()
        },
    );
    app.typing.insert(
        ETHAN_CHAT,
        vec![Typing::new(Sender::User(ETHAN), Activity::Typing, None)],
    );
    app.refresh_lists();
    (app, rx)
}

#[test]
#[ignore = "writes demo frames to $GRAMTTY_DEMO_OUT for scripts/make-demo.sh"]
fn demo_frames() {
    let Some(out) = std::env::var_os("GRAMTTY_DEMO_OUT").map(PathBuf::from) else {
        return;
    };
    let keys = KEYS.get_or_init(|| out.join("keys"));
    std::fs::create_dir_all(keys).unwrap();
    write_meta(&out);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    crate::termcolors::set(Some(PALETTE));
    let now = clock();
    let mut d = Demo::start(out, now);
    d.caption = "Chats, folders, live updates";
    d.app.enter_chat(MAYA_CHAT);
    d.feed();
    d.press(KeyCode::Esc);
    d.shot(700);

    d.typing(HIKERS, PRIYA, Activity::Typing);
    d.shot(800);
    d.arrive(HIKERS, 12);
    d.shot(500);
    d.arrive(LANTERN, 8);
    d.shot(500);
    d.typing(HIKERS, CLARA, Activity::UploadingPhoto);
    d.shot(800);
    d.arrive(HIKERS, 13);
    d.shot(500);

    let (x, y) = d.find("Weekend Hikers");
    d.pointer = Some((66, 18));
    d.shot(150);
    d.glide(x + 6, y, 540);
    d.click(300);
    d.press(KeyCode::Esc);
    d.read(HIKERS);
    d.caption = "Photos, albums, animated stickers";
    d.shot(1100);
    d.typing(HIKERS, TOBIAS, Activity::ChoosingSticker);
    d.shot(700);
    d.arrive(HIKERS, 14);
    d.shot(1500);
    d.glide(84, 15, 420);
    let lines = d.wheel(true, 60, 60);
    d.shot(1000);
    d.wheel(false, lines, 30);
    d.shot(300);

    d.caption = "Full-size photo viewer";
    let (x, y) = d.find_from("The view from the saddle", 48);
    d.glide(x + 12, y - 5, 450);
    d.click(360);
    d.press(KeyCode::Char('v'));
    d.pop(1700);
    d.press(KeyCode::Esc);
    d.pop(300);

    d.caption = "React and reply";
    d.hide_pointer();
    d.press(KeyCode::Char('r'));
    if let Some(crate::app::Popup::Reactions(r)) = &mut d.app.popup {
        r.loading = false;
        r.emojis = EMOJIS.iter().map(|e| (*e).to_owned()).collect();
    }
    d.pop(600);
    d.keys(KeyCode::Char('l'), 4, 120);
    d.shot(350);
    d.press(KeyCode::Enter);
    d.app.popup = None;
    if let Some(m) = d.app.histories.get_mut(&HIKERS).and_then(|h| h.get_mut(13)) {
        m.reactions = vec![reaction("😍", 1, true)];
    }
    d.pop(700);
    d.press(KeyCode::Char('R'));
    d.shot(400);
    d.type_text(REPLY);
    d.shot(300);
    d.press(KeyCode::Enter);
    d.arrive(HIKERS, 15);
    d.shot(500);
    d.app.chats.get_mut(&HIKERS).unwrap().last_read_outbox = 15;
    d.typing(HIKERS, MAYA, Activity::Typing);
    d.shot(1000);
    d.arrive(HIKERS, 16);
    d.shot(700);

    d.caption = "Mouse selection copies text";
    d.press(KeyCode::Esc);
    let (x, y) = d.find_from(DRAGGED, 48);
    d.pointer = Some((x + 20, y - 3));
    d.shot(150);
    d.glide(x, y, 420);
    d.drag_to(x + DRAGGED.chars().count() as u16 - 1, 30);
    d.shot(500);
    d.mouse(MouseEventKind::Up(MouseButton::Left));
    d.app.handle(Event::Notice(format!(
        "copied {} characters",
        DRAGGED.chars().count()
    )));
    d.glide(x + 36, y - 2, 390);
    d.shot(510);

    d.caption = "Search across everything";
    d.hide_pointer();
    d.press(KeyCode::Char(' '));
    d.pop(1000);
    d.press(KeyCode::Char(' '));
    d.settle_search();
    d.pop(500);
    d.type_text("lantern");
    d.shot(1000);
    d.press(KeyCode::Enter);
    d.press(KeyCode::Esc);
    d.read(LANTERN);
    d.pop(1200);

    d.caption = "Links and mentions open in-app";
    let (x, y) = d.find_from("Docs preview", 48);
    d.pointer = Some((x + 30, y + 4));
    d.shot(150);
    d.glide(x + 8, y, 420);
    d.click(330);
    d.press(KeyCode::Char('o'));
    d.pop(900);
    d.keys(KeyCode::Char('j'), 2, 180);
    d.shot(300);
    d.press(KeyCode::Enter);
    d.app.handle(Event::Link(Ok(crate::td::Resolved::Chat {
        chat_id: MAYA_CHAT,
        profile: false,
    })));
    d.feed();
    d.press(KeyCode::Esc);
    d.hide_pointer();
    d.pop(1300);

    d.caption = "Every key binding one keystroke away";
    d.press(KeyCode::Char('?'));
    d.pop(700);
    d.keys(KeyCode::Char('j'), 12, 60);
    d.shot(700);
    d.press(KeyCode::Esc);
    d.pop(250);

    let count = d.count;
    let out = d.out.clone();
    d = Demo::start(out, now);
    d.count = count;
    d.caption = "Chats, folders, live updates";
    d.app.enter_chat(MAYA_CHAT);
    d.feed();
    d.press(KeyCode::Esc);
    d.fade(600);
    crate::termcolors::set(None);
}
