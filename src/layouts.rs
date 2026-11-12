use super::*;

struct RunPos<'a> {
    prev: Option<&'a Msg>,
    new_day: bool,
    first: bool,
    last: bool,
    group_chat: bool,
}

fn run_pos<'a>(
    app: &App,
    msgs: &'a [Msg],
    range: &std::ops::Range<usize>,
    split: Option<Ctx>,
) -> RunPos<'a> {
    let m = &msgs[range.start];
    let last = &msgs[range.end - 1];
    let prev = range.start.checked_sub(1).map(|j| &msgs[j]);
    let next = msgs.get(range.end);
    let new_day = prev.is_none_or(|p| !format::same_day(p.date, m.date));
    let sel = split.and_then(|c| c.sel);
    let is_sel = |i: usize| sel.is_some_and(|(s, e)| (s..e).contains(&i));
    let breaks = |a: usize, b: usize| is_sel(a) != is_sel(b) || is_sel(a);
    RunPos {
        prev,
        new_day,
        first: new_day
            || prev.is_none_or(|p| !same_run(p, m))
            || (range.start > 0 && breaks(range.start - 1, range.start)),
        last: next.is_none_or(|n| !same_run(last, n)) || breaks(range.end - 1, range.end),
        group_chat: app
            .chats
            .get(&m.chat_id)
            .is_some_and(|c| matches!(c.kind, ChatKind::Group | ChatKind::Supergroup)),
    }
}

fn lead(ctx: Ctx, pos: &RunPos, date: i64, airy: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if pos.new_day {
        if pos.prev.is_some() && airy {
            lines.push(Line::default());
        }
        let sep = day_separator(ctx, date);
        let has_sep = !sep.is_empty();
        lines.extend(sep);
        if has_sep && airy {
            lines.push(Line::default());
        }
    } else if pos.first && airy {
        lines.push(Line::default());
    }
    lines
}

fn time_width() -> usize {
    let l = &theme::get().layout;
    match (l.clock, l.seconds) {
        (theme::Clock::H24, false) => 5,
        (theme::Clock::H24, true) => 8,
        (theme::Clock::H12, false) => 8,
        (theme::Clock::H12, true) => 11,
    }
}

fn flags(group: &[Msg]) -> (bool, bool) {
    (
        group.iter().any(|g| g.edited),
        group.iter().any(|g| g.pinned),
    )
}

pub(super) fn marks(
    edited: bool,
    pinned: bool,
    tick: Option<Span<'static>>,
    lang: format::Lang,
) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut push = |s: Span<'static>| {
        if !out.is_empty() {
            out.push(Span::raw(" "));
        }
        out.push(s);
    };
    if edited {
        push(edited_span(lang));
    }
    if pinned {
        push(pin_span());
    }
    if let Some(t) = tick {
        push(t);
    }
    out
}

pub(super) fn media_end(images: &[Place]) -> usize {
    images
        .iter()
        .map(|p| p.line + p.rows as usize)
        .max()
        .unwrap_or(0)
}

pub(super) fn attach_right(
    body: &mut Vec<Line<'static>>,
    from: usize,
    tail: Vec<Span<'static>>,
    width: usize,
) {
    if tail.is_empty() {
        return;
    }
    let tail_w = spans_width(&tail);
    let has_text = body.len() > from;
    match body.last_mut() {
        Some(l) if has_text && spans_width(&l.spans) + 1 + tail_w <= width => {
            let pad = width - spans_width(&l.spans) - tail_w;
            l.spans.push(Span::raw(" ".repeat(pad)));
            l.spans.extend(tail);
        }
        _ => {
            let tail = truncate_spans(tail, width);
            let pad = width.saturating_sub(spans_width(&tail));
            let mut spans = vec![Span::raw(" ".repeat(pad))];
            spans.extend(tail);
            body.push(Line::from(spans));
        }
    }
}

