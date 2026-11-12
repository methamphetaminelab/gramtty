use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, LogView, MembersView, MenuItem, OPTION_GROUPS, OPTIONS, Popup, ProfileView,
    ReactionPicker, SearchView, option_key,
};
use crate::format;
use crate::keymap::HelpLine;
use crate::model::{self, ChatKind, Msg, RichText, Sender};
use crate::search::{self, FindView, Hit, Row, Scope};
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
    let hint = fit_hint(hint, rect.width.saturating_sub(4) as usize);
    if !hint.is_empty() {
        block = block.title_bottom(Span::styled(format!(" {hint} "), theme::muted()));
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
    crate::textmap::panel(rect, inner);
    inner
}

fn fit_hint(hint: &str, max: usize) -> String {
    let mut parts: Vec<&str> = hint.split(" · ").collect();
    while parts.len() > 1 && parts.join(" · ").width() > max {
        parts.remove(0);
    }
    format::truncate(&parts.join(" · "), max)
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

pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
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
        Popup::ReactionList {
            items, sel, read, ..
        } => {
            let rect = centered(area, 46, 20);
            let w = rect.width.saturating_sub(2) as usize;
            let rows = rect.height.saturating_sub(2) as usize;
            app.popup_rows.set(rows);
            let lines = match items {
                None => vec![muted(" Loading…")],
                Some(list) if list.is_empty() => vec![muted(" No one yet")],
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
                                    Span::raw(if *read {
                                        format!("{emoji}  ")
                                    } else {
                                        format!("{}  ", model::reaction_chip(Some(emoji)))
                                    }),
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
            let title = if *read { "Read by" } else { "Reactions" };
            framed(frame, rect, title, "Enter profile · Esc close", lines);
        }
        Popup::Search(sv) => draw_search(frame, area, app, sv),
        Popup::Find(v) => draw_find(frame, area, app, v),
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
        Popup::Links { links, sel } => {
            let width = 72u16.min(area.width.saturating_sub(2)).max(1);
            let w = width.saturating_sub(2) as usize;
            let rows = area.height.saturating_sub(4).max(1) as usize;
            app.popup_rows.set(rows);
            let sel = (*sel).min(links.len().saturating_sub(1));
            let start = list_window(sel, links.len(), rows);
            let lines = links
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, link)| {
                    let key = crate::bots::button_key(i).unwrap_or(' ');
                    let mut spans = vec![
                        Span::styled(format!("{key} "), theme::active_field()),
                        Span::raw(format::sanitize(&format::one_line(&link.text))),
                    ];
                    if link.hidden {
                        spans.push(Span::styled(
                            format!("  {}", format::sanitize(&link.url)),
                            theme::muted(),
                        ));
                    }
                    row_line(spans, i == sel, true, w)
                })
                .collect::<Vec<_>>();
            let rect = centered(area, width, lines.len() as u16 + 2);
            framed(frame, rect, "Open link", "key or Enter open · Esc", lines);
        }
        Popup::Files { items, sel } => {
            let width = 72u16.min(area.width.saturating_sub(2)).max(1);
            let w = width.saturating_sub(2) as usize;
            let rows = area.height.saturating_sub(4).max(1) as usize;
            app.popup_rows.set(rows);
            let sel = (*sel).min(items.len().saturating_sub(1));
            let start = list_window(sel, items.len(), rows);
            let lines = items
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, (msg_id, att))| {
                    let key = crate::bots::button_key(i).unwrap_or(' ');
                    let size = app.media.file(&att.file).size;
                    let mut spans = vec![
                        Span::styled(format!("{key} "), theme::active_field()),
                        Span::raw(format::sanitize(&crate::app::attachment_name(att, *msg_id))),
                    ];
                    if size > 0 {
                        spans.push(Span::styled(
                            format!("  {}", format::file_size(size)),
                            theme::muted(),
                        ));
                    }
                    row_line(spans, i == sel, true, w)
                })
                .collect::<Vec<_>>();
            let rect = centered(area, width, lines.len() as u16 + 2);
            framed(frame, rect, "Open file", "key or Enter open · Esc", lines);
        }
        Popup::ConfirmStart {
            chat_id, parameter, ..
        } => {
            let name = app
                .chats
                .get(chat_id)
                .map_or_else(|| "this bot".to_owned(), |c| c.title.clone());
            let w = 60usize;
            let mut lines = vec![Line::from(format!(
                " Start {}?",
                format::truncate(&format::sanitize(&name), w - 8)
            ))];
            if !parameter.is_empty() {
                lines.push(muted(
                    " The bot gets /start with this parameter from the link:",
                ));
                lines.push(Line::from(Span::styled(
                    format!(" {}", format::truncate(&format::sanitize(parameter), w - 2)),
                    theme::link(),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" y ", theme::active_field()),
                Span::raw("start   "),
                Span::styled("n ", theme::active_field()),
                Span::raw("cancel"),
            ]));
            let h = (lines.len() as u16 + 2).min(area.height);
            framed(
                frame,
                centered(area, w as u16 + 2, h),
                "Start bot",
                "",
                lines,
            );
        }
        Popup::ConfirmJoin(invite) => {
            let w = 60usize;
            let (kind, people) = if invite.channel {
                ("Channel", "subscribers")
            } else {
                ("Group", "members")
            };
            let mut lines = vec![
                Line::from(format!(
                    " Join {}?",
                    format::truncate(&format::sanitize(&invite.title), w - 8)
                )),
                muted(format!(
                    " {kind} · {} {people}",
                    format::count(
                        i64::from(invite.members),
                        app.settings.appearance.exact_numbers
                    )
                )),
            ];
            if invite.request {
                lines.push(muted(" An admin has to approve the request"));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" y ", theme::active_field()),
                Span::raw(if invite.request {
                    "send request   "
                } else {
                    "join   "
                }),
                Span::styled("n ", theme::active_field()),
                Span::raw("cancel"),
            ]));
            let h = (lines.len() as u16 + 2).min(area.height);
            framed(
                frame,
                centered(area, w as u16 + 2, h),
                "Join chat",
                "",
                lines,
            );
        }
        Popup::Options { sel } => {
            let width = 72u16.min(area.width.saturating_sub(2)).max(1);
            let w = width.saturating_sub(2) as usize;
            let sel = (*sel).min(OPTIONS.len() - 1);
            let current = crate::app::option_group(sel);
            let items = OPTION_GROUPS[current].1;
            let left_w = OPTION_GROUPS
                .iter()
                .map(|(name, _)| name.width())
                .max()
                .unwrap_or(0)
                + 1;
            let sep = if w >= 62 { " │ " } else { "│" };
            let right_w = w.saturating_sub(left_w + sep.width());
            let mut list = Vec::new();
            let mut sel_row = 0;
            let tallest_group = OPTION_GROUPS
                .iter()
                .map(|(_, items)| items.len())
                .max()
                .unwrap_or(0);
            for r in 0..OPTION_GROUPS.len().max(tallest_group) {
                let mut spans = vec![match OPTION_GROUPS.get(r) {
                    Some((name, _)) => Span::styled(
                        format!(" {name:<width$}", width = left_w - 1),
                        if r == current {
                            theme::picker_selection()
                        } else {
                            theme::muted()
                        },
                    ),
                    None => Span::raw(" ".repeat(left_w)),
                }];
                spans.push(Span::styled(sep, theme::border()));
                if let Some(&i) = items.get(r) {
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
                    spans.extend(
                        row_line(
                            vec![
                                Span::styled(format!("{} ", option_key(i)), theme::active_field()),
                                Span::raw(text),
                            ],
                            i == sel,
                            true,
                            right_w,
                        )
                        .spans,
                    );
                }
                list.push(Line::from(spans));
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
                    format::truncate(&format!("{row}…"), w.saturating_sub(1))
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
                "Tab category · j/k move · h/l change · saved to config.toml",
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
            let hint = if app.inbox.is_empty() {
                "Esc close"
            } else {
                "Enter jump · c clear · Esc close"
            };
            framed(frame, rect, "Notifications", hint, lines);
        }
        Popup::QuotePick { input, .. } => draw_quote_pick(frame, area, input),
        Popup::TextSelect { input, anchor } => draw_text_select(frame, area, app, input, *anchor),
        Popup::SharedMedia { chat_id, sel } => {
            let rect = centered(area, 40.min(area.width), 9.min(area.height));
            let w = rect.width.saturating_sub(2) as usize;
            let counts = app.media_counts.get(chat_id);
            let lines = model::MediaFilter::ALL
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let count = match counts.and_then(|c| c.iter().find(|(k, _)| k == f)) {
                        Some((_, Some(n))) => app.count(i64::from(*n)),
                        Some((_, None)) => "?".to_owned(),
                        None => "...".to_owned(),
                    };
                    row_line(
                        vec![
                            Span::raw(format!("{:<16}", f.label())),
                            Span::styled(count, theme::muted()),
                        ],
                        i == *sel,
                        true,
                        w,
                    )
                })
                .collect();
            framed(frame, rect, "Shared media", "Enter open · Esc close", lines);
        }
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
                None => lines.push(muted(format!(" Translating to {lang} via Telegram…"))),
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
        Popup::Menu { title, items, sel } => draw_menu(frame, area, app, title, items, *sel),
        Popup::Help { scroll } => draw_help(frame, area, app, *scroll),
        Popup::ConfirmRemoveChat {
            chat_id,
            leave,
            revoke,
        } => {
            let name = format::truncate(
                &format::sanitize(&app.sender_name(Sender::Chat(*chat_id))),
                32,
            );
            let (title, question) = if *leave {
                ("Leave", format!(" Leave {name}?"))
            } else {
                ("Delete chat", format!(" Delete the chat with {name}?"))
            };
            let key = |k: &str| Span::styled(format!(" {k} "), theme::active_field());
            let choices = match (*leave, *revoke) {
                (true, _) => vec![key("y"), Span::raw("leave")],
                (false, true) => vec![
                    key("y"),
                    Span::raw(format!("for me and {name}  ")),
                    key("m"),
                    Span::raw("only for me"),
                    Span::styled(" (Enter)", theme::muted()),
                ],
                (false, false) => vec![
                    key("y"),
                    Span::raw("delete"),
                    Span::styled(" (Enter)", theme::muted()),
                ],
            };
            let lines = vec![
                Line::from(question),
                Line::from(""),
                Line::from(choices),
                Line::from(vec![key("n"), Span::raw("cancel")]),
            ];
            let width = lines.iter().map(Line::width).max().unwrap_or(0) + 4;
            framed(
                frame,
                centered(area, width.max(40) as u16, 6),
                title,
                "",
                lines,
            );
        }
        Popup::StatusLog { scroll } => {
            let rect = centered(area, 90, 22);
            app.popup_rows.set(rect.height.saturating_sub(2) as usize);
            let w = rect.width.saturating_sub(4) as usize;
            let mut lines: Vec<Line<'static>> = Vec::new();
            if app.status_log.is_empty() {
                lines.push(muted(" No notices or errors yet"));
            }
            for entry in app.status_log.iter().rev() {
                let time = format::time_label(entry.at);
                let indent = format::cells(&time) + 2;
                let style = if entry.error {
                    theme::error()
                } else {
                    theme::normal()
                };
                let text = crate::td::sentence_case(&format::one_line(&entry.text));
                for (i, row) in wrap_words(&text, w.saturating_sub(indent).max(10))
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled(format!(" {time} "), theme::muted())
                    } else {
                        Span::raw(" ".repeat(indent))
                    };
                    lines.push(Line::from(vec![lead, Span::styled(row, style)]));
                }
            }
            scrolled(frame, rect, "Recent notices and errors", lines, *scroll);
        }
        _ => {}
    }
}

