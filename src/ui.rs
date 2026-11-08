use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Auth, Pane, Popup};
use crate::config::APP_NAME;

const QR_CAPTION: &str = "Scan in Telegram → Settings → Devices → Link Desktop Device";
use std::time::Duration;

use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};

use crate::format::{self, render_text};
use crate::media::{self, Input, Key, Look, Media, Src, album_layout, fit_cells, layout_height};
use crate::model::{self, Attachment, ChatKind, Decode, MediaKind, Msg, SendState, Sender, Visual};
use crate::qr;
use crate::theme;

const AUTHOR_COLORS: [Color; 7] = [
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    theme::ORANGE,
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.animating = false;
    frame.render_widget(Block::default().style(theme::normal()), area);
    if app.auth != Auth::Ready {
        draw_login(frame, area, app);
        return;
    }
    app.refresh_lists();
    let [header, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    draw_header(frame, header, app);
    let width = |visible: bool, w: u16| Constraint::Length(if visible { w } else { 0 });
    let [folders, chats, messages, info] = Layout::horizontal([
        width(app.show_folders, 18),
        width(app.show_chats, 30),
        Constraint::Min(30),
        width(app.show_info, 30),
    ])
    .areas(main);
    if app.show_folders {
        draw_folders(frame, folders, app);
    }
    if app.show_chats {
        draw_chats(frame, chats, app);
    }
    draw_messages(frame, messages, app);
    if app.show_info {
        draw_info(frame, info, app);
    }
    if matches!(app.popup, Some(Popup::Viewer { .. })) {
        draw_viewer(frame, area, app);
    } else {
        draw_popup(frame, area, app);
    }
    draw_key_hint(frame, main, app);
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let (style, title_style) = if focused {
        (theme::focus_border(), theme::strong())
    } else {
        (theme::border(), theme::strong())
    };
    Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(style)
        .title(Span::styled(title, title_style))
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        format!(" {APP_NAME} - v{} ", env!("CARGO_PKG_VERSION")),
        theme::header_title(),
    )];
    if let Some(state) = app.connection {
        spans.push(Span::styled(format!(" {state} "), theme::warning()));
    } else if let Some(name) = app.me_name() {
        spans.push(Span::styled(" Connected as ", theme::muted()));
        spans.push(Span::styled(format!("{name} "), theme::strong()));
    }
    if let Some(p) = &app.playing {
        spans.push(Span::styled(format!(" ▶ {} ", p.label), theme::media()));
    }
    if let Some((text, error)) = &app.status {
        let style = if *error {
            theme::error()
        } else {
            theme::warning()
        };
        spans.push(Span::styled(format!(" {} ", format::one_line(text)), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn split_filter(area: Rect, active: bool) -> (Rect, Option<Rect>) {
    if active && area.height >= 2 {
        let list = Rect {
            height: area.height - 1,
            ..area
        };
        let bar = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        (list, Some(bar))
    } else {
        (area, None)
    }
}

fn draw_filter_bar(frame: &mut Frame, area: Option<Rect>, app: &App, pane: Pane) {
    let (Some(area), Some(f)) = (area, app.filter.as_ref().filter(|f| f.pane == pane)) else {
        return;
    };
    let style = if f.active {
        theme::active_field()
    } else {
        theme::muted()
    };
    let shown = if f.input.is_empty() {
        Span::styled("search...", theme::muted())
    } else {
        Span::styled(f.input.text().to_owned(), style)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled("/ ", style), shown])),
        area,
    );
    if f.active {
        let (_, col) = f.input.cursor_position(0);
        frame.set_cursor_position(Position {
            x: (area.x + 2 + col as u16).min(area.right().saturating_sub(1)),
            y: area.y,
        });
    }
}

fn list_window(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        0
    } else {
        selected.saturating_sub(height / 2).min(len - height)
    }
}

fn row_line(
    label: Vec<Span<'static>>,
    selected: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected && focused {
        Span::styled(theme::SELECTION_MARKER, theme::selection())
    } else {
        Span::raw(" ".repeat(theme::SELECTION_MARKER.width()))
    };
    let mut spans = vec![marker];
    let mut used = theme::SELECTION_MARKER.width();
    for mut span in label {
        let avail = width.saturating_sub(used);
        if avail == 0 {
            break;
        }
        if span.content.width() > avail {
            span = Span::styled(format::truncate(&span.content, avail), span.style);
        }
        used += span.content.width();
        if selected {
            span.style = span.style.patch(theme::selection());
        }
        spans.push(span);
    }
    Line::from(spans)
}

