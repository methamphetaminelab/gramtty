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
use crate::format::{self, render_text};
use crate::model::{ChatKind, Msg, SendState, Sender};
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
    draw_popup(frame, area, app);
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

struct Rendered {
    lines: Vec<Line<'static>>,
}

fn message_block(app: &App, msgs: &[Msg], i: usize, width: usize, selected: bool) -> Rendered {
    let m = &msgs[i];
    let prev = i.checked_sub(1).map(|j| &msgs[j]);
    let new_day = prev.is_none_or(|p| !format::same_day(p.date, m.date));
    let show_header = new_day || prev.is_none_or(|p| p.sender != m.sender || m.date - p.date > 300);
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
        if m.edited {
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
    if let Some(media) = &m.media {
        body.extend(render_text(
            &crate::model::RichText::plain(media.clone()),
            theme::media(),
            inner,
        ));
    }
    if !m.body.text.is_empty() {
        body.extend(render_text(&m.body, theme::normal(), inner));
    }
    match &m.state {
        SendState::Sent => {}
        SendState::Pending => body.push(Line::from(Span::styled("sending...", theme::muted()))),
        SendState::Failed(e) => body.push(Line::from(Span::styled(
            format::truncate(&format!("failed: {e}"), inner),
            theme::error(),
        ))),
    }
    if !show_header
        && m.edited
        && let Some(last) = body.last_mut()
    {
        last.spans.push(Span::styled(" (edited)", theme::muted()));
    }
    if selected {
        lines.extend(card(body, width, (!show_header).then_some(time.as_str())));
    } else {
        lines.extend(body);
    }
    Rendered { lines }
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
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut want_older = false;
    match app.msg_sel {
        None => {
            let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
            let mut total = 0;
            let mut i = msgs.len();
            while i > 0 && total < height + app.bottom_offset {
                i -= 1;
                let b = message_block(app, msgs, i, width, false).lines;
                total += b.len();
                blocks.push(b);
            }
            if total < height + app.bottom_offset && !reached_start {
                want_older = true;
            }
            let all: Vec<Line<'static>> = blocks.into_iter().rev().flatten().collect();
            let max_offset = all.len().saturating_sub(height);
            app.bottom_offset = app.bottom_offset.min(max_offset);
            let end = all.len() - app.bottom_offset;
            let start = end.saturating_sub(height);
            lines.extend(all[start..end].iter().cloned());
        }
        Some(sel) => {
            let sel = sel.min(msgs.len() - 1);
            let heights = |from: usize, to: usize, app: &App| -> usize {
                (from..=to)
                    .map(|i| message_block(app, msgs, i, width, i == sel).lines.len())
                    .sum()
            };
            let mut top = app.msg_top.min(sel);
            let mut cache: Option<usize> = None;
            while top < sel {
                let used = *cache.get_or_insert_with(|| heights(top, sel, app));
                if used <= height {
                    break;
                }
                top += 1;
                cache = None;
            }
            let mut below = 0;
            let mut j = top;
            while j < msgs.len() && below <= height {
                below += message_block(app, msgs, j, width, j == sel).lines.len();
                j += 1;
            }
            while top > 0 && below < height {
                let add = message_block(app, msgs, top - 1, width, top - 1 == sel)
                    .lines
                    .len();
                if below + add > height {
                    break;
                }
                below += add;
                top -= 1;
            }
            if top == 0 && !reached_start {
                want_older = true;
            }
            let mut i = top;
            while i < msgs.len() && lines.len() < height {
                lines.extend(message_block(app, msgs, i, width, i == sel).lines);
                i += 1;
            }
            lines.truncate(height);
            app.msg_top = top;
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
    if want_older {
        app.want_older = true;
    }
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
        Popup::ConfirmDelete { .. } => {
            let lines = vec![
                Line::from(" Delete this message?"),
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
            media: (id == 5).then(|| "[photo]".to_owned()),
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
