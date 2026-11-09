use chrono::{DateTime, Datelike, Local, TimeZone};
use ratatui::buffer::CellWidth;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::{EntityKind, RichText};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    None,
    Quote,
    Pre,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    pub block: Block,
}

pub fn styled_cells(rt: &RichText, base: Style) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(rt.text.len());
    let mut pos16 = 0usize;
    for ch in rt.text.chars() {
        let mut style = base;
        let mut block = Block::None;
        for e in &rt.entities {
            if pos16 < e.offset || pos16 >= e.offset + e.length {
                continue;
            }
            match &e.kind {
                EntityKind::Bold => style = style.add_modifier(Modifier::BOLD),
                EntityKind::Italic => style = style.add_modifier(Modifier::ITALIC),
                EntityKind::Underline => style = style.add_modifier(Modifier::UNDERLINED),
                EntityKind::Strike => style = style.add_modifier(Modifier::CROSSED_OUT),
                EntityKind::Spoiler => style = style.patch(theme::spoiler()),
                EntityKind::Code => style = style.patch(theme::code()),
                EntityKind::Pre => {
                    style = style.patch(theme::code());
                    block = Block::Pre;
                }
                EntityKind::Link(_) => style = style.patch(theme::link()),
                EntityKind::Mention => style = style.patch(theme::mention()),
                EntityKind::Quote => {
                    style = style.add_modifier(Modifier::ITALIC);
                    if block == Block::None {
                        block = Block::Quote;
                    }
                }
            }
        }
        pos16 += ch.len_utf16();
        if ch == '\t' {
            for _ in 0..4 {
                cells.push(Cell {
                    ch: ' ',
                    style,
                    block,
                });
            }
        } else if ch == '\n' || ch.width().is_some() {
            cells.push(Cell { ch, style, block });
        }
    }
    cells
}

fn cells_to_line(prefix: Option<Span<'static>>, cells: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = prefix.into_iter().collect();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for c in cells {
        if cur != Some(c.style) {
            if let Some(style) = cur
                && !buf.is_empty()
            {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
            }
            cur = Some(c.style);
        }
        buf.push(c.ch);
    }
    if let Some(style) = cur
        && !buf.is_empty()
    {
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

fn block_prefix(block: Block) -> Option<Span<'static>> {
    match block {
        Block::None => None,
        Block::Quote => Some(Span::styled(
            format!("{} ", theme::get().glyphs.quote_bar),
            theme::quote(),
        )),
        Block::Pre => Some(Span::styled(
            format!("{} ", theme::get().glyphs.code_bar),
            theme::code_bar(),
        )),
    }
}

pub fn wrap_cells(cells: &[Cell], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for logical in cells.split(|c| c.ch == '\n') {
        let block = logical.first().map_or(Block::None, |c| c.block);
        let prefix = block_prefix(block);
        let avail = width
            .saturating_sub(prefix.as_ref().map_or(0, |p| p.content.width()))
            .max(1);
        if logical.is_empty() {
            out.push(cells_to_line(prefix, &[]));
            continue;
        }
        let mut start = 0usize;
        let mut line_w = 0usize;
        let mut last_space: Option<usize> = None;
        let mut i = 0usize;
        while i < logical.len() {
            let w = logical[i].ch.width().unwrap_or(0);
            if line_w + w > avail && i > start {
                let (end, next) = match last_space {
                    _ if logical[i].ch == ' ' => (i, i + 1),
                    Some(sp) if sp > start => (sp, sp + 1),
                    _ => (i, i),
                };
                out.push(cells_to_line(prefix.clone(), &logical[start..end]));
                start = next;
                i = next;
                line_w = 0;
                last_space = None;
                continue;
            }
            if logical[i].ch == ' ' {
                last_space = Some(i);
            }
            line_w += w;
            i += 1;
        }
        out.push(cells_to_line(prefix.clone(), &logical[start..]));
    }
    out
}

pub fn render_text(rt: &RichText, base: Style, width: usize) -> Vec<Line<'static>> {
    wrap_cells(&styled_cells(rt, base), width)
}

