use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::{BorderStyle, MessageLayout, ThemeChoice};
use crate::termcolors::TermPalette;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeColor(pub Option<Color>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeStyle(pub Style);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Derived {
    Auto,
    Fixed(Option<Color>),
}

const NAMED: [(&str, Color); 16] = [
    ("black", Color::Black),
    ("red", Color::Red),
    ("green", Color::Green),
    ("yellow", Color::Yellow),
    ("blue", Color::Blue),
    ("magenta", Color::Magenta),
    ("cyan", Color::Cyan),
    ("gray", Color::Gray),
    ("darkgray", Color::DarkGray),
    ("lightred", Color::LightRed),
    ("lightgreen", Color::LightGreen),
    ("lightyellow", Color::LightYellow),
    ("lightblue", Color::LightBlue),
    ("lightmagenta", Color::LightMagenta),
    ("lightcyan", Color::LightCyan),
    ("white", Color::White),
];

pub fn parse_color(raw: &str) -> Result<Option<Color>, String> {
    let key: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace("grey", "gray");
    if matches!(key.as_str(), "none" | "default" | "reset" | "transparent") {
        return Ok(None);
    }
    if let Some(hex) = key.strip_prefix('#') {
        let digits: Option<Vec<u8>> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect();
        return match digits.as_deref() {
            Some([r, g, b]) => Ok(Some(Color::Rgb(r * 17, g * 17, b * 17))),
            Some([r1, r0, g1, g0, b1, b0]) => {
                Ok(Some(Color::Rgb(r1 * 16 + r0, g1 * 16 + g0, b1 * 16 + b0)))
            }
            _ => Err(format!("invalid hex colour {raw:?}; use #rgb or #rrggbb")),
        };
    }
    if let Some((_, c)) = NAMED.iter().find(|(n, _)| *n == key) {
        return Ok(Some(*c));
    }
    if let Ok(n) = key.trim_start_matches("color").parse::<u8>() {
        return Ok(Some(Color::Indexed(n)));
    }
    Err(format!(
        "unknown colour {raw:?}; use #rrggbb, #rgb, an ANSI index 0-255, none, or one of: {}",
        NAMED.map(|(n, _)| n).join(", ")
    ))
}

fn color_name(c: Option<Color>) -> String {
    match c {
        None | Some(Color::Reset) => "none".to_owned(),
        Some(Color::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        Some(Color::Indexed(n)) => format!("color{n}"),
        Some(other) => NAMED.iter().find(|(_, c)| *c == other).map_or_else(
            || format!("{other:?}").to_lowercase(),
            |(n, _)| (*n).to_owned(),
        ),
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&color_name(self.0))
    }
}

struct ColorVisitor;

impl Visitor<'_> for ColorVisitor {
    type Value = ThemeColor;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a colour: \"#rrggbb\", \"#rgb\", a name, \"none\" or an ANSI index 0-255")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<ThemeColor, E> {
        parse_color(v).map(ThemeColor).map_err(E::custom)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<ThemeColor, E> {
        u8::try_from(v)
            .map(|n| ThemeColor(Some(Color::Indexed(n))))
            .map_err(|_| E::custom(format!("ANSI colour index {v} is out of range 0-255")))
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ColorVisitor)
    }
}

impl Serialize for Derived {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Derived::Auto => s.serialize_str("auto"),
            Derived::Fixed(c) => s.serialize_str(&color_name(*c)),
        }
    }
}

struct DerivedVisitor;

impl Visitor<'_> for DerivedVisitor {
    type Value = Derived;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            "\"auto\" or a colour: \"#rrggbb\", \"#rgb\", a name, \"none\" or an ANSI index 0-255",
        )
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Derived, E> {
        if v.trim().eq_ignore_ascii_case("auto") {
            return Ok(Derived::Auto);
        }
        ColorVisitor.visit_str(v).map(|c| Derived::Fixed(c.0))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Derived, E> {
        ColorVisitor.visit_i64(v).map(|c| Derived::Fixed(c.0))
    }
}

impl<'de> Deserialize<'de> for Derived {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DerivedVisitor)
    }
}

const FLAGS: [(&str, Modifier); 6] = [
    ("bold", Modifier::BOLD),
    ("dim", Modifier::DIM),
    ("italic", Modifier::ITALIC),
    ("underline", Modifier::UNDERLINED),
    ("reversed", Modifier::REVERSED),
    ("strike", Modifier::CROSSED_OUT),
];

impl Serialize for ThemeStyle {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let st = self.0;
        if st.bg.is_none() && st.add_modifier.is_empty() {
            return s.serialize_str(&color_name(st.fg));
        }
        let mut map = s.serialize_map(None)?;
        if st.fg.is_some() {
            map.serialize_entry("fg", &color_name(st.fg))?;
        }
        if st.bg.is_some() {
            map.serialize_entry("bg", &color_name(st.bg))?;
        }
        for (name, m) in FLAGS {
            if st.add_modifier.contains(m) {
                map.serialize_entry(name, &true)?;
            }
        }
        map.end()
    }
}

struct StyleVisitor;

impl<'de> Visitor<'de> for StyleVisitor {
    type Value = ThemeStyle;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            "a colour or a table { fg, bg, bold, dim, italic, underline, reversed, strike }",
        )
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<ThemeStyle, E> {
        let c = ColorVisitor.visit_str(v)?;
        Ok(ThemeStyle(Style {
            fg: c.0,
            ..Style::default()
        }))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<ThemeStyle, E> {
        let c = ColorVisitor.visit_i64(v)?;
        Ok(ThemeStyle(Style {
            fg: c.0,
            ..Style::default()
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeStyle, A::Error> {
        let mut st = Style::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "fg" => st.fg = map.next_value::<ThemeColor>()?.0,
                "bg" => st.bg = map.next_value::<ThemeColor>()?.0,
                other => {
                    let Some((_, m)) = FLAGS.iter().find(|(n, _)| *n == other) else {
                        return Err(de::Error::custom(format!(
                            "unknown style key `{other}`; expected fg, bg, {}",
                            FLAGS.map(|(n, _)| n).join(", ")
                        )));
                    };
                    if map.next_value::<bool>()? {
                        st.add_modifier |= *m;
                    }
                }
            }
        }
        Ok(ThemeStyle(st))
    }
}

impl<'de> Deserialize<'de> for ThemeStyle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StyleVisitor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneWidth {
    Cols(u16),
    Percent(u16),
}

impl PaneWidth {
    pub fn constraint(self) -> Constraint {
        match self {
            PaneWidth::Cols(n) => Constraint::Length(n),
            PaneWidth::Percent(p) => Constraint::Percentage(p),
        }
    }
}

impl Serialize for PaneWidth {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PaneWidth::Cols(n) => s.serialize_u16(*n),
            PaneWidth::Percent(p) => s.serialize_str(&format!("{p}%")),
        }
    }
}

struct WidthVisitor;

impl Visitor<'_> for WidthVisitor {
    type Value = PaneWidth;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a number of columns (e.g. 30) or a percentage string (e.g. \"20%\")")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<PaneWidth, E> {
        match u16::try_from(v) {
            Ok(n) if (1..=200).contains(&n) => Ok(PaneWidth::Cols(n)),
            _ => Err(E::custom(format!(
                "pane width {v} is out of range 1-200 columns"
            ))),
        }
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<PaneWidth, E> {
        let p = v
            .trim()
            .strip_suffix('%')
            .and_then(|n| n.trim().parse::<u16>().ok())
            .ok_or_else(|| E::custom(format!("invalid pane width {v:?}; use 30 or \"20%\"")))?;
        if (1..=80).contains(&p) {
            Ok(PaneWidth::Percent(p))
        } else {
            Err(E::custom(format!(
                "pane width {v:?} is out of range 1%-80%"
            )))
        }
    }
}