fn draw_menu(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    items: &[MenuItem],
    sel: usize,
) {
    const HINT: &str = "Enter run · Esc close";
    let label_w = items.iter().map(|i| i.label.width()).max().unwrap_or(0);
    let row_w = theme::selection_marker().width() + 2 + label_w + 1;
    let inner = row_w.max(title.width() + 4).max(HINT.width() + 4);
    let rows = items
        .len()
        .min(area.height.saturating_sub(4).max(1) as usize);
    app.popup_rows.set(rows);
    let rect = centered(area, inner as u16 + 2, rows as u16 + 2);
    let w = rect.width.saturating_sub(2) as usize;
    let sel = sel.min(items.len().saturating_sub(1));
    let start = list_window(sel, items.len(), rows);
    let lines = items
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, item)| {
            row_line(
                vec![
                    Span::styled(format!("{} ", item.key), theme::active_field()),
                    Span::raw(item.label),
                ],
                i == sel,
                true,
                w,
            )
        })
        .collect();
    let hint = if rows < items.len() {
        format!("{}/{} · {HINT}", sel + 1, items.len())
    } else {
        HINT.to_owned()
    };
    framed(frame, rect, title, &hint, lines);
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App, scroll: u16) {
    let help = app.keymap.help();
    let col = |f: fn(&HelpLine) -> usize| help.iter().map(f).max().unwrap_or(0);
    let key_w = col(|l| match l {
        HelpLine::Row { keys, .. } => keys.width(),
        _ => 0,
    });
    let desc_w = col(|l| match l {
        HelpLine::Row { desc, .. } => desc.width(),
        _ => 0,
    });
    let name_w = col(|l| match l {
        HelpLine::Row { action, .. } => action.map_or(0, |a| a.name().width()),
        _ => 0,
    });
    let full = 2 + key_w + 2 + desc_w + 2 + name_w + 1;
    let width = (full as u16 + 2).min(area.width.saturating_sub(2)).max(1);
    let inner_w = width.saturating_sub(2) as usize;
    let names = inner_w >= full;
    let desc_room = inner_w.saturating_sub(2 + key_w + 2);
    let rows = (area.height.saturating_sub(4).max(1) as usize).min(help.len());
    app.popup_rows.set(rows);
    let skip = (scroll as usize).min(help.len() - rows);
    let lines = help
        .iter()
        .skip(skip)
        .take(rows)
        .map(|line| match line {
            HelpLine::Title(section) => Line::from(Span::styled(
                format!(" {}", section.title()),
                theme::hint_group(),
            )),
            HelpLine::Blank => Line::default(),
            HelpLine::Row { keys, desc, action } => {
                let (keys, style) = if keys.is_empty() {
                    ("-", theme::muted())
                } else {
                    (keys.as_str(), theme::hint_key())
                };
                let keys = format::truncate(keys, key_w);
                let desc = format::truncate(desc, desc_room);
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(format!("{keys:<key_w$}"), style),
                    Span::raw("  "),
                ];
                let desc_style = if action.is_some_and(|a| a.destructive()) {
                    theme::error()
                } else {
                    theme::normal()
                };
                if let Some(a) = action.filter(|_| names) {
                    spans.push(Span::styled(format!("{desc:<desc_w$}  "), desc_style));
                    spans.push(Span::styled(a.name(), theme::muted()));
                } else {
                    spans.push(Span::styled(desc, desc_style));
                }
                Line::from(spans)
            }
        })
        .collect();
    let hint = if rows < help.len() {
        "j/k scroll · ? or Esc close"
    } else {
        "? or Esc close"
    };
    let title = if names {
        "Key bindings · action names for keymap.toml"
    } else {
        "Key bindings"
    };
    framed(
        frame,
        centered(area, width, rows as u16 + 2),
        title,
        hint,
        lines,
    );
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
        "Enter reply · <C-w>/<C-u>/Backspace trim · Esc",
        lines,
    );
    frame.set_cursor_position(Position {
        x: (inner.x + 1 + cur_col as u16).min(inner.right().saturating_sub(1)),
        y: (inner.y + 2 + (cur_row - start) as u16).min(inner.bottom().saturating_sub(1)),
    });
}