fn draw_folders(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Pane::Folders;
    let block = pane_block(" Folders ".to_owned(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let filter_on = app.filter.as_ref().is_some_and(|f| f.pane == Pane::Folders);
    let (list, bar) = split_filter(inner, filter_on);
    let entries = app.folder_entries();
    let h = list.height as usize;
    let start = list_window(app.folder_sel, entries.len(), h);
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(h)
        .map(|(i, (_, name))| {
            row_line(
                vec![Span::raw(name.clone())],
                i == app.folder_sel,
                focused,
                list.width as usize,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    draw_filter_bar(frame, bar, app, Pane::Folders);
}

fn draw_chats(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Pane::Chats;
    let title = app
        .folder_entries()
        .get(app.folder_sel)
        .map_or_else(|| "Chats".to_owned(), |(_, n)| n.clone());
    let block = pane_block(format!(" {title} "), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let filter_on = app.filter.as_ref().is_some_and(|f| f.pane == Pane::Chats);
    let (list, bar) = split_filter(inner, filter_on);
    let ids = app.visible_chats();
    if ids.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(" Loading chats...", theme::muted())),
            list,
        );
    }
    let h = list.height as usize;
    let w = list.width as usize;
    let sel = app.chat_sel.min(ids.len().saturating_sub(1));
    let start = list_window(sel, ids.len(), h);
    let lines: Vec<Line> = ids
        .iter()
        .enumerate()
        .skip(start)
        .take(h)
        .filter_map(|(i, id)| {
            let c = app.chats.get(id)?;
            let mut badge = String::new();
            let mut badge_style = theme::muted();
            if c.mentions > 0 {
                badge = " @".to_owned();
                badge_style = theme::mention_badge();
            }
            if c.unread > 0 {
                badge.push_str(&format!(" {}", c.unread));
                if c.mentions == 0 && !c.muted {
                    badge_style = theme::strong();
                }
            }
            let mut title_style = if c.unread > 0 && !c.muted {
                theme::unread()
            } else if c.muted {
                theme::muted()
            } else {
                theme::normal()
            };
            if app.open_chat == Some(c.id) {
                title_style = title_style.patch(Style::default().fg(Color::Green));
            }
            let prefix = match c.kind {
                ChatKind::Secret(_) => "🔒 ",
                _ => "",
            };
            let pinned = c.position_in(app.current_list()).is_some_and(|p| p.pinned);
            let avail = w
                .saturating_sub(theme::SELECTION_MARKER.width())
                .saturating_sub(badge.width())
                .saturating_sub(usize::from(pinned) * 2);
            let title = format::truncate(&format!("{prefix}{}", c.title), avail);
            let pad = avail.saturating_sub(title.width());
            let mut spans = vec![Span::styled(title, title_style), Span::raw(" ".repeat(pad))];
            if pinned {
                spans.push(Span::styled(" ▴", theme::muted()));
            }
            spans.push(Span::styled(badge, badge_style));
            Some(row_line(spans, i == sel, focused, w))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    draw_filter_bar(frame, bar, app, Pane::Chats);
}

fn author_style(sender: Sender) -> Style {
    let id = match sender {
        Sender::User(id) | Sender::Chat(id) => id,
    };
    theme::strong().fg(AUTHOR_COLORS[id.unsigned_abs() as usize % AUTHOR_COLORS.len()])
}

#[derive(Clone, Copy, Debug)]
struct Place {
    line: usize,
    col: u16,
    cols: u16,
    rows: u16,
    msg: usize,
    att: usize,
}

struct Rendered {
    lines: Vec<Line<'static>>,
    images: Vec<Place>,
}

const PHOTO_COLS: u16 = 60;
const PHOTO_ROWS: u16 = 15;
const STICKER_COLS: u16 = 32;
const STICKER_ROWS: u16 = 8;
const NOTE_ROWS: u16 = 8;
const ALBUM_COLS: u16 = 80;
const ALBUM_ROWS: u16 = 7;
const DOC_THUMB_COLS: u16 = 10;
const DOC_THUMB_ROWS: u16 = 4;

fn visual_dims(att: &Attachment) -> (i32, i32) {
    if att.width > 0 && att.height > 0 {
        return (att.width, att.height);
    }
    att.preview.as_ref().map_or((0, 0), |v| (v.width, v.height))
}

fn media_box(att: &Attachment, font: (u16, u16), max_w: u16) -> (u16, u16) {
    let (w, h) = visual_dims(att);
    match att.kind {
        MediaKind::Sticker => fit_cells(w, h, font, max_w.min(STICKER_COLS), STICKER_ROWS),
        MediaKind::VideoNote => fit_cells(w, h, font, max_w, NOTE_ROWS),
        MediaKind::Document => fit_cells(w, h, font, max_w.min(DOC_THUMB_COLS), DOC_THUMB_ROWS),
        _ => fit_cells(w, h, font, max_w.min(PHOTO_COLS), PHOTO_ROWS),
    }
}

fn kind_label(kind: MediaKind) -> &'static str {
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

fn meta_text(app: &App, m: &Msg, idx: usize, att: &Attachment, width: usize) -> Option<String> {
    let f = app.media.file(&att.file);
    let size = (f.size > 0).then(|| format::file_size(f.size));
    let dur = format::duration(att.duration);
    let join = |parts: Vec<String>| parts.join(" · ");
    let mut text = match att.kind {
        MediaKind::Photo | MediaKind::Sticker => String::new(),
        MediaKind::Video => join(
            [Some(format!("▶ video {dur}")), size]
                .into_iter()
                .flatten()
                .collect(),
        ),
        MediaKind::Animation => join(
            [Some("GIF".to_owned()), size]
                .into_iter()
                .flatten()
                .collect(),
        ),
        MediaKind::VideoNote => format!("◉ video message {dur}"),
        MediaKind::Voice => {
            let bars_w = width.saturating_sub(24).clamp(0, 32);
            let bars = model::waveform_bars(&att.waveform, bars_w);
            join(
                [Some(format!("🎤 {dur} {bars}").trim_end().to_owned()), size]
                    .into_iter()
                    .flatten()
                    .collect(),
            )
        }
        MediaKind::Audio => {
            let title = m
                .media
                .as_deref()
                .map_or(att.name.as_str(), |l| l.trim_start_matches("♪ "));
            join(
                [Some(format!("♪ {title}")), Some(dur), size]
                    .into_iter()
                    .flatten()
                    .collect(),
            )
        }
        MediaKind::Document => join(
            [Some(format!("📄 {}", att.name)), size]
                .into_iter()
                .flatten()
                .collect(),
        ),
    };
    if let Some(p) = f.percent() {
        text.push_str(&format!(" ⇣ {p}%"));
    } else if f.active {
        text.push_str(" ⇣");
    }
    if let Some(e) = app.media.file_error(f.id) {
        text.push_str(&format!(" ⚠ {e}"));
    }
    if app
        .playing
        .as_ref()
        .is_some_and(|p| p.target == (m.id, idx))
    {
        text.push_str(" ▶ playing");
    }
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn group_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    width: usize,
    selected: bool,
) -> Rendered {
    let m = &msgs[range.start];
    let group = &msgs[range.clone()];
    let prev = range.start.checked_sub(1).map(|j| &msgs[j]);
    let new_day = prev.is_none_or(|p| !format::same_day(p.date, m.date));
    let show_header = new_day || prev.is_none_or(|p| p.sender != m.sender || m.date - p.date > 300);
    let edited = group.iter().any(|g| g.edited);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if new_day {
        lines.push(Line::from(Span::styled(
            format::separator(&format!(" {} ", format::date_label(m.date)), width),
            theme::muted(),
        )));
    } else if show_header {
        lines.push(Line::from(""));
    }
    let inner = if selected {
        width.saturating_sub(4).max(4)
    } else {
        width.max(1)
    };
    let mut body: Vec<Line<'static>> = Vec::new();
    let time = format::time_label(m.date);
    if show_header {
        let name = format::truncate(
            &app.sender_name(m.sender),
            inner.saturating_sub(time.width() + 1).max(1),
        );
        let mut header = vec![
            Span::styled(name, author_style(m.sender)),
            Span::raw(" "),
            Span::styled(time.clone(), theme::muted()),
        ];
        if edited {
            header.push(Span::styled(
                " (edited)",
                theme::muted().add_modifier(ratatui::style::Modifier::ITALIC),
            ));
        }
        body.push(Line::from(header));
    }
    if let Some(reply_id) = m.reply_to {
        let snippet = match app.histories.get(&m.chat_id).and_then(|h| h.get(reply_id)) {
            Some(r) => {
                let text = if r.body.text.is_empty() {
                    r.media.clone().unwrap_or_default()
                } else {
                    format::one_line(&r.body.text)
                };
                format!("↳ {}: {}", app.sender_name(r.sender), text)
            }
            None => "↳ reply".to_owned(),
        };
        body.push(Line::from(Span::styled(
            format::truncate(&snippet, inner),
            theme::muted(),
        )));
    }
    let media_w = width.saturating_sub(4).max(1) as u16;
    let font = app.media.font();
    let visuals: Vec<(usize, usize)> = range
        .clone()
        .flat_map(|mi| {
            msgs[mi]
                .attachments
                .iter()
                .enumerate()
                .filter(|(_, a)| a.is_visual())
                .map(move |(ai, _)| (mi, ai))
        })
        .collect();
    let mut images = Vec::new();
    if let [(mi, ai)] = visuals.as_slice() {
        let (cols, rows) = media_box(&msgs[*mi].attachments[*ai], font, media_w);
        images.push(Place {
            line: body.len(),
            col: 0,
            cols,
            rows,
            msg: *mi,
            att: *ai,
        });
        body.extend(std::iter::repeat_n(Line::default(), rows as usize));
    } else if visuals.len() > 1 {
        let dims: Vec<(i32, i32)> = visuals
            .iter()
            .map(|(mi, ai)| visual_dims(&msgs[*mi].attachments[*ai]))
            .collect();
        let cells = album_layout(&dims, font, media_w.min(ALBUM_COLS), ALBUM_ROWS);
        let base = body.len();
        for (c, (mi, ai)) in cells.iter().zip(&visuals) {
            images.push(Place {
                line: base + c.y as usize,
                col: c.x,
                cols: c.w,
                rows: c.h,
                msg: *mi,
                att: *ai,
            });
        }
        body.extend(std::iter::repeat_n(
            Line::default(),
            layout_height(&cells) as usize,
        ));
    }
    for g in group {
        for (ai, att) in g.attachments.iter().enumerate() {
            if let Some(text) = meta_text(app, g, ai, att, inner) {
                body.push(Line::from(Span::styled(
                    format::truncate(&text, inner),
                    theme::media(),
                )));
            }
        }
        if g.attachments.is_empty()
            && let Some(media) = &g.media
        {
            body.extend(render_text(
                &crate::model::RichText::plain(media.clone()),
                theme::media(),
                inner,
            ));
        }
    }
    for g in group {
        if !g.body.text.is_empty() {
            body.extend(render_text(&g.body, theme::normal(), inner));
        }
    }
    for g in group {
        match &g.state {
            SendState::Sent => {}
            SendState::Pending => body.push(Line::from(Span::styled("sending...", theme::muted()))),
            SendState::Failed(e) => body.push(Line::from(Span::styled(
                format::truncate(&format!("failed: {e}"), inner),
                theme::error(),
            ))),
        }
    }
    if !show_header
        && edited
        && let Some(last) = body.last_mut()
    {
        last.spans.push(Span::styled(" (edited)", theme::muted()));
    }
    let offset = lines.len() + usize::from(selected);
    for p in &mut images {
        p.line += offset;
        if selected {
            p.col += 2;
        }
    }
    if selected {
        lines.extend(card(body, width, (!show_header).then_some(time.as_str())));
    } else {
        lines.extend(body);
    }
    Rendered { lines, images }
}

fn card(body: Vec<Line<'static>>, width: usize, bottom_time: Option<&str>) -> Vec<Line<'static>> {
    let style = theme::selection_border();
    let inner = width.saturating_sub(2);
    let mut out = vec![Line::from(Span::styled(
        format!("╭{}╮", "─".repeat(inner)),
        style,
    ))];
    for line in body {
        let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
        let pad = width.saturating_sub(4).saturating_sub(used);
        let mut spans = vec![Span::styled("│ ", style)];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(" │", style));
        out.push(Line::from(spans));
    }
    match bottom_time.filter(|t| inner > t.width() + 3) {
        Some(t) => out.push(Line::from(vec![
            Span::styled(format!("╰{}", "─".repeat(inner - t.width() - 3)), style),
            Span::styled(format!(" {t} "), theme::muted()),
            Span::styled("─╯", style),
        ])),
        None => out.push(Line::from(Span::styled(
            format!("╰{}╯", "─".repeat(inner)),
            style,
        ))),
    }
    out
}

fn composer_height(app: &App, width: usize) -> u16 {
    if app.open_chat.is_none() {
        return 0;
    }
    let lines = app
        .composer
        .input
        .visual_lines(width.saturating_sub(2))
        .len()
        .max(1);
    (lines.min(8) + 2) as u16
}

fn draw_messages(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Messages;
    let title = app
        .open_chat
        .and_then(|id| app.chats.get(&id))
        .map_or_else(|| "Messages".to_owned(), |c| c.title.clone());
    let block = pane_block(
        format!(
            " {} ",
            format::truncate(&title, area.width.saturating_sub(4) as usize)
        ),
        focused,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let ch = composer_height(app, inner.width as usize);
    let [list, composer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(ch)]).areas(inner);
    app.msg_view_height = list.height as usize;
    draw_message_list(frame, list, app);
    if ch > 0 {
        draw_composer(frame, composer, app);
    }
}

fn draw_message_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(chat_id) = app.open_chat else {
        let hint = Line::from(vec![
            Span::styled(" Select a chat and press ", theme::muted()),
            Span::styled("Enter", theme::strong()),
            Span::styled(", or ", theme::muted()),
            Span::styled(
                app.keymap
                    .label_for(crate::keymap::Action::ChatSwitcher)
                    .unwrap_or_default(),
                theme::strong(),
            ),
            Span::styled(" to find one", theme::muted()),
        ]);
        frame.render_widget(Paragraph::new(hint), area);
        return;
    };
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }
    let Some(h) = app.histories.get(&chat_id) else {
        return;
    };
    let msgs = h.msgs();
    if h.is_empty() {
        let text = if h.loading_older {
            " Loading..."
        } else {
            " No messages yet"
        };
        frame.render_widget(Paragraph::new(Span::styled(text, theme::muted())), area);
        return;
    }
    let reached_start = h.reached_start;
    let loading = h.loading_older;
    let groups = model::groups(msgs);
    let block = |app: &App, g: usize, sel: Option<usize>| {
        group_block(app, msgs, groups[g].clone(), width, sel == Some(g))
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut placed: Vec<(usize, Place)> = Vec::new();
    let mut first_line = 0usize;
    let mut want_older = false;
    match app.msg_sel {
        None => {
            let mut blocks: Vec<Rendered> = Vec::new();
            let mut total = 0;
            let mut g = groups.len();
            while g > 0 && total < height + app.bottom_offset {
                g -= 1;
                let b = block(app, g, None);
                total += b.lines.len();
                blocks.push(b);
            }
            if total < height + app.bottom_offset && !reached_start {
                want_older = true;
            }
            let mut all: Vec<Line<'static>> = Vec::with_capacity(total);
            for b in blocks.into_iter().rev() {
                let base = all.len();
                placed.extend(b.images.into_iter().map(|p| (base + p.line, p)));
                all.extend(b.lines);
            }
            let max_offset = all.len().saturating_sub(height);
            app.bottom_offset = app.bottom_offset.min(max_offset);
            let end = all.len() - app.bottom_offset;
            let start = end.saturating_sub(height);
            first_line = start;
            all.truncate(end);
            lines.extend(all.drain(start..));
        }
        Some(sel) => {
            let gsel = model::group_of(&groups, sel.min(msgs.len() - 1));
            let heights = |from: usize, to: usize, app: &App| -> usize {
                (from..=to)
                    .map(|g| block(app, g, Some(gsel)).lines.len())
                    .sum()
            };
            let mut top = model::group_of(&groups, app.msg_top).min(gsel);
            while top < gsel && heights(top, gsel, app) > height {
                top += 1;
            }
            let mut below = 0;
            let mut j = top;
            while j < groups.len() && below <= height {
                below += block(app, j, Some(gsel)).lines.len();
                j += 1;
            }
            while top > 0 && below < height {
                let add = block(app, top - 1, Some(gsel)).lines.len();
                if below + add > height {
                    break;
                }
                below += add;
                top -= 1;
            }
            if top == 0 && !reached_start {
                want_older = true;
            }
            let mut g = top;
            while g < groups.len() && lines.len() < height {
                let b = block(app, g, Some(gsel));
                let base = lines.len();
                placed.extend(b.images.into_iter().map(|p| (base + p.line, p)));
                lines.extend(b.lines);
                g += 1;
            }
            lines.truncate(height);
            app.msg_top = groups[top].start;
        }
    }
    if loading && !reached_start && lines.len() >= height {
        lines[0] = Line::from(Span::styled(
            format::separator(" loading older ", width),
            theme::muted(),
        ));
    }
    let pad = height.saturating_sub(lines.len());
    let mut shown: Vec<Line<'static>> = vec![Line::from(""); pad];
    shown.extend(lines);
    frame.render_widget(Paragraph::new(shown), area);
    let overlay = app.popup.is_some() || !app.pending.is_empty();
    for (abs, p) in placed {
        let y = pad as isize + abs as isize - first_line as isize;
        if y + p.rows as isize <= 0 || y >= height as isize {
            continue;
        }
        let m = &msgs[p.msg];
        let Some(att) = m.attachments.get(p.att) else {
            continue;
        };
        let target = ImageTarget {
            area,
            x: p.col,
            y,
            cols: p.cols,
            rows: p.rows,
        };
        draw_attachment(
            frame,
            target,
            &mut app.media,
            att,
            (chat_id, m.id, p.att),
            overlay,
            false,
            Duration::ZERO,
        );
    }
    if want_older {
        app.want_older = true;
    }
}

#[derive(Clone, Copy)]
struct ImageTarget {
    area: Rect,
    x: u16,
    y: isize,
    cols: u16,
    rows: u16,
}

fn render_proto(frame: &mut Frame, t: ImageTarget, proto: &SlicedProtocol) {
    let (Ok(x), Ok(y)) = (i16::try_from(t.x), i16::try_from(t.y)) else {
        return;
    };
    frame.render_widget(SlicedImage::new(proto, SignedPosition { x, y }), t.area);
}

fn box_note(frame: &mut Frame, t: ImageTarget, text: &str) {
    if t.y < 0 || t.y >= t.area.height as isize || t.x >= t.area.width {
        return;
    }
    let w = t.cols.min(t.area.width - t.x).max(1);
    let rect = Rect {
        x: t.area.x + t.x,
        y: t.area.y + t.y as u16,
        width: w,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format::truncate(text, w as usize),
            theme::muted(),
        )),
        rect,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_attachment(
    frame: &mut Frame,
    t: ImageTarget,
    media: &mut Media,
    att: &Attachment,
    (chat_id, msg_id, idx): (i64, i64, usize),
    overlay: bool,
    full: bool,
    elapsed: Duration,
) -> bool {
    let round = att.kind == MediaKind::VideoNote;
    let mut note: Option<String> = None;
    let mut animated = false;
    let sources: Vec<&Visual> = if full {
        att.full.iter().chain(att.preview.iter()).collect()
    } else {
        att.preview.iter().collect()
    };
    let priority = if full { 32 } else { 1 };
    for (n, v) in sources.iter().enumerate() {
        let f = media.file(&v.file);
        let Some(input) = media::input_for(&f, v.decode) else {
            if n == 0 {
                match media.file_error(v.file.id) {
                    Some(e) => note = Some(format!("⚠ {e}")),
                    None => {
                        media.want_download(v.file.id, priority);
                        note = Some(match f.percent() {
                            Some(p) => format!("⇣ {} {p}%", kind_label(att.kind)),
                            None => format!("⇣ {}", kind_label(att.kind)),
                        });
                    }
                }
            }
            continue;
        };
        let key = Key {
            src: Src::File(v.file.id),
            cols: t.cols,
            rows: t.rows,
            round,
            animate: full && v.decode == Decode::Tgs,
        };
        match media.look(key, input, overlay) {
            Look::Ready(frames) => {
                if let Some(proto) = frames.at(elapsed) {
                    render_proto(frame, t, proto);
                    animated = frames.animated();
                }
                return animated;
            }
            Look::Failed(e) => {
                if n == 0 {
                    note = Some(format!("⚠ {e}"));
                }
            }
            Look::Hidden => return false,
            Look::Pending => {
                if n == 0 && note.is_none() {
                    note = Some(format!("⧗ {}", kind_label(att.kind)));
                }
            }
        }
    }
    if let Some(bytes) = &att.mini {
        let key = Key {
            src: Src::Mini {
                chat: chat_id,
                msg: msg_id,
                idx: idx.min(255) as u8,
            },
            cols: t.cols,
            rows: t.rows,
            round,
            animate: false,
        };
        match media.look(key, Input::Bytes(bytes.clone()), overlay) {
            Look::Ready(frames) => {
                if let Some(proto) = frames.at(Duration::ZERO) {
                    render_proto(frame, t, proto);
                    return false;
                }
            }
            Look::Hidden => return false,
            Look::Pending | Look::Failed(_) => {}
        }
    }
    let text = note.unwrap_or_else(|| format!("[{}]", kind_label(att.kind)));
    box_note(frame, t, &text);
    animated
}

fn draw_viewer(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(Popup::Viewer {
        chat_id,
        items,
        pos,
    }) = &app.popup
    else {
        return;
    };
    let (chat_id, len, pos) = (*chat_id, items.len(), *pos);
    let Some(&(msg_id, idx)) = items.get(pos) else {
        return;
    };
    frame.render_widget(Clear, area);
    let att = app.attachment(chat_id, msg_id, idx).cloned();
    let title = match &att {
        Some(a) => format!(" {} {}/{} ", kind_label(a.kind), pos + 1, len),
        None => format!(" attachment {}/{} ", pos + 1, len),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(theme::focus_border())
        .title(Span::styled(title, theme::strong()))
        .title_bottom(Span::styled(
            " j/k prev/next · x play/open · s save · o xdg-open · Esc close ",
            theme::muted(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(att) = att else {
        frame.render_widget(
            Paragraph::new(Span::styled(" message is gone", theme::muted())),
            inner,
        );
        return;
    };
    if inner.height < 2 || inner.width < 2 {
        return;
    }
    let image_h = inner.height - 1;
    let dims = att
        .full
        .as_ref()
        .filter(|v| v.width > 0 && v.height > 0)
        .map_or_else(|| visual_dims(&att), |v| (v.width, v.height));
    let (cols, rows) = fit_cells(dims.0, dims.1, app.media.font(), inner.width, image_h);
    let image_area = Rect {
        height: image_h,
        ..inner
    };
    let t = ImageTarget {
        area: image_area,
        x: (inner.width - cols) / 2,
        y: ((image_h - rows) / 2) as isize,
        cols,
        rows,
    };
    let elapsed = app.anim_start.elapsed();
    let animated = if att.viewable() {
        draw_attachment(
            frame,
            t,
            &mut app.media,
            &att,
            (chat_id, msg_id, idx),
            false,
            true,
            elapsed,
        )
    } else {
        box_note(frame, t, "nothing to preview; x to open");
        false
    };
    app.animating |= animated;
    let f = app.media.file(&att.file);
    let mut meta = vec![if att.name.is_empty() || att.kind == MediaKind::Photo {
        kind_label(att.kind).to_owned()
    } else {
        att.name.clone()
    }];
    if dims.0 > 0 {
        meta.push(format!("{}×{}", dims.0, dims.1));
    }
    if att.duration > 0 {
        meta.push(format::duration(att.duration));
    }
    if f.size > 0 {
        meta.push(format::file_size(f.size));
    }
    if let Some(p) = att
        .full
        .as_ref()
        .and_then(|v| app.media.file(&v.file).percent())
    {
        meta.push(format!("⇣ {p}%"));
    }
    let line = Rect {
        y: inner.y + image_h,
        height: 1,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format::truncate(&format!(" {}", meta.join(" · ")), inner.width as usize),
            theme::muted(),
        )),
        line,
    );
}

fn draw_composer(frame: &mut Frame, area: Rect, app: &App) {
    let c = &app.composer;
    let title = if c.edit.is_some() {
        Span::styled(" Edit message ", theme::editing())
    } else if let Some(id) = c.reply_to {
        let name = app
            .selected_message()
            .filter(|m| m.id == id)
            .or_else(|| {
                app.histories
                    .get(&app.open_chat.unwrap_or(0))
                    .and_then(|h| h.get(id))
            })
            .map_or_else(|| "message".to_owned(), |m| app.sender_name(m.sender));
        Span::styled(format!(" Reply to {name} "), theme::strong())
    } else {
        Span::styled(" Message ", theme::strong())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(if c.active {
            theme::focus_border()
        } else {
            theme::border()
        })
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    if c.input.is_empty() && !c.active {
        let label = app
            .keymap
            .label_for(crate::keymap::Action::StartComposer)
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("press {label} to write"),
                theme::muted(),
            )),
            inner,
        );
        return;
    }
    let lines = c.input.visual_lines(width);
    let (row, col) = c.input.cursor_position(width);
    let h = inner.height as usize;
    let skip = (row + 1).saturating_sub(h);
    let shown: Vec<Line> = lines
        .into_iter()
        .skip(skip)
        .take(h)
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(shown), inner);
    if c.active {
        frame.set_cursor_position(Position {
            x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
            y: inner.y + (row - skip) as u16,
        });
    }
}

fn draw_info(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block(" Chat info ".to_owned(), app.focus == Pane::Info);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(chat) = app.open_chat.and_then(|id| app.chats.get(&id)) else {
        frame.render_widget(
            Paragraph::new(Span::styled(" No chat open", theme::muted())),
            inner,
        );
        return;
    };
    let kind = match chat.kind {
        ChatKind::Private(_) => "private chat",
        ChatKind::Group => "group",
        ChatKind::Supergroup => "supergroup",
        ChatKind::Channel => "channel",
        ChatKind::Secret(_) => "secret chat",
    };
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!(" {label}: "), theme::muted()),
            Span::raw(value),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled(format!(" {}", chat.title), theme::strong())),
        Line::from(""),
        field("Type", kind.to_owned()),
        field("Unread", chat.unread.to_string()),
        field("Muted", if chat.muted { "yes" } else { "no" }.to_owned()),
        field("ID", chat.id.to_string()),
    ];
    if let Some(h) = app.histories.get(&chat.id) {
        lines.push(field("Loaded", format!("{} messages", h.len())));
    }
    let (images, bytes) = app.media.stats();
    lines.push(field(
        "Images",
        format!(
            "{} · {images} cached · {}",
            app.media.protocol_name(),
            format::file_size(bytes as i64)
        ),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn modal(frame: &mut Frame, rect: Rect, title: &str, lines: Vec<Line<'static>>) -> Rect {
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(theme::focus_border())
        .title(Span::styled(format!(" {title} "), theme::strong()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
    inner
}

fn draw_popup(frame: &mut Frame, area: Rect, app: &App) {
    let Some(popup) = &app.popup else {
        return;
    };
    match popup {
        Popup::Menu { title, items, sel } => {
            let lines: Vec<Line> = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let selected = i == *sel;
                    row_line(
                        vec![
                            Span::styled(format!("{} ", item.key), theme::active_field()),
                            Span::raw(item.label),
                        ],
                        selected,
                        true,
                        32,
                    )
                })
                .collect();
            let rect = centered(area, 34, lines.len() as u16 + 2);
            modal(frame, rect, title, lines);
        }
        Popup::Viewer { .. } => {}
        Popup::ConfirmDelete { message_ids, .. } => {
            let question = if message_ids.len() > 1 {
                format!(" Delete these {} messages?", message_ids.len())
            } else {
                " Delete this message?".to_owned()
            };
            let lines = vec![
                Line::from(question),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" y ", theme::active_field()),
                    Span::raw("for everyone   "),
                    Span::styled("m ", theme::active_field()),
                    Span::raw("only for me"),
                ]),
                Line::from(vec![
                    Span::styled(" n ", theme::active_field()),
                    Span::raw("cancel"),
                ]),
            ];
            let rect = centered(area, 40, 6);
            modal(frame, rect, "Delete", lines);
        }
        Popup::ConfirmLogout => {
            let lines = vec![
                Line::from(" Log out of Telegram? y/N"),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" y ", theme::active_field()),
                    Span::raw("log out   "),
                    Span::styled("n ", theme::active_field()),
                    Span::raw("cancel"),
                ]),
            ];
            let rect = centered(area, 40, 5);
            modal(frame, rect, "Log out", lines);
        }
        Popup::Switcher { input, sel } => {
            let results = app.switcher_results(input.text());
            let rect = centered(area, 60.min(area.width), 20.min(area.height));
            let inner_w = rect.width.saturating_sub(2) as usize;
            let rows = rect.height.saturating_sub(4) as usize;
            let sel = (*sel).min(results.len().saturating_sub(1));
            let start = list_window(sel, results.len(), rows);
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("> ", theme::active_field()),
                    if input.is_empty() {
                        Span::styled("type a chat name...", theme::muted())
                    } else {
                        Span::raw(input.text().to_owned())
                    },
                ]),
                Line::from(Span::styled("─".repeat(inner_w), theme::border())),
            ];
            lines.extend(
                results
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(rows)
                    .filter_map(|(i, id)| {
                        let c = app.chats.get(id)?;
                        let mut spans = vec![Span::raw(c.title.clone())];
                        if c.unread > 0 {
                            spans.push(Span::styled(format!(" {}", c.unread), theme::muted()));
                        }
                        Some(row_line(spans, i == sel, true, inner_w))
                    }),
            );
            let inner = modal(frame, rect, "Find chat", lines);
            let (_, col) = input.cursor_position(0);
            frame.set_cursor_position(Position {
                x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
                y: inner.y,
            });
        }
    }
}