pub fn utf16_slice(text: &str, offset: usize, length: usize) -> String {
    let mut pos = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        if pos >= offset + length {
            break;
        }
        if pos >= offset {
            out.push(ch);
        }
        pos += ch.len_utf16();
    }
    out
}

pub struct Link {
    pub url: String,
    pub hidden: bool,
}

pub fn links(rt: &RichText) -> Vec<Link> {
    rt.entities
        .iter()
        .filter_map(|e| {
            let shown = utf16_slice(&rt.text, e.offset, e.length);
            match &e.kind {
                EntityKind::Link(Some(url)) => Some(Link {
                    hidden: normalize_url(shown.trim()) != normalize_url(url),
                    url: url.clone(),
                }),
                EntityKind::Link(None) => Some(Link {
                    url: shown,
                    hidden: false,
                }),
                _ => None,
            }
        })
        .filter(|l| !l.url.is_empty())
        .collect()
}

pub fn normalize_url(url: &str) -> String {
    if url.contains("://") || url.starts_with("mailto:") || url.starts_with("tg:") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

pub fn duration(secs: i32) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

pub fn ttl(secs: i32) -> String {
    if secs < 0 {
        return "view once".to_owned();
    }
    if secs == 0 {
        return "off".to_owned();
    }
    for (unit, label) in [(604_800, "w"), (86_400, "d"), (3600, "h"), (60, "m")] {
        if secs % unit == 0 {
            return format!("{}{label}", secs / unit);
        }
    }
    format!("{secs}s")
}

pub fn count(n: i64, exact: bool) -> String {
    let a = n.unsigned_abs();
    if exact || a < 1000 {
        return n.to_string();
    }
    let sign = if n < 0 { "-" } else { "" };
    let (value, unit) = if a < 1_000_000 {
        (a as f64 / 1000.0, "K")
    } else if a < 1_000_000_000 {
        (a as f64 / 1_000_000.0, "M")
    } else {
        (a as f64 / 1_000_000_000.0, "B")
    };
    let tenths = (value * 10.0).floor() / 10.0;
    if tenths >= 100.0 || tenths.fract() == 0.0 {
        format!("{sign}{}{unit}", tenths.floor() as u64)
    } else {
        format!("{sign}{tenths:.1}{unit}")
    }
}

pub fn stamp(ts: i64) -> String {
    local(ts).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

pub fn file_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{} B", bytes.max(0))
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", b / 1024.0 / 1024.0 / 1024.0)
    }
}

fn local(ts: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(ts, 0).single()
}

pub fn time_label(ts: i64) -> String {
    let layout = &theme::get().layout;
    let pattern = match (layout.clock, layout.seconds) {
        (theme::Clock::H24, false) => "%H:%M",
        (theme::Clock::H24, true) => "%H:%M:%S",
        (theme::Clock::H12, false) => "%-I:%M %p",
        (theme::Clock::H12, true) => "%-I:%M:%S %p",
    };
    local(ts).map_or_else(String::new, |t| t.format(pattern).to_string())
}