fn draw_text_select(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    input: &crate::input::TextInput,
    anchor: Option<usize>,
) {
    let rect = centered(area, 90.min(area.width), 24.min(area.height));
    let width = rect.width.saturating_sub(4).max(1) as usize;
    app.popup_cols.set(width);
    let cursor = input.cursor();
    let selected = anchor.map(|a| (a.min(cursor), a.max(cursor)));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used = 0;
    let mut cursor_row = 0;
    let mut count = 0;
    for (i, ch) in input.text().chars().enumerate() {
        count = i + 1;
        let style = if i == cursor {
            theme::picker_selection()
        } else if selected.is_some_and(|(a, b)| i >= a && i <= b) {
            theme::selection()
        } else {
            theme::normal()
        };
        if i == cursor {
            cursor_row = lines.len();
        }
        if ch == '\n' {
            row.push(Span::styled(" ", style));
            lines.push(Line::from(std::mem::replace(&mut row, vec![Span::raw(" ")])));
            used = 0;
            continue;
        }
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > width {
            lines.push(Line::from(std::mem::replace(&mut row, vec![Span::raw(" ")])));
            used = 0;
            if i == cursor {
                cursor_row = lines.len();
            }
        }
        row.push(Span::styled(ch.to_string(), style));
        used += cw;
    }
    if cursor >= count {
        cursor_row = lines.len();
        row.push(Span::styled(" ", theme::picker_selection()));
    }
    lines.push(Line::from(row));
    let visible = rect.height.saturating_sub(2) as usize;
    let skip = (cursor_row + 1).saturating_sub(visible);
    let hint = if anchor.is_some() {
        "←↑↓→ w b extend · y copy selection · v cancel selection · Esc"
    } else {
        "←↑↓→ w b move · v start selection · y copy all · Esc"
    };
    framed(
        frame,
        rect,
        "Select text",
        hint,
        lines.into_iter().skip(skip).collect(),
    );
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
                Span::styled("Type to search members…", theme::muted())
            } else {
                Span::raw(mv.input.text().to_owned())
            },
        ]),
        Line::from(filters),
    ];
    let rows = (rect.height as usize).saturating_sub(2 + lines.len() + 1);
    app.popup_rows.set(rows);
    match &mv.page {
        None => lines.push(muted(" Loading…")),
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
            lines.push(muted(" Loading…"));
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
        lines.push(muted(" Loading more…"));
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
        (None, None) => lines.push(muted(" Loading profile…")),
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
            if app.settings.appearance.show_ids {
                match d.user_id {
                    Some(u) if u == d.id => lines.push(field("User ID", u.to_string())),
                    Some(u) => {
                        lines.push(field("Chat ID", d.id.to_string()));
                        lines.push(field("User ID", u.to_string()));
                    }
                    None => lines.push(field("Chat ID", d.id.to_string())),
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
    let mut keys = Vec::new();
    if max_scroll > 0 {
        keys.push("j/k scroll");
    }
    if p.data.is_some() && app.open_chat.is_some() {
        keys.push("a their messages in this chat");
    }
    if secret {
        keys.push("S secret chat");
    }
    keys.push("Esc close");
    framed(frame, rect, "Profile", &keys.join(" · "), shown);
}

fn draw_reactions(frame: &mut Frame, area: Rect, app: &App, r: &ReactionPicker) {
    const PER_ROW: usize = ReactionPicker::PER_ROW;
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
        lines.push(muted(" Loading reactions…"));
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
        "arrows move · Enter toggle · Esc",
        lines,
    );
}

fn draw_search(frame: &mut Frame, area: Rect, app: &App, sv: &SearchView) {
    let rect = centered(area, 84, 24);
    let w = rect.width.saturating_sub(2) as usize;
    let listing = sv.listing();
    let mut title = match (sv.sender, sv.chat_id) {
        (Some(s), _) => format!(
            "Messages from {}",
            format::truncate(&app.sender_name(s), 40)
        ),
        (None, _) if listing && sv.media.is_some() => sv.media.map_or("", |m| m.label()).to_owned(),
        (None, _) if listing && sv.pinned => "Pinned messages".to_owned(),
        (None, Some(id)) => format!(
            "Search in {}",
            format::truncate(&app.sender_name(Sender::Chat(id)), 40)
        ),
        (None, None) => "Search all chats".to_owned(),
    };
    if !sv.results.is_empty() {
        let total = sv.total.max(sv.results.len() as i32);
        title.push_str(&format!(" · {}", app.count(i64::from(total))));
    }
    let query = sv.input.text().trim();
    let mut lines: Vec<Line<'static>> = Vec::new();
    if !listing {
        lines.push(Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if sv.input.is_empty() {
                Span::styled("Type to search…", theme::muted())
            } else {
                Span::raw(sv.input.text().to_owned())
            },
        ]));
        if sv.chat_id.is_some() {
            lines.push(media_chips(app, sv, w));
        }
        lines.push(Line::from(Span::styled(
            theme::get().glyphs.separator.repeat(w),
            theme::border(),
        )));
    }
    let rows = (rect.height as usize).saturating_sub(2 + lines.len());
    app.popup_rows.set(rows);
    let date = sv.date();
    let waiting = sv.due.is_some() || (sv.loading && !sv.paging);
    if !listing && query.is_empty() && !sv.filtered() {
        let tip = if sv.chat_id.is_some() {
            " type words to find messages, or a date like 2024-05-01 to go there"
        } else {
            " type words to find messages"
        };
        lines.push(muted(tip));
    } else {
        let sel = sv.sel.min(sv.rows().saturating_sub(1));
        let footer = if waiting {
            Some(muted(" Searching…"))
        } else if let Some(e) = &sv.error {
            Some(Line::from(Span::styled(format!(" {e}"), theme::error())))
        } else if sv.loading && sv.paging {
            Some(muted(" Loading more…"))
        } else if sv.results.is_empty() && sv.searched_for.is_some() {
            Some(muted(if listing {
                " No messages found".to_owned()
            } else {
                format!(" No results for “{query}”")
            }))
        } else {
            None
        };
        let room = rows.saturating_sub(usize::from(footer.is_some()));
        let shown = if waiting { sv.offset() } else { sv.rows() };
        let start = list_window(sel, shown, room);
        for i in (start..shown).take(room) {
            let line = match i.checked_sub(sv.offset()) {
                None => row_line(
                    vec![Span::styled(
                        format!("Go to {}", date.map(|d| d.to_string()).unwrap_or_default()),
                        theme::active_field(),
                    )],
                    i == sel,
                    true,
                    w,
                ),
                Some(r) => {
                    let m = &sv.results[r];
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
                    message_spans(&mut spans, m, query, w);
                    row_line(spans, i == sel, true, w)
                }
            };
            lines.push(line);
        }
        lines.extend(footer);
    }
    let hint = if listing && sv.results.is_empty() {
        "/ search · Esc"
    } else if listing {
        "/ search · j/k select · Enter jump · Esc"
    } else if sv.chat_id.is_some() {
        "Backspace on empty drops a filter · Tab media type · ↑↓ select · Enter jump, then n/N · Esc"
    } else {
        "↑↓ select · Enter jump · Esc"
    };
    let inner = framed(frame, rect, &title, hint, lines);
    if !listing {
        let (_, col) = sv.input.cursor_position(0);
        frame.set_cursor_position(Position {
            x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
            y: inner.y,
        });
    }
}