fn selected_if(line: Line<'static>, selected: bool, width: usize) -> Line<'static> {
    if selected {
        select_row(line, width)
    } else {
        line
    }
}

fn fit(line: Line<'static>, width: usize) -> Line<'static> {
    if spans_width(&line.spans) <= width {
        line
    } else {
        Line::from(truncate_spans(line.spans, width))
    }
}

pub(super) fn compact_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let pos = run_pos(app, msgs, &range, None);
    let m = &msgs[range.start];
    let last = &msgs[range.end - 1];
    let th = theme::get();
    let tw = time_width();
    let nw = (th.layout.compact_name_width as usize)
        .min(width / 4)
        .max(3);
    let sep = th.glyphs.compact_separator.clone();
    let prefix_w = 1 + tw + 1 + nw + 1 + sep.width() + 1;
    let text_w = width.saturating_sub(prefix_w + 1).max(4);
    let mut lines = lead(ctx, &pos, m.date, false);
    let mut body: Vec<Line<'static>> = Vec::new();
    body.extend(forward_line(app, m, ctx.lang, text_w));
    body.extend(reply_strip(app, m, ctx.lang, text_w));
    let mut images = Vec::new();
    let mut texts = group_content(
        app,
        msgs,
        range.clone(),
        text_w,
        true,
        &mut body,
        &mut images,
    );
    if body.is_empty() {
        body.push(Line::default());
    }
    let (edited, pinned) = flags(&msgs[range.clone()]);
    let from = media_end(&images);
    attach_right(
        &mut body,
        from,
        marks(edited, pinned, tick_span(app, last), ctx.lang),
        text_w,
    );
    let time_style = theme::time();
    let sep_style = theme::border();
    let name_style = if selected {
        theme::active_field()
    } else {
        author_style(app, m.sender)
    };
    let top = lines.len();
    for (i, l) in body.into_iter().enumerate() {
        let mut spans = vec![Span::raw(" ")];
        if i == 0 {
            let t = format::time_label(m.date);
            spans.push(Span::styled(
                format!("{t:>tw$}", tw = tw.max(t.width())),
                time_style,
            ));
        } else {
            spans.push(Span::raw(" ".repeat(tw)));
        }
        spans.push(Span::raw(" "));
        if i == 0 && pos.first {
            let name = format::truncate(&app.sender_name(m.sender), nw);
            spans.push(Span::raw(" ".repeat(nw.saturating_sub(name.width()))));
            spans.push(Span::styled(name, name_style));
        } else {
            spans.push(Span::raw(" ".repeat(nw)));
        }
        spans.push(Span::styled(format!(" {sep} "), sep_style));
        spans.extend(fit(l, text_w).spans);
        lines.push(selected_if(Line::from(spans), selected, width));
    }
    for p in &mut images {
        p.line += top;
        p.col += prefix_w as u16;
    }
    for t in &mut texts {
        t.line += top;
        t.col += prefix_w as u16;
    }
    Rendered {
        lines,
        images,
        texts,
    }
}

pub(super) fn minimal_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let pos = run_pos(app, msgs, &range, None);
    let m = &msgs[range.start];
    let last = &msgs[range.end - 1];
    let th = theme::get();
    let tw = time_width();
    let left = gutter().max(1) + 2;
    let right = tw + 6;
    let text_w = width.saturating_sub(left + right).max(8);
    let mut lines = lead(ctx, &pos, m.date, true);
    let mut body: Vec<Line<'static>> = Vec::new();
    if pos.first {
        body.push(Line::from(Span::styled(
            format::truncate(&app.sender_name(m.sender), text_w),
            if selected {
                theme::active_field()
            } else {
                author_style(app, m.sender)
            },
        )));
    }
    if let Some(origin) = &m.forward {
        let text = format!("{} {}", th.glyphs.forwarded, origin_name(app, origin));
        body.push(Line::from(Span::styled(
            format::truncate(text.trim_start(), text_w),
            theme::forwarded(),
        )));
    }
    if let Some((sender, name, text)) = reply_parts(app, m) {
        let bar = format!("{} ", th.glyphs.reply_bar);
        let room = text_w.saturating_sub(bar.width());
        let label = match (name.is_empty(), text.is_empty()) {
            (true, _) => match ctx.lang {
                format::Lang::Ru => "ответ".to_owned(),
                format::Lang::En => "reply".to_owned(),
            },
            (false, true) => name,
            (false, false) => format!("{name}: {text}"),
        };
        body.push(Line::from(vec![
            Span::styled(bar, theme::reply_bar(sender.map(sender_id))),
            Span::styled(format::truncate(&label, room), theme::reply_text()),
        ]));
    }
    let mut images = Vec::new();
    let before = body.len();
    let mut texts = group_content(
        app,
        msgs,
        range.clone(),
        text_w,
        true,
        &mut body,
        &mut images,
    );
    if body.is_empty() {
        body.push(Line::default());
    }
    let (edited, pinned) = flags(&msgs[range.clone()]);
    let tail = marks(edited, pinned, None, ctx.lang);
    if !tail.is_empty() {
        let from = before.max(media_end(&images));
        let tail_w = spans_width(&tail);
        let has_text = body.len() > from;
        match body.last_mut() {
            Some(l) if has_text && spans_width(&l.spans) + 1 + tail_w <= text_w => {
                l.spans.push(Span::raw(" "));
                l.spans.extend(tail);
            }
            _ => body.push(Line::from(truncate_spans(tail, text_w))),
        }
    }
    let mut meta = vec![Span::styled(
        format!("{:>tw$}", format::time_label(m.date)),
        theme::time(),
    )];
    if let Some(t) = tick_span(app, last) {
        meta.push(Span::raw(" "));
        meta.push(t);
    }
    let top = lines.len();
    for (i, l) in body.into_iter().enumerate() {
        let mut spans = vec![Span::raw(" ".repeat(left))];
        let l = fit(l, text_w);
        let used = spans_width(&l.spans);
        spans.extend(l.spans);
        if i == 0 {
            spans.push(Span::raw(" ".repeat(text_w - used + 3)));
            spans.extend(truncate_spans(meta.clone(), right - 3));
        }
        lines.push(selected_if(Line::from(spans), selected, width));
    }
    for p in &mut images {
        p.line += top;
        p.col += left as u16;
    }
    for t in &mut texts {
        t.line += top;
        t.col += left as u16;
    }
    Rendered {
        lines,
        images,
        texts,
    }
}

