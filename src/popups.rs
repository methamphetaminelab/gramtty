use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, LogView, MembersView, OPTION_GROUPS, OPTIONS, Popup, ProfileView, ReactionPicker,
    SearchView, option_key,
};
use crate::format;
use crate::model::ProfileTarget;
use crate::model::{self, RichText, Sender};
use crate::theme;
use crate::ui::{centered, list_window, row_line};

fn framed(
    frame: &mut Frame,
    rect: Rect,
    title: &str,
    hint: &str,
    lines: Vec<Line<'static>>,
) -> Rect {
    frame.render_widget(Clear, rect);
    let mut block = theme::popup_block().title(Span::styled(
        format!(" {} ", format::sanitize(title)),
        theme::popup_title(),
    ));
    if !hint.is_empty() {
        block = block.title_bottom(Span::styled(format!(" {hint} "), theme::muted()));
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
    inner
}

fn scrolled(frame: &mut Frame, rect: Rect, title: &str, lines: Vec<Line<'static>>, scroll: u16) {
    let visible = rect.height.saturating_sub(2) as usize;
    let max = lines.len().saturating_sub(visible);
    let skip = (scroll as usize).min(max);
    let hint = if max > 0 {
        "j/k scroll · Esc close"
    } else {
        "Esc close"
    };
    framed(
        frame,
        rect,
        title,
        hint,
        lines.into_iter().skip(skip).collect(),
    );
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match rows.last_mut() {
            Some(row) if row.width() + 1 + word.width() <= width => {
                row.push(' ');
                row.push_str(word);
            }
            _ => rows.push(word.to_owned()),
        }
    }
    rows
}

