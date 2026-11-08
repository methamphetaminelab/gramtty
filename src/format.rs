use chrono::{DateTime, Local, TimeZone};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
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
        Block::Quote => Some(Span::styled("▎ ", theme::mention())),
        Block::Pre => Some(Span::styled("│ ", theme::muted())),
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

pub fn urls(rt: &RichText) -> Vec<String> {
    rt.entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Link(Some(url)) => Some(url.clone()),
            EntityKind::Link(None) => Some(utf16_slice(&rt.text, e.offset, e.length)),
            _ => None,
        })
        .filter(|u| !u.is_empty())
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
    local(ts).map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

pub fn date_label(ts: i64) -> String {
    local(ts).map_or_else(String::new, |t| t.format("%Y-%m-%d").to_string())
}

pub fn same_day(a: i64, b: i64) -> bool {
    match (local(a), local(b)) {
        (Some(a), Some(b)) => a.date_naive() == b.date_naive(),
        _ => false,
    }
}

pub fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
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
    format!("{}{}{}", "─".repeat(left), label, "─".repeat(dashes - left))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Entity;

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
        assert_eq!(urls(&rt), vec!["example.com", "https://docs.rs"]);
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
}