impl<'de> Deserialize<'de> for PaneWidth {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(WidthVisitor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarShape {
    Circle,
    Rounded,
    Square,
}

impl AvatarShape {
    pub const ALL: [AvatarShape; 3] = [
        AvatarShape::Square,
        AvatarShape::Rounded,
        AvatarShape::Circle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AvatarShape::Square => "square",
            AvatarShape::Rounded => "rounded",
            AvatarShape::Circle => "circle",
        }
    }

    pub fn step(self, forward: bool) -> AvatarShape {
        crate::config::cycle(&Self::ALL, self, forward)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Clock {
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "12h")]
    H12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateSeparator {
    Auto,
    Line,
    Badge,
    Plain,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    pub background: ThemeColor,
    pub text: ThemeStyle,
    pub dim: ThemeStyle,
    pub strong: ThemeStyle,
    pub header: ThemeStyle,
    pub title: ThemeStyle,
    pub title_focused: ThemeStyle,
    pub border: ThemeStyle,
    pub border_focused: ThemeStyle,
    pub accent: ThemeStyle,
    pub selection: ThemeStyle,
    pub selection_border: ThemeStyle,
    pub selection_bar: ThemeStyle,
    pub selection_bg: Derived,
    pub picker_selection: ThemeStyle,
    pub own_name: ThemeStyle,
    pub name: ThemeStyle,
    pub name_palette: Vec<ThemeColor>,
    pub avatar_text: ThemeColor,
    pub link: ThemeStyle,
    pub mention: ThemeStyle,
    pub code: ThemeStyle,
    pub code_bar: ThemeStyle,
    pub spoiler: ThemeStyle,
    pub quote: ThemeStyle,
    pub reply: ThemeStyle,
    pub reply_text: ThemeStyle,
    pub forwarded: ThemeStyle,
    pub date_separator: ThemeStyle,
    pub date_badge: ThemeStyle,
    pub time: ThemeStyle,
    pub tick_pending: ThemeStyle,
    pub tick_sent: ThemeStyle,
    pub tick_read: ThemeStyle,
    pub tick_failed: ThemeStyle,
    pub edited: ThemeStyle,
    pub pinned: ThemeStyle,
    pub reactions: ThemeStyle,
    pub unread_title: ThemeStyle,
    pub unread_badge: ThemeStyle,
    pub muted_badge: ThemeStyle,
    pub mention_badge: ThemeStyle,
    pub open_chat: ThemeStyle,
    pub online: ThemeStyle,
    pub typing: ThemeStyle,
    pub status: ThemeStyle,
    pub error: ThemeStyle,
    pub warning: ThemeStyle,
    pub editing: ThemeStyle,
    pub media: ThemeStyle,
    pub waveform: ThemeStyle,
    pub recording: ThemeStyle,
    pub jump_badge: ThemeStyle,
    pub pinned_bar: ThemeStyle,
    pub popup_background: Derived,
    pub popup_border: ThemeStyle,
    pub popup_title: ThemeStyle,
    pub hint_key: ThemeStyle,
    pub hint_group: ThemeStyle,
    pub bubble_incoming: Derived,
    pub bubble_incoming_selected: Derived,
    pub bubble_outgoing: Derived,
    pub bubble_outgoing_selected: Derived,
    pub bubble_edge: ThemeStyle,
    pub card_border: ThemeStyle,
    pub card_fill: Derived,
    pub button_border: ThemeStyle,
    pub button_text: ThemeStyle,
    pub button_selected: ThemeStyle,
    pub scrollbar: ThemeStyle,
    pub scrollbar_thumb: ThemeStyle,
    pub qr: ThemeStyle,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutOptions {
    pub message_layout: MessageLayout,
    pub border_style: BorderStyle,
    pub folders_width: PaneWidth,
    pub chats_width: PaneWidth,
    pub info_width: PaneWidth,
    pub message_gutter: u16,
    pub composer_padding: u16,
    pub avatars: bool,
    pub bubble_avatar_rows: u16,
    pub bubble_max_width: u16,
    pub avatar_shape: AvatarShape,
    pub cards_max_width: u16,
    pub split_max_width: u16,
    pub compact_name_width: u16,
    pub clock: Clock,
    pub seconds: bool,
    pub date_separator: DateSeparator,
    pub scrollbar: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Glyphs {
    pub selection_marker: String,
    pub tick_pending: String,
    pub tick_sent: String,
    pub tick_read: String,
    pub tick_failed: String,
    pub pinned: String,
    pub pinned_chat: String,
    pub edited: String,
    pub unread_dot: String,
    pub mention_dot: String,
    pub reaction_badge: String,
    pub online: String,
    pub typing: String,
    pub secret: String,
    pub forwarded: String,
    pub reply_bar: String,
    pub compact_separator: String,
    pub bubble_reply_bar: String,
    pub quote_bar: String,
    pub code_bar: String,
    pub separator: String,
    pub selected_left: String,
    pub selected_right: String,
    pub selection_bar: String,
    pub attachment: String,
    pub video: String,
    pub video_note: String,
    pub voice: String,
    pub audio: String,
    pub file: String,
    pub gif: String,
    pub keyboard: String,
    pub clipboard: String,
    pub upload: String,
    pub download: String,
    pub playing: String,
    pub recording: String,
    pub recording_blink: String,
    pub jump: String,
    pub ghost: String,
    pub no_read: String,
    pub url_button: String,
    pub forum_closed: String,
    pub forum_open: String,
    pub topic: String,
    pub topic_closed: String,
    pub timer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub colors: Colors,
    pub layout: LayoutOptions,
    pub glyphs: Glyphs,
}

fn fg(c: Color) -> ThemeStyle {
    ThemeStyle(Style::default().fg(c))
}

fn bold(c: Color) -> ThemeStyle {
    ThemeStyle(Style::default().fg(c).add_modifier(Modifier::BOLD))
}

fn plain() -> ThemeStyle {
    ThemeStyle(Style::default())
}

fn modifier(m: Modifier) -> ThemeStyle {
    ThemeStyle(Style::default().add_modifier(m))
}

fn with(c: Color, m: Modifier) -> ThemeStyle {
    ThemeStyle(Style::default().fg(c).add_modifier(m))
}

fn on(f: Color, b: Color, m: Modifier) -> ThemeStyle {
    ThemeStyle(Style::default().fg(f).bg(b).add_modifier(m))
}

fn col(c: Color) -> ThemeColor {
    ThemeColor(Some(c))
}

const ORANGE: Color = Color::Rgb(255, 165, 0);

pub type Rgb = crate::termcolors::Rgb;

fn named_index(c: Color) -> Option<usize> {
    use Color::*;
    Some(match c {
        Black => 0,
        Red => 1,
        Green => 2,
        Yellow => 3,
        Blue => 4,
        Magenta => 5,
        Cyan => 6,
        Gray => 7,
        DarkGray => 8,
        LightRed => 9,
        LightGreen => 10,
        LightYellow => 11,
        LightBlue => 12,
        LightMagenta => 13,
        LightCyan => 14,
        White => 15,
        _ => return None,
    })
}

pub fn rgb_of(c: Option<Color>, fallback: Rgb, pal: &TermPalette) -> Rgb {
    match c {
        None | Some(Color::Reset) => fallback,
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        Some(Color::Indexed(n)) if n < 16 => pal.ansi[n as usize],
        Some(Color::Indexed(n)) if n >= 232 => {
            let v = 8 + 10 * (n - 232);
            (v, v, v)
        }
        Some(Color::Indexed(n)) => {
            let i = n - 16;
            let level = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        Some(other) => named_index(other).map_or(fallback, |i| pal.ansi[i]),
    }
}

pub fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

pub fn luminance(c: Rgb) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn saturation(c: Rgb) -> f32 {
    let hi = c.0.max(c.1).max(c.2);
    let lo = c.0.min(c.1).min(c.2);
    if hi == 0 {
        0.0
    } else {
        f32::from(hi - lo) / f32::from(hi)
    }
}

const MIN_CONTRAST: f32 = 4.5;

fn tint(base: Rgb, toward: Rgb, t: f32, text: Rgb) -> Rgb {
    let mut t = t;
    loop {
        let c = blend(base, toward, t);
        if t <= 0.0 || contrast(c, text) >= MIN_CONTRAST {
            return c;
        }
        t -= 0.02;
    }
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fills {
    pub selection_bg: Option<Color>,
    pub popup: Option<Color>,
    pub card: Option<Color>,
    pub bubble_in: Option<Color>,
    pub bubble_in_sel: Option<Color>,
    pub bubble_out: Option<Color>,
    pub bubble_out_sel: Option<Color>,
}

pub struct Bases {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
}

pub fn bases(c: &Colors, pal: &TermPalette) -> Bases {
    let foreground = rgb_of(c.text.0.fg, pal.fg, pal);
    Bases {
        background: rgb_of(c.background.0, pal.bg, pal),
        foreground,
        accent: rgb_of(c.accent.0.fg, foreground, pal),
    }
}

pub fn resolve(c: &Colors, pal: &TermPalette) -> Fills {
    let Bases {
        background: b,
        foreground: f,
        accent: a,
    } = bases(c, pal);
    let incoming = tint(b, f, 0.08, f);
    let auto = |d: Derived, v: Rgb| match d {
        Derived::Auto => Some(rgb(v)),
        Derived::Fixed(x) => x,
    };
    Fills {
        selection_bg: auto(c.selection_bg, tint(b, a, 0.18, f)),
        popup: auto(c.popup_background, tint(b, a, 0.05, f)),
        card: auto(c.card_fill, tint(b, a, 0.06, f)),
        bubble_in: auto(c.bubble_incoming, incoming),
        bubble_in_sel: auto(c.bubble_incoming_selected, tint(incoming, a, 0.16, f)),
        bubble_out: auto(c.bubble_outgoing, tint(b, a, 0.14, f)),
        bubble_out_sel: auto(c.bubble_outgoing_selected, tint(b, a, 0.26, f)),
    }
}

pub fn pick_accent(p: &TermPalette) -> Rgb {
    let order = [4, 12, 6, 14, 2, 10, 5, 13];
    order
        .iter()
        .map(|&i| p.ansi[i])
        .find(|c| contrast(*c, p.bg) >= MIN_CONTRAST && saturation(*c) >= 0.15)
        .unwrap_or_else(|| {
            order.iter().map(|&i| p.ansi[i]).fold(p.fg, |best, c| {
                if contrast(c, p.bg) > contrast(best, p.bg) && saturation(c) >= 0.15 {
                    c
                } else {
                    best
                }
            })
        })
}

fn terminal_colors(p: &TermPalette) -> Colors {
    let a = rgb(pick_accent(p));
    let line = rgb(blend(p.bg, p.fg, 0.3));
    let soft = rgb(blend(p.bg, p.fg, 0.62));
    let on_accent = rgb(p.bg);
    let badge = rgb(tint(p.bg, pick_accent(p), 0.12, p.fg));
    let mut c = default_colors();
    c.dim = fg(soft);
    c.header = bold(a);
    c.title_focused = bold(a);
    c.border = fg(line);
    c.border_focused = bold(a);
    c.accent = bold(a);
    c.selection = bold(a);
    c.selection_border = fg(a);
    c.selection_bar = bold(a);
    c.picker_selection = on(on_accent, a, Modifier::BOLD);
    c.mention = fg(a);
    c.code = fg(rgb(p.ansi[3]));
    c.quote = fg(a);
    c.reply = fg(line);
    c.forwarded = with(a, Modifier::ITALIC);
    c.date_badge = on(soft, badge, Modifier::empty());
    c.time = fg(soft);
    c.tick_read = fg(a);
    c.reactions = fg(a);
    c.mention_badge = bold(rgb(p.ansi[3]));
    c.jump_badge = on(on_accent, a, Modifier::BOLD);
    c.pinned_bar = bold(a);
    c.popup_border = bold(a);
    c.hint_key = bold(a);
    c.bubble_edge = fg(a);
    c.card_border = fg(line);
    c.button_border = fg(line);
    c.button_text = fg(a);
    c.button_selected = on(on_accent, a, Modifier::BOLD);
    c.scrollbar = fg(line);
    c.scrollbar_thumb = fg(a);
    c
}

fn default_layout() -> LayoutOptions {
    LayoutOptions {
        message_layout: MessageLayout::Bubbles,
        border_style: BorderStyle::Plain,
        folders_width: PaneWidth::Cols(18),
        chats_width: PaneWidth::Cols(30),
        info_width: PaneWidth::Cols(30),
        message_gutter: 2,
        composer_padding: 1,
        avatars: true,
        bubble_avatar_rows: 2,
        bubble_max_width: 68,
        avatar_shape: AvatarShape::Square,
        cards_max_width: 72,
        split_max_width: 70,
        compact_name_width: 12,
        clock: Clock::H24,
        seconds: false,
        date_separator: DateSeparator::Auto,
        scrollbar: false,
    }
}

fn default_glyphs() -> Glyphs {
    let s = |v: &str| v.to_owned();
    Glyphs {
        selection_marker: s("❯ "),
        tick_pending: s("◷"),
        tick_sent: s("✓"),
        tick_read: s("✓✓"),
        tick_failed: s("⚠"),
        pinned: s("📌"),
        pinned_chat: s("▴"),
        edited: s("auto"),
        unread_dot: String::new(),
        mention_dot: s("@"),
        reaction_badge: s("❤"),
        online: s("●"),
        typing: s("✎"),
        secret: s("🔒"),
        forwarded: s("↪"),
        reply_bar: s("│"),
        compact_separator: s("│"),
        bubble_reply_bar: s("▎"),
        quote_bar: s("▎"),
        code_bar: s("│"),
        separator: s("─"),
        selected_left: s("▌"),
        selected_right: s("▐"),
        selection_bar: s("▎"),
        attachment: s("📎"),
        video: s("▶"),
        video_note: s("◉"),
        voice: s("🎤"),
        audio: s("♪"),
        file: s("📄"),
        gif: s("GIF"),
        keyboard: s("⌨"),
        clipboard: s("📋"),
        upload: s("⇡"),
        download: s("⇣"),
        playing: s("▶"),
        recording: s("●"),
        recording_blink: s("○"),
        jump: s("↓"),
        ghost: s("👻"),
        no_read: s("◌"),
        url_button: s("↗"),
        forum_closed: s("▸"),
        forum_open: s("▾"),
        topic: s("#"),
        topic_closed: s("⊘"),
        timer: s("⏱"),
    }
}

fn default_colors() -> Colors {
    use Color::*;
    Colors {
        background: ThemeColor(None),
        text: plain(),
        dim: modifier(Modifier::DIM),
        strong: modifier(Modifier::BOLD),
        header: bold(Cyan),
        title: modifier(Modifier::BOLD),
        title_focused: modifier(Modifier::BOLD),
        border: fg(DarkGray),
        border_focused: bold(Cyan),
        accent: bold(Cyan),
        selection: bold(Cyan),
        selection_border: fg(Green),
        selection_bar: bold(Cyan),
        selection_bg: Derived::Auto,
        picker_selection: with(Cyan, Modifier::BOLD | Modifier::REVERSED),
        own_name: modifier(Modifier::BOLD),
        name: modifier(Modifier::BOLD),
        name_palette: vec![
            col(Red),
            col(Green),
            col(Yellow),
            col(Blue),
            col(Magenta),
            col(Cyan),
            col(ORANGE),
        ],
        avatar_text: col(Black),
        link: with(Blue, Modifier::UNDERLINED),
        mention: fg(Cyan),
        code: fg(Rgb(230, 170, 90)),
        code_bar: modifier(Modifier::DIM),
        spoiler: on(DarkGray, DarkGray, Modifier::empty()),
        quote: fg(Cyan),
        reply: fg(DarkGray),
        reply_text: modifier(Modifier::DIM),
        forwarded: with(Cyan, Modifier::ITALIC),
        date_separator: modifier(Modifier::DIM),
        date_badge: ThemeStyle(
            Style::default()
                .bg(Rgb(38, 41, 50))
                .add_modifier(Modifier::DIM),
        ),
        time: modifier(Modifier::DIM),
        tick_pending: modifier(Modifier::DIM),
        tick_sent: modifier(Modifier::DIM),
        tick_read: fg(Cyan),
        tick_failed: bold(Red),
        edited: modifier(Modifier::DIM | Modifier::ITALIC),
        pinned: plain(),
        reactions: fg(Cyan),
        unread_title: modifier(Modifier::BOLD),
        unread_badge: modifier(Modifier::BOLD),
        muted_badge: modifier(Modifier::DIM),
        mention_badge: fg(ORANGE),
        open_chat: fg(Green),
        online: fg(Green),
        typing: plain(),
        status: bold(Yellow),
        error: bold(Red),
        warning: bold(Yellow),
        editing: fg(Yellow),
        media: with(Magenta, Modifier::ITALIC),
        waveform: with(Magenta, Modifier::ITALIC),
        recording: bold(Red),
        jump_badge: on(Black, Cyan, Modifier::BOLD),
        pinned_bar: bold(Cyan),
        popup_background: Derived::Auto,
        popup_border: bold(Cyan),
        popup_title: modifier(Modifier::BOLD),
        hint_key: bold(Cyan),
        hint_group: modifier(Modifier::DIM | Modifier::BOLD),
        bubble_incoming: Derived::Auto,
        bubble_incoming_selected: Derived::Auto,
        bubble_outgoing: Derived::Auto,
        bubble_outgoing_selected: Derived::Auto,
        bubble_edge: fg(Cyan),
        card_border: fg(DarkGray),
        card_fill: Derived::Auto,
        button_border: fg(DarkGray),
        button_text: fg(Cyan),
        button_selected: on(Black, Cyan, Modifier::BOLD),
        scrollbar: fg(DarkGray),
        scrollbar_thumb: fg(Cyan),
        qr: on(White, Black, Modifier::empty()),
    }
}

fn high_contrast_colors() -> Colors {
    use Color::*;
    let gold = Rgb(255, 215, 0);
    Colors {
        background: col(Black),
        text: fg(White),
        dim: fg(Rgb(200, 200, 200)),
        strong: bold(White),
        header: on(Black, gold, Modifier::BOLD),
        title: bold(White),
        title_focused: bold(gold),
        border: fg(Rgb(160, 160, 160)),
        border_focused: bold(gold),
        accent: bold(gold),
        selection: on(Black, gold, Modifier::BOLD),
        selection_border: bold(LightGreen),
        selection_bar: bold(gold),
        selection_bg: Derived::Auto,
        picker_selection: on(Black, gold, Modifier::BOLD),
        own_name: bold(LightGreen),
        name: modifier(Modifier::BOLD),
        name_palette: vec![
            col(LightRed),
            col(LightGreen),
            col(LightYellow),
            col(LightBlue),
            col(LightMagenta),
            col(LightCyan),
            col(Rgb(255, 180, 60)),
        ],
        avatar_text: col(Black),
        link: with(LightCyan, Modifier::UNDERLINED),
        mention: bold(LightCyan),
        code: fg(Rgb(255, 200, 120)),
        code_bar: fg(Rgb(160, 160, 160)),
        spoiler: on(Rgb(90, 90, 90), Rgb(90, 90, 90), Modifier::empty()),
        quote: fg(LightCyan),
        reply: fg(Rgb(160, 160, 160)),
        reply_text: fg(Rgb(200, 200, 200)),
        forwarded: with(LightCyan, Modifier::ITALIC),
        date_separator: fg(Rgb(200, 200, 200)),
        date_badge: on(Black, Rgb(200, 200, 200), Modifier::BOLD),
        time: fg(Rgb(200, 200, 200)),
        tick_pending: fg(Rgb(200, 200, 200)),
        tick_sent: fg(White),
        tick_read: bold(LightCyan),
        tick_failed: bold(LightRed),
        edited: with(Rgb(200, 200, 200), Modifier::ITALIC),
        pinned: fg(gold),
        reactions: fg(LightCyan),
        unread_title: bold(White),
        unread_badge: on(Black, White, Modifier::BOLD),
        muted_badge: fg(Rgb(160, 160, 160)),
        mention_badge: on(Black, Rgb(255, 140, 0), Modifier::BOLD),
        open_chat: bold(LightGreen),
        online: bold(LightGreen),
        typing: with(LightCyan, Modifier::ITALIC),
        status: bold(LightYellow),
        error: on(White, Red, Modifier::BOLD),
        warning: bold(LightYellow),
        editing: bold(LightYellow),
        media: with(LightMagenta, Modifier::ITALIC),
        waveform: bold(LightMagenta),
        recording: bold(LightRed),
        jump_badge: on(Black, gold, Modifier::BOLD),
        pinned_bar: bold(gold),
        popup_background: Derived::Auto,
        popup_border: bold(gold),
        popup_title: bold(gold),
        hint_key: bold(gold),
        hint_group: with(White, Modifier::BOLD | Modifier::UNDERLINED),
        bubble_incoming: Derived::Auto,
        bubble_incoming_selected: Derived::Auto,
        bubble_outgoing: Derived::Auto,
        bubble_outgoing_selected: Derived::Auto,
        bubble_edge: bold(gold),
        card_border: fg(Rgb(160, 160, 160)),
        card_fill: Derived::Auto,
        button_border: fg(Rgb(200, 200, 200)),
        button_text: bold(gold),
        button_selected: on(Black, gold, Modifier::BOLD),
        scrollbar: fg(Rgb(120, 120, 120)),
        scrollbar_thumb: fg(gold),
        qr: on(White, Black, Modifier::empty()),
    }
}

fn light_colors() -> Colors {
    use Color::*;
    let ink = Rgb(30, 30, 36);
    let grey = Rgb(110, 110, 120);
    let blue = Rgb(0, 95, 200);
    let paper = Rgb(250, 250, 248);
    Colors {
        background: col(paper),
        text: fg(ink),
        dim: fg(grey),
        strong: bold(ink),
        header: bold(blue),
        title: bold(ink),
        title_focused: bold(blue),
        border: fg(Rgb(180, 180, 190)),
        border_focused: bold(blue),
        accent: bold(blue),
        selection: bold(blue),
        selection_border: fg(Rgb(40, 140, 60)),
        selection_bar: bold(blue),
        selection_bg: Derived::Auto,
        picker_selection: on(White, blue, Modifier::BOLD),
        own_name: bold(Rgb(40, 130, 60)),
        name: modifier(Modifier::BOLD),
        name_palette: vec![
            col(Rgb(198, 40, 40)),
            col(Rgb(46, 125, 50)),
            col(Rgb(154, 103, 0)),
            col(Rgb(21, 101, 192)),
            col(Rgb(142, 36, 170)),
            col(Rgb(0, 131, 143)),
            col(Rgb(230, 81, 0)),
        ],
        avatar_text: col(White),
        link: with(blue, Modifier::UNDERLINED),
        mention: fg(blue),
        code: fg(Rgb(166, 38, 100)),
        code_bar: fg(grey),
        spoiler: on(Rgb(190, 190, 195), Rgb(190, 190, 195), Modifier::empty()),
        quote: fg(blue),
        reply: fg(grey),
        reply_text: fg(grey),
        forwarded: with(blue, Modifier::ITALIC),
        date_separator: fg(grey),
        date_badge: on(grey, Rgb(232, 232, 236), Modifier::empty()),
        time: fg(grey),
        tick_pending: fg(grey),
        tick_sent: fg(grey),
        tick_read: fg(blue),
        tick_failed: bold(Rgb(198, 40, 40)),
        edited: with(grey, Modifier::ITALIC),
        pinned: plain(),
        reactions: fg(blue),
        unread_title: bold(ink),
        unread_badge: bold(blue),
        muted_badge: fg(grey),
        mention_badge: bold(Rgb(230, 81, 0)),
        open_chat: bold(Rgb(40, 130, 60)),
        online: fg(Rgb(40, 140, 60)),
        typing: with(blue, Modifier::ITALIC),
        status: bold(Rgb(154, 103, 0)),
        error: bold(Rgb(198, 40, 40)),
        warning: bold(Rgb(154, 103, 0)),
        editing: fg(Rgb(154, 103, 0)),
        media: with(Rgb(142, 36, 170), Modifier::ITALIC),
        waveform: fg(Rgb(142, 36, 170)),
        recording: bold(Rgb(198, 40, 40)),
        jump_badge: on(White, blue, Modifier::BOLD),
        pinned_bar: bold(blue),
        popup_background: Derived::Auto,
        popup_border: bold(blue),
        popup_title: bold(ink),
        hint_key: bold(blue),
        hint_group: with(grey, Modifier::BOLD),
        bubble_incoming: Derived::Auto,
        bubble_incoming_selected: Derived::Auto,
        bubble_outgoing: Derived::Auto,
        bubble_outgoing_selected: Derived::Auto,
        bubble_edge: fg(blue),
        card_border: fg(Rgb(180, 180, 190)),
        card_fill: Derived::Auto,
        button_border: fg(Rgb(170, 180, 200)),
        button_text: fg(blue),
        button_selected: on(White, blue, Modifier::BOLD),
        scrollbar: fg(Rgb(200, 200, 205)),
        scrollbar_thumb: fg(blue),
        qr: on(White, Black, Modifier::empty()),
    }
}

fn mono_colors() -> Colors {
    let none = ThemeColor(None);
    let b = Modifier::BOLD;
    let d = Modifier::DIM;
    Colors {
        background: none,
        text: plain(),
        dim: modifier(d),
        strong: modifier(b),
        header: modifier(b),
        title: plain(),
        title_focused: modifier(b),
        border: modifier(d),
        border_focused: modifier(b),
        accent: modifier(b),
        selection: modifier(Modifier::REVERSED),
        selection_border: modifier(b),
        selection_bar: modifier(b),
        selection_bg: Derived::Fixed(None),
        picker_selection: modifier(Modifier::REVERSED),
        own_name: modifier(b | Modifier::UNDERLINED),
        name: modifier(b),
        name_palette: vec![none],
        avatar_text: none,
        link: modifier(Modifier::UNDERLINED),
        mention: modifier(b),
        code: modifier(Modifier::ITALIC),
        code_bar: modifier(d),
        spoiler: modifier(Modifier::REVERSED | d),
        quote: modifier(d),
        reply: modifier(d),
        reply_text: modifier(d),
        forwarded: modifier(Modifier::ITALIC),
        date_separator: modifier(d),
        date_badge: modifier(d),
        time: modifier(d),
        tick_pending: modifier(d),
        tick_sent: modifier(d),
        tick_read: modifier(b),
        tick_failed: modifier(b | Modifier::REVERSED),
        edited: modifier(d | Modifier::ITALIC),
        pinned: plain(),
        reactions: plain(),
        unread_title: modifier(b),
        unread_badge: modifier(b),
        muted_badge: modifier(d),
        mention_badge: modifier(b | Modifier::REVERSED),
        open_chat: modifier(Modifier::UNDERLINED),
        online: modifier(b),
        typing: modifier(Modifier::ITALIC),
        status: modifier(b),
        error: modifier(b | Modifier::REVERSED),
        warning: modifier(b),
        editing: modifier(b),
        media: modifier(Modifier::ITALIC),
        waveform: plain(),
        recording: modifier(b),
        jump_badge: modifier(Modifier::REVERSED),
        pinned_bar: modifier(b),
        popup_background: Derived::Fixed(None),
        popup_border: modifier(b),
        popup_title: modifier(b),
        hint_key: modifier(b),
        hint_group: modifier(d | Modifier::UNDERLINED),
        bubble_incoming: Derived::Fixed(None),
        bubble_incoming_selected: Derived::Fixed(None),
        bubble_outgoing: Derived::Fixed(None),
        bubble_outgoing_selected: Derived::Fixed(None),
        bubble_edge: modifier(b),
        card_border: modifier(d),
        card_fill: Derived::Fixed(None),
        button_border: modifier(d),
        button_text: plain(),
        button_selected: modifier(Modifier::REVERSED | b),
        scrollbar: modifier(d),
        scrollbar_thumb: modifier(b),
        qr: on(Color::White, Color::Black, Modifier::empty()),
    }
}

pub const PRESETS: [ThemeChoice; 5] = [
    ThemeChoice::Terminal,
    ThemeChoice::Default,
    ThemeChoice::HighContrast,
    ThemeChoice::Light,
    ThemeChoice::Mono,
];

impl Default for Theme {
    fn default() -> Self {
        Theme::preset(ThemeChoice::Terminal)
    }
}

const REMOVED_KEYS: [(&str, &str); 4] = [
    ("colors", "surface"),
    ("layout", "nerd_font"),
    ("glyphs", "soft_cap_left"),
    ("glyphs", "soft_cap_right"),
];

fn is_auto(v: &toml::Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("auto"))
}

fn colour_table(colors: &Colors) -> toml::Table {
    match toml::Value::try_from(colors) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    }
}

fn derived_keys() -> Vec<String> {
    colour_table(&default_colors())
        .into_iter()
        .filter(|(_, v)| is_auto(v))
        .map(|(k, _)| k)
        .collect()
}

fn palette_driven(choice: ThemeChoice) -> Vec<String> {
    let a = crate::termcolors::FALLBACK;
    let inv = |(r, g, b): Rgb| (255 - r, 255 - g, 255 - b);
    let b = TermPalette {
        fg: inv(a.fg),
        bg: inv(a.bg),
        ansi: a.ansi.map(inv),
    };
    let x = colour_table(&Theme::preset_with(choice, Some(&a)).colors);
    let y = colour_table(&Theme::preset_with(choice, Some(&b)).colors);
    x.into_iter()
        .filter(|(k, v)| y.get(k) != Some(v))
        .map(|(k, _)| k)
        .collect()
}

fn canonical_key<'a>(section: &str, key: &'a str) -> &'a str {
    match (section, key) {
        ("layout", "messages") => "message_layout",
        _ => key,
    }
}

fn path_of(section: &str, key: &str) -> String {
    format!("{section}.{key}")
}

impl Theme {
    pub fn preset(choice: ThemeChoice) -> Theme {
        Theme::preset_with(choice, crate::termcolors::get().as_ref())
    }

    pub fn preset_with(choice: ThemeChoice, pal: Option<&TermPalette>) -> Theme {
        let mut layout = default_layout();
        let mut glyphs = default_glyphs();
        let colors = match choice {
            ThemeChoice::Terminal => pal.map_or_else(default_colors, terminal_colors),
            ThemeChoice::Default | ThemeChoice::Custom => default_colors(),
            ThemeChoice::HighContrast => {
                layout.border_style = BorderStyle::Thick;
                layout.scrollbar = true;
                high_contrast_colors()
            }
            ThemeChoice::Light => {
                layout.message_layout = MessageLayout::Bubbles;
                light_colors()
            }
            ThemeChoice::Mono => {
                layout.border_style = BorderStyle::Plain;
                layout.avatars = false;
                layout.date_separator = DateSeparator::Line;
                glyphs.selection_marker = "> ".to_owned();
                glyphs.pinned = "*".to_owned();
                glyphs.attachment = "+".to_owned();
                glyphs.voice = "[voice]".to_owned();
                glyphs.file = "[file]".to_owned();
                glyphs.clipboard = "[clip]".to_owned();
                glyphs.ghost = "ghost".to_owned();
                glyphs.secret = "[s]".to_owned();
                glyphs.timer = "[t]".to_owned();
                glyphs.topic_closed = "x".to_owned();
                glyphs.unread_dot = "*".to_owned();
                glyphs.reaction_badge = "<3".to_owned();
                mono_colors()
            }
        };
        Theme {
            colors,
            layout,
            glyphs,
        }
    }

    pub fn to_toml(&self, choice: ThemeChoice) -> Result<String, String> {
        let value = toml::Value::try_from(self).map_err(|e| e.to_string())?;
        let toml::Value::Table(root) = value else {
            return Err("theme did not serialise to a table".to_owned());
        };
        let driven = palette_driven(choice);
        let mut out = format!("base = \"{}\"\n", choice.base_name());
        for section in ["colors", "layout", "glyphs"] {
            let Some(toml::Value::Table(t)) = root.get(section) else {
                return Err(format!("theme section {section} is missing"));
            };
            out.push_str(&format!("\n[{section}]\n"));
            for (k, v) in t {
                if section == "colors" && driven.contains(k) {
                    out.push_str(&format!("{k} = \"auto\"\n"));
                } else {
                    out.push_str(&format!("{k} = {v}\n"));
                }
            }
        }
        Ok(out)
    }

    pub fn parse(raw: &str, notes: &mut Vec<String>) -> Result<Theme, String> {
        let user: toml::Table = raw.parse().map_err(|e: toml::de::Error| {
            let line = e
                .span()
                .map(|s| raw[..s.start.min(raw.len())].matches('\n').count() + 1);
            match line {
                Some(l) => format!("line {l}: {}", e.message()),
                None => e.message().to_owned(),
            }
        })?;
        let base = match user.get("base") {
            None => ThemeChoice::Terminal,
            Some(toml::Value::String(name)) => PRESETS
                .into_iter()
                .find(|p| p.base_name() == name)
                .ok_or_else(|| {
                    format!(
                        "base: unknown preset {name:?}; use one of {}",
                        PRESETS.map(|p| p.base_name()).join(", ")
                    )
                })?,
            Some(other) => return Err(format!("base: expected a preset name, got {other}")),
        };
        let toml::Value::Table(mut merged) =
            toml::Value::try_from(Theme::preset(base)).map_err(|e| e.to_string())?
        else {
            return Err("theme did not serialise to a table".to_owned());
        };
        let derived = derived_keys();
        for (section, value) in &user {
            if section == "base" {
                continue;
            }
            let Some(toml::Value::Table(target)) = merged.get_mut(section) else {
                return Err(format!(
                    "unknown section [{section}]; expected [colors], [layout] or [glyphs]"
                ));
            };
            let toml::Value::Table(entries) = value else {
                return Err(format!("{section} must be a table"));
            };
            for (key, v) in entries {
                let key = canonical_key(section, key);
                if REMOVED_KEYS.contains(&(section.as_str(), key)) {
                    continue;
                }
                if key == "message_layout" && v.as_str() == Some("soft") {
                    notes.push(format!("theme.toml: {}", crate::config::SOFT_REMOVED));
                }
                if !target.contains_key(key) {
                    return Err(format!("unknown key {}", path_of(section, key)));
                }
                if section == "colors" && is_auto(v) && !derived.contains(&key.to_owned()) {
                    continue;
                }
                target.insert(key.to_owned(), v.clone());
            }
        }
        let theme = match Theme::deserialize(toml::Value::Table(merged)) {
            Ok(t) => t,
            Err(e) => return Err(locate_error(&user, base, e.message())),
        };
        theme.validate()?;
        Ok(theme)
    }

    fn validate(&self) -> Result<(), String> {
        let l = &self.layout;
        let range = |name: &str, v: u16, lo: u16, hi: u16| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("layout.{name}: {v} is out of range {lo}-{hi}"))
            }
        };
        range("message_gutter", l.message_gutter, 0, 8)?;
        range("composer_padding", l.composer_padding, 0, 4)?;
        range("bubble_avatar_rows", l.bubble_avatar_rows, 1, 4)?;
        range("bubble_max_width", l.bubble_max_width, 20, 100)?;
        range("cards_max_width", l.cards_max_width, 30, 100)?;
        range("split_max_width", l.split_max_width, 30, 100)?;
        range("compact_name_width", l.compact_name_width, 4, 32)?;
        if self.colors.name_palette.is_empty() {
            return Err("colors.name_palette must contain at least one colour".to_owned());
        }
        let value = toml::Value::try_from(&self.glyphs).map_err(|e| e.to_string())?;
        if let toml::Value::Table(t) = value {
            for (k, v) in t {
                if v.as_str().is_some_and(|s| s.chars().any(char::is_control)) {
                    return Err(format!("glyphs.{k}: control characters are not allowed"));
                }
            }
        }
        if self.glyphs.separator.chars().count() != 1 {
            return Err("glyphs.separator must be exactly one character".to_owned());
        }
        Ok(())
    }

    pub fn name_color(&self, id: i64) -> Option<Color> {
        let p = &self.colors.name_palette;
        p[id.unsigned_abs() as usize % p.len()].0
    }
}