fn card_footer(
    app: &App,
    last: &Msg,
    (edited, pinned): (bool, bool),
    lang: format::Lang,
) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    if pinned {
        parts.push(pin_span());
    }
    if edited {
        parts.push(edited_span(lang));
    }
    parts.push(Span::styled(format::time_label(last.date), theme::time()));
    parts.extend(tick_span(app, last));
    let mut out = Vec::with_capacity(parts.len() * 2);
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.push(Span::raw(" "));
        }
        out.push(p);
    }
    out
}

struct CardPart {
    first: bool,
    last: bool,
    body: Vec<Line<'static>>,
    images: Vec<Place>,
    texts: Vec<TextPlace>,
    title: Option<String>,
}

fn card_part(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    inner_max: usize,
) -> CardPart {
    let pos = run_pos(app, msgs, &range, None);
    let m = &msgs[range.start];
    let mut body: Vec<Line<'static>> = Vec::new();
    body.extend(forward_line(app, m, ctx.lang, inner_max));
    body.extend(reply_strip(app, m, ctx.lang, inner_max));
    let mut images = Vec::new();
    let texts = group_content(app, msgs, range, inner_max, true, &mut body, &mut images);
    if body.is_empty() {
        body.push(Line::default());
    }
    let title = (!m.outgoing && pos.first).then(|| app.sender_name(m.sender));
    CardPart {
        first: pos.first,
        last: pos.last,
        body,
        images,
        texts,
        title,
    }
}

fn card_need(part: &CardPart) -> usize {
    let image_w = part
        .images
        .iter()
        .map(|p| (p.col + p.cols) as usize)
        .max()
        .unwrap_or(0);
    let body_w = part
        .body
        .iter()
        .map(|l| spans_width(&l.spans))
        .max()
        .unwrap_or(0);
    let title_w = part.title.as_ref().map_or(0, |t| t.width());
    body_w.max(image_w).max(title_w)
}

pub(super) fn card_run(msgs: &[Msg], range: &std::ops::Range<usize>) -> std::ops::Range<usize> {
    let mut start = range.start;
    while start > 0 && same_run(&msgs[start - 1], &msgs[start]) {
        start -= 1;
    }
    let mut end = range.end;
    while end < msgs.len() && same_run(&msgs[end - 1], &msgs[end]) {
        end += 1;
    }
    start..end
}

