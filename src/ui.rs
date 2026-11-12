use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell, CellDiffOption, CellWidth};
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::{Frame, Terminal};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Auth, Pane, Popup, Row};
use crate::config::{APP_NAME, MessageLayout};

const QR_CAPTION: &str = "Scan in Telegram → Settings → Devices → Link Desktop Device";
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};

use crate::format::{self, render_text};
use crate::media::{self, Input, Key, Look, Media, Src, album_layout, fit_cells, layout_height};
use crate::model::{self, Attachment, ChatKind, Decode, MediaKind, Msg, SendState, Sender, Visual};
use crate::popups::draw as draw_stage4_popup;
use crate::qr;
use crate::textmap::{self, Hit, Join, Zone};
use crate::theme;

#[path = "layouts.rs"]
mod layouts;

pub struct Resync<B>(pub B);

fn settled(cell: &Cell) -> bool {
    if matches!(cell.diff_option, CellDiffOption::ForcedWidth(_)) {
        return true;
    }
    let mut chars = cell.symbol().chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            c < '\u{600}'
                || ('\u{2000}'..'\u{2600}').contains(&c)
                || (c < '\u{1F000}' && UnicodeWidthChar::width(c) == Some(2))
        }
        _ => false,
    }
}

impl<B: Backend> Backend for Resync<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut cells: Vec<(u16, u16, Cow<'a, Cell>, bool)> = Vec::new();
        let mut covered: Option<(u16, u16)> = None;
        for (x, y, cell) in content {
            if covered.is_some_and(|(cy, end)| cy == y && x < end) {
                continue;
            }
            covered = None;
            if settled(cell) {
                cells.push((x, y, Cow::Borrowed(cell), false));
                continue;
            }
            let w = cell.cell_width();
            for dx in 1..w {
                let mut blank = cell.clone();
                blank.set_symbol(" ");
                cells.push((x.saturating_add(dx), y, Cow::Owned(blank), false));
            }
            if w > 1 {
                covered = Some((y, x.saturating_add(w)));
            }
            cells.push((x, y, Cow::Borrowed(cell), true));
        }
        let mut start = 0;
        for i in 1..=cells.len() {
            let cut = cells.get(i).is_none_or(|&(x, y, _, _)| {
                let (px, py, _, unsure) = &cells[i - 1];
                *unsure && y == *py && x == px.saturating_add(1)
            });
            if cut {
                self.0.draw(
                    cells[start..i]
                        .iter()
                        .map(|(x, y, c, _)| (*x, *y, c.as_ref())),
                )?;
                start = i;
            }
        }
        if cells.is_empty() {
            self.0.draw(std::iter::empty())?;
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.0.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Backend::flush(&mut self.0)
    }
}

impl<W: std::io::Write> std::io::Write for Resync<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.0)
    }
}

pub fn draw_frame<W: std::io::Write>(
    terminal: &mut Terminal<Resync<CrosstermBackend<W>>>,
    app: &mut App,
    layout: &mut u64,
    drawn: impl FnOnce(&Buffer),
) -> std::io::Result<()> {
    app.sync_inline();
    queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let force = std::mem::take(&mut app.force_clear);
    let frame = terminal.draw(|f| draw(f, app))?;
    let next = image_layout(frame.buffer);
    if next == *layout && !force {
        drawn(frame.buffer);
    } else {
        *layout = next;
        terminal.draw(|_| {})?;
        terminal.backend_mut().clear_region(ClearType::All)?;
        drawn(terminal.draw(|f| draw(f, app))?.buffer);
    }
    app.media.anim.finish(terminal.backend_mut())?;
    app.inline.flush(terminal.backend_mut())?;
    for seq in std::mem::take(&mut app.terminal_out) {
        std::io::Write::write_all(terminal.backend_mut(), seq.as_bytes())?;
    }
    execute!(terminal.backend_mut(), EndSynchronizedUpdate)
}

pub fn image_layout(buf: &Buffer) -> u64 {
    let mut h = DefaultHasher::new();
    buf.area.hash(&mut h);
    for (i, c) in buf.content.iter().enumerate() {
        if !matches!(c.diff_option, CellDiffOption::None) {
            i.hash(&mut h);
        }
    }
    h.finish()
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    textmap::begin();
    paint(frame, app);
    app.text_map = textmap::finish(frame.buffer_mut(), app.selection.as_ref());
}

fn paint(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let th = app.frame_theme();
    theme::install(th.clone());
    app.animating = false;
    app.redraw_due = None;
    let covered = matches!(app.popup, Some(Popup::Viewer { .. } | Popup::Stickers(_)));
    let unfocused = app.settings.media.pause_unfocused && !app.term_focused;
    app.media.anim.begin(covered || unfocused);
    app.visible_msgs.clear();
    app.prune_typing(std::time::Instant::now());
    app.expire_status(std::time::Instant::now());
    frame.render_widget(Block::default().style(theme::surface()), area);
    if app.auth != Auth::Ready {
        draw_login(frame, area, app);
        return;
    }
    app.refresh_lists();
    let [header, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    draw_header(frame, header, app);
    let [fw, cw, iw] = pane_widths(main.width, app, &th.layout);
    let [folders, chats, messages, info] = Layout::horizontal([
        Constraint::Length(fw),
        Constraint::Length(cw),
        Constraint::Min(0),
        Constraint::Length(iw),
    ])
    .areas(main);
    if fw > 0 {
        draw_folders(frame, folders, app);
    }
    if cw > 0 {
        draw_chats(frame, chats, app);
    }
    draw_messages(frame, messages, app);
    if iw > 0 {
        draw_info(frame, info, app);
    }
    if matches!(app.popup, Some(Popup::Viewer { .. })) {
        draw_viewer(frame, area, app);
    } else if matches!(app.popup, Some(Popup::Stickers(_))) {
        draw_stickers(frame, area, app);
    } else {
        draw_popup(frame, area, app);
        draw_profile_avatar(frame, area, app);
    }
    draw_key_hint(frame, main, app);
}

const MIN_MESSAGES: u16 = 30;
const MIN_FOLDERS: u16 = 16;
const MIN_CHATS: u16 = 24;
const MIN_INFO: u16 = 24;

fn pane_widths(total: u16, app: &App, l: &theme::LayoutOptions) -> [u16; 3] {
    let want = |shown: bool, w: theme::PaneWidth| if shown { w.cols(total) } else { 0 };
    let mut w = [
        want(app.show_folders, l.folders_width),
        want(app.show_chats, l.chats_width),
        want(app.show_info, l.info_width),
    ];
    let floor = |w: &[u16; 3]| {
        [
            w[0].min(MIN_FOLDERS),
            w[1].min(MIN_CHATS),
            w[2].min(MIN_INFO),
        ]
    };
    let need = |w: [u16; 3]| w.iter().sum::<u16>() + MIN_MESSAGES;
    if app.focus != Pane::Info {
        let spare = total.saturating_sub(need([w[0], w[1], 0]));
        w[2] = if spare >= w[2].min(MIN_INFO) {
            w[2].min(spare)
        } else {
            0
        };
    }
    if need(floor(&w)) > total && app.focus != Pane::Folders {
        w[0] = 0;
    }
    let min = floor(&w);
    for i in [2, 0, 1] {
        let over = need(w).saturating_sub(total);
        w[i] -= over.min(w[i] - min[i]);
    }
    for i in [2, 0, 1] {
        let over = w.iter().sum::<u16>().saturating_sub(total);
        w[i] -= over.min(w[i]);
    }
    w
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        theme::focus_border()
    } else {
        theme::border()
    };
    let title_style = theme::title(focused);
    Block::default()
        .borders(Borders::ALL)
        .border_set(theme::border_set())
        .border_style(style)
        .title(Span::styled(format::sanitize(&title), title_style))
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let g = &theme::get().glyphs;
    let mut spans = vec![Span::styled(format!(" {APP_NAME} "), theme::header_title())];
    if let Some(state) = app.connection {
        spans.push(Span::styled(format!(" {state} "), theme::warning()));
    }
    if app.settings.privacy.ghost_mode {
        spans.push(Span::styled(
            format!(" {} ghost ", g.ghost),
            theme::warning(),
        ));
    } else if app.settings.privacy.dont_mark_read {
        spans.push(Span::styled(
            format!(" {} no-read ", g.no_read),
            theme::muted(),
        ));
    }
    let status = app.status.as_ref().map(|(text, error)| {
        let style = if *error {
            theme::error()
        } else {
            theme::status()
        };
        (crate::td::sentence_case(&format::one_line(text)), style)
    });
    let used: usize = spans.iter().map(|s| format::cells(&s.content)).sum();
    let mut free = width
        .saturating_sub(used)
        .saturating_sub(status.as_ref().map_or(0, |(t, _)| format::cells(t) + 2));
    if app.connection.is_none()
        && let Some(name) = app.me_name()
    {
        let name = format::sanitize(&name);
        let need = format::cells(&name) + " Connected as  ".len();
        if need <= free {
            free -= need;
            spans.push(Span::styled(" Connected as ", theme::muted()));
            spans.push(Span::styled(format!("{name} "), theme::strong()));
        }
    }
    if let Some(p) = &app.playing {
        let label = format::sanitize(&p.label);
        let need = format::cells(&label) + format::cells(&g.playing) + 3;
        let label = if need <= free {
            Some(label)
        } else if free >= 16 {
            Some(format::truncate(
                &label,
                free - format::cells(&g.playing) - 3,
            ))
        } else {
            None
        };
        if let Some(label) = label {
            spans.push(Span::styled(
                format!(" {} {label} ", g.playing),
                theme::media(),
            ));
        }
    }
    if let Some((text, style)) = status {
        let used: usize = spans.iter().map(|s| format::cells(&s.content)).sum();
        let room = width.saturating_sub(used + 2);
        if room > 0 {
            spans.push(Span::styled(
                format!(" {} ", format::truncate(&text, room)),
                style,
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn split_filter(area: Rect, active: bool) -> (Rect, Option<Rect>) {
    if !active || area.height < 2 {
        return (area, None);
    }
    let bar_y = area.bottom() - 1;
    (
        Rect::new(area.x, area.y, area.width, bar_y - area.y),
        Some(Rect::new(area.x, bar_y, area.width, 1)),
    )
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
        Span::styled("Search…", theme::muted())
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

pub(crate) fn list_window(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        0
    } else {
        selected.saturating_sub(height / 2).min(len - height)
    }
}

pub(crate) fn row_line(
    label: Vec<Span<'static>>,
    selected: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let mark = theme::selection_marker();
    let mut used = mark.width();
    let marker = if selected && focused {
        Span::styled(mark, theme::selection())
    } else {
        Span::raw(" ".repeat(used))
    };
    let mut spans = vec![marker];
    for mut span in label {
        let avail = width.saturating_sub(used);
        if avail == 0 {
            break;
        }
        span = Span::styled(format::truncate(&span.content, avail), span.style);
        used += format::cells(&span.content);
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
    textmap::area(Zone::Pane(Pane::Folders), area);
    let filter_on = app.filter.as_ref().is_some_and(|f| f.pane == Pane::Folders);
    let (list, bar) = split_filter(inner, filter_on);
    let entries = app.folder_entries();
    let h = list.height as usize;
    app.folder_rows.set(h);
    let none = entries.is_empty();
    if let Some(f) = app
        .filter
        .as_ref()
        .filter(|f| f.pane == Pane::Folders && none)
    {
        let text = format!("No folders match ‘{}’", f.input.text());
        let wrap = ratatui::widgets::Wrap { trim: true };
        let pad = Block::default().padding(Padding::horizontal(1));
        let empty = Paragraph::new(Span::styled(text, theme::muted())).wrap(wrap);
        frame.render_widget(empty.block(pad), list);
    }
    let start = list_window(app.folder_sel, entries.len(), h);
    let mark = theme::selection_marker().width() as u16;
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(h)
        .map(|(i, (id, name))| {
            let w = (list.width as usize).saturating_sub(theme::selection_marker().width());
            let badge = app
                .folder_badge(*id)
                .map(|n| format!(" {n}"))
                .unwrap_or_default();
            let title = format::truncate(name, w.saturating_sub(badge.width()));
            let y = list.y + (i - start) as u16;
            textmap::hit(Rect::new(list.x, y, list.width, 1), Hit::Folder(i));
            textmap::row(list.x + mark, y, format::cells(&title) as u16, Join::Line);
            let pad = w.saturating_sub(title.width() + badge.width());
            row_line(
                vec![
                    Span::raw(title),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(badge, theme::muted()),
                ],
                i == app.folder_sel,
                focused,
                list.width as usize,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    draw_filter_bar(frame, bar, app, Pane::Folders);
}

const TWO_LINE_MIN_WIDTH: usize = 16;
const TITLE_MIN: usize = 8;

fn empty_chats_text(app: &App) -> String {
    let filter = app.filter.as_ref().filter(|f| f.pane == Pane::Chats);
    if let Some(f) = filter.filter(|f| !f.input.is_empty()) {
        return format!("No chats match ‘{}’", f.input.text());
    }
    let list = app.current_list();
    if !app.list_complete(list) {
        return "Loading chats…".to_owned();
    }
    match list {
        model::ListId::Main => "No chats yet",
        model::ListId::Archive => "Archive is empty",
        model::ListId::Folder(_) => "No chats in this folder",
    }
    .to_owned()
}

fn rows_window(heights: &[usize], selected: usize, height: usize) -> usize {
    let Some(last) = heights.len().checked_sub(1).filter(|_| height > 0) else {
        return 0;
    };
    let sel = selected.min(last);
    let mut tail = heights.len();
    let mut below = 0;
    while tail > 0 && below + heights[tail - 1] <= height {
        tail -= 1;
        below += heights[tail];
    }
    let mut start = sel;
    let mut above = 0;
    while start > 0
        && above + heights[start - 1] <= height / 2
        && above + heights[start - 1] + heights[sel] <= height
    {
        start -= 1;
        above += heights[start];
    }
    start.min(tail)
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
    textmap::area(Zone::Pane(Pane::Chats), area);
    let filter_on = app.filter.as_ref().is_some_and(|f| f.pane == Pane::Chats);
    let (list, bar) = split_filter(inner, filter_on);
    let rows = app.chat_rows();
    let h = list.height as usize;
    let w = list.width as usize;
    if rows.is_empty() {
        app.chat_rows.set(h);
        let text = Span::styled(empty_chats_text(app), theme::muted());
        let wrap = ratatui::widgets::Wrap { trim: true };
        let pad = Block::default().padding(Padding::horizontal(1));
        frame.render_widget(Paragraph::new(text).wrap(wrap).block(pad), list);
        draw_filter_bar(frame, bar, app, Pane::Chats);
        return;
    }
    let two = !app.settings.appearance.compact_chat_list && w >= TWO_LINE_MIN_WIDTH;
    let heights: Vec<usize> = rows
        .iter()
        .map(|r| {
            if two && matches!(r, Row::Chat(_)) {
                2
            } else {
                1
            }
        })
        .collect();
    let sel = app.chat_sel.min(rows.len() - 1);
    let start = rows_window(&heights, sel, h);
    let now = crate::app::unix_now();
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    let mut shown = 0;
    for (i, row) in rows.iter().enumerate().skip(start) {
        let room = h.saturating_sub(lines.len());
        if room == 0 {
            break;
        }
        let selected = i == sel;
        let (row_lines, text) = match *row {
            Row::Chat(id) => match app.chats.get(&id) {
                Some(c) => chat_lines(app, c, selected, focused, w, two, now),
                None => continue,
            },
            Row::Topic(chat, t) => (vec![topic_line(app, chat, t, selected, focused, w)], None),
            Row::Loading(_) => {
                let label = vec![Span::styled("   Loading topics…", theme::muted())];
                (vec![row_line(label, selected, focused, w)], None)
            }
        };
        let n = row_lines.len().min(room);
        let y = list.y + lines.len() as u16;
        textmap::hit(Rect::new(list.x, y, list.width, n as u16), Hit::Chat(*row));
        if let Some((x, width)) = text {
            textmap::row(list.x + x as u16, y, width as u16, Join::Line);
        }
        lines.extend(row_lines.into_iter().take(n));
        shown += 1;
    }
    app.chat_rows.set(shown);
    frame.render_widget(Paragraph::new(lines), list);
    draw_filter_bar(frame, bar, app, Pane::Chats);
}

fn chat_badge(app: &App, c: &model::ChatInfo, muted: bool) -> (String, Style) {
    let g = &theme::get().glyphs;
    let mut badge = String::new();
    let mut style = theme::muted_badge();
    if c.mentions > 0 {
        badge = format!(" {}", g.mention_dot);
        style = theme::mention_badge();
    }
    let unread = app.chat_unread(c.id);
    if unread > 0 {
        let n = app.count(i64::from(unread));
        if g.unread_dot.is_empty() {
            badge.push_str(&format!(" {n}"));
        } else {
            badge.push_str(&format!(" {} {n}", g.unread_dot));
        }
        if c.mentions == 0 && !muted {
            style = theme::unread_badge();
        }
    }
    (badge, style)
}

fn typing_preview(app: &App, c: &model::ChatInfo) -> Option<String> {
    let private = matches!(c.kind, ChatKind::Private(_) | ChatKind::Secret(_));
    if !private {
        return app.typing_line(c.id, None);
    }
    let now = std::time::Instant::now();
    let entries: Vec<(String, crate::presence::Activity)> = app
        .typing
        .get(&c.id)?
        .iter()
        .filter(|t| !t.expired(now))
        .map(|t| (String::new(), t.activity))
        .collect();
    crate::presence::typing_text_in(app.lang, &entries, true)
}

fn media_preview(label: &str) -> (String, bool) {
    let label = format::one_line(label);
    match label.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        Some(inner) => {
            let mut chars = inner.chars();
            let head: String = chars
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .collect();
            (format!("{head}{}", chars.as_str()), true)
        }
        None => (label, false),
    }
}

fn chat_preview(app: &App, c: &model::ChatInfo) -> Vec<Span<'static>> {
    if let Some(text) = typing_preview(app, c) {
        return vec![Span::styled(text, theme::typing())];
    }
    if let Some(draft) = app
        .drafts
        .get(&(c.id, None))
        .filter(|d| !d.text.trim().is_empty())
    {
        return vec![
            Span::styled("Draft: ", theme::error()),
            Span::styled(format::one_line(&draft.text), theme::muted()),
        ];
    }
    let Some(m) = c.last_message.as_ref() else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    let who = match c.kind {
        ChatKind::Channel => None,
        _ if m.outgoing => Some("You".to_owned()),
        ChatKind::Group | ChatKind::Supergroup => {
            let name = app.sender_name(m.sender);
            Some(name.split(' ').next().unwrap_or(&name).to_owned())
        }
        _ => None,
    };
    if let Some(who) = who {
        spans.push(Span::styled(format!("{who}: "), theme::normal()));
    }
    let text = format::one_line(&m.body.text);
    if let Some(label) = m.media.as_deref() {
        let (label, media) = media_preview(label);
        let style = if media {
            theme::media()
        } else {
            theme::muted()
        };
        spans.push(Span::styled(label, style));
        if !text.is_empty() {
            spans.push(Span::styled(format!(" {text}"), theme::muted()));
        }
    } else {
        spans.push(Span::styled(text, theme::muted()));
    }
    spans
}

fn chat_lines(
    app: &App,
    c: &model::ChatInfo,
    selected: bool,
    focused: bool,
    w: usize,
    two: bool,
    now: i64,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let muted = app.is_muted(c.id);
    let th = theme::get();
    let g = &th.glyphs;
    let (badge, badge_style) = chat_badge(app, c, muted);
    let mut title_style = if app.chat_unread(c.id) > 0 && !muted {
        theme::unread()
    } else if muted {
        theme::muted()
    } else {
        theme::normal()
    };
    if app.open_chat == Some(c.id) {
        title_style = title_style.patch(theme::open_chat());
    }
    let (prefix, prefix_style) = match c.kind {
        _ if app.typing.contains_key(&c.id) => (g.typing.as_str(), theme::typing()),
        ChatKind::Private(uid) | ChatKind::Secret(uid) if app.is_online(uid) => {
            (g.online.as_str(), theme::online())
        }
        ChatKind::Secret(_) => (g.secret.as_str(), Style::default()),
        _ if c.forum && app.expanded == Some(c.id) => (g.forum_open.as_str(), theme::muted()),
        _ if c.forum => (g.forum_closed.as_str(), theme::muted()),
        _ => ("", Style::default()),
    };
    let state = app
        .secret_chat(c.id)
        .map(|s| s.state.short())
        .filter(|s| !s.is_empty())
        .map(|s| format!(" ({s})"))
        .unwrap_or_default();
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix} ")
    };
    let pinned = c.position_in(app.current_list()).is_some_and(|p| p.pinned);
    let pin_mark = if pinned {
        format!(" {}", g.pinned_chat)
    } else {
        String::new()
    };
    let mark = theme::selection_marker().width();
    let avail = w.saturating_sub(mark);
    let head = (format::cells(&prefix) + format::cells(&c.title)).min(TITLE_MIN);
    let fits = |extra: usize| avail.saturating_sub(extra) >= head;
    let time = c
        .last_message
        .as_ref()
        .filter(|_| two)
        .map(|m| format!(" {}", format::list_time(app.lang, m.date, now)))
        .filter(|t| fits(t.width()))
        .unwrap_or_default();
    let mut right = vec![Span::styled(time, theme::time())];
    let mut extras = vec![
        Span::styled(pin_mark, theme::muted()),
        Span::styled(badge, badge_style),
    ];
    if !two {
        right = std::mem::take(&mut extras);
    }
    if !fits(spans_width(&right)) {
        right.remove(0);
    }
    if !fits(spans_width(&right)) {
        right.clear();
    }
    let avail_left = avail.saturating_sub(spans_width(&right));
    let prefix = format::truncate(&prefix, avail_left);
    let room = avail_left.saturating_sub(prefix.width());
    let state = format::truncate(&state, room / 2);
    let title = format::truncate(&c.title, room.saturating_sub(state.width()));
    let pad = avail_left.saturating_sub(prefix.width() + title.width() + state.width());
    let text = (mark + format::cells(&prefix), format::cells(&title));
    let indent = format::cells(&prefix);
    let mut spans = vec![
        Span::styled(prefix, title_style.patch(prefix_style)),
        Span::styled(title, title_style),
        Span::styled(state, theme::muted()),
        Span::raw(" ".repeat(pad)),
    ];
    spans.extend(right);
    let mut lines = vec![row_line(spans, selected, focused, w)];
    if two {
        let extras_w = spans_width(&extras);
        let room = avail.saturating_sub(indent + extras_w);
        let preview = truncate_spans(chat_preview(app, c), room);
        let pad = room.saturating_sub(spans_width(&preview));
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        spans.extend(preview);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(extras);
        lines.push(row_line(spans, selected, false, w));
    }
    (lines, Some(text))
}

fn topic_line(
    app: &App,
    chat: i64,
    topic: i32,
    selected: bool,
    focused: bool,
    w: usize,
) -> Line<'static> {
    let th = theme::get();
    let g = &th.glyphs;
    let Some(t) = app.topic(chat, topic) else {
        return row_line(Vec::new(), selected, focused, w);
    };
    let muted = app.topic_muted(chat, topic);
    let open = app.open_chat == Some(chat) && app.open_topic == Some(topic);
    let mut style = if t.unread > 0 && !muted {
        theme::unread()
    } else if muted || (t.general && t.hidden) {
        theme::muted()
    } else {
        theme::normal()
    };
    if open {
        style = style.patch(theme::open_chat());
    }
    let (icon, icon_style) = if t.icon.is_empty() {
        (g.topic.clone(), theme::topic_icon(t.color))
    } else {
        (t.icon.clone(), Style::default())
    };
    let mut badge = String::new();
    let mut badge_style = theme::muted_badge();
    if t.mentions > 0 {
        badge = format!(" {}", g.mention_dot);
        badge_style = theme::mention_badge();
    }
    if t.unread > 0 {
        badge.push_str(&format!(" {}", app.count(i64::from(t.unread))));
        if t.mentions == 0 && !muted {
            badge_style = theme::unread_badge();
        }
    }
    let mut marks = String::new();
    if t.pinned {
        marks.push_str(&format!(" {}", g.pinned_chat));
    }
    if t.closed && !t.hidden {
        marks.push_str(&format!(" {}", g.topic_closed));
    }
    if t.general && t.hidden {
        marks.push_str(" (hidden)");
    }
    let lead = format!("  {icon} ");
    let avail = w
        .saturating_sub(theme::selection_marker().width())
        .saturating_sub(badge.width())
        .saturating_sub(marks.width());
    let lead = format::truncate(&lead, avail);
    let name = format::truncate(&t.name, avail.saturating_sub(lead.width()));
    let rest = avail.saturating_sub(lead.width() + name.width());
    let preview = match t.last_message.as_ref() {
        Some(m) if rest > 6 => {
            let text = model::preview(m);
            let who = if m.outgoing {
                String::new()
            } else {
                let n = app.sender_name(m.sender);
                format!("{}: ", n.split(' ').next().unwrap_or(&n))
            };
            format::truncate(&format!(" {who}{text}"), rest)
        }
        _ => String::new(),
    };
    let pad = rest.saturating_sub(preview.width());
    let spans = vec![
        Span::styled(lead, style.patch(icon_style)),
        Span::styled(name, style),
        Span::styled(preview, theme::muted()),
        Span::raw(" ".repeat(pad)),
        Span::styled(marks, theme::muted()),
        Span::styled(badge, badge_style),
    ];
    row_line(spans, selected, focused, w)
}

fn sender_id(sender: Sender) -> i64 {
    match sender {
        Sender::User(id) | Sender::Chat(id) => id,
    }
}

fn author_style(app: &App, sender: Sender) -> Style {
    let own = matches!(sender, Sender::User(id) if app.my_id == Some(id));
    theme::author(sender_id(sender), own)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pic {
    Att { rel: usize, att: usize },
    Avatar(Sender),
}

#[derive(Clone, Copy, Debug)]
struct Place {
    line: usize,
    col: u16,
    cols: u16,
    rows: u16,
    pic: Pic,
}

#[derive(Clone, Copy, Debug)]
struct TextPlace {
    line: usize,
    col: u16,
    width: u16,
    join: Join,
}

#[derive(Clone)]
struct Rendered {
    lines: Vec<Line<'static>>,
    images: Vec<Place>,
    texts: Vec<TextPlace>,
}

fn push_text(
    body: &mut Vec<Line<'static>>,
    texts: &mut Vec<TextPlace>,
    rt: &model::RichText,
    style: Style,
    width: usize,
) {
    for (line, wrap) in format::render_rows(rt, style, width) {
        texts.push(TextPlace {
            line: body.len(),
            col: wrap.indent as u16,
            width: wrap.width as u16,
            join: wrap.join,
        });
        body.push(line);
    }
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

fn truncate_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let room = width.saturating_sub(used);
        if room == 0 {
            break;
        }
        let safe = format::sanitize(&span.content);
        let w = format::cells(&safe);
        if w > room {
            out.push(Span::styled(format::truncate(&safe, room), span.style));
            break;
        }
        used += w;
        out.push(Span::styled(safe, span.style));
    }
    out
}

fn meta_line(
    app: &App,
    m: &Msg,
    idx: usize,
    att: &Attachment,
    width: usize,
) -> Option<Line<'static>> {
    let th = theme::get();
    let g = &th.glyphs;
    let f = app.media.file(&att.file);
    let size = (f.size > 0).then(|| format::file_size(f.size));
    let dur = format::duration(att.duration);
    let join = |parts: Vec<String>| parts.join(" · ");
    let media = theme::media();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let text = match att.kind {
        MediaKind::Photo | MediaKind::Sticker => String::new(),
        MediaKind::Video => join(
            [Some(format!("{} video {dur}", g.video)), size]
                .into_iter()
                .flatten()
                .collect(),
        ),
        MediaKind::Animation => join([Some(g.gif.clone()), size].into_iter().flatten().collect()),
        MediaKind::VideoNote => format!("{} video message {dur}", g.video_note),
        MediaKind::Voice => {
            let bars_w = width.saturating_sub(24).clamp(0, 32);
            let bars = model::waveform_bars(&att.waveform, bars_w);
            spans.push(Span::styled(format!("{} {dur}", g.voice), media));
            if !bars.trim().is_empty() {
                spans.push(Span::styled(" ", media));
                spans.push(Span::styled(bars.trim_end().to_owned(), theme::waveform()));
            }
            size.map(|s| format!(" · {s}")).unwrap_or_default()
        }
        MediaKind::Audio => {
            let title = m
                .media
                .as_deref()
                .map_or(att.name.as_str(), |l| l.trim_start_matches("♪ "));
            join(
                [Some(format!("{} {title}", g.audio)), Some(dur), size]
                    .into_iter()
                    .flatten()
                    .collect(),
            )
        }
        MediaKind::Document => join(
            [Some(format!("{} {}", g.file, att.name)), size]
                .into_iter()
                .flatten()
                .collect(),
        ),
    };
    let mut tail = text;
    if let Some(p) = f.upload_percent() {
        tail.push_str(&format!(" {} {p}%", g.upload));
    } else if let Some(p) = f.percent() {
        tail.push_str(&format!(" {} {p}%", g.download));
    } else if f.active {
        tail.push_str(&format!(" {}", g.download));
    }
    if let Some(e) = app.media.file_error(f.id) {
        tail.push_str(&format!(" {} {e}", g.tick_failed));
    }
    if app
        .playing
        .as_ref()
        .is_some_and(|p| p.target == (m.id, idx))
    {
        tail.push_str(&format!(" {} playing", g.playing));
    }
    if spans.is_empty() {
        tail = tail.trim().to_owned();
    }
    if !tail.is_empty() {
        spans.push(Span::styled(tail, media));
    }
    (!spans.is_empty()).then(|| Line::from(truncate_spans(spans, width)))
}

fn gutter() -> usize {
    theme::get().layout.message_gutter as usize
}

fn composer_pad() -> u16 {
    theme::get().layout.composer_padding
}

const CLASSIC_AVATAR_COLS: u16 = 2;
const MIN_AVATAR_ROWS: u16 = 2;
const CLASSIC_RUN_SECS: i64 = 300;

fn tick_span(app: &App, last: &Msg) -> Option<Span<'static>> {
    if !last.outgoing {
        return None;
    }
    let read = app.read_outbox(last.chat_id) >= last.id;
    let (glyph, style) = theme::tick(match &last.state {
        SendState::Pending => theme::Tick::Pending,
        SendState::Failed(_) => theme::Tick::Failed,
        SendState::Sent if read => theme::Tick::Read,
        SendState::Sent => theme::Tick::Sent,
    });
    Some(Span::styled(glyph, style))
}

fn edited_label(lang: format::Lang) -> String {
    let glyph = theme::get().glyphs.edited.clone();
    if glyph != "auto" {
        return glyph;
    }
    match lang {
        format::Lang::Ru => "изм.",
        format::Lang::En => "edited",
    }
    .to_owned()
}

fn edited_span(lang: format::Lang) -> Span<'static> {
    Span::styled(edited_label(lang), theme::edited())
}

fn pin_span() -> Span<'static> {
    Span::styled(theme::get().glyphs.pinned.clone(), theme::pinned())
}

fn day_separator(ctx: Ctx, ts: i64) -> Vec<Line<'static>> {
    let width = ctx.width;
    let label = format!(" {} ", format::day_label(ctx.lang, ts, ctx.now));
    let mode = match theme::get().layout.date_separator {
        theme::DateSeparator::Auto => match ctx.layout {
            MessageLayout::Bubbles => theme::DateSeparator::Badge,
            MessageLayout::Minimal => theme::DateSeparator::Plain,
            _ => theme::DateSeparator::Line,
        },
        other => other,
    };
    let pad = width.saturating_sub(label.width()) / 2;
    match mode {
        theme::DateSeparator::None => Vec::new(),
        theme::DateSeparator::Line | theme::DateSeparator::Auto => vec![Line::from(Span::styled(
            format::separator(&label, width),
            theme::date_separator(),
        ))],
        theme::DateSeparator::Badge => vec![Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(label, theme::date_badge()),
        ])],
        theme::DateSeparator::Plain => vec![Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(label, theme::date_separator()),
        ])],
    }
}

fn forward_line(app: &App, m: &Msg, lang: format::Lang, width: usize) -> Option<Line<'static>> {
    let origin = m.forward.as_ref()?;
    Some(Line::from(Span::styled(
        format::truncate(&forward_text(app, origin, lang), width),
        theme::forwarded(),
    )))
}

fn forward_text(app: &App, origin: &model::Origin, lang: format::Lang) -> String {
    let label = match lang {
        format::Lang::Ru => "Переслано от",
        format::Lang::En => "Forwarded from",
    };
    let glyph = &theme::get().glyphs.forwarded;
    format!("{glyph} {label} {}", origin_name(app, origin))
        .trim_start()
        .to_owned()
}

fn reply_strip(app: &App, m: &Msg, lang: format::Lang, width: usize) -> Option<Line<'static>> {
    let (sender, name, text) = reply_parts(app, m)?;
    let bar_style = theme::reply_bar(sender.map(sender_id));
    let bar = format!("{} ", theme::get().glyphs.reply_bar);
    let room = width.saturating_sub(bar.width());
    let mut spans = vec![Span::styled(bar, bar_style)];
    if name.is_empty() {
        let label = match lang {
            format::Lang::Ru => "ответ на сообщение",
            format::Lang::En => "reply to a message",
        };
        spans.push(Span::styled(
            format::truncate(label, room),
            theme::reply_text(),
        ));
        return Some(Line::from(spans));
    }
    let name = format::truncate(&name, room);
    let rest = room.saturating_sub(name.width());
    spans.push(Span::styled(name, theme::strong().patch(bar_style)));
    if !text.is_empty() && rest > 3 {
        spans.push(Span::styled(
            format::truncate(&format!(": {text}"), rest),
            theme::reply_text(),
        ));
    }
    Some(Line::from(spans))
}

fn classic_avatar(app: &App, sender: Sender) -> Span<'static> {
    let letter: String = initials(&app.sender_name(sender)).chars().take(1).collect();
    let cols = CLASSIC_AVATAR_COLS as usize;
    let pad = cols.saturating_sub(letter.width());
    Span::styled(
        format!(
            "{}{letter}{}",
            " ".repeat(pad / 2),
            " ".repeat(pad - pad / 2)
        ),
        theme::avatar(sender_id(sender)),
    )
}

fn group_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let m = &msgs[range.start];
    let group = &msgs[range.clone()];
    let last = &msgs[range.end - 1];
    let prev = range.start.checked_sub(1).map(|j| &msgs[j]);
    let new_day = prev.is_none_or(|p| !format::same_day(p.date, m.date));
    let show_header =
        new_day || prev.is_none_or(|p| p.sender != m.sender || m.date - p.date > CLASSIC_RUN_SECS);
    let edited = group.iter().any(|g| g.edited);
    let pinned = group.iter().any(|g| g.pinned);
    let tick = tick_span(app, last);
    let group_chat = app
        .chats
        .get(&m.chat_id)
        .is_some_and(|c| matches!(c.kind, ChatKind::Group | ChatKind::Supergroup));
    let mut lines: Vec<Line<'static>> = Vec::new();
    if new_day {
        if prev.is_some() {
            lines.push(Line::default());
        }
        lines.extend(day_separator(ctx, m.date));
    } else if show_header {
        lines.push(Line::default());
    }
    let gutter = gutter();
    let inner = width.saturating_sub(2 * gutter.max(2)).max(4);
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut images = Vec::new();
    let time = format::time_label(m.date);
    if show_header {
        let mut header = Vec::new();
        let mut tail = vec![Span::raw(" "), Span::styled(time, theme::time())];
        if let Some(t) = tick.clone() {
            tail.push(Span::raw(" "));
            tail.push(t);
        }
        if edited {
            tail.push(Span::styled(" · ", theme::muted()));
            tail.push(edited_span(ctx.lang));
        }
        if pinned {
            tail.push(Span::raw(" "));
            tail.push(pin_span());
        }
        let room = inner
            .saturating_sub(spans_width(&header) + spans_width(&tail))
            .max(1);
        header.push(Span::styled(
            format::truncate(&app.sender_name(m.sender), room),
            if selected {
                theme::active_field()
            } else {
                author_style(app, m.sender)
            },
        ));
        header.extend(tail);
        body.push(Line::from(header));
    }
    body.extend(forward_line(app, m, ctx.lang, inner));
    body.extend(reply_strip(app, m, ctx.lang, inner));
    let before = body.len();
    let mut texts = group_content(
        app,
        msgs,
        range.clone(),
        inner,
        true,
        &mut body,
        &mut images,
    );
    if !show_header {
        let from = before.max(layouts::media_end(&images));
        layouts::attach_right(
            &mut body,
            from,
            layouts::marks(edited, pinned, tick, ctx.lang),
            inner,
        );
    }
    let offset = lines.len();
    let lead = gutter.max(usize::from(selected));
    for p in &mut images {
        p.line += offset;
        p.col += lead as u16;
    }
    for t in &mut texts {
        t.line += offset;
        t.col += lead as u16;
    }
    lines.extend(body.into_iter().map(|mut l| {
        l.spans.insert(0, Span::raw(" ".repeat(lead)));
        if selected { select_row(l, width) } else { l }
    }));
    if !selected
        && show_header
        && group_chat
        && theme::get().layout.avatars
        && gutter >= CLASSIC_AVATAR_COLS as usize
    {
        let rest = gutter - CLASSIC_AVATAR_COLS as usize;
        lines[offset].spans[0] = Span::raw(" ".repeat(rest));
        lines[offset].spans.insert(0, classic_avatar(app, m.sender));
    }
    Rendered {
        lines,
        images,
        texts,
    }
}

fn group_content(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    inner: usize,
    ticks: bool,
    body: &mut Vec<Line<'static>>,
    images: &mut Vec<Place>,
) -> Vec<TextPlace> {
    let mut texts = Vec::new();
    let group = &msgs[range.clone()];
    let media_w = inner as u16;
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
    if let [(mi, ai)] = visuals.as_slice() {
        let (cols, rows) = media_box(&msgs[*mi].attachments[*ai], font, media_w);
        images.push(Place {
            line: body.len(),
            col: 0,
            cols,
            rows,
            pic: Pic::Att {
                rel: *mi - range.start,
                att: *ai,
            },
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
                pic: Pic::Att {
                    rel: *mi - range.start,
                    att: *ai,
                },
            });
        }
        body.extend(std::iter::repeat_n(
            Line::default(),
            layout_height(&cells) as usize,
        ));
    }
    for g in group {
        for (ai, att) in g.attachments.iter().enumerate() {
            if let Some(line) = meta_line(app, g, ai, att, inner) {
                body.push(line);
            }
        }
        if g.attachments.is_empty()
            && let Some(media) = &g.media
        {
            push_text(
                body,
                &mut texts,
                &crate::model::RichText::plain(media.clone()),
                theme::media(),
                inner,
            );
        }
    }
    for g in group {
        if !g.body.text.is_empty() {
            push_text(body, &mut texts, &g.body, theme::normal(), inner);
        }
        if g.ttl != 0 {
            let text = format!("{} {}", theme::get().glyphs.timer, format::ttl(g.ttl));
            body.push(Line::from(Span::styled(
                format::truncate(&text, inner),
                theme::muted(),
            )));
        }
    }
    for g in group {
        if let Some(kb) = g.keyboard.as_ref().filter(|k| k.inline) {
            body.extend(keyboard_block(
                kb,
                inner,
                button_focus(app, g.chat_id, g.id, true),
                false,
            ));
        }
        let views = (g.meta.channel_post && g.meta.views > 0)
            .then(|| format!("👁 {}", app.count(i64::from(g.meta.views))));
        if !g.reactions.is_empty() || views.is_some() {
            let mut text = model::reactions_text(&g.reactions, &app.custom_emoji);
            if let Some(v) = views {
                if !text.is_empty() {
                    text.push_str("  ");
                }
                text.push_str(&v);
            }
            body.push(Line::from(Span::styled(
                format::truncate(&text, inner),
                theme::reactions(),
            )));
        }
    }
    for g in group {
        match &g.state {
            SendState::Sent => {}
            SendState::Pending => {
                let up = g
                    .attachments
                    .iter()
                    .any(|a| app.media.file(&a.file).uploading);
                if ticks && !up {
                    continue;
                }
                let text = if up {
                    "Uploading… (Enter → c to cancel)"
                } else {
                    "Sending…"
                };
                body.push(Line::from(Span::styled(
                    format::truncate(text, inner),
                    theme::muted(),
                )))
            }
            SendState::Failed(e) => body.push(Line::from(Span::styled(
                format::truncate(&format!("failed: {e}"), inner),
                theme::error(),
            ))),
        }
    }
    texts
}

fn button_focus(app: &App, chat_id: i64, msg_id: i64, inline: bool) -> Option<usize> {
    match &app.popup {
        Some(Popup::Buttons(b))
            if b.chat_id == chat_id && b.msg_id == msg_id && b.keyboard.inline == inline =>
        {
            Some(b.sel)
        }
        _ => None,
    }
}

pub(crate) fn keyboard_block(
    kb: &crate::bots::Keyboard,
    width: usize,
    focus: Option<usize>,
    keys: bool,
) -> Vec<Line<'static>> {
    let th = theme::get();
    let mark = th.glyphs.url_button.clone();
    let key = |i: usize| {
        if keys {
            crate::bots::button_key(i).map_or_else(String::new, |k| format!("{k} "))
        } else {
            String::new()
        }
    };
    let rows = crate::bots::keyboard_layout(kb, width, |i, b| {
        key(i).width() + crate::bots::button_label(b, &mark).width()
    });
    let b = theme::border_set();
    let mut out = Vec::with_capacity(rows.len() * 3);
    for row in rows {
        let mut top: Vec<Span<'static>> = Vec::new();
        let mut mid: Vec<Span<'static>> = Vec::new();
        let mut bottom: Vec<Span<'static>> = Vec::new();
        let mut col = 0;
        for cell in row {
            let Some(button) = kb.button(cell.index) else {
                continue;
            };
            if cell.x > col {
                let gap = " ".repeat(cell.x - col);
                top.push(Span::raw(gap.clone()));
                mid.push(Span::raw(gap.clone()));
                bottom.push(Span::raw(gap));
            }
            let focused = focus == Some(cell.index);
            let (edge, text, hint) = if focused {
                let s = theme::button_selected();
                (s, s, s)
            } else {
                (
                    theme::button_border(),
                    theme::button_text(),
                    theme::hint_key(),
                )
            };
            let inner = cell.width.saturating_sub(2);
            let room = if inner >= 4 { inner - 2 } else { inner };
            let k = format::truncate(&key(cell.index), room);
            let label = format::truncate(
                &crate::bots::button_label(button, &mark),
                room.saturating_sub(k.width()),
            );
            let pad = inner.saturating_sub(k.width() + label.width());
            top.push(Span::styled(
                format!(
                    "{}{}{}",
                    b.top_left,
                    b.horizontal_top.repeat(inner),
                    b.top_right
                ),
                edge,
            ));
            mid.push(Span::styled(b.vertical_left.to_owned(), edge));
            mid.push(Span::styled(" ".repeat(pad / 2), text));
            if !k.is_empty() {
                mid.push(Span::styled(k, hint));
            }
            mid.push(Span::styled(label, text));
            mid.push(Span::styled(" ".repeat(pad - pad / 2), text));
            mid.push(Span::styled(b.vertical_right.to_owned(), edge));
            bottom.push(Span::styled(
                format!(
                    "{}{}{}",
                    b.bottom_left,
                    b.horizontal_bottom.repeat(inner),
                    b.bottom_right
                ),
                edge,
            ));
            col = cell.x + cell.width;
        }
        out.push(Line::from(top));
        out.push(Line::from(mid));
        out.push(Line::from(bottom));
    }
    out
}

fn select_row(line: Line<'static>, width: usize) -> Line<'static> {
    let mut spans = line.spans;
    if let Some(first) = spans.first_mut()
        && first.content.starts_with(' ')
    {
        *first = Span::styled(first.content[1..].to_owned(), first.style);
    }
    spans.insert(
        0,
        Span::styled(
            format::truncate(&theme::get().glyphs.selection_bar, 1),
            theme::selection_bar(),
        ),
    );
    let tint = theme::selection_bg();
    let mut line = with_bg(Line::from(spans), tint);
    let used = spans_width(&line.spans);
    if used < width {
        line.spans
            .push(Span::styled(" ".repeat(width - used), bg_style(tint)));
    }
    line
}

const MAX_ATTACHMENT_LINES: usize = 5;
const MAX_REPLY_KEYBOARD_LINES: usize = 9;

fn attachment_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let c = &app.composer;
    let n = c.attachments.len();
    let mut out = Vec::new();
    let start = match c.att_sel {
        Some(sel) if n > MAX_ATTACHMENT_LINES => sel
            .saturating_sub(MAX_ATTACHMENT_LINES - 1)
            .min(n - MAX_ATTACHMENT_LINES),
        _ => n.saturating_sub(MAX_ATTACHMENT_LINES),
    };
    if start > 0 {
        out.push(Line::from(Span::styled(
            format!("  … {start} more above"),
            theme::muted(),
        )));
    }
    for (i, a) in c
        .attachments
        .iter()
        .enumerate()
        .skip(start)
        .take(MAX_ATTACHMENT_LINES)
    {
        let kind = if a.as_file {
            format!("{} as file", a.kind.label())
        } else {
            a.send_kind().label().to_owned()
        };
        let label = format!(
            "{} {} · {} · {kind}",
            theme::get().glyphs.attachment,
            a.name,
            format::file_size(a.size as i64)
        );
        let style = if c.att_sel == Some(i) {
            theme::selection()
        } else {
            theme::warning()
        };
        out.push(Line::from(Span::styled(
            format::truncate(&label, width),
            style,
        )));
    }
    if c.pasting {
        out.push(Line::from(Span::styled(
            format!("{} Reading the clipboard…", theme::get().glyphs.clipboard),
            theme::muted(),
        )));
    }
    if let Some((id, kb)) = app.reply_keyboard() {
        let rows = keyboard_block(
            kb,
            width,
            app.open_chat
                .and_then(|chat| button_focus(app, chat, id, false)),
            false,
        );
        let shown = rows.len().min(MAX_REPLY_KEYBOARD_LINES);
        let hidden = (rows.len() - shown) / 3;
        let mut label = format!("{} <C-k>", theme::get().glyphs.keyboard);
        if !kb.placeholder.is_empty() {
            label = format!("{label} · {}", kb.placeholder);
        }
        if hidden > 0 {
            label = format!("{label} · +{hidden}");
        }
        out.push(Line::from(Span::styled(
            format::truncate(&label, width),
            theme::muted(),
        )));
        out.extend(rows.into_iter().take(shown));
    }
    out
}

fn composer_height(app: &App, width: usize) -> u16 {
    if app.open_chat.is_none() {
        return 0;
    }
    if app.recording.is_some() {
        return 3;
    }
    let text_w = width.saturating_sub(2 + 2 * composer_pad() as usize);
    let lines = app.composer.input.visual_lines(text_w).len().max(1);
    let extra = attachment_lines(app, text_w).len();
    (lines.min(8) + 2 + extra) as u16
}

fn draw_messages(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Messages;
    let title = app.view_title();
    let title = match app.open_chat.and_then(|id| app.chat_subtitle(id)) {
        Some(sub) => format!("{title} · {sub}"),
        None => title,
    };
    let block = pane_block(
        format!(
            " {} ",
            format::truncate(&title, area.width.saturating_sub(4) as usize)
        ),
        focused,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    textmap::area(Zone::Pane(Pane::Messages), area);
    let ch = composer_height(app, inner.width as usize);
    let [list, composer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(ch)]).areas(inner);
    app.msg_view_height = list.height as usize;
    draw_message_list(frame, list, app);
    if ch > 0 {
        textmap::hit(composer, Hit::Composer);
        draw_composer(frame, composer, app);
        draw_command_suggestions(frame, list, app);
    }
}

fn draw_command_suggestions(frame: &mut Frame, area: Rect, app: &App) {
    let mentions = app.mention_suggestions();
    let list: Vec<(String, String)> = if mentions.is_empty() {
        app.command_suggestions()
            .into_iter()
            .map(|c| (format!("/{} ", c.name), c.description.clone()))
            .collect()
    } else {
        mentions
            .into_iter()
            .map(|(username, name)| (format!("@{username} "), name))
            .collect()
    };
    let title = if list.first().is_some_and(|(k, _)| k.starts_with('@')) {
        " Mention "
    } else {
        " Commands "
    };
    if list.is_empty() || area.height < 3 {
        return;
    }
    let rows = list.len().min(8).min(area.height as usize - 2);
    let sel = app.cmd_sel.min(list.len() - 1);
    let start = list_window(sel, list.len(), rows);
    let width = area.width.min(64);
    let inner_w = width.saturating_sub(2) as usize;
    let lines: Vec<Line<'static>> = list
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, (key, desc))| {
            row_line(
                vec![
                    Span::styled(key.clone(), theme::active_field()),
                    Span::styled(format::sanitize(desc), theme::muted()),
                ],
                i == sel,
                true,
                inner_w,
            )
        })
        .collect();
    let height = rows as u16 + 2;
    let rect = Rect {
        x: area.x,
        y: area.y + area.height - height,
        width,
        height,
    };
    textmap::cover(rect);
    frame.render_widget(Clear, rect);
    let block = theme::popup_block()
        .title(Span::styled(title, theme::popup_title()))
        .title_bottom(Span::styled(" Tab complete · ↑↓ select ", theme::muted()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

const RUN_GAP_SECS: i64 = 600;
const MAX_CACHED_BLOCKS: usize = 4096;

#[derive(Default)]
pub struct LayoutCache {
    map: HashMap<(i64, i64, bool), (u64, Rendered)>,
}

impl LayoutCache {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

#[derive(Clone, Copy)]
struct Ctx {
    width: usize,
    layout: MessageLayout,
    theme: u64,
    now: i64,
    today: i64,
    lang: format::Lang,
    sel: Option<(usize, usize)>,
}

fn bg_style(bg: Option<Color>) -> Style {
    Style {
        bg,
        ..Style::default()
    }
}

fn same_run(a: &Msg, b: &Msg) -> bool {
    a.sender == b.sender
        && a.outgoing == b.outgoing
        && format::same_day(a.date, b.date)
        && (b.date - a.date).abs() <= RUN_GAP_SECS
}

pub(crate) fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

fn origin_name(app: &App, origin: &model::Origin) -> String {
    match origin {
        model::Origin::Sender(s) => app.sender_name(*s),
        model::Origin::Name(n) => n.clone(),
    }
}

fn reply_parts(app: &App, m: &Msg) -> Option<(Option<Sender>, String, String)> {
    let reply_id = m.reply_to?;
    Some(
        match app.histories.get(&m.chat_id).and_then(|h| h.get(reply_id)) {
            Some(r) => {
                let text = if r.body.text.is_empty() {
                    r.media.clone().unwrap_or_default()
                } else {
                    format::one_line(&r.body.text)
                };
                (Some(r.sender), app.sender_name(r.sender), text)
            }
            None => (None, String::new(), String::new()),
        },
    )
}

fn fingerprint(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> u64 {
    let mut h = DefaultHasher::new();
    (
        ctx.width, ctx.layout, ctx.theme, ctx.today, ctx.lang, selected,
    )
        .hash(&mut h);
    app.media.font().hash(&mut h);
    msgs[range.clone()].hash(&mut h);
    if ctx.layout == MessageLayout::Cards {
        let run = layouts::card_run(msgs, &range);
        msgs[run.clone()].hash(&mut h);
        for m in &msgs[run] {
            app.sender_name(m.sender).hash(&mut h);
        }
    }
    let edge = |m: Option<&Msg>| m.map(|m| (m.sender, m.date, m.outgoing));
    edge(range.start.checked_sub(1).map(|j| &msgs[j])).hash(&mut h);
    edge(msgs.get(range.end)).hash(&mut h);
    ctx.sel
        .map(|(s, e)| (s == range.end, e == range.start))
        .hash(&mut h);
    let chat = msgs
        .get(range.start)
        .and_then(|m| app.chats.get(&m.chat_id));
    chat.map(|c| (c.kind.clone(), app.read_outbox(c.id)))
        .hash(&mut h);
    app.playing.as_ref().map(|p| p.target).hash(&mut h);
    for m in &msgs[range] {
        app.sender_name(m.sender).hash(&mut h);
        reply_parts(app, m).hash(&mut h);
        m.forward.as_ref().map(|o| origin_name(app, o)).hash(&mut h);
        for a in &m.attachments {
            app.media.file(&a.file).hash(&mut h);
            app.media.file_error(a.file.id).hash(&mut h);
        }
        button_focus(app, m.chat_id, m.id, true).hash(&mut h);
        for r in m.reactions.iter().filter(|r| r.custom != 0) {
            app.custom_emoji.get(&r.custom).hash(&mut h);
        }
    }
    h.finish()
}

pub const DELETED_MARK: &str = "🗑 deleted";
pub const BLOCKED_PLACEHOLDER: &str = "⊘ message from a blocked user · Enter to show";

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub fn mark_deleted(m: &mut Msg) {
    let prefix = format!("{DELETED_MARK} · ");
    let shift = utf16_len(&prefix);
    if m.body.text.is_empty() {
        let label = m.media.take().unwrap_or_default();
        m.media = Some(if label.is_empty() {
            DELETED_MARK.to_owned()
        } else {
            format!("{prefix}{label}")
        });
        return;
    }
    let len = utf16_len(&m.body.text);
    for e in &mut m.body.entities {
        e.offset += shift;
    }
    m.body.entities.push(model::Entity {
        offset: shift,
        length: len,
        kind: model::EntityKind::Strike,
    });
    m.body.text = format!("{prefix}{}", m.body.text);
}

pub fn hide_blocked(m: &mut Msg) {
    m.body = model::RichText::plain(BLOCKED_PLACEHOLDER);
    m.media = None;
    m.attachments.clear();
    m.reactions.clear();
    m.keyboard = None;
    m.forward = None;
    m.reply_to = None;
}

pub fn display_msgs<'a>(app: &App, msgs: &'a [Msg]) -> std::borrow::Cow<'a, [Msg]> {
    let touched = |m: &Msg| m.deleted || app.hidden_blocked(m);
    if !msgs.iter().any(touched) {
        return std::borrow::Cow::Borrowed(msgs);
    }
    let mut out = msgs.to_vec();
    for m in &mut out {
        if app.hidden_blocked(m) {
            hide_blocked(m);
        } else if m.deleted {
            mark_deleted(m);
        }
    }
    std::borrow::Cow::Owned(out)
}

fn cached_block(
    app: &App,
    cache: &mut LayoutCache,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let first = &msgs[range.start];
    let key = (first.chat_id, first.id, selected);
    let print = fingerprint(app, msgs, range.clone(), ctx, selected);
    if let Some((p, r)) = cache.map.get(&key)
        && *p == print
    {
        return r.clone();
    }
    let r = match ctx.layout {
        MessageLayout::Classic => group_block(app, msgs, range, ctx, selected),
        MessageLayout::Bubbles => bubble_block(app, msgs, range, ctx, selected),
        MessageLayout::Cards => layouts::cards_block(app, msgs, range, ctx, selected),
        MessageLayout::Compact => layouts::compact_block(app, msgs, range, ctx, selected),
        MessageLayout::Minimal => layouts::minimal_block(app, msgs, range, ctx, selected),
        MessageLayout::Split => layouts::split_block(app, msgs, range, ctx, selected),
    };
    if cache.map.len() >= MAX_CACHED_BLOCKS {
        cache.map.clear();
    }
    cache.map.insert(key, (print, r.clone()));
    r
}

fn with_bg(mut line: Line<'static>, bg: Option<Color>) -> Line<'static> {
    for span in &mut line.spans {
        if span.style.bg.is_none() {
            span.style.bg = bg;
        }
    }
    line
}

fn bubble_footer(
    app: &App,
    last: &Msg,
    edited: bool,
    pinned: bool,
    lang: format::Lang,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if pinned {
        spans.push(pin_span());
        spans.push(Span::raw(" "));
    }
    if edited {
        spans.push(edited_span(lang));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(format::time_label(last.date), theme::time()));
    if let Some(tick) = tick_span(app, last) {
        spans.push(Span::raw(" "));
        spans.push(tick);
    }
    spans
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| format::cells(&s.content)).sum()
}

fn bubble_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let m = &msgs[range.start];
    let group = &msgs[range.clone()];
    let last = &msgs[range.end - 1];
    let prev = range.start.checked_sub(1).map(|j| &msgs[j]);
    let next = msgs.get(range.end);
    let new_day = prev.is_none_or(|p| !format::same_day(p.date, m.date));
    let first_of_run = new_day || prev.is_none_or(|p| !same_run(p, m));
    let last_of_run = next.is_none_or(|n| !same_run(last, n));
    let group_chat = app
        .chats
        .get(&m.chat_id)
        .is_some_and(|c| matches!(c.kind, ChatKind::Group | ChatKind::Supergroup));
    let outgoing = m.outgoing;
    let th = theme::get();
    let show_names = group_chat && !outgoing;
    let with_avatar = show_names && th.layout.avatars;
    let avatar_rows = th.layout.bubble_avatar_rows as usize;
    let avatar_cols = avatar_rows * 2;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if new_day {
        if prev.is_some() {
            lines.push(Line::default());
        }
        let sep = day_separator(ctx, m.date);
        if !sep.is_empty() {
            lines.extend(sep);
            lines.push(Line::default());
        }
    } else if first_of_run {
        lines.push(Line::default());
    }
    let gutter = if with_avatar { avatar_cols + 1 } else { 1 };
    let room = width.saturating_sub(gutter + 1).max(4);
    let outer_max = (width * th.layout.bubble_max_width as usize / 100)
        .max(18)
        .min(room);
    let inner_max = outer_max.saturating_sub(2).max(2);
    let mut body: Vec<Line<'static>> = Vec::new();
    if show_names && first_of_run {
        body.push(Line::from(Span::styled(
            format::truncate(&app.sender_name(m.sender), inner_max),
            author_style(app, m.sender),
        )));
    }
    if let Some(origin) = &m.forward {
        body.push(Line::from(Span::styled(
            format::truncate(&forward_text(app, origin, ctx.lang), inner_max),
            theme::forwarded(),
        )));
    }
    if let Some((sender, name, text)) = reply_parts(app, m) {
        let bar_style = theme::reply_bar(sender.map(sender_id));
        let bar = Span::styled(th.glyphs.bubble_reply_bar.clone(), bar_style);
        let bar_w = bar.content.width();
        let name = if name.is_empty() {
            match ctx.lang {
                format::Lang::Ru => "ответ".to_owned(),
                format::Lang::En => "reply".to_owned(),
            }
        } else {
            name
        };
        body.push(Line::from(vec![
            bar.clone(),
            Span::styled(
                format::truncate(&name, inner_max.saturating_sub(bar_w)),
                theme::strong().patch(bar_style),
            ),
        ]));
        if !text.is_empty() {
            body.push(Line::from(vec![
                bar,
                Span::styled(
                    format::truncate(&text, inner_max.saturating_sub(bar_w)),
                    theme::reply_text(),
                ),
            ]));
        }
    }
    let mut images = Vec::new();
    let before = body.len();
    let mut texts = group_content(
        app,
        msgs,
        range.clone(),
        inner_max,
        true,
        &mut body,
        &mut images,
    );
    let media_end = images
        .iter()
        .map(|p| p.line + p.rows as usize)
        .max()
        .unwrap_or(0);
    let last_is_text = body.len() > before.max(media_end);
    let edited = group.iter().any(|g| g.edited);
    let pinned = group.iter().any(|g| g.pinned);
    let footer = bubble_footer(app, last, edited, pinned, ctx.lang);
    let footer_w = spans_width(&footer);
    let image_w = images
        .iter()
        .map(|p| (p.col + p.cols) as usize)
        .max()
        .unwrap_or(0);
    let content_w = body
        .iter()
        .map(|l| spans_width(&l.spans))
        .max()
        .unwrap_or(0)
        .max(image_w);
    let fits_last = last_is_text
        && body
            .last()
            .is_some_and(|l| spans_width(&l.spans) + 1 + footer_w <= inner_max);
    let inner_w = if fits_last {
        content_w.max(body.last().map_or(0, |l| spans_width(&l.spans)) + 1 + footer_w)
    } else {
        content_w.max(footer_w)
    }
    .min(inner_max);
    if fits_last {
        if let Some(l) = body.last_mut() {
            let pad = inner_w.saturating_sub(spans_width(&l.spans) + footer_w);
            l.spans.push(Span::raw(" ".repeat(pad)));
            l.spans.extend(footer);
        }
    } else {
        let mut spans = vec![Span::raw(" ".repeat(inner_w.saturating_sub(footer_w)))];
        spans.extend(footer);
        body.push(Line::from(spans));
    }
    let bg = theme::bubble_bg(outgoing, selected);
    let outer_w = inner_w + 2;
    let x = if outgoing {
        width.saturating_sub(outer_w + 1)
    } else {
        gutter
    };
    let show_avatar = with_avatar && last_of_run;
    let lift = if show_avatar {
        avatar_rows.saturating_sub(body.len())
    } else {
        0
    };
    lines.extend(std::iter::repeat_n(Line::default(), lift));
    let top = lines.len();
    let edge = |left: bool| {
        let g = if left {
            &th.glyphs.selected_left
        } else {
            &th.glyphs.selected_right
        };
        if selected {
            Span::styled(
                format::truncate(g, 1),
                theme::bubble_edge().patch(bg_style(bg)),
            )
        } else {
            Span::styled(" ", bg_style(bg))
        }
    };
    for l in body {
        let used = spans_width(&l.spans);
        let mut spans = vec![Span::raw(" ".repeat(x)), edge(true)];
        let content = if used > inner_w {
            Line::from(Span::raw(format::truncate(
                &l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>(),
                inner_w,
            )))
        } else {
            l
        };
        let used = spans_width(&content.spans);
        spans.extend(with_bg(content, bg).spans);
        spans.push(Span::styled(
            " ".repeat(inner_w.saturating_sub(used)),
            bg_style(bg),
        ));
        spans.push(edge(false));
        lines.push(Line::from(spans));
    }
    for p in &mut images {
        p.line += top;
        p.col += (x + 1) as u16;
    }
    for t in &mut texts {
        t.width = t.width.min((inner_w as u16).saturating_sub(t.col));
        t.line += top;
        t.col += (x + 1) as u16;
    }
    if show_avatar {
        let first_row = lines.len() - avatar_rows;
        let label = initials(&app.sender_name(m.sender));
        let cols = avatar_cols;
        for (i, line) in lines[first_row..].iter_mut().enumerate() {
            let text = if i == 0 {
                let pad = cols.saturating_sub(label.width());
                format!(
                    "{}{label}{}",
                    " ".repeat(pad / 2),
                    " ".repeat(pad - pad / 2)
                )
            } else {
                " ".repeat(cols)
            };
            let cell = Span::styled(text, theme::avatar(sender_id(m.sender)));
            match line.spans.first_mut() {
                Some(first) if first.content.width() >= cols => {
                    let rest = first.content.width() - cols;
                    *first = Span::raw(" ".repeat(rest));
                    line.spans.insert(0, cell);
                }
                _ => line.spans.insert(0, cell),
            }
        }
        images.push(Place {
            line: first_row,
            col: 0,
            cols: avatar_cols as u16,
            rows: avatar_rows as u16,
            pic: Pic::Avatar(m.sender),
        });
    }
    Rendered {
        lines,
        images,
        texts,
    }
}

fn avatar_cells(t: ImageTarget) -> impl Iterator<Item = (u16, u16, u16, u16)> {
    (0..t.rows).flat_map(move |cy| {
        (0..t.cols).filter_map(move |cx| {
            let y = t.y + cy as isize;
            let x = t.x + cx;
            (y >= 0 && y < t.area.height as isize && x < t.area.width)
                .then(|| (cx, cy, t.area.x + x, t.area.y + y as u16))
        })
    })
}

fn mask_halfblock_cell(
    cell: &mut ratatui::buffer::Cell,
    upper_in: bool,
    lower_in: bool,
    behind: Color,
) {
    let (fg, bg) = (cell.fg, cell.bg);
    let (upper, lower) = match cell.symbol() {
        "▀" => (fg, bg),
        "▄" => (bg, fg),
        _ => (bg, bg),
    };
    let (symbol, fg, bg) = match (upper_in, lower_in) {
        (true, true) => return,
        (false, false) => (" ", behind, behind),
        (true, false) => ("▀", upper, behind),
        (false, true) => ("▄", lower, behind),
    };
    cell.set_symbol(symbol);
    cell.set_fg(fg);
    cell.set_bg(bg);
}

fn draw_avatar(
    frame: &mut Frame,
    t: ImageTarget,
    media: &mut Media,
    photo: &model::FileRef,
    overlay: bool,
    behind: Option<Color>,
) {
    let f = media.file(photo);
    let Some(input) = media::input_for(&f, Decode::Raster) else {
        media.want_download(f.id, 1);
        return;
    };
    let shape = theme::get().layout.avatar_shape;
    let key = Key {
        src: Src::File(f.id),
        cols: t.cols,
        rows: t.rows,
        shape: Some(shape),
        animate: false,
    };
    let font = media.font();
    let Look::Ready(frames) = media.look(key, input, overlay) else {
        return;
    };
    let Some(proto) = frames.at(Duration::ZERO) else {
        return;
    };
    let behind = theme::cell_bg(behind);
    let buf = frame.buffer_mut();
    for (_, _, x, y) in avatar_cells(t) {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.reset();
            cell.set_bg(behind);
        }
    }
    render_proto(frame, t, proto);
    if matches!(proto, SlicedProtocol::Halfblocks(_)) {
        mask_avatar(frame, t, font, shape, behind);
    }
}

fn mask_avatar(
    frame: &mut Frame,
    t: ImageTarget,
    font: (u16, u16),
    shape: theme::AvatarShape,
    behind: Color,
) {
    let (fw, fh) = (f32::from(font.0.max(1)), f32::from(font.1.max(1)));
    let (w, h) = (f32::from(t.cols) * fw, f32::from(t.rows) * fh);
    let buf = frame.buffer_mut();
    for (cx, cy, x, y) in avatar_cells(t) {
        let px = (f32::from(cx) + 0.5) * fw;
        let top = f32::from(cy) * fh;
        let upper = media::inside_shape(shape, w, h, px, top + fh * 0.25);
        let lower = media::inside_shape(shape, w, h, px, top + fh * 0.75);
        if let Some(cell) = buf.cell_mut((x, y)) {
            mask_halfblock_cell(cell, upper, lower, behind);
        }
    }
}

fn draw_profile_avatar(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(Popup::Profile(p)) = &app.popup else {
        return;
    };
    if p.data.is_none() || p.error.is_some() || p.scroll > 0 {
        return;
    }
    let Some(photo) = app.profile_photo(p.target) else {
        return;
    };
    let rect = crate::popups::profile_rect(area);
    let inner = Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    };
    let (cols, rows) = crate::popups::PROFILE_AVATAR;
    if inner.width <= cols + 1 || inner.height < rows {
        return;
    }
    let t = ImageTarget {
        area: inner,
        x: 1,
        y: 0,
        cols,
        rows,
    };
    textmap::cover(Rect::new(inner.x + 1, inner.y, cols, rows));
    draw_avatar(
        frame,
        t,
        &mut app.media,
        &photo,
        false,
        theme::popup_surface().bg,
    );
}

fn pinned_bar(app: &App, chat_id: i64, width: usize) -> Option<Line<'static>> {
    let m = app.pinned_msg.get(&chat_id)?;
    let label = match app.lang {
        format::Lang::Ru => "Закреплённое",
        format::Lang::En => "Pinned",
    };
    let key = app
        .keymap
        .label_for(crate::keymap::Action::JumpToPinned)
        .map(|k| format!("  {k}"))
        .unwrap_or_default();
    let head = format!("{} {label}: ", theme::get().glyphs.pinned);
    let room = width.saturating_sub(head.width() + key.width() + 1);
    Some(Line::from(vec![
        Span::styled(head, theme::pinned_bar()),
        Span::raw(format::truncate(&model::preview(m), room)),
        Span::styled(key, theme::muted()),
    ]))
}

pub fn lines_from(app: &mut App, chat_id: i64, from: usize) -> usize {
    let mut cache = std::mem::take(&mut app.layout_cache);
    let total = match app.histories.get(&chat_id) {
        Some(h) if from < h.len() => {
            let shown = display_msgs(app, h.msgs());
            let msgs: &[Msg] = &shown;
            let groups = model::groups(msgs);
            let now = chrono::Local::now();
            let ctx = Ctx {
                width: app.msg_view_width.max(1),
                layout: app.message_layout(),
                theme: app.theme_key(),
                now: now.timestamp(),
                today: i64::from(chrono::Datelike::num_days_from_ce(&now.date_naive())),
                lang: app.lang,
                sel: None,
            };
            let first = model::group_of(&groups, from);
            (first..groups.len())
                .map(|g| {
                    cached_block(app, &mut cache, msgs, groups[g].clone(), ctx, false)
                        .lines
                        .len()
                })
                .sum()
        }
        _ => 0,
    };
    app.layout_cache = cache;
    total
}

fn draw_message_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(chat_id) = app.open_chat else {
        let hint = Line::from(vec![
            Span::styled("  Select a chat and press ", theme::muted()),
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
    let mut area = area;
    if let Some(bar) = pinned_bar(app, chat_id, area.width as usize).filter(|_| area.height > 3) {
        let [top, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(bar), top);
        area = rest;
        app.msg_view_height = area.height as usize;
    }
    let full = area;
    if theme::get().layout.scrollbar && area.width > 2 {
        area.width -= 1;
    }
    let height = area.height as usize;
    let width = area.width as usize;
    app.msg_view_width = width;
    if height == 0 || width == 0 {
        return;
    }
    let Some(h) = app.histories.get(&chat_id) else {
        return;
    };
    if let Some(error) = h.error.as_ref().filter(|_| h.is_empty()) {
        let lines = vec![
            Line::from(Span::styled(
                crate::td::failed("load messages", &format::one_line(error)),
                theme::error(),
            )),
            Line::from(vec![
                Span::styled("Enter", theme::strong()),
                Span::styled(" retry", theme::muted()),
            ]),
        ];
        let wrap = ratatui::widgets::Wrap { trim: true };
        let pad = Block::default().padding(Padding::horizontal(2));
        frame.render_widget(Paragraph::new(lines).wrap(wrap).block(pad), area);
        return;
    }
    if h.is_empty() {
        let text = if h.loading_older {
            "  Loading…"
        } else {
            "  No messages yet"
        };
        frame.render_widget(Paragraph::new(Span::styled(text, theme::muted())), area);
        return;
    }
    let mut cache = std::mem::take(&mut app.layout_cache);
    let Some(h) = app.histories.get(&chat_id) else {
        return;
    };
    let shown_msgs = display_msgs(app, h.msgs());
    let msgs: &[Msg] = &shown_msgs;
    let reached_start = h.reached_start;
    let loading = h.loading_older;
    let indicator = h.older_indicator(std::time::Instant::now());
    if loading && !indicator && !reached_start {
        app.animating = true;
    }
    let groups = model::groups(msgs);
    let now = chrono::Local::now();
    let sel_group = app
        .msg_sel
        .map(|s| model::group_of(&groups, s.min(msgs.len() - 1)));
    let ctx = Ctx {
        width,
        layout: app.message_layout(),
        theme: app.theme_key(),
        now: now.timestamp(),
        today: i64::from(chrono::Datelike::num_days_from_ce(&now.date_naive())),
        lang: app.lang,
        sel: sel_group.map(|g| (groups[g].start, groups[g].end)),
    };
    let mut block = |app: &App, g: usize, sel: Option<usize>| {
        cached_block(
            app,
            &mut cache,
            msgs,
            groups[g].clone(),
            ctx,
            sel == Some(g),
        )
    };
    let absolute = |p: Place, g: usize| match p.pic {
        Pic::Att { rel, att } => Place {
            pic: Pic::Att {
                rel: groups[g].start + rel,
                att,
            },
            ..p
        },
        Pic::Avatar(_) => p,
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut placed: Vec<(usize, Place)> = Vec::new();
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut want_older = false;
    let mut blocks: Vec<(usize, Rendered)> = Vec::new();
    let mut total = 0;
    let mut g = groups.len();
    while g > 0 && (sel_group.is_some_and(|s| g > s) || total < height + app.bottom_offset) {
        g -= 1;
        let b = block(app, g, sel_group);
        total += b.lines.len();
        blocks.push((g, b));
    }
    let unknown: Vec<i64> = blocks
        .iter()
        .flat_map(|(g, _)| &msgs[groups[*g].clone()])
        .flat_map(|m| &m.reactions)
        .map(|r| r.custom)
        .filter(|id| *id != 0 && !app.custom_emoji.contains_key(id))
        .collect();
    app.want_emoji.extend(unknown);
    let mut all: Vec<Line<'static>> = Vec::with_capacity(total);
    let mut sel_span = None;
    let mut texts: Vec<(usize, TextPlace)> = Vec::new();
    for (g, b) in blocks.into_iter().rev() {
        let base = all.len();
        starts.push((base, g));
        placed.extend(
            b.images
                .into_iter()
                .map(|p| (base + p.line, absolute(p, g))),
        );
        texts.extend(b.texts.into_iter().map(|t| (base + t.line, t)));
        if sel_group == Some(g) {
            sel_span = Some((base, base + b.lines.len()));
        }
        all.extend(b.lines);
    }
    let max_offset = all.len().saturating_sub(height);
    let mut end = all.len() - app.bottom_offset.min(max_offset);
    if let Some((s0, s1)) = sel_span {
        if s1 > end {
            end = s1;
        }
        if s0 + height < end {
            end = s0 + height;
        }
        end = end.clamp(height.min(all.len()), all.len());
    }
    app.bottom_offset = all.len() - end;
    let at_bottom = app.bottom_offset == 0;
    let start = end.saturating_sub(height);
    if start == 0 && !reached_start {
        want_older = true;
    }
    let first_line = start;
    if let Some(&(_, g)) = starts.iter().rev().find(|(line, _)| *line <= start) {
        app.msg_top = groups[g].start;
    }
    app.visible_msgs = starts
        .iter()
        .enumerate()
        .filter(|&(i, &(top, _))| {
            let bottom = starts.get(i + 1).map_or(all.len(), |n| n.0);
            (bottom > start && bottom <= end) || (top <= start && bottom >= end)
        })
        .flat_map(|(_, &(_, g))| msgs[groups[g].clone()].iter().map(|m| m.id))
        .collect();
    all.truncate(end);
    lines.extend(all.drain(start..));
    app.layout_cache = cache;
    let covered_top = indicator && lines.len() >= height;
    if covered_top {
        lines[0] = Line::from(Span::styled(
            format::separator(" Loading older… ", width),
            theme::date_separator(),
        ));
    }
    let pad = height.saturating_sub(lines.len());
    let mut shown: Vec<Line<'static>> = vec![Line::from(""); pad];
    shown.extend(lines);
    frame.render_widget(Paragraph::new(shown), area);
    let screen_y = |line: usize| area.y + (pad + line - first_line) as u16;
    let text_from = first_line + usize::from(covered_top);
    for (line, t) in texts.iter().filter(|(l, _)| (text_from..end).contains(l)) {
        textmap::row(area.x + t.col, screen_y(*line), t.width, t.join);
    }
    for (i, &(top, g)) in starts.iter().enumerate() {
        let top = top.max(first_line);
        let bottom = starts.get(i + 1).map_or(end, |n| n.0).min(end);
        if top < bottom {
            let rows = Rect::new(area.x, screen_y(top), area.width, (bottom - top) as u16);
            textmap::hit(rows, Hit::Message(msgs[groups[g].start].id));
        }
    }
    if !app.marked.is_empty() && area.width > 0 {
        let buf = frame.buffer_mut();
        for (i, &(top, g)) in starts.iter().enumerate() {
            if !msgs[groups[g].clone()]
                .iter()
                .any(|m| app.marked.contains(&m.id))
            {
                continue;
            }
            let bottom = starts.get(i + 1).map_or(end, |n| n.0).min(end);
            for line in top.max(first_line)..bottom {
                let y = area.y + (pad + line - first_line) as u16;
                if y < area.bottom() {
                    buf[(area.x, y)]
                        .set_symbol("▌")
                        .set_style(theme::active_field());
                }
            }
        }
    }
    if full.width > area.width {
        let view_end = first_line + height;
        let top_group = starts
            .iter()
            .rev()
            .find(|(line, _)| *line <= first_line)
            .or(starts.first())
            .map_or(0, |(_, g)| *g);
        let visible = starts
            .iter()
            .filter(|(line, _)| *line >= first_line && *line < view_end)
            .count()
            .max(1);
        draw_scrollbar(frame, full, groups.len(), top_group, visible);
    }
    let overlay = app.popup.is_some() || !app.pending.is_empty();
    let sel = sel_group.map(|g| groups[g].clone());
    let animate = pick_animations(app, msgs, &placed, sel, |abs, rows| {
        let y = pad as isize + abs as isize - first_line as isize;
        y + rows as isize > 0 && y < height as isize
    });
    let still = app.media.anim.paused() || (overlay && !app.media.anim.native());
    let now = app.chat_freeze.at(app.anim_start.elapsed(), still);
    let mut head = crate::anim::Playhead::new(now, animate.len(), still);
    for (n, (abs, p)) in placed.into_iter().enumerate() {
        let y = pad as isize + abs as isize - first_line as isize;
        if y + p.rows as isize <= 0 || y >= height as isize {
            continue;
        }
        let target = ImageTarget {
            area,
            x: p.col,
            y,
            cols: p.cols,
            rows: p.rows,
        };
        match p.pic {
            Pic::Att { rel, att } => {
                let m = &msgs[rel];
                let Some(a) = m.attachments.get(att) else {
                    continue;
                };
                if animate.contains(&n)
                    && draw_animation(frame, target, &mut app.media, a, &mut head)
                {
                    continue;
                }
                draw_attachment(
                    frame,
                    target,
                    &mut app.media,
                    a,
                    (chat_id, m.id, att),
                    overlay,
                    false,
                    None,
                );
            }
            Pic::Avatar(sender) => {
                if y < 0
                    || y + p.rows as isize > height as isize
                    || p.rows < MIN_AVATAR_ROWS
                    || p.cols < 2 * MIN_AVATAR_ROWS
                {
                    continue;
                }
                if let Some(photo) = app.sender_photo(sender) {
                    draw_avatar(
                        frame,
                        target,
                        &mut app.media,
                        &photo,
                        overlay,
                        theme::surface().bg,
                    );
                }
            }
        }
    }
    app.redraw_due = min_due(app.redraw_due, head.due);
    if !at_bottom || app.window_detached() {
        draw_jump_indicator(frame, area, app, chat_id);
    }
    if want_older {
        app.want_older = true;
    }
}

fn min_due(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    a.into_iter().chain(b).min()
}

fn pick_animations(
    app: &App,
    msgs: &[Msg],
    placed: &[(usize, Place)],
    sel: Option<std::ops::Range<usize>>,
    visible: impl Fn(usize, u16) -> bool,
) -> std::collections::HashSet<usize> {
    let mode = app.settings.media.animations;
    if !app.media.anim.enabled() || mode == crate::config::Animations::Off {
        return std::collections::HashSet::new();
    }
    let mut picks: Vec<(bool, usize)> = placed
        .iter()
        .enumerate()
        .filter_map(|(n, (abs, p))| {
            let Pic::Att { rel, att } = p.pic else {
                return None;
            };
            if !visible(*abs, p.rows) {
                return None;
            }
            crate::anim::source(msgs.get(rel)?.attachments.get(att)?)?;
            let selected = sel.as_ref().is_some_and(|r| r.contains(&rel));
            (mode == crate::config::Animations::All || selected).then_some((selected, n))
        })
        .collect();
    picks.sort_by_key(|&(selected, n)| (!selected, std::cmp::Reverse(n)));
    let cap = if app.media.anim.native() {
        app.settings.media.max_animations()
    } else {
        app.settings
            .media
            .max_animations()
            .min(crate::anim::REDRAW_MAX_VISIBLE)
    };
    picks.into_iter().take(cap).map(|(_, n)| n).collect()
}

fn draw_animation(
    frame: &mut Frame,
    t: ImageTarget,
    media: &mut Media,
    att: &Attachment,
    head: &mut crate::anim::Playhead,
) -> bool {
    let Some((file, decode, seconds)) = crate::anim::source(att) else {
        return false;
    };
    let f = media.file(&file);
    let Some(path) = f.local().map(str::to_owned) else {
        media.want_download(f.id, 1);
        return false;
    };
    let (fw, fh) = media.font();
    let key = crate::anim::Key {
        file: f.id,
        cols: t.cols,
        rows: t.rows,
    };
    let job = || crate::anim::Job {
        path,
        decode,
        seconds,
        box_px: (
            u32::from(t.cols) * u32::from(fw),
            u32::from(t.rows) * u32::from(fh),
        ),
    };
    if !media.anim.native() {
        return draw_redrawn(frame, t, media, key, job, head);
    }
    let Some(id) = media.anim.show(key, job) else {
        return false;
    };
    crate::anim::placeholders(frame.buffer_mut(), t.area, id, (t.x, t.y), (t.cols, t.rows));
    true
}

fn draw_redrawn(
    frame: &mut Frame,
    t: ImageTarget,
    media: &mut Media,
    key: crate::anim::Key,
    job: impl FnOnce() -> crate::anim::Job,
    head: &mut crate::anim::Playhead,
) -> bool {
    let Some(clip) = media.anim.clip(key, job) else {
        return false;
    };
    let mkey = Key {
        src: Src::Clip(key.file),
        cols: key.cols,
        rows: key.rows,
        shape: None,
        animate: true,
    };
    let error = match media.look(mkey, Input::Clip(clip.clone()), head.paused) {
        Look::Ready(frames) => {
            let Some(proto) = frames.at(head.frame(frames.cost)) else {
                return false;
            };
            render_proto(frame, t, proto);
            return true;
        }
        Look::Failed(e) => e.to_owned(),
        Look::Pending | Look::Hidden => return false,
    };
    if clip.exists() {
        media.anim.report(format!("inline animation: {error}"));
    } else {
        media.forget(&mkey);
        media.anim.forget(key);
    }
    false
}

fn draw_scrollbar(frame: &mut Frame, area: Rect, total: usize, top: usize, visible: usize) {
    let total = total.max(1);
    let mut state = ScrollbarState::new(total.saturating_sub(visible).max(1))
        .position(top.min(total))
        .viewport_content_length(visible);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_style(theme::scrollbar())
        .thumb_style(theme::scrollbar_thumb());
    frame.render_stateful_widget(bar, area, &mut state);
}

fn jump_label(app: &App, chat_id: i64) -> String {
    let unread = app.chats.get(&chat_id).map_or(0, |c| c.unread);
    let key = app
        .keymap
        .label_for(crate::keymap::Action::JumpBottom)
        .unwrap_or_default();
    let arrow = theme::get().glyphs.jump.clone();
    match (unread, app.lang) {
        (0, _) => format!(" {arrow} {key} "),
        (n, format::Lang::Ru) => format!(" {arrow} {n} новых · {key} "),
        (n, format::Lang::En) => format!(" {arrow} {n} new · {key} "),
    }
}

fn unread_badges(app: &App, chat_id: i64) -> Vec<String> {
    let topic = app.open_topic.filter(|_| app.is_forum(chat_id));
    let (mentions, reactions) = app.unread_counts(chat_id, topic);
    let glyphs = &theme::get().glyphs;
    let heart = format::emoji_cell(&glyphs.reaction_badge)
        .unwrap_or_else(|| format::sanitize(&glyphs.reaction_badge));
    let at = format::sanitize(&glyphs.mention_dot);
    let key = |a| app.keymap.label_for(a).unwrap_or_default();
    let mut out = Vec::new();
    for (n, glyph, action) in [
        (reactions, heart, crate::keymap::Action::JumpToNextReaction),
        (mentions, at, crate::keymap::Action::JumpToNextMention),
    ] {
        if n <= 0 {
            continue;
        }
        let parts = [glyph, app.count(i64::from(n)), key(action)];
        let text: Vec<&str> = parts
            .iter()
            .map(String::as_str)
            .filter(|p| !p.is_empty())
            .collect();
        out.push(format!(" {} ", text.join(" ")));
    }
    out
}

fn draw_jump_indicator(frame: &mut Frame, area: Rect, app: &App, chat_id: i64) {
    let label = jump_label(app, chat_id);
    let w = (label.width() as u16).min(area.width);
    if area.height == 0 || w == 0 {
        return;
    }
    let rect = Rect {
        x: area.x + area.width - w,
        y: area.y + area.height - 1,
        width: w,
        height: 1,
    };
    textmap::cover(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Span::styled(label, theme::jump_badge())),
        rect,
    );
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
    elapsed: Option<Duration>,
) -> bool {
    let shape = (att.kind == MediaKind::VideoNote).then_some(theme::AvatarShape::Circle);
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
                    Some(e) => note = Some(format!("{} {e}", theme::get().glyphs.tick_failed)),
                    None => {
                        media.want_download(v.file.id, priority);
                        let down = theme::get().glyphs.download.clone();
                        note = Some(match f.percent() {
                            Some(p) => format!("{down} {} {p}%", kind_label(att.kind)),
                            None => format!("{down} {}", kind_label(att.kind)),
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
            shape,
            animate: full && v.decode == Decode::Tgs && elapsed.is_some(),
        };
        match media.look(key, input, overlay) {
            Look::Ready(frames) => {
                if let Some(proto) = frames.at(elapsed.unwrap_or_default()) {
                    render_proto(frame, t, proto);
                    animated = frames.animated();
                }
                return animated;
            }
            Look::Failed(e) => {
                if n == 0 {
                    note = Some(format!("{} {e}", theme::get().glyphs.tick_failed));
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
            shape,
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

fn viewer_hint(player: bool, file: bool, pos: usize, len: usize) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if player {
        parts.extend(["space play/pause", "←/→ seek", "↑/↓ volume", "m mute"]);
    }
    match (pos > 0, pos + 1 < len) {
        (true, true) => parts.push("j/k prev/next"),
        (false, true) => parts.push("j next"),
        (true, false) => parts.push("k prev"),
        (false, false) => {}
    }
    if file {
        parts.push(if player { "x mpv" } else { "x play/open" });
        parts.push("s save");
        if !player {
            parts.push(crate::player::OPEN_HINT);
        }
    }
    parts.push("Esc close");
    format!(" {} ", parts.join(" · "))
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
    textmap::cover(area);
    frame.render_widget(Clear, area);
    let att = app.attachment(chat_id, msg_id, idx).cloned();
    let title = match &att {
        Some(a) => format!(" {} {}/{} ", kind_label(a.kind), pos + 1, len),
        None => format!(" attachment {}/{} ", pos + 1, len),
    };
    let live = app.inline.active() == Some((msg_id, idx));
    let player = att.as_ref().is_some_and(|a| {
        live || (crate::video::playable(a.kind) && app.inline.available().is_ok())
    });
    let file = att.as_ref().is_some_and(|a| app.file_reachable(&a.file));
    let hint = viewer_hint(player, file, pos, len);
    let block = theme::popup_block()
        .title(Span::styled(title, theme::popup_title()))
        .title_bottom(Span::styled(hint, theme::muted()));
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
    let bar = player && inner.height > 3;
    let image_h = inner.height - 1 - u16::from(bar);
    let status = app.inline.status().filter(|_| live).cloned();
    let dims = match status.as_ref().and_then(|s| s.dims) {
        Some((w, h)) => (w as i32, h as i32),
        None => att
            .full
            .as_ref()
            .filter(|v| v.width > 0 && v.height > 0)
            .map_or_else(|| visual_dims(&att), |v| (v.width, v.height)),
    };
    let font = app.media.font();
    let (cols, rows) = fit_cells(dims.0, dims.1, font, inner.width, image_h);
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
    if let Some(d) = status.as_ref().and_then(|s| s.dims) {
        let (w, h) = crate::video::fit_pixels(d, (cols, rows), font);
        app.inline.want(crate::video::Place { cols, rows, w, h });
    }
    let elapsed = app.anim_start.elapsed();
    let video = app.inline.image().filter(|_| live);
    let animated = if let Some(id) = video {
        crate::anim::placeholders(frame.buffer_mut(), t.area, id, (t.x, t.y), (cols, rows));
        false
    } else if att.viewable() {
        draw_attachment(
            frame,
            t,
            &mut app.media,
            &att,
            (chat_id, msg_id, idx),
            false,
            true,
            Some(elapsed),
        )
    } else if player {
        box_note(frame, t, &format!("[{}]", kind_label(att.kind)));
        false
    } else {
        box_note(
            frame,
            t,
            if file {
                "nothing to preview; x to open"
            } else {
                "nothing to preview"
            },
        );
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
    let loading = att.full.as_ref().map_or(&att.file, |v| &v.file);
    if let Some(p) = app.media.file(loading).percent().or_else(|| f.percent()) {
        meta.push(format!("{} {p}%", theme::get().glyphs.download));
    }
    if bar {
        let text = match &status {
            Some(st) => crate::video::progress_line(st, inner.width as usize),
            None if app.inline.requested == Some((msg_id, idx)) && f.local().is_none() => {
                " ⧗ Downloading…".to_owned()
            }
            None => " ▶ space to play here".to_owned(),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                format::truncate(&text, inner.width as usize),
                theme::popup_title(),
            )),
            Rect {
                y: inner.y + image_h,
                height: 1,
                ..inner
            },
        );
    }
    let line = Rect {
        y: inner.y + inner.height - 1,
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

fn draw_recording(frame: &mut Frame, area: Rect, rec: &crate::app::Recording) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(theme::border_set())
        .border_style(theme::recording())
        .padding(Padding::horizontal(composer_pad()))
        .title(Span::styled(" Voice message ", theme::recording()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let line = if rec.stopping {
        Line::from(Span::styled("⧗ Encoding…", theme::muted()))
    } else {
        let secs = rec.started.elapsed().as_secs() as i32;
        let levels = rec
            .levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recording_line(secs, &levels, inner.width as usize)
    };
    frame.render_widget(Paragraph::new(line), inner);
}

fn recording_line(secs: i32, levels: &[u16], width: usize) -> Line<'static> {
    const MIN_BARS: usize = 8;
    let th = theme::get();
    let dot = if secs % 2 == 0 {
        &th.glyphs.recording
    } else {
        &th.glyphs.recording_blink
    };
    let label = format!("{dot} REC {}", format::duration(secs));
    let full = vec![
        Span::styled("Enter", theme::strong()),
        Span::styled(" send · ", theme::muted()),
        Span::styled("Esc", theme::strong()),
        Span::styled(" cancel", theme::muted()),
    ];
    let compact = vec![
        Span::styled("Enter", theme::strong()),
        Span::styled(" · ", theme::muted()),
        Span::styled("Esc", theme::strong()),
    ];
    let room = |hints: &[Span]| {
        let w: usize = hints.iter().map(|h| h.content.width()).sum();
        width.saturating_sub(label.width() + w + 4)
    };
    let hints = if room(&full) >= MIN_BARS || room(&compact) < MIN_BARS {
        full
    } else {
        compact
    };
    let bars_w = room(&hints);
    let mut spans = vec![Span::styled(label, theme::recording()), Span::raw("  ")];
    if bars_w >= MIN_BARS {
        let bars = crate::voice::live_bars(levels, bars_w);
        let fill = bars_w - bars.chars().count();
        spans.push(Span::styled(bars, theme::waveform()));
        spans.push(Span::raw(" ".repeat(fill + 2)));
    }
    spans.extend(hints);
    Line::from(spans)
}

fn draw_composer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(rec) = &app.recording {
        draw_recording(frame, area, rec);
        return;
    }
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
        match &c.quote {
            Some((q, _)) => Span::styled(
                format!(
                    " Reply to {name}: “{}” ",
                    format::truncate(&format::one_line(q), 32)
                ),
                theme::strong(),
            ),
            None => Span::styled(format!(" Reply to {name} "), theme::strong()),
        }
    } else {
        Span::styled(" Message ", theme::strong())
    };
    let mut room = (area.width as usize).saturating_sub(2 + format::cells(&title.content));
    let mut marks: Vec<Span> = Vec::new();
    for label in app
        .open_chat
        .map(|id| unread_badges(app, id))
        .unwrap_or_default()
        .into_iter()
        .rev()
    {
        let need = format::cells(&label) + 1;
        if need > room {
            break;
        }
        room -= need;
        if !marks.is_empty() {
            marks.insert(0, Span::raw(" "));
        }
        marks.insert(0, Span::styled(label, theme::jump_badge()));
    }
    let typing = app
        .open_chat
        .and_then(|id| app.typing_line(id, app.open_topic))
        .map(|t| format::truncate(&format!("· {t}"), room.saturating_sub(1)))
        .filter(|t| t.chars().count() > 2)
        .map(|t| Span::styled(format!("{t} "), theme::typing()));
    let title = Line::from(std::iter::once(title).chain(typing).collect::<Vec<_>>());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(theme::border_set())
        .border_style(if c.active {
            theme::focus_border()
        } else {
            theme::border()
        })
        .padding(Padding::horizontal(composer_pad()))
        .title(title);
    let block = if marks.is_empty() {
        block
    } else {
        block.title_top(Line::from(marks).right_aligned())
    };
    let block = if c.att_sel.is_some() {
        block.title_bottom(Span::styled(
            " j/k select · J/K move · f as file · d remove · Tab text ",
            theme::muted(),
        ))
    } else if !c.attachments.is_empty() && c.active {
        block.title_bottom(Span::styled(
            " Tab attachments · Del remove last · Enter send ",
            theme::muted(),
        ))
    } else if c.active {
        block.title_bottom(Span::styled(
            " <C-o> file · <C-v> paste · <C-r> voice · <C-s> stickers · <C-g> GIFs ",
            theme::muted(),
        ))
    } else {
        block
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let att = attachment_lines(app, width);
    let att_h = (att.len() as u16).min(inner.height.saturating_sub(1));
    let [att_area, inner] =
        Layout::vertical([Constraint::Length(att_h), Constraint::Min(0)]).areas(inner);
    if att_h > 0 {
        frame.render_widget(Paragraph::new(att), att_area);
    }
    if c.input.is_empty() && !c.active {
        frame.render_widget(
            Paragraph::new(Span::styled(app.composer_hint(), theme::muted())),
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
    if c.active && c.att_sel.is_none() && h > 0 {
        frame.set_cursor_position(Position {
            x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
            y: inner.y + (row - skip) as u16,
        });
    }
}

fn draw_info(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = pane_block(" Chat info ".to_owned(), app.focus == Pane::Info);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    textmap::area(Zone::Pane(Pane::Info), area);
    app.info_height = inner.height as usize;
    let Some(chat) = app.open_chat.and_then(|id| app.chats.get(&id)) else {
        frame.render_widget(
            Paragraph::new(Span::styled(" No chat open", theme::muted())),
            inner,
        );
        return;
    };
    let chat_id = chat.id;
    let font = app.media.font();
    let info = info_lines(app, chat, inner.width as usize);
    let height = inner.height as usize;
    let avatar = info_avatar_size(inner, font).map(|(cols, rows)| ImageTarget {
        area: inner,
        x: (inner.width - cols) / 2,
        y: 1,
        cols,
        rows,
    });
    let lead = avatar.map_or(0, |t| t.rows as usize + 2);
    let max = match lead {
        _ if lead + info.len() <= height => 0,
        0 => info.len() - height,
        _ => 1 + info.len().saturating_sub(height),
    };
    let scroll = app.info_scroll.min(max);
    let avatar = avatar.filter(|_| scroll == 0);
    let skip = if lead == 0 {
        scroll
    } else {
        scroll.saturating_sub(1)
    };
    let mut lines = Vec::new();
    if let Some(t) = avatar {
        let color = match chat.kind {
            ChatKind::Private(uid) | ChatKind::Secret(uid) => uid,
            _ => chat.id,
        };
        let label = initials(&chat.title);
        let cols = t.cols as usize;
        lines.push(Line::from(""));
        for row in 0..t.rows {
            let text = if row == t.rows / 2 {
                let pad = cols.saturating_sub(label.width());
                format!(
                    "{}{label}{}",
                    " ".repeat(pad / 2),
                    " ".repeat(pad - pad / 2)
                )
            } else {
                " ".repeat(cols)
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(t.x as usize)),
                Span::styled(text, theme::avatar(color)),
            ]));
        }
        lines.push(Line::from(""));
    }
    let shown = info.len().saturating_sub(skip);
    let text_top = inner.y.saturating_add(lines.len() as u16);
    for y in (text_top..inner.bottom()).take(shown) {
        textmap::row(inner.x, y, inner.width, Join::Line);
    }
    lines.extend(info.into_iter().skip(skip));
    app.info_scroll = scroll;
    frame.render_widget(Paragraph::new(lines), inner);
    if max > 0 && area.width > 2 {
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(None)
            .thumb_symbol("┃")
            .thumb_style(theme::scrollbar_thumb());
        let mut state = ScrollbarState::new(max)
            .position(scroll)
            .viewport_content_length(height);
        let edge = Rect::new(area.right() - 1, inner.y, 1, inner.height);
        frame.render_stateful_widget(bar, edge, &mut state);
    }
    let Some(t) = avatar else {
        return;
    };
    let behind = theme::surface().bg;
    let shape = theme::get().layout.avatar_shape;
    mask_avatar(frame, t, font, shape, theme::cell_bg(behind));
    if let Some(photo) = app.profile_photo(model::ProfileTarget::Chat(chat_id)) {
        let overlay = app.popup.is_some() || !app.pending.is_empty();
        draw_avatar(frame, t, &mut app.media, &photo, overlay, behind);
    }
}

fn info_avatar_size(inner: Rect, font: (u16, u16)) -> Option<(u16, u16)> {
    let per_row = (f32::from(font.1.max(1)) / f32::from(font.0.max(1))).max(1.0);
    let room = inner.width.saturating_sub(2);
    let wanted = (f32::from(inner.width) * 0.6 / per_row) as u16;
    let rows = wanted
        .clamp(8, 12)
        .min((f32::from(room) / per_row) as u16)
        .min(inner.height / 2);
    let cols = (f32::from(rows) * per_row).round() as u16;
    (rows >= 3 && cols <= room).then_some((cols, rows))
}

fn info_field(label: &str, value: &str, width: usize) -> Vec<Line<'static>> {
    let key = format!(" {label}: ");
    let key_w = format::cells(&key);
    let own_line = width < key_w + 8;
    let indent = if own_line { 2 } else { key_w };
    let body = render_text(
        &model::RichText::plain(value.to_owned()),
        theme::normal(),
        width.saturating_sub(indent).max(1),
    );
    let mut out = Vec::new();
    let mut key = Some(Span::styled(key, theme::muted()));
    if own_line {
        out.extend(key.take().map(Line::from));
    }
    for line in body {
        let lead = key.take().unwrap_or_else(|| Span::raw(" ".repeat(indent)));
        let mut spans = vec![lead];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    out
}

fn info_lines(app: &App, chat: &model::ChatInfo, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for l in render_text(
        &model::RichText::plain(chat.title.clone()),
        theme::strong(),
        width.saturating_sub(2).max(1),
    ) {
        let pad = width.saturating_sub(line_width(&l)) / 2;
        let mut spans = vec![Span::raw(" ".repeat(pad))];
        spans.extend(l.spans);
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    let user = match chat.kind {
        ChatKind::Private(uid) | ChatKind::Secret(uid) => Some(uid),
        _ => None,
    };
    let profile = app.chat_profiles.get(&chat.id);
    let mut field = |label: &str, value: String| lines.extend(info_field(label, &value, width));
    if let Some(p) = profile.filter(|p| !p.usernames.is_empty()) {
        let names: Vec<String> = p.usernames.iter().map(|n| format!("@{n}")).collect();
        field("Username", names.join(" · "));
    }
    if let Some(status) = user.and_then(|uid| app.user_status(uid)) {
        field("Status", status);
    }
    if let Some(n) = profile.and_then(|p| p.members).filter(|_| user.is_none()) {
        field("Members", app.count(i64::from(n)));
    }
    if let Some(p) = profile.filter(|p| !p.bio.is_empty()) {
        let heading = if p.user_id.is_some() {
            "Bio"
        } else {
            "Description"
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {heading}"),
            theme::muted(),
        )));
        for l in render_text(
            &model::RichText::plain(p.bio.clone()),
            theme::normal(),
            width.saturating_sub(2).max(1),
        ) {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(l.spans);
            lines.push(Line::from(spans));
        }
    }
    let mut details = Vec::new();
    let mut field = |label: &str, value: String| details.extend(info_field(label, &value, width));
    match chat.kind {
        ChatKind::Group | ChatKind::Supergroup => field("Type", "group".to_owned()),
        ChatKind::Channel => field("Type", "channel".to_owned()),
        ChatKind::Private(_) | ChatKind::Secret(_) => {}
    }
    if let Some(p) = profile {
        let hidden = user.is_some_and(|u| app.my_id == Some(u)) && app.settings.privacy.hide_phone;
        if user.is_some() && !p.phone.is_empty() && !hidden {
            field("Phone", p.phone.clone());
        }
        let invites = p.links.iter().filter(|l| {
            !p.usernames
                .iter()
                .any(|n| **l == format!("https://t.me/{n}"))
        });
        for link in invites.filter(|_| user.is_none()) {
            field("Link", link.clone());
        }
    }
    if let Some(sc) = app.secret_chat(chat.id) {
        field("Secret", sc.state.label().to_owned());
        field("Stored", "only on this device".to_owned());
    }
    if chat.auto_delete > 0 {
        let label = if matches!(chat.kind, ChatKind::Secret(_)) {
            "Self-destruct"
        } else {
            "Auto-delete"
        };
        field(label, format::ttl(chat.auto_delete));
    }
    if chat.forum {
        let n = app.topics.get(&chat.id).map_or(0, |l| l.topics().len());
        field("Topics", n.to_string());
    }
    if let Some(t) = app.open_topic_info() {
        let mut state = vec![format!("{} unread", app.count(i64::from(t.unread)))];
        if t.closed {
            state.push("closed".to_owned());
        }
        if app.topic_muted(chat.id, t.id) {
            state.push("muted".to_owned());
        }
        field("Topic", format!("{} · {}", t.name, state.join(", ")));
    }
    let unread = app.chat_unread(chat.id);
    if unread > 0 {
        field("Unread", app.count(i64::from(unread)));
    }
    if app.is_muted(chat.id) {
        field("Notifications", "off".to_owned());
    }
    if !details.is_empty() {
        lines.push(Line::from(""));
        lines.extend(details);
    }
    if !app.settings.appearance.show_ids {
        return lines;
    }
    lines.push(Line::from(""));
    let mut field = |label: &str, value: String| lines.extend(info_field(label, &value, width));
    match user {
        Some(uid) if uid == chat.id => field("User ID", uid.to_string()),
        Some(uid) => {
            field("Chat ID", chat.id.to_string());
            field("User ID", uid.to_string());
        }
        None => field("Chat ID", chat.id.to_string()),
    }
    if chat.group_id != 0 {
        field("Supergroup ID", chat.group_id.to_string());
    }
    if chat.secret_id != 0 {
        field("Secret chat ID", chat.secret_id.to_string());
    }
    if let Some(t) = app.open_topic {
        field("Topic ID", t.to_string());
    }
    if let Some(m) = app.selected_message() {
        field("Message ID", m.id.to_string());
    }
    if let Some(h) = app.histories.get(&chat.id) {
        field("Loaded", format!("{} messages", h.len()));
    }
    let (images, bytes) = app.media.stats();
    field(
        "Images",
        format!(
            "{} · {images} cached · {}",
            app.media.protocol_name(),
            format::file_size(bytes as i64)
        ),
    );
    lines
}

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let fit = |want: u16, avail: u16| want.clamp(1, avail.saturating_sub(2).max(1));
    let (w, h) = (fit(width, area.width), fit(height, area.height));
    let left = (area.width.max(w) - w) / 2;
    let top = (area.height.max(h) - h) / 2;
    Rect::new(area.x + left, area.y + top, w, h)
}

pub(crate) fn modal(frame: &mut Frame, rect: Rect, title: &str, lines: Vec<Line<'static>>) -> Rect {
    frame.render_widget(Clear, rect);
    let block =
        theme::popup_block().title(Span::styled(format!(" {title} "), theme::popup_title()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
    textmap::panel(rect, inner);
    inner
}

fn draw_popup(frame: &mut Frame, area: Rect, app: &App) {
    let Some(popup) = &app.popup else {
        return;
    };
    match popup {
        Popup::Viewer { .. } | Popup::Stickers(_) => {}
        Popup::Menu { .. }
        | Popup::Help { .. }
        | Popup::ConfirmRemoveChat { .. }
        | Popup::Profile(_)
        | Popup::Reactions(_)
        | Popup::ReactionList { .. }
        | Popup::Search(_)
        | Popup::Find(_)
        | Popup::Buttons(_)
        | Popup::ConfirmUrl(_)
        | Popup::Links { .. }
        | Popup::Files { .. }
        | Popup::ConfirmStart { .. }
        | Popup::ConfirmJoin(_)
        | Popup::Options { .. }
        | Popup::Details { .. }
        | Popup::EditHistory { .. }
        | Popup::ConfirmClearDeleted
        | Popup::QuotePick { .. }
        | Popup::TextSelect { .. }
        | Popup::StatusLog { .. }
        | Popup::SharedMedia { .. }
        | Popup::Translate { .. }
        | Popup::Members(_)
        | Popup::EventLog(_)
        | Popup::Inbox { .. } => draw_stage4_popup(frame, area, app, popup),
        Popup::FilePicker(fp) => draw_file_picker(frame, area, app, fp),
        Popup::ConfirmDelete {
            message_ids,
            everyone,
            me,
            ..
        } => {
            let question = if message_ids.len() > 1 {
                format!(" Delete these {} messages?", message_ids.len())
            } else {
                " Delete this message?".to_owned()
            };
            let mut choices = vec![Span::raw(" ")];
            if *everyone {
                choices.push(Span::styled("y ", theme::active_field()));
                choices.push(Span::raw("for everyone   "));
            }
            if *me {
                choices.push(Span::styled("m ", theme::active_field()));
                choices.push(Span::raw("only for me"));
                choices.push(Span::styled(" (Enter)", theme::muted()));
            }
            let lines = vec![
                Line::from(question),
                Line::from(""),
                Line::from(choices),
                Line::from(vec![
                    Span::styled(" n ", theme::active_field()),
                    Span::raw("cancel"),
                ]),
            ];
            let rect = centered(area, 40, 6);
            modal(frame, rect, "Delete", lines);
        }
        Popup::NewTopic { input, .. } => {
            let lines = vec![
                Line::from(vec![
                    Span::styled("> ", theme::active_field()),
                    if input.is_empty() {
                        Span::styled("Topic name…", theme::muted())
                    } else {
                        Span::raw(input.text().to_owned())
                    },
                ]),
                Line::from(Span::styled(" Enter create · Esc cancel", theme::muted())),
            ];
            let rect = centered(area, 44, 4);
            let inner = modal(frame, rect, "New topic", lines);
            let (_, col) = input.cursor_position(0);
            frame.set_cursor_position(Position {
                x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
                y: inner.y,
            });
        }
        Popup::Ttl {
            chat_id,
            choices,
            sel,
        } => {
            let secret = app
                .chats
                .get(chat_id)
                .is_some_and(|c| matches!(c.kind, ChatKind::Secret(_)));
            let current = app.chats.get(chat_id).map_or(0, |c| c.auto_delete);
            let mut lines: Vec<Line> = choices
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let mark = if *v == current { " ✓" } else { "" };
                    row_line(
                        vec![Span::raw(format!("{}{mark}", format::ttl(*v)))],
                        i == *sel,
                        true,
                        32,
                    )
                })
                .collect();
            if secret {
                lines.push(Line::from(Span::styled(
                    " applies to new messages",
                    theme::muted(),
                )));
            }
            let title = if secret {
                "Self-destruct timer"
            } else {
                "Auto-delete timer"
            };
            let rect = centered(area, 34, lines.len() as u16 + 2);
            modal(frame, rect, title, lines);
        }
        Popup::ConfirmSecret { delete, .. } => {
            let question = if *delete {
                " Delete this secret chat and its messages?"
            } else {
                " Close this secret chat? It can't be reopened."
            };
            let lines = vec![
                Line::from(question),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" y ", theme::active_field()),
                    Span::raw(if *delete { "delete   " } else { "close   " }),
                    Span::styled("n ", theme::active_field()),
                    Span::raw("cancel"),
                ]),
            ];
            let rect = centered(area, 50, 5);
            modal(frame, rect, "Secret chat", lines);
        }
        Popup::ConfirmQuit(work) => {
            let mut lines = vec![Line::from(" Still in progress:")];
            for w in work {
                lines.push(Line::from(Span::styled(
                    format!(" · {}", crate::td::sentence_case(w)),
                    theme::warning(),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" y ", theme::active_field()),
                Span::raw("quit anyway   "),
                Span::styled("n ", theme::active_field()),
                Span::raw("cancel"),
            ]));
            let w = lines
                .iter()
                .map(|l| line_width(l))
                .max()
                .unwrap_or(0)
                .max(36) as u16
                + 3;
            let rect = centered(area, w, lines.len() as u16 + 2);
            modal(frame, rect, "Quit", lines);
        }
        Popup::ConfirmLogout => {
            let lines = vec![
                Line::from(" Log out of Telegram?"),
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
        Popup::Switcher {
            input,
            sel,
            forward,
        } => {
            let results = app.switcher_results(input.text());
            let rect = centered(area, 60.min(area.width), 20.min(area.height));
            let inner_w = rect.width.saturating_sub(2) as usize;
            let rows = rect.height.saturating_sub(4) as usize;
            app.popup_rows.set(rows);
            let sel = (*sel).min(results.len().saturating_sub(1));
            let start = list_window(sel, results.len(), rows);
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("> ", theme::active_field()),
                    if input.is_empty() {
                        Span::styled("Type a chat name…", theme::muted())
                    } else {
                        Span::raw(input.text().to_owned())
                    },
                ]),
                Line::from(Span::styled(
                    theme::get().glyphs.separator.repeat(inner_w),
                    theme::border(),
                )),
            ];
            lines.extend(
                results
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(rows)
                    .filter_map(|(i, id)| {
                        let c = app.chats.get(id)?;
                        let mut spans = Vec::new();
                        if forward.as_ref().is_some_and(|f| f.targets.contains(id)) {
                            spans.push(Span::styled("✓ ", theme::active_field()));
                        }
                        spans.push(Span::raw(c.title.clone()));
                        let unread = app.chat_unread(*id);
                        if unread > 0 {
                            spans.push(Span::styled(
                                format!(" {}", app.count(i64::from(unread))),
                                theme::muted(),
                            ));
                        }
                        Some(row_line(spans, i == sel, true, inner_w))
                    }),
            );
            let title = match forward {
                Some(f) => format!(
                    "Forward to{} · Tab mark · <C-t> sender name: {}",
                    match f.targets.len() {
                        0 => String::new(),
                        n => format!(" {n} chats"),
                    },
                    if f.hide_sender { "hidden" } else { "shown" }
                ),
                None => "Find chat".to_owned(),
            };
            let inner = modal(frame, rect, &title, lines);
            let (_, col) = input.cursor_position(0);
            frame.set_cursor_position(Position {
                x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
                y: inner.y,
            });
        }
    }
}

fn draw_file_picker(frame: &mut Frame, area: Rect, app: &App, fp: &crate::app::FilePicker) {
    let (dir, _, _) = fp.query(&app.cwd, &app.home);
    let matches = fp.matches(&app.cwd, &app.home);
    let rect = centered(area, 72.min(area.width), 22.min(area.height));
    let inner_w = rect.width.saturating_sub(2) as usize;
    let rows = rect.height.saturating_sub(6) as usize;
    app.popup_rows.set(rows);
    let sel = fp.sel.min(matches.len().saturating_sub(1));
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if fp.input.is_empty() {
                Span::styled("Type a path (~/ for home)…", theme::muted())
            } else {
                Span::raw(fp.input.text().to_owned())
            },
        ]),
        Line::from(Span::styled(
            format::truncate(&dir.display().to_string(), inner_w),
            theme::muted(),
        )),
        Line::from(Span::styled(
            theme::get().glyphs.separator.repeat(inner_w),
            theme::border(),
        )),
    ];
    if let Some(e) = fp.error.as_ref().filter(|_| dir == fp.dir) {
        lines.push(Line::from(Span::styled(
            format::truncate(&format!("{} {e}", theme::get().glyphs.tick_failed), inner_w),
            theme::error(),
        )));
    } else if dir != fp.dir {
        lines.push(Line::from(Span::styled("⧗ Reading…", theme::muted())));
    } else if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching files",
            theme::muted(),
        )));
    }
    let start = list_window(sel, matches.len(), rows);
    for (i, idx) in matches.iter().enumerate().skip(start).take(rows) {
        let e = &fp.entries[*idx];
        let span = if e.dir {
            Span::styled(format!("{}/", e.name), theme::link())
        } else {
            Span::raw(e.name.clone())
        };
        lines.push(row_line(vec![span], i == sel, true, inner_w));
    }
    let used = lines.len();
    let total = rect.height.saturating_sub(2) as usize;
    lines.extend(std::iter::repeat_n(
        Line::default(),
        total.saturating_sub(used + 1),
    ));
    lines.push(Line::from(Span::styled(
        format::truncate(
            " Tab complete · Enter attach / open dir · Esc close",
            inner_w,
        ),
        theme::muted(),
    )));
    let inner = modal(frame, rect, "Attach file", lines);
    let (_, col) = fp.input.cursor_position(0);
    frame.set_cursor_position(Position {
        x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
        y: inner.y,
    });
}

const STICKER_CELL_COLS: u16 = 10;
const PICKER_ANIMATION_FACTOR: usize = 4;

fn draw_stickers(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(Popup::Stickers(mut sp)) = app.popup.take() else {
        return;
    };
    let rect = centered(
        area,
        (area.width * 9 / 10).max(30),
        (area.height * 9 / 10).max(12),
    );
    textmap::cover(rect);
    frame.render_widget(Clear, rect);
    let tab = sp.current_tab(&app.stickers);
    let title = if tab == crate::stickers::Tab::Gifs && sp.query.is_empty() {
        " GIFs "
    } else {
        " Stickers "
    };
    let block = theme::popup_block()
        .title(Span::styled(title, theme::popup_title()))
        .title_bottom(Span::styled(
            " Tab/S-Tab section · arrows move · Enter send · <C-o> big preview · type words or emoji to search · Esc ",
            theme::muted(),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 4 || inner.width < 4 {
        app.popup = Some(Popup::Stickers(sp));
        return;
    }
    let [query_row, tabs_row, grid] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let q = sp.query.text().to_owned();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if q.is_empty() {
                Span::styled("Emoji or word to search…", theme::muted())
            } else {
                Span::raw(q.clone())
            },
        ])),
        query_row,
    );
    let (_, col) = sp.query.cursor_position(0);
    frame.set_cursor_position(Position {
        x: (query_row.x + 2 + col as u16).min(query_row.right().saturating_sub(1)),
        y: query_row.y,
    });
    let tabs = app.stickers.tabs();
    let cur = sp.tab.min(tabs.len() - 1);
    let width = tabs_row.width as usize;
    let label = |i: usize| format!(" {} ", format::truncate(&tabs[i].1, 20));
    let mut first = cur;
    let mut used = label(cur).width();
    while first > 0 && used + label(first - 1).width() < width {
        first -= 1;
        used += label(first).width() + 1;
    }
    let mut spans = Vec::new();
    for i in first..tabs.len() {
        let l = label(i);
        if spans
            .iter()
            .map(|s: &Span| s.content.width())
            .sum::<usize>()
            + l.width()
            > width
        {
            break;
        }
        let style = if !q.is_empty() {
            theme::muted()
        } else if i == cur {
            theme::active_field()
        } else {
            theme::normal()
        };
        spans.push(Span::styled(l, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    let items = sp.visible(&app.stickers);
    let font = app.media.font();
    let cell_cols = STICKER_CELL_COLS;
    let cell_rows = ((u32::from(cell_cols) * u32::from(font.0) / u32::from(font.1)) as u16).max(2);
    let pitch_w = cell_cols + 2;
    let pitch_h = cell_rows + 2;
    let cols = (grid.width / pitch_w).max(1) as usize;
    let rows = (grid.height / pitch_h).max(1) as usize;
    sp.cols = cols;
    sp.rows = rows;
    sp.sel = sp.sel.min(items.len().saturating_sub(1));
    sp.fix_scroll(cols, rows);
    if items.is_empty() {
        let loading = if !q.is_empty() {
            sp.searching
        } else {
            app.stickers.items(tab).is_none()
        };
        let text = if loading {
            "⧗ Loading…"
        } else {
            "nothing here"
        };
        frame.render_widget(Paragraph::new(Span::styled(text, theme::muted())), grid);
    }
    let animated = app.media.anim.enabled()
        && app.settings.media.animations != crate::config::Animations::Off;
    let paused = app.settings.media.pause_unfocused && !app.term_focused;
    app.media.anim.pause(paused);
    let now = app.picker_freeze.at(app.anim_start.elapsed(), paused);
    let mut head = crate::anim::Playhead::new(now, 1, paused);
    if sp.preview
        && let Some(item) = items.get(sp.sel)
    {
        frame.render_widget(Clear, grid);
        let (w, h) = (item.width.max(1), item.height.max(1));
        let (c, r) = fit_cells(
            w,
            h,
            font,
            grid.width.saturating_sub(2).max(1),
            grid.height.saturating_sub(1).max(1),
        );
        let t = ImageTarget {
            area: grid,
            x: (grid.width.saturating_sub(c)) / 2,
            y: (grid.height.saturating_sub(r) / 2) as isize,
            cols: c,
            rows: r,
        };
        if !(animated && draw_animation(frame, t, &mut app.media, &item.att, &mut head)) {
            draw_attachment(
                frame,
                t,
                &mut app.media,
                &item.att,
                (0, i64::from(item.file_id), 0),
                false,
                true,
                None,
            );
        }
        app.redraw_due = min_due(app.redraw_due, head.due);
        app.popup = Some(Popup::Stickers(sp));
        return;
    }
    let mut moving: Vec<usize> = (sp.scroll * cols..items.len().min((sp.scroll + rows) * cols))
        .filter(|i| animated && crate::anim::source(&items[*i].att).is_some())
        .collect();
    moving.sort_by_key(|i| i.abs_diff(sp.sel));
    moving.truncate(if app.media.anim.native() {
        app.settings.media.max_animations() * PICKER_ANIMATION_FACTOR
    } else {
        1
    });
    for (i, item) in items
        .iter()
        .enumerate()
        .skip(sp.scroll * cols)
        .take(rows * cols)
    {
        let slot = i - sp.scroll * cols;
        let (cx, cy) = (
            (slot % cols) as u16 * pitch_w,
            (slot / cols) as u16 * pitch_h,
        );
        if cy + pitch_h > grid.height {
            break;
        }
        let cell = Rect {
            x: grid.x + cx,
            y: grid.y + cy,
            width: pitch_w,
            height: pitch_h,
        };
        if i == sp.sel {
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(theme::border_set())
                    .border_style(theme::selection_border()),
                cell,
            );
        }
        let (w, h) = (item.width.max(1), item.height.max(1));
        let (c, r) = fit_cells(w, h, font, cell_cols, cell_rows);
        let t = ImageTarget {
            area: grid,
            x: cx + 1 + (cell_cols - c) / 2,
            y: (cy + 1 + (cell_rows - r) / 2) as isize,
            cols: c,
            rows: r,
        };
        if moving.contains(&i) && draw_animation(frame, t, &mut app.media, &item.att, &mut head) {
            continue;
        }
        draw_attachment(
            frame,
            t,
            &mut app.media,
            &item.att,
            (0, i64::from(item.file_id), 0),
            false,
            false,
            None,
        );
    }
    app.redraw_due = min_due(app.redraw_due, head.due);
    app.popup = Some(Popup::Stickers(sp));
}

const HINT_GAP: usize = 4;
const HINT_CANCEL: &str = " Esc cancel ";

fn hint_column(group: crate::keymap::Group, hints: &[crate::keymap::Hint]) -> Vec<Line<'static>> {
    let key_w = hints.iter().map(|h| h.key.width() + 2).max().unwrap_or(3);
    let mut out = vec![Line::from(Span::styled(
        group.title().to_owned(),
        theme::hint_group(),
    ))];
    for h in hints {
        let key = format!("[{}]", h.key);
        let pad = key_w.saturating_sub(key.width());
        let desc_style = if h.action.destructive() {
            theme::error()
        } else {
            theme::normal()
        };
        out.push(Line::from(vec![
            Span::styled(key, theme::hint_key()),
            Span::raw(" ".repeat(pad + 2)),
            Span::styled(h.desc.clone(), desc_style),
        ]));
    }
    out
}

fn line_width(line: &Line) -> usize {
    line.spans.iter().map(|s| format::cells(&s.content)).sum()
}

fn key_hint_lines(
    groups: &[(crate::keymap::Group, Vec<crate::keymap::Hint>)],
    max_w: usize,
) -> Vec<Line<'static>> {
    let cols: Vec<(Vec<Line<'static>>, usize)> = groups
        .iter()
        .map(|(g, h)| {
            let lines = hint_column(*g, h);
            let w = lines.iter().map(line_width).max().unwrap_or(0);
            (lines, w)
        })
        .collect();
    let mut rows: Vec<Vec<(Vec<Line<'static>>, usize)>> = Vec::new();
    let mut used = 0;
    for col in cols {
        match rows.last_mut() {
            Some(row) if used + HINT_GAP + col.1 <= max_w => {
                used += HINT_GAP + col.1;
                row.push(col);
            }
            _ => {
                used = col.1;
                rows.push(vec![col]);
            }
        }
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for (ri, row) in rows.into_iter().enumerate() {
        if ri > 0 {
            out.push(Line::default());
        }
        let height = row.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        let last = row.len() - 1;
        for r in 0..height {
            let mut spans = Vec::new();
            for (ci, (lines, w)) in row.iter().enumerate() {
                let line = lines.get(r);
                let lw = line.map_or(0, line_width);
                if let Some(l) = line {
                    spans.extend(l.spans.iter().cloned());
                }
                if ci < last {
                    spans.push(Span::raw(" ".repeat(w - lw + HINT_GAP)));
                }
            }
            out.push(Line::from(spans));
        }
    }
    out
}

fn draw_key_hint(frame: &mut Frame, area: Rect, app: &App) {
    if app.pending.is_empty() {
        return;
    }
    let groups = app.key_hints();
    if groups.is_empty() {
        return;
    }
    let lines = key_hint_lines(&groups, area.width.saturating_sub(4) as usize);
    let title = if app.pending.first() == Some(&app.keymap.leader) && app.pending.len() == 1 {
        "Leader".to_owned()
    } else {
        app.pending.iter().map(|c| c.label()).collect::<String>()
    };
    let content_w = lines
        .iter()
        .map(line_width)
        .max()
        .unwrap_or(0)
        .max(title.width() + 2)
        .max(HINT_CANCEL.width());
    let width = (content_w + 4).min(area.width as usize) as u16;
    let height = (lines.len() as u16 + 2).min(area.height);
    let lift = u16::from(area.height > height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height + lift),
        width,
        height,
    };
    textmap::cover(rect);
    frame.render_widget(Clear, rect);
    let block = theme::popup_block()
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), theme::popup_title()))
        .title_bottom(Span::styled(HINT_CANCEL, theme::muted()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_login(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_row: Option<(usize, usize)> = None;
    let hint = |text: &str| Line::from(Span::styled(text.to_owned(), theme::muted()));
    let max_text = (area.width as usize).saturating_sub(5).max(10);
    let wrapped = |text: &str, style: Style| -> Vec<Line<'static>> {
        crate::popups::wrap_words(text, max_text)
            .into_iter()
            .map(|row| Line::from(Span::styled(row, style)))
            .collect()
    };
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
        Auth::Starting => lines.push(hint("Connecting to Telegram…")),
        Auth::Closing => lines.push(hint("Closing the session…")),
        Auth::Ready => {}
        Auth::Blocked(text) => {
            lines.extend(wrapped(text, theme::warning()));
            lines.push(Line::from(""));
            lines.push(hint("Enter: log in again · q: quit"));
        }
        Auth::Qr(None) => {
            lines.push(hint("Requesting a QR code…"));
            lines.push(Line::from(""));
            lines.push(hint("p: log in with phone number · q: quit"));
        }
        Auth::Qr(Some(link)) => match qr::qr_lines(link) {
            Ok(qr) => {
                let fits = qr.len() + 5 <= area.height as usize
                    && qr.first().map_or(0, |l| l.chars().count()) + 4 <= area.width as usize;
                if fits {
                    let qr_style = theme::qr();
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
                            qr.len() + 5
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
                "Enter: send code · Esc: back to QR login · <C-c>: quit",
            ));
        }
        Auth::Code { phone, via } => {
            lines.extend(field(
                &format!("Code sent to {phone} via {via}"),
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
            lines.push(Line::from(""));
            lines.push(hint("Enter: confirm · Esc: change number · <C-c>: quit"));
        }
        Auth::Password { hint: pw_hint } => {
            let masked = "•".repeat(app.login_input.text().chars().count());
            lines.extend(field("Two-step verification password", masked));
            cursor_row = Some((1, 2));
            if !pw_hint.is_empty() {
                lines.push(hint(&format!("Hint: {pw_hint}")));
            }
            lines.push(Line::from(""));
            lines.push(hint("Enter: confirm · Esc: clear · <C-c>: quit"));
        }
        Auth::Email => {
            lines.extend(field(
                "Login email address",
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
            lines.push(Line::from(""));
            lines.push(hint("Enter: send code · Esc: change number · <C-c>: quit"));
        }
        Auth::EmailCode { pattern } => {
            lines.extend(field(
                &format!("Code sent to {pattern}"),
                app.login_input.text().to_owned(),
            ));
            cursor_row = Some((1, 2));
            lines.push(Line::from(""));
            lines.push(hint("Enter: confirm · Esc: change number · <C-c>: quit"));
        }
    }
    if app.login_busy && app.auth.takes_text() {
        let busy = if app.auth == Auth::Phone {
            "Sending the code…"
        } else {
            "Checking…"
        };
        lines.push(Line::from(Span::styled(busy, theme::status())));
    }
    if let Some(err) = &app.auth_error {
        lines.push(Line::from(""));
        lines.extend(wrapped(err, theme::error()));
    }
    let content_w = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.width()).sum::<usize>())
        .max()
        .unwrap_or(20);
    let width = (content_w as u16 + 4).max(50).min(area.width.max(1));
    let height = (lines.len() as u16 + 2).min(area.height.max(1));
    let mut rect = centered(area, width, height);
    if rect.height < height {
        rect.y = area.y;
        rect.height = height;
    }
    let block = theme::popup_block().title(Span::styled(
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

    const MSG_GUTTER: usize = 2;
    const AVATAR_COLS: u16 = 4;
    const BUBBLE_PERCENT: usize = 68;

    #[test]
    fn test_constants_match_the_default_theme() {
        let l = crate::theme::Theme::default().layout;
        assert_eq!(l.message_gutter as usize, MSG_GUTTER);
        assert_eq!(l.bubble_avatar_rows * 2, AVATAR_COLS);
        assert_eq!(l.bubble_max_width as usize, BUBBLE_PERCENT);
    }
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

    fn classic_rounded() -> theme::Theme {
        let mut t = theme::Theme::default();
        t.layout.message_layout = crate::config::MessageLayout::Classic;
        t.layout.border_style = crate::config::BorderStyle::Rounded;
        t
    }

    fn app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
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
        app.set_theme(classic_rounded());
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
                reactions: 0,
                last_read_inbox: 0,
                last_read_outbox: 0,
                photo: None,
                muted: false,
                use_default_mute: false,
                positions: vec![Position {
                    list: ListId::Main,
                    order: 5,
                    pinned: true,
                }],
                last_message: None,
                group_id: 0,
                secret_id: 0,
                forum: false,
                auto_delete: 0,
                perms: crate::model::Rights::default(),
                protected: false,
                reactions_off: false,
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
        app.settings.appearance.layout = Some(crate::config::MessageLayout::Classic);
        app
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) {
        app.handle(Event::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(code, mods),
        )));
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gramtty-ui-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Pictures")).unwrap();
        std::fs::write(dir.join("a.jpg"), [1u8; 2048]).unwrap();
        std::fs::write(dir.join("b.pdf"), [2u8; 100]).unwrap();
        dir
    }

    #[tokio::test]
    async fn composer_attachments_select_reorder_toggle_remove() {
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let dir = scratch("att");
        let mut app = app();
        app.composer.active = true;
        app.attach_paths(vec![dir.join("a.jpg"), dir.join("b.pdf"), dir.join("nope")]);
        assert_eq!(app.composer.attachments.len(), 2);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|(t, e)| *e && t.contains("nope"))
        );
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("a.jpg · 2 KB · photo"), "{screen}");
        assert!(screen.contains("b.pdf · 100 B · file"));
        assert!(screen.contains("Tab attachments"));
        press(&mut app, K::Tab, M::NONE);
        assert_eq!(app.composer.att_sel, Some(1));
        press(&mut app, K::Char('K'), M::SHIFT);
        assert_eq!(app.composer.attachments[0].name, "b.pdf");
        assert_eq!(app.composer.att_sel, Some(0));
        press(&mut app, K::Char('j'), M::NONE);
        press(&mut app, K::Char('f'), M::NONE);
        assert!(app.composer.attachments[1].as_file);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("photo as file"));
        assert!(screen.contains("f as file"));
        press(&mut app, K::Char('d'), M::NONE);
        assert_eq!(app.composer.attachments.len(), 1);
        assert_eq!(app.composer.att_sel, Some(0));
        press(&mut app, K::Esc, M::NONE);
        assert_eq!(app.composer.att_sel, None);
        assert!(app.composer.active);
        press(&mut app, K::Char('h'), M::NONE);
        press(&mut app, K::Left, M::NONE);
        press(&mut app, K::Delete, M::NONE);
        assert_eq!(app.composer.attachments.len(), 1);
        assert_eq!(app.composer.input.text(), "");
        press(&mut app, K::Delete, M::NONE);
        assert!(app.composer.attachments.is_empty());
        app.composer.edit = Some(5);
        app.attach_paths(vec![dir.join("a.jpg")]);
        assert!(app.composer.attachments.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn file_picker_lists_filters_and_attaches() {
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let dir = scratch("picker");
        let mut app = app();
        app.cwd = dir.clone();
        app.composer.active = true;
        press(&mut app, K::Char('o'), M::CONTROL);
        assert!(matches!(app.popup, Some(Popup::FilePicker(_))));
        assert!(app.composer.active);
        app.handle(Event::DirListing {
            dir: dir.clone(),
            result: crate::upload::list_dir(&dir),
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Attach file"));
        assert!(screen.contains("Pictures/"));
        assert!(screen.contains("a.jpg"));
        press(&mut app, K::Char('p'), M::NONE);
        press(&mut app, K::Char('d'), M::NONE);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("b.pdf"));
        assert!(!screen.contains("Pictures/"));
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none());
        assert_eq!(app.composer.attachments.len(), 1);
        assert_eq!(app.composer.attachments[0].path, dir.join("b.pdf"));
        press(&mut app, K::Char('o'), M::CONTROL);
        let Some(Popup::FilePicker(fp)) = &app.popup else {
            panic!("picker not reopened");
        };
        assert_eq!(fp.input.text(), "");
        press(&mut app, K::Char('P'), M::SHIFT);
        app.handle(Event::DirListing {
            dir: dir.clone(),
            result: crate::upload::list_dir(&dir),
        });
        press(&mut app, K::Tab, M::NONE);
        let Some(Popup::FilePicker(fp)) = &app.popup else {
            panic!("picker closed");
        };
        assert_eq!(fp.input.text(), "Pictures/");
        press(&mut app, K::Esc, M::NONE);
        assert!(app.popup.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn sticker_picker_renders_tabs_and_navigates() {
        use crate::stickers::{Data, Item, ItemKind};
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let item = |id: i32, emoji: &str| {
            let mut att = Attachment::new(MediaKind::Sticker, file(id, false));
            att.preview = Some(Visual {
                file: file(id, false),
                decode: Decode::Raster,
                width: 512,
                height: 512,
            });
            Item {
                kind: ItemKind::Sticker,
                file_id: id,
                emoji: emoji.to_owned(),
                width: 512,
                height: 512,
                duration: 0,
                att,
            }
        };
        let mut app = app();
        app.pending = vec![app.keymap.leader];
        press(&mut app, K::Char('s'), M::NONE);
        assert!(matches!(app.popup, Some(Popup::Stickers(_))));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Loading"));
        app.handle(Event::Stickers(Data::Recent(
            (1..=30)
                .map(|i| item(i, if i % 2 == 0 { "🐱" } else { "😀" }))
                .collect(),
        )));
        app.handle(Event::Stickers(Data::Sets(vec![(9, "Cats".into())])));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Recent"));
        assert!(screen.contains("Favorites"));
        assert!(screen.contains("Cats"));
        assert!(screen.contains("Stickers"));
        press(&mut app, K::Down, M::NONE);
        let Some(Popup::Stickers(sp)) = &app.popup else {
            panic!("picker closed");
        };
        assert!(sp.cols > 1);
        assert_eq!(sp.sel, sp.cols);
        press(&mut app, K::Char('🐱'), M::NONE);
        let Some(Popup::Stickers(sp)) = &app.popup else {
            panic!("picker closed");
        };
        assert_eq!(sp.visible(&app.stickers).len(), 15);
        assert!(sp.searching);
        press(&mut app, K::BackTab, M::NONE);
        let Some(Popup::Stickers(sp)) = &app.popup else {
            panic!("picker closed");
        };
        assert_eq!(sp.current_tab(&app.stickers), crate::stickers::Tab::Set(9));
        assert!(app.stickers.sets[0].loading);
        press(&mut app, K::Enter, M::NONE);
        assert!(app.popup.is_none());
        app.composer.active = true;
        press(&mut app, K::Char('g'), M::CONTROL);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("GIFs"));
        press(&mut app, K::BackTab, M::NONE);
        press(&mut app, K::BackTab, M::NONE);
        press(&mut app, K::Char('o'), M::CONTROL);
        assert!(matches!(&app.popup, Some(Popup::Stickers(sp)) if sp.preview));
        render(&mut app, 120, 40);
        press(&mut app, K::Esc, M::NONE);
        assert!(matches!(&app.popup, Some(Popup::Stickers(sp)) if !sp.preview));
        app.popup = Some(Popup::TextSelect {
            input: crate::input::TextInput::with_text("первая строка\nвторая строка"),
            anchor: Some(3),
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Select text") && screen.contains("вторая строка"));
        app.popup = Some(Popup::SharedMedia { chat_id: 10, sel: 1 });
        app.media_counts.insert(
            10,
            vec![(crate::model::MediaFilter::Photos, Some(63))],
        );
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Shared media") && screen.contains("63"), "{screen}");
        assert!(screen.contains("Voice messages"));
    }

    #[test]
    fn recording_indicator_and_upload_progress_render() {
        let mut app = app();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.recording = Some(crate::app::Recording {
            started: std::time::Instant::now() - std::time::Duration::from_secs(7),
            chat_id: 10,
            topic: None,
            reply_to: None,
            stopping: false,
            stop: Some(tx),
            levels: Default::default(),
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Voice message"));
        assert!(screen.contains("REC 0:07"));
        assert!(screen.contains("Enter · Esc"));
        app.recording.as_mut().unwrap().stopping = true;
        assert!(render(&mut app, 120, 40).contains("Encoding"));
        app.recording = None;
        let h = app.histories.get_mut(&10).unwrap();
        let m = h.get_mut(40).unwrap();
        let mut f = file(77, true);
        f.uploading = true;
        f.uploaded = 512;
        let mut att = Attachment::new(MediaKind::Document, f);
        att.name = "big.zip".to_owned();
        m.attachments = vec![att];
        m.state = SendState::Pending;
        m.outgoing = true;
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("big.zip · 2 KB ⇡ 25%"), "{screen}");
        assert!(screen.contains("Uploading…"));
        app.focus = Pane::Messages;
        app.msg_sel = Some(39);
        press(
            &mut app,
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let Some(Popup::Menu { items, .. }) = &app.popup else {
            panic!("no menu");
        };
        assert_eq!(items[0].label, "Cancel sending");
    }

    #[test]
    fn viewer_hint_lists_only_keys_that_do_something() {
        assert_eq!(viewer_hint(false, false, 0, 1), " Esc close ");
        let first = viewer_hint(false, true, 0, 3);
        assert!(
            first.starts_with(" j next · x play/open · s save · o "),
            "{first}"
        );
        assert!(!first.contains("prev"), "{first}");
        let middle = viewer_hint(false, false, 1, 3);
        assert_eq!(middle, " j/k prev/next · Esc close ");
        let last = viewer_hint(true, true, 2, 3);
        assert!(
            last.ends_with("m mute · k prev · x mpv · s save · Esc close "),
            "{last}"
        );
    }

    #[tokio::test]
    async fn delete_popup_shows_only_the_allowed_modes() {
        let mut app = app();
        for (everyone, me) in [(true, false), (false, true), (true, true)] {
            app.popup = Some(Popup::ConfirmDelete {
                chat_id: 10,
                message_ids: vec![1],
                everyone,
                me,
            });
            let screen = render(&mut app, 100, 40);
            assert_eq!(screen.contains("for everyone"), everyone, "{everyone} {me}");
            assert_eq!(screen.contains("only for me"), me, "{everyone} {me}");
        }
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn locate(app: &mut App, w: u16, h: u16, needle: &str) -> Option<(u16, u16)> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let chars: Vec<char> = needle.chars().collect();
        (0..h).find_map(|y| {
            (0..w).find_map(|x| {
                let hit = chars.iter().enumerate().all(|(i, ch)| {
                    let cx = x + i as u16;
                    cx < w && buf[(cx, y)].symbol() == ch.to_string()
                });
                hit.then_some((x, y))
            })
        })
    }

    fn border_left_of(app: &mut App, w: u16, h: u16, needle: &str) -> (u16, u16) {
        let (x, y) = locate(app, w, h, needle).unwrap_or_else(|| panic!("{needle} not found"));
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let border = (0..x)
            .rev()
            .find(|cx| buf[(*cx, y)].symbol() == "│")
            .expect("pane border");
        (x, border)
    }

    #[test]
    fn messages_composer_and_recording_share_left_gutter() {
        let mut app = app();
        app.msg_sel = None;
        let (body_x, pane) = border_left_of(&mut app, 120, 40, "hello there");
        assert_eq!(body_x - pane - 1, MSG_GUTTER as u16);
        let x = |app: &mut App, needle: &str| {
            locate(app, 120, 40, needle)
                .unwrap_or_else(|| panic!("{needle} not found"))
                .0
        };
        assert_eq!(x(&mut app, "Alice Longname 0"), body_x);
        assert_eq!(x(&mut app, "Me 0"), body_x);
        app.msg_sel = Some(39);
        assert_eq!(x(&mut app, "hello there"), body_x);
        app.msg_sel = Some(3);
        assert_eq!(x(&mut app, "│ Alice Longname: "), body_x);
        app.msg_sel = Some(5);
        assert_eq!(x(&mut app, "│ Alice Longname: "), body_x);
        app.msg_sel = None;
        app.composer.active = true;
        app.composer.input = crate::input::TextInput::with_text("draftword");
        let (draft_x, composer_border) = border_left_of(&mut app, 120, 40, "draftword");
        assert_eq!(composer_border, pane + 1);
        assert_eq!(draft_x, body_x);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.recording = Some(crate::app::Recording {
            started: std::time::Instant::now(),
            chat_id: 10,
            topic: None,
            reply_to: None,
            stopping: false,
            stop: Some(tx),
            levels: std::sync::Arc::new(std::sync::Mutex::new(vec![0, 300, 3000, 32_767])),
        });
        assert_eq!(x(&mut app, "REC 0:00") - 2, body_x);
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("REC 0:00  ▁▂▅█ "), "{screen}");
        assert!(screen.contains("Enter · Esc"));
        let screen = render(&mut app, 160, 40);
        assert!(screen.contains("REC 0:00  ▁▂▅█ "), "{screen}");
        assert!(screen.contains("Enter send · Esc cancel"));
    }

    #[test]
    fn recording_line_scrolls_live_tail_and_keeps_hints_right() {
        let flat = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };
        let levels: Vec<u16> = (0..500)
            .map(|i| if i == 499 { 32_767 } else { 0 })
            .collect();
        let line = recording_line(3, &levels, 60);
        let text = flat(&line);
        assert_eq!(text.width(), 60);
        assert!(text.starts_with("○ REC 0:03  ▁"));
        assert!(text.ends_with("█  Enter send · Esc cancel"), "{text}");
        let short = flat(&recording_line(2, &[32_767, 32_767], 60));
        assert_eq!(short.width(), 60);
        assert!(short.starts_with("● REC 0:02  ██ "));
        assert!(short.ends_with("Enter send · Esc cancel"));
        let compact = flat(&recording_line(0, &levels, 36));
        assert_eq!(compact.width(), 36);
        assert!(compact.ends_with("█  Enter · Esc"), "{compact}");
        let narrow = flat(&recording_line(0, &levels, 20));
        assert!(!narrow.contains('▁'));
        assert!(narrow.starts_with("● REC 0:00  Enter send"));
    }

    fn bubble_app() -> App {
        let mut app = app();
        app.settings.appearance.layout = Some(crate::config::MessageLayout::Bubbles);
        app.show_info = false;
        app.show_folders = false;
        app.show_chats = false;
        let now = chrono::Local::now().timestamp();
        let mut h = History::default();
        h.reached_start = true;
        let plain = |id: i64, sender: i64, date: i64, text: &str| {
            let mut m = msg(id, sender, date, text);
            m.body = RichText::plain(text);
            m.edited = false;
            m.reply_to = None;
            m.media = None;
            m
        };
        let mut batch = vec![
            plain(1, 2, now - 2 * 86_400, "old news"),
            plain(2, 2, now - 600, "first from alice"),
            plain(
                3,
                2,
                now - 590,
                "second from alice, which is long enough that it has to wrap inside the bubble at least once or twice",
            ),
            plain(4, 1, now - 500, "mine read"),
            plain(5, 1, now - 400, "mine unread"),
        ];
        batch[2].edited = true;
        h.merge_older(batch);
        app.histories.insert(10, h);
        app.chats.get_mut(&10).unwrap().last_read_outbox = 4;
        app.chats.get_mut(&10).unwrap().unread = 0;
        app
    }

    #[test]
    fn forum_ticks_follow_the_open_topic_read_state() {
        let mut app = bubble_app();
        let chat = app.chats.get_mut(&10).unwrap();
        chat.forum = true;
        chat.last_read_outbox = 0;
        app.topics.entry(10).or_default().upsert(model::Topic {
            id: 1,
            last_read_outbox: 4,
            ..Default::default()
        });
        app.open_topic = Some(1);
        let rows = snapshot(&mut app, 100, 40);
        assert!(rows[row_of(&rows, "mine read")].contains("✓✓"));
        let unread = &rows[row_of(&rows, "mine unread")];
        assert!(!unread.contains("✓✓"), "{unread}");
    }

    fn row_of(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not found in\n{}", rows.join("\n")))
    }

    fn col_of(row: &str, needle: &str) -> usize {
        let at = row.find(needle).unwrap();
        row[..at].chars().count()
    }

    #[test]
    fn bubbles_align_wrap_group_and_tick() {
        let mut app = bubble_app();
        let (w, h) = (100u16, 40u16);
        let rows = snapshot(&mut app, w, h);
        let pane_right = rows[5].chars().count() - 1;
        let mine = row_of(&rows, "mine unread");
        let r = &rows[mine];
        assert!(r.ends_with('│'), "{r}");
        assert!(r.contains("✓") && !r.contains("✓✓"), "{r}");
        let mine_end = r.trim_end_matches('│').trim_end().chars().count();
        assert!(
            pane_right - mine_end <= 2,
            "own bubble is right aligned: {r}"
        );
        assert!(rows[row_of(&rows, "mine read")].contains("✓✓"));
        let first = row_of(&rows, "first from alice");
        let pane_left = 0;
        let text_col = col_of(&rows[first], "first from alice");
        assert_eq!(text_col - pane_left, 1 + AVATAR_COLS as usize + 1 + 1);
        assert_eq!(
            rows.iter().filter(|r| r.contains("Alice Longname")).count(),
            2
        );
        assert_eq!(
            row_of(&rows, "Alice Longname"),
            row_of(&rows, "old news") - 1
        );
        assert!(
            rows[first - 1].contains("Alice Longname"),
            "{}",
            rows.join("\n")
        );
        let second = row_of(&rows, "second from alice");
        assert!(second > first && second - first <= 2, "tightly stacked");
        let wrapped: Vec<&String> = rows[second..]
            .iter()
            .take_while(|r| !r.contains("mine read"))
            .filter(|r| r.trim_matches(|c| c == '│' || c == ' ').len() > 2)
            .collect();
        assert!(wrapped.len() >= 2, "long text wraps");
        let max_w = (w as usize - 2) * BUBBLE_PERCENT / 100;
        for r in &rows[second..second + wrapped.len()] {
            let inner: String = r.chars().skip(1).collect();
            let used = inner
                .trim()
                .trim_end_matches('│')
                .trim_end()
                .chars()
                .count();
            assert!(used <= max_w + AVATAR_COLS as usize + 2, "{r}");
        }
        let edited_row = rows[second..]
            .iter()
            .position(|r| r.contains("edited"))
            .unwrap()
            + second;
        assert!(edited_row < row_of(&rows, "mine read"));
        let avatar_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.starts_with("│ AL ") || r.starts_with("│ AL"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            avatar_rows.len(),
            2,
            "one avatar per run:\n{}",
            rows.join("\n")
        );
        assert_eq!(
            avatar_rows[1],
            edited_row - 1,
            "avatar sits at the bottom of the run"
        );
        if std::env::var_os("GRAMTTY_SHOW").is_some() {
            println!("{}", rows.join("\n"));
        }
        assert!(row_of(&rows, "Today") < first);
        assert!(row_of(&rows, "Today") > row_of(&rows, "old news"));
        let old_label = format::day_label(
            format::Lang::En,
            chrono::Local::now().timestamp() - 2 * 86_400,
            chrono::Local::now().timestamp(),
        );
        assert!(row_of(&rows, &old_label) < row_of(&rows, "old news"));
        let today = &rows[row_of(&rows, "Today")];
        let c = col_of(today, "Today");
        assert!(
            (c as isize - (w as isize / 2)).abs() <= 6,
            "date centered: {today}"
        );
    }

    #[test]
    fn bubble_selection_and_private_chat_without_avatars() {
        let mut app = bubble_app();
        app.chats.get_mut(&10).unwrap().kind = ChatKind::Private(2);
        let rows = snapshot(&mut app, 90, 30);
        assert!(!rows.iter().any(|r| r.contains("Alice Longname")));
        assert!(!rows.iter().any(|r| r.contains(" AL ")));
        let first = row_of(&rows, "first from alice");
        assert_eq!(col_of(&rows[first], "first from alice"), 3);
        let before = rows.clone();
        app.msg_sel = Some(1);
        let rows = snapshot(&mut app, 90, 30);
        let sel = row_of(&rows, "first from alice");
        assert!(
            rows[sel].contains('▌') && rows[sel].contains('▐'),
            "{}",
            rows[sel]
        );
        assert_eq!(rows.len(), before.len());
        assert_eq!(rows.iter().filter(|r| r.contains('▌')).count(), 1);
    }

    #[test]
    fn layout_option_switches_to_classic_and_cache_follows_edits() {
        let mut app = bubble_app();
        let bubbles = snapshot(&mut app, 100, 30);
        assert!(app.layout_cache.len() > 0);
        assert_eq!(snapshot(&mut app, 100, 30), bubbles);
        app.histories.get_mut(&10).unwrap().get_mut(5).unwrap().body =
            RichText::plain("mine changed");
        let edited = snapshot(&mut app, 100, 30);
        assert!(edited.iter().any(|r| r.contains("mine changed")));
        app.chats.get_mut(&10).unwrap().last_read_outbox = 5;
        let read = snapshot(&mut app, 100, 30);
        assert!(read[row_of(&read, "mine changed")].contains("✓✓"));
        app.settings.appearance.layout = Some(crate::config::MessageLayout::Classic);
        let classic = snapshot(&mut app, 100, 30);
        let mine = &classic[row_of(&classic, "mine changed")];
        assert_eq!(col_of(mine, "mine changed"), 1 + MSG_GUTTER);
        assert!(
            mine.trim_end_matches('│').trim_end().ends_with("✓✓"),
            "tick right-aligned: {mine}"
        );
        let header = &classic[row_of(&classic, "mine read") - 1];
        assert!(header.contains("Me ") && header.contains("✓✓"), "{header}");
    }

    #[test]
    fn replies_forwards_pinned_bar_and_jump_indicator() {
        let mut app = bubble_app();
        let h = app.histories.get_mut(&10).unwrap();
        h.get_mut(5).unwrap().reply_to = Some(2);
        h.get_mut(4).unwrap().forward = Some(crate::model::Origin::Name("Bob Hidden".to_owned()));
        let pinned = h.get(2).unwrap().clone();
        app.pinned_msg.insert(10, pinned);
        let rows = snapshot(&mut app, 100, 30);
        let bar = &rows[row_of(&rows, "Pinned:")];
        assert_eq!(row_of(&rows, "Pinned:"), 2);
        assert!(
            bar.contains("Pinned: first from alice") && bar.contains("gp"),
            "{bar}"
        );
        let fwd = row_of(&rows, "Forwarded from Bob Hidden");
        assert_eq!(fwd + 1, row_of(&rows, "mine read"));
        let quote = row_of(&rows, "▎Alice Longname");
        assert_eq!(quote + 1, row_of(&rows, "▎first from alice"));
        assert_eq!(quote + 2, row_of(&rows, "mine unread"));
        assert!(!rows.iter().any(|r| r.contains("↓")));
        app.chats.get_mut(&10).unwrap().unread = 3;
        app.msg_sel = Some(0);
        let rows = snapshot(&mut app, 100, 12);
        assert!(
            rows.iter().any(|r| r.contains("↓ 3 new · G")),
            "{}",
            rows.join("\n")
        );
    }

    fn visible(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < w {
                    let sym = buf[(x, y)].symbol();
                    row.push_str(sym);
                    x += (sym.width() as u16).max(1);
                }
                row
            })
            .collect()
    }

    #[test]
    fn unread_badges_live_in_the_composer_border_and_never_move_content() {
        let mut app = bubble_app();
        app.chats.get_mut(&10).unwrap().mentions = 0;
        let (w, h) = (100u16, 12u16);
        let base = visible(&mut app, w, h);
        assert!(!base.iter().any(|r| r.contains("]r") || r.contains("]m")));
        let border = row_of(&base, " Message ");
        let corner: String = base[border]
            .chars()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        app.chats.get_mut(&10).unwrap().reactions = 2;
        app.chats.get_mut(&10).unwrap().mentions = 1;
        let rows = visible(&mut app, w, h);
        for (y, (a, b)) in base.iter().zip(&rows).enumerate() {
            if y != border {
                assert_eq!(a, b, "row {y} must not change");
            }
        }
        let row = &rows[border];
        assert_eq!(row.width(), w as usize, "{row}");
        assert!(
            row.ends_with(&format!(" ❤️ 2 ]r   @ 1 ]m {corner}")),
            "{row}"
        );
        assert!(row.contains("╭ Message ─"), "{row}");
        let at = col_of(row, "@ 1 ]m");
        let heart = col_of(row, "❤️");

        app.msg_sel = Some(0);
        app.chats.get_mut(&10).unwrap().unread = 3;
        for _ in 0..3 {
            let again = visible(&mut app, w, h);
            assert!(again.iter().any(|r| r.contains("↓ 3 new · G")));
            assert_eq!(row_of(&again, "]m"), border);
            assert_eq!(col_of(&again[border], "@ 1 ]m"), at);
            assert_eq!(col_of(&again[border], "❤️"), heart);
        }
        app.msg_sel = None;
        app.bottom_offset = 0;
        app.chats.get_mut(&10).unwrap().unread = 0;

        app.chats.get_mut(&10).unwrap().reactions = 0;
        let rows = visible(&mut app, w, h);
        assert!(!rows[border].contains("]r"));
        assert_eq!(col_of(&rows[border], "@ 1 ]m"), at);
        app.chats.get_mut(&10).unwrap().mentions = 0;
        assert_eq!(visible(&mut app, w, h), base, "hidden when zero");
    }

    #[test]
    fn unread_badges_drop_rather_than_overlap_on_narrow_composers() {
        let mut app = bubble_app();
        app.chats.get_mut(&10).unwrap().reactions = 12;
        app.chats.get_mut(&10).unwrap().mentions = 3;
        let mut seen = (false, false);
        for w in [14u16, 18, 22, 26, 30, 34] {
            let rows = visible(&mut app, w, 20);
            let border = row_of(&rows, " Message ");
            let row = &rows[border];
            assert_eq!(row.width(), w as usize, "{row}");
            assert!(row.ends_with("╮│"), "corner kept: {row}");
            if row.contains("]r") {
                assert!(row.contains("]m"), "mentions are kept first: {row}");
                seen.1 = true;
            } else if row.contains("]m") {
                seen.0 = true;
            }
        }
        assert_eq!(
            seen,
            (true, true),
            "both the partial and the full set appear"
        );
    }

    fn classic_app() -> App {
        let mut app = bubble_app();
        app.settings.appearance.layout = Some(crate::config::MessageLayout::Classic);
        app.lang = format::Lang::Ru;
        let h = app.histories.get_mut(&10).unwrap();
        h.get_mut(5).unwrap().reply_to = Some(2);
        h.get_mut(4).unwrap().forward = Some(crate::model::Origin::Name("Bob Hidden".to_owned()));
        app
    }

    fn cell_at(app: &mut App, w: u16, h: u16, x: u16, y: u16) -> ratatui::buffer::Cell {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer()[(x, y)].clone()
    }

    #[test]
    fn defaults_are_bubbles_plain_square_and_classic_shows_dates_headers_ticks_and_grouping() {
        assert_eq!(crate::config::Settings::default().appearance.layout, None);
        let d = crate::theme::Theme::default().layout;
        assert_eq!(d.message_layout, crate::config::MessageLayout::Bubbles);
        assert_eq!(d.border_style, crate::config::BorderStyle::Plain);
        assert_eq!(d.avatar_shape, theme::AvatarShape::Square);
        let mut app = classic_app();
        let rows = snapshot(&mut app, 100, 30);
        let today = &rows[row_of(&rows, " Сегодня ")];
        let at = col_of(today, " Сегодня ");
        assert!(today[..today.find(" Сегодня ").unwrap()].ends_with('─'));
        assert!(today.contains(" Сегодня ─"));
        let right = today.chars().count() - at - " Сегодня ".chars().count();
        assert!(at.abs_diff(right) <= 2, "{today}");
        assert!(
            rows.iter().any(|r| r.contains("─ Вчера ─")
                || r.contains(" ноября ")
                || r.contains(" октября "))
        );
        let alice: Vec<&String> = rows
            .iter()
            .filter(|r| r.contains("Alice Longname "))
            .collect();
        assert_eq!(alice.len(), 2, "{}", rows.join("\n"));
        let first = row_of(&rows, "first from alice");
        let header = &rows[first - 1];
        let first_msg = app.histories[&10].get(2).unwrap().date;
        assert!(header.contains(&format!("Alice Longname {}", format::time_label(first_msg))));
        assert!(!rows[first + 1].contains("Alice Longname"));
        let edited_row = row_of(&rows, "изм.");
        assert!(edited_row > first && edited_row < row_of(&rows, "mine read"));
        assert!(!rows[edited_row].contains("Alice"));
        let me = row_of(&rows, "Me ");
        let mine_date = app.histories[&10].get(4).unwrap().date;
        assert!(
            rows[me].contains(&format!("Me {} ✓✓", format::time_label(mine_date))),
            "{}",
            rows[me]
        );
        assert_eq!(row_of(&rows, "↪ Переслано от Bob Hidden"), me + 1);
        assert_eq!(me + 2, row_of(&rows, "mine read"));
        let strip = row_of(&rows, "│ Alice Longname: first from alice");
        assert_eq!(strip + 1, row_of(&rows, "mine unread"));
        let unread = rows[strip + 1].trim_end_matches('│').trim_end();
        assert!(unread.ends_with('✓') && !unread.contains("✓✓"), "{unread}");
        assert!(
            !unread.contains("mine unread ✓"),
            "tick is not glued: {unread}"
        );
        assert_eq!(rows.iter().filter(|r| r.contains("Me ")).count(), 1);
        let x = |row: &str, needle: &str| col_of(row, needle);
        assert_eq!(
            x(&rows[row_of(&rows, "mine read")], "mine read"),
            1 + MSG_GUTTER
        );
        assert_eq!(x(&rows[first], "first from alice"), 1 + MSG_GUTTER);
        assert_eq!(x(header, "Alice Longname"), 1 + MSG_GUTTER);
    }

    #[test]
    fn classic_has_no_background_blocks_and_avatar_only_in_groups() {
        let mut app = classic_app();
        let rows = snapshot(&mut app, 100, 30);
        let first = row_of(&rows, "first from alice") as u16;
        let col = (1 + MSG_GUTTER) as u16;
        for dx in 0..20 {
            let c = cell_at(&mut app, 100, 30, col + dx, first);
            assert_eq!(c.bg, Color::Reset, "body cell {dx} has a background");
        }
        let header = first - 1;
        let avatar = cell_at(&mut app, 100, 30, 1, header);
        assert_eq!(avatar.symbol(), "A");
        assert_eq!(Some(avatar.bg), theme::get().name_color(2));
        let me = row_of(&rows, "Me ") as u16;
        assert_eq!(cell_at(&mut app, 100, 30, 1, me).symbol(), "M");
        app.chats.get_mut(&10).unwrap().kind = ChatKind::Private(2);
        let rows = snapshot(&mut app, 100, 30);
        let header = row_of(&rows, "Alice Longname ") as u16;
        let c = cell_at(&mut app, 100, 30, 1, header);
        assert_eq!(c.symbol(), " ");
        assert_eq!(c.bg, Color::Reset);
    }

    #[test]
    fn classic_pinned_bar_jump_indicator_and_selection() {
        let mut app = classic_app();
        let pinned = app.histories[&10].get(2).unwrap().clone();
        app.pinned_msg.insert(10, pinned);
        let rows = snapshot(&mut app, 100, 30);
        assert_eq!(row_of(&rows, "Закреплённое: first from alice"), 2);
        assert!(!rows.iter().any(|r| r.contains("↓")));
        app.chats.get_mut(&10).unwrap().unread = 3;
        app.msg_sel = Some(0);
        let rows = snapshot(&mut app, 100, 12);
        assert!(
            rows.iter().any(|r| r.contains("↓ 3 новых · G")),
            "{}",
            rows.join("\n")
        );
        app.msg_sel = Some(2);
        let rows = snapshot(&mut app, 100, 30);
        let sel = row_of(&rows, "second from alice");
        assert_eq!(rows[sel].chars().nth(1), Some('▎'), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("╭─")), "no selection box");
        for (w, h) in [(30u16, 8u16), (50, 12), (80, 24), (200, 60)] {
            for sel in [None, Some(0), Some(3), Some(4)] {
                app.msg_sel = sel;
                snapshot(&mut app, w, h);
            }
        }
    }

    #[test]
    fn border_style_option_reaches_every_bordered_widget() {
        use crate::config::BorderStyle;
        let mut app = classic_app();
        app.show_folders = true;
        app.show_chats = true;
        for (style, tl, br) in [
            (BorderStyle::Rounded, '╭', '╯'),
            (BorderStyle::Plain, '┌', '┘'),
            (BorderStyle::Thick, '┏', '┛'),
            (BorderStyle::Double, '╔', '╝'),
        ] {
            app.settings.appearance.border_style = Some(style);
            app.msg_sel = None;
            app.popup = None;
            let rows = snapshot(&mut app, 120, 30);
            assert!(
                rows[1].starts_with(&format!("{tl} Folders")),
                "{style:?}: {}",
                rows[1]
            );
            assert!(rows[1].contains(&format!("{tl} All chats")));
            assert!(rows[1].contains(&format!("{tl} Rust чат")));
            assert!(rows.iter().any(|r| r.contains(&format!("{tl} Message"))));
            assert!(rows.last().unwrap().ends_with(br));
            app.msg_sel = Some(2);
            let screen = render(&mut app, 120, 30);
            assert_eq!(screen.matches(tl).count(), 4, "{style:?} no selection box");
            assert!(screen.contains('▎'), "{style:?} selection bar");
            app.msg_sel = None;
            app.popup = Some(Popup::Options { sel: 5 });
            let rows = snapshot(&mut app, 120, 30);
            let opt = &rows[row_of(&rows, " Options ")];
            assert!(opt.contains(&format!("{tl} Options")), "{opt}");
            assert!(
                rows.iter()
                    .any(|r| r.contains(&format!("2 Border style: {}", style.name())))
            );
            app.popup = None;
            app.pending = vec![app.keymap.leader];
            let rows = snapshot(&mut app, 120, 30);
            assert!(
                rows.iter().any(|r| r.contains(&format!("{tl} Leader"))),
                "{style:?} leader"
            );
            app.pending.clear();
        }
    }

    #[test]
    fn options_popup_shows_groups_hotkeys_and_full_description() {
        let mut app = bubble_app();
        app.popup = Some(Popup::Options {
            sel: crate::app::OPEN_FOCUS_OPTION,
        });
        let rows = snapshot(&mut app, 70, 44);
        let text = rows.join("\n");
        for head in [
            "Notifications",
            "Privacy",
            "Appearance",
            "Chat list",
            "Animations",
            "Messages",
            "Navigation",
        ] {
            assert!(text.contains(head), "{head}\n{text}");
        }
        for (i, (label, _)) in crate::app::OPTIONS.iter().enumerate() {
            app.popup = Some(Popup::Options { sel: i });
            let rows = snapshot(&mut app, 70, 44);
            let text = rows.join("\n");
            let line = format!("{} ", crate::app::option_key(i));
            assert!(
                rows.iter()
                    .any(|r| r.contains(&line) && r.contains(label.split(':').next().unwrap())),
                "{label}\n{text}"
            );
        }
        let flat: String = rows
            .iter()
            .map(|r| r.trim_matches(|c: char| c == '│' || c == ' '))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            flat.contains(
                "Smart: composer if unread fit on screen, else first unread; composer; messages"
            ),
            "{text}"
        );
        assert!(!text.contains('…'), "{text}");
        let rows = snapshot(&mut app, 70, 18);
        assert!(rows.iter().any(|r| r.contains("Options")));
    }

    #[test]
    fn options_popup_keeps_its_rect_for_every_entry() {
        let mut app = bubble_app();
        for (w, h) in [(100u16, 40u16), (70, 30), (60, 24)] {
            let mut first = None;
            for sel in 0..crate::app::OPTIONS.len() {
                app.popup = Some(Popup::Options { sel });
                let rows = snapshot(&mut app, w, h);
                let top = row_of(&rows, " Options ");
                let bottom = row_of(&rows, "j/k move");
                let left = col_of(&rows[top], "╭ Options");
                let right = rows[top].chars().count() - 1;
                let entry = (h >= 40).then(|| row_of(&rows, "Notifications"));
                let (label, _) = crate::app::OPTIONS[sel];
                assert!(
                    rows[top..bottom].iter().any(|r| r.contains(label)),
                    "{w}x{h} sel {sel} not visible\n{}",
                    rows.join("\n")
                );
                let rect = (top, bottom, left, right, entry);
                match first {
                    None => first = Some(rect),
                    Some(f) => assert_eq!(rect, f, "{w}x{h} sel {sel}\n{}", rows.join("\n")),
                }
            }
        }
    }

    #[test]
    fn bubbles_render_at_many_sizes() {
        let mut app = bubble_app();
        app.show_info = true;
        app.show_chats = true;
        for (w, h) in [(30u16, 8u16), (50, 12), (80, 24), (200, 60), (36, 40)] {
            for sel in [None, Some(0), Some(4)] {
                app.msg_sel = sel;
                snapshot(&mut app, w, h);
            }
        }
    }

    fn snapshot(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn leader_popup_groups_columns_and_wraps() {
        let mut app = app();
        app.pending = vec![app.keymap.leader];
        let wide = snapshot(&mut app, 160, 40);
        let inner = |rows: &[String]| -> Vec<String> {
            let top = rows.iter().position(|l| l.contains("╭ Leader")).unwrap();
            rows[top..]
                .iter()
                .take_while(|l| !l.is_empty())
                .map(|l| {
                    let start = l.find("╭ Leader").or_else(|| l.find('│')).unwrap_or(0);
                    l[start..].to_owned()
                })
                .collect()
        };
        let w = inner(&wide);
        assert!(
            w[1].contains("Navigate")
                && w[1].contains("Compose")
                && w[1].contains("Panes")
                && w[1].contains("Account"),
            "{w:#?}"
        );
        let gap = |line: &str, a: &str, b: &str| {
            let (i, j) = (line.find(a).unwrap(), line.find(b).unwrap());
            i < j && line[i + a.len()..j].trim().is_empty()
        };
        assert!(
            gap(&w[2], "[Space]  find chat", "[u]  attach file"),
            "{w:#?}"
        );
        assert!(
            gap(&w[3], "[a]      actions", "[v]  record voice message"),
            "{w:#?}"
        );
        assert!(w[4].contains("[p]      profile"), "{w:#?}");
        assert!(
            w[2].trim_end_matches(['│', ' ', '╯', '╮'])
                .ends_with("[?]  all key bindings"),
            "{w:#?}"
        );
        assert!(
            w[3].trim_end_matches(['│', ' ', '╯', '╮'])
                .ends_with("[o]  options"),
            "{w:#?}"
        );
        assert!(
            w[4].trim_end_matches(['│', ' ', '╯', '╮'])
                .ends_with("[L]  log out"),
            "{w:#?}"
        );
        assert_eq!(wide.last().map(|l| l.contains('╰')), Some(true));
        let bottom = &wide[wide.len() - 2];
        assert!(bottom.contains("╰ Esc cancel ─"), "{wide:#?}");
        assert!(
            !wide.last().unwrap().contains("Esc cancel"),
            "the popup sits above the pane borders instead of cutting through them"
        );
        let narrow = snapshot(&mut app, 70, 40);
        let n = inner(&narrow);
        assert!(
            n[1].contains("Navigate") && n[1].contains("Compose") && !n[1].contains("Panes"),
            "{n:#?}"
        );
        let panes = n.iter().position(|l| l.contains("Panes")).unwrap();
        assert!(n[panes - 1].trim_matches(['│', ' ']).is_empty());
        assert!(n[panes].contains("Account"));
        assert!(n[panes + 4].contains("[r]  redraw"), "{n:#?}");
        let buf_row = |app: &mut App| {
            let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
            term.draw(|f| draw(f, app)).unwrap();
            let buf = term.backend().buffer().clone();
            let (x, y) = (0..40u16)
                .flat_map(|y| (0..160u16).map(move |x| (x, y)))
                .find(|(x, y)| {
                    buf[(*x, *y)].symbol() == "l"
                        && buf[(*x + 1, *y)].symbol() == "o"
                        && buf[(*x + 4, *y)].symbol() == "o"
                        && buf[(*x + 5, *y)].symbol() == "u"
                })
                .unwrap();
            buf[(x, y)].fg
        };
        assert_eq!(buf_row(&mut app), Color::Red);
    }

    fn popup_box(rows: &[String], title: &str) -> Vec<String> {
        let corner = format!("╭ {title} ");
        let top = rows.iter().position(|l| l.contains(&corner)).unwrap();
        let byte = rows[top].find(&corner).unwrap();
        let left = rows[top][..byte].chars().count();
        let right = rows[top]
            .chars()
            .skip(left + 1)
            .position(|c| c == '╮')
            .unwrap()
            + left
            + 1;
        let cut = |l: &String| -> String { l.chars().skip(left).take(right - left + 1).collect() };
        let end = rows[top..]
            .iter()
            .position(|l| cut(l).starts_with('╰'))
            .unwrap();
        rows[top..=top + end].iter().map(cut).collect()
    }

    #[test]
    fn short_prefix_hints_fit_their_items_and_say_how_to_cancel() {
        let mut app = app();
        app.pending = vec![crate::keymap::Chord::new(
            crossterm::event::KeyCode::Char(']'),
            crossterm::event::KeyModifiers::NONE,
        )];
        let rows = snapshot(&mut app, 120, 30);
        let hint = popup_box(&rows, "]");
        let width = hint[0].chars().count();
        assert!(width < 40, "{hint:#?}");
        assert!(
            hint.iter()
                .any(|l| l.contains("[m]  next unread mention or reply")),
            "{hint:#?}"
        );
        assert!(hint.last().unwrap().contains("Esc cancel"), "{hint:#?}");
        let bottom = rows.iter().rposition(|l| l.contains("Esc cancel")).unwrap();
        assert_eq!(bottom, rows.len() - 2, "one row above the pane borders");
        assert!(!rows.last().unwrap().contains("╰╰"), "{rows:#?}");
    }

    #[test]
    fn long_menus_scroll_fit_their_labels_and_name_their_keys() {
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let mut app = app();
        app.focus = Pane::Messages;
        app.msg_sel = Some(39);
        press(&mut app, K::Enter, M::NONE);
        let labels: Vec<&str> = match &app.popup {
            Some(Popup::Menu { items, .. }) => items.iter().map(|i| i.label).collect(),
            _ => panic!("no menu"),
        };
        let n = labels.len();
        let rows = snapshot(&mut app, 100, 12);
        let menu = popup_box(&rows, "Message actions");
        assert_eq!(menu.len(), 10, "{menu:#?}");
        assert!(
            menu.last()
                .unwrap()
                .contains(&format!("1/{n} · Enter run · Esc close")),
            "{menu:#?}"
        );
        let mark = labels
            .iter()
            .position(|l| l.starts_with("Mark / unmark"))
            .unwrap();
        for _ in 0..mark {
            press(&mut app, K::Char('j'), M::NONE);
        }
        let menu = popup_box(&snapshot(&mut app, 100, 12), "Message actions");
        assert!(
            menu.iter()
                .any(|l| l.contains("m Mark / unmark (select several) ")),
            "{menu:#?}"
        );
        for _ in mark..n - 1 {
            press(&mut app, K::Char('j'), M::NONE);
        }
        let menu = popup_box(&snapshot(&mut app, 100, 12), "Message actions");
        assert!(
            menu[menu.len() - 2].contains(labels[n - 1]),
            "the last item is on screen when selected: {menu:#?}"
        );
        assert!(
            menu.last().unwrap().contains(&format!("{n}/{n}")),
            "{menu:#?}"
        );
        let tall = snapshot(&mut app, 100, 50);
        let menu = popup_box(&tall, "Message actions");
        assert_eq!(menu.len(), n + 2, "everything fits, nothing to scroll");
        assert!(menu.last().unwrap().contains("╰ Enter run · Esc close"));
    }

    #[test]
    fn help_popup_lists_bindings_by_context_and_scrolls() {
        use crossterm::event::{KeyCode as K, KeyModifiers as M};
        let mut app = app();
        press(&mut app, K::Char('?'), M::NONE);
        let rows = snapshot(&mut app, 100, 30);
        let help = popup_box(&rows, "Key bindings");
        assert_eq!(help.len(), 28, "{help:#?}");
        let text = help.join("\n");
        for want in [
            " Global",
            "?, Space ?",
            "all key bindings",
            "ShowKeyBindings",
            "Space Space",
            "j, ↓",
            "action names for keymap.toml",
            "j/k scroll · ? or Esc close",
        ] {
            assert!(text.contains(want), "{want}: {help:#?}");
        }
        press(&mut app, K::Char('G'), M::NONE);
        let help = popup_box(&snapshot(&mut app, 100, 30), "Key bindings");
        let text = help.join("\n");
        assert!(
            text.contains("text selection: move, select, copy"),
            "{help:#?}"
        );
        assert!(text.contains(" Popups, menus and search"), "{help:#?}");
        let narrow = popup_box(&snapshot(&mut app, 60, 20), "Key bindings");
        assert!(narrow[0].chars().count() <= 58, "{narrow:#?}");
        assert!(
            !narrow.join("\n").contains("ShowKeyBindings"),
            "action names give way on narrow screens: {narrow:#?}"
        );
    }

    #[test]
    fn read_only_chats_show_why_in_place_of_the_composer_hint() {
        let mut app = app();
        assert!(render(&mut app, 120, 30).contains("press i to write"));
        let c = app.chats.get_mut(&10).unwrap();
        c.kind = ChatKind::Channel;
        c.group_id = 7;
        app.groups.insert(
            7,
            crate::model::GroupMeta {
                forum: false,
                status: crate::model::Membership::Member,
            },
        );
        let screen = render(&mut app, 120, 30);
        assert!(
            screen.contains("only admins can post in this channel"),
            "{screen}"
        );
        assert!(!screen.contains("to write"));
        app.groups.get_mut(&7).unwrap().status = crate::model::Membership::Left;
        assert!(render(&mut app, 120, 30).contains("press i to join"));
    }

    #[test]
    fn chat_removal_confirmations_name_the_chat_and_the_choices() {
        let mut app = app();
        app.popup = Some(Popup::ConfirmRemoveChat {
            chat_id: 10,
            leave: true,
            revoke: false,
        });
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Leave Rust чат?"), "{screen}");
        assert!(screen.contains(" y leave"));
        assert!(!screen.contains("Enter"));
        app.chats.insert(
            20,
            ChatInfo {
                id: 20,
                title: "Alice Longname".to_owned(),
                kind: ChatKind::Private(2),
                ..app.chats[&10].clone()
            },
        );
        app.popup = Some(Popup::ConfirmRemoveChat {
            chat_id: 20,
            leave: false,
            revoke: true,
        });
        let screen = render(&mut app, 100, 30);
        assert!(
            screen.contains("Delete the chat with Alice Longname?"),
            "{screen}"
        );
        assert!(screen.contains(" y for me and Alice Longname"), "{screen}");
        assert!(screen.contains(" m only for me (Enter)"), "{screen}");
        app.popup = Some(Popup::ConfirmRemoveChat {
            chat_id: 20,
            leave: false,
            revoke: false,
        });
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains(" y delete (Enter)"), "{screen}");
        assert!(!screen.contains("for me and"), "{screen}");
    }

    #[test]
    fn info_pane_wraps_long_values_instead_of_clipping() {
        let mut app = app();
        app.settings.appearance.show_ids = true;
        let rows = snapshot(&mut app, 140, 40);
        let at = rows
            .iter()
            .position(|l| l.contains("Images: halfblocks"))
            .unwrap();
        assert!(rows[at + 1].contains("cached"), "{rows:#?}");
    }

    #[test]
    fn stage4_message_decorations_and_header_render() {
        use crate::bots::{Button, ButtonAction, Command, Keyboard};
        let mut app = app();
        app.settings.privacy.ghost_mode = true;
        app.unread_counts.insert(ListId::Main, 42);
        app.chats.insert(
            20,
            ChatInfo {
                id: 20,
                title: "Alice DM".to_owned(),
                kind: ChatKind::Private(2),
                unread: 0,
                mentions: 0,
                reactions: 0,
                last_read_inbox: 0,
                last_read_outbox: 0,
                photo: None,
                muted: false,
                use_default_mute: false,
                positions: vec![Position {
                    list: ListId::Main,
                    order: 9,
                    pinned: false,
                }],
                last_message: None,
                group_id: 0,
                secret_id: 0,
                forum: false,
                auto_delete: 0,
                perms: crate::model::Rights::default(),
                protected: false,
                reactions_off: false,
            },
        );
        app.user_meta.insert(
            2,
            crate::model::UserMeta {
                status: crate::presence::Status::Online {
                    expires: crate::app::unix_now() + 100,
                },
                ..Default::default()
            },
        );
        app.typing.insert(
            10,
            vec![crate::presence::Typing::new(
                Sender::User(2),
                crate::presence::Activity::Typing,
                None,
            )],
        );
        let h = app.histories.get_mut(&10).unwrap();
        let m = h.get_mut(40).unwrap();
        m.pinned = true;
        m.reactions = vec![crate::model::Reaction {
            label: "❤".to_owned(),
            emoji: Some("❤".to_owned()),
            custom: 0,
            count: 3,
            chosen: true,
        }];
        m.keyboard = Some(Keyboard {
            inline: true,
            rows: vec![vec![
                Button {
                    text: "Approve".to_owned(),
                    action: ButtonAction::Callback("YQ==".to_owned()),
                },
                Button {
                    text: "Docs".to_owned(),
                    action: ButtonAction::Url("https://example.org".to_owned()),
                },
            ]],
            placeholder: String::new(),
        });
        let screen = render(&mut app, 140, 40);
        assert!(
            screen.contains("👻") && screen.contains(" ghost "),
            "{screen}"
        );
        assert!(!screen.contains("Rust чат · Alice"), "{screen}");
        assert!(screen.contains(" Message · Alice is typing… "), "{screen}");
        assert!(screen.contains("Approve"));
        assert!(screen.contains("Docs ↗"));
        assert!(screen.contains("❤\u{FE0F}  3•"));
        assert!(screen.contains("📌"));
        assert!(screen.contains("All chats"));
        assert!(screen.contains("42"));
        assert!(screen.contains("● Alice DM"));
        app.settings.privacy.ghost_mode = false;
        app.settings.privacy.dont_mark_read = true;
        assert!(render(&mut app, 140, 40).contains("◌ no-read"));
        app.open_chat = Some(20);
        app.histories.insert(20, History::default());
        app.typing.clear();
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains("Alice DM · online"), "{screen}");
        assert!(screen.contains("Status: online"));
        app.bot_commands.insert(
            20,
            vec![Command {
                name: "start".to_owned(),
                description: "Start the bot".to_owned(),
                bot: "abot".to_owned(),
            }],
        );
        app.composer.active = true;
        app.composer.input = crate::input::TextInput::with_text("/st");
        let screen = render(&mut app, 140, 40);
        assert!(screen.contains("Commands"));
        assert!(screen.contains("/start Start the bot"));
    }

    #[test]
    fn avatars_load_photos_in_bubbles_and_profile() {
        let fixture =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".work/fixtures/photo.jpg");
        let mut app = bubble_app();
        snapshot(&mut app, 100, 30);
        assert!(!app.media.has_work());
        app.user_meta.entry(2).or_default().photo = Some(crate::model::FileRef {
            id: 501,
            ..crate::model::FileRef::default()
        });
        snapshot(&mut app, 100, 30);
        assert!(
            app.media.has_work(),
            "missing avatar is requested for download"
        );
        let mut app = bubble_app();
        app.user_meta.entry(2).or_default().photo = Some(crate::model::FileRef {
            id: 502,
            path: fixture.to_string_lossy().into_owned(),
            done: true,
            ..crate::model::FileRef::default()
        });
        let rows = snapshot(&mut app, 100, 30);
        assert!(app.media.has_work(), "downloaded avatar is decoded");
        assert!(
            rows.iter().any(|r| r.contains(" AL ")),
            "initials stay as fallback"
        );
        let mut app = super::tests::app();
        app.popup = Some(Popup::Profile(crate::app::ProfileView {
            target: crate::model::ProfileTarget::User(2),
            data: Some(crate::model::Profile {
                title: "Alice Longname".to_owned(),
                id: 2,
                user_id: Some(2),
                ..Default::default()
            }),
            error: None,
            scroll: 0,
        }));
        let rows = snapshot(&mut app, 120, 40);
        let title = row_of(&rows, "Alice Longname");
        assert!(col_of(&rows[title], "Alice Longname") > col_of(&rows[title], "│") + 10);
        assert!(
            rows.iter().any(|r| r.contains("    AL    ")),
            "{}",
            rows.join("\n")
        );
        assert!(!app.media.has_work());
        app.user_meta.entry(2).or_default().photo = Some(crate::model::FileRef {
            id: 503,
            path: fixture.to_string_lossy().into_owned(),
            done: true,
            ..crate::model::FileRef::default()
        });
        snapshot(&mut app, 120, 40);
        assert!(app.media.has_work(), "profile avatar is decoded");
    }

    #[test]
    fn stage4_popups_render() {
        use crate::app::{ButtonsView, ProfileView, ReactionPicker, SearchView};
        use crate::bots::{Button, ButtonAction, Keyboard};
        let mut app = app();
        app.popup = Some(Popup::Profile(ProfileView {
            target: crate::model::ProfileTarget::User(2),
            data: None,
            error: None,
            scroll: 0,
        }));
        assert!(render(&mut app, 120, 40).contains("Loading profile"));
        app.user_meta.insert(
            2,
            crate::model::UserMeta {
                status: crate::presence::Status::Recently,
                ..Default::default()
            },
        );
        app.popup = Some(Popup::Profile(ProfileView {
            target: crate::model::ProfileTarget::User(2),
            data: Some(crate::model::Profile {
                title: "Alice Longname".to_owned(),
                kind: "user".to_owned(),
                id: 2,
                user_id: Some(2),
                usernames: vec!["alice".to_owned(), "al".to_owned()],
                phone: "+79990000000".to_owned(),
                bio: "Rustacean and tea lover".to_owned(),
                common_chats: Some(3),
                members: None,
                links: vec!["https://t.me/alice".to_owned()],
                commands: Vec::new(),
            }),
            error: None,
            scroll: 0,
        }));
        let screen = render(&mut app, 120, 40);
        for want in [
            "Profile",
            "@alice · @al",
            "last seen recently · user",
            "Rustacean and tea lover",
            "+79990000000",
            "Common chats 3",
            "https://t.me/alice",
        ] {
            assert!(screen.contains(want), "{want}: {screen}");
        }
        app.popup = Some(Popup::Options { sel: 0 });
        app.settings.privacy.ghost_mode = true;
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("[x] Desktop notifications"));
        app.popup = Some(Popup::Options { sel: 2 });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("[ ] Don't mark as read"));
        assert!(screen.contains("[x] Ghost mode"));
        assert!(screen.contains("Never online, no typing status"));
        let found = |id: i64| {
            let mut m = msg(id, 2, 1_700_000_000, "needle here");
            m.chat_id = 10;
            m
        };
        app.popup = Some(Popup::Search(SearchView {
            chat_id: None,
            pinned: false,
            sender: None,
            media: None,
            input: crate::input::TextInput::with_text("needle"),
            searched_for: Some("needle".to_owned()),
            results: vec![found(1), found(2)],
            sel: 1,
            token: 1,
            loading: false,
            error: None,
            browse: false,
            due: None,
            next: None,
            total: 2,
            paging: false,
            follow: false,
        }));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Search all chats"));
        assert!(screen.contains("Rust чат · Alice Longname: needle here"));
        let mut hit = found(3);
        hit.body = RichText::plain("learning Rust today");
        let mut view = crate::search::FindView::new(crate::search::Scope::All);
        view.input = crate::input::TextInput::with_text("rust");
        view.sent = Some("rust".to_owned());
        view.peers = Some(crate::td::Peers::default());
        view.msgs = vec![hit];
        view.total = 12;
        app.popup = Some(Popup::Find(view));
        let screen = render(&mut app, 120, 40);
        for want in [
            "Chats and contacts",
            "Rust чат · group",
            "Messages · 12 found",
            "Rust чат · Alice Longname: learning Rust today",
        ] {
            assert!(screen.contains(want), "{want}: {screen}");
        }
        let buf = buffer(&mut app, 120, 40);
        let (x, y) = find(&buf, "Rust today");
        let mods = |x: u16| buf[(x, y)].modifier;
        assert!(mods(x).contains(ratatui::style::Modifier::UNDERLINED));
        assert!(!mods(x + 5).contains(ratatui::style::Modifier::UNDERLINED));
        for (w, h) in [(40, 12), (20, 6), (1, 1)] {
            render(&mut app, w, h);
        }
        if let Some(Popup::Find(v)) = &mut app.popup {
            v.input = crate::input::TextInput::with_text("zzz");
            v.sent = Some("zzz".to_owned());
            v.msgs.clear();
        }
        assert!(render(&mut app, 120, 40).contains("No results for “zzz”"));
        app.inbox.push_front(crate::model::InboxItem {
            chat_id: 10,
            msg_id: 1,
            sender: Sender::User(2),
            preview: "ping".to_owned(),
            date: 1_700_000_000,
            mention: true,
            topic: None,
        });
        app.popup = Some(Popup::Inbox { sel: 0 });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Notifications"));
        assert!(screen.contains("@ Rust чат · Alice Longname: ping"));
        app.popup = Some(Popup::Reactions(ReactionPicker {
            chat_id: 10,
            msg_id: 40,
            emojis: vec!["👍".to_owned(), "🔥".to_owned()],
            sel: 1,
            loading: false,
            note: None,
        }));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("React"));
        assert!(screen.contains("🔥"));
        app.popup = Some(Popup::Buttons(ButtonsView {
            chat_id: 10,
            msg_id: 40,
            keyboard: Keyboard {
                inline: false,
                rows: vec![vec![
                    Button {
                        text: "Yes".to_owned(),
                        action: ButtonAction::Text,
                    },
                    Button {
                        text: "Phone".to_owned(),
                        action: ButtonAction::Unsupported("sharing your phone number"),
                    },
                ]],
                placeholder: "Choose".to_owned(),
            },
            sel: 0,
        }));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Keyboard"));
        assert!(screen.contains("1 Yes"), "{screen}");
        assert!(screen.contains("│ 2 Phone ✕ │"), "{screen}");
        assert!(screen.contains("Choose"));
        app.popup = Some(Popup::ConfirmUrl("https://example.org/x".to_owned()));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Open this link"));
        assert!(screen.contains("https://example.org/x"));
        app.popup = Some(Popup::ReactionList {
            chat_id: 10,
            msg_id: 40,
            items: Some(vec![(Sender::User(2), "👍".to_owned())]),
            sel: 0,
            read: false,
        });
        assert!(render(&mut app, 120, 40).contains("Alice Longname"));
        app.popup = Some(Popup::ReactionList {
            chat_id: 10,
            msg_id: 40,
            items: Some(vec![(Sender::User(2), "10.11 12:30".to_owned())]),
            sel: 0,
            read: true,
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Read by") && screen.contains("10.11 12:30  Alice Longname"));
        app.popup = Some(Popup::Switcher {
            input: crate::input::TextInput::default(),
            sel: 0,
            forward: Some(crate::app::Forward {
                from: 10,
                ids: vec![40],
                targets: Vec::new(),
                hide_sender: false,
            }),
        });
        assert!(render(&mut app, 120, 40).contains("Forward to"));
        for (w, h) in [(40, 12), (80, 24), (200, 60)] {
            for popup in [
                Popup::Options { sel: 0 },
                Popup::Inbox { sel: 0 },
                Popup::ConfirmUrl("u".to_owned()),
            ] {
                app.popup = Some(popup);
                render(&mut app, w, h);
            }
        }
    }

    #[test]
    fn link_popups_render() {
        let mut app = app();
        let link = |url: &str, text: &str, hidden: bool| format::Link {
            url: url.to_owned(),
            hidden,
            text: text.to_owned(),
        };
        let links = || {
            vec![
                link("https://t.me/rustlang", "@rustlang", false),
                link("tg://user?id=2", "Alice", false),
                link("https://evil.example/x", "docs", true),
            ]
        };
        app.popup = Some(Popup::Links {
            links: links(),
            sel: 1,
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Open link"), "{screen}");
        assert!(screen.contains("1 @rustlang"), "{screen}");
        assert!(screen.contains("2 Alice"), "{screen}");
        assert!(
            screen.contains("3 docs  https://evil.example/x"),
            "{screen}"
        );
        app.popup = Some(Popup::ConfirmStart {
            chat_id: 10,
            bot_id: 2,
            parameter: "ref42".to_owned(),
        });
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Start Rust чат?"), "{screen}");
        assert!(screen.contains("ref42"), "{screen}");
        assert!(screen.contains("y start"), "{screen}");
        app.popup = Some(Popup::ConfirmJoin(crate::td::Invite {
            link: "https://t.me/+abc".to_owned(),
            title: "Rustaceans".to_owned(),
            channel: true,
            members: 1234,
            request: true,
        }));
        let screen = render(&mut app, 120, 40);
        assert!(screen.contains("Join Rustaceans?"), "{screen}");
        assert!(screen.contains("Channel · 1.2K subscribers"), "{screen}");
        assert!(screen.contains("approve"), "{screen}");
        assert!(screen.contains("y send request"), "{screen}");
        for (w, h) in [(30, 8), (80, 24), (200, 60), (4, 3)] {
            for popup in [
                Popup::Links {
                    links: links(),
                    sel: 2,
                },
                Popup::ConfirmStart {
                    chat_id: 99,
                    bot_id: 2,
                    parameter: String::new(),
                },
            ] {
                app.popup = Some(popup);
                render(&mut app, w, h);
            }
        }
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
        assert!(screen.contains("Log out of Telegram?"));
        app.popup = Some(Popup::Switcher {
            input: crate::input::TextInput::with_text("rust"),
            sel: 0,
            forward: None,
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
            ..crate::model::FileRef::default()
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
            cost: 1,
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
                shape: None,
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
                shape: None,
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

    #[cfg(target_os = "linux")]
    fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
            .unwrap()
    }

    #[cfg(target_os = "linux")]
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
            crate::td::Td::recording(tx).0,
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
                ..crate::model::FileRef::default()
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
        app.auth = Auth::Qr(Some(format!("tg://login?token={}", "A".repeat(43))));
        let small = render(&mut app, 80, 24);
        assert!(small.contains("Link Desktop Device"), "{small}");
        assert!(small.contains("log in with phone number"), "{small}");
        assert!(!small.contains("too small"), "{small}");
    }

    #[test]
    fn login_screens_show_keys_progress_and_wrap_long_messages() {
        let mut app = app();
        app.auth = Auth::Email;
        assert!(render(&mut app, 100, 30).contains("Enter: send code · Esc: change number"));
        app.auth = Auth::EmailCode {
            pattern: "a***@x.com".to_owned(),
        };
        let text = render(&mut app, 100, 30);
        assert!(
            text.contains("Enter: confirm · Esc: change number · <C-c>: quit"),
            "{text}"
        );
        assert!(!text.contains("Checking"));
        app.login_busy = true;
        assert!(render(&mut app, 100, 30).contains("Checking…"));
        app.auth = Auth::Phone;
        assert!(render(&mut app, 100, 30).contains("Sending the code…"));
        app.login_busy = false;
        app.auth = Auth::Blocked(
            "This phone number has no Telegram account. Sign up in an official app first, or log in with a QR code."
                .to_owned(),
        );
        let rows = snapshot(&mut app, 80, 24);
        let text = rows.join("\n");
        assert!(
            text.contains("This phone number has no Telegram account."),
            "{text}"
        );
        assert!(
            text.contains("log in with a QR code."),
            "wrapped, not cut\n{text}"
        );
        assert!(text.contains("Enter: log in again · q: quit"), "{text}");
        app.auth = Auth::Phone;
        app.auth_error = Some(format!("Couldn't send the code: {}", "very ".repeat(30)));
        let text = snapshot(&mut app, 80, 24).join("\n");
        assert!(text.contains("very very"), "{text}");
        assert!(
            !text.contains(
                "very very very very very very very very very very very very very very very very"
            ),
            "{text}"
        );
    }

    #[test]
    fn header_has_no_version_and_shortens_what_does_not_fit() {
        let mut app = app();
        app.my_id = Some(1);
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[0].contains("gramtty"), "{}", rows[0]);
        assert!(!rows[0].contains(env!("CARGO_PKG_VERSION")), "{}", rows[0]);
        assert!(rows[0].contains("Connected as Me"), "{}", rows[0]);
        app.set_status("saved to ~/Downloads", false);
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[0].contains("Connected as Me"), "{}", rows[0]);
        assert!(rows[0].contains("Saved to ~/Downloads"), "{}", rows[0]);
        app.set_status(
            format!("couldn't load messages: {}", "a long reason ".repeat(20)),
            true,
        );
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[0].contains("Couldn't load messages"), "{}", rows[0]);
        assert!(rows[0].trim_end().ends_with('…'), "{}", rows[0]);
        assert!(!rows[0].contains("Connected as"), "{}", rows[0]);
        assert!(rows[0].chars().count() <= 100);
    }

    #[test]
    fn status_log_and_quit_confirmation_render() {
        let mut app = app();
        app.popup = Some(Popup::StatusLog { scroll: 0 });
        assert!(render(&mut app, 100, 30).contains("No notices or errors yet"));
        app.set_status("first notice", false);
        app.set_status(
            format!("couldn't load messages: {}", "a long reason ".repeat(12)),
            true,
        );
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Recent notices and errors")));
        let error = row_of(&rows, "Couldn't load messages");
        let notice = row_of(&rows, "First notice");
        assert!(notice > error + 1, "newest first, long entries wrap");
        app.popup = Some(Popup::ConfirmQuit(vec![
            "a file is still uploading".to_owned(),
        ]));
        let text = render(&mut app, 100, 30);
        assert!(text.contains("Still in progress"), "{text}");
        assert!(text.contains("A file is still uploading"), "{text}");
        assert!(text.contains("y quit anyway"), "{text}");
    }

    #[test]
    fn profile_hint_offers_their_messages_only_once_loaded() {
        let mut app = app();
        app.popup = Some(Popup::Profile(crate::app::ProfileView {
            target: crate::model::ProfileTarget::User(2),
            data: None,
            error: None,
            scroll: 0,
        }));
        let text = render(&mut app, 120, 40);
        assert!(text.contains("Loading profile…"), "{text}");
        assert!(!text.contains("their messages"), "{text}");
    }

    #[test]
    fn terminal_clipboard_sequences_are_written_with_the_frame() {
        use crate::ui::repaint_tests::Sink;
        let mut app = app();
        app.terminal_out.push(crate::upload::osc52("hi", false));
        let sink = Sink::default();
        let area = Rect::new(0, 0, 80, 24);
        let mut term = Terminal::with_options(
            Resync(CrosstermBackend::new(sink.clone())),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(area),
            },
        )
        .unwrap();
        let mut layout = image_layout(&Buffer::empty(area));
        draw_frame(&mut term, &mut app, &mut layout, |_| {}).unwrap();
        let out = String::from_utf8_lossy(&sink.0.borrow()).into_owned();
        assert!(out.contains("\x1b]52;c;aGk=\x07"));
        assert!(app.terminal_out.is_empty());
        sink.0.borrow_mut().clear();
        draw_frame(&mut term, &mut app, &mut layout, |_| {}).unwrap();
        assert!(!String::from_utf8_lossy(&sink.0.borrow()).contains("]52;"));
    }

    const COLOUR_ONLY: [&str; 10] = [
        "background",
        "selection_bg",
        "surface",
        "avatar_text",
        "popup_background",
        "bubble_incoming",
        "bubble_incoming_selected",
        "bubble_outgoing",
        "bubble_outgoing_selected",
        "card_fill",
    ];

    fn distinctive_theme() -> theme::Theme {
        use serde::Deserialize;
        let toml::Value::Table(mut root) = toml::Value::try_from(classic_rounded()).unwrap() else {
            panic!("theme is not a table");
        };
        let Some(toml::Value::Table(colors)) = root.get_mut("colors") else {
            panic!("no colors");
        };
        let mut n = 0u32;
        let mut next = || {
            n += 1;
            toml::Value::String(format!("#c8{:02x}{:02x}", n / 256, n % 256))
        };
        let keys: Vec<String> = colors.keys().cloned().collect();
        for key in keys {
            let old = colors[&key].clone();
            let new = if COLOUR_ONLY.contains(&key.as_str()) {
                next()
            } else if key == "name_palette" {
                toml::Value::Array(vec![next(), next(), next()])
            } else {
                let mut t = match old {
                    toml::Value::Table(t) => t,
                    _ => toml::Table::new(),
                };
                if key != "name" {
                    t.insert("fg".to_owned(), next());
                }
                if t.contains_key("bg") {
                    t.insert("bg".to_owned(), next());
                }
                toml::Value::Table(t)
            };
            colors.insert(key, new);
        }
        theme::Theme::deserialize(toml::Value::Table(root)).unwrap()
    }

    fn buffer(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn themed(c: Color) -> bool {
        matches!(c, Color::Rgb(200, _, _))
    }

    fn assert_all_themed(buf: &Buffer, what: &str) {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                assert!(
                    c.fg == Color::Reset || themed(c.fg),
                    "{what}: cell ({x},{y}) {:?} has non-theme fg {:?}",
                    c.symbol(),
                    c.fg
                );
                assert!(
                    themed(c.bg),
                    "{what}: cell ({x},{y}) {:?} has non-theme bg {:?}",
                    c.symbol(),
                    c.bg
                );
            }
        }
    }

    fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
        find_in(buf, needle, 0..buf.area.width)
    }

    fn find_in(buf: &Buffer, needle: &str, xs: std::ops::Range<u16>) -> (u16, u16) {
        let chars: Vec<char> = needle.chars().collect();
        for y in 1..buf.area.height {
            for x in xs.clone() {
                let hit = chars.iter().enumerate().all(|(i, ch)| {
                    let xi = x as usize + i;
                    xi < buf.area.width as usize && buf[(xi as u16, y)].symbol() == ch.to_string()
                });
                if hit {
                    return (x, y);
                }
            }
        }
        panic!("{needle} not on screen");
    }

    fn info_rows(buf: &Buffer) -> Vec<String> {
        let w = buf.area.width;
        (0..buf.area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = w - 30;
                while x < w {
                    let sym = buf[(x, y)].symbol();
                    row.push_str(sym);
                    x += (sym.width() as u16).max(1);
                }
                row
            })
            .collect()
    }

    fn info_app() -> App {
        let mut app = app();
        app.set_theme(distinctive_theme());
        app.show_folders = false;
        app.show_chats = false;
        app.show_info = true;
        app.lang = format::Lang::Ru;
        let mut chat = app.chats[&10].clone();
        chat.id = 20;
        chat.title = "Alice Longname".to_owned();
        chat.kind = ChatKind::Private(2);
        app.chats.insert(20, chat);
        app.histories.insert(20, History::default());
        app.open_chat = Some(20);
        app.user_meta.insert(
            2,
            crate::model::UserMeta {
                status: crate::presence::Status::Offline {
                    was_online: crate::app::unix_now() - 630,
                },
                ..Default::default()
            },
        );
        app
    }

    #[test]
    fn chat_info_avatar_grows_with_the_panel_and_keeps_room_for_text() {
        let font = (10, 20);
        let at = |w, h| info_avatar_size(Rect::new(0, 0, w, h), font);
        assert_eq!(at(28, 40), Some((16, 8)));
        assert_eq!(at(40, 40), Some((24, 12)));
        assert_eq!(at(80, 40), Some((24, 12)));
        assert_eq!(at(14, 40), Some((12, 6)), "narrow panels shrink it");
        assert_eq!(at(28, 12), Some((12, 6)), "short panels keep half for text");
        assert_eq!(at(6, 40), None);
        assert_eq!(at(28, 5), None);
        assert_eq!(
            info_avatar_size(Rect::new(0, 0, 28, 40), (8, 20)),
            Some((20, 8)),
            "square in pixels for the real cell size"
        );
    }

    #[test]
    fn chat_info_puts_an_initials_avatar_on_top_and_wraps_values_under_their_column() {
        let mut app = info_app();
        let buf = buffer(&mut app, 120, 40);
        let th = app.frame_theme();
        let rows = info_rows(&buf);
        let x0 = 120 - 30 + 1;
        let fill = th.name_color(2).unwrap();
        let painted: Vec<(u16, u16)> = (0..40u16)
            .flat_map(|y| (x0..119).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].bg == fill)
            .collect();
        let xs: Vec<u16> = painted.iter().map(|p| p.0).collect();
        let ys: Vec<u16> = painted.iter().map(|p| p.1).collect();
        let (left, right) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        let (top, bottom) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
        assert_eq!(
            (right - left + 1, bottom - top + 1),
            (16, 8),
            "square avatar"
        );
        assert_eq!(painted.len(), 16 * 8);
        assert_eq!(left - x0, (28 - 16) / 2, "centred");
        assert_eq!(top, 3, "right under the pane title");
        let label = row_of(&rows, "AL");
        assert_eq!(label as u16, top + 4);

        let title = row_of(&rows, "Alice Longname");
        assert!(title as u16 > bottom);
        assert_eq!(col_of(&rows[title], "Alice"), 1 + (28 - 14) / 2);
        let status = row_of(&rows, " Status: ");
        assert!(
            rows[status].contains("Status: был(а) 10 мин."),
            "{}",
            rows[status]
        );
        assert!(
            rows[status + 1].starts_with(&format!("│{}назад", " ".repeat(9))),
            "hanging indent:\n{}",
            rows.join("\n")
        );
        let key = col_of(&rows[status], "Status") as u16 + 120 - 30;
        let value = col_of(&rows[status], "был") as u16 + 120 - 30;
        assert_eq!(buf[(key, status as u16)].fg, th.colors.dim.0.fg.unwrap());
        assert_eq!(buf[(value, status as u16)].fg, th.colors.text.0.fg.unwrap());
        assert_eq!(
            buf[(value, status as u16 + 1)].fg,
            th.colors.text.0.fg.unwrap()
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("Loaded") || r.contains("Images"))
        );
        assert!(!rows.iter().any(|r| r.contains("User ID")));

        app.settings.appearance.show_ids = true;
        let rows = info_rows(&buffer(&mut app, 120, 40));
        for needle in ["User ID: 2", "Loaded: 0 messages", "Images: "] {
            assert!(
                rows.iter().any(|r| r.contains(needle)),
                "{needle}:\n{}",
                rows.join("\n")
            );
        }
    }

    #[test]
    fn chat_info_avatar_follows_the_theme_shape() {
        let mut app = info_app();
        let mut t = distinctive_theme();
        t.layout.avatar_shape = theme::AvatarShape::Circle;
        app.set_theme(t);
        let buf = buffer(&mut app, 120, 40);
        let fill = app.frame_theme().name_color(2).unwrap();
        let x0 = 120 - 30 + 1 + 6;
        assert_ne!(buf[(x0, 3)].bg, fill, "corner cut");
        assert_eq!(buf[(x0 + 8, 7)].bg, fill, "centre kept");
        assert_eq!(buf[(x0 + 3, 3)].symbol(), "▄", "edges are half a cell");
        assert_eq!(buf[(x0 + 3, 3)].fg, fill);
    }

    #[test]
    fn chat_info_lists_profile_details_for_people_and_groups() {
        let mut app = info_app();
        app.chat_profiles.insert(
            20,
            crate::model::Profile {
                user_id: Some(2),
                usernames: vec!["alice".to_owned()],
                phone: "+7 900 000 00 00".to_owned(),
                bio: "Rustacean and tea enthusiast from Kazan".to_owned(),
                links: vec!["https://t.me/alice".to_owned()],
                common_chats: Some(3),
                ..Default::default()
            },
        );
        let rows = info_rows(&buffer(&mut app, 120, 40));
        let text = rows.join("\n");
        for needle in [
            "Username: @alice",
            "Phone: +7 900 000 00",
            " Bio ",
            " Rustacean and tea",
        ] {
            assert!(text.contains(needle), "{needle}:\n{text}");
        }
        assert!(
            !text.contains("Link:") && !text.contains("Members"),
            "{text}"
        );

        app.open_chat = Some(10);
        app.chat_profiles.insert(
            10,
            crate::model::Profile {
                usernames: vec!["rust_ru".to_owned()],
                bio: "Everything about Rust".to_owned(),
                members: Some(1234),
                links: vec![
                    "https://t.me/rust_ru".to_owned(),
                    "https://t.me/+AbCdEf".to_owned(),
                ],
                ..Default::default()
            },
        );
        let rows = info_rows(&buffer(&mut app, 120, 40));
        let text = rows.join("\n");
        let members = format!("Members: {}", app.count(1234));
        for needle in [
            members.as_str(),
            "Username: @rust_ru",
            "Link: https://t.me/+AbCdEf",
            " Description ",
            " Everything about Rust",
        ] {
            assert!(text.contains(needle), "{needle}:\n{text}");
        }
        assert!(!text.contains("https://t.me/rust_ru"), "{text}");
        assert!(!text.contains("Phone"), "{text}");
    }

    fn group_profile() -> crate::model::Profile {
        crate::model::Profile {
            usernames: vec!["rust_ru".to_owned()],
            bio: "Everything about Rust. ".repeat(30),
            members: Some(1234),
            links: vec![
                "https://t.me/rust_ru".to_owned(),
                "https://t.me/+AbCdEf".to_owned(),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn chat_info_puts_who_first_then_about_then_details_and_hides_ids_unless_asked() {
        let mut app = info_app();
        app.open_chat = Some(10);
        app.chat_profiles.insert(10, group_profile());
        let rows = info_rows(&buffer(&mut app, 120, 80));
        let text = rows.join("\n");
        let at = |needle: &str| {
            rows.iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| panic!("{needle}:\n{text}"))
        };
        let members = format!("Members: {}", app.count(1234));
        let order = [
            at("Rust чат"),
            at("Username: @rust_ru"),
            at(&members),
            at(" Description"),
            at("Type: group"),
            at("Link: https://t.me/+AbCdEf"),
            at("Unread: 3"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}\n{text}");
        for absent in ["supergroup", "ID:", "Muted", "Notifications"] {
            assert!(!text.contains(absent), "{absent}:\n{text}");
        }
        app.chats.get_mut(&10).unwrap().muted = true;
        app.settings.appearance.show_ids = true;
        let text = info_rows(&buffer(&mut app, 120, 80)).join("\n");
        assert!(text.contains("Notifications: off"), "{text}");
        assert!(
            text.contains("Chat ID: 10") && !text.contains("User ID"),
            "{text}"
        );

        let mut own = app.chats[&20].clone();
        own.id = 2;
        app.chats.insert(2, own);
        app.histories.insert(2, History::default());
        app.open_chat = Some(2);
        let text = info_rows(&buffer(&mut app, 120, 40)).join("\n");
        assert_eq!(text.matches("ID: 2").count(), 1, "{text}");
        assert!(
            text.contains("User ID: 2") && !text.contains("Chat ID"),
            "{text}"
        );
        assert!(!text.contains("Type:"), "people need no type line:\n{text}");
    }

    #[test]
    fn focused_chat_info_scrolls_with_the_navigation_keys() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = info_app();
        app.open_chat = Some(10);
        app.chat_profiles.insert(10, group_profile());
        let top = |rows: &[String]| rows.iter().position(|r| r.contains("Rust чат"));
        let thumb = |rows: &[String]| rows.iter().any(|r| r.ends_with('┃'));
        let rows = info_rows(&buffer(&mut app, 120, 80));
        assert!(!thumb(&rows), "everything fits, no scroll thumb");
        let rows = info_rows(&buffer(&mut app, 120, 24));
        let first = top(&rows).unwrap();
        assert!(first > 4, "avatar above the title");
        assert!(thumb(&rows), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("Unread: 3")));
        app.focus = Pane::Info;
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        let rows = info_rows(&buffer(&mut app, 120, 24));
        assert_eq!(top(&rows), Some(2), "the avatar folds away first");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        let rows = info_rows(&buffer(&mut app, 120, 24));
        assert_eq!(top(&rows), None, "then the text moves line by line");
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        press(&mut app, KeyCode::End, KeyModifiers::NONE);
        let rows = info_rows(&buffer(&mut app, 120, 24));
        assert!(
            rows.iter().any(|r| r.contains("Unread: 3")),
            "{}",
            rows.join("\n")
        );
        let bottom = app.info_scroll;
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        buffer(&mut app, 120, 24);
        assert_eq!(app.info_scroll, bottom, "stops at the end");
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        buffer(&mut app, 120, 24);
        assert_eq!(app.info_scroll, bottom - 1);
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        let rows = info_rows(&buffer(&mut app, 120, 24));
        assert_eq!(top(&rows), Some(first));
        app.open_chat = None;
        app.open(20);
        assert_eq!(app.info_scroll, 0, "another chat starts at the top");
    }

    #[test]
    fn profile_popup_shows_each_id_once_and_only_with_show_ids() {
        use crate::app::ProfileView;
        let mut app = app();
        app.show_info = false;
        let profile = |id: i64| crate::model::Profile {
            title: "Alice Longname".to_owned(),
            kind: "user".to_owned(),
            id,
            user_id: Some(2),
            ..Default::default()
        };
        let show = |app: &mut App, id: i64| {
            app.popup = Some(Popup::Profile(ProfileView {
                target: crate::model::ProfileTarget::User(2),
                data: Some(profile(id)),
                error: None,
                scroll: 0,
            }));
            render(app, 120, 40)
        };
        let screen = show(&mut app, 2);
        assert!(!screen.contains("ID "), "{screen}");
        app.settings.appearance.show_ids = true;
        let screen = show(&mut app, 2);
        assert_eq!(screen.matches("User ID").count(), 1, "{screen}");
        assert!(!screen.contains("Chat ID"), "{screen}");
        let screen = show(&mut app, 77);
        assert!(
            screen.contains("Chat ID") && screen.contains("User ID"),
            "{screen}"
        );
    }

    fn themed_classic() -> (App, std::rc::Rc<theme::Theme>) {
        let mut app = classic_app();
        app.my_id = Some(1);
        app.show_folders = true;
        app.show_chats = true;
        app.show_info = true;
        app.open_chat = Some(10);
        app.chats.get_mut(&10).unwrap().unread = 2;
        let mut other = app.chats[&10].clone();
        other.id = 11;
        other.title = "Other".to_owned();
        other.unread = 0;
        other.positions[0].order = 9;
        app.chats.insert(11, other);
        app.chat_sel = 0;
        app.settings.appearance.layout = None;
        app.set_theme(distinctive_theme());
        let th = app.frame_theme();
        (app, th)
    }

    #[test]
    fn every_widget_takes_its_colours_from_the_theme() {
        let (mut app, th) = themed_classic();
        let c = &th.colors;
        app.focus = Pane::Messages;
        let buf = buffer(&mut app, 160, 40);
        assert_all_themed(&buf, "classic");
        let (x, y) = find_in(&buf, "Rust чат", 0..48);
        assert_eq!(buf[(x, y)].fg, c.open_chat.0.fg.unwrap());
        assert_eq!(buf[(0, 1)].fg, c.border.0.fg.unwrap());
        assert_eq!(buf[(48, 1)].symbol(), "╭");
        assert_eq!(buf[(48, 1)].fg, c.border_focused.0.fg.unwrap());
        assert_eq!(buf[(50, 1)].fg, c.title_focused.0.fg.unwrap());
        assert_eq!(buf[(2, 1)].fg, c.title.0.fg.unwrap());
        let (x, y) = find(&buf, "Сегодня");
        assert_eq!(buf[(x - 2, y)].fg, c.date_separator.0.fg.unwrap());
        let (x, y) = find_in(&buf, "Me ", 48..130);
        assert_eq!(buf[(x, y)].fg, c.own_name.0.fg.unwrap());
        let (x, y) = find(&buf, "Alice Longname ");
        assert_eq!(Some(buf[(x, y)].fg), th.name_color(2));
        assert_eq!(Some(buf[(x - 2, y)].bg), th.name_color(2));
        assert_eq!(buf[(x - 2, y)].fg, c.avatar_text.0.unwrap());
        let (x, y) = find(&buf, "✓✓");
        assert_eq!(buf[(x, y)].fg, c.tick_read.0.fg.unwrap());
        let (x, y) = find(&buf, "Bob Hidden");
        assert_eq!(buf[(x, y)].fg, c.forwarded.0.fg.unwrap());
        assert_eq!(buf[(1, 0)].fg, c.header.0.fg.unwrap());
        assert_eq!(buf[(159, 39)].bg, c.background.0.unwrap());

        app.msg_sel = Some(3);
        let buf = buffer(&mut app, 160, 40);
        assert_all_themed(&buf, "classic selected");
        let (x, y) = find(&buf, "mine read");
        assert_eq!(buf[(x, y)].bg, theme::fills().selection_bg.unwrap());
        let bar = (0..x)
            .rev()
            .find(|&bx| buf[(bx, y)].symbol() == "▎")
            .unwrap();
        assert_eq!(buf[(bar, y)].fg, c.selection_bar.0.fg.unwrap());
        assert_eq!(buf[(bar, y - 1)].symbol(), "▎");
        assert_eq!(buf[(bar, y - 2)].symbol(), "▎");
        assert_eq!(buf[(bar + 2, y - 2)].symbol(), "M");
        assert_eq!(buf[(bar + 2, y - 2)].fg, c.accent.0.fg.unwrap());
        assert_eq!(
            buf[(x + 30, y)].bg,
            theme::fills().selection_bg.unwrap(),
            "tint spans the row"
        );

        app.msg_sel = None;
        app.pending = vec![app.keymap.leader];
        let buf = buffer(&mut app, 160, 40);
        assert_all_themed(&buf, "leader window");
        let (x, y) = find(&buf, " Leader ");
        assert_eq!(buf[(x - 1, y)].fg, c.popup_border.0.fg.unwrap());
        assert_eq!(buf[(x + 1, y)].fg, c.popup_title.0.fg.unwrap());
        assert_eq!(buf[(x + 1, y + 1)].bg, theme::fills().popup.unwrap());
        let (x, y) = find(&buf, "[Space]");
        assert_eq!(buf[(x, y)].fg, c.hint_key.0.fg.unwrap());
        app.pending.clear();

        app.inbox.push_back(crate::model::InboxItem {
            chat_id: 10,
            msg_id: 1,
            sender: Sender::User(2),
            preview: "ping".to_owned(),
            date: 1_700_000_000,
            mention: true,
            topic: None,
        });
        for popup in [
            Popup::Options { sel: 6 },
            Popup::Inbox { sel: 0 },
            Popup::ConfirmLogout,
            Popup::ConfirmDelete {
                chat_id: 10,
                message_ids: vec![1],
                everyone: true,
                me: true,
            },
            Popup::Switcher {
                input: crate::input::TextInput::default(),
                sel: 0,
                forward: None,
            },
        ] {
            let name = format!("{:?}", std::mem::discriminant(&popup));
            app.popup = Some(popup);
            let buf = buffer(&mut app, 160, 40);
            assert_all_themed(&buf, &name);
        }
        app.popup = Some(Popup::Options { sel: 6 });
        let buf = buffer(&mut app, 160, 40);
        let (x, y) = find(&buf, &format!("❯ {}", crate::app::option_key(6)));
        assert_eq!(buf[(x, y)].fg, c.selection.0.fg.unwrap());
        app.popup = None;

        app.settings.appearance.layout = Some(crate::config::MessageLayout::Bubbles);
        app.msg_sel = Some(2);
        let buf = buffer(&mut app, 160, 40);
        assert_all_themed(&buf, "bubbles");
        let (x, y) = find(&buf, "old news");
        assert_eq!(buf[(x, y)].bg, theme::fills().bubble_in.unwrap());
        let (x, y) = find(&buf, "mine unread");
        assert_eq!(buf[(x, y)].bg, theme::fills().bubble_out.unwrap());
        let (x, y) = find(&buf, "second from alice");
        assert_eq!(buf[(x, y)].bg, theme::fills().bubble_in_sel.unwrap());
        assert_eq!(buf[(x - 1, y)].fg, c.bubble_edge.0.fg.unwrap());
        let (x, y) = find(&buf, "Сегодня");
        assert_eq!(buf[(x, y)].bg, c.date_badge.0.bg.unwrap());

        app.auth = Auth::Qr(Some("tg://login?token=abc".to_owned()));
        let buf = buffer(&mut app, 120, 50);
        assert_all_themed(&buf, "login");
        let (x, y) = find(&buf, "████");
        assert_eq!(buf[(x, y)].fg, c.qr.0.fg.unwrap());
        assert_eq!(buf[(x, y)].bg, c.qr.0.bg.unwrap());
    }

    #[test]
    fn message_text_formatting_and_badges_use_the_theme() {
        let (mut app, th) = themed_classic();
        let c = &th.colors;
        let h = app.histories.get_mut(&10).unwrap();
        let m = h.get_mut(5).unwrap();
        m.body = RichText {
            text: "see link and code and @bob".to_owned(),
            entities: vec![
                Entity {
                    offset: 4,
                    length: 4,
                    kind: EntityKind::Link(None),
                },
                Entity {
                    offset: 13,
                    length: 4,
                    kind: EntityKind::Code,
                },
                Entity {
                    offset: 22,
                    length: 4,
                    kind: EntityKind::Mention,
                },
            ],
        };
        app.chats.get_mut(&10).unwrap().mentions = 0;
        app.open_chat = None;
        let buf = buffer(&mut app, 160, 40);
        let (ox, _) = find_in(&buf, "Other", 0..48);
        let (x, y) = find_in(&buf, "Rust чат", 0..48);
        assert_eq!(x, ox);
        assert_eq!(buf[(x, y)].fg, c.unread_title.0.fg.unwrap());
        let badge_y = y + 1;
        let badge_x = (x..47)
            .rev()
            .find(|i| buf[(*i, badge_y)].symbol() != " ")
            .unwrap();
        assert_eq!(buf[(badge_x, badge_y)].symbol(), "2");
        assert_eq!(buf[(badge_x, badge_y)].fg, c.unread_badge.0.fg.unwrap());
        app.open_chat = Some(10);
        let buf = buffer(&mut app, 160, 40);
        assert_all_themed(&buf, "formatted text");
        let (x, y) = find(&buf, "see link");
        assert_eq!(buf[(x + 4, y)].fg, c.link.0.fg.unwrap());
        assert_eq!(buf[(x + 13, y)].fg, c.code.0.fg.unwrap());
        assert_eq!(buf[(x + 22, y)].fg, c.mention.0.fg.unwrap());
        assert_eq!(buf[(x + 9, y)].fg, c.text.0.fg.unwrap());
    }

    #[test]
    fn layout_options_and_glyphs_change_the_screen() {
        let mut app = classic_app();
        app.my_id = Some(1);
        let base = |f: &dyn Fn(&mut theme::Theme)| {
            let mut t = classic_rounded();
            f(&mut t);
            t
        };
        let gutter_of = |app: &mut App| {
            let rows = snapshot(app, 100, 30);
            col_of(&rows[row_of(&rows, "first from alice")], "first from alice")
        };
        let normal = gutter_of(&mut app);
        app.set_theme(base(&|t| t.layout.message_gutter = 5));
        assert_eq!(gutter_of(&mut app), normal + 3);

        app.set_theme(base(&|t| t.layout.avatars = false));
        let rows = snapshot(&mut app, 100, 30);
        let header = &rows[row_of(&rows, "Alice Longname ")];
        assert!(header.starts_with("│  Alice"), "{header}");
        app.set_theme(classic_rounded());
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[row_of(&rows, "Alice Longname ")].starts_with("│A"));

        let first = app.histories[&10].get(2).unwrap().date;
        app.set_theme(base(&|t| {
            t.layout.clock = theme::Clock::H12;
            t.layout.seconds = true;
        }));
        let rows = snapshot(&mut app, 100, 30);
        let label = format::time_label(first);
        assert!(label.ends_with("AM") || label.ends_with("PM"), "{label}");
        assert_eq!(label.matches(':').count(), 2);
        assert!(rows.iter().any(|r| r.contains(&label)));

        app.set_theme(base(&|t| {
            t.layout.date_separator = theme::DateSeparator::None
        }));
        assert!(
            !snapshot(&mut app, 100, 30)
                .iter()
                .any(|r| r.contains("Сегодня"))
        );
        app.set_theme(base(&|t| {
            t.layout.date_separator = theme::DateSeparator::Plain
        }));
        let rows = snapshot(&mut app, 100, 30);
        let today = &rows[row_of(&rows, "Сегодня")];
        assert!(!today.contains('─'), "{today}");

        app.set_theme(base(&|t| {
            t.glyphs.tick_read = "RR".to_owned();
            t.glyphs.separator = "=".to_owned();
            t.glyphs.edited = "(e)".to_owned();
        }));
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[row_of(&rows, "Me ")].contains(" RR"));
        assert!(rows[row_of(&rows, "Сегодня")].contains("= Сегодня ="));

        app.show_folders = true;
        app.show_chats = true;
        app.set_theme(base(&|t| {
            t.layout.folders_width = theme::PaneWidth::Cols(10);
            t.layout.chats_width = theme::PaneWidth::Percent(50);
            t.glyphs.selection_marker = "> ".to_owned();
        }));
        app.focus = Pane::Chats;
        let rows = snapshot(&mut app, 100, 30);
        let corners: Vec<usize> = rows[1]
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '╭')
            .map(|(i, _)| i)
            .collect();
        assert_eq!(corners, vec![0, 10, 60], "{}", rows[1]);
        assert!(
            rows.iter().any(|r| r.contains("│> ")),
            "{}",
            rows.join("\n")
        );
    }

    #[test]
    fn scrollbar_and_bubble_options_render() {
        let mut app = bubble_app();
        let rows = snapshot(&mut app, 100, 30);
        assert!(rows[row_of(&rows, "second from")].contains("enough"));
        let edge: String = rows.iter().filter_map(|r| r.chars().nth(98)).collect();
        assert!(!edge.contains('█') && !edge.contains('║'));
        let mut t = theme::Theme::default();
        t.layout.scrollbar = true;
        t.layout.bubble_avatar_rows = 3;
        t.layout.bubble_max_width = 40;
        app.set_theme(t);
        let buf = buffer(&mut app, 100, 30);
        let thumb = theme::get().colors.scrollbar_thumb.0.fg.unwrap();
        assert!(
            (1..29).any(|y| buf[(98, y)].fg == thumb),
            "no scrollbar thumb"
        );
        let rows = snapshot(&mut app, 100, 30);
        let first = row_of(&rows, "first from alice");
        assert_eq!(col_of(&rows[first], "first from alice"), 1 + 6 + 1 + 1);
        let second = &rows[row_of(&rows, "second from")];
        assert!(!second.contains("enough"), "{second}");
    }
    mod layout_variants {
        use super::*;
        use crate::config::MessageLayout;

        pub(super) fn conversation(layout: MessageLayout) -> App {
            let mut app = app();
            app.settings.appearance.layout = Some(layout);
            app.show_info = false;
            app.show_folders = false;
            app.show_chats = false;
            app.users.insert(3, "Bob".to_owned());
            let now = chrono::Local::now().timestamp();
            let base = now - now.rem_euclid(86_400) + 12 * 3600 - 86_400;
            let plain = |id: i64, sender: i64, date: i64, text: &str| {
                let mut m = msg(id, sender, date, text);
                m.body = RichText::plain(text);
                m.edited = false;
                m.reply_to = None;
                m.media = None;
                m
            };
            let mut batch = vec![
                plain(1, 2, base, "hey, did you see the new release?"),
                plain(
                    2,
                    2,
                    base + 60,
                    "the changelog is long enough to wrap around a couple of lines in a narrow pane",
                ),
                plain(3, 1, base + 180, "yes! upgrading tonight"),
                plain(4, 1, base + 200, "screenshot"),
                plain(5, 3, base + 3000, "Rust 1.99 released"),
            ];
            batch[1].edited = true;
            batch[2].reply_to = Some(2);
            batch[3].attachments = vec![photo(900)];
            batch[4].forward = Some(crate::model::Origin::Name("Rust News".to_owned()));
            batch[4].pinned = true;
            let mut h = History::default();
            h.reached_start = true;
            h.merge_older(batch);
            app.histories.insert(10, h);
            let chat = app.chats.get_mut(&10).unwrap();
            chat.last_read_outbox = 3;
            chat.unread = 0;
            app
        }

        fn tiny_cards() -> App {
            let mut app = conversation(MessageLayout::Cards);
            app.users.insert(7, "twink".to_owned());
            let h = app.histories.get_mut(&10).unwrap();
            let last = h.msgs().last().unwrap().clone();
            let mut a = last.clone();
            a.id = 50;
            a.sender = Sender::User(7);
            a.date = last.date + 4000;
            a.body = RichText::plain("Ппп");
            a.forward = None;
            a.pinned = false;
            a.outgoing = false;
            let mut b = a.clone();
            b.id = 51;
            b.sender = Sender::User(1);
            b.outgoing = true;
            b.date += 600;
            b.body = RichText::plain("Xd");
            let mut c = b.clone();
            c.id = 52;
            c.body = RichText::plain("D");
            for m in [a, b, c] {
                h.upsert(m);
            }
            app
        }

        #[test]
        fn deleted_marker_blocked_placeholder_and_details_render_in_every_layout() {
            for layout in MessageLayout::ALL {
                let mut app = conversation(layout);
                app.settings.privacy.keep_deleted = true;
                app.settings.privacy.hide_blocked = true;
                app.blocked.insert(Sender::User(3));
                app.histories
                    .get_mut(&10)
                    .unwrap()
                    .get_mut(1)
                    .unwrap()
                    .deleted = true;
                let text = snapshot(&mut app, 100, 70).join("\n");
                assert!(text.contains("deleted"), "{layout:?}\n{text}");
                assert!(text.contains("hey, did you see"), "{layout:?}\n{text}");
                assert!(
                    text.contains("message from a blocked user"),
                    "{layout:?}\n{text}"
                );
                assert!(!text.contains("Rust 1.99 released"), "{layout:?}\n{text}");
                app.revealed.insert((10, 5));
                let text = snapshot(&mut app, 100, 70).join("\n");
                assert!(text.contains("Rust 1.99 released"), "{layout:?}\n{text}");
            }
            let mut app = conversation(MessageLayout::Classic);
            app.popup = Some(Popup::Details {
                chat_id: 10,
                msg_id: 2,
                flags: None,
                scroll: 0,
            });
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("Message details"), "{text}");
            assert!(text.contains("Message ID"), "{text}");
            assert!(text.contains("Sender ID        2 (user)"), "{text}");
            assert!(text.contains("Checking…"), "{text}");
        }

        #[test]
        fn stage5c_popups_and_quote_header_render() {
            use crate::admin::{LogEntry, Member, MemberFilter, MemberPage};
            use crate::app::{LogView, MembersView};
            use crate::input::TextInput;
            let mut app = conversation(MessageLayout::Classic);
            app.popup = Some(Popup::QuotePick {
                chat_id: 10,
                msg_id: 2,
                input: TextInput::with_text("a quoted fragment"),
            });
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("Reply with quote"), "{text}");
            assert!(text.contains("a quoted fragment"), "{text}");
            app.popup = Some(Popup::Translate {
                chat_id: 10,
                msg_id: 2,
                lang: "ru".to_owned(),
                result: None,
                scroll: 0,
            });
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("Translating to ru via Telegram"), "{text}");
            app.popup = Some(Popup::Translate {
                chat_id: 10,
                msg_id: 2,
                lang: "ru".to_owned(),
                result: Some(Ok("привет из перевода".to_owned())),
                scroll: 0,
            });
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("Translation → ru"), "{text}");
            assert!(!text.contains("LibreTranslate"), "{text}");
            assert!(text.contains("привет из перевода"), "{text}");
            assert!(text.contains("y copy"), "{text}");
            app.popup = Some(Popup::Members(MembersView {
                chat_id: 10,
                supergroup_id: 77,
                filter: MemberFilter::Admins,
                input: TextInput::default(),
                page: Some(Ok(MemberPage {
                    total: 1500,
                    members: vec![Member {
                        sender: Sender::User(2),
                        status: "admin · mod".to_owned(),
                        joined: 1_700_000_000,
                    }],
                })),
                sel: 0,
                token: 1,
            }));
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("Members of"), "{text}");
            assert!(text.contains(" admins "), "{text}");
            assert!(text.contains("admin · mod"), "{text}");
            assert!(text.contains("1 shown of 1.5K"), "{text}");
            app.popup = Some(Popup::EventLog(LogView {
                chat_id: 10,
                entries: vec![LogEntry {
                    id: 1,
                    date: 1_700_000_000,
                    actor: Sender::User(2),
                    action: "changed the title".to_owned(),
                    target: None,
                    detail: "\"Old\" → \"New\"".to_owned(),
                }],
                sel: 0,
                loading: false,
                done: true,
                error: None,
            }));
            let text = snapshot(&mut app, 110, 40).join("\n");
            assert!(text.contains("Recent actions in"), "{text}");
            assert!(
                text.contains("changed the title: \"Old\" → \"New\""),
                "{text}"
            );
            app.popup = None;
            app.open_chat = Some(10);
            app.composer.active = true;
            app.composer.reply_to = Some(2);
            app.composer.quote = Some(("did you see".to_owned(), 5));
            let text = snapshot(&mut app, 100, 40).join("\n");
            assert!(text.contains("“did you see”"), "{text}");
            let me = app.my_id.unwrap_or(1);
            app.my_id = Some(me);
            let profile = |id: i64| {
                Popup::Profile(crate::app::ProfileView {
                    target: crate::model::ProfileTarget::User(id),
                    data: Some(crate::model::Profile {
                        title: "Somebody".to_owned(),
                        kind: "user".to_owned(),
                        id,
                        user_id: Some(id),
                        phone: "+15550001".to_owned(),
                        ..Default::default()
                    }),
                    error: None,
                    scroll: 0,
                })
            };
            app.settings.privacy.hide_phone = true;
            app.popup = Some(profile(me));
            let text = snapshot(&mut app, 120, 40).join("\n");
            assert!(text.contains("hidden"), "{text}");
            assert!(!text.contains("+15550001"), "{text}");
            app.popup = Some(profile(me + 1000));
            let text = snapshot(&mut app, 120, 40).join("\n");
            assert!(text.contains("+15550001"), "{text}");
            app.settings.privacy.hide_phone = false;
            app.popup = Some(profile(me));
            let text = snapshot(&mut app, 120, 40).join("\n");
            assert!(text.contains("+15550001"), "{text}");
        }

        #[test]
        fn partially_visible_top_message_stays_when_selecting() {
            for layout in MessageLayout::ALL {
                let mut app = conversation(layout);
                let h = app.histories.get_mut(&10).unwrap();
                let first = h.msgs()[0].id;
                let m = h.get_mut(first).unwrap();
                let text: Vec<String> = (1..=40)
                    .map(|i| format!("строка {i}"))
                    .chain(["Дата розыгрыша: завтра".to_owned()])
                    .collect();
                m.body = RichText::plain(text.join("\n"));
                app.focus = Pane::Messages;
                let n = app.histories[&10].msgs().len();
                let before = pane(&snapshot(&mut app, 80, 40));
                let tail = row_of(&before, "Дата розыгрыша");
                let top_text = before
                    .iter()
                    .position(|r| r.contains("строка"))
                    .unwrap_or_else(|| {
                        panic!(
                            "{layout:?}: tall message clipped at top\n{}",
                            before.join("\n")
                        )
                    });
                assert!(top_text <= 3, "{layout:?}\n{}", before.join("\n"));
                for sel in [n - 1, n - 2, 0] {
                    app.msg_sel = Some(sel);
                    let after = pane(&snapshot(&mut app, 80, 40));
                    let screen = after.join("\n");
                    if sel != 0 {
                        assert_eq!(
                            row_of(&after, "Дата розыгрыша"),
                            tail,
                            "{layout:?} sel {sel}\n{screen}"
                        );
                        assert!(
                            after[..=3].iter().any(|r| r.contains("строка")),
                            "{layout:?} sel {sel}: top kept\n{screen}"
                        );
                    } else {
                        assert!(
                            screen.contains("строка 1"),
                            "{layout:?}: selected tall message shows its top\n{screen}"
                        );
                    }
                }
                app.msg_sel = None;
                let back = pane(&snapshot(&mut app, 80, 40));
                assert!(
                    back.iter().any(|r| r.contains("строка 1")),
                    "{layout:?}: viewport kept after Esc"
                );
            }
        }

        fn ansi(buf: &Buffer, rows: std::ops::Range<u16>) -> String {
            use ratatui::style::Modifier;
            let sgr = |c: Color, fg: bool| -> String {
                let base = if fg { 30 } else { 40 };
                match c {
                    Color::Reset => format!("{}", base + 9),
                    Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
                    Color::Indexed(n) => format!("{};5;{n}", base + 8),
                    other => {
                        let names = [
                            Color::Black,
                            Color::Red,
                            Color::Green,
                            Color::Yellow,
                            Color::Blue,
                            Color::Magenta,
                            Color::Cyan,
                            Color::Gray,
                            Color::DarkGray,
                            Color::LightRed,
                            Color::LightGreen,
                            Color::LightYellow,
                            Color::LightBlue,
                            Color::LightMagenta,
                            Color::LightCyan,
                            Color::White,
                        ];
                        let i = names.iter().position(|n| *n == other).unwrap_or(7);
                        format!("{};5;{i}", base + 8)
                    }
                }
            };
            let mut out = String::new();
            for y in rows {
                let mut skip = 0;
                for x in 0..buf.area.width {
                    let c = &buf[(x, y)];
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let mut codes = vec!["0".to_owned(), sgr(c.fg, true), sgr(c.bg, false)];
                    if c.modifier.contains(Modifier::BOLD) {
                        codes.push("1".to_owned());
                    }
                    if c.modifier.contains(Modifier::ITALIC) {
                        codes.push("3".to_owned());
                    }
                    out.push_str(&format!("\x1b[{}m{}", codes.join(";"), c.symbol()));
                    skip = c.symbol().width().saturating_sub(1);
                }
                out.push_str("\x1b[0m\n");
            }
            out
        }

        #[test]
        #[ignore = "writes .work/palette-preview.ansi; run with --ignored"]
        fn write_palette_preview() {
            let mut pal = crate::termcolors::FALLBACK;
            pal.bg = (19, 21, 12);
            pal.fg = (227, 228, 211);
            pal.ansi[2] = (184, 207, 132);
            pal.ansi[4] = (190, 203, 152);
            pal.ansi[6] = (160, 210, 180);
            pal.ansi[8] = (90, 94, 74);
            let mut out = String::new();
            for (name, palette) in [
                ("terminal preset, matugen-like olive palette", Some(pal)),
                ("default preset (terminal did not answer)", None),
            ] {
                crate::termcolors::set(palette);
                for (layout, sel) in [
                    (MessageLayout::Classic, Some(3)),
                    (MessageLayout::Split, Some(1)),
                    (MessageLayout::Bubbles, Some(0)),
                    (MessageLayout::Cards, Some(1)),
                ] {
                    let mut app = conversation(layout);
                    let mut t = theme::Theme::preset(if palette.is_some() {
                        crate::config::ThemeChoice::Terminal
                    } else {
                        crate::config::ThemeChoice::Default
                    });
                    t.layout.message_layout = layout;
                    app.set_theme(t);
                    app.focus = Pane::Messages;
                    app.msg_sel = sel;
                    let buf = buffer(&mut app, 90, 40);
                    let rows = buf_rows(&buf);
                    let top = row_of(&rows, "Rust чат") as u16;
                    let start = rows
                        .iter()
                        .position(|r| r.contains("hey, did") || r.contains("Alice"))
                        .unwrap_or(top as usize)
                        .saturating_sub(2) as u16;
                    out.push_str(&format!("\x1b[0m\n== {} · {name} ==\n", layout.name()));
                    out.push_str(&ansi(&buf, top..start.max(top + 1).min(top + 1)));
                    out.push_str(&ansi(&buf, start..buf.area.height));
                }
            }
            crate::termcolors::set(None);
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".work");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("palette-preview.ansi"), out).unwrap();
        }

        fn pane(rows: &[String]) -> Vec<String> {
            rows.iter()
                .map(|r| {
                    let inner: String = r.chars().skip(1).collect();
                    inner.trim_end_matches('│').trim_end().to_owned()
                })
                .collect()
        }

        fn visible_width(s: &str) -> usize {
            s.width()
        }

        #[test]
        fn cards_box_runs_align_sides_and_carry_time_in_border() {
            let mut app = conversation(MessageLayout::Cards);
            let rows = pane(&snapshot(&mut app, 80, 40));
            let screen = rows.join("\n");
            assert_eq!(screen.matches("Alice Longname").count(), 2, "{screen}");
            let title = row_of(&rows, "╭─Alice Longname─");
            assert!(rows[title].trim_end().ends_with('╮'), "{screen}");
            let first = row_of(&rows, "hey, did you");
            let second = row_of(&rows, "the changelog");
            assert!(
                second > first && !rows[second - 1].contains('╰'),
                "one card per run\n{screen}"
            );
            let bottom = (second..rows.len())
                .find(|&i| rows[i].contains('╰'))
                .unwrap();
            assert!(
                rows[bottom].contains("изм. ") || rows[bottom].contains("edited "),
                "{screen}"
            );
            let mine = row_of(&rows, "upgrading tonight");
            assert!(rows[mine].ends_with('│'), "{screen}");
            assert!(
                visible_width(&rows[mine]) >= 76,
                "own card is right aligned\n{screen}"
            );
            assert!(
                rows[mine - 1].contains("│ Alice Longname: the changelog"),
                "{screen}"
            );
            assert!(screen.contains("photo"), "{screen}");
            let own_bottom = (mine..rows.len()).find(|&i| rows[i].contains('╰')).unwrap();
            assert!(
                rows[own_bottom].contains('✓') && !rows[own_bottom].contains("✓✓"),
                "{screen}"
            );
            assert!(!rows[row_of(&rows, "upgrading")].contains('✓'), "{screen}");
            assert!(
                screen.contains("Forwarded from Rust News")
                    || screen.contains("Переслано от Rust News")
            );
            assert!(screen.contains("📌"), "{screen}");
            for t in (0..rows.len()).filter(|&i| rows[i].contains("╭─")) {
                let b = (t..rows.len()).find(|&i| rows[i].contains('╰')).unwrap();
                let left = col_of(&rows[t], "╭");
                for r in &rows[t + 1..b] {
                    assert_eq!(r.chars().nth(left), Some('│'), "{screen}");
                    assert_eq!(r.width(), rows[t].width(), "sides align\n{screen}");
                }
            }
        }

        #[test]
        fn cards_follow_the_border_style_and_selection() {
            let mut app = conversation(MessageLayout::Cards);
            app.settings.appearance.border_style = Some(crate::config::BorderStyle::Double);
            app.msg_sel = Some(4);
            app.focus = Pane::Messages;
            let mut term = Terminal::new(TestBackend::new(80, 40)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer().clone();
            let rows: Vec<String> = (0..40)
                .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect();
            let screen = rows.join("\n");
            assert!(screen.contains("╔═Bob═"), "{screen}");
            let y = row_of(&rows, "╔═Bob");
            let x = col_of(&rows[y], "╔");
            let sel = crate::theme::selection_border();
            assert_eq!(buf[(x as u16, y as u16)].fg, sel.fg.unwrap());
            let y = row_of(&rows, "hey, did");
            let x = col_of(&rows[y], "║ hey");
            assert_ne!(buf[(x as u16, y as u16)].fg, sel.fg.unwrap());
        }

        #[test]
        fn compact_aligns_columns_and_groups_names() {
            let mut app = conversation(MessageLayout::Compact);
            let rows = pane(&snapshot(&mut app, 80, 30));
            let screen = rows.join("\n");
            let first = row_of(&rows, "hey, did you");
            let second = row_of(&rows, "the changelog");
            assert_eq!(second, first + 1, "dense\n{screen}");
            assert!(rows[first].contains("Alice Longn"), "{screen}");
            assert!(!rows[second].contains("Alice"), "{screen}");
            let col = col_of(&rows[first], "hey, did");
            assert_eq!(col_of(&rows[second], "the changelog"), col, "{screen}");
            assert_eq!(
                col_of(&rows[second + 1], "lines"),
                col,
                "continuation aligned\n{screen}"
            );
            assert_eq!(col_of(&rows[first], "│ hey"), col - 2, "{screen}");
            let mine = row_of(&rows, "upgrading tonight");
            assert!(rows[mine].ends_with("✓✓"), "tick at line end\n{screen}");
            assert!(
                rows[mine - 1].contains("Me │ │ Alice Longname"),
                "reply strip in text column\n{screen}"
            );
            let name_end = col_of(&rows[mine - 1], " │ │");
            assert_eq!(name_end, col - 3, "{screen}");
            assert!(
                rows[second + 1].contains("изм.") || rows[second + 1].contains("edited"),
                "{screen}"
            );
            assert!(screen.contains("photo"));
        }

        #[test]
        fn minimal_names_once_per_run_and_time_on_the_right() {
            let mut app = conversation(MessageLayout::Minimal);
            let rows = pane(&snapshot(&mut app, 80, 34));
            let screen = rows.join("\n");
            assert_eq!(
                screen.matches("Alice Longname").count(),
                2,
                "name + reply marker\n{screen}"
            );
            let name = row_of(&rows, "Alice Longname");
            assert_eq!(row_of(&rows, "hey, did you"), name + 1, "{screen}");
            let time = format::time_label(app.histories[&10].msgs()[0].date);
            assert!(rows[name].ends_with(&time), "{screen}");
            assert!(rows[name + 2].contains("the changelog"), "{screen}");
            assert!(!rows[name + 2].contains("Alice"), "{screen}");
            assert!(
                rows[name + 2].contains(&format::time_label(app.histories[&10].msgs()[1].date))
            );
            let me = row_of(&rows, "Me");
            assert!(rows[me - 1].is_empty(), "whitespace between runs\n{screen}");
            assert!(
                rows[me].ends_with('✓') || rows[me].trim_end().ends_with("✓✓"),
                "{screen}"
            );
            assert!(
                rows[me + 1].contains("│ Alice Longname: the changelog"),
                "{screen}"
            );
            assert!(
                !screen.contains('╭') || !screen[screen.find('╭').unwrap()..].contains("Alice")
            );
            assert!(screen.contains("↪ Rust News"), "{screen}");
        }

        #[test]
        fn every_layout_fits_narrow_panes_and_keeps_features() {
            for layout in MessageLayout::ALL {
                let mut app = conversation(layout);
                let rows = snapshot(&mut app, 40, 60);
                let screen = rows.join("\n");
                for needle in ["hey, did", "upgrading", "Rust 1.99", "screen"] {
                    assert!(screen.contains(needle), "{layout:?} {needle}\n{screen}");
                }
                let read = if layout == MessageLayout::Cards {
                    "✓"
                } else {
                    "✓✓"
                };
                assert!(
                    screen.contains(read) && screen.contains("📌"),
                    "{layout:?}\n{screen}"
                );
                assert!(screen.contains("pane"), "{layout:?} wraps\n{screen}");
                for width in [40usize, 44, 52] {
                    let ctx = Ctx {
                        width,
                        layout,
                        theme: 0,
                        now: 0,
                        today: 0,
                        lang: format::Lang::En,
                        sel: None,
                    };
                    let msgs = app.histories[&10].msgs();
                    for (g, range) in model::groups(msgs).into_iter().enumerate() {
                        let mut cache = LayoutCache::default();
                        let r = cached_block(&app, &mut cache, msgs, range, ctx, g == 2);
                        for l in &r.lines {
                            assert!(spans_width(&l.spans) <= width, "{layout:?} {width}: {l:?}");
                        }
                        for p in &r.images {
                            assert!(
                                (p.col + p.cols) as usize <= width,
                                "{layout:?} {width}: {p:?}"
                            );
                        }
                    }
                }
                let styled = |app: &mut App| {
                    let mut term = Terminal::new(TestBackend::new(40, 60)).unwrap();
                    term.draw(|f| draw(f, app)).unwrap();
                    term.backend().buffer().clone()
                };
                let plain_buf = styled(&mut app);
                app.msg_sel = Some(2);
                app.focus = Pane::Messages;
                let selected = snapshot(&mut app, 40, 60).join("\n");
                assert!(selected.contains("upgrading"), "{layout:?}\n{selected}");
                assert!(
                    styled(&mut app) != plain_buf,
                    "{layout:?} shows the selection"
                );
            }
        }

        #[test]
        fn layouts_are_cached_per_message_and_width() {
            let mut app = conversation(MessageLayout::Compact);
            snapshot(&mut app, 80, 30);
            let n = app.layout_cache.len();
            assert!(n >= 5);
            snapshot(&mut app, 80, 30);
            assert_eq!(app.layout_cache.len(), n);
            app.settings.appearance.layout = Some(MessageLayout::Cards);
            let cards = snapshot(&mut app, 80, 30).join("\n");
            assert!(cards.contains('╭'));
        }

        #[test]
        fn avatar_corners_show_the_background_not_the_initials_colour() {
            let green = Color::Rgb(0, 200, 0);
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                64,
                64,
                image::Rgba([0, 200, 0, 255]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
            for shape in [
                crate::theme::AvatarShape::Circle,
                crate::theme::AvatarShape::Rounded,
                crate::theme::AvatarShape::Square,
            ] {
                let mut app = bubble_app();
                let mut t = crate::theme::Theme::default();
                t.layout.avatar_shape = shape;
                app.set_theme(t);
                app.user_meta.entry(2).or_default().photo = Some(crate::model::FileRef {
                    id: 777,
                    path: "/nonexistent/avatar.png".to_owned(),
                    done: true,
                    ..crate::model::FileRef::default()
                });
                let key = Key {
                    src: Src::File(777),
                    cols: AVATAR_COLS,
                    rows: AVATAR_COLS / 2,
                    shape: Some(shape),
                    animate: false,
                };
                let frames = media::render(
                    &ratatui_image::picker::Picker::halfblocks(),
                    &Input::Bytes(png.clone().into()),
                    key,
                )
                .unwrap();
                app.media.insert_ready(key, frames, 0);
                let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
                term.draw(|f| draw(f, &mut app)).unwrap();
                let buf = term.backend().buffer().clone();
                let greenish = |c: Color| matches!(c, Color::Rgb(r, g, _) if g > 60 && r < 40);
                let mut hits: Vec<(u16, u16)> = (0..30)
                    .flat_map(|y| (0..100).map(move |x| (x, y)))
                    .filter(|&(x, y)| greenish(buf[(x, y)].fg) || greenish(buf[(x, y)].bg))
                    .collect();
                assert!(!hits.is_empty(), "{shape:?} avatar drawn");
                let y0 = hits.iter().map(|h| h.1).min().unwrap();
                hits.retain(|h| h.1 < y0 + AVATAR_COLS / 2);
                let x0 = hits.iter().map(|h| h.0).min().unwrap();
                let x1 = hits.iter().map(|h| h.0).max().unwrap();
                let y1 = hits.iter().map(|h| h.1).max().unwrap();
                assert_eq!(
                    (x1 - x0 + 1, y1 - y0 + 1),
                    (AVATAR_COLS, AVATAR_COLS / 2),
                    "{shape:?}"
                );
                let initials = crate::theme::avatar(2).bg.unwrap();
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        assert_ne!(buf[(x, y)].bg, initials, "{shape:?} {x},{y}");
                    }
                }
                let corner = &buf[(x0, y0)];
                let centre = &buf[(x0 + 1, y0)];
                match shape {
                    crate::theme::AvatarShape::Square => {
                        assert_eq!(corner.bg, green, "{shape:?}");
                    }
                    crate::theme::AvatarShape::Rounded => {
                        assert!(greenish(corner.bg), "{shape:?}");
                    }
                    crate::theme::AvatarShape::Circle => {
                        assert_eq!(corner.bg, Color::Reset, "{shape:?}");
                        assert_eq!(corner.symbol(), "▄", "{shape:?}");
                        assert!(greenish(corner.fg), "{shape:?}");
                        assert!(greenish(centre.bg), "{shape:?}");
                    }
                }
            }
        }

        #[test]
        fn halfblock_cells_outside_the_mask_take_the_background() {
            let behind = Color::Rgb(1, 2, 3);
            let mut cell = ratatui::buffer::Cell::default();
            cell.set_symbol("▀");
            cell.set_fg(Color::Rgb(9, 9, 9));
            cell.set_bg(Color::Rgb(7, 7, 7));
            let mut c = cell.clone();
            mask_halfblock_cell(&mut c, false, true, behind);
            assert_eq!((c.symbol(), c.fg, c.bg), ("▄", Color::Rgb(7, 7, 7), behind));
            let mut c = cell.clone();
            mask_halfblock_cell(&mut c, true, false, behind);
            assert_eq!((c.symbol(), c.fg, c.bg), ("▀", Color::Rgb(9, 9, 9), behind));
            let mut c = cell.clone();
            mask_halfblock_cell(&mut c, false, false, behind);
            assert_eq!((c.symbol(), c.bg), (" ", behind));
            let mut c = cell.clone();
            mask_halfblock_cell(&mut c, true, true, behind);
            assert_eq!(c, cell);
        }

        fn pane_end(rows: &[String]) -> usize {
            let first = row_of(rows, "the changelog");
            (first..rows.len())
                .find(|&i| rows[i].contains("pane"))
                .unwrap()
        }

        fn buf_rows(buf: &Buffer) -> Vec<String> {
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect()
        }

        #[test]
        fn cards_selection_recolours_without_moving_anything() {
            let plain_rows = {
                let mut app = conversation(MessageLayout::Cards);
                app.focus = Pane::Messages;
                app.msg_sel = Some(4);
                buf_rows(&buffer(&mut app, 80, 40))
            };
            let shape = |rows: &[String]| -> Vec<String> {
                rows.iter().map(|r| r.replace('▎', " ")).collect()
            };
            for sel in [0usize, 1] {
                let mut app = conversation(MessageLayout::Cards);
                app.focus = Pane::Messages;
                app.msg_sel = Some(4);
                let before = buf_rows(&buffer(&mut app, 80, 40));
                assert_eq!(before, plain_rows);
                app.msg_sel = Some(sel);
                let buf = buffer(&mut app, 80, 40);
                let rows = buf_rows(&buf);
                let screen = rows.join("\n");
                let needle = ["hey, did you", "the changelog"][sel];
                let other = ["the changelog", "hey, did you"][sel];
                let y = row_of(&rows, needle);
                let oy = row_of(&rows, other);
                let top = (0..y).rev().find(|&i| rows[i].contains('╭')).unwrap();
                let bottom = (y..rows.len()).find(|&i| rows[i].contains('╰')).unwrap();
                assert!(top < oy && oy < bottom, "one card per run\n{screen}");
                let left = col_of(&rows[top], "╭");
                let right = col_of(&rows[top], "╮");
                let sel_fg = theme::selection_border().fg.unwrap();
                let bar_x = left as u16 + 1;
                assert_eq!(buf[(bar_x, y as u16)].symbol(), "▎", "{screen}");
                assert_eq!(buf[(left as u16, y as u16)].fg, sel_fg, "{screen}");
                assert_eq!(buf[(right as u16, y as u16)].fg, sel_fg, "{screen}");
                assert_eq!(
                    buf[(bar_x + 2, y as u16)].bg,
                    theme::selection_bg().unwrap()
                );
                assert_ne!(buf[(bar_x, oy as u16)].symbol(), "▎", "{screen}");
                assert_ne!(buf[(left as u16, oy as u16)].fg, sel_fg, "{screen}");
                let boxes = |r: &[String]| -> Vec<(Option<usize>, Option<usize>)> {
                    shape(r)
                        .iter()
                        .map(|l| (l.find('╭'), l.find('╰')))
                        .collect()
                };
                assert_eq!(boxes(&rows), boxes(&plain_rows), "{screen}");
            }
        }

        #[test]
        fn cards_geometry_is_identical_with_and_without_selection() {
            let mut app = tiny_cards();
            app.focus = Pane::Messages;
            let n = app.histories[&10].msgs().len();
            let frame = |app: &mut App, sel: Option<usize>| {
                app.msg_sel = sel;
                let rows = buf_rows(&buffer(app, 80, 60));
                let strip = |r: &String| r.replace('▎', " ");
                let rows: Vec<String> = rows.iter().map(strip).collect();
                let start = row_of(&rows, "Rust 1.99");
                rows[start..].to_vec()
            };
            let base = frame(&mut app, Some(n - 1));
            for sel in (n - 4)..n {
                let rows = frame(&mut app, Some(sel));
                let shape = |r: &[String]| -> Vec<String> {
                    r.iter().map(|l| l.replace("↓ G", "   ")).collect()
                };
                let (a, b) = (shape(&base), shape(&rows));
                let boxes = |r: &[String]| -> Vec<(usize, Option<usize>, Option<usize>)> {
                    r.iter()
                        .enumerate()
                        .map(|(i, l)| (i, l.find('╭'), l.find('╰')))
                        .collect()
                };
                assert_eq!(
                    boxes(&a),
                    boxes(&b),
                    "sel {sel}\n{}\n---\n{}",
                    a.join("\n"),
                    b.join("\n")
                );
            }
        }

        fn cell_text(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
            let mut out = String::new();
            let mut x = from;
            while x < to {
                let sym = buf[(x, y)].symbol();
                out.push_str(sym);
                x += sym.width().max(1) as u16;
            }
            out
        }

        fn has_clock(text: &str) -> bool {
            let c: Vec<char> = text.chars().collect();
            c.windows(5).any(|w| {
                w[0].is_ascii_digit()
                    && w[1].is_ascii_digit()
                    && w[2] == ':'
                    && w[3].is_ascii_digit()
                    && w[4].is_ascii_digit()
            })
        }

        fn edge_label(inner: &str, h: &str, top: bool) -> Option<String> {
            let body = inner.trim_start_matches(h);
            if body.is_empty() && !inner.is_empty() {
                return Some(String::new());
            }
            let lead = (inner.len() - body.len()) / h.len();
            let label = body.trim_end_matches(h);
            let tail = (body.len() - label.len()) / h.len();
            let ok = lead >= 1
                && tail >= 1
                && !label.starts_with(' ')
                && !label.ends_with(' ')
                && (label.is_empty() || if top { lead == 1 } else { tail == 1 });
            ok.then(|| label.to_owned())
        }

        fn card_frames(
            buf: &Buffer,
            b: &ratatui::symbols::border::Set,
        ) -> Vec<(u16, u16, u16, u16)> {
            let screen = buf_rows(buf).join("\n");
            let (w, h) = (buf.area.width, buf.area.height);
            let mut out = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    if buf[(x, y)].symbol() != b.top_left || x <= 1 {
                        continue;
                    }
                    let Some(x2) = (x + 1..w).find(|&c| buf[(c, y)].symbol() == b.top_right) else {
                        panic!("card at {x},{y} has no top-right corner\n{screen}");
                    };
                    let top = cell_text(buf, y, x + 1, x2);
                    let title = edge_label(&top, b.horizontal_top, true).unwrap_or_else(|| {
                        panic!("broken top border {top:?} at {x},{y}\n{screen}")
                    });
                    let yb = (y + 1..h)
                        .find(|&r| buf[(x, r)].symbol() != b.vertical_left)
                        .unwrap_or_else(|| panic!("card at {x},{y} runs off\n{screen}"));
                    assert_eq!(
                        buf[(x, yb)].symbol(),
                        b.bottom_left,
                        "left side broken at {x},{yb}\n{screen}"
                    );
                    for r in y + 1..yb {
                        assert_eq!(
                            buf[(x2, r)].symbol(),
                            b.vertical_right,
                            "right side broken at {x2},{r}\n{screen}"
                        );
                        let body = cell_text(buf, r, x + 1, x2);
                        assert!(!has_clock(&body), "time inside the card {body:?}\n{screen}");
                    }
                    assert_eq!(
                        buf[(x2, yb)].symbol(),
                        b.bottom_right,
                        "bottom-right corner at {x2},{yb}\n{screen}"
                    );
                    let bottom = cell_text(buf, yb, x + 1, x2);
                    let foot = edge_label(&bottom, b.horizontal_bottom, false)
                        .unwrap_or_else(|| panic!("broken bottom border {bottom:?}\n{screen}"));
                    assert!(
                        has_clock(&foot),
                        "time in the bottom border {bottom:?}\n{screen}"
                    );
                    assert_eq!(
                        cell_text(buf, y, x, x2 + 1).width(),
                        (x2 - x + 1) as usize,
                        "{screen}"
                    );
                    assert_eq!(
                        cell_text(buf, yb, x, x2 + 1).width(),
                        (x2 - x + 1) as usize,
                        "{screen}"
                    );
                    let _ = title;
                    out.push((x, y, x2, yb));
                }
            }
            out
        }

        #[test]
        fn every_card_has_a_closed_frame_and_one_time_in_its_bottom_border() {
            use crate::config::BorderStyle;
            for style in [
                BorderStyle::Rounded,
                BorderStyle::Plain,
                BorderStyle::Thick,
                BorderStyle::Double,
            ] {
                for tiny in [true, false] {
                    let base = if tiny {
                        tiny_cards()
                    } else {
                        conversation(MessageLayout::Cards)
                    };
                    let n = base.histories[&10].msgs().len();
                    let sels = std::iter::once(None).chain((0..n).map(Some));
                    for sel in sels {
                        let mut app = if tiny {
                            tiny_cards()
                        } else {
                            conversation(MessageLayout::Cards)
                        };
                        app.settings.appearance.border_style = Some(style);
                        app.focus = Pane::Messages;
                        app.msg_sel = sel;
                        let buf = buffer(&mut app, 80, 70);
                        let screen = buf_rows(&buf).join("\n");
                        let set = theme::border_set();
                        let frames = card_frames(&buf, &set);
                        let expect = if tiny { 5 } else { 3 };
                        assert_eq!(frames.len(), expect, "{style:?} sel {sel:?}\n{screen}");
                        let right = frames.iter().filter(|f| f.2 >= 70).count();
                        assert!(
                            right >= 1 && right < frames.len(),
                            "own cards right, incoming left\n{screen}"
                        );
                    }
                }
            }
        }

        #[test]
        fn small_cards_shrink_wrap_with_time_in_the_border() {
            for sel in [None, Some(5), Some(6), Some(7)] {
                let mut app = tiny_cards();
                app.focus = Pane::Messages;
                app.msg_sel = sel;
                let rows = pane(&snapshot(&mut app, 80, 60));
                let screen = rows.join("\n");
                let t = row_of(&rows, "╭─twink─╮");
                assert!(
                    rows[t + 1].contains("│ Ппп   │") || rows[t + 1].contains("│▎Ппп   │"),
                    "{screen}"
                );
                let msgs = app.histories[&10].msgs();
                let a = format::time_label(msgs[5].date);
                assert!(rows[t + 2].trim().starts_with("╰─"), "{screen}");
                assert!(
                    rows[t + 2].trim_end().ends_with(&format!("{a}─╯")),
                    "{screen}"
                );
                let xd = row_of(&rows, "Xd");
                let top = rows[xd - 1].trim();
                assert!(top.starts_with('╭') && top.ends_with('╮'), "{screen}");
                assert!(
                    !has_clock(&rows[xd]) && !has_clock(&rows[xd + 1]),
                    "{screen}"
                );
                let bottom = rows[xd + 2].trim();
                let b = format::time_label(msgs[7].date);
                assert!(
                    bottom.starts_with("╰─") && bottom.contains(&b) && bottom.ends_with("─╯"),
                    "{screen}"
                );
                assert_eq!(rows[xd].width(), rows[xd + 1].width(), "{screen}");
                assert_eq!(rows[xd - 1].width(), rows[xd + 2].width(), "{screen}");
            }
            let mut app = tiny_cards();
            app.settings.appearance.border_style = Some(crate::config::BorderStyle::Plain);
            let rows = pane(&snapshot(&mut app, 80, 60));
            assert!(
                rows.iter().any(|r| r.contains("┌─twink─┐")),
                "{}",
                rows.join("\n")
            );
            assert!(!rows.iter().any(|r| r.contains('╭')), "{}", rows.join("\n"));
        }

        #[test]
        fn cards_show_each_time_once() {
            let mut app = conversation(MessageLayout::Cards);
            let rows = pane(&snapshot(&mut app, 80, 40));
            let screen = rows.join("\n");
            let msgs = app.histories[&10].msgs();
            let own = format::time_label(msgs[2].date);
            assert_eq!(
                format::time_label(msgs[3].date),
                own,
                "fixture: same minute"
            );
            assert_eq!(screen.matches(&own).count(), 1, "{screen}");
            let own_bottom = (row_of(&rows, "screenshot")..rows.len())
                .find(|&i| rows[i].contains('╰'))
                .unwrap();
            assert!(rows[own_bottom].contains(&format!("{own} ✓─")), "{screen}");
            let first = format::time_label(msgs[0].date);
            let last = format::time_label(msgs[1].date);
            assert_ne!(first, last, "fixture: different minutes");
            assert_eq!(
                screen.matches(&first).count(),
                0,
                "one time per card\n{screen}"
            );
            assert_eq!(screen.matches(&last).count(), 1, "{screen}");
        }

        #[test]
        fn classic_edited_marker_is_dim_and_not_glued() {
            let mut app = conversation(MessageLayout::Classic);
            app.lang = format::Lang::from_locale(Some("ru_RU.UTF-8"));
            let buf = buffer(&mut app, 80, 50);
            let rows = buf_rows(&buf);
            let y = pane_end(&rows);
            assert!(!rows[y].contains("pane изм."), "{}", rows[y]);
            let (x, my) = find(&buf, "изм.");
            assert_eq!(my as usize, y);
            assert!(x as usize > col_of(&rows[y], "pane") + 15, "{}", rows[y]);
            assert_eq!(
                buf[(x, my)].style().add_modifier,
                theme::edited().add_modifier
            );
            app.histories
                .get_mut(&10)
                .unwrap()
                .get_mut(1)
                .unwrap()
                .edited = true;
            let rows = snapshot(&mut app, 80, 50);
            let header = &rows[row_of(&rows, "Alice Longname ")];
            assert!(header.contains("· изм."), "edited header message: {header}");
        }

        #[test]
        fn text_layouts_mark_every_row_of_the_selected_message() {
            for layout in [
                MessageLayout::Classic,
                MessageLayout::Compact,
                MessageLayout::Minimal,
                MessageLayout::Split,
            ] {
                let mut app = conversation(layout);
                app.msg_sel = Some(1);
                app.focus = Pane::Messages;
                let buf = buffer(&mut app, 80, 50);
                let rows = buf_rows(&buf);
                let screen = rows.join("\n");
                let first = row_of(&rows, "the changelog");
                let last = pane_end(&rows);
                assert!(last > first, "{layout:?} wraps\n{screen}");
                let bar = theme::get().glyphs.selection_bar.clone();
                for y in first..=last {
                    let cell = &buf[(1, y as u16)];
                    assert_eq!(cell.symbol(), bar, "{layout:?} row {y}\n{screen}");
                    assert_eq!(cell.fg, theme::selection_bar().fg.unwrap(), "{layout:?}");
                    for x in [2u16, 40, 77] {
                        assert_eq!(
                            Some(buf[(x, y as u16)].bg),
                            theme::selection_bg(),
                            "{layout:?} tint at ({x},{y})\n{screen}"
                        );
                    }
                }
                let hey = row_of(&rows, "hey, did you");
                assert_ne!(buf[(1, hey as u16)].symbol(), bar, "{layout:?}\n{screen}");
                assert!(!screen.contains("╭─"), "{layout:?} draws no box\n{screen}");
                app.msg_sel = Some(0);
                let buf = buffer(&mut app, 80, 50);
                let rows = buf_rows(&buf);
                let name = (0..rows.len())
                    .find(|&y| rows[y].contains("Alice Longn") && !rows[y].contains("│ Alice"))
                    .unwrap();
                let (nx, _) = find_in(&buf, "Alice Longn", 0..80);
                assert_eq!(
                    buf[(nx, name as u16)].fg,
                    theme::active_field().fg.unwrap(),
                    "{layout:?} name in accent"
                );
            }
        }

        #[test]
        fn split_puts_own_messages_right_and_others_left() {
            let mut app = conversation(MessageLayout::Split);
            let rows = pane(&snapshot(&mut app, 80, 50));
            let screen = rows.join("\n");
            assert!(!screen.contains('▄'), "{screen}");
            let gutter = MSG_GUTTER;
            let hey = row_of(&rows, "hey, did you");
            assert_eq!(col_of(&rows[hey], "hey"), gutter, "{screen}");
            let alice = row_of(&rows, "Alice Longname ");
            assert_eq!(col_of(&rows[alice], "Alice"), gutter, "{screen}");
            let mine = row_of(&rows, "upgrading tonight");
            assert_eq!(rows[mine].width(), 78 - gutter, "{screen}");
            let me = row_of(&rows, "Me ");
            assert!(rows[me].ends_with("✓✓"), "{screen}");
            assert_eq!(
                rows[me].width(),
                78 - gutter,
                "header on its side\n{screen}"
            );
            let strip = row_of(&rows, "│ Alice Longname: ");
            assert_eq!(rows[strip].width(), 78 - gutter, "{screen}");
            assert!(col_of(&rows[strip], "│ Alice") >= 78 - 78 * 70 / 100 - gutter);
            let wrap = row_of(&rows, "the changelog");
            assert!(rows[wrap].width() <= gutter + 78 * 70 / 100, "{screen}");
            let bob = row_of(&rows, "Bob ");
            assert_eq!(col_of(&rows[bob], "Bob"), gutter);
        }

        fn keyboard_msg(app: &mut App) {
            use crate::bots::{Button, ButtonAction, Keyboard};
            let h = app.histories.get_mut(&10).unwrap();
            let m = h.get_mut(5).unwrap();
            m.keyboard = Some(Keyboard {
                inline: true,
                rows: vec![
                    vec![Button {
                        text: "Участвовать".to_owned(),
                        action: ButtonAction::Url("https://example.org".to_owned()),
                    }],
                    vec![
                        Button {
                            text: "Да".to_owned(),
                            action: ButtonAction::Callback("eQ==".to_owned()),
                        },
                        Button {
                            text: "Нет".to_owned(),
                            action: ButtonAction::Callback("bg==".to_owned()),
                        },
                    ],
                ],
                placeholder: String::new(),
            });
        }

        #[test]
        fn inline_keyboards_render_as_separate_rounded_buttons() {
            for layout in MessageLayout::ALL {
                let mut app = conversation(layout);
                keyboard_msg(&mut app);
                let buf = buffer(&mut app, 90, 40);
                let rows = buf_rows(&buf);
                let screen = rows.join("\n");
                let y = row_of(&rows, "│ Участвовать ↗ │");
                assert!(
                    rows[y - 1].contains("╭───────────────╮"),
                    "{layout:?}\n{screen}"
                );
                assert!(
                    rows[y + 1].contains("╰───────────────╯"),
                    "{layout:?}\n{screen}"
                );
                let yes = row_of(&rows, "Да");
                assert_eq!(yes, y + 3, "{layout:?}\n{screen}");
                assert_eq!(
                    rows[yes].matches('│').count() - rows[y].matches('│').count(),
                    2
                );
                let x0 = col_of(&rows[y], "│ Участвовать");
                let row_end = x0 + "│ Участвовать ↗ │".width();
                let da_end = col_of(&rows[yes], "│ Нет") + "│ Нет  │".width();
                assert_eq!(da_end, row_end, "{layout:?} rows share a width\n{screen}");
                assert!(!screen.contains("[ "), "{layout:?}\n{screen}");
                let (bx, by) = find(&buf, "Участвовать");
                assert_eq!(buf[(bx, by)].fg, theme::button_text().fg.unwrap());
                assert_eq!(buf[(bx - 2, by)].fg, theme::button_border().fg.unwrap());
                let id = app.histories[&10].get(5).unwrap().id;
                let kb = app.histories[&10].get(5).unwrap().keyboard.clone().unwrap();
                app.popup = Some(Popup::Buttons(crate::app::ButtonsView {
                    chat_id: 10,
                    msg_id: id,
                    keyboard: kb,
                    sel: 2,
                }));
                let buf = buffer(&mut app, 90, 40);
                let focused = theme::button_selected();
                let hits: Vec<(u16, u16)> = (0..40u16)
                    .flat_map(|y| (0..90u16).map(move |x| (x, y)))
                    .filter(|&(x, y)| {
                        buf[(x, y)].symbol() == "Н" && buf[(x, y)].bg == focused.bg.unwrap()
                    })
                    .collect();
                assert!(hits.len() >= 2, "{layout:?}: focused in message and popup");
                let (dx, dy) = find(&buf, "Да");
                assert_ne!(buf[(dx, dy)].bg, focused.bg.unwrap(), "{layout:?}");
            }
        }

        #[test]
        fn reply_keyboard_sits_above_the_composer_as_buttons() {
            use crate::bots::{Button, ButtonAction, Keyboard};
            let mut app = conversation(MessageLayout::Classic);
            let h = app.histories.get_mut(&10).unwrap();
            let m = h.get_mut(5).unwrap();
            m.keyboard = Some(Keyboard {
                inline: false,
                rows: vec![vec![
                    Button {
                        text: "Меню".to_owned(),
                        action: ButtonAction::Text,
                    },
                    Button {
                        text: "Помощь".to_owned(),
                        action: ButtonAction::Text,
                    },
                ]],
                placeholder: "Выберите".to_owned(),
            });
            app.set_reply_keyboard_id(10, 5);
            let rows = snapshot(&mut app, 80, 30);
            let screen = rows.join("\n");
            let composer = row_of(&rows, "╭ Message");
            let mid = row_of(&rows, "Помощь");
            assert!(mid > composer, "{screen}");
            assert!(
                rows[mid].contains("│  Меню  │") || rows[mid].contains("│ Меню"),
                "{screen}"
            );
            assert!(
                rows[mid - 1].contains('╭') && rows[mid + 1].contains('╰'),
                "{screen}"
            );
            assert!(rows[mid - 2].contains("C-k") && rows[mid - 2].contains("Выберите"));
            app.popup = Some(Popup::Buttons(crate::app::ButtonsView {
                chat_id: 10,
                msg_id: 5,
                keyboard: app.reply_keyboard().unwrap().1.clone(),
                sel: 1,
            }));
            let buf = buffer(&mut app, 80, 30);
            let focused = theme::button_selected().bg.unwrap();
            let lit = (0..30u16)
                .flat_map(|y| (0..80u16).map(move |x| (x, y)))
                .filter(|&(x, y)| buf[(x, y)].symbol() == "П" && buf[(x, y)].bg == focused)
                .count();
            assert_eq!(lit, 2, "composer row and popup show the focus");
        }

        #[test]
        fn date_separators_follow_the_russian_locale() {
            use chrono::TimeZone;
            let at = |d: u32, h: u32| {
                chrono::Local
                    .with_ymd_and_hms(2026, 11, d, h, 0, 0)
                    .single()
                    .unwrap()
                    .timestamp()
            };
            let lang = format::Lang::from_locale(Some("ru_RU.UTF-8"));
            assert_eq!(lang, format::Lang::Ru);
            for layout in MessageLayout::ALL {
                let mut app = conversation(layout);
                let h = app.histories.get_mut(&10).unwrap();
                h.get_mut(1).unwrap().date = at(7, 12);
                h.get_mut(2).unwrap().date = at(7, 12) + 60;
                for id in 3..=5 {
                    h.get_mut(id).unwrap().date = at(9, 10) + id * 60;
                }
                let ctx = Ctx {
                    width: 70,
                    layout,
                    theme: 0,
                    now: at(9, 15),
                    today: 0,
                    lang,
                    sel: None,
                };
                let msgs = app.histories[&10].msgs();
                let mut text = String::new();
                for range in model::groups(msgs) {
                    let mut cache = LayoutCache::default();
                    let r = cached_block(&app, &mut cache, msgs, range, ctx, false);
                    for l in r.lines {
                        text.extend(l.spans.iter().map(|s| s.content.to_string()));
                        text.push('\n');
                    }
                }
                assert!(text.contains(" 7 ноября "), "{layout:?}\n{text}");
                assert!(text.contains(" Сегодня "), "{layout:?}\n{text}");
                assert!(!text.contains("November") && !text.contains("Today"));
            }
        }

        #[test]
        #[ignore = "writes .work/layouts-preview.txt"]
        fn write_layouts_preview() {
            use crate::bots::{Button, ButtonAction, Keyboard};
            let mut out = String::new();
            for layout in MessageLayout::ALL {
                for (label, sel) in [("", None), (" · selected", Some(1usize))] {
                    let mut app = conversation(layout);
                    app.lang = format::Lang::from_locale(Some("ru_RU.UTF-8"));
                    let h = app.histories.get_mut(&10).unwrap();
                    let mut shot = h.msgs()[3].clone();
                    let mut doc = Attachment::new(MediaKind::Document, file(901, false));
                    doc.name = "release-notes.pdf".to_owned();
                    shot.attachments = vec![doc];
                    h.upsert(shot);
                    let mut bob = h.msgs()[4].clone();
                    bob.keyboard = Some(Keyboard {
                        inline: true,
                        rows: vec![
                            vec![Button {
                                text: "Участвовать".to_owned(),
                                action: ButtonAction::Url("https://example.org".to_owned()),
                            }],
                            vec![
                                Button {
                                    text: "Да".to_owned(),
                                    action: ButtonAction::Callback("eQ==".to_owned()),
                                },
                                Button {
                                    text: "Нет".to_owned(),
                                    action: ButtonAction::Callback("bg==".to_owned()),
                                },
                            ],
                        ],
                        placeholder: String::new(),
                    });
                    h.upsert(bob);
                    app.msg_sel = sel;
                    if sel.is_some() {
                        app.focus = Pane::Messages;
                    }
                    let (w, rows) = (70u16, 40u16);
                    let mut term = Terminal::new(TestBackend::new(w, rows)).unwrap();
                    term.draw(|f| draw(f, &mut app)).unwrap();
                    let buf = term.backend().buffer().clone();
                    out.push_str(&format!("=== {}{label} ===\n", layout.name()));
                    for y in 0..rows {
                        let mut row = String::new();
                        let mut x = 0;
                        while x < w {
                            let sym = buf[(x, y)].symbol();
                            row.push_str(sym);
                            x += sym.width().max(1) as u16;
                        }
                        out.push_str(row.trim_end());
                        out.push('\n');
                    }
                    out.push('\n');
                }
            }
            let path =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".work/layouts-preview.txt");
            std::fs::write(path, out).unwrap();
        }
    }

    mod stray_cells {
        use super::*;
        use crate::config::MessageLayout;
        use crate::model::Reaction;

        const ALL: [MessageLayout; 6] = [
            MessageLayout::Classic,
            MessageLayout::Bubbles,
            MessageLayout::Cards,
            MessageLayout::Compact,
            MessageLayout::Minimal,
            MessageLayout::Split,
        ];
        const AVATAR_RED: Color = Color::Rgb(200, 30, 30);
        const PARTY: i64 = 5_368_324_170_671_202_286;

        fn react(label: &str, custom: i64, count: i32, chosen: bool) -> Reaction {
            Reaction {
                label: label.to_owned(),
                emoji: (custom == 0).then(|| label.to_owned()),
                custom,
                count,
                chosen,
            }
        }

        fn chat_scene(layout: MessageLayout) -> App {
            let mut app = app();
            app.settings.appearance.layout = Some(layout);
            app.settings.appearance.seconds = Some(true);
            app.show_info = false;
            app.show_folders = false;
            app.show_chats = false;
            app.my_id = Some(1);
            app.users.insert(1, "mihail".to_owned());
            app.users.insert(2, "рустем".to_owned());
            let now = chrono::Local::now().timestamp();
            let base = now - now.rem_euclid(86_400) + 12 * 3600 - 86_400;
            let plain = |id: i64, sender: i64, date: i64, text: &str| {
                let mut m = msg(id, sender, date, text);
                m.body = RichText::plain(text);
                m.edited = false;
                m.reply_to = None;
                m.media = None;
                m
            };
            let mut batch = vec![
                plain(1, 2, base, "не впускай"),
                plain(2, 1, base + 10, "будет фотки"),
                plain(3, 1, base + 12, "ещё"),
                plain(4, 2, base + 20, "ДААДАДАДДА"),
                plain(5, 2, base + 22, "ага"),
                plain(6, 1, base + 30, "выкладывать"),
                plain(7, 1, base + 31, ""),
                plain(8, 2, base + 40, "ок"),
            ];
            batch[0].reactions = vec![react("👍", 0, 1, false)];
            batch[1].reactions = vec![react("❤", 0, 1, false)];
            batch[2].reactions = vec![react("•", PARTY, 2, true)];
            batch[3].reactions = vec![react("🔥", 0, 1, false), react("•", PARTY, 1, false)];
            batch[5].reactions = vec![react("😁", 0, 3, false)];
            batch[6].attachments = vec![photo(900)];
            let mut h = History::default();
            h.reached_start = true;
            h.merge_older(batch);
            app.histories.insert(10, h);
            let chat = app.chats.get_mut(&10).unwrap();
            chat.last_read_outbox = 10;
            chat.unread = 0;
            for (uid, fid) in [(1i64, 701), (2, 702)] {
                app.user_meta.entry(uid).or_default().photo = Some(crate::model::FileRef {
                    id: fid,
                    path: format!("/nonexistent/avatar{fid}.png"),
                    done: true,
                    ..crate::model::FileRef::default()
                });
            }
            for (cols, rows) in [(2, 1), (4, 2)] {
                for fid in [701, 702] {
                    for shape in [
                        crate::theme::AvatarShape::Circle,
                        crate::theme::AvatarShape::Rounded,
                        crate::theme::AvatarShape::Square,
                    ] {
                        let key = Key {
                            src: Src::File(fid),
                            cols,
                            rows,
                            shape: Some(shape),
                            animate: false,
                        };
                        let frames = ready_frames(&app, cols, rows);
                        app.media.insert_ready(key, frames, 0);
                    }
                }
            }
            app
        }

        fn pane_rows(app: &mut App) -> Vec<String> {
            let buf = buffer(app, 140, 60);
            (0..60u16)
                .map(|y| {
                    let mut row = String::new();
                    let mut x = 0u16;
                    while x < 140 {
                        let sym = buf[(x, y)].symbol();
                        row.push_str(sym);
                        x += (sym.width() as u16).max(1);
                    }
                    row
                })
                .skip(2)
                .take_while(|r| !r.contains("╭ Message"))
                .map(|r| r.trim_matches(|c: char| c == '│' || c == ' ').to_owned())
                .collect()
        }

        fn avatar_cells(app: &mut App) -> usize {
            let buf = buffer(app, 140, 60);
            buf.content
                .iter()
                .filter(|c| c.fg == AVATAR_RED || c.bg == AVATAR_RED)
                .count()
        }

        #[test]
        fn reactions_render_as_emoji_count_chips_in_every_layout() {
            for layout in ALL {
                let mut app = chat_scene(layout);
                app.custom_emoji.insert(PARTY, "🎉".to_owned());
                let rows = pane_rows(&mut app);
                let screen = rows.join("\n");
                for chip in ["👍 1", "❤\u{FE0F} 1", "🎉 2•", "😁 3"] {
                    assert!(screen.contains(chip), "{layout:?} {chip}\n{screen}");
                }
                assert!(
                    rows.iter()
                        .any(|r| r.contains("🔥 1") && r.contains("🎉 1")),
                    "{layout:?}\n{screen}"
                );
                assert!(
                    !screen.split_whitespace().any(|w| w == "•"),
                    "{layout:?}\n{screen}"
                );
                for r in &rows {
                    let lone = r
                        .split(|c: char| c == '│' || c.is_whitespace())
                        .filter(|w| !w.is_empty())
                        .any(|w| ["👍", "❤\u{FE0F}", "🔥", "😁", "🎉"].contains(&w))
                        && !r.chars().any(|c| c.is_ascii_digit());
                    assert!(!lone, "{layout:?} bare reaction glyph: [{r}]\n{screen}");
                }
            }
        }

        #[test]
        fn reaction_chips_have_terminal_widths_and_repaint_cleanly_while_selecting() {
            let qualified = [
                "❤\u{FE0F}",
                "👍🏽",
                "👨\u{200D}👩\u{200D}👧",
                "1\u{FE0F}\u{20E3}",
                "❤\u{FE0F}\u{200D}🔥",
                "🤷\u{200D}♂\u{FE0F}",
                "🎉",
            ];
            let raw = ["❤\u{200D}🔥", "🤷\u{200D}♂", "1\u{20E3}"];
            for layout in ALL {
                let mut app = chat_scene(layout);
                {
                    let h = app.histories.get_mut(&10).unwrap();
                    h.get_mut(1).unwrap().reactions =
                        vec![react("❤", 0, 1, false), react("👍🏽", 0, 2, false)];
                    h.get_mut(4).unwrap().reactions = vec![
                        react("👨\u{200D}👩\u{200D}👧", 0, 1, false),
                        react("1\u{20E3}", 0, 1, false),
                    ];
                    h.get_mut(5).unwrap().reactions = vec![
                        react("❤\u{200D}🔥", 0, 4, true),
                        react("🤷\u{200D}♂", 0, 1, false),
                        react("•", PARTY, 1, false),
                    ];
                }
                let mut term = Terminal::new(TestBackend::new(140, 60)).unwrap();
                for step in 0..12 {
                    if step == 6 {
                        app.custom_emoji.insert(PARTY, "🎉".to_owned());
                    }
                    app.msg_sel = match step % 6 {
                        0 => None,
                        n => Some(8 - n),
                    };
                    term.draw(|f| draw(f, &mut app)).unwrap();
                    let incremental = term.backend().buffer().clone();
                    let fresh = buffer(&mut app, 140, 60);
                    let syms = |b: &Buffer| -> Vec<String> {
                        b.content.iter().map(|c| c.symbol().to_owned()).collect()
                    };
                    let (a, b) = (syms(&incremental), syms(&fresh));
                    let diff: Vec<(usize, usize, &String, &String)> = a
                        .iter()
                        .zip(&b)
                        .enumerate()
                        .filter(|(i, (x, y))| {
                            let hidden =
                                i % 140 > 0 && crate::format::grapheme_cells(&b[i - 1]) == 2;
                            x != y && !hidden
                        })
                        .map(|(i, (x, y))| (i % 140, i / 140, x, y))
                        .collect();
                    assert!(diff.is_empty(), "{layout:?} step {step}: {diff:?}");
                    let on_screen: Vec<&str> = fresh.content.iter().map(|c| c.symbol()).collect();
                    for q in qualified.iter().take(6) {
                        assert!(
                            on_screen.contains(q),
                            "{layout:?} step {step}: {q:?} missing"
                        );
                        assert_eq!(crate::format::grapheme_cells(q), 2);
                    }
                    assert_eq!(
                        on_screen.contains(&"🎉"),
                        step >= 6,
                        "{layout:?} step {step}"
                    );
                    for r in raw.iter().chain(["❤", "◆", "\u{25A1}"].iter()) {
                        assert!(!on_screen.contains(r), "{layout:?} step {step}: raw {r:?}");
                    }
                    for (i, c) in fresh.content.iter().enumerate() {
                        let sym = c.symbol();
                        if crate::format::grapheme_cells(sym) == 2 && sym.chars().count() > 1 {
                            assert!(
                                qualified.contains(&sym) || sym.chars().all(|ch| ch.len_utf8() < 4),
                                "{layout:?} step {step}: unexpected wide cluster {sym:?} at {i}"
                            );
                        }
                    }
                }
            }
        }

        #[tokio::test]
        async fn custom_emoji_reactions_are_resolved_once_and_redrawn() {
            let mut app = chat_scene(MessageLayout::Split);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let (td, log) = crate::td::Td::recording(tx);
            app.td = td;
            let asked = |log: &crate::td::RequestLog| {
                log.lock()
                    .unwrap()
                    .iter()
                    .filter(|w| w.as_str() == "custom emoji")
                    .count()
            };
            let rows = pane_rows(&mut app);
            assert!(rows.iter().any(|r| r.contains("• 2•")), "{rows:#?}");
            app.after_draw();
            pane_rows(&mut app);
            app.after_draw();
            assert_eq!(asked(&log), 1);
            app.handle(Event::CustomEmoji(Ok(std::collections::HashMap::from([(
                PARTY,
                "🎉".to_owned(),
            )]))));
            let screen = pane_rows(&mut app).join("\n");
            assert!(
                screen.contains("🎉 2•") && !screen.split_whitespace().any(|w| w == "•"),
                "{screen}"
            );
            app.after_draw();
            assert_eq!(asked(&log), 1);
            app.handle(Event::CustomEmoji(Err("boom".to_owned())));
            assert!(
                app.status
                    .as_ref()
                    .is_some_and(|(s, err)| *err && s.contains("boom"))
            );
        }

        #[test]
        fn split_draws_no_avatars_and_text_layouts_never_draw_tiny_avatar_images() {
            for layout in ALL {
                let mut app = chat_scene(layout);
                let drawn = avatar_cells(&mut app);
                if layout == MessageLayout::Bubbles {
                    assert!(
                        drawn > 0 && drawn.is_multiple_of(4 * 2),
                        "{layout:?} keeps its 4x2 photo avatars: {drawn}"
                    );
                } else {
                    assert_eq!(drawn, 0, "{layout:?} draws an avatar image");
                }
            }
            let mut app = chat_scene(MessageLayout::Classic);
            let rows = pane_rows(&mut app);
            let header = row_of(&rows, "рустем ");
            assert!(rows[header].starts_with("Р рустем"), "{rows:#?}");
            let mut app = chat_scene(MessageLayout::Bubbles);
            let mut t = crate::theme::Theme::default();
            t.layout.bubble_avatar_rows = 1;
            app.set_theme(t);
            assert_eq!(
                avatar_cells(&mut app),
                0,
                "1-row bubble avatar stays initials"
            );
            assert!(pane_rows(&mut app).iter().any(|r| r.starts_with("Р")));
        }

        fn emoji_chat(layout: MessageLayout) -> App {
            let mut app = chat_scene(layout);
            app.focus = Pane::Messages;
            let now = chrono::Local::now().timestamp();
            let base = now - now.rem_euclid(86_400) + 12 * 3600 - 86_400;
            let texts = [
                "привет",
                "ок 👍🏽",
                "сейчас скину 中文 1\u{20E3}",
                "да ❤\u{200D}🔥 raw",
                "хаха смешно 😀😀",
                "ну 🤷\u{200D}♂\u{FE0F} ✓",
            ];
            let batch = (0..40i64)
                .map(|i| {
                    let sender = if i % 3 == 0 { 2 } else { 1 };
                    let mut m = msg(i + 1, sender, base + i * 70, "");
                    m.body = RichText::plain(texts[i as usize % texts.len()]);
                    m.edited = false;
                    m.reply_to = None;
                    m.media = None;
                    if i % 2 == 0 {
                        let count = if sender == 1 { 3 } else { 1 };
                        m.reactions = vec![react("❤\u{200D}🔥", 0, count, i % 4 == 0)];
                    }
                    m
                })
                .collect();
            let mut h = History::default();
            h.reached_start = true;
            h.merge_older(batch);
            app.histories.insert(10, h);
            app.chats.get_mut(&10).unwrap().last_read_outbox = 20;
            app
        }

        #[test]
        fn moving_through_emoji_reactions_leaves_no_stale_cells_in_kitty() {
            use crate::ui::repaint_tests::{Screen, Sink};
            use crossterm::event::{KeyCode, KeyModifiers};
            let area = Rect::new(0, 0, 120, 30);
            let keys = [
                (KeyCode::Up, 25),
                (KeyCode::Down, 25),
                (KeyCode::PageUp, 4),
                (KeyCode::PageDown, 4),
                (KeyCode::Char('K'), 8),
                (KeyCode::Char('J'), 8),
            ];
            for layout in ALL {
                let mut app = emoji_chat(layout);
                let sink = Sink::default();
                let mut term = Terminal::with_options(
                    Resync(CrosstermBackend::new(sink.clone())),
                    ratatui::TerminalOptions {
                        viewport: ratatui::Viewport::Fixed(area),
                    },
                )
                .unwrap();
                let mut screen = Screen::new(area.width, area.height);
                let mut hash = image_layout(&Buffer::empty(area));
                let mut fire = 0;
                let presses = keys
                    .iter()
                    .flat_map(|&(k, n)| std::iter::repeat_n(Some(k), n));
                for (step, key) in std::iter::once(None).chain(presses).enumerate() {
                    if let Some(k) = key {
                        let mods = if matches!(k, KeyCode::Char(c) if c.is_uppercase()) {
                            KeyModifiers::SHIFT
                        } else {
                            KeyModifiers::NONE
                        };
                        press(&mut app, k, mods);
                    }
                    let mut shown = None;
                    draw_frame(&mut term, &mut app, &mut hash, |b| shown = Some(b.clone()))
                        .unwrap();
                    let shown = shown.unwrap();
                    screen.feed(&std::mem::take(&mut *sink.0.borrow_mut()));
                    fire += shown
                        .content
                        .iter()
                        .filter(|c| c.symbol() == "❤\u{FE0F}\u{200D}🔥")
                        .count();
                    let stale = screen.stale_cells(&shown);
                    assert!(
                        stale.is_empty(),
                        "{layout:?} step {step} ({key:?}): {} stale cells, e.g. {:?}",
                        stale.len(),
                        &stale[..stale.len().min(6)]
                    );
                }
                assert!(fire > 100, "{layout:?}: {fire}");
            }
        }
    }

    fn screen_rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
            .collect()
    }

    fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
        rows.iter()
            .find(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not on screen:\n{}", rows.join("\n")))
    }

    #[test]
    fn typing_sits_in_the_composer_border_without_moving_anything() {
        use crate::presence::{Activity, Typing};
        let mut app = app();
        app.lang = format::Lang::Ru;
        app.show_info = false;
        app.show_folders = false;
        app.show_chats = false;
        app.users.insert(2, "рустем Гарипов".to_owned());
        app.users.insert(3, "ильнур".to_owned());
        app.users.insert(4, "Константинопольский".to_owned());
        let quiet = snapshot(&mut app, 100, 30);
        let typing = |who: &[i64], act: Activity, topic: Option<i32>| -> Vec<Typing> {
            who.iter()
                .map(|u| Typing::new(Sender::User(*u), act, topic))
                .collect()
        };
        let border = quiet.iter().position(|r| r.contains("╭ Message ")).unwrap();
        for (who, act, want) in [
            (&[2][..], Activity::Typing, " Message · рустем печатает… "),
            (
                &[2, 3][..],
                Activity::Typing,
                " Message · рустем, ильнур печатают… ",
            ),
            (
                &[2, 3, 4][..],
                Activity::Typing,
                " Message · рустем и ещё 2 печатают… ",
            ),
            (
                &[2][..],
                Activity::UploadingPhoto,
                " Message · рустем отправляет фото… ",
            ),
            (
                &[2][..],
                Activity::UploadingFile,
                " Message · рустем отправляет файл… ",
            ),
        ] {
            app.typing.insert(10, typing(who, act, None));
            let rows = snapshot(&mut app, 100, 30);
            for y in 0..30 {
                if y == border {
                    assert!(rows[y].contains(want), "{}", rows[y]);
                    assert_eq!(rows[y].chars().count(), quiet[y].chars().count());
                    assert!(rows[y].trim_end_matches('│').ends_with('╮'), "{}", rows[y]);
                } else {
                    assert_eq!(rows[y], quiet[y], "row {y} moved");
                }
            }
            assert!(
                !rows[1].contains("печата"),
                "header keeps no typing: {}",
                rows[1]
            );
        }
        app.users
            .insert(2, "Длинноеимякотороенепомещаетсявзаголовокрамки".to_owned());
        app.typing
            .insert(10, typing(&[2, 3, 4], Activity::RecordingVoice, None));
        for w in [40u16, 60, 100] {
            let saved = app.typing.remove(&10).unwrap();
            let calm = snapshot(&mut app, w, 30);
            app.typing.insert(10, saved);
            let rows = snapshot(&mut app, w, 30);
            let y = rows.iter().position(|r| r.contains("╭ Message ")).unwrap();
            assert_eq!(
                rows[y].chars().count(),
                calm[y].chars().count(),
                "{}",
                rows[y]
            );
            assert!(
                rows[y].trim_end_matches('│').ends_with('╮'),
                "{w}: {}",
                rows[y]
            );
            assert!(rows[y].contains('…'), "{w}: {}", rows[y]);
            assert_eq!(rows.len(), calm.len());
        }
        app.open_topic = Some(7);
        app.typing
            .insert(10, typing(&[3], Activity::Typing, Some(8)));
        let rows = snapshot(&mut app, 100, 30);
        assert!(
            !rows.iter().any(|r| r.contains("печата")),
            "other topic stays quiet"
        );
        app.typing
            .insert(10, typing(&[3], Activity::Typing, Some(7)));
        let rows = snapshot(&mut app, 100, 30);
        assert!(
            rows.iter()
                .any(|r| r.contains(" Message · ильнур печатает… "))
        );
        app.settings.privacy.ghost_mode = true;
        let rows = snapshot(&mut app, 100, 30);
        assert!(
            rows.iter().any(|r| r.contains("ильнур печатает…")),
            "ghost mode only hides our own actions"
        );
    }

    #[test]
    fn loading_older_shows_late_in_a_fixed_row_without_moving_messages() {
        let mut app = app();
        app.show_info = false;
        let h = app.histories.get_mut(&10).unwrap();
        h.reached_start = false;
        h.start_older();
        let quiet = snapshot(&mut app, 100, 30);
        assert!(
            !quiet.iter().any(|r| r.contains("Loading older")),
            "{}",
            quiet.join("\n")
        );
        assert!(app.animating, "keeps ticking until the delay passes");
        app.histories.get_mut(&10).unwrap().older_since =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let slow = snapshot(&mut app, 100, 30);
        let changed: Vec<usize> = (0..30).filter(|y| quiet[*y] != slow[*y]).collect();
        assert_eq!(changed.len(), 1, "{}", slow.join("\n"));
        assert!(
            slow[changed[0]].contains("Loading older"),
            "{}",
            slow[changed[0]]
        );
        assert!(!app.animating);
        let row = changed[0];
        for _ in 0..3 {
            let again = snapshot(&mut app, 100, 30);
            assert!(again[row].contains("Loading older"));
        }
        app.handle(Event::History {
            chat_id: 10,
            msgs: Vec::new(),
            initial: false,
            epoch: app.histories[&10].epoch,
        });
        let done = snapshot(&mut app, 100, 30);
        assert!(!done.iter().any(|r| r.contains("Loading older")));
    }

    #[test]
    fn wide_titles_never_push_badges_past_the_pane_border() {
        let mut app = app();
        app.show_folders = true;
        app.show_info = false;
        app.folders = vec![(3, "🅰\u{FE0F}⚡▸ Работа и ещё много слов".to_owned())];
        let base = app.chats[&10].clone();
        let c10 = app.chats.get_mut(&10).unwrap();
        c10.title = "🅰\u{FE0F}⚡▸ Очень длинный канал про всё".to_owned();
        c10.forum = true;
        let extra = [
            (11, "✍ ❤\u{200D}🔥 Writers club ✍✍✍✍", 120, false),
            (12, "ㅤ", 5, false),
            (13, "🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸", 7, false),
            (14, "▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸▸", 2, true),
            (15, "⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡⚡", 3000, false),
        ];
        for (id, title, unread, muted) in extra {
            let mut c = base.clone();
            c.id = id;
            c.title = title.to_owned();
            c.unread = unread;
            c.mentions = 0;
            c.muted = muted;
            c.positions[0].order = 100 - id;
            c.positions[0].pinned = false;
            app.chats.insert(id, c);
        }
        app.expanded = Some(10);
        let mut topic = crate::model::Topic {
            id: 2,
            name: "⚡🅰\u{FE0F} Тема с длинным именем ✍".to_owned(),
            order: 50,
            use_default_mute: true,
            ..crate::model::Topic::default()
        };
        topic.icon = "✍".to_owned();
        topic.unread = 9;
        topic.mentions = 1;
        topic.pinned = true;
        let mut list = crate::model::TopicList::default();
        list.merge_page(vec![topic], None, 1);
        app.topics.insert(10, list);
        app.refresh_lists();
        for (compact, width) in [false, true]
            .into_iter()
            .flat_map(|c| [60u16, 80, 100, 140].map(|w| (c, w)))
        {
            app.settings.appearance.compact_chat_list = compact;
            let mut term = Terminal::new(TestBackend::new(width, 30)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer().clone();
            let corners: Vec<u16> = (0..width)
                .filter(|x| buf[(*x, 1)].symbol() == "╮")
                .collect();
            assert!(corners.len() >= 2, "{width}: {corners:?}");
            let mut lefts = vec![0u16];
            lefts.extend(corners.iter().map(|c| c + 1));
            let mut seen = std::collections::HashSet::new();
            for (pane, right) in corners.iter().take(2).enumerate() {
                let left = lefts[pane];
                let mut texts = Vec::new();
                for y in 2..29u16 {
                    assert_eq!(
                        buf[(*right, y)].symbol(),
                        "│",
                        "{width}: border at ({right},{y})"
                    );
                    let mut text = String::new();
                    let mut x = left + 1;
                    while x < *right {
                        let sym = buf[(x, y)].symbol();
                        let cw = crate::format::grapheme_cells(sym).max(1) as u16;
                        assert!(
                            x + cw <= *right,
                            "{width}: {sym:?} at ({x},{y}) crosses the border"
                        );
                        for bad in ["✍", "ㅤ", "🇺", "🇸"] {
                            assert_ne!(sym, bad, "{width}: unsafe glyph at ({x},{y})");
                        }
                        text.push_str(sym);
                        x += cw;
                    }
                    texts.push(text.trim_end().to_owned());
                }
                for (i, text) in texts.iter().enumerate() {
                    for (needle, badge) in [
                        ("Writers", "120"),
                        ("🇺🇸", "7"),
                        ("▸▸▸", "2"),
                        ("⚡⚡⚡", "3K"),
                        ("Очень", "9"),
                        ("Тема", "9"),
                    ] {
                        if text.contains(needle) {
                            let two_lines = !compact && needle != "Тема";
                            let row = if two_lines { &texts[i + 1] } else { text };
                            assert!(
                                row.ends_with(badge),
                                "{width}: [{row}] under [{text}] should end with {badge}"
                            );
                            seen.insert(needle);
                        }
                    }
                    if pane == 0 && text.contains("Работа") {
                        seen.insert("Работа");
                    }
                }
            }
            let needles: &[&str] = if width >= 100 {
                &["Writers", "🇺🇸", "▸▸▸", "⚡⚡⚡", "Очень", "Работа"]
            } else {
                &["🇺🇸", "▸▸▸", "⚡⚡⚡"]
            };
            for needle in needles {
                assert!(seen.contains(needle), "{width}: {needle} not drawn");
            }
        }
    }

    fn chat_list_app() -> App {
        let mut app = app();
        app.lang = format::Lang::En;
        app.my_id = Some(1);
        app.show_info = false;
        app.focus = Pane::Chats;
        app.users.insert(3, "Bob Stone".to_owned());
        let now = crate::app::unix_now();
        let base = app.chats[&10].clone();
        let mut last = msg(100, 2, now, "new build is out");
        last.edited = false;
        app.chats.get_mut(&10).unwrap().last_message = Some(last);
        let mut add = |id: i64, title: &str, kind: ChatKind, m: Option<Msg>| {
            let mut c = base.clone();
            c.id = id;
            c.title = title.to_owned();
            c.kind = kind;
            c.unread = 0;
            c.mentions = 0;
            c.positions[0].order = 100 - id;
            c.positions[0].pinned = false;
            c.last_message = m;
            app.chats.insert(id, c);
        };
        add(
            11,
            "Bob Stone",
            ChatKind::Private(3),
            Some(msg(101, 1, now - 2 * 86_400, "see you tomorrow")),
        );
        let mut photo = msg(5, 9, 1_740_909_600, "Release notes");
        photo.chat_id = 12;
        add(12, "Rust News", ChatKind::Channel, Some(photo));
        add(13, "Carol", ChatKind::Private(4), None);
        add(14, "Dave", ChatKind::Private(5), None);
        let news = app.chats.get_mut(&12).unwrap();
        news.unread = 120;
        news.muted = true;
        app.drafts.insert(
            (13, None),
            model::Draft {
                text: "half-written reply".to_owned(),
                ..model::Draft::default()
            },
        );
        app.typing.insert(
            14,
            vec![crate::presence::Typing::new(
                Sender::User(5),
                crate::presence::Activity::Typing,
                None,
            )],
        );
        app
    }

    fn pane_text(rows: &[String], from: usize, to: usize) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let cells: Vec<char> = r.chars().collect();
                cells[from.min(cells.len())..to.min(cells.len())]
                    .iter()
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn rows_window_matches_list_window_and_keeps_two_line_rows_whole() {
        for len in 0..12 {
            for h in 0..8 {
                for sel in 0..len {
                    let ones = vec![1; len];
                    assert_eq!(
                        rows_window(&ones, sel, h),
                        list_window(sel, len, h),
                        "{len} {h} {sel}"
                    );
                }
            }
        }
        let mixed = [2, 2, 1, 1, 2, 2, 2, 1, 2, 2];
        for h in 2..12 {
            for sel in 0..mixed.len() {
                let start = rows_window(&mixed, sel, h);
                let used: usize = mixed[start..=sel].iter().sum();
                assert!(start <= sel && used <= h, "{h} {sel} {start}");
            }
        }
    }

    #[test]
    fn chat_rows_show_time_preview_sender_draft_and_typing_on_two_lines() {
        let mut app = chat_list_app();
        let rows = screen_rows(&mut app, 100, 30);
        let chats = pane_text(&rows, 19, 47);
        let i = chats.iter().position(|r| r.contains("Rust чат")).unwrap();
        let now = crate::app::unix_now();
        let clock = format::list_time(format::Lang::En, now, now);
        assert!(chats[i].ends_with(&clock), "{}", chats.join("\n"));
        assert!(
            chats[i + 1].contains("Alice: new build"),
            "{}",
            chats.join("\n")
        );
        assert!(chats[i + 1].ends_with("▴ @ 3"), "{}", chats.join("\n"));
        let row = |needle: &str| chats.iter().position(|r| r.contains(needle)).unwrap();
        let bob = row("Bob Stone");
        assert!(
            chats[bob + 1].contains("You: see you tomorrow"),
            "{}",
            chats.join("\n")
        );
        let weekday = format::list_time(
            format::Lang::En,
            crate::app::unix_now() - 2 * 86_400,
            crate::app::unix_now(),
        );
        assert!(chats[bob].ends_with(&weekday), "{}", chats.join("\n"));
        let news = row("Rust News");
        let old = format::list_time(format::Lang::En, 1_740_909_600, crate::app::unix_now());
        assert!(chats[news].ends_with(&old), "{}", chats.join("\n"));
        assert!(
            chats[news + 1].contains("Photo Release notes"),
            "{}",
            chats.join("\n")
        );
        assert!(!chats[news + 1].contains(':'), "no sender in channels");
        assert!(chats[news + 1].ends_with("120"), "{}", chats.join("\n"));
        assert!(chats[row("Carol") + 1].contains("Draft: half-written reply"));
        assert!(
            chats[row("Dave") + 1].contains("typing…"),
            "{}",
            chats.join("\n")
        );

        app.settings.appearance.compact_chat_list = true;
        let rows = screen_rows(&mut app, 100, 30);
        let chats = pane_text(&rows, 19, 47);
        let i = chats.iter().position(|r| r.contains("Rust чат")).unwrap();
        assert!(chats[i].ends_with("▴ @ 3"), "{}", chats.join("\n"));
        let row = |needle: &str| chats.iter().position(|r| r.contains(needle)).unwrap();
        assert_eq!(
            row("Rust News"),
            row("Bob Stone") + 1,
            "{}",
            chats.join("\n")
        );
        assert!(
            chats[row("Rust News")].ends_with("120"),
            "{}",
            chats.join("\n")
        );
        assert!(!chats.iter().any(|r| r.contains("new build")));
    }

    #[test]
    fn a_draft_left_in_the_composer_shows_in_the_chat_row_until_it_is_cleared() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = chat_list_app();
        let bob_preview = |app: &mut App| {
            let rows = screen_rows(app, 100, 30);
            let chats = pane_text(&rows, 19, 47);
            let bob = chats.iter().position(|r| r.contains("Bob Stone")).unwrap();
            chats[bob + 1].clone()
        };
        assert!(bob_preview(&mut app).contains("You: see you tomorrow"));
        app.open(11);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        for c in "at five".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.composer.input.text(), "at five");
        app.open(12);
        assert!(app.composer.input.text().is_empty());
        let preview = bob_preview(&mut app);
        assert!(preview.contains("Draft: at five"), "{preview}");

        app.open(11);
        assert_eq!(app.composer.input.text(), "at five");
        app.composer.input = crate::input::TextInput::with_text("");
        app.open(12);
        let preview = bob_preview(&mut app);
        assert!(preview.contains("You: see you tomorrow"), "{preview}");
        assert!(!preview.contains("Draft"), "{preview}");
    }

    #[test]
    fn two_line_rows_keep_selection_scrolling_and_title_selection_in_step() {
        let mut app = chat_list_app();
        let base = app.chats[&11].clone();
        for id in 20..40 {
            let mut c = base.clone();
            c.id = id;
            c.title = format!("Chat number {id}");
            c.positions[0].order = 100 - id;
            app.chats.insert(id, c);
        }
        app.chat_sel = 18;
        let rows = screen_rows(&mut app, 100, 24);
        let sel = rows
            .iter()
            .position(|r| r.contains("❯ Chat number 34"))
            .unwrap();
        assert!(sel > 2 && sel < 21, "{}", rows.join("\n"));
        assert!(rows[sel + 1].contains("You: see you tomorrow"));
        assert!(app.chat_rows.get() >= 9, "page moves by visible chats");
        let below = ratatui::layout::Position::new(25, sel as u16 + 1);
        let hit = app.text_map.hit_at(below);
        assert_eq!(
            hit,
            Some(Hit::Chat(Row::Chat(34))),
            "both lines are the same chat"
        );
        for n in 0..app.chat_rows().len() {
            app.chat_sel = n;
            let rows = screen_rows(&mut app, 100, 24);
            assert!(
                rows.iter().any(|r| r.contains('❯')),
                "selection {n} on screen"
            );
        }
    }

    #[test]
    fn narrow_terminals_hide_the_info_pane_but_keep_chat_titles() {
        let mut app = chat_list_app();
        app.show_info = true;
        app.focus = Pane::Messages;
        for (w, h) in [(80u16, 24u16), (70, 24), (60, 24), (100, 30)] {
            let rows = screen_rows(&mut app, w, h);
            let screen = rows.join("\n");
            for title in ["Rust чат", "Bob Stone", "Rust News"] {
                assert!(screen.contains(title), "{w}x{h}: {title}\n{screen}");
            }
            assert!(
                !screen.contains("Chat info"),
                "{w}x{h}: info auto-hidden\n{screen}"
            );
        }
        assert!(app.show_info, "the setting itself is untouched");
        let rows = screen_rows(&mut app, 120, 30);
        assert!(rows.join("\n").contains("Chat info"), "fits at 120 columns");
        app.focus = Pane::Info;
        let rows = screen_rows(&mut app, 80, 24);
        let screen = rows.join("\n");
        assert!(
            screen.contains("Chat info"),
            "focusing it shows it\n{screen}"
        );
        assert!(
            screen.contains("Rust чат") && screen.contains("Bob Sto"),
            "{screen}"
        );
        let l = theme::Theme::default().layout;
        let mut app = chat_list_app();
        app.show_info = true;
        assert_eq!(pane_widths(80, &app, &l), [18, 30, 0]);
        assert_eq!(pane_widths(100, &app, &l), [18, 30, 0]);
        assert_eq!(pane_widths(104, &app, &l), [18, 30, 26]);
        assert_eq!(pane_widths(140, &app, &l), [18, 30, 30]);
        assert_eq!(pane_widths(70, &app, &l), [16, 24, 0]);
        assert_eq!(pane_widths(60, &app, &l), [0, 30, 0]);
        assert_eq!(pane_widths(40, &app, &l), [0, 24, 0]);
        app.focus = Pane::Info;
        assert_eq!(pane_widths(80, &app, &l), [0, 26, 24]);
        app.focus = Pane::Folders;
        assert_eq!(pane_widths(60, &app, &l), [16, 24, 0]);

        let mut app = chat_list_app();
        let mut t = classic_rounded();
        t.layout.chats_width = theme::PaneWidth::Cols(14);
        app.set_theme(t);
        let rows = screen_rows(&mut app, 100, 30);
        let chats = pane_text(&rows, 19, 31);
        assert!(
            chats.iter().any(|r| r == "  Rust чат"),
            "{}",
            chats.join("\n")
        );
        assert!(
            chats.iter().any(|r| r == "❯ Bob Stone"),
            "{}",
            chats.join("\n")
        );
    }

    #[test]
    fn empty_chat_lists_say_why_instead_of_loading_forever() {
        let mut app = chat_list_app();
        app.filter = Some(crate::app::Filter {
            pane: Pane::Chats,
            input: crate::input::TextInput::with_text("zzz"),
            active: false,
        });
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("No chats match ‘zzz’"), "{screen}");
        assert!(!screen.contains("Loading chats"));
        app.filter = None;
        app.folders = vec![(7, "Work".to_owned())];
        app.folder_sel = 1;
        assert_eq!(app.current_list(), ListId::Folder(7));
        app.refresh_lists();
        assert!(render(&mut app, 100, 30).contains("Loading chats…"));
        app.handle(Event::ChatListLoaded(ListId::Folder(7)));
        assert!(render(&mut app, 100, 30).contains("No chats in this folder"));
        app.folder_sel = 2;
        app.refresh_lists();
        app.handle(Event::ChatListLoaded(ListId::Archive));
        assert!(render(&mut app, 100, 30).contains("Archive is empty"));
        app.filter = Some(crate::app::Filter {
            pane: Pane::Folders,
            input: crate::input::TextInput::with_text("qqq"),
            active: false,
        });
        let screen = render(&mut app, 100, 30);
        assert!(
            screen.contains("No folders") && screen.contains("‘qqq’"),
            "{screen}"
        );
    }

    #[test]
    fn failed_history_shows_the_reason_and_enter_retries() {
        let mut app = app();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (td, log) = crate::td::Td::recording(tx);
        app.td = td;
        app.show_info = false;
        app.histories.clear();
        app.open_chat = None;
        app.open(10);
        app.composer.active = true;
        app.handle(Event::HistoryFailed {
            chat_id: 10,
            epoch: 0,
            error: "network is unreachable".to_owned(),
        });
        assert!(
            !app.composer.active,
            "an empty composer steps aside for the retry key"
        );
        let screen = render(&mut app, 120, 30);
        assert!(
            screen.contains("Couldn't load messages: network is unreachable"),
            "{screen}"
        );
        assert!(screen.contains("Enter retry"), "{screen}");
        assert!(!screen.contains("No messages yet"));
        let loads = || {
            log.lock()
                .unwrap()
                .iter()
                .filter(|e| *e == "load history")
                .count()
        };
        let before = loads();
        app.focus = Pane::Messages;
        press(
            &mut app,
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        assert_eq!(loads(), before + 1);
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("Loading…"), "{screen}");
        app.handle(Event::History {
            chat_id: 10,
            msgs: vec![msg(1, 2, 1_700_000_000, "finally here")],
            initial: true,
            epoch: 0,
        });
        assert!(app.histories[&10].error.is_none());
        assert!(render(&mut app, 120, 30).contains("finally here"));
    }

    #[test]
    fn forum_topics_render_under_the_chat_with_marks_and_previews() {
        let mut app = app();
        app.chats.get_mut(&10).unwrap().forum = true;
        app.expanded = Some(10);
        let topic = |id: i32, name: &str, order: i64| crate::model::Topic {
            id,
            name: name.to_owned(),
            order,
            use_default_mute: true,
            ..crate::model::Topic::default()
        };
        let mut general = topic(1, "General", 5);
        general.general = true;
        general.hidden = true;
        let mut releases = topic(2, "Releases", 50);
        releases.icon = "🚀".to_owned();
        releases.unread = 4;
        releases.mentions = 1;
        releases.pinned = true;
        let mut m = msg(90, 2, 1_700_000_000, "new build is out");
        m.topic = Some(2);
        releases.last_message = Some(m);
        let mut archive = topic(3, "Old stuff", 40);
        archive.closed = true;
        archive.color = 0x6FB9F0;
        let mut list = crate::model::TopicList::default();
        list.merge_page(vec![general, releases, archive], None, 3);
        app.topics.insert(10, list);
        app.open_topic = Some(2);
        app.histories.get_mut(&10).unwrap().topic = Some(2);
        app.refresh_lists();
        let rows = screen_rows(&mut app, 140, 30);
        let chat = rows.iter().position(|r| r.contains("▾ Rust чат")).unwrap();
        let rel = rows.iter().position(|r| r.contains("🚀")).unwrap();
        let old = rows.iter().position(|r| r.contains("# Old stuff")).unwrap();
        let general_row = rows.iter().position(|r| r.contains("(hidden)")).unwrap();
        assert!(
            chat < rel && rel < old && old < general_row,
            "{}",
            rows.join("\n")
        );
        let r = row_with(&rows, "🚀");
        assert!(r.contains("🚀"), "{r}");
        assert!(r.contains("Alice"), "{r}");
        assert!(r.contains("@ 4"), "{r}");
        assert!(r.contains("▴"), "pinned topic: {r}");
        assert!(row_with(&rows, "Old stuff").contains("# Old stuff"));
        assert!(row_with(&rows, "Old stuff").contains("⊘"));
        assert!(row_with(&rows, "General").contains("(hidden)"));
        assert!(row_with(&rows, "Rust чат › Releases").contains("Releases"));

        app.expanded = None;
        let rows = screen_rows(&mut app, 140, 30);
        assert!(!rows.iter().any(|r| r.contains("Old stuff")));
        assert!(rows.iter().any(|r| r.contains("▸ Rust чат")));
    }

    #[test]
    fn secret_chat_state_timer_and_popups_render() {
        let mut app = app();
        app.chats.insert(
            50,
            ChatInfo {
                id: 50,
                title: "Bob".to_owned(),
                kind: ChatKind::Secret(2),
                unread: 0,
                mentions: 0,
                reactions: 0,
                last_read_inbox: 0,
                last_read_outbox: 0,
                photo: None,
                muted: false,
                use_default_mute: false,
                positions: vec![Position {
                    list: ListId::Main,
                    order: 9,
                    pinned: false,
                }],
                last_message: None,
                group_id: 0,
                secret_id: 7,
                forum: false,
                auto_delete: 30,
                perms: crate::model::Rights::default(),
                protected: false,
                reactions_off: false,
            },
        );
        app.secret_chats.insert(
            7,
            crate::model::SecretChat {
                id: 7,
                user_id: 2,
                state: crate::model::SecretState::Pending,
                outbound: true,
            },
        );
        let mut h = History::default();
        let mut m = msg(1, 2, 1_700_000_000, "burn after reading");
        m.chat_id = 50;
        m.ttl = 30;
        h.merge_older(vec![m]);
        app.histories.insert(50, h);
        app.open_chat = Some(50);
        app.refresh_lists();
        let rows = screen_rows(&mut app, 140, 30);
        assert!(row_with(&rows, "Bob (waiting)").contains("🔒"));
        let header = row_with(&rows, "secret chat: waiting");
        assert!(header.contains("⏱ 30s"), "{header}");
        assert!(
            rows.iter()
                .any(|r| r.contains("⏱ 30s") && !r.contains("secret chat"))
        );
        assert!(rows.iter().any(|r| r.contains("only on this device")));
        assert!(rows.iter().any(|r| r.contains("Self-destruct: 30s")));

        app.secret_chats.get_mut(&7).unwrap().state = crate::model::SecretState::Closed;
        let rows = screen_rows(&mut app, 140, 30);
        assert!(rows.iter().any(|r| r.contains("Bob (closed)")));

        app.popup = Some(Popup::Ttl {
            chat_id: 50,
            choices: crate::model::ttl_choices(true),
            sel: 2,
        });
        let rows = screen_rows(&mut app, 140, 30);
        assert!(rows.iter().any(|r| r.contains("Self-destruct timer")));
        assert!(rows.iter().any(|r| r.contains("30s ✓")));
        assert!(rows.iter().any(|r| r.contains("1w")));
        app.popup = Some(Popup::ConfirmSecret {
            chat_id: 50,
            secret_id: 7,
            delete: true,
        });
        let rows = screen_rows(&mut app, 140, 30);
        assert!(rows.iter().any(|r| r.contains("Delete this secret chat")));
        app.popup = Some(Popup::NewTopic {
            chat_id: 10,
            input: crate::input::TextInput::with_text("Ideas"),
        });
        let rows = screen_rows(&mut app, 140, 30);
        assert!(row_with(&rows, "New topic").contains("New topic"));
        assert!(rows.iter().any(|r| r.contains("> Ideas")));
        for w in [40u16, 60, 90] {
            screen_rows(&mut app, w, 12);
        }
    }

    mod mouse_selection {
        use super::*;
        use crossterm::event::{
            Event as TermEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        use ratatui::layout::Position;
        use ratatui::style::Modifier;

        const LONG: &str = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
        const W: u16 = 90;
        const H: u16 = 30;

        fn scene(layout: MessageLayout) -> (App, crate::td::RequestLog) {
            let mut app = app();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let (td, log) = crate::td::Td::recording(tx);
            app.td = td;
            app.settings.appearance.layout = Some(layout);
            app.show_info = false;
            let plain = |id: i64, sender: i64, date: i64, text: &str| {
                let mut m = msg(id, sender, date, text);
                m.body = RichText::plain(text);
                m.edited = false;
                m.reply_to = None;
                m.media = None;
                m
            };
            let mut h = History::default();
            h.reached_start = true;
            h.merge_older(vec![
                plain(1, 2, 1_700_000_000, LONG),
                plain(2, 1, 1_700_000_100, "second message"),
            ]);
            app.histories.insert(10, h);
            (app, log)
        }

        fn find(buf: &Buffer, needle: &str) -> Position {
            let chars: Vec<String> = needle.chars().map(String::from).collect();
            let (w, h) = (buf.area.width, buf.area.height);
            (0..h)
                .flat_map(|y| (0..w).map(move |x| Position::new(x, y)))
                .find(|p| {
                    chars.iter().enumerate().all(|(i, c)| {
                        let x = p.x + i as u16;
                        x < w && buf[(x, p.y)].symbol() == c
                    })
                })
                .unwrap_or_else(|| panic!("{needle} not on screen"))
        }

        fn end_of(buf: &Buffer, needle: &str) -> Position {
            let p = find(buf, needle);
            Position::new(p.x + needle.chars().count() as u16 - 1, p.y)
        }

        fn mouse(app: &mut App, kind: MouseEventKind, at: Position, modifiers: KeyModifiers) {
            app.handle(Event::Input(TermEvent::Mouse(MouseEvent {
                kind,
                column: at.x,
                row: at.y,
                modifiers,
            })));
        }

        fn drag(app: &mut App, from: Position, to: Position) {
            let left = MouseButton::Left;
            mouse(app, MouseEventKind::Down(left), from, KeyModifiers::NONE);
            mouse(app, MouseEventKind::Drag(left), to, KeyModifiers::NONE);
            mouse(app, MouseEventKind::Up(left), to, KeyModifiers::NONE);
        }

        fn click(app: &mut App, at: Position) {
            drag(app, at, at);
        }

        fn copies(log: &crate::td::RequestLog) -> Vec<String> {
            log.lock()
                .unwrap()
                .iter()
                .filter_map(|e| e.strip_prefix("copy "))
                .map(str::to_owned)
                .collect()
        }

        #[test]
        fn dragging_copies_message_text_without_chrome_in_every_layout() {
            for (layout, sel) in MessageLayout::ALL
                .into_iter()
                .flat_map(|l| [None, Some(0), Some(1)].map(|s| (l, s)))
            {
                let (mut app, log) = scene(layout);
                app.msg_sel = sel;
                let buf = buffer(&mut app, W, H);
                let from = find(&buf, "alpha");
                let to = end_of(&buf, "second message");
                let (left, none) = (MouseButton::Left, KeyModifiers::NONE);
                mouse(&mut app, MouseEventKind::Down(left), from, none);
                mouse(&mut app, MouseEventKind::Drag(left), to, none);
                let lit = buffer(&mut app, W, H);
                let reversed = |p: Position| lit[p].modifier.contains(Modifier::REVERSED);
                assert!(reversed(from) && reversed(to), "{layout:?}");
                assert!(!reversed(Position::new(from.x - 1, from.y)), "{layout:?}");
                mouse(&mut app, MouseEventKind::Up(left), to, none);
                assert_eq!(
                    copies(&log),
                    vec![format!("{LONG}\nsecond message")],
                    "{layout:?}"
                );
                assert!(app.selection.is_none());
                let after = buffer(&mut app, W, H);
                assert!(!after[from].modifier.contains(Modifier::REVERSED));
            }
        }

        #[test]
        fn dragging_past_the_pane_is_clipped_to_the_pane_it_started_in() {
            let (mut app, log) = scene(MessageLayout::Bubbles);
            let buf = buffer(&mut app, W, H);
            drag(&mut app, find(&buf, "beta"), Position::new(0, H - 1));
            let rest = LONG.strip_prefix("alpha ").unwrap();
            assert_eq!(copies(&log), vec![format!("{rest}\nsecond message")]);
            let last = end_of(&buf, "second message");
            drag(&mut app, last, Position::new(W - 1, 0));
            assert_eq!(copies(&log)[1..], [format!("{LONG}\nsecond message")]);
        }

        #[test]
        fn chat_titles_and_popups_are_selectable_too() {
            let (mut app, log) = scene(MessageLayout::Classic);
            let buf = buffer(&mut app, W, H);
            let title = find(&buf, "❯ Rust чат");
            let border = Position::new(title.x - 1, title.y);
            drag(&mut app, border, Position::new(W - 1, title.y));
            app.popup = Some(Popup::ConfirmLogout);
            let buf = buffer(&mut app, W, H);
            let question = find(&buf, "Log out of Telegram");
            let cancel = end_of(&buf, "cancel");
            drag(&mut app, question, cancel);
            assert_eq!(
                copies(&log),
                vec![
                    "Rust чат".to_owned(),
                    "Log out of Telegram?\n\ny log out   n cancel".to_owned()
                ]
            );
        }

        #[test]
        fn shift_drag_and_plain_clicks_copy_nothing() {
            let (mut app, log) = scene(MessageLayout::Classic);
            let buf = buffer(&mut app, W, H);
            let from = find(&buf, "alpha");
            let to = end_of(&buf, "second message");
            let shift = KeyModifiers::SHIFT;
            let left = MouseButton::Left;
            mouse(&mut app, MouseEventKind::Down(left), from, shift);
            mouse(&mut app, MouseEventKind::Drag(left), to, shift);
            assert!(app.selection.is_none());
            mouse(&mut app, MouseEventKind::Up(left), to, shift);
            click(&mut app, from);
            assert!(copies(&log).is_empty());
        }

        #[test]
        fn clicks_focus_and_select_what_is_under_the_cursor() {
            let (mut app, log) = scene(MessageLayout::Bubbles);
            let mut other = app.chats[&10].clone();
            other.id = 20;
            other.title = "Second chat".to_owned();
            other.positions = vec![crate::model::Position {
                list: ListId::Main,
                order: 4,
                pinned: false,
            }];
            app.chats.insert(20, other);
            app.focus = Pane::Chats;
            let buf = buffer(&mut app, W, H);
            click(&mut app, find(&buf, "second message"));
            assert_eq!((app.focus, app.msg_sel), (Pane::Messages, Some(1)));
            click(&mut app, find(&buf, "gamma"));
            assert_eq!(app.msg_sel, Some(0));
            let folders = find(&buf, "Folders");
            click(&mut app, Position::new(folders.x, folders.y + 1));
            assert_eq!(app.focus, Pane::Folders);
            click(&mut app, find(&buf, "Second chat"));
            assert_eq!((app.open_chat, app.chat_sel), (Some(20), 1));
            assert!(copies(&log).is_empty());
        }

        #[test]
        fn wheel_scrolls_the_pane_under_the_cursor() {
            let mut app = app();
            app.msg_sel = None;
            app.focus = Pane::Chats;
            let buf = buffer(&mut app, 120, 40);
            let text = find(&buf, "hello there");
            let none = KeyModifiers::NONE;
            mouse(&mut app, MouseEventKind::ScrollUp, text, none);
            assert_eq!((app.bottom_offset, app.focus), (3, Pane::Chats));
            buffer(&mut app, 120, 40);
            mouse(&mut app, MouseEventKind::ScrollDown, text, none);
            assert_eq!(app.bottom_offset, 0);
            app.msg_sel = Some(39);
            mouse(&mut app, MouseEventKind::ScrollUp, text, none);
            assert_eq!(app.msg_sel, Some(38));
            app.popup = Some(Popup::ConfirmLogout);
            buffer(&mut app, 120, 40);
            mouse(&mut app, MouseEventKind::ScrollUp, text, none);
            assert_eq!(app.msg_sel, Some(38));
        }
    }
}

#[cfg(test)]
mod repaint_tests {
    use super::*;
    use crate::config::ApiCredentials;
    use crate::keymap::Keymap;
    use crate::model::{FileRef, History, RichText};
    use crate::td::Event;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tokio::sync::mpsc::UnboundedReceiver;
    use unicode_segmentation::UnicodeSegmentation;

    const PLACEHOLDER: char = '\u{10EEEE}';

    #[derive(Clone, Default)]
    pub(super) struct Sink(pub(super) Rc<RefCell<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn kitty_cells(g: &str) -> usize {
        let first = g.chars().next().unwrap_or(' ');
        let wide_base = ('\u{1F1E6}'..='\u{1F1FF}').contains(&first)
            || [
                '\u{261D}',
                '\u{26F9}',
                '\u{270C}',
                '\u{270D}',
                '\u{1F3CB}',
                '\u{1F3CC}',
                '\u{1F574}',
                '\u{1F575}',
                '\u{1F590}',
            ]
            .contains(&first);
        match UnicodeWidthChar::width(first).unwrap_or(0) {
            0 => 0,
            _ if wide_base || g.contains('\u{FE0F}') => 2,
            w => w,
        }
    }

    #[test]
    fn kitty_model_widths_match_kitty() {
        for (g, cells) in [
            ("❤\u{FE0F}\u{200D}🔥", 2),
            ("❤\u{200D}🔥", 1),
            ("❤\u{FE0F}", 2),
            ("❤", 1),
            ("👍🏽", 2),
            ("🤷\u{200D}♂\u{FE0F}", 2),
            ("🤷\u{200D}♂", 2),
            ("👨\u{200D}👩\u{200D}👧", 2),
            ("🏳\u{FE0F}\u{200D}🌈", 2),
            ("🇺🇸", 2),
            ("🇺", 2),
            ("😀", 2),
            ("✍", 2),
            ("中", 2),
            ("1\u{FE0F}\u{20E3}", 2),
            ("1\u{20E3}", 1),
            ("e\u{301}", 1),
            ("✓", 1),
            ("▎", 1),
            ("─", 1),
        ] {
            assert_eq!(kitty_cells(g), cells, "{g:?}");
            let mut screen = Screen::new(8, 1);
            screen.feed(format!("{g}x").as_bytes());
            assert_eq!(screen.cells[cells], "x", "{g:?}");
        }
        let mut screen = Screen::new(8, 1);
        screen.feed("abcdefgh\x1b[1;1H❤\u{FE0F}\u{200D}🔥 3".as_bytes());
        assert_eq!(screen.cells[..5].concat(), "❤\u{FE0F}\u{200D}🔥 3e");
        screen.feed(b"\x1b[1;2Hz");
        assert_eq!(screen.cells[..4].concat(), " z 3");
        screen.feed("\x1b[1;4H中\x1b[1;3H🔥".as_bytes());
        assert_eq!(screen.cells[2..6].concat(), "🔥 f");
    }

    #[derive(Clone)]
    pub(super) struct Screen {
        w: usize,
        h: usize,
        cells: Vec<String>,
        x: usize,
        y: usize,
        saved: (usize, usize),
    }

    impl Screen {
        pub(super) fn new(w: u16, h: u16) -> Self {
            let (w, h) = (usize::from(w), usize::from(h));
            Self {
                w,
                h,
                cells: vec![" ".to_owned(); w * h],
                x: 0,
                y: 0,
                saved: (0, 0),
            }
        }

        fn csi(&mut self, params: &str, fin: char) {
            let n: Vec<usize> = params
                .trim_start_matches('?')
                .split(';')
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            let a = n[0].max(1);
            match fin {
                'H' => {
                    self.y = (a - 1).min(self.h - 1);
                    self.x = (n.get(1).copied().unwrap_or(1).max(1) - 1).min(self.w - 1);
                }
                's' => self.saved = (self.x, self.y),
                'u' => (self.x, self.y) = self.saved,
                'A' => self.y = self.y.saturating_sub(a),
                'B' => self.y = (self.y + a).min(self.h - 1),
                'C' => self.x = (self.x + a).min(self.w - 1),
                'D' => self.x = self.x.saturating_sub(a),
                'J' if n[0] == 2 => self.cells.fill(" ".to_owned()),
                _ => {}
            }
        }

        pub(super) fn feed(&mut self, bytes: &[u8]) {
            let text = String::from_utf8_lossy(bytes);
            let mut it = text.chars().peekable();
            let mut run = String::new();
            while let Some(c) = it.next() {
                if c == '\x1b' {
                    self.print(&std::mem::take(&mut run));
                    match it.next() {
                        Some('[') => {
                            let mut params = String::new();
                            for d in it.by_ref() {
                                if ('@'..='~').contains(&d) {
                                    self.csi(&params, d);
                                    break;
                                }
                                params.push(d);
                            }
                        }
                        Some('_') => {
                            while let Some(d) = it.next() {
                                if d == '\x1b' && it.peek() == Some(&'\\') {
                                    it.next();
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                run.push(c);
            }
            self.print(&run);
        }

        fn print(&mut self, text: &str) {
            for g in text.graphemes(true) {
                let w = kitty_cells(g);
                if w == 0 {
                    let row = self.y * self.w;
                    let mut lead = self.x.min(self.w);
                    while lead > 0 {
                        lead -= 1;
                        if !self.cells[row + lead].is_empty() {
                            self.cells[row + lead].push_str(g);
                            break;
                        }
                    }
                    continue;
                }
                if self.x + w > self.w {
                    self.x = 0;
                    self.y = (self.y + 1).min(self.h - 1);
                }
                let row = self.y * self.w;
                for x in self.x..self.x + w {
                    self.break_wide(row, x);
                }
                self.cells[row + self.x] = g.to_owned();
                for x in self.x + 1..self.x + w {
                    self.cells[row + x].clear();
                }
                self.x += w;
            }
        }

        fn break_wide(&mut self, row: usize, x: usize) {
            let mut lead = x;
            while lead > 0 && self.cells[row + lead].is_empty() {
                lead -= 1;
            }
            let mut end = lead + 1;
            while end < self.w && self.cells[row + end].is_empty() {
                end += 1;
            }
            if end - lead > 1 {
                self.cells[row + lead..row + end].fill(" ".to_owned());
            }
        }

        fn soil_blanks(&mut self) {
            for (i, c) in self.cells.iter_mut().enumerate() {
                if c == " " {
                    *c = char::from(b'a' + (i % 26) as u8).to_string();
                }
            }
        }

        pub(super) fn stale_cells(&self, buf: &Buffer) -> Vec<(u16, u16, String)> {
            let mut out = Vec::new();
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    let want = &buf[(x, y)];
                    let got = &self.cells[usize::from(y) * self.w + usize::from(x)];
                    let image = !matches!(want.diff_option, CellDiffOption::None);
                    let ok = if image {
                        got.starts_with(PLACEHOLDER)
                    } else {
                        got == want.symbol()
                            || (got.trim().is_empty() && want.symbol().trim().is_empty())
                    };
                    if !ok {
                        out.push((x, y, got.clone()));
                    }
                }
            }
            out
        }
    }

    struct Rig {
        app: App,
        rx: UnboundedReceiver<Event>,
        term: Terminal<Resync<CrosstermBackend<Sink>>>,
        sink: Sink,
        screen: Screen,
        layout: u64,
        buf: Buffer,
    }

    impl Rig {
        fn new(w: u16, h: u16) -> Self {
            Self::with_protocol(w, h, ratatui_image::picker::ProtocolType::Kitty)
        }

        fn with_protocol(w: u16, h: u16, protocol: ratatui_image::picker::ProtocolType) -> Self {
            let dir = std::env::temp_dir().join(format!("gramtty-repaint-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let png = dir.join(format!("photo{n}.png"));
            image::RgbaImage::from_pixel(320, 240, image::Rgba([200, 30, 30, 255]))
                .save(&png)
                .unwrap();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            #[allow(deprecated)]
            let mut picker =
                ratatui_image::picker::Picker::from_fontsize(ratatui_image::FontSize {
                    width: 10,
                    height: 20,
                });
            picker.set_protocol_type(protocol);
            let mut app = App::new(
                crate::td::Td::recording(tx).0,
                ApiCredentials {
                    api_id: 1,
                    api_hash: "x".to_owned(),
                },
                std::env::temp_dir(),
                Keymap::default(),
                picker,
            );
            app.auth = Auth::Ready;
            let mut history = History::default();
            history.reached_start = true;
            for i in 0..40i64 {
                let mut m = Msg {
                    id: i + 1,
                    chat_id: 10,
                    sender: Sender::User(2 + i % 2),
                    date: 1_700_000_000 + i * 400,
                    edited: false,
                    outgoing: false,
                    state: SendState::Sent,
                    reply_to: None,
                    album: 0,
                    media: None,
                    attachments: Vec::new(),
                    body: RichText {
                        text: if i % 2 == 0 {
                            "о".to_owned()
                        } else {
                            format!(
                                "{i} используй этот проект https://github.com/example/project/{i}"
                            )
                        },
                        entities: Vec::new(),
                    },
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
                };
                if i % 4 == 1 {
                    let f = FileRef {
                        id: 500 + i as i32,
                        size: 1,
                        path: png.to_string_lossy().into_owned(),
                        done: true,
                        downloaded: 1,
                        ..FileRef::default()
                    };
                    let visual = Visual {
                        file: f.clone(),
                        decode: Decode::Raster,
                        width: 320,
                        height: 240,
                    };
                    let mut a = Attachment::new(MediaKind::Photo, f);
                    a.width = 320;
                    a.height = 240;
                    a.preview = Some(visual.clone());
                    a.full = Some(visual);
                    m.attachments = vec![a];
                    m.body = RichText::default();
                }
                history.upsert(m);
            }
            app.histories.insert(10, history);
            app.open_chat = Some(10);
            app.focus = Pane::Messages;
            let sink = Sink::default();
            let area = Rect::new(0, 0, w, h);
            let term = Terminal::with_options(
                Resync(CrosstermBackend::new(sink.clone())),
                ratatui::TerminalOptions {
                    viewport: ratatui::Viewport::Fixed(area),
                },
            )
            .unwrap();
            Self {
                app,
                rx,
                term,
                sink,
                screen: Screen::new(w, h),
                layout: image_layout(&Buffer::empty(area)),
                buf: Buffer::empty(area),
            }
        }

        fn frame(&mut self) -> Vec<u8> {
            let mut buf = None;
            draw_frame(&mut self.term, &mut self.app, &mut self.layout, |b| {
                buf = Some(b.clone())
            })
            .unwrap();
            self.buf = buf.unwrap();
            self.app.after_draw();
            let out = std::mem::take(&mut *self.sink.0.borrow_mut());
            self.screen.feed(&out);
            out
        }

        async fn settle(&mut self) {
            for _ in 0..40 {
                self.frame();
                match tokio::time::timeout(Duration::from_millis(200), self.rx.recv()).await {
                    Ok(Some(ev)) => self.app.handle(ev),
                    _ => return,
                }
                while let Ok(ev) = self.rx.try_recv() {
                    self.app.handle(ev);
                }
            }
        }

        fn images_on_screen(&self) -> usize {
            self.buf
                .content
                .iter()
                .filter(|c| matches!(c.diff_option, CellDiffOption::ForcedWidth(_)))
                .count()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forced_clear_repaints_inside_one_synchronized_update_without_terminal_queries() {
        let mut rig = Rig::new(100, 30);
        rig.settle().await;
        rig.app.force_clear = true;
        let t = std::time::Instant::now();
        let out = rig.frame();
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        assert!(!rig.app.force_clear);
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("\x1b[?2026h"), "{text:?}");
        assert!(text.ends_with("\x1b[?2026l"));
        assert!(text.contains("\x1b[2J"));
        assert!(!text.contains("\x1b[6n"));
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scrolling_images_repaints_cells_the_terminal_still_shows() {
        let mut rig = Rig::new(100, 30);
        rig.app.msg_sel = Some(39);
        rig.settle().await;
        assert!(rig.images_on_screen() > 0);
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
        for sel in (0..39).rev() {
            rig.app.msg_sel = Some(sel);
            rig.settle().await;
        }
        rig.app.msg_sel = Some(39);
        rig.settle().await;
        let mut moves = 0;
        for sel in (0..39).rev() {
            let clean = rig.screen.clone();
            rig.screen.soil_blanks();
            let before = rig.layout;
            rig.app.msg_sel = Some(sel);
            let out = rig.frame();
            if rig.layout == before {
                rig.screen = clean;
                rig.screen.feed(&out);
                continue;
            }
            moves += 1;
            let stale = rig.screen.stale_cells(&rig.buf);
            assert!(
                stale.is_empty(),
                "sel {sel}: {} stale cells, e.g. {:?}",
                stale.len(),
                &stale[..stale.len().min(5)]
            );
        }
        assert!(moves > 5, "{moves}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn chat_info_avatar_is_a_kitty_image_that_leaves_no_stale_cells() {
        let mut rig = Rig::new(120, 40);
        let photo = rig.app.histories[&10]
            .msgs()
            .iter()
            .find_map(|m| m.attachments.first())
            .map(|a| FileRef {
                id: 900,
                ..a.file.clone()
            })
            .unwrap();
        rig.app.chats.insert(
            10,
            crate::model::ChatInfo {
                id: 10,
                title: "Alice Longname".to_owned(),
                kind: ChatKind::Private(2),
                unread: 0,
                mentions: 0,
                reactions: 0,
                last_read_inbox: 0,
                last_read_outbox: 0,
                photo: Some(photo),
                muted: false,
                use_default_mute: false,
                positions: Vec::new(),
                last_message: None,
                group_id: 0,
                secret_id: 0,
                forum: false,
                auto_delete: 0,
                perms: crate::model::Rights::default(),
                protected: false,
                reactions_off: false,
            },
        );
        rig.app.show_info = true;
        rig.settle().await;
        let x0 = 120 - 30 + 1;
        let placed: Vec<(u16, u16)> = (0..40u16)
            .flat_map(|y| (x0..119).map(move |x| (x, y)))
            .filter(|&(x, y)| matches!(rig.buf[(x, y)].diff_option, CellDiffOption::ForcedWidth(_)))
            .collect();
        let avatar: Vec<(u16, u16)> = (3..11u16)
            .flat_map(|y| (x0 + 6..x0 + 22).map(move |x| (x, y)))
            .collect();
        assert_eq!(placed, avatar, "{}", buffer_text(&rig.buf));
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
        let text = buffer_text(&rig.buf);
        assert!(text.contains("Alice Longname"), "{text}");
    }

    #[cfg(target_os = "linux")]
    fn kitty_reads_shm(out: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(out);
        let mut cmds = Vec::new();
        for part in text.split("\x1b_G").skip(1) {
            let Some(end) = part.find("\x1b\\") else {
                continue;
            };
            let cmd = &part[..end];
            if cmd.contains("t=s") {
                let (keys, payload) = cmd.split_once(';').unwrap();
                let name = String::from_utf8(
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
                        .unwrap(),
                )
                .unwrap();
                let path = format!("/dev/shm/{}", name.trim_start_matches('/'));
                let Ok(meta) = std::fs::metadata(&path) else {
                    cmds.push(format!("missing {keys}"));
                    continue;
                };
                let len = meta.len();
                let size: u64 = keys
                    .split(',')
                    .find_map(|k| k.strip_prefix("S="))
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(len, size, "{keys}");
                rustix::shm::unlink(name.as_str()).unwrap();
            }
            cmds.push(cmd.split(';').next().unwrap().to_owned());
        }
        cmds
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn viewer_plays_video_inline_through_shared_memory_frames() {
        if crate::video::probe().is_err() {
            eprintln!("libmpv is not installed; skipping");
            return;
        }
        let Some(path) = crate::video::tests::test_video() else {
            return;
        };
        let mut rig = Rig::new(120, 45);
        rig.app.inline.enable();
        rig.app.inline.audio_out = Some("null".to_owned());
        let f = FileRef {
            id: 900,
            size: std::fs::metadata(&path).unwrap().len() as i64,
            path: path.clone(),
            done: true,
            downloaded: 1,
            ..FileRef::default()
        };
        let mut a = Attachment::new(MediaKind::Video, f);
        a.width = 640;
        a.height = 360;
        a.duration = 2;
        let mut m = rig.app.histories[&10].get(39).unwrap().clone();
        m.id = 41;
        m.attachments = vec![a];
        m.body = model::RichText::default();
        rig.app.histories.get_mut(&10).unwrap().upsert(m);
        rig.app.msg_sel = None;
        rig.settle().await;
        rig.app.popup = Some(Popup::Viewer {
            chat_id: 10,
            items: vec![(41, 0), (38, 0)],
            pos: 0,
        });
        let mut cmds = Vec::new();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < until
            && cmds
                .iter()
                .filter(|c: &&String| c.starts_with("a=f,r=1"))
                .count()
                < 5
        {
            let out = rig.frame();
            cmds.extend(kitty_reads_shm(&out));
            let _ = tokio::time::timeout(Duration::from_millis(50), rig.rx.recv()).await;
            while let Ok(ev) = rig.rx.try_recv() {
                rig.app.handle(ev);
            }
        }
        let first = cmds
            .iter()
            .find(|c| c.starts_with("a=T,U=1"))
            .expect("placement");
        assert!(first.contains("t=s") && first.contains("f=32"), "{first}");
        let id = rig.app.inline.image().unwrap();
        assert!(first.contains(&format!("i={id},")), "{first}");
        assert!(!cmds.iter().any(|c| c.starts_with("missing")), "{cmds:?}");
        let edits = cmds.iter().filter(|c| c.starts_with("a=f,r=1")).count();
        assert!(edits >= 5, "{cmds:?}");
        assert!(cmds.iter().all(|c| c.len() < 200));
        let fg = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
        let video_cells = rig.buf.content.iter().filter(|c| c.fg == fg).count();
        assert!(video_cells > 1000, "{video_cells}");
        let text = buffer_text(&rig.buf);
        assert!(text.contains(" ▶ 0:0"), "{text}");
        assert!(text.contains("vol 100%"), "{text}");
        assert!(text.contains("space play/pause"), "{text}");
        let key = |code| {
            Event::Input(crossterm::event::Event::Key(
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
            ))
        };
        rig.app.handle(key(crossterm::event::KeyCode::Char(' ')));
        rig.app.handle(key(crossterm::event::KeyCode::Down));
        rig.app.handle(key(crossterm::event::KeyCode::Char('m')));
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let out = rig.frame();
            kitty_reads_shm(&out);
            let text = buffer_text(&rig.buf);
            if text.contains('⏸') && text.contains("muted") {
                break;
            }
            assert!(std::time::Instant::now() < until, "{text}");
            let _ = tokio::time::timeout(Duration::from_millis(50), rig.rx.recv()).await;
            while let Ok(ev) = rig.rx.try_recv() {
                rig.app.handle(ev);
            }
        }
        rig.app.handle(key(crossterm::event::KeyCode::Char('m')));
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while rig.app.inline.status().is_none_or(|s| s.muted) {
            assert!(std::time::Instant::now() < until);
            let out = rig.frame();
            kitty_reads_shm(&out);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        assert_eq!(rig.app.inline.status().unwrap().volume.round(), 95.0);
        rig.app.handle(key(crossterm::event::KeyCode::Char('j')));
        let out = rig.frame();
        kitty_reads_shm(&out);
        assert!(rig.app.inline.active().is_none());
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")),
            "{text}"
        );
        assert!(matches!(rig.app.popup, Some(Popup::Viewer { pos: 1, .. })));
        rig.app.handle(key(crossterm::event::KeyCode::Char('k')));
        rig.frame();
        assert_eq!(rig.app.inline.requested, Some((41, 0)));
        rig.app.handle(key(crossterm::event::KeyCode::Esc));
        let out = rig.frame();
        kitty_reads_shm(&out);
        assert!(rig.app.inline.active().is_none());
        assert_eq!(rig.app.inline.requested, None);
        let leftover = std::fs::read_dir("/dev/shm")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("gramtty-video-{}-", std::process::id()))
            })
            .count();
        assert_eq!(leftover, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opening_and_closing_viewer_repaints_everything() {
        let mut rig = Rig::new(100, 30);
        rig.app.msg_sel = Some(39);
        rig.settle().await;
        rig.screen.soil_blanks();
        rig.app.popup = Some(Popup::Viewer {
            chat_id: 10,
            items: vec![(38, 0), (34, 0)],
            pos: 0,
        });
        rig.frame();
        let stale = rig.screen.stale_cells(&rig.buf);
        assert!(
            stale.is_empty(),
            "viewer: {:?}",
            &stale[..stale.len().min(5)]
        );
        assert!(buffer_text(&rig.buf).contains(" photo 1/2 "));
        rig.screen.soil_blanks();
        rig.app.popup = None;
        rig.frame();
        let stale = rig.screen.stale_cells(&rig.buf);
        assert!(
            stale.is_empty(),
            "closed viewer: {:?}",
            &stale[..stale.len().min(5)]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unchanged_image_layout_does_not_clear_the_terminal() {
        let mut rig = Rig::new(100, 30);
        rig.app.msg_sel = Some(39);
        rig.settle().await;
        assert!(rig.images_on_screen() > 0);
        let out = rig.frame();
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("\x1b[2J"));
        assert!(!text.contains(PLACEHOLDER));
        rig.app.msg_sel = Some(30);
        let text = String::from_utf8_lossy(&rig.frame()).into_owned();
        assert!(text.contains("\x1b[2J"));
        assert!(text.starts_with("\x1b[?2026h"));
        assert!(text.ends_with("\x1b[?2026l"));
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("gramtty-anim-ui-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn media_msg(id: i64, att: Attachment) -> Msg {
        Msg {
            id,
            chat_id: 10,
            sender: Sender::User(2 + id % 2),
            date: 1_700_000_000 + id * 400,
            edited: false,
            outgoing: false,
            state: SendState::Sent,
            reply_to: None,
            album: 0,
            media: None,
            attachments: vec![att],
            body: RichText::default(),
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

    fn local_file(id: i32, path: &std::path::Path) -> FileRef {
        FileRef {
            id,
            size: std::fs::metadata(path).map_or(1, |m| m.len() as i64),
            path: path.to_string_lossy().into_owned(),
            done: true,
            downloaded: 1,
            ..FileRef::default()
        }
    }

    fn tgs_sticker(id: i32, path: &std::path::Path) -> Attachment {
        let f = local_file(id, path);
        let visual = Visual {
            file: f.clone(),
            decode: Decode::Tgs,
            width: 512,
            height: 512,
        };
        let mut a = Attachment::new(MediaKind::Sticker, f);
        a.width = 512;
        a.height = 512;
        a.preview = Some(visual.clone());
        a.full = Some(visual);
        a
    }

    fn sticker_rig(w: u16, h: u16, n: i64) -> (Rig, std::path::PathBuf) {
        sticker_rig_on(ratatui_image::picker::ProtocolType::Kitty, w, h, n)
    }

    fn sticker_rig_on(
        protocol: ratatui_image::picker::ProtocolType,
        w: u16,
        h: u16,
        n: i64,
    ) -> (Rig, std::path::PathBuf) {
        let mut rig = Rig::with_protocol(w, h, protocol);
        let dir = scratch_dir("rig");
        let tgs = dir.join("s.tgs");
        std::fs::write(&tgs, crate::anim::test_lottie(30, 30)).unwrap();
        let mut history = History::default();
        history.reached_start = true;
        for i in 0..n {
            history.upsert(media_msg(i + 1, tgs_sticker(700 + i as i32, &tgs)));
        }
        rig.app.histories.insert(10, history);
        rig.app.msg_sel = Some(n as usize - 1);
        if protocol == ratatui_image::picker::ProtocolType::Kitty {
            rig.app.media.anim.enable(dir.join("cache"));
        } else {
            rig.app.media.anim.enable_redraw(dir.join("cache"));
        }
        (rig, dir)
    }

    async fn run_until(rig: &mut Rig, log: &mut Vec<u8>, done: impl Fn(&App) -> bool) {
        run_within(rig, log, Duration::from_secs(20), done).await;
    }

    async fn run_within(
        rig: &mut Rig,
        log: &mut Vec<u8>,
        limit: Duration,
        done: impl Fn(&App) -> bool,
    ) {
        let deadline = std::time::Instant::now() + limit;
        loop {
            log.extend(rig.frame());
            if done(&rig.app) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out; live {:?}, status {:?}\n{}",
                rig.app.media.anim.live_ids(),
                rig.app.status,
                buffer_text(&rig.buf)
            );
            if let Ok(Some(ev)) =
                tokio::time::timeout(Duration::from_millis(100), rig.rx.recv()).await
            {
                rig.app.handle(ev);
            }
            while let Ok(ev) = rig.rx.try_recv() {
                rig.app.handle(ev);
            }
        }
    }

    fn kitty_dump(name: &str, log: &[u8]) {
        if let Some(dir) = std::env::var_os("GRAMTTY_KITTY_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join(name), log).unwrap();
        }
    }

    fn placed_ids(buf: &Buffer, ids: &[u32]) -> Vec<usize> {
        ids.iter()
            .map(|id| {
                let fg = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, *id as u8);
                buf.content
                    .iter()
                    .filter(|c| c.symbol().starts_with(PLACEHOLDER) && c.fg == fg)
                    .count()
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inline_animations_are_sent_once_then_kitty_plays_them_without_redraws() {
        let (mut rig, dir) = sticker_rig(100, 60, 12);
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_ids().len() == 4
        })
        .await;
        let ids = rig.app.media.anim.live_ids();
        kitty_dump("inline-live.bin", &log);
        let text = String::from_utf8_lossy(&log).into_owned();
        for id in &ids {
            assert_eq!(
                text.matches(&format!("\x1b_Ga=T,U=1,i={id},")).count(),
                1,
                "transmitted once"
            );
            assert!(text.matches(&format!("\x1b_Ga=f,i={id},")).count() >= 10);
            assert!(text.contains(&crate::anim::start(*id)));
        }
        assert!(
            placed_ids(&rig.buf, &ids).iter().all(|n| *n > 0),
            "{}",
            buffer_text(&rig.buf)
        );
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
        assert_eq!(
            rig.app.tick_interval(),
            None,
            "the app sleeps while kitty animates"
        );
        for _ in 0..3 {
            let out = rig.frame();
            log.extend(&out);
            assert!(!String::from_utf8_lossy(&out).contains("\x1b_G"));
        }
        rig.app.msg_sel = Some(10);
        let out = rig.frame();
        log.extend(&out);
        assert!(
            !String::from_utf8_lossy(&out).contains("a=T"),
            "still on screen"
        );
        rig.app
            .handle(Event::Input(crossterm::event::Event::FocusLost));
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        log.extend(out.as_bytes());
        for id in &ids {
            assert!(out.contains(&crate::anim::stop(*id)), "{out:?}");
        }
        rig.app
            .handle(Event::Input(crossterm::event::Event::FocusGained));
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        log.extend(out.as_bytes());
        for id in &ids {
            assert!(out.contains(&crate::anim::start(*id)));
        }
        rig.app.popup = Some(Popup::Viewer {
            chat_id: 10,
            items: vec![(11, 0)],
            pos: 0,
        });
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        log.extend(out.as_bytes());
        assert!(ids.iter().all(|id| out.contains(&crate::anim::stop(*id))));
        rig.app.popup = None;
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        log.extend(out.as_bytes());
        assert!(ids.iter().all(|id| out.contains(&crate::anim::start(*id))));
        assert_eq!(rig.app.media.anim.live_ids(), ids);
        rig.app.msg_sel = Some(0);
        run_until(&mut rig, &mut log, |app| {
            let live = app.media.anim.live_ids();
            live.len() == 4 && live.iter().all(|id| !ids.contains(id))
        })
        .await;
        let text = String::from_utf8_lossy(&log).into_owned();
        for id in &ids {
            assert_eq!(text.matches(&crate::anim::delete(*id)).count(), 1);
        }
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
        rig.app.media.reset();
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        log.extend(out.as_bytes());
        assert_eq!(out.matches("a=d,d=I").count(), 4);
        kitty_dump("inline.bin", &log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn animation_settings_choose_how_many_and_which_stickers_play() {
        let (mut rig, dir) = sticker_rig(100, 60, 12);
        let mut log = Vec::new();
        rig.app.settings.media.max_animations = 2;
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_ids().len() == 2
        })
        .await;
        for _ in 0..5 {
            log.extend(rig.frame());
            assert_eq!(rig.app.media.anim.live_ids().len(), 2);
        }
        let bottom = rig.app.media.anim.live_ids();
        rig.app.settings.media.animations = crate::config::Animations::Selected;
        rig.app.msg_sel = Some(10);
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_ids().len() == 1
        })
        .await;
        let sel = rig.app.media.anim.live_ids()[0];
        assert_eq!(rig.app.media.anim.live_files(), vec![710]);
        assert!(placed_ids(&rig.buf, &[sel])[0] > 0);
        assert_eq!(bottom.len(), 2);
        rig.app.settings.media.animations = crate::config::Animations::Off;
        let out = String::from_utf8_lossy(&rig.frame()).into_owned();
        assert!(out.contains(&crate::anim::delete(sel)));
        assert!(rig.app.media.anim.live_ids().is_empty());
        rig.settle().await;
        assert!(rig.images_on_screen() > 0, "stickers stay as still images");
        assert!(rig.screen.stale_cells(&rig.buf).is_empty());
        rig.app.settings.media.animations = crate::config::Animations::All;
        rig.app.settings.media.pause_unfocused = false;
        rig.app
            .handle(Event::Input(crossterm::event::Event::FocusLost));
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_ids().len() == 2
        })
        .await;
        let text = String::from_utf8_lossy(&log).into_owned();
        let ids = rig.app.media.anim.live_ids();
        assert!(ids.iter().all(|id| text.contains(&crate::anim::start(*id))));
        assert!(!ids.iter().any(|id| text.contains(&crate::anim::stop(*id))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn picker_items(atts: &[Attachment]) -> Vec<crate::stickers::Item> {
        atts.iter()
            .map(|a| crate::stickers::Item {
                kind: if a.kind == MediaKind::Animation {
                    crate::stickers::ItemKind::Animation
                } else {
                    crate::stickers::ItemKind::Sticker
                },
                file_id: a.file.id,
                emoji: String::new(),
                width: a.width,
                height: a.height,
                duration: a.duration,
                att: a.clone(),
            })
            .collect()
    }

    fn open_picker(rig: &mut Rig, atts: &[Attachment], preview: bool) {
        rig.app.stickers.recent = Some(picker_items(atts));
        let mut sp = crate::stickers::Picker::new(0);
        sp.preview = preview;
        rig.app.popup = Some(Popup::Stickers(sp));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sticker_picker_animations_play_in_kitty_while_the_chat_behind_pauses() {
        let (mut rig, dir) = sticker_rig(100, 40, 2);
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_files() == vec![700, 701]
        })
        .await;
        let chat = rig.app.media.anim.live_ids();
        let tgs = dir.join("s.tgs");
        let atts: Vec<Attachment> = (0..3).map(|i| tgs_sticker(750 + i, &tgs)).collect();
        open_picker(&mut rig, &atts, true);
        log.clear();
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_id(750).is_some()
        })
        .await;
        log.extend(rig.frame());
        let preview = rig.app.media.anim.live_id(750).unwrap();
        let text = String::from_utf8_lossy(&log).into_owned();
        for id in &chat {
            assert!(
                text.contains(&crate::anim::stop(*id)),
                "the chat behind pauses"
            );
        }
        assert!(
            text.contains(&crate::anim::start(preview)),
            "the preview plays"
        );
        assert!(!text.contains(&crate::anim::stop(preview)));
        assert!(placed_ids(&rig.buf, &[preview])[0] > 0);
        if let Some(Popup::Stickers(sp)) = &mut rig.app.popup {
            sp.preview = false;
        }
        log.clear();
        run_until(&mut rig, &mut log, |app| {
            (751..=752).all(|f| app.media.anim.live_id(f).is_some())
        })
        .await;
        log.extend(rig.frame());
        let text = String::from_utf8_lossy(&log).into_owned();
        for f in 750..=752 {
            let id = rig.app.media.anim.live_id(f).unwrap();
            assert!(
                text.contains(&crate::anim::start(id)) || id == preview,
                "grid cell {f} plays"
            );
            assert!(!text.contains(&crate::anim::stop(id)));
        }
        rig.app.popup = None;
        log.clear();
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_files() == vec![700, 701]
        })
        .await;
        let text = String::from_utf8_lossy(&log).into_owned();
        for id in &chat {
            assert!(text.contains(&crate::anim::start(*id)), "the chat resumes");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct Playback {
        changes: usize,
        bytes: usize,
        biggest: usize,
        most_images: usize,
        span: Duration,
    }

    fn sixels(out: &[u8]) -> usize {
        String::from_utf8_lossy(out).matches("\x1bP").count()
    }

    fn play(rig: &mut Rig, steps: usize, step: Duration) -> Playback {
        let start = rig.app.anim_start.elapsed();
        let mut prev = rig.buf.clone();
        let mut p = Playback {
            changes: 0,
            bytes: 0,
            biggest: 0,
            most_images: 0,
            span: Duration::ZERO,
        };
        for _ in 0..steps {
            rig.app.anim_start = rig.app.anim_start.checked_sub(step).unwrap();
            let out = rig.frame();
            p.bytes += out.len();
            p.most_images = p.most_images.max(sixels(&out));
            if rig.buf != prev {
                p.changes += 1;
                p.biggest = p.biggest.max(out.len());
                prev = rig.buf.clone();
            }
        }
        p.span = rig.app.anim_start.elapsed() - start;
        p
    }

    fn assert_within_budget(p: &Playback, what: &str) {
        let floor = crate::anim::frame_gap(0, 1);
        let most = (p.span.as_secs_f64() / floor.as_secs_f64()).ceil() as usize + 1;
        assert!(p.changes >= 4, "{what}: it plays ({} changes)", p.changes);
        assert!(
            p.changes <= most,
            "{what}: {} changes in {:?}, at most {most}",
            p.changes,
            p.span
        );
        let rate = p.bytes as f64 / p.span.as_secs_f64();
        assert!(
            rate <= crate::anim::REDRAW_BYTES_PER_SEC as f64,
            "{what}: {rate} bytes/s"
        );
        eprintln!(
            "{what}: {} frame changes in {:?}, {} bytes per change at most, {:.0} bytes/s",
            p.changes, p.span, p.biggest, rate
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redraw_terminals_animate_the_sticker_picker_within_the_budget() {
        use ratatui_image::picker::ProtocolType;
        for protocol in [
            ProtocolType::Sixel,
            ProtocolType::Iterm2,
            ProtocolType::Halfblocks,
        ] {
            let what = format!("{protocol:?} preview");
            let mut rig = Rig::with_protocol(60, 24, protocol);
            let dir = scratch_dir("redraw-picker");
            let tgs = dir.join("s.tgs");
            std::fs::write(&tgs, crate::anim::test_lottie(30, 30)).unwrap();
            rig.app.media.anim.enable_redraw(dir.join("cache"));
            let atts: Vec<Attachment> = (0..3).map(|i| tgs_sticker(750 + i, &tgs)).collect();
            open_picker(&mut rig, &atts, true);
            let mut log = Vec::new();
            run_until(&mut rig, &mut log, |app| app.redraw_due.is_some()).await;
            assert!(rig.app.tick_interval().unwrap() <= crate::anim::frame_gap(0, 1));
            assert!(rig.app.media.anim.live_ids().is_empty());
            let p = play(&mut rig, 150, Duration::from_millis(10));
            assert_within_budget(&p, &what);
            if protocol == ProtocolType::Sixel {
                assert_eq!(p.most_images, 1, "{what}: one animation at a time");
            }
            assert!(
                !String::from_utf8_lossy(&log).contains("\x1b_Ga=a"),
                "{what}: no kitty animation commands"
            );
            rig.app
                .handle(Event::Input(crossterm::event::Event::FocusLost));
            rig.frame();
            assert_eq!(rig.app.redraw_due, None, "{what}: paused while unfocused");
            let p = play(&mut rig, 20, Duration::from_millis(50));
            assert_eq!(p.changes, 0, "{what}: frozen while unfocused");
            rig.app
                .handle(Event::Input(crossterm::event::Event::FocusGained));
            if let Some(Popup::Stickers(sp)) = &mut rig.app.popup {
                sp.preview = false;
                sp.sel = 1;
            }
            run_until(&mut rig, &mut log, |app| {
                app.redraw_due.is_some() && app.media.clips_ready() >= 2
            })
            .await;
            let p = play(&mut rig, 150, Duration::from_millis(10));
            assert_within_budget(&p, &format!("{protocol:?} grid"));
            if protocol == ProtocolType::Sixel {
                assert_eq!(p.most_images, 1, "{what}: only the selected cell moves");
            }
            rig.app.settings.media.animations = crate::config::Animations::Off;
            rig.frame();
            assert_eq!(rig.app.redraw_due, None);
            assert_eq!(play(&mut rig, 10, Duration::from_millis(50)).changes, 0);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redraw_terminals_animate_few_chat_stickers_and_freeze_under_popups() {
        use ratatui_image::picker::ProtocolType;
        let (mut rig, dir) = sticker_rig_on(ProtocolType::Sixel, 100, 60, 12);
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| {
            app.redraw_due.is_some() && app.media.clips_ready() == crate::anim::REDRAW_MAX_VISIBLE
        })
        .await;
        let p = play(&mut rig, 150, Duration::from_millis(10));
        assert_within_budget(&p, "Sixel chat");
        assert!(
            p.most_images <= crate::anim::REDRAW_MAX_VISIBLE,
            "{}",
            p.most_images
        );
        assert!(p.most_images >= 1);
        assert!(rig.app.media.anim.live_ids().is_empty());
        rig.app.popup = Some(Popup::Options { sel: 0 });
        rig.frame();
        assert_eq!(rig.app.redraw_due, None);
        let mut written = 0;
        for _ in 0..10 {
            rig.app.anim_start = rig
                .app
                .anim_start
                .checked_sub(Duration::from_millis(50))
                .unwrap();
            written += sixels(&rig.frame());
        }
        assert_eq!(written, 0, "frozen under a popup");
        rig.app.popup = None;
        rig.frame();
        assert!(rig.app.redraw_due.is_some(), "plays again");
        rig.app.settings.media.animations = crate::config::Animations::Selected;
        rig.frame();
        let p = play(&mut rig, 100, Duration::from_millis(10));
        assert!(p.changes >= 4 && p.most_images == 1, "{}", p.most_images);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redraw_terminals_keep_a_still_preview_when_decoding_fails() {
        let mut rig = Rig::with_protocol(100, 40, ratatui_image::picker::ProtocolType::Sixel);
        let dir = scratch_dir("redraw-broken");
        let bad = dir.join("bad.webm");
        std::fs::write(&bad, b"not a video").unwrap();
        let thumb = dir.join("t.png");
        image::RgbaImage::from_pixel(64, 64, image::Rgba([20, 90, 200, 255]))
            .save(&thumb)
            .unwrap();
        let f = local_file(760, &bad);
        let mut a = Attachment::new(MediaKind::Sticker, f.clone());
        a.width = 512;
        a.height = 512;
        a.full = Some(Visual {
            file: f,
            decode: Decode::Video,
            width: 512,
            height: 512,
        });
        a.preview = Some(Visual {
            file: local_file(761, &thumb),
            decode: Decode::Raster,
            width: 64,
            height: 64,
        });
        rig.app.media.anim.enable_redraw(dir.join("cache"));
        open_picker(&mut rig, &[a], true);
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| {
            app.status
                .as_ref()
                .is_some_and(|(s, e)| *e && s.contains("inline animation"))
        })
        .await;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while rig.images_on_screen() == 0 && std::time::Instant::now() < deadline {
            if let Ok(Some(ev)) =
                tokio::time::timeout(Duration::from_millis(100), rig.rx.recv()).await
            {
                rig.app.handle(ev);
            }
            rig.frame();
        }
        assert!(rig.images_on_screen() > 0, "the still thumbnail is shown");
        assert_eq!(rig.app.redraw_due, None);
        assert_eq!(rig.app.media.clips_ready(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redraw_terminals_animate_gif_previews() {
        let dir = scratch_dir("redraw-gif");
        let mp4 = dir.join("g.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x120:rate=25:duration=2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&mp4)
            .status();
        match made {
            Ok(s) => assert!(s.success()),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && std::env::var_os("GRAMTTY_REQUIRE_FIXTURES").is_none() =>
            {
                return;
            }
            Err(e) => panic!("ffmpeg: {e}"),
        }
        let mut rig = Rig::with_protocol(100, 40, ratatui_image::picker::ProtocolType::Sixel);
        let mut gif = Attachment::new(MediaKind::Animation, local_file(800, &mp4));
        gif.width = 160;
        gif.height = 120;
        gif.duration = 2;
        rig.app.media.anim.enable_redraw(dir.join("cache"));
        rig.app.stickers.gifs = Some(picker_items(&[gif]));
        let mut sp = crate::stickers::Picker::new(2);
        sp.preview = true;
        rig.app.popup = Some(Popup::Stickers(sp));
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| app.redraw_due.is_some()).await;
        let p = play(&mut rig, 150, Duration::from_millis(10));
        assert_within_budget(&p, "Sixel GIF preview");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn redraw_animation_budget() {
        use ratatui_image::picker::ProtocolType;
        let dir = scratch_dir("redraw-budget");
        let mp4 = dir.join("busy.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=512x512:rate=30:duration=3")
            .args(["-pix_fmt", "yuv420p"])
            .arg(&mp4)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let thumb = dir.join("t.png");
        image::RgbaImage::from_pixel(512, 512, image::Rgba([20, 90, 200, 255]))
            .save(&thumb)
            .unwrap();
        let gif = |id: i32| {
            let mut a = Attachment::new(MediaKind::Animation, local_file(id, &mp4));
            a.width = 512;
            a.height = 512;
            a.duration = 3;
            a.preview = Some(Visual {
                file: local_file(id + 100, &thumb),
                decode: Decode::Raster,
                width: 512,
                height: 512,
            });
            a.full = a.preview.clone();
            a
        };
        for protocol in [
            ProtocolType::Sixel,
            ProtocolType::Iterm2,
            ProtocolType::Halfblocks,
        ] {
            let mut rig = Rig::with_protocol(160, 50, protocol);
            rig.app
                .media
                .anim
                .enable_redraw(dir.join(format!("cache-{protocol:?}")));
            rig.app.stickers.gifs = Some(picker_items(&[gif(800)]));
            let mut sp = crate::stickers::Picker::new(2);
            sp.preview = true;
            rig.app.popup = Some(Popup::Stickers(sp));
            let mut log = Vec::new();
            let started = std::time::Instant::now();
            run_within(&mut rig, &mut log, Duration::from_secs(600), |app| {
                app.redraw_due.is_some()
            })
            .await;
            let ready = started.elapsed();
            let (_, _, cpu0) = proc_stats();
            let p = play(&mut rig, 300, Duration::from_millis(10));
            let (_, _, cpu1) = proc_stats();
            assert_within_budget(&p, &format!("{protocol:?} big GIF preview"));
            eprintln!(
                "  ready after {ready:?}; {:.1} ms CPU per draw",
                (cpu1 - cpu0) as f64 / 300.0
            );
            let mut history = History::default();
            history.reached_start = true;
            for i in 0..4 {
                history.upsert(media_msg(i + 1, gif(810 + i as i32)));
            }
            rig.app.popup = None;
            rig.app.histories.insert(10, history);
            rig.app.msg_sel = Some(3);
            rig.frame();
            run_within(&mut rig, &mut log, Duration::from_secs(600), |app| {
                app.redraw_due.is_some()
            })
            .await;
            let (_, _, cpu0) = proc_stats();
            let p = play(&mut rig, 300, Duration::from_millis(10));
            let (_, _, cpu1) = proc_stats();
            assert_within_budget(&p, &format!("{protocol:?} chat GIFs"));
            eprintln!(
                "  {:.1} ms CPU per draw, {} images at most per draw",
                (cpu1 - cpu0) as f64 / 300.0,
                p.most_images
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gifs_play_inline_from_their_downloaded_file() {
        let dir = scratch_dir("gif");
        let mp4 = dir.join("g.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x120:rate=10:duration=2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&mp4)
            .status();
        match made {
            Ok(s) => assert!(s.success()),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && std::env::var_os("GRAMTTY_REQUIRE_FIXTURES").is_none() =>
            {
                return;
            }
            Err(e) => panic!("ffmpeg: {e}"),
        }
        let mut rig = Rig::new(100, 40);
        let mut history = History::default();
        history.reached_start = true;
        let thumb = dir.join("t.png");
        image::RgbaImage::from_pixel(160, 120, image::Rgba([20, 90, 200, 255]))
            .save(&thumb)
            .unwrap();
        let mut gif = Attachment::new(MediaKind::Animation, local_file(800, &mp4));
        gif.width = 160;
        gif.height = 120;
        gif.duration = 2;
        gif.preview = Some(Visual {
            file: local_file(802, &thumb),
            decode: Decode::Raster,
            width: 160,
            height: 120,
        });
        gif.full = gif.preview.clone();
        history.upsert(media_msg(1, gif.clone()));
        let mut pending = gif.clone();
        pending.file = FileRef {
            id: 801,
            size: 1000,
            ..FileRef::default()
        };
        history.upsert(media_msg(2, pending));
        rig.app.histories.insert(10, history);
        rig.app.msg_sel = Some(1);
        rig.app.media.anim.enable(dir.join("cache"));
        let mut log = Vec::new();
        run_until(&mut rig, &mut log, |app| {
            app.media.anim.live_ids().len() == 1
        })
        .await;
        let id = rig.app.media.anim.live_ids()[0];
        let text = String::from_utf8_lossy(&log).into_owned();
        let frames = text.matches(&format!("\x1b_Ga=f,i={id},")).count() + 1;
        assert!((10..=21).contains(&frames), "{frames}");
        assert!(placed_ids(&rig.buf, &[id])[0] > 0);
        assert!(
            rig.app.media.download_wanted(801),
            "the GIF that is not on disk yet is fetched"
        );
        kitty_dump("gif.bin", &log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn proc_stats() -> (u64, u64, u64) {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let kb = |key: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap()
        };
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        let ticks: u64 = fields[11..15]
            .iter()
            .map(|f| f.parse::<u64>().unwrap())
            .sum();
        (kb("VmRSS:"), kb("VmHWM:"), ticks * 10)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn inline_animation_budget() {
        let dir = scratch_dir("budget");
        let ff = |name: &str, lavfi: &str, codec: &[&str]| {
            let out = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", lavfi])
                .args(codec)
                .arg(&out)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{name}");
            out
        };
        let mut atts = Vec::new();
        for i in 0..6 {
            let p = dir.join(format!("s{i}.tgs"));
            std::fs::write(&p, crate::anim::test_lottie(60, 180 - i * 10)).unwrap();
            atts.push(tgs_sticker(900 + i as i32, &p));
        }
        for i in 0..3 {
            let p = ff(
                &format!("w{i}.webm"),
                &format!(
                    "color=c=green@0.0:s=512x512:d=3:r=30,format=yuva420p[bg];color=c=yellow:s=160x160:d=3:r=30,format=yuva420p[b];[bg][b]overlay=x='t*100+{i}':y=170"
                ),
                &[
                    "-c:v",
                    "libvpx-vp9",
                    "-pix_fmt",
                    "yuva420p",
                    "-auto-alt-ref",
                    "0",
                ],
            );
            let f = local_file(920 + i, &p);
            let v = Visual {
                file: f.clone(),
                decode: Decode::Video,
                width: 512,
                height: 512,
            };
            let mut a = Attachment::new(MediaKind::Sticker, f);
            a.width = 512;
            a.height = 512;
            a.preview = Some(v.clone());
            a.full = Some(v);
            atts.push(a);
        }
        for i in 0..3 {
            let p = ff(
                &format!("g{i}.mp4"),
                &format!("testsrc2=size=480x270:rate=30:duration={}", 3 + i * 2),
                &["-pix_fmt", "yuv420p"],
            );
            let f = local_file(940 + i, &p);
            let v = Visual {
                file: f.clone(),
                decode: Decode::Video,
                width: 480,
                height: 270,
            };
            let mut a = Attachment::new(MediaKind::Animation, f);
            a.width = 480;
            a.height = 270;
            a.duration = 3 + i * 2;
            a.preview = Some(v.clone());
            a.full = Some(v);
            atts.push(a);
        }
        let mut rig = Rig::new(160, 50);
        rig.app.show_folders = false;
        rig.app.show_chats = false;
        let mut history = History::default();
        history.reached_start = true;
        let n = atts.len();
        for (i, a) in atts.into_iter().enumerate() {
            history.upsert(media_msg(i as i64 + 1, a));
        }
        rig.app.histories.insert(10, history);
        rig.app.media.anim.enable(dir.join("cache"));
        let keep = std::env::var_os("GRAMTTY_KITTY_DUMP").is_some();
        let (rss0, _, cpu0) = proc_stats();
        let started = std::time::Instant::now();
        let mut log = Vec::new();
        let mut transmitted = 0usize;
        let mut sent = 0usize;
        for sel in (0..n).rev() {
            rig.app.msg_sel = Some(sel);
            loop {
                let out = rig.frame();
                transmitted += out.len();
                sent += String::from_utf8_lossy(&out)
                    .matches("\x1b_Ga=T,U=1,i=")
                    .count();
                if keep {
                    log.extend(out);
                }
                match tokio::time::timeout(Duration::from_millis(1500), rig.rx.recv()).await {
                    Ok(Some(ev)) => rig.app.handle(ev),
                    _ => break,
                }
                while let Ok(ev) = rig.rx.try_recv() {
                    rig.app.handle(ev);
                }
            }
        }
        let (rss1, hwm, cpu1) = proc_stats();
        let (entries, still_bytes) = rig.app.media.stats();
        let cache: u64 = std::fs::read_dir(dir.join("cache"))
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        eprintln!(
            "{n} animations decoded in {:?} wall, {} ms CPU with ffmpeg; {sent} transmissions, {} KB written to the terminal; disk cache {} KB; rss {} -> {} MB, peak {} MB; still-image cache {entries} entries {} KB; live now {}",
            started.elapsed(),
            cpu1 - cpu0,
            transmitted / 1024,
            cache / 1024,
            rss0 / 1024,
            rss1 / 1024,
            hwm / 1024,
            still_bytes / 1024,
            rig.app.media.anim.live_ids().len()
        );
        assert_eq!(rig.app.tick_interval(), None);
        let (_, _, idle0) = proc_stats();
        let idle_start = std::time::Instant::now();
        let mut idle_bytes = 0;
        while idle_start.elapsed() < Duration::from_secs(3) {
            if let Ok(Some(ev)) =
                tokio::time::timeout(Duration::from_millis(500), rig.rx.recv()).await
            {
                rig.app.handle(ev);
                idle_bytes += rig.frame().len();
            }
        }
        let (_, _, idle1) = proc_stats();
        eprintln!(
            "3 s idle with {} live animations: {} ms CPU, {idle_bytes} bytes written",
            rig.app.media.anim.live_ids().len(),
            idle1 - idle0
        );
        kitty_dump("budget.bin", &log);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn buffer_text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }
}

#[cfg(test)]
mod layout_helper_tests {
    use super::*;

    #[test]
    fn centered_even_and_odd_sizes() {
        assert_eq!(
            centered(Rect::new(0, 0, 100, 40), 40, 10),
            Rect::new(30, 15, 40, 10)
        );
        assert_eq!(
            centered(Rect::new(0, 0, 101, 41), 40, 10),
            Rect::new(30, 15, 40, 10)
        );
        assert_eq!(
            centered(Rect::new(0, 0, 100, 40), 41, 11),
            Rect::new(29, 14, 41, 11)
        );
    }

    #[test]
    fn centered_respects_area_origin() {
        assert_eq!(
            centered(Rect::new(10, 5, 50, 20), 20, 6),
            Rect::new(25, 12, 20, 6)
        );
    }

    #[test]
    fn centered_clamps_to_area_minus_border() {
        assert_eq!(
            centered(Rect::new(0, 0, 30, 10), 80, 40),
            Rect::new(1, 1, 28, 8)
        );
        assert_eq!(
            centered(Rect::new(3, 4, 30, 10), 30, 10),
            Rect::new(4, 5, 28, 8)
        );
    }

    #[test]
    fn centered_tiny_areas_keep_one_cell() {
        assert_eq!(
            centered(Rect::new(0, 0, 2, 2), 10, 10),
            Rect::new(0, 0, 1, 1)
        );
        assert_eq!(
            centered(Rect::new(0, 0, 1, 1), 10, 10),
            Rect::new(0, 0, 1, 1)
        );
        assert_eq!(
            centered(Rect::new(5, 6, 0, 0), 10, 10),
            Rect::new(5, 6, 1, 1)
        );
        assert_eq!(
            centered(Rect::new(0, 0, 10, 10), 0, 0),
            Rect::new(4, 4, 1, 1)
        );
    }

    #[test]
    fn split_filter_inactive_returns_whole_area() {
        let a = Rect::new(2, 3, 20, 8);
        assert_eq!(split_filter(a, false), (a, None));
    }

    #[test]
    fn split_filter_active_takes_last_row() {
        let a = Rect::new(2, 3, 20, 8);
        assert_eq!(
            split_filter(a, true),
            (Rect::new(2, 3, 20, 7), Some(Rect::new(2, 10, 20, 1)))
        );
        assert_eq!(
            split_filter(Rect::new(0, 0, 5, 2), true),
            (Rect::new(0, 0, 5, 1), Some(Rect::new(0, 1, 5, 1)))
        );
    }

    #[test]
    fn split_filter_too_short_returns_whole_area() {
        for h in [0, 1] {
            let a = Rect::new(1, 1, 9, h);
            assert_eq!(split_filter(a, true), (a, None));
        }
    }
}