fn muted(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), theme::muted()))
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App, popup: &Popup) {
    match popup {
        Popup::Profile(p) => draw_profile(frame, area, app, p),
        Popup::Reactions(r) => draw_reactions(frame, area, app, r),
        Popup::ReactionList { items, sel, .. } => {
            let rect = centered(area, 46, 20);
            let w = rect.width.saturating_sub(2) as usize;
            let rows = rect.height.saturating_sub(2) as usize;
            app.popup_rows.set(rows);
            let lines = match items {
                None => vec![muted(" loading...")],
                Some(list) if list.is_empty() => vec![muted(" nobody yet")],
                Some(list) => {
                    let sel = (*sel).min(list.len() - 1);
                    let start = list_window(sel, list.len(), rows);
                    list.iter()
                        .enumerate()
                        .skip(start)
                        .take(rows)
                        .map(|(i, (sender, emoji))| {
                            row_line(
                                vec![
                                    Span::raw(format!("{}  ", model::reaction_chip(Some(emoji)))),
                                    Span::raw(app.sender_name(*sender)),
                                ],
                                i == sel,
                                true,
                                w,
                            )
                        })
                        .collect()
                }
            };
            framed(frame, rect, "Reactions", "Enter profile · Esc close", lines);
        }
        Popup::Search(sv) => draw_search(frame, area, app, sv),
        Popup::Details {
            chat_id,
            msg_id,
            scroll,
            ..
        } => {
            let rows = app.details_rows(*chat_id, *msg_id).unwrap_or_else(|| {
                vec![(
                    "Message".to_owned(),
                    "no longer loaded in this chat".to_owned(),
                )]
            });
            let rect = centered(area, 76, rows.len() as u16 + 2);
            let w = rect.width.saturating_sub(2) as usize;
            let lines: Vec<Line<'static>> = rows
                .into_iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!(" {k:<17}"), theme::muted()),
                        Span::raw(format::truncate(&v, w.saturating_sub(18))),
                    ])
                })
                .collect();
            scrolled(frame, rect, "Message details", lines, *scroll);
        }
        Popup::EditHistory {
            chat_id,
            msg_id,
            scroll,
        } => {
            let rect = centered(area, 76, 24);
            let w = rect.width.saturating_sub(4) as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            for (when, text) in app.edit_versions(*chat_id, *msg_id) {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(format!(" {when}"), theme::muted())));
                for l in format::render_text(&RichText::plain(text), theme::normal(), w) {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(l.spans);
                    lines.push(Line::from(spans));
                }
            }
            scrolled(frame, rect, "Edit history", lines, *scroll);
        }
        Popup::ConfirmClearDeleted => {
            let lines = vec![
                Line::from(format!(
                    " Erase {} saved deleted messages and edits?",
                    app.archive.len()
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" y ", theme::active_field()),
                    Span::raw("erase   "),
                    Span::styled("n ", theme::active_field()),
                    Span::raw("cancel"),
                ]),
            ];
            framed(
                frame,
                centered(area, 56, 5),
                "Saved deleted messages",
                "",
                lines,
            );
        }
        Popup::Buttons(b) => {
            let w = 62usize.min(area.width.saturating_sub(4) as usize);
            let mut lines = Vec::new();
            if !b.keyboard.placeholder.is_empty() {
                lines.push(muted(format!(" {}", b.keyboard.placeholder)));
            }
            lines.extend(
                crate::ui::keyboard_block(&b.keyboard, w, Some(b.sel), true)
                    .into_iter()
                    .map(|mut l| {
                        l.spans.insert(0, Span::raw(" "));
                        l
                    }),
            );
            let hint = "key or Enter press · ←→ select · Esc";
            let content = lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.width()).sum::<usize>())
                .max()
                .unwrap_or(0);
            let rect = centered(
                area,
                (content + 3).max(hint.width() + 4) as u16,
                (lines.len() as u16 + 2).max(5),
            );
            let title = if b.keyboard.inline {
                "Bot buttons"
            } else {
                "Keyboard"
            };
            framed(frame, rect, title, hint, lines);
        }
        Popup::ConfirmUrl(url) => {
            let rect = centered(area, 64, 8);
            let w = rect.width.saturating_sub(4) as usize;
            let mut lines = vec![
                Line::from(" Open this link in the browser?"),
                Line::from(""),
            ];
            for l in format::render_text(&RichText::plain(url.clone()), theme::link(), w) {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(l.spans);
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" y ", theme::active_field()),
                Span::raw("open   "),
                Span::styled("c ", theme::active_field()),
                Span::raw("copy   "),
                Span::styled("n ", theme::active_field()),
                Span::raw("cancel"),
            ]));
            let h = (lines.len() as u16 + 2).min(area.height);
            framed(frame, centered(area, 64, h), "Open link", "", lines);
        }
        Popup::Options { sel } => {
            let width = 72u16.min(area.width.saturating_sub(2)).max(1);
            let w = width.saturating_sub(2) as usize;
            let sel = (*sel).min(OPTIONS.len() - 1);
            let mut list = Vec::new();
            let mut sel_row = 0;
            for (group, items) in OPTION_GROUPS {
                list.push(Line::from(Span::styled(
                    format!(" {group}"),
                    theme::popup_title(),
                )));
                for &i in items {
                    let (label, _) = OPTIONS[i];
                    let value = app.option_value(i);
                    let text = if value.starts_with('[') {
                        format!("{value} {label}")
                    } else {
                        format!("{label}: {value}")
                    };
                    if i == sel {
                        sel_row = list.len();
                    }
                    list.push(row_line(
                        vec![
                            Span::styled(format!("{} ", option_key(i)), theme::active_field()),
                            Span::raw(text),
                        ],
                        i == sel,
                        true,
                        w,
                    ));
                }
            }
            let desc_w = w.saturating_sub(2);
            let desc_rows = wrap_words(OPTIONS[sel].1, desc_w);
            let tallest = OPTIONS
                .iter()
                .map(|(_, d)| wrap_words(d, desc_w).len())
                .max()
                .unwrap_or(0);
            let inner_h = (area.height as usize).saturating_sub(2);
            let slots = tallest.min(inner_h.saturating_sub(6).max(1));
            let list_room = inner_h.saturating_sub(slots + 2).max(1);
            let start = if list.len() > list_room {
                sel_row
                    .saturating_sub(list_room / 2)
                    .min(list.len() - list_room)
            } else {
                0
            };
            let mut lines: Vec<Line<'static>> =
                list.into_iter().skip(start).take(list_room).collect();
            lines.push(Line::from(""));
            let ghost = app.settings.privacy.ghost_mode;
            let shown = desc_rows.len().min(slots);
            for (n, row) in desc_rows.iter().take(shown).enumerate() {
                let row = if n + 1 == shown && shown < desc_rows.len() {
                    format::truncate(&format!("{row} ..."), w.saturating_sub(1))
                } else {
                    row.clone()
                };
                lines.push(muted(format!(" {row}")));
            }
            lines.extend((shown..slots).map(|_| Line::from("")));
            lines.push(if ghost {
                Line::from(Span::styled(
                    " sending a message still updates your last seen on the server",
                    theme::warning(),
                ))
            } else {
                Line::from("")
            });
            let h = (lines.len() as u16 + 2).min(area.height);
            framed(
                frame,
                centered(area, width, h),
                "Options",
                "j/k move · h/l change · saved to config.toml",
                lines,
            );
        }
        Popup::Inbox { sel } => {
            let rect = centered(area, 80, 22);
            let w = rect.width.saturating_sub(2) as usize;
            let rows = rect.height.saturating_sub(2) as usize;
            app.popup_rows.set(rows);
            let lines = if app.inbox.is_empty() {
                vec![muted(" No notifications yet in this session")]
            } else {
                let sel = (*sel).min(app.inbox.len() - 1);
                let start = list_window(sel, app.inbox.len(), rows);
                app.inbox
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(rows)
                    .map(|(i, item)| {
                        let chat = app.sender_name(Sender::Chat(item.chat_id));
                        let from = app.sender_name(item.sender);
                        let who = if from == chat {
                            chat
                        } else {
                            format!("{chat} · {from}")
                        };
                        let mut spans = vec![Span::styled(
                            format!("{} ", format::time_label(item.date)),
                            theme::time(),
                        )];
                        if item.mention {
                            spans.push(Span::styled(
                                format!("{} ", theme::get().glyphs.mention_dot),
                                theme::mention_badge(),
                            ));
                        }
                        spans.push(Span::styled(format!("{who}: "), theme::strong()));
                        spans.push(Span::raw(item.preview.clone()));
                        row_line(spans, i == sel, true, w)
                    })
                    .collect()
            };
            framed(
                frame,
                rect,
                "Notifications",
                "Enter jump · c clear · Esc close",
                lines,
            );
        }
        Popup::QuotePick { input, .. } => draw_quote_pick(frame, area, input),
        Popup::Translate {
            lang,
            result,
            scroll,
            ..
        } => {
            let rect = centered(area, 76, 20);
            let w = rect.width.saturating_sub(4) as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            match result {
                None => lines.push(muted(format!(" translating to {lang} via Telegram..."))),
                Some(Err(e)) => {
                    for row in wrap_words(e, w) {
                        lines.push(Line::from(Span::styled(format!(" {row}"), theme::error())));
                    }
                }
                Some(Ok(text)) => {
                    for l in format::render_text(&RichText::plain(text.clone()), theme::normal(), w)
                    {
                        let mut spans = vec![Span::raw(" ")];
                        spans.extend(l.spans);
                        lines.push(Line::from(spans));
                    }
                }
            }
            let title = format!("Translation → {lang}");
            let visible = rect.height.saturating_sub(2) as usize;
            let max = lines.len().saturating_sub(visible);
            let skip = (*scroll as usize).min(max);
            let hint = if matches!(result, Some(Ok(_))) {
                "y copy · j/k scroll · Esc close"
            } else {
                "Esc close"
            };
            framed(
                frame,
                rect,
                &title,
                hint,
                lines.into_iter().skip(skip).collect(),
            );
        }
        Popup::Members(mv) => draw_members(frame, area, app, mv),
        Popup::EventLog(lv) => draw_event_log(frame, area, app, lv),
        _ => {}
    }
}