pub fn date_label(ts: i64) -> String {
    local(ts).map_or_else(String::new, |t| t.format("%Y-%m-%d").to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    pub fn from_locale(value: Option<&str>) -> Lang {
        match value {
            Some(v) if v.to_ascii_lowercase().starts_with("en") => Lang::En,
            _ => Lang::Ru,
        }
    }

    pub fn detect() -> Lang {
        let value = ["LC_ALL", "LC_TIME", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty());
        Lang::from_locale(value.as_deref())
    }
}

const MONTHS_RU: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn day_label(lang: Lang, ts: i64, now: i64) -> String {
    let (Some(t), Some(n)) = (local(ts), local(now)) else {
        return String::new();
    };
    let (day, today) = (t.date_naive(), n.date_naive());
    if day == today {
        return match lang {
            Lang::Ru => "Сегодня",
            Lang::En => "Today",
        }
        .to_owned();
    }
    if today.pred_opt() == Some(day) {
        return match lang {
            Lang::Ru => "Вчера",
            Lang::En => "Yesterday",
        }
        .to_owned();
    }
    let month = day.month0() as usize;
    let same_year = day.year() == today.year();
    match (lang, same_year) {
        (Lang::Ru, true) => format!("{} {}", day.day(), MONTHS_RU[month]),
        (Lang::Ru, false) => format!("{} {} {}", day.day(), MONTHS_RU[month], day.year()),
        (Lang::En, true) => format!("{} {}", MONTHS_EN[month], day.day()),
        (Lang::En, false) => format!("{} {}, {}", MONTHS_EN[month], day.day(), day.year()),
    }
}

pub fn same_day(a: i64, b: i64) -> bool {
    match (local(a), local(b)) {
        (Some(a), Some(b)) => a.date_naive() == b.date_naive(),
        _ => false,
    }
}

const VS16: char = '\u{FE0F}';
const VS15: char = '\u{FE0E}';

const WIDE_MODIFIER_BASES: [char; 9] = [
    '\u{261D}',
    '\u{26F9}',
    '\u{270C}',
    '\u{270D}',
    '\u{1F3CB}',
    '\u{1F3CC}',
    '\u{1F574}',
    '\u{1F575}',
    '\u{1F590}',
];

const NARROW_IN_TERMINAL: [(u32, u32); 8] = [
    (0x1F1AE, 0x1F1AE),
    (0x1F6D9, 0x1F6D9),
    (0x1F7DA, 0x1F7DA),
    (0x1FA8B, 0x1FA8D),
    (0x1FACC, 0x1FACC),
    (0x1FADD, 0x1FADD),
    (0x1FAEB, 0x1FAEB),
    (0x1FAF9, 0x1FAFA),
];

pub const EMOJI_PLACEHOLDER: &str = "•";

fn narrow_in_terminal(c: char) -> bool {
    let c = c as u32;
    NARROW_IN_TERMINAL
        .iter()
        .any(|(lo, hi)| (*lo..=*hi).contains(&c))
}

fn regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

fn private_use(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c) || c >= '\u{F0000}'
}

pub fn grapheme_cells(g: &str) -> usize {
    if g.contains(char::is_control) {
        0
    } else {
        usize::from(g.cell_width())
    }
}

pub fn cells(text: &str) -> usize {
    text.graphemes(true).map(grapheme_cells).sum()
}

fn safe_grapheme(g: &str, out: &mut String) {
    let mut chars = g.chars();
    let first = chars.next();
    let lone = chars.next().is_none();
    match first {
        Some('\u{3164}' | '\u{FFA0}') if lone => out.push(' '),
        Some(c) if lone && WIDE_MODIFIER_BASES.contains(&c) => {
            out.push(c);
            out.push(VS16);
        }
        Some(c) if lone && regional(c) => out.push('\u{FFFD}'),
        Some(c) if narrow_in_terminal(c) => out.push('\u{FFFD}'),
        _ => out.push_str(g),
    }
}

pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for g in text.graphemes(true) {
        safe_grapheme(g, &mut out);
    }
    out
}

fn qualify(g: &str) -> String {
    let chars: Vec<char> = g.chars().filter(|c| *c != VS16).collect();
    let mut out = String::with_capacity(g.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        out.push(*c);
        let next = chars.get(i + 1).copied();
        let modifier = next.is_some_and(|n| ('\u{1F3FB}'..='\u{1F3FF}').contains(&n));
        if c.width() == Some(1)
            && next != Some(VS15)
            && !modifier
            && format!("{c}{VS16}").width() == 2
        {
            out.push(VS16);
        }
    }
    out
}

pub fn emoji_cell(label: &str) -> Option<String> {
    let mut out = String::new();
    for g in label.graphemes(true) {
        let q = qualify(g);
        let mut safe = String::new();
        safe_grapheme(&q, &mut safe);
        let ok = grapheme_cells(&safe) == 2
            && !safe.contains('\u{FFFD}')
            && !safe.chars().any(private_use);
        if !ok {
            return None;
        }
        out.push_str(&safe);
    }
    (!out.is_empty()).then_some(out)
}