fn border_line(
    b: &ratatui::symbols::border::Set<'static>,
    top: bool,
    outer: usize,
    label: Vec<Span<'static>>,
    style: Style,
) -> Vec<Span<'static>> {
    let (l, h, r) = if top {
        (b.top_left, b.horizontal_top, b.top_right)
    } else {
        (b.bottom_left, b.horizontal_bottom, b.bottom_right)
    };
    let inner = outer.saturating_sub(2);
    let label_w = spans_width(&label);
    if label.is_empty() || label_w + 2 > inner {
        return vec![Span::styled(format!("{l}{}{r}", h.repeat(inner)), style)];
    }
    let rest = inner - label_w;
    let (before, after) = if top { (1, rest - 1) } else { (rest - 1, 1) };
    let mut out = vec![Span::styled(format!("{l}{}", h.repeat(before)), style)];
    out.extend(label);
    out.push(Span::styled(format!("{}{r}", h.repeat(after)), style));
    out
}

pub(super) fn cards_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let pos = run_pos(app, msgs, &range, None);
    let m = &msgs[range.start];
    let th = theme::get();
    let outgoing = m.outgoing;
    let avatar_col = pos.group_chat && !outgoing && th.layout.avatars;
    let gutter = if avatar_col {
        CLASSIC_AVATAR_COLS as usize + 1
    } else {
        1
    };
    let room = width.saturating_sub(gutter + 1).max(8);
    let outer_max = (width * th.layout.cards_max_width as usize / 100)
        .max(28)
        .min(room);
    let inner_max = outer_max.saturating_sub(4).max(2);
    let part = card_part(app, msgs, range.clone(), ctx, inner_max);
    let run = card_run(msgs, &range);
    let run_last = &msgs[run.end - 1];
    let footer = card_footer(app, run_last, flags(&msgs[run.clone()]), ctx.lang);
    let mut need = card_need(&part).max(spans_width(&footer));
    for g in model::groups(&msgs[run.clone()]) {
        let g = (g.start + run.start)..(g.end + run.start);
        if g != range {
            need = need.max(card_need(&card_part(app, msgs, g, ctx, inner_max)));
        }
    }
    let inner_w = need.clamp(1, inner_max);
    let CardPart {
        first,
        last: is_last,
        body,
        mut images,
        mut texts,
        title,
    } = part;
    let outer = inner_w + 4;
    let x = if outgoing {
        width.saturating_sub(outer + 1)
    } else {
        gutter
    };
    let border = if selected {
        theme::selection_border()
    } else {
        theme::card_border()
    };
    let fill = if selected {
        theme::selection_bg().or(theme::card_fill())
    } else {
        theme::card_fill()
    };
    let b = theme::border_set();
    let indent = || Span::raw(" ".repeat(x));
    let mut lines = lead(ctx, &pos, m.date, true);
    let top = lines.len();
    if first {
        let label = title
            .map(|t| {
                vec![Span::styled(
                    format::truncate(&t, outer.saturating_sub(4)),
                    author_style(app, m.sender),
                )]
            })
            .unwrap_or_default();
        let mut spans = vec![indent()];
        spans.extend(border_line(&b, true, outer, label, border));
        lines.push(Line::from(spans));
    }
    let body_top = lines.len();
    let bar = format::truncate(&th.glyphs.selection_bar, 1);
    for l in body {
        let l = fit(l, inner_w);
        let pad = inner_w - spans_width(&l.spans);
        let mut inner = vec![if selected {
            Span::styled(bar.clone(), theme::selection_bar())
        } else {
            Span::raw(" ")
        }];
        inner.extend(l.spans);
        inner.push(Span::raw(" ".repeat(pad + 1)));
        let mut spans = vec![indent(), Span::styled(b.vertical_left.to_owned(), border)];
        spans.extend(with_bg(Line::from(inner), fill).spans);
        spans.push(Span::styled(b.vertical_right.to_owned(), border));
        lines.push(Line::from(spans));
    }
    if is_last {
        let mut tail = footer;
        if spans_width(&tail) + 2 > outer - 2 {
            tail = card_footer(app, run_last, (false, false), ctx.lang);
        }
        let mut spans = vec![indent()];
        spans.extend(border_line(&b, false, outer, tail, border));
        lines.push(Line::from(spans));
    }
    for p in &mut images {
        p.line += body_top;
        p.col += (x + 2) as u16;
    }
    for t in &mut texts {
        t.width = t.width.min((inner_w as u16).saturating_sub(t.col));
        t.line += body_top;
        t.col += (x + 2) as u16;
    }
    if avatar_col && first {
        lines[top].spans[0] = Span::raw(" ".repeat(x - CLASSIC_AVATAR_COLS as usize));
        lines[top].spans.insert(0, classic_avatar(app, m.sender));
    }
    Rendered {
        lines,
        images,
        texts,
    }
}