fn draw_quote_pick(frame: &mut Frame, area: Rect, input: &crate::input::TextInput) {
    let rect = centered(area, 76, 14);
    let w = rect.width.saturating_sub(4).max(1) as usize;
    let rows = input.visual_lines(w);
    let mut lines: Vec<Line<'static>> = vec![muted(
        " Trim the text to the part you want to quote, then Enter",
    )];
    lines.push(Line::from(""));
    let room = rect.height.saturating_sub(5) as usize;
    let (cur_row, cur_col) = input.cursor_position(w);
    let start = cur_row.saturating_sub(room.saturating_sub(1));
    for row in rows.iter().skip(start).take(room) {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(row.clone(), theme::quote()),
        ]));
    }
    let inner = framed(
        frame,
        rect,
        "Reply with quote",
        "Enter reply · C-w/C-u/Backspace trim · Esc",
        lines,
    );
    frame.set_cursor_position(Position {
        x: (inner.x + 1 + cur_col as u16).min(inner.right().saturating_sub(1)),
        y: (inner.y + 2 + (cur_row - start) as u16).min(inner.bottom().saturating_sub(1)),
    });
}

fn draw_members(frame: &mut Frame, area: Rect, app: &App, mv: &MembersView) {
    let rect = centered(area, 76, 24);
    let w = rect.width.saturating_sub(2) as usize;
    let filters: Vec<Span<'static>> = crate::admin::MemberFilter::ALL
        .iter()
        .flat_map(|f| {
            let style = if *f == mv.filter {
                theme::active_field()
            } else {
                theme::muted()
            };
            [
                Span::styled(format!(" {} ", f.name()), style),
                Span::raw(" "),
            ]
        })
        .collect();
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if mv.input.is_empty() {
                Span::styled("type to search members...", theme::muted())
            } else {
                Span::raw(mv.input.text().to_owned())
            },
        ]),
        Line::from(filters),
    ];
    let rows = (rect.height as usize).saturating_sub(2 + lines.len() + 1);
    app.popup_rows.set(rows);
    match &mv.page {
        None => lines.push(muted(" loading...")),
        Some(Err(e)) => lines.push(Line::from(Span::styled(format!(" {e}"), theme::error()))),
        Some(Ok(p)) if p.members.is_empty() => lines.push(muted(" nobody found")),
        Some(Ok(p)) => {
            lines.push(muted(format!(
                " {} shown of {}",
                p.members.len(),
                app.count(i64::from(p.total))
            )));
            let sel = mv.sel.min(p.members.len() - 1);
            let start = list_window(sel, p.members.len(), rows);
            for (i, m) in p.members.iter().enumerate().skip(start).take(rows) {
                let mut spans = vec![Span::styled(app.sender_name(m.sender), theme::strong())];
                spans.push(Span::styled(format!("  {}", m.status), theme::muted()));
                if m.joined > 0 {
                    spans.push(Span::styled(
                        format!("  joined {}", format::date_label(i64::from(m.joined))),
                        theme::muted(),
                    ));
                }
                lines.push(row_line(spans, i == sel, true, w));
            }
        }
    }
    let title = format!(
        "Members of {}",
        format::truncate(&app.sender_name(Sender::Chat(mv.chat_id)), 40)
    );
    let inner = framed(
        frame,
        rect,
        &title,
        "Tab filter · ↑↓ select · Enter profile · Esc",
        lines,
    );
    let (_, col) = mv.input.cursor_position(0);
    frame.set_cursor_position(Position {
        x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
        y: inner.y,
    });
}