fn media_chips(app: &App, sv: &SearchView, w: usize) -> Line<'static> {
    let chip = |label: String, on: bool| {
        Span::styled(
            format!(" {label} "),
            if on {
                theme::selection()
            } else {
                theme::muted()
            },
        )
    };
    let mut spans = Vec::new();
    if let Some(s) = sv.sender {
        spans.push(chip(format!("from {}", app.sender_name(s)), true));
    }
    spans.push(chip("All".to_owned(), sv.media.is_none()));
    for m in model::MediaFilter::ALL {
        spans.push(chip(m.label().to_owned(), sv.media == Some(m)));
    }
    let mut used = 0;
    let mut out = Vec::new();
    for span in spans {
        used += span.content.width();
        if used > w {
            break;
        }
        out.push(span);
    }
    Line::from(out)
}

fn hit_style() -> Style {
    theme::active_field().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

fn marked(text: &str, query: &str, width: usize, base: Style) -> Vec<Span<'static>> {
    let mut marks = search::marks(text, query);
    if marks.is_empty()
        && query.chars().count() >= 3
        && let Some(alt) = search::swap_layout(&query.to_lowercase())
    {
        marks = search::marks(text, &alt);
    }
    let start = search::excerpt_start(text, &marks, width);
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled("…", base));
    }
    let mut pos = start;
    for (a, b) in marks {
        if b <= pos {
            continue;
        }
        let a = a.max(pos);
        if a > pos {
            spans.push(Span::styled(text[pos..a].to_owned(), base));
        }
        spans.push(Span::styled(text[a..b].to_owned(), base.patch(hit_style())));
        pos = b;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_owned(), base));
    }
    spans
}