fn draw_key_hint(frame: &mut Frame, area: Rect, app: &App) {
    if app.pending.is_empty() {
        return;
    }
    let items = app.keymap.continuations(&app.pending);
    if items.is_empty() {
        return;
    }
    let key_w = items.iter().map(|(k, _)| k.width()).max().unwrap_or(1);
    let lines: Vec<Line> = items
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {k:<key_w$} "), theme::active_field()),
                Span::raw(format!("{d} ")),
            ])
        })
        .collect();
    let content_w = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.width()).sum::<usize>())
        .max()
        .unwrap_or(10);
    let width = (content_w as u16 + 2).max(28).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height),
        width,
        height,
    };
    let title = if app.pending.first() == Some(&app.keymap.leader) && app.pending.len() == 1 {
        "Leader".to_owned()
    } else {
        app.pending.iter().map(|c| c.label()).collect::<String>()
    };
    modal(frame, rect, &title, lines);
}

fn draw_login(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_row: Option<(usize, usize)> = None;
    let hint = |text: &str| Line::from(Span::styled(text.to_owned(), theme::muted()));
    let field = |label: &str, value: String| {
        vec![
            Line::from(Span::styled(label.to_owned(), theme::strong())),
            Line::from(vec![
                Span::styled("> ", theme::active_field()),
                Span::raw(value),
            ]),
        ]
    };
    match &app.auth {
        Auth::Starting => lines.push(hint("Connecting to Telegram...")),
        Auth::Closing => lines.push(hint("Closing session...")),
        Auth::Ready => {}
        Auth::Blocked(text) => {
            lines.push(Line::from(Span::styled(text.clone(), theme::warning())));
            lines.push(Line::from(""));
            lines.push(hint("q: quit"));
        }
        Auth::Qr(None) => {
            lines.push(hint("Requesting QR code..."));
            lines.push(Line::from(""));
            lines.push(hint("p: log in with phone number · q: quit"));
        }
        Auth::Qr(Some(link)) => match qr::qr_lines(link) {
            Ok(qr) => {
                let fits = qr.len() + 6 <= area.height as usize
                    && qr.first().map_or(0, |l| l.chars().count()) + 4 <= area.width as usize;
                if fits {
                    let qr_style = Style::default().fg(Color::White).bg(Color::Black);
                    let qr_w = qr.first().map_or(0, |l| l.chars().count());
                    let pad = " ".repeat(QR_CAPTION.width().saturating_sub(qr_w) / 2);
                    lines.extend(qr.into_iter().map(|l| {
                        Line::from(vec![Span::raw(pad.clone()), Span::styled(l, qr_style)])
                    }));
                } else {
                    let w = qr.first().map_or(0, |l| l.chars().count()) + 4;
                    lines.push(Line::from(Span::styled(
                        format!(
                            "Terminal too small for the QR code (need {}x{}).",
                            w,
                            qr.len() + 6
                        ),
                        theme::warning(),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(QR_CAPTION, theme::strong())));
                lines.push(hint("p: log in with phone number · q: quit"));
            }
            Err(e) => lines.push(Line::from(Span::styled(e, theme::error()))),
        },
        Auth::Phone => {
            lines.extend(field(
                "Phone number in international format",
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
            lines.push(Line::from(""));
            lines.push(hint(
                "Enter: send code · Esc: back to QR login · Ctrl+C: quit",
            ));
        }
        Auth::Code { phone, via } => {
            lines.extend(field(
                &format!("Code sent to {phone} via {via}"),
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
            lines.push(Line::from(""));
            lines.push(hint("Enter: confirm · Esc: change number · Ctrl+C: quit"));
        }
        Auth::Password { hint: pw_hint } => {
            let masked = "•".repeat(app.login_input.text().chars().count());
            lines.extend(field("Two-step verification password", masked));
            cursor_row = Some((1, 2));
            if !pw_hint.is_empty() {
                lines.push(hint(&format!("Hint: {pw_hint}")));
            }
            lines.push(Line::from(""));
            lines.push(hint("Enter: confirm · Ctrl+C: quit"));
        }
        Auth::Email => {
            lines.extend(field(
                "Login email address",
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
        }
        Auth::EmailCode { pattern } => {
            lines.extend(field(
                &format!("Code sent to {pattern}"),
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
        }
    }
    if let Some(err) = &app.auth_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(err.clone(), theme::error())));
    }
    let content_w = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.width()).sum::<usize>())
        .max()
        .unwrap_or(20);
    let width = (content_w as u16 + 4).max(50).min(area.width.max(1));
    let height = (lines.len() as u16 + 2).min(area.height.max(1));
    let rect = centered(area, width, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(theme::focus_border())
        .title(Span::styled(
            format!(" {APP_NAME} login "),
            theme::header_title(),
        ));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let inner_padded = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    frame.render_widget(Paragraph::new(lines), inner_padded);
    if let Some((row, col)) = cursor_row {
        let (_, c) = app.login_input.cursor_position(0);
        frame.set_cursor_position(Position {
            x: (inner_padded.x + (col + c) as u16).min(inner_padded.right().saturating_sub(1)),
            y: inner_padded.y + row as u16,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::ApiCredentials;
    use crate::keymap::{Chord, Keymap};
    use crate::model::{ChatInfo, Entity, EntityKind, History, ListId, Position, RichText};
    use crate::td::Event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn msg(id: i64, sender: i64, date: i64, text: &str) -> Msg {
        Msg {
            id,
            chat_id: 10,
            sender: Sender::User(sender),
            date,
            edited: id == 3,
            outgoing: sender == 1,
            state: SendState::Sent,
            reply_to: (id == 4).then_some(2),
            album: 0,
            media: (id == 5).then(|| "[photo]".to_owned()),
            attachments: Vec::new(),
            body: RichText {
                text: text.to_owned(),
                entities: vec![Entity {
                    offset: 0,
                    length: 2,
                    kind: EntityKind::Bold,
                }],
            },
        }
    }

    fn app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let creds = ApiCredentials {
            api_id: 1,
            api_hash: "x".to_owned(),
        };
        let mut app = App::new(
            crate::td::Td::new(tx),
            creds,
            std::env::temp_dir(),
            Keymap::default(),
            ratatui_image::picker::Picker::halfblocks(),
        );
        app.auth = Auth::Ready;
        app.users.insert(1, "Me".to_owned());
        app.users.insert(2, "Alice Longname".to_owned());
        app.chats.insert(
            10,
            ChatInfo {
                id: 10,
                title: "Rust чат".to_owned(),
                kind: ChatKind::Supergroup,
                unread: 3,
                mentions: 1,
                last_read_inbox: 0,
                muted: false,
                positions: vec![Position {
                    list: ListId::Main,
                    order: 5,
                    pinned: true,
                }],
                last_message: None,
            },
        );
        let mut h = History::default();
        h.merge_older(
            (1..=40)
                .map(|i| {
                    msg(
                        i,
                        if i % 3 == 0 { 1 } else { 2 },
                        1_700_000_000 + i * 400,
                        "hello there, this is a fairly long message that should wrap around",
                    )
                })
                .collect(),
        );
        app.histories.insert(10, h);
        app.open_chat = Some(10);
        app.show_info = true;
        app
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn dashboard_renders_at_many_sizes() {
        let mut app = app();
        for (w, h) in [(160, 50), (80, 24), (40, 10), (10, 4), (1, 1)] {
            app.msg_sel = None;
            render(&mut app, w, h);
            app.msg_sel = Some(3);
            render(&mut app, w, h);
        }
        app.msg_sel = None;
        let screen = render(&mut app, 160, 50);
        assert!(screen.contains("Folders"));
        assert!(screen.contains("Rust чат"));
        assert!(screen.contains("Alice Longname"));
        assert!(screen.contains("Chat info"));
    }

    #[test]
    fn selection_card_and_popups_render() {
        let mut app = app();
        app.msg_sel = Some(4);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains('╭'));
        assert!(screen.contains("[photo]"));
        app.pending = vec![Chord::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        )];
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Leader"));
        assert!(screen.contains("find chat"));
        assert!(screen.contains("log out"));
        app.pending.clear();
        app.popup = Some(Popup::ConfirmLogout);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Log out of Telegram? y/N"));
        app.popup = Some(Popup::Switcher {
            input: crate::input::TextInput::with_text("rust"),
            sel: 0,
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Find chat"));
    }

    fn shows_image(app: &mut App, w: u16, h: u16) -> bool {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let red = Color::Rgb(200, 30, 30);
        term.backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.fg == red || c.bg == red)
    }

    fn file(id: i32, local: bool) -> crate::model::FileRef {
        crate::model::FileRef {
            id,
            size: 2048,
            path: if local {
                format!("/nonexistent/{id}")
            } else {
                String::new()
            },
            done: local,
            active: false,
            downloaded: if local { 2048 } else { 0 },
        }
    }

    fn photo(id: i32) -> Attachment {
        let mut a = Attachment::new(MediaKind::Photo, file(id, true));
        a.width = 800;
        a.height = 600;
        a.preview = Some(Visual {
            file: file(id, true),
            decode: Decode::Raster,
            width: 800,
            height: 600,
        });
        a.full = a.preview.clone();
        a
    }

    fn ready_frames(app: &App, cols: u16, rows: u16) -> media::Frames {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            u32::from(cols) * 10,
            u32::from(rows) * 20,
            image::Rgba([200, 30, 30, 255]),
        ));
        let _ = app;
        media::Frames {
            frames: vec![(
                SlicedProtocol::new_with_resize(
                    &ratatui_image::picker::Picker::halfblocks(),
                    img,
                    ratatui::layout::Size::new(cols, rows),
                    ratatui_image::Resize::Fit(None),
                )
                .unwrap(),
                Duration::ZERO,
            )],
            bytes: 1,
        }
    }

    fn media_app() -> App {
        let mut app = app();
        app.show_info = false;
        app.show_folders = false;
        app.show_chats = false;
        let h = app.histories.get_mut(&10).unwrap();
        for (i, id) in [41i64, 42, 43].iter().enumerate() {
            let mut m = msg(*id, 2, 1_700_100_000 + i as i64, "");
            m.body = RichText::default();
            m.album = 777;
            m.attachments = vec![photo(100 + i as i32)];
            h.upsert(m);
        }
        let mut voice = msg(44, 2, 1_700_100_010, "");
        voice.body = RichText::default();
        let mut va = Attachment::new(MediaKind::Voice, file(200, false));
        va.duration = 7;
        va.waveform = vec![0, 10, 31, 20, 5, 0, 31];
        voice.attachments = vec![va];
        h.upsert(voice);
        let mut doc = msg(45, 2, 1_700_100_020, "the caption");
        let mut da = Attachment::new(MediaKind::Document, file(300, false));
        da.name = "report.pdf".to_owned();
        doc.attachments = vec![da];
        h.upsert(doc);
        let mut poll = msg(46, 2, 1_700_100_030, "");
        poll.body = RichText::default();
        poll.media = Some("📊 Lunch?\n  ○ pizza — 60%\n  ○ sushi — 40%".to_owned());
        h.upsert(poll);
        app
    }

    #[tokio::test]
    async fn album_renders_as_one_block_with_images() {
        let mut app = media_app();
        let screen = render(&mut app, 100, 40);
        assert!(screen.contains("⧗ photo"));
        assert!(screen.contains(" 0:07 ▁▁▁▁▁▃▃▃▃▃"));
        assert!(screen.contains(" report.pdf · 2 KB"));
        assert!(screen.contains("the caption"));
        assert!(screen.contains("pizza — 60%"));
        let font = app.media.font();
        let cells = album_layout(&[(800, 600); 3], font, 92u16.min(ALBUM_COLS), ALBUM_ROWS);
        for (i, c) in cells.iter().enumerate() {
            let key = Key {
                src: Src::File(100 + i as i32),
                cols: c.w,
                rows: c.h,
                round: false,
                animate: false,
            };
            let frames = ready_frames(&app, c.w, c.h);
            app.media.insert_ready(key, frames, 0);
        }
        let screen = render(&mut app, 100, 40);
        assert!(!screen.contains("⧗ photo"));
        assert!(shows_image(&mut app, 100, 40));

        let len = app.histories[&10].len();
        app.msg_sel = None;
        app.focus = Pane::Messages;
        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('k')),
        )));
        assert_eq!(app.msg_sel, Some(len - 1));
        for _ in 0..3 {
            app.handle(Event::Input(crossterm::event::Event::Key(
                crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('k')),
            )));
        }
        let sel = app.msg_sel.unwrap();
        assert_eq!(app.histories[&10].msgs()[sel].id, 41);
        assert_eq!(app.selected_group().len(), 3);
        let screen = render(&mut app, 100, 40);
        assert!(screen.contains('╭'));

        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('v')),
        )));
        assert!(
            matches!(app.popup, Some(Popup::Viewer { pos: 0, ref items, .. }) if items.len() == 3)
        );
        let screen = render(&mut app, 100, 40);
        assert!(screen.contains("photo 1/3"));
        assert!(screen.contains("800×600"));
        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('j')),
        )));
        assert!(render(&mut app, 100, 40).contains("photo 2/3"));
        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Esc),
        )));
        assert!(app.popup.is_none());

        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('d')),
        )));
        assert!(
            matches!(&app.popup, Some(Popup::ConfirmDelete { message_ids, .. }) if message_ids == &vec![41, 42, 43])
        );
        assert!(render(&mut app, 100, 40).contains("Delete these 3 messages?"));
    }

    #[tokio::test]
    async fn media_menu_and_sizes_render_everywhere() {
        let mut app = media_app();
        for (w, h) in [(160, 50), (80, 24), (40, 10), (12, 5), (1, 1)] {
            app.msg_sel = None;
            render(&mut app, w, h);
            app.msg_sel = Some(app.histories[&10].len() - 2);
            render(&mut app, w, h);
        }
        app.focus = Pane::Messages;
        app.msg_sel = Some(app.histories[&10].len() - 2);
        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Enter),
        )));
        let screen = render(&mut app, 100, 40);
        assert!(screen.contains("Save to Downloads"));
        assert!(screen.contains("Play / open media"));
    }

    #[test]
    fn hidden_until_drawn_without_overlay() {
        let mut app = media_app();
        app.histories.get_mut(&10).unwrap().remove(&[44]);
        app.msg_sel = None;
        let font = app.media.font();
        let cells = album_layout(&[(800, 600); 3], font, 92u16.min(ALBUM_COLS), ALBUM_ROWS);
        for (i, c) in cells.iter().enumerate() {
            let key = Key {
                src: Src::File(100 + i as i32),
                cols: c.w,
                rows: c.h,
                round: false,
                animate: false,
            };
            let frames = ready_frames(&app, c.w, c.h);
            app.media.insert_ready(key, frames, 0);
        }
        app.popup = Some(Popup::ConfirmLogout);
        assert!(!shows_image(&mut app, 100, 40));
        app.popup = None;
        assert!(shows_image(&mut app, 100, 40));
        app.popup = Some(Popup::ConfirmLogout);
        assert!(shows_image(&mut app, 100, 40));
    }

    fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn fixture_scroll_memory() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".work/fixtures");
        let sources = [
            (
                "photo_big.jpg",
                Decode::Raster,
                MediaKind::Photo,
                2560,
                1920,
            ),
            ("photo.jpg", Decode::Raster, MediaKind::Photo, 320, 200),
            ("sticker.webp", Decode::Raster, MediaKind::Sticker, 256, 256),
            ("sticker.tgs", Decode::Tgs, MediaKind::Sticker, 512, 512),
            ("sticker.webm", Decode::Video, MediaKind::Sticker, 128, 128),
            ("video.mp4", Decode::Video, MediaKind::VideoNote, 320, 240),
        ];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut app = App::new(
            crate::td::Td::new(tx),
            ApiCredentials {
                api_id: 1,
                api_hash: "x".to_owned(),
            },
            std::env::temp_dir(),
            Keymap::default(),
            picker,
        );
        app.auth = Auth::Ready;
        app.show_folders = false;
        app.show_chats = false;
        let mut h = History::default();
        h.reached_start = true;
        for i in 0..400i64 {
            let (name, decode, kind, w, ht) = sources[i as usize % sources.len()];
            let path = dir.join(name).to_string_lossy().into_owned();
            let f = crate::model::FileRef {
                id: 1000 + i as i32,
                size: 1,
                path,
                done: true,
                active: false,
                downloaded: 1,
            };
            let mut a = Attachment::new(kind, f.clone());
            a.width = w;
            a.height = ht;
            a.preview = Some(Visual {
                file: f,
                decode,
                width: w,
                height: ht,
            });
            let mut m = msg(i + 1, 2 + i % 2, 1_700_000_000 + i * 400, "");
            m.body = RichText::default();
            m.attachments = vec![a];
            h.upsert(m);
        }
        let len = h.len();
        app.histories.insert(10, h);
        app.open_chat = Some(10);
        app.focus = Pane::Messages;
        let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
        let start_rss = rss_kb();
        let mut peak = start_rss;
        let started = std::time::Instant::now();
        for step in 0..len {
            app.msg_sel = Some(len - 1 - step);
            for _ in 0..50 {
                term.draw(|f| draw(f, &mut app)).unwrap();
                app.after_draw();
                let ev = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
                match ev {
                    Ok(Some(ev)) => app.handle(ev),
                    _ => break,
                }
                while let Ok(ev) = rx.try_recv() {
                    app.handle(ev);
                }
                if step % 2 == 1 {
                    break;
                }
            }
            peak = peak.max(rss_kb());
            assert!(app.media.stats().0 <= crate::media::MAX_ENTRIES + 4);
        }
        let (entries, bytes) = app.media.stats();
        eprintln!(
            "scrolled {len} media messages in {:?}: rss start {} MB, peak {} MB, end {} MB; cache {entries} entries, {} MB",
            started.elapsed(),
            start_rss / 1024,
            peak / 1024,
            rss_kb() / 1024,
            bytes / 1024 / 1024
        );
        assert!(bytes <= crate::media::MAX_BYTES);
    }

    #[test]
    fn login_screens_render() {
        let mut app = app();
        for auth in [
            Auth::Starting,
            Auth::Qr(None),
            Auth::Qr(Some(
                "tg://login?token=abcdefghijklmnopqrstuvwxyz0123456789".to_owned(),
            )),
            Auth::Phone,
            Auth::Code {
                phone: "+1".to_owned(),
                via: "SMS".to_owned(),
            },
            Auth::Password {
                hint: "pet".to_owned(),
            },
            Auth::Blocked("nope".to_owned()),
        ] {
            app.auth = auth;
            for (w, h) in [(120, 50), (60, 20), (5, 3)] {
                render(&mut app, w, h);
            }
        }
        app.auth = Auth::Qr(Some("tg://login?token=abc".to_owned()));
        assert!(render(&mut app, 120, 50).contains("Link Desktop Device"));
    }
}