fn draw_event_log(frame: &mut Frame, area: Rect, app: &App, lv: &LogView) {
    let rect = centered(area, 96, 24);
    let w = rect.width.saturating_sub(2) as usize;
    let rows = (rect.height as usize).saturating_sub(3);
    app.popup_rows.set(rows);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if lv.entries.is_empty() {
        if lv.loading {
            lines.push(muted(" loading..."));
        } else if lv.error.is_none() {
            lines.push(muted(" no recent actions in the last 48 hours"));
        }
    }
    let sel = lv.sel.min(lv.entries.len().saturating_sub(1));
    let start = list_window(sel, lv.entries.len(), rows);
    for (i, e) in lv.entries.iter().enumerate().skip(start).take(rows) {
        let mut spans = vec![
            Span::styled(
                format!(
                    "{} {} ",
                    format::date_label(i64::from(e.date)),
                    format::time_label(i64::from(e.date))
                ),
                theme::muted(),
            ),
            Span::styled(app.sender_name(e.actor), theme::strong()),
        ];
        if !e.action.is_empty() {
            spans.push(Span::raw(format!(" {}", e.action)));
        }
        if let Some(t) = e.target {
            spans.push(Span::styled(
                format!(" {}", app.sender_name(t)),
                theme::strong(),
            ));
        }
        if !e.detail.is_empty() {
            let sep = if e.action.is_empty() && e.target.is_none() {
                " "
            } else {
                ": "
            };
            spans.push(Span::styled(
                format!("{sep}{}", format::one_line(&e.detail)),
                theme::muted(),
            ));
        }
        lines.push(row_line(spans, i == sel, true, w));
    }
    if let Some(e) = &lv.error {
        lines.push(Line::from(Span::styled(format!(" {e}"), theme::error())));
    } else if lv.loading && !lv.entries.is_empty() {
        lines.push(muted(" loading more..."));
    }
    let title = format!(
        "Recent actions in {}",
        format::truncate(&app.sender_name(Sender::Chat(lv.chat_id)), 40)
    );
    framed(
        frame,
        rect,
        &title,
        "j/k select · Enter profile · Esc",
        lines,
    );
}