fn message_spans(spans: &mut Vec<Span<'static>>, m: &Msg, query: &str, w: usize) {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let room = w.saturating_sub(used + theme::selection_marker().width());
    spans.extend(marked(&model::preview(m), query, room, Style::default()));
}

fn draw_find(frame: &mut Frame, area: Rect, app: &App, v: &FindView) {
    let rect = centered(area, 84, 26);
    let w = rect.width.saturating_sub(2) as usize;
    let (hits, rows) = app.find_rows();
    let query = v.query();
    let placeholder = match v.scope {
        Scope::All => "Chats, people, channels, messages…",
        Scope::Messages => "Messages in all chats…",
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", theme::active_field()),
            if v.input.is_empty() {
                Span::styled(placeholder, theme::muted())
            } else {
                Span::raw(v.input.text().to_owned())
            },
        ]),
        Line::from(Span::styled(
            theme::get().glyphs.separator.repeat(w),
            theme::border(),
        )),
    ];
    let room = (rect.height as usize).saturating_sub(2 + lines.len());
    app.popup_rows.set(room);
    let sel = v.sel.min(hits.len().saturating_sub(1));
    let start = list_window(search::row_of(&rows, sel), rows.len(), room);
    for row in rows.iter().skip(start).take(room) {
        lines.push(match row {
            Row::Header(section, extra) => Line::from(vec![
                Span::styled(format!(" {}", section.title()), theme::hint_group()),
                Span::styled(
                    if extra.is_empty() {
                        String::new()
                    } else {
                        format!(" · {extra}")
                    },
                    theme::muted(),
                ),
            ]),
            Row::Note(text, error) => {
                let text = match app.connection {
                    Some(net) if !*error && text == "Searching…" => {
                        format!(" Searching… {net}")
                    }
                    _ => format!(" {text}"),
                };
                let style = if *error {
                    theme::error()
                } else {
                    theme::muted()
                };
                Line::from(Span::styled(text, style))
            }
            Row::Hit(i, hit) => row_line(find_spans(app, v, *hit, query, w), *i == sel, true, w),
        });
    }
    let joinable = matches!(hits.get(sel), Some(&(_, Hit::Chat(id))) if !app.is_member(id));
    let hint = match (v.scope, joinable) {
        (Scope::All, true) => "Tab next section · ↑↓ select · Enter preview · Alt-Enter join · Esc",
        (Scope::All, false) => "Tab next section · ↑↓ select · Enter open · Esc",
        (Scope::Messages, _) => "↑↓ select · Enter open · Esc",
    };
    let title = match v.scope {
        Scope::All => "Search",
        Scope::Messages => "Search messages",
    };
    let inner = framed(frame, rect, title, hint, lines);
    let (_, col) = v.input.cursor_position(0);
    frame.set_cursor_position(Position {
        x: (inner.x + 2 + col as u16).min(inner.right().saturating_sub(1)),
        y: inner.y,
    });
}