pub fn truncate(text: &str, width: usize) -> String {
    let text = sanitize(text);
    if cells(&text) <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for g in text.graphemes(true) {
        let gw = grapheme_cells(g);
        if w + gw + 1 > width {
            break;
        }
        out.push_str(g);
        w += gw;
    }
    out.push('…');
    out
}

pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn separator(label: &str, width: usize) -> String {
    let lw = label.width();
    let total = width.max(lw + 2);
    let dashes = total - lw;
    let left = dashes / 2;
    let rule = theme::get().glyphs.separator.clone();
    format!(
        "{}{}{}",
        rule.repeat(left),
        label,
        rule.repeat(dashes - left)
    )
}

pub fn quote_block(text: &str, author: &str, when: &str) -> String {
    let mut out: Vec<String> = text
        .trim_end()
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_owned()
            } else {
                format!("> {l}")
            }
        })
        .collect();
    let sign = match (author.is_empty(), when.is_empty()) {
        (false, false) => format!("— {author}, {when}"),
        (false, true) => format!("— {author}"),
        (true, false) => format!("— {when}"),
        (true, true) => String::new(),
    };
    if !sign.is_empty() {
        out.push(sign);
    }
    out.join("\n")
}

pub fn quote_position(full: &str, fragment: &str) -> Option<i32> {
    if fragment.is_empty() {
        return None;
    }
    let byte = full.find(fragment)?;
    i32::try_from(full[..byte].encode_utf16().count()).ok()
}

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn counts_round_unless_exact() {
        assert_eq!(count(999, false), "999");
        assert_eq!(count(1000, false), "1K");
        assert_eq!(count(1234, false), "1.2K");
        assert_eq!(count(1299, false), "1.2K");
        assert_eq!(count(15_670, false), "15.6K");
        assert_eq!(count(999_999, false), "999K");
        assert_eq!(count(2_500_000, false), "2.5M");
        assert_eq!(count(3_000_000_000, false), "3B");
        assert_eq!(count(1234, true), "1234");
        assert_eq!(count(2_500_000, true), "2500000");
        assert_eq!(count(-1500, false), "-1.5K");
    }

    #[test]
    fn ttl_labels() {
        assert_eq!(ttl(0), "off");
        assert_eq!(ttl(-1), "view once");
        assert_eq!(ttl(5), "5s");
        assert_eq!(ttl(30), "30s");
        assert_eq!(ttl(90), "90s");
        assert_eq!(ttl(60), "1m");
        assert_eq!(ttl(3600), "1h");
        assert_eq!(ttl(7200), "2h");
        assert_eq!(ttl(86_400), "1d");
        assert_eq!(ttl(604_800), "1w");
        assert_eq!(ttl(2_678_400), "31d");
    }

    use super::*;

    #[test]
    fn quotes_prefix_lines_and_sign() {
        assert_eq!(
            quote_block("one\n\ntwo\n", "Alice", "2026-11-09 10:00"),
            "> one\n>\n> two\n— Alice, 2026-11-09 10:00"
        );
        assert_eq!(quote_block("x", "", ""), "> x");
        assert_eq!(quote_position("ab😀cd", "cd"), Some(4));
        assert_eq!(quote_position("привет мир", "мир"), Some(7));
        assert_eq!(quote_position("abc", "zz"), None);
        assert_eq!(quote_position("abc", ""), None);
    }
    use crate::model::Entity;

    #[test]
    fn day_labels_are_localised() {
        let now = Local
            .with_ymd_and_hms(2026, 11, 9, 15, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let at = |y, m, d| {
            Local
                .with_ymd_and_hms(y, m, d, 10, 0, 0)
                .single()
                .unwrap()
                .timestamp()
        };
        assert_eq!(day_label(Lang::Ru, now - 60, now), "Сегодня");
        assert_eq!(day_label(Lang::Ru, at(2026, 11, 8), now), "Вчера");
        assert_eq!(day_label(Lang::Ru, at(2026, 11, 1), now), "1 ноября");
        assert_eq!(day_label(Lang::Ru, at(2025, 3, 2), now), "2 марта 2025");
        assert_eq!(day_label(Lang::En, at(2026, 11, 1), now), "November 1");
        assert_eq!(day_label(Lang::En, now, now), "Today");
        assert_eq!(Lang::from_locale(Some("en_US.UTF-8")), Lang::En);
        assert_eq!(Lang::from_locale(Some("ru_RU.UTF-8")), Lang::Ru);
        assert_eq!(Lang::from_locale(None), Lang::Ru);
    }

    fn ent(offset: usize, length: usize, kind: EntityKind) -> Entity {
        Entity {
            offset,
            length,
            kind,
        }
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn entities_apply_styles_by_utf16_offset() {
        let rt = RichText {
            text: "😀 bold".to_owned(),
            entities: vec![ent(3, 4, EntityKind::Bold)],
        };
        let cells = styled_cells(&rt, Style::default());
        assert!(!cells[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(!cells[1].style.add_modifier.contains(Modifier::BOLD));
        assert!(cells[2].style.add_modifier.contains(Modifier::BOLD));
        assert!(cells[5].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn nested_entities_combine() {
        let rt = RichText {
            text: "abc".to_owned(),
            entities: vec![ent(0, 3, EntityKind::Bold), ent(1, 1, EntityKind::Italic)],
        };
        let cells = styled_cells(&rt, Style::default());
        assert!(
            cells[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
        );
        assert!(!cells[2].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn spans_merge_by_style() {
        let rt = RichText {
            text: "say hi now".to_owned(),
            entities: vec![ent(4, 2, EntityKind::Code)],
        };
        let lines = render_text(&rt, Style::default(), 80);
        assert_eq!(lines.len(), 1);
        let spans: Vec<&str> = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(spans, vec!["say ", "hi", " now"]);
        assert_eq!(lines[0].spans[1].style.fg, theme::code().fg);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let lines = render_text(
            &RichText::plain("hello brave new world"),
            Style::default(),
            11,
        );
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["hello brave", "new world"]);
    }

    #[test]
    fn hard_wraps_long_words_and_wide_chars() {
        let lines = render_text(&RichText::plain("abcdefgh"), Style::default(), 3);
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["abc", "def", "gh"]);
        let lines = render_text(&RichText::plain("你好世界"), Style::default(), 5);
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["你好", "世界"]);
    }

    #[test]
    fn newlines_and_blocks() {
        let rt = RichText {
            text: "intro\nquoted\n\nend".to_owned(),
            entities: vec![ent(6, 6, EntityKind::Quote)],
        };
        let lines = render_text(&rt, Style::default(), 40);
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["intro", "▎ quoted", "", "end"]);
    }

    #[test]
    fn urls_from_entities() {
        let rt = RichText {
            text: "see example.com and docs".to_owned(),
            entities: vec![
                ent(4, 11, EntityKind::Link(None)),
                ent(20, 4, EntityKind::Link(Some("https://docs.rs".to_owned()))),
            ],
        };
        let urls: Vec<String> = links(&rt).into_iter().map(|l| l.url).collect();
        assert_eq!(urls, vec!["example.com", "https://docs.rs"]);
        let hidden: Vec<bool> = links(&rt).iter().map(|l| l.hidden).collect();
        assert_eq!(hidden, vec![false, true]);
        let same = RichText {
            text: "docs.rs".to_owned(),
            entities: vec![ent(
                0,
                7,
                EntityKind::Link(Some("https://docs.rs".to_owned())),
            )],
        };
        assert!(!links(&same)[0].hidden);
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://a.b"), "https://a.b");
    }

    #[test]
    fn helpers() {
        assert_eq!(duration(5), "0:05");
        assert_eq!(duration(125), "2:05");
        assert_eq!(duration(3725), "1:02:05");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hi", 6), "hi");
        assert_eq!(one_line("a\n b\tc"), "a b c");
        assert_eq!(separator(" x ", 9), "─── x ───");
        assert_eq!(utf16_slice("😀ab", 2, 1), "a");
    }

    const KITTY_WIDTHS: [(&str, usize); 16] = [
        ("❤\u{FE0F}", 2),
        ("👍🏽", 2),
        ("👨\u{200D}👩\u{200D}👧", 2),
        ("1\u{FE0F}\u{20E3}", 2),
        ("❤\u{FE0F}\u{200D}🔥", 2),
        ("🤷\u{200D}♂\u{FE0F}", 2),
        ("✍\u{FE0F}", 2),
        ("✍🏽", 2),
        ("🕊\u{FE0F}", 2),
        ("🏳\u{FE0F}\u{200D}🌈", 2),
        ("🅰\u{FE0F}", 2),
        ("🇺🇸", 2),
        ("🅰", 1),
        ("⚡", 2),
        ("▸", 1),
        ("•", 1),
    ];

    fn kitty(g: &str) -> usize {
        KITTY_WIDTHS
            .iter()
            .find(|(k, _)| *k == g)
            .map(|(_, w)| *w)
            .unwrap_or_else(|| panic!("no measured kitty width for {g:?}"))
    }

    #[test]
    fn reaction_emoji_become_two_cell_graphemes_that_kitty_agrees_with() {
        for (raw, want) in [
            ("❤", "❤\u{FE0F}"),
            ("❤\u{FE0F}", "❤\u{FE0F}"),
            ("👍🏽", "👍🏽"),
            ("👨\u{200D}👩\u{200D}👧", "👨\u{200D}👩\u{200D}👧"),
            ("1\u{FE0F}\u{20E3}", "1\u{FE0F}\u{20E3}"),
            ("1\u{20E3}", "1\u{FE0F}\u{20E3}"),
            ("❤\u{200D}🔥", "❤\u{FE0F}\u{200D}🔥"),
            ("🤷\u{200D}♂", "🤷\u{200D}♂\u{FE0F}"),
            ("✍", "✍\u{FE0F}"),
            ("✍🏽", "✍🏽"),
            ("🕊", "🕊\u{FE0F}"),
            ("🏳\u{200D}🌈", "🏳\u{FE0F}\u{200D}🌈"),
            ("🇺🇸", "🇺🇸"),
        ] {
            let got = emoji_cell(raw).unwrap_or_else(|| panic!("{raw:?} rejected"));
            assert_eq!(got, want, "{raw:?}");
            assert_eq!(got.graphemes(true).count(), 1, "{raw:?}");
            assert_eq!(cells(&got), 2, "{raw:?}");
            assert_eq!(cells(&got), kitty(&got), "{raw:?}");
        }
        for bad in ["", "\u{E000}", "\u{1FAEB}", "\u{1F1FA}", "a", "◆"] {
            assert_eq!(emoji_cell(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn truncate_counts_graphemes_and_keeps_terminal_widths() {
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
        assert_eq!(
            truncate("🅰\u{FE0F}🅰\u{FE0F}🅰\u{FE0F}", 5),
            "🅰\u{FE0F}🅰\u{FE0F}…"
        );
        assert_eq!(cells(&truncate("❤\u{FE0F}\u{200D}🔥 hot ▸ ⚡ news", 6)), 6);
        for title in ["🅰⚡▸ Канал", "✍ пишем", "ㅤ", "🇺 x", "👍🏽👍🏽👍🏽"]
        {
            for w in 0..12 {
                let t = truncate(title, w);
                assert!(cells(&t) <= w, "{title:?} {w} -> {t:?}");
                for g in t.graphemes(true) {
                    if KITTY_WIDTHS.iter().any(|(k, _)| *k == g) {
                        assert_eq!(grapheme_cells(g), kitty(g), "{g:?}");
                    }
                }
            }
        }
        assert_eq!(truncate("✍", 4), "✍\u{FE0F}");
        assert_eq!(truncate("ㅤ", 4), " ");
        assert_eq!(truncate("🇺 x", 4), "\u{FFFD} x");
    }
}