pub const PROFILE_AVATAR: (u16, u16) = (10, 5);

pub fn profile_rect(area: Rect) -> Rect {
    centered(area, 64, 24)
}

fn avatar_cells(name: &str, id: i64, row: u16) -> Span<'static> {
    let cols = PROFILE_AVATAR.0 as usize;
    let text = if row == PROFILE_AVATAR.1 / 2 {
        let label = crate::ui::initials(name);
        let pad = cols.saturating_sub(label.width());
        format!(
            "{}{label}{}",
            " ".repeat(pad / 2),
            " ".repeat(pad - pad / 2)
        )
    } else {
        " ".repeat(cols)
    };
    Span::styled(text, theme::avatar(id))
}

fn draw_profile(frame: &mut Frame, area: Rect, app: &App, p: &ProfileView) {
    let rect = profile_rect(area);
    let w = rect.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    match (&p.data, &p.error) {
        (_, Some(e)) => lines.push(Line::from(Span::styled(format!(" {e}"), theme::error()))),
        (None, None) => lines.push(muted(" Loading profile...")),
        (Some(d), None) => {
            lines.push(Line::from(Span::styled(
                format!(" {}", format::truncate(&d.title, w)),
                theme::strong(),
            )));
            if !d.usernames.is_empty() {
                let names: Vec<String> = d.usernames.iter().map(|n| format!("@{n}")).collect();
                lines.push(Line::from(Span::styled(
                    format!(" {}", format::truncate(&names.join(" · "), w)),
                    theme::mention(),
                )));
            }
            let status = d
                .user_id
                .and_then(|id| app.user_status(id).map(|s| (s, app.is_online(id))));
            match status {
                Some((s, true)) => {
                    lines.push(Line::from(Span::styled(format!(" {s}"), theme::online())))
                }
                Some((s, false)) => lines.push(muted(format!(" {s} · {}", d.kind))),
                None => lines.push(muted(format!(" {}", d.kind))),
            }
            if !d.bio.is_empty() {
                lines.push(Line::from(""));
                let heading = if d.user_id.is_some() {
                    "Bio"
                } else {
                    "Description"
                };
                lines.push(muted(format!(" {heading}")));
                for l in format::render_text(&RichText::plain(d.bio.clone()), theme::normal(), w) {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(l.spans);
                    lines.push(Line::from(spans));
                }
            }
            lines.push(Line::from(""));
            let field = |label: &str, value: String| {
                Line::from(vec![
                    Span::styled(format!(" {label:<13}"), theme::muted()),
                    Span::raw(format::truncate(&value, w.saturating_sub(14))),
                ])
            };
            if !d.phone.is_empty() {
                let mine = d.user_id.is_some() && d.user_id == app.my_id;
                let phone = if mine && app.settings.privacy.hide_phone {
                    "hidden".to_owned()
                } else {
                    d.phone.clone()
                };
                lines.push(field("Phone", phone));
            }
            if let Some(n) = d.common_chats.filter(|_| app.my_id != d.user_id) {
                lines.push(field("Common chats", n.to_string()));
            }
            if let Some(n) = d.members {
                lines.push(field("Members", app.count(i64::from(n))));
            }
            for (i, link) in d.links.iter().enumerate() {
                lines.push(field(if i == 0 { "Link" } else { "" }, link.clone()));
            }
            lines.push(field("ID", d.id.to_string()));
            if app.settings.appearance.show_ids {
                match (p.target, d.user_id) {
                    (ProfileTarget::Chat(_), Some(u)) => {
                        lines.push(field("User ID", u.to_string()))
                    }
                    (ProfileTarget::User(_), _) => {
                        lines.push(field("Chat ID", d.id.to_string()));
                    }
                    _ => {}
                }
            }
            if !d.commands.is_empty() {
                lines.push(Line::from(""));
                lines.push(muted(" Commands"));
                for c in &d.commands {
                    lines.push(Line::from(vec![
                        Span::styled(format!(" /{} ", c.name), theme::active_field()),
                        Span::styled(
                            format::truncate(&c.description, w.saturating_sub(c.name.width() + 3)),
                            theme::muted(),
                        ),
                    ]));
                }
            }
        }
    }
    if let Some(d) = &p.data {
        let rows = PROFILE_AVATAR.1 as usize;
        while lines.len() < rows {
            lines.push(Line::from(""));
        }
        for (i, line) in lines.iter_mut().take(rows).enumerate() {
            let mut spans = vec![Span::raw(" "), avatar_cells(&d.title, d.id, i as u16)];
            spans.extend(std::mem::take(&mut line.spans));
            *line = Line::from(spans);
        }
    }
    let visible = rect.height.saturating_sub(2) as usize;
    app.popup_rows.set(visible);
    let max_scroll = lines.len().saturating_sub(visible);
    let skip = (p.scroll as usize).min(max_scroll);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(skip).collect();
    let secret = p
        .data
        .as_ref()
        .and_then(|d| d.user_id)
        .is_some_and(|u| app.can_start_secret(u));
    let hint = match (max_scroll > 0, secret) {
        (true, true) => {
            "j/k scroll · a their messages here · S secret chat (this device only) · Esc"
        }
        (false, true) => "a their messages here · S secret chat (this device only) · Esc close",
        (true, false) => "j/k scroll · a their messages here · Esc close",
        (false, false) => "a their messages here · Esc close",
    };
    framed(frame, rect, "Profile", hint, shown);
}