fn locate_error(user: &toml::Table, base: ThemeChoice, msg: &str) -> String {
    let Ok(toml::Value::Table(clean)) = toml::Value::try_from(Theme::preset(base)) else {
        return msg.to_owned();
    };
    for (section, value) in user {
        let Some(entries) = value.as_table() else {
            continue;
        };
        for (key, v) in entries {
            let mut probe = clean.clone();
            if let Some(toml::Value::Table(t)) = probe.get_mut(section) {
                t.insert(canonical_key(section, key).to_owned(), v.clone());
            }
            if let Err(e) = Theme::deserialize(toml::Value::Table(probe)) {
                return format!("{}: {}", path_of(section, key), e.message());
            }
        }
    }
    msg.to_owned()
}

impl ThemeChoice {
    pub fn base_name(self) -> &'static str {
        match self {
            ThemeChoice::Terminal => "terminal",
            ThemeChoice::Default | ThemeChoice::Custom => "default",
            ThemeChoice::HighContrast => "high-contrast",
            ThemeChoice::Light => "light",
            ThemeChoice::Mono => "mono",
        }
    }
}

pub fn load_custom(path: &std::path::Path, notes: &mut Vec<String>) -> Result<Theme, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Theme::parse(&raw, notes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Theme::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn load(
    choice: ThemeChoice,
    path: &std::path::Path,
    notes: &mut Vec<String>,
) -> Result<Theme, String> {
    match choice {
        ThemeChoice::Custom => load_custom(path, notes),
        other => Ok(Theme::preset(other)),
    }
}

#[cfg(target_os = "linux")]
pub fn watch(
    path: &std::path::Path,
    notify: impl Fn(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    use rustix::fs::inotify::{self, CreateFlags, ReadFlags, Reader, WatchFlags};
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("cannot watch {}", path.display()));
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let fd = inotify::init(CreateFlags::CLOEXEC).map_err(|e| format!("inotify: {e}"))?;
    inotify::add_watch(
        &fd,
        dir,
        WatchFlags::CLOSE_WRITE | WatchFlags::MOVED_TO | WatchFlags::DELETE,
    )
    .map_err(|e| format!("cannot watch {}: {e}", dir.display()))?;
    let name = name.as_encoded_bytes().to_vec();
    let shown = path.display().to_string();
    std::thread::Builder::new()
        .name("theme-watch".to_owned())
        .spawn(move || {
            let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
            let mut reader = Reader::new(&fd, &mut buf);
            loop {
                match reader.next() {
                    Ok(ev) if ev.events().contains(ReadFlags::IGNORED) => {
                        notify(Err(format!("stopped watching {shown}: directory removed")));
                        return;
                    }
                    Ok(ev) => {
                        if ev.file_name().is_some_and(|n| n.to_bytes() == name) {
                            notify(Ok(()));
                        }
                    }
                    Err(rustix::io::Errno::INTR) => {}
                    Err(e) => {
                        notify(Err(format!("stopped watching {shown}: {e}")));
                        return;
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|e| format!("cannot start theme watcher: {e}"))
}

#[cfg(target_os = "macos")]
pub fn watch(
    path: &std::path::Path,
    notify: impl Fn(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    let Some(dir) = path.parent() else {
        return Err(format!("cannot watch {}", path.display()));
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = path.to_path_buf();
    let stamp = |p: &std::path::Path| -> Result<Option<(std::time::SystemTime, u64)>, String> {
        match std::fs::metadata(p) {
            Ok(m) => m
                .modified()
                .map(|t| Some((t, m.len())))
                .map_err(|e| format!("stopped watching {}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("stopped watching {}: {e}", p.display())),
        }
    };
    let mut last = stamp(&path)?;
    std::thread::Builder::new()
        .name("theme-watch".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                match stamp(&path) {
                    Ok(now) if now != last => {
                        last = now;
                        notify(Ok(()));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        notify(Err(e));
                        return;
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|e| format!("cannot start theme watcher: {e}"))
}

struct Installed {
    theme: Rc<Theme>,
    fills: Fills,
}

impl Installed {
    fn new(theme: Rc<Theme>) -> Installed {
        let fills = resolve(&theme.colors, &crate::termcolors::base());
        Installed { theme, fills }
    }
}

thread_local! {
    static CURRENT: RefCell<Installed> = RefCell::new(Installed::new(Rc::new(Theme::default())));
}

pub fn install(theme: Rc<Theme>) {
    CURRENT.with(|c| *c.borrow_mut() = Installed::new(theme));
}

pub fn get() -> Rc<Theme> {
    CURRENT.with(|c| c.borrow().theme.clone())
}

pub fn fills() -> Fills {
    CURRENT.with(|c| c.borrow().fills)
}

fn pick(f: impl FnOnce(&Colors) -> ThemeStyle) -> Style {
    CURRENT.with(|c| f(&c.borrow().theme.colors).0)
}

pub fn border_set() -> border::Set<'static> {
    match get().layout.border_style {
        BorderStyle::Rounded => border::ROUNDED,
        BorderStyle::Plain => border::PLAIN,
        BorderStyle::Thick => border::THICK,
        BorderStyle::Double => border::DOUBLE,
    }
}

pub fn selection_marker() -> String {
    get().glyphs.selection_marker.clone()
}

pub fn surface() -> Style {
    let t = get();
    let mut st = t.colors.text.0;
    st.bg = t.colors.background.0;
    st
}

pub fn popup_surface() -> Style {
    let mut st = surface();
    if let Some(bg) = fills().popup {
        st.bg = Some(bg);
    }
    st
}

pub fn popup_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(border_set())
        .border_style(popup_border())
        .style(popup_surface())
}

pub fn normal() -> Style {
    pick(|c| c.text)
}

pub fn strong() -> Style {
    pick(|c| c.strong)
}

pub fn muted() -> Style {
    pick(|c| c.dim)
}

pub fn topic_icon(color: i32) -> Style {
    if color <= 0 {
        return muted();
    }
    let [_, r, g, b] = color.to_be_bytes();
    Style::default().fg(Color::Rgb(r, g, b))
}

pub fn border() -> Style {
    pick(|c| c.border)
}

pub fn focus_border() -> Style {
    pick(|c| c.border_focused)
}

pub fn title(focused: bool) -> Style {
    if focused {
        pick(|c| c.title_focused)
    } else {
        pick(|c| c.title)
    }
}

pub fn selection() -> Style {
    let st = pick(|c| c.selection);
    if st.add_modifier.contains(Modifier::DIM) {
        st
    } else {
        st.remove_modifier(Modifier::DIM)
    }
}

pub fn selection_border() -> Style {
    pick(|c| c.selection_border)
}

pub fn selection_bar() -> Style {
    pick(|c| c.selection_bar)
}

pub fn selection_bg() -> Option<Color> {
    fills().selection_bg
}

pub fn button_border() -> Style {
    pick(|c| c.button_border)
}

pub fn button_text() -> Style {
    pick(|c| c.button_text)
}

pub fn button_selected() -> Style {
    pick(|c| c.button_selected)
}

pub fn picker_selection() -> Style {
    pick(|c| c.picker_selection)
}

pub fn header_title() -> Style {
    pick(|c| c.header)
}

pub fn active_field() -> Style {
    pick(|c| c.accent)
}

pub fn error() -> Style {
    pick(|c| c.error)
}

pub fn warning() -> Style {
    pick(|c| c.warning)
}

pub fn status() -> Style {
    pick(|c| c.status)
}

pub fn editing() -> Style {
    pick(|c| c.editing)
}

pub fn unread() -> Style {
    pick(|c| c.unread_title)
}

pub fn unread_badge() -> Style {
    pick(|c| c.unread_badge)
}

pub fn muted_badge() -> Style {
    pick(|c| c.muted_badge)
}

pub fn mention_badge() -> Style {
    pick(|c| c.mention_badge)
}

pub fn open_chat() -> Style {
    pick(|c| c.open_chat)
}

pub fn online() -> Style {
    pick(|c| c.online)
}

pub fn typing() -> Style {
    pick(|c| c.typing)
}

pub fn link() -> Style {
    pick(|c| c.link)
}

pub fn mention() -> Style {
    pick(|c| c.mention)
}

pub fn code() -> Style {
    pick(|c| c.code)
}

pub fn code_bar() -> Style {
    pick(|c| c.code_bar)
}

pub fn spoiler() -> Style {
    pick(|c| c.spoiler)
}

pub fn quote() -> Style {
    pick(|c| c.quote)
}

pub fn reply_text() -> Style {
    pick(|c| c.reply_text)
}

pub fn forwarded() -> Style {
    pick(|c| c.forwarded)
}

pub fn date_separator() -> Style {
    pick(|c| c.date_separator)
}

pub fn date_badge() -> Style {
    pick(|c| c.date_badge)
}

pub fn time() -> Style {
    pick(|c| c.time)
}

pub fn edited() -> Style {
    pick(|c| c.edited)
}

pub fn pinned() -> Style {
    pick(|c| c.pinned)
}

pub fn reactions() -> Style {
    pick(|c| c.reactions)
}

pub fn media() -> Style {
    pick(|c| c.media)
}

pub fn waveform() -> Style {
    pick(|c| c.waveform)
}

pub fn recording() -> Style {
    pick(|c| c.recording)
}

pub fn jump_badge() -> Style {
    pick(|c| c.jump_badge)
}

pub fn pinned_bar() -> Style {
    pick(|c| c.pinned_bar)
}

pub fn popup_border() -> Style {
    pick(|c| c.popup_border)
}

pub fn popup_title() -> Style {
    pick(|c| c.popup_title)
}

pub fn hint_key() -> Style {
    pick(|c| c.hint_key)
}

pub fn hint_group() -> Style {
    pick(|c| c.hint_group)
}

pub fn card_border() -> Style {
    pick(|c| c.card_border)
}

pub fn card_fill() -> Option<Color> {
    fills().card
}

pub fn cell_bg(bg: Option<Color>) -> Color {
    bg.unwrap_or(Color::Reset)
}

pub fn bubble_edge() -> Style {
    pick(|c| c.bubble_edge)
}

pub fn scrollbar() -> Style {
    pick(|c| c.scrollbar)
}

pub fn scrollbar_thumb() -> Style {
    pick(|c| c.scrollbar_thumb)
}

pub fn qr() -> Style {
    pick(|c| c.qr)
}

pub fn author(id: i64, own: bool) -> Style {
    let t = get();
    let base = if own {
        t.colors.own_name.0
    } else {
        t.colors.name.0
    };
    match (base.fg, t.name_color(id)) {
        (None, Some(c)) => base.fg(c),
        _ => base,
    }
}

pub fn reply_bar(sender: Option<i64>) -> Style {
    let t = get();
    match sender.and_then(|id| t.name_color(id)) {
        Some(c) => Style::default().fg(c),
        None => t.colors.reply.0,
    }
}

pub fn avatar(id: i64) -> Style {
    let t = get();
    let mut st = t.colors.strong.0;
    st.fg = t.colors.avatar_text.0;
    st.bg = t.name_color(id);
    st
}

pub fn tick(state: Tick) -> (String, Style) {
    let t = get();
    let (g, c) = match state {
        Tick::Pending => (&t.glyphs.tick_pending, t.colors.tick_pending),
        Tick::Sent => (&t.glyphs.tick_sent, t.colors.tick_sent),
        Tick::Read => (&t.glyphs.tick_read, t.colors.tick_read),
        Tick::Failed => (&t.glyphs.tick_failed, t.colors.tick_failed),
    };
    (g.clone(), c.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Pending,
    Sent,
    Read,
    Failed,
}

pub fn bubble_bg(outgoing: bool, selected: bool) -> Option<Color> {
    let f = fills();
    match (outgoing, selected) {
        (true, true) => f.bubble_out_sel,
        (true, false) => f.bubble_out,
        (false, true) => f.bubble_in_sel,
        (false, false) => f.bubble_in,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_quiet(raw: &str) -> Result<Theme, String> {
        Theme::parse(raw, &mut Vec::new())
    }

    fn olive() -> TermPalette {
        let mut p = crate::termcolors::FALLBACK;
        p.bg = (19, 21, 12);
        p.fg = (227, 228, 211);
        p.ansi[2] = (184, 207, 132);
        p.ansi[4] = (190, 203, 152);
        p.ansi[6] = (160, 210, 180);
        p
    }

    #[test]
    fn blend_math_is_linear_and_clamped() {
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(blend((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
        assert_eq!(blend((10, 20, 30), (110, 120, 130), 0.1), (20, 30, 40));
        assert_eq!(blend((0, 0, 0), (255, 0, 0), 2.0), (255, 0, 0));
        assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast((119, 119, 119), (255, 255, 255)) - 4.48).abs() < 0.02);
        assert_eq!(
            rgb_of(Some(Color::Cyan), (1, 1, 1), &olive()),
            (160, 210, 180)
        );
        assert_eq!(rgb_of(None, (1, 2, 3), &olive()), (1, 2, 3));
        assert_eq!(
            rgb_of(Some(Color::Indexed(196)), (0, 0, 0), &olive()),
            (255, 0, 0)
        );
        assert_eq!(
            rgb_of(Some(Color::Indexed(232)), (0, 0, 0), &olive()),
            (8, 8, 8)
        );
    }

    #[test]
    fn derived_fills_follow_the_theme_and_stay_readable() {
        let samples = [crate::termcolors::FALLBACK, olive(), {
            let mut p = crate::termcolors::FALLBACK;
            p.bg = (250, 248, 240);
            p.fg = (30, 30, 30);
            p.ansi[4] = (40, 90, 200);
            p
        }];
        for pal in samples {
            for choice in PRESETS {
                let t = Theme::preset_with(choice, Some(&pal));
                let f = resolve(&t.colors, &pal);
                let base = bases(&t.colors, &pal);
                for (name, fill) in [
                    ("selection_bg", f.selection_bg),
                    ("popup", f.popup),
                    ("card", f.card),
                    ("bubble_in", f.bubble_in),
                    ("bubble_in_sel", f.bubble_in_sel),
                    ("bubble_out", f.bubble_out),
                    ("bubble_out_sel", f.bubble_out_sel),
                ] {
                    let Some(Color::Rgb(r, g, b)) = fill else {
                        assert_eq!(choice, ThemeChoice::Mono, "{name} {choice:?}");
                        continue;
                    };
                    let ratio = contrast((r, g, b), base.foreground);
                    assert!(ratio >= 4.5, "{choice:?} {name} {ratio} {pal:?}");
                }
            }
        }
        let pal = olive();
        let t = Theme::preset_with(ThemeChoice::Terminal, Some(&pal));
        let a = pick_accent(&pal);
        assert_eq!(t.colors.accent.0.fg, Some(rgb(a)));
        assert_eq!(t.colors.selection_bar.0.fg, Some(rgb(a)));
        assert!(contrast(a, pal.bg) >= 4.5);
        let f = resolve(&t.colors, &pal);
        assert_eq!(f.card, Some(rgb(blend(pal.bg, a, 0.06))));
        assert_eq!(f.selection_bg, Some(rgb(blend(pal.bg, a, 0.18))));
        assert_eq!(f.bubble_in, Some(rgb(blend(pal.bg, pal.fg, 0.08))));
        let Some(Color::Rgb(r, g, b)) = f.selection_bg else {
            panic!()
        };
        assert!(g > b, "olive tint, not cold slate: {:?}", (r, g, b));
        let custom = parse_quiet(
            "base = \"terminal\"\n[colors]\ncard_fill = \"#102030\"\nselection_bg = \"auto\"\naccent = \"#ff0000\"\n",
        )
        .unwrap();
        assert_eq!(
            custom.colors.card_fill,
            Derived::Fixed(Some(Color::Rgb(16, 32, 48)))
        );
        let f = resolve(&custom.colors, &pal);
        assert_eq!(f.card, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(
            f.selection_bg,
            Some(rgb(tint(pal.bg, (255, 0, 0), 0.18, pal.fg)))
        );
        assert!(
            custom
                .to_toml(ThemeChoice::Custom)
                .unwrap()
                .contains("selection_bg = \"auto\"")
        );
        assert_eq!(
            Theme::preset_with(ThemeChoice::Terminal, None),
            Theme::preset_with(ThemeChoice::Default, None)
        );
    }

    #[test]
    fn colours_parse_and_reject_garbage() {
        assert_eq!(parse_color("#ff8000"), Ok(Some(Color::Rgb(255, 128, 0))));
        assert_eq!(parse_color("#f80"), Ok(Some(Color::Rgb(255, 136, 0))));
        assert_eq!(parse_color("Dark_Grey"), Ok(Some(Color::DarkGray)));
        assert_eq!(parse_color("light-cyan"), Ok(Some(Color::LightCyan)));
        assert_eq!(parse_color("200"), Ok(Some(Color::Indexed(200))));
        assert_eq!(parse_color("none"), Ok(None));
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("bluish").unwrap_err().contains("bluish"));
        assert!(parse_color("300").is_err());
    }

    #[test]
    fn every_preset_round_trips_through_toml() {
        for p in PRESETS {
            let t = Theme::preset(p);
            let text = t.to_toml(p).unwrap();
            assert!(text.starts_with(&format!("base = \"{}\"", p.base_name())));
            assert_eq!(parse_quiet(&text).unwrap(), t, "{p:?}");
        }
        assert_ne!(Theme::preset(ThemeChoice::Light), Theme::default());
        assert_ne!(Theme::preset(ThemeChoice::Mono), Theme::default());
        assert_ne!(Theme::preset(ThemeChoice::HighContrast), Theme::default());
    }

    #[test]
    fn written_terminal_theme_keeps_derived_roles_auto() {
        let mut pal = crate::termcolors::FALLBACK;
        pal.bg = (19, 21, 12);
        pal.fg = (227, 228, 211);
        pal.ansi[4] = (190, 203, 152);
        let t = Theme::preset_with(ThemeChoice::Terminal, Some(&pal));
        let text = t.to_toml(ThemeChoice::Terminal).unwrap();
        for key in [
            "accent",
            "card_border",
            "time",
            "dim",
            "selection_bg",
            "card_fill",
        ] {
            assert!(
                text.contains(&format!("\n{key} = \"auto\"\n")),
                "{key}\n{text}"
            );
        }
        assert!(!text.contains(&format!("{:?}", pal.bg)), "{text}");
        assert!(text.contains("\nbackground = \"none\"\n"), "{text}");
        let parsed = parse_quiet(&text).unwrap();
        assert_eq!(parsed.colors, Theme::preset(ThemeChoice::Terminal).colors);
        let other = parse_quiet(&text.replace("base = \"terminal\"", "base = \"light\"")).unwrap();
        assert_eq!(
            other.colors.accent,
            Theme::preset(ThemeChoice::Light).colors.accent
        );
        for p in [ThemeChoice::Default, ThemeChoice::Light, ThemeChoice::Mono] {
            let text = Theme::preset(p).to_toml(p).unwrap();
            assert!(!text.contains("\naccent = \"auto\""), "{p:?}\n{text}");
            assert!(!text.contains("\nheader = \"auto\""), "{p:?}\n{text}");
        }
        let custom = parse_quiet("[colors]\nheader = \"auto\"\naccent = \"#ff0000\"\n").unwrap();
        assert_eq!(
            custom.colors.header,
            Theme::preset(ThemeChoice::Terminal).colors.header
        );
        assert_eq!(custom.colors.accent.0.fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn removed_soft_options_are_accepted() {
        let mut notes = Vec::new();
        let t = Theme::parse(
            "[layout]\nmessages = \"soft\"\nnerd_font = false\n[colors]\nsurface = \"#101010\"\n[glyphs]\nsoft_cap_left = \"(\"\nsoft_cap_right = \")\"\n",
            &mut notes,
        )
        .unwrap();
        assert_eq!(t.layout.message_layout, MessageLayout::Classic);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("soft") && n.contains("classic")),
            "{notes:?}"
        );
    }

    #[test]
    fn partial_file_inherits_from_base() {
        let t = parse_quiet(
            "base = \"light\"\n[colors]\nlink = { fg = \"#ff0000\", bold = true }\nborder = 33\n[layout]\nchats_width = \"25%\"\nclock = \"12h\"\n[glyphs]\ntick_read = \"RR\"\n",
        )
        .unwrap();
        let light = Theme::preset(ThemeChoice::Light);
        assert_eq!(
            t.colors.link.0,
            Style::default()
                .fg(Color::Rgb(255, 0, 0))
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(t.colors.border.0.fg, Some(Color::Indexed(33)));
        assert_eq!(t.colors.text, light.colors.text);
        assert_eq!(t.layout.chats_width, PaneWidth::Percent(25));
        assert_eq!(t.layout.clock, Clock::H12);
        assert_eq!(t.layout.message_layout, MessageLayout::Bubbles);
        assert_eq!(t.glyphs.tick_read, "RR");
        assert_eq!(parse_quiet("").unwrap(), Theme::default());
    }

    #[test]
    fn bad_files_give_precise_errors() {
        let err = |raw: &str| parse_quiet(raw).unwrap_err();
        assert_eq!(err("[colors]\nlnk = \"red\"\n"), "unknown key colors.lnk");
        assert!(err("[colours]\n").contains("unknown section [colours]"));
        let e = err("[colors]\ntext = \"bluish\"\n");
        assert!(
            e.starts_with("colors.text: unknown colour \"bluish\""),
            "{e}"
        );
        let e = err("[colors]\nlink = { fg = \"red\", blink = true }\n");
        assert!(
            e.starts_with("colors.link: unknown style key `blink`"),
            "{e}"
        );
        let e = err("[layout]\nmessage_layout = \"fancy\"\n");
        assert!(e.starts_with("layout.message_layout:"), "{e}");
        let e = err("[layout]\nmessage_gutter = 30\n");
        assert_eq!(e, "layout.message_gutter: 30 is out of range 0-8");
        let e = err("[layout]\nchats_width = \"95%\"\n");
        assert!(e.starts_with("layout.chats_width:"), "{e}");
        let e = err("[layout]\nscrollbar = \"yes\"\n");
        assert!(e.starts_with("layout.scrollbar:"), "{e}");
        assert!(err("base = \"neon\"\n").contains("unknown preset \"neon\""));
        assert!(err("[colors]\nname_palette = []\n").contains("name_palette"));
        assert!(err("[glyphs]\nseparator = \"==\"\n").contains("glyphs.separator"));
        assert!(err("[colors\n").starts_with("line 1:"));
        assert!(err("[glyphs]\npinned = 5\n").starts_with("glyphs.pinned:"));
    }

    #[test]
    fn message_layouts_and_their_options_parse() {
        for (name, layout) in [
            ("classic", MessageLayout::Classic),
            ("cards", MessageLayout::Cards),
            ("compact", MessageLayout::Compact),
            ("minimal", MessageLayout::Minimal),
            ("bubbles", MessageLayout::Bubbles),
            ("split", MessageLayout::Split),
        ] {
            let t = parse_quiet(&format!("[layout]\nmessages = \"{name}\"\n")).unwrap();
            assert_eq!(t.layout.message_layout, layout);
            assert_eq!(layout.name(), name);
        }
        let t = parse_quiet(
            "[layout]\ncompact_name_width = 8\ncards_max_width = 90\navatar_shape = \"rounded\"\n[colors]\ncard_fill = \"#202020\"\ncard_border = { fg = \"#ff0000\" }\n[glyphs]\ncompact_separator = \"┃\"\n",
        )
        .unwrap();
        assert_eq!(t.layout.compact_name_width, 8);
        assert_eq!(t.layout.cards_max_width, 90);
        assert_eq!(t.layout.avatar_shape, AvatarShape::Rounded);
        assert_eq!(
            t.colors.card_fill,
            Derived::Fixed(Some(Color::Rgb(32, 32, 32)))
        );
        assert_eq!(t.glyphs.compact_separator, "┃");
        assert_eq!(Theme::default().colors.card_fill, Derived::Auto);
        for p in PRESETS {
            assert_eq!(Theme::preset(p).layout.avatar_shape, AvatarShape::Square);
        }
        assert_eq!(
            Theme::preset_with(ThemeChoice::Terminal, Some(&crate::termcolors::FALLBACK))
                .layout
                .avatar_shape,
            AvatarShape::Square
        );
        let err = |raw: &str| parse_quiet(raw).unwrap_err();
        let e = err("[layout]\nmessages = \"fancy\"\n");
        assert!(e.starts_with("layout.messages: unknown variant"), "{e}");
        assert_eq!(
            err("[layout]\ncompact_name_width = 2\n"),
            "layout.compact_name_width: 2 is out of range 4-32"
        );
        assert!(err("[layout]\navatar_shape = \"hexagon\"\n").starts_with("layout.avatar_shape:"));
        let t = parse_quiet(
            "[layout]\nsplit_max_width = 50\n[colors]\ncard_fill = \"none\"\nselection_bg = \"#101010\"\nbutton_selected = { fg = \"black\", bg = \"yellow\" }\n[glyphs]\nselection_bar = \"┃\"\n",
        )
        .unwrap();
        assert_eq!(t.layout.split_max_width, 50);
        assert_eq!(t.colors.card_fill, Derived::Fixed(None));
        assert_eq!(
            t.colors.selection_bg,
            Derived::Fixed(Some(Color::Rgb(16, 16, 16)))
        );
        assert_eq!(t.colors.button_selected.0.bg, Some(Color::Yellow));
        assert_eq!(t.glyphs.selection_bar, "┃");
        assert_eq!(
            err("[layout]\nsplit_max_width = 10\n"),
            "layout.split_max_width: 10 is out of range 30-100"
        );
        for p in PRESETS.into_iter().filter(|p| *p != ThemeChoice::Mono) {
            let t = Theme::preset(p);
            let pal = crate::termcolors::FALLBACK;
            let bg = rgb(bases(&t.colors, &pal).background);
            assert_ne!(
                resolve(&t.colors, &pal).card,
                Some(bg),
                "{p:?} card fill stands out"
            );
        }
        let mono = Theme::preset(ThemeChoice::Mono);
        assert_eq!(
            resolve(&mono.colors, &crate::termcolors::FALLBACK).card,
            None
        );
    }

    #[test]
    fn no_colours_are_hard_coded_outside_the_theme() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "theme.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let code = src.split("#[cfg(test)]").next().unwrap();
            for (i, line) in code.lines().enumerate() {
                let hit = line.contains("Rgb(")
                    || line.contains(".fg(")
                    || line.contains(".bg(")
                    || (line.contains("Color::") && !line.contains("Color::Light"));
                assert!(!hit, "{}:{}: {line}", path.display(), i + 1);
            }
        }
    }

    #[test]
    fn watcher_reports_changes_to_the_theme_file_only() {
        let dir = std::env::temp_dir().join(format!("gramtty-watch-{}", std::process::id()));
        let path = dir.join("theme.toml");
        let (tx, rx) = std::sync::mpsc::channel();
        watch(&path, move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
        let wait = std::time::Duration::from_secs(5);
        std::fs::write(dir.join("other.toml"), "x").unwrap();
        std::fs::write(&path, "[layout]\nseconds = true\n").unwrap();
        assert_eq!(rx.recv_timeout(wait), Ok(Ok(())));
        assert!(rx.try_recv().is_err());
        let tmp = dir.join("theme.toml.swp");
        std::fs::write(&tmp, "").unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        assert_eq!(rx.recv_timeout(wait), Ok(Ok(())));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(rx.recv_timeout(wait), Ok(Ok(())));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_custom_file_is_default_and_bad_file_errors() {
        let dir = std::env::temp_dir().join(format!("gramtty-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("theme.toml");
        assert_eq!(
            load(ThemeChoice::Custom, &path, &mut Vec::new()).unwrap(),
            Theme::default()
        );
        std::fs::write(&path, "[layout]\nbogus = 1\n").unwrap();
        let e = load(ThemeChoice::Custom, &path, &mut Vec::new()).unwrap_err();
        assert!(e.contains("theme.toml: unknown key layout.bogus"), "{e}");
        assert_eq!(
            load(ThemeChoice::Mono, &path, &mut Vec::new()).unwrap(),
            Theme::preset(ThemeChoice::Mono)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