fn find_spans(app: &App, v: &FindView, hit: Hit, query: &str, w: usize) -> Vec<Span<'static>> {
    let half = w / 2;
    match hit {
        Hit::Chat(id) => {
            let Some(c) = app.chats.get(&id) else {
                return vec![Span::raw(format!("chat {id}"))];
            };
            let mut spans = marked(&c.title, query, half, Style::default());
            if let Some(name) = app.chat_usernames(id).first() {
                spans.push(Span::styled(format!("  @{name}"), theme::muted()));
            }
            let mut kind = match c.kind {
                ChatKind::Private(u) if app.user_meta.get(&u).is_some_and(|m| m.bot) => "bot",
                ChatKind::Private(_) => "",
                ChatKind::Secret(_) => "secret chat",
                ChatKind::Group | ChatKind::Supergroup => "group",
                ChatKind::Channel => "channel",
            }
            .to_owned();
            if !app.is_member(id) {
                kind.push_str(", not joined");
            }
            if !kind.is_empty() {
                spans.push(Span::styled(format!(" · {kind}"), theme::muted()));
            }
            let unread = app.chat_unread(id);
            if unread > 0 {
                spans.push(Span::styled(
                    format!("  {}", app.count(i64::from(unread))),
                    theme::muted(),
                ));
            }
            spans
        }
        Hit::Contact(user) => {
            let name = app.sender_name(Sender::User(user));
            let mut spans = marked(&name, query, half, Style::default());
            if let Some(u) = app.user_meta.get(&user).and_then(|m| m.usernames.first()) {
                spans.push(Span::styled(format!("  @{u}"), theme::muted()));
            }
            spans.push(Span::styled(" · contact", theme::muted()));
            spans
        }
        Hit::Message(i) => {
            let Some(m) = v.msgs.get(i) else {
                return Vec::new();
            };
            let mut spans = vec![
                Span::styled(
                    format!(
                        "{} {} ",
                        format::date_label(m.date),
                        format::time_label(m.date)
                    ),
                    theme::muted(),
                ),
                Span::styled(
                    format!(
                        "{} · ",
                        format::truncate(&app.sender_name(Sender::Chat(m.chat_id)), 24)
                    ),
                    theme::mention(),
                ),
                Span::styled(format!("{}: ", app.sender_name(m.sender)), theme::strong()),
            ];
            message_spans(&mut spans, m, query, w);
            spans
        }
    }
}