fn draw_reactions(frame: &mut Frame, area: Rect, app: &App, r: &ReactionPicker) {
    const PER_ROW: usize = 8;
    let chosen: Vec<String> = app
        .histories
        .get(&r.chat_id)
        .and_then(|h| h.get(r.msg_id))
        .map(|m| {
            m.reactions
                .iter()
                .filter(|x| x.chosen)
                .filter_map(|x| x.emoji.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut lines: Vec<Line<'static>> = Vec::new();
    if r.loading {
        lines.push(muted(" loading reactions..."));
    }
    for (ri, chunk) in r.emojis.chunks(PER_ROW).enumerate() {
        let mut spans = vec![Span::raw(" ")];
        for (ci, e) in chunk.iter().enumerate() {
            let i = ri * PER_ROW + ci;
            let mark = if chosen.contains(e) { "•" } else { " " };
            let style = if i == r.sel {
                theme::picker_selection()
            } else {
                theme::normal()
            };
            spans.push(Span::styled(
                format!(" {}{mark}", model::reaction_chip(Some(e))),
                style,
            ));
        }
        lines.push(Line::from(spans));
    }
    if let Some(n) = &r.note {
        lines.push(Line::from(Span::styled(format!(" {n}"), theme::warning())));
    } else if !r.loading && r.emojis.is_empty() {
        lines.push(muted(" no reactions available here"));
    }
    let h = (lines.len() as u16 + 2).max(4);
    framed(
        frame,
        centered(area, 44, h),
        "React",
        "←→ select · Enter toggle · Esc",
        lines,
    );
}

fn draw_search(frame: &mut Frame, area: Rect, app: &App, sv: &SearchView) {
    let rect = centered(area, 84, 24);
    let w = rect.width.saturating_sub(2) as usize;
    let title = match (sv.sender, sv.pinned, sv.chat_id) {
        (Some(s), _, _) => format!(
            "Messages from {}",
            format::truncate(&app.sender_name(s), 40)
        ),
        (None, true, _) => "Pinned messages".to_owned(),
        (None, false, Some(id)) => format!(
            "Search in {}",
            format::truncate(&app.sender_name(Sender::Chat(id)), 40)
        ),
        (None, false, None) => "Search all chats".to_owned(),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if !sv.listing() {
        lines.push(Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if sv.input.is_empty() {
                Span::styled("type and press Enter...", theme::muted())
            } else {
                Span::raw(sv.input.text().to_owned())
            },
        ]));
        lines.push(Line::from(Span::styled(
            theme::get().glyphs.separator.repeat(w),
            theme::border(),
        )));
    }
    let rows = (rect.height as usize).saturating_sub(2 + lines.len());
    app.popup_rows.set(rows);
    if sv.loading {
        lines.push(muted(" searching..."));
    } else if let Some(e) = &sv.error {
        lines.push(Line::from(Span::styled(format!(" {e}"), theme::error())));
    } else if sv.results.is_empty() && sv.searched_for.is_some() {
        lines.push(muted(" nothing found"));
    } else {
        let sel = sv.sel.min(sv.results.len().saturating_sub(1));
        let start = list_window(sel, sv.results.len(), rows);
        for (i, m) in sv.results.iter().enumerate().skip(start).take(rows) {
            let mut spans = vec![Span::styled(
                format!(
                    "{} {} ",
                    format::date_label(m.date),
                    format::time_label(m.date)
                ),
                theme::muted(),
            )];
            if sv.chat_id.is_none() {
                spans.push(Span::styled(
                    format!("{} · ", app.sender_name(Sender::Chat(m.chat_id))),
                    theme::mention(),
                ));
            }
            spans.push(Span::styled(
                format!("{}: ", app.sender_name(m.sender)),
                theme::strong(),
            ));
            spans.push(Span::raw(model::preview(m)));
            lines.push(row_line(spans, i == sel, true, w));
        }
    }
    let hint = if sv.listing() {
        "Enter jump · j/k select · Esc"
    } else {
        "Enter search / jump · ↑↓ select · Esc"
    };
    let inner = framed(frame, rect, &title, hint, lines);
    if !sv.listing() {
        let (_, col) = sv.input.cursor_position(0);
        frame.set_cursor_position(Position {
            x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
            y: inner.y,
        });
    }
}