pub(super) fn split_block(
    app: &App,
    msgs: &[Msg],
    range: std::ops::Range<usize>,
    ctx: Ctx,
    selected: bool,
) -> Rendered {
    let width = ctx.width;
    let pos = run_pos(app, msgs, &range, None);
    let m = &msgs[range.start];
    let last = &msgs[range.end - 1];
    let th = theme::get();
    let outgoing = m.outgoing;
    let margin = gutter().max(1);
    let room = width.saturating_sub(2 * margin).max(8);
    let max_w = (width * th.layout.split_max_width as usize / 100)
        .max(20)
        .min(room);
    let (edited, pinned) = flags(&msgs[range.clone()]);
    let tick = tick_span(app, last);
    let mut body: Vec<Line<'static>> = Vec::new();
    if pos.first {
        let mut meta = vec![Span::styled(format::time_label(m.date), theme::time())];
        if let Some(t) = tick.clone() {
            meta.push(Span::raw(" "));
            meta.push(t);
        }
        if edited {
            meta.push(Span::styled(" · ", theme::muted()));
            meta.push(edited_span(ctx.lang));
        }
        if pinned {
            meta.push(Span::raw(" "));
            meta.push(pin_span());
        }
        let name_room = max_w.saturating_sub(spans_width(&meta) + 1).max(1);
        let mut spans = vec![
            Span::styled(
                format::truncate(&app.sender_name(m.sender), name_room),
                if selected {
                    theme::active_field()
                } else {
                    author_style(app, m.sender)
                },
            ),
            Span::raw(" "),
        ];
        spans.extend(meta);
        body.push(Line::from(spans));
    }
    body.extend(forward_line(app, m, ctx.lang, max_w));
    body.extend(reply_strip(app, m, ctx.lang, max_w));
    let before = body.len();
    let mut images = Vec::new();
    let mut texts = group_content(
        app,
        msgs,
        range.clone(),
        max_w,
        true,
        &mut body,
        &mut images,
    );
    if body.is_empty() {
        body.push(Line::default());
    }
    if !pos.first {
        let tail = marks(edited, pinned, tick, ctx.lang);
        let has_text = body.len() > before.max(media_end(&images));
        let tail_w = spans_width(&tail);
        if !tail.is_empty() {
            match body.last_mut() {
                Some(l) if has_text && spans_width(&l.spans) + 2 + tail_w <= max_w => {
                    l.spans.push(Span::raw("  "));
                    l.spans.extend(tail);
                }
                _ => body.push(Line::from(truncate_spans(tail, max_w))),
            }
        }
    }
    let image_w = images
        .iter()
        .map(|p| (p.col + p.cols) as usize)
        .max()
        .unwrap_or(0);
    let right = width.saturating_sub(margin);
    let mut lines = lead(ctx, &pos, m.date, true);
    let top = lines.len();
    let mut shifts = Vec::with_capacity(body.len());
    for l in body {
        let l = fit(l, max_w);
        let shift = if outgoing {
            right.saturating_sub(spans_width(&l.spans))
        } else {
            margin
        };
        shifts.push(shift);
        let mut spans = vec![Span::raw(" ".repeat(shift))];
        spans.extend(l.spans);
        lines.push(selected_if(Line::from(spans), selected, width));
    }
    for t in &mut texts {
        t.col += shifts[t.line] as u16;
        t.line += top;
    }
    let image_x = if outgoing {
        right.saturating_sub(image_w.min(max_w))
    } else {
        margin
    };
    for p in &mut images {
        p.line += top;
        p.col += image_x as u16;
    }
    Rendered {
        lines,
        images,
        texts,
    }
}
