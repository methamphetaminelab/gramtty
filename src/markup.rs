use std::cmp::Reverse;
use std::ops::Range;

use crate::model::{Entity, EntityKind, RichText};

const MARKER_CHARS: &[char] = &['\\', '*', '_', '~', '|', '`', '[', ']', '<', '>'];
const MENTION_PREFIX: &str = "tg://user?id=";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub rich: RichText,
    pub markers: Vec<Range<usize>>,
    pub escapes: Vec<Range<usize>>,
    pub styled: Vec<(Range<usize>, EntityKind)>,
}

type Src = (char, usize);

#[derive(Default)]
struct Builder {
    out: String,
    pos16: usize,
    entities: Vec<Entity>,
    markers: Vec<Range<usize>>,
    escapes: Vec<Range<usize>>,
    styled: Vec<(Range<usize>, EntityKind)>,
}

fn is_marker_char(c: char) -> bool {
    MARKER_CHARS.contains(&c)
}

fn rank(kind: &EntityKind) -> u8 {
    match kind {
        EntityKind::Quote => 0,
        EntityKind::Pre(_) => 1,
        EntityKind::Bold => 2,
        EntityKind::Italic => 3,
        EntityKind::Underline => 4,
        EntityKind::Strike => 5,
        EntityKind::Spoiler => 6,
        EntityKind::Link(_) => 7,
        EntityKind::MentionName(_) => 8,
        EntityKind::Code => 9,
        EntityKind::Mention => 10,
    }
}

fn double_kind(c: char) -> Option<EntityKind> {
    match c {
        '*' => Some(EntityKind::Bold),
        '_' => Some(EntityKind::Italic),
        '~' => Some(EntityKind::Strike),
        '|' => Some(EntityKind::Spoiler),
        _ => None,
    }
}

fn mention_id(url: &str) -> Option<i64> {
    url.strip_prefix(MENTION_PREFIX)?.parse().ok()
}

fn is_quote_line(chars: &[char], line: (usize, usize)) -> bool {
    chars[line.0..line.1].starts_with(&['>', ' '])
}

fn fence_at(chars: &[char], lines: &[(usize, usize)], i: usize) -> Option<(String, usize)> {
    let (s, e) = lines[i];
    let line = &chars[s..e];
    let rest = line.strip_prefix(&['`', '`', '`'])?;
    if rest.iter().any(|c| c.is_whitespace() || *c == '`') {
        return None;
    }
    let close = (i + 1..lines.len()).find(|&k| {
        let l = &chars[lines[k].0..lines[k].1];
        let end = l
            .iter()
            .rposition(|c| !c.is_whitespace())
            .map_or(0, |p| p + 1);
        l[..end] == ['`', '`', '`']
    })?;
    Some((rest.iter().collect(), close))
}

impl Builder {
    fn push(&mut self, c: char) {
        self.out.push(c);
        self.pos16 += c.len_utf16();
    }

    fn entity(&mut self, start: usize, kind: EntityKind) {
        if self.pos16 > start {
            self.entities.push(Entity {
                offset: start,
                length: self.pos16 - start,
                kind,
            });
        }
    }

    fn marker(&mut self, seg: &[Src], from: usize, to: usize) {
        self.markers.push(seg[from].1..seg[to - 1].1 + 1);
    }

    fn inline(&mut self, seg: &[Src], lo: usize, hi: usize) {
        let mut i = lo;
        while i < hi {
            let c = seg[i].0;
            if c == '\\' && i + 1 < hi && is_marker_char(seg[i + 1].0) {
                self.escapes.push(seg[i].1..seg[i].1 + 1);
                self.push(seg[i + 1].0);
                i += 2;
                continue;
            }
            if let Some(next) = self.try_marker(seg, i, hi) {
                i = next;
                continue;
            }
            self.push(c);
            i += 1;
        }
    }

    fn try_marker(&mut self, seg: &[Src], i: usize, hi: usize) -> Option<usize> {
        let c = seg[i].0;
        match c {
            '*' | '_' | '~' | '|' => self.try_double(seg, i, hi),
            '`' => self.try_code(seg, i, hi),
            '[' => self.try_link(seg, i, hi),
            '<' => self.try_underline(seg, i, hi),
            _ => None,
        }
    }

    fn try_double(&mut self, seg: &[Src], i: usize, hi: usize) -> Option<usize> {
        let c = seg[i].0;
        if i + 2 >= hi || seg[i + 1].0 != c || seg[i + 2].0.is_whitespace() {
            return None;
        }
        if c == '_' && i > 0 && seg[i - 1].0.is_alphanumeric() {
            return None;
        }
        let mut j = i + 3;
        while j + 1 < hi {
            let d = seg[j].0;
            if d == '\\' && is_marker_char(seg[j + 1].0) {
                j += 2;
                continue;
            }
            if d == c
                && seg[j + 1].0 == c
                && !seg[j - 1].0.is_whitespace()
                && (c != '_' || j + 2 >= hi || !seg[j + 2].0.is_alphanumeric())
            {
                let kind = double_kind(c)?;
                self.marker(seg, i, i + 2);
                let start = self.pos16;
                self.inline(seg, i + 2, j);
                self.marker(seg, j, j + 2);
                self.styled
                    .push((seg[i + 2].1..seg[j - 1].1 + 1, kind.clone()));
                self.entity(start, kind);
                return Some(j + 2);
            }
            j += 1;
        }
        None
    }

    fn try_code(&mut self, seg: &[Src], i: usize, hi: usize) -> Option<usize> {
        if i + 1 >= hi || seg[i + 1].0 == '`' || seg[i + 1].0.is_whitespace() {
            return None;
        }
        let mut j = i + 1;
        while j < hi && seg[j].0 != '`' {
            if seg[j].0 == '\n' {
                return None;
            }
            j += 1;
        }
        if j >= hi || seg[j - 1].0.is_whitespace() {
            return None;
        }
        self.marker(seg, i, i + 1);
        let start = self.pos16;
        for s in &seg[i + 1..j] {
            self.push(s.0);
        }
        self.marker(seg, j, j + 1);
        self.styled
            .push((seg[i + 1].1..seg[j - 1].1 + 1, EntityKind::Code));
        self.entity(start, EntityKind::Code);
        Some(j + 1)
    }

    fn try_underline(&mut self, seg: &[Src], i: usize, hi: usize) -> Option<usize> {
        let open = ['<', 'u', '>'];
        let close = ['<', '/', 'u', '>'];
        let matches = |at: usize, pat: &[char]| {
            at + pat.len() <= hi
                && seg[at..at + pat.len()]
                    .iter()
                    .map(|s| s.0)
                    .eq(pat.iter().copied())
        };
        if !matches(i, &open) || i + 3 >= hi || seg[i + 3].0.is_whitespace() {
            return None;
        }
        let mut j = i + 4;
        while j < hi {
            if seg[j].0 == '\\' && j + 1 < hi && is_marker_char(seg[j + 1].0) {
                j += 2;
                continue;
            }
            if matches(j, &close) && !seg[j - 1].0.is_whitespace() {
                self.marker(seg, i, i + 3);
                let start = self.pos16;
                self.inline(seg, i + 3, j);
                self.marker(seg, j, j + 4);
                self.styled
                    .push((seg[i + 3].1..seg[j - 1].1 + 1, EntityKind::Underline));
                self.entity(start, EntityKind::Underline);
                return Some(j + 4);
            }
            j += 1;
        }
        None
    }

    fn try_link(&mut self, seg: &[Src], i: usize, hi: usize) -> Option<usize> {
        let mut j = i + 1;
        let text_end = loop {
            if j >= hi || seg[j].0 == '\n' {
                return None;
            }
            if seg[j].0 == '\\' && j + 1 < hi && is_marker_char(seg[j + 1].0) {
                j += 2;
                continue;
            }
            if seg[j].0 == ']' && j + 1 < hi && seg[j + 1].0 == '(' {
                break j;
            }
            j += 1;
        };
        if text_end == i + 1 {
            return None;
        }
        let mut k = text_end + 2;
        let mut depth = 1;
        let mut url = String::new();
        let url_end = loop {
            if k >= hi || seg[k].0.is_whitespace() {
                return None;
            }
            match seg[k].0 {
                '\\' if k + 1 < hi && matches!(seg[k + 1].0, '\\' | '(' | ')') => {
                    url.push(seg[k + 1].0);
                    k += 2;
                    continue;
                }
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break k;
                    }
                }
                _ => {}
            }
            url.push(seg[k].0);
            k += 1;
        };
        if url.is_empty() {
            return None;
        }
        let kind = match mention_id(&url) {
            Some(id) => EntityKind::MentionName(id),
            None => EntityKind::Link(Some(url)),
        };
        self.marker(seg, i, i + 1);
        let start = self.pos16;
        self.inline(seg, i + 1, text_end);
        self.marker(seg, text_end, url_end + 1);
        self.styled
            .push((seg[i + 1].1..seg[text_end - 1].1 + 1, kind.clone()));
        self.entity(start, kind);
        Some(url_end + 1)
    }
}

pub fn parse(source: &str) -> Parsed {
    let chars: Vec<char> = source.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    lines.push((start, chars.len()));

    let mut b = Builder::default();
    let mut emitted = false;
    let mut i = 0;
    while i < lines.len() {
        if let Some((lang, close)) = fence_at(&chars, &lines, i) {
            b.markers.push(lines[i].0..lines[i].1);
            b.markers.push(lines[close].0..lines[close].1);
            if close > i + 1 {
                if emitted {
                    b.push('\n');
                }
                emitted = true;
                let (first, last) = (lines[i + 1].0, lines[close - 1].1);
                let begin = b.pos16;
                let kind = EntityKind::Pre((!lang.is_empty()).then_some(lang));
                for c in &chars[first..last] {
                    b.push(*c);
                }
                if last > first {
                    b.styled.push((first..last, kind.clone()));
                }
                b.entity(begin, kind);
            }
            i = close + 1;
            continue;
        }
        let quoted = is_quote_line(&chars, lines[i]);
        let mut j = i;
        while j < lines.len()
            && if quoted {
                is_quote_line(&chars, lines[j])
            } else {
                !is_quote_line(&chars, lines[j]) && fence_at(&chars, &lines, j).is_none()
            }
        {
            j += 1;
        }
        let mut seg: Vec<Src> = Vec::new();
        for (n, line) in lines[i..j].iter().enumerate() {
            if n > 0 {
                seg.push(('\n', lines[i + n - 1].1));
            }
            let from = if quoted {
                b.markers.push(line.0..line.0 + 2);
                line.0 + 2
            } else {
                line.0
            };
            seg.extend((from..line.1).map(|p| (chars[p], p)));
        }
        if emitted {
            b.push('\n');
        }
        emitted = true;
        let begin = b.pos16;
        b.inline(&seg, 0, seg.len());
        if quoted {
            if let (Some(first), Some(last)) = (seg.first(), seg.last()) {
                b.styled.push((first.1..last.1 + 1, EntityKind::Quote));
            }
            b.entity(begin, EntityKind::Quote);
        }
        i = j;
    }

    b.entities
        .sort_by_key(|e| (e.offset, Reverse(e.length), rank(&e.kind)));
    b.markers.sort_by_key(|r| r.start);
    b.escapes.sort_by_key(|r| r.start);
    Parsed {
        rich: RichText {
            text: b.out,
            entities: b.entities,
        },
        markers: b.markers,
        escapes: b.escapes,
        styled: b.styled,
    }
}

#[derive(Clone, Debug)]
enum Tok {
    Text(char),
    Raw(char),
    Mark(String),
}

impl Tok {
    fn first(&self) -> Option<char> {
        match self {
            Tok::Text(c) | Tok::Raw(c) => Some(*c),
            Tok::Mark(s) => s.chars().next(),
        }
    }

    fn last(&self) -> Option<char> {
        match self {
            Tok::Text(c) | Tok::Raw(c) => Some(*c),
            Tok::Mark(s) => s.chars().last(),
        }
    }

    fn is_newline(&self) -> bool {
        matches!(self, Tok::Text('\n') | Tok::Raw('\n'))
    }
}

#[derive(Clone)]
struct Span {
    start: usize,
    end: usize,
    kind: EntityKind,
}

fn open_mark(kind: &EntityKind) -> &'static str {
    match kind {
        EntityKind::Bold => "**",
        EntityKind::Italic => "__",
        EntityKind::Strike => "~~",
        EntityKind::Spoiler => "||",
        EntityKind::Underline => "<u>",
        EntityKind::Code => "`",
        _ => "[",
    }
}

fn close_mark(kind: &EntityKind) -> String {
    match kind {
        EntityKind::Underline => "</u>".to_owned(),
        EntityKind::Link(Some(url)) => {
            let mut s = String::from("](");
            for c in url.chars() {
                if matches!(c, '\\' | '(' | ')') {
                    s.push('\\');
                }
                s.push(c);
            }
            s.push(')');
            s
        }
        EntityKind::MentionName(id) => format!("]({MENTION_PREFIX}{id})"),
        other => open_mark(other).to_owned(),
    }
}

fn inline_span(chars: &[char], base: usize, e: &Span) -> Option<Span> {
    let (mut a, mut b) = (e.start, e.end);
    while a < b && chars[a - base].is_whitespace() {
        a += 1;
    }
    while b > a && chars[b - 1 - base].is_whitespace() {
        b -= 1;
    }
    (a < b).then(|| Span {
        start: a,
        end: b,
        kind: e.kind.clone(),
    })
}

fn overlaps(a: &Span, b: &Span) -> bool {
    a.start < b.end && b.start < a.end
}

fn block_tokens(chars: &[char], base: usize, spans: Vec<Span>) -> Vec<Tok> {
    let end = base + chars.len();
    let mut spans: Vec<Span> = spans
        .into_iter()
        .filter(|s| {
            matches!(
                s.kind,
                EntityKind::Bold
                    | EntityKind::Italic
                    | EntityKind::Underline
                    | EntityKind::Strike
                    | EntityKind::Spoiler
                    | EntityKind::Code
                    | EntityKind::Link(Some(_))
                    | EntityKind::MentionName(_)
            ) && s.start < end
                && s.end > base
        })
        .filter_map(|s| {
            inline_span(
                chars,
                base,
                &Span {
                    start: s.start.max(base),
                    end: s.end.min(end),
                    kind: s.kind,
                },
            )
        })
        .collect();
    spans.sort_by_key(|s| (s.start, Reverse(s.end - s.start), rank(&s.kind)));
    let mut codes: Vec<Span> = Vec::new();
    for s in &spans {
        if s.kind == EntityKind::Code && !codes.iter().any(|c| overlaps(c, s)) {
            codes.push(Span {
                start: s.start,
                end: s.end,
                kind: EntityKind::Code,
            });
        }
    }
    let spans: Vec<Span> = spans
        .into_iter()
        .filter(|s| {
            if s.kind == EntityKind::Code {
                codes.iter().any(|c| c.start == s.start && c.end == s.end)
            } else {
                codes
                    .iter()
                    .all(|c| !overlaps(c, s) || (s.start <= c.start && c.end <= s.end))
            }
        })
        .collect();

    let mut toks = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut k = base;
    loop {
        if let Some(lo) = stack.iter().position(|&i| spans[i].end <= k) {
            let tail = stack.split_off(lo);
            for &i in tail.iter().rev() {
                toks.push(Tok::Mark(close_mark(&spans[i].kind)));
            }
            for &i in &tail {
                if spans[i].end > k {
                    stack.push(i);
                    toks.push(Tok::Mark(open_mark(&spans[i].kind).to_owned()));
                }
            }
        }
        let mut jumped = false;
        while next < spans.len() && spans[next].start == k {
            let s = &spans[next];
            if s.kind == EntityKind::Code {
                toks.push(Tok::Mark("`".to_owned()));
                toks.extend(
                    chars[s.start - base..s.end - base]
                        .iter()
                        .map(|c| Tok::Raw(*c)),
                );
                toks.push(Tok::Mark("`".to_owned()));
                k = s.end;
                jumped = true;
            } else {
                stack.push(next);
                toks.push(Tok::Mark(open_mark(&s.kind).to_owned()));
            }
            next += 1;
        }
        if jumped {
            continue;
        }
        if k >= end {
            break;
        }
        toks.push(Tok::Text(chars[k - base]));
        k += 1;
    }
    toks
}

fn render(toks: &[Tok]) -> String {
    let closer_after: Vec<bool> = {
        let mut flags = vec![false; toks.len() + 1];
        for i in (0..toks.len()).rev() {
            flags[i] = flags[i + 1] || matches!(&toks[i], Tok::Mark(m) if m.starts_with("]("));
        }
        flags
    };
    let mut out = String::new();
    let mut line_start = true;
    for (i, tok) in toks.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| toks[p].last());
        let next = toks.get(i + 1).and_then(Tok::first);
        match tok {
            Tok::Raw(c) => out.push(*c),
            Tok::Mark(m) => out.push_str(m),
            Tok::Text(c) => {
                let escape = match c {
                    '\\' => next.is_some_and(is_marker_char),
                    '*' | '_' | '~' | '|' => prev == Some(*c) || next == Some(*c),
                    '`' => true,
                    '[' => closer_after[i + 1],
                    ']' => next == Some('('),
                    '<' => matches!(next, Some('u' | '/')),
                    '>' => line_start && next == Some(' '),
                    _ => false,
                };
                if escape {
                    out.push('\\');
                }
                out.push(*c);
            }
        }
        line_start = tok.is_newline() || (line_start && matches!(tok, Tok::Mark(m) if m == "> "));
    }
    out
}

pub fn to_source(rt: &RichText) -> String {
    let chars: Vec<char> = rt.text.chars().collect();
    let mut off = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0;
    for c in &chars {
        off.push(acc);
        acc += c.len_utf16();
    }
    off.push(acc);
    let char_at = |p: usize| off.partition_point(|&o| o < p);

    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    lines.push((start, chars.len()));

    let spans: Vec<Span> = rt
        .entities
        .iter()
        .filter(|e| e.length > 0)
        .map(|e| Span {
            start: char_at(e.offset),
            end: char_at(e.offset + e.length).min(chars.len()),
            kind: e.kind.clone(),
        })
        .filter(|s| s.start < s.end)
        .collect();

    let mut pres: Vec<&Span> = Vec::new();
    let mut sorted: Vec<&Span> = spans
        .iter()
        .filter(|s| matches!(s.kind, EntityKind::Pre(_)))
        .collect();
    sorted.sort_by_key(|s| s.start);
    for s in sorted {
        if pres.last().is_none_or(|p| p.end <= s.start) {
            pres.push(s);
        }
    }
    let quotes: Vec<&Span> = spans
        .iter()
        .filter(|s| s.kind == EntityKind::Quote)
        .collect();
    let touches = |line: (usize, usize), s: &Span| {
        if line.1 > line.0 {
            line.0 < s.end && line.1 > s.start
        } else {
            line.0 >= s.start && line.0 < s.end
        }
    };

    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Normal,
        Quote,
        Pre(usize),
    }
    let kinds: Vec<Kind> = lines
        .iter()
        .map(|&l| {
            if let Some(p) = pres.iter().position(|s| touches(l, s)) {
                Kind::Pre(p)
            } else if quotes.iter().any(|s| touches(l, s)) {
                Kind::Quote
            } else {
                Kind::Normal
            }
        })
        .collect();

    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && kinds[j] == kinds[i] {
            j += 1;
        }
        let (cs, ce) = (lines[i].0, lines[j - 1].1);
        match kinds[i] {
            Kind::Pre(p) => {
                let lang = match &pres[p].kind {
                    EntityKind::Pre(Some(l)) => l.as_str(),
                    _ => "",
                };
                let body: String = chars[cs..ce].iter().collect();
                parts.push(format!("```{lang}\n{body}\n```"));
            }
            kind => {
                let mut toks = block_tokens(&chars[cs..ce], cs, spans.clone());
                if kind == Kind::Quote {
                    let mut prefixed = vec![Tok::Mark("> ".to_owned())];
                    for t in toks {
                        let nl = t.is_newline();
                        prefixed.push(t);
                        if nl {
                            prefixed.push(Tok::Mark("> ".to_owned()));
                        }
                    }
                    toks = prefixed;
                }
                parts.push(render(&toks));
            }
        }
        i = j;
    }
    parts.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    Code,
    Pre,
    Link,
    Quote,
    Clear,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    pub anchor: Option<usize>,
    pub cursor: usize,
}

fn line_start(chars: &[char], pos: usize) -> usize {
    chars[..pos]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1)
}

fn line_end(chars: &[char], pos: usize) -> usize {
    chars[pos..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |i| pos + i)
}

fn joined(parts: &[&[char]]) -> String {
    parts.iter().flat_map(|p| p.iter()).collect()
}

fn wrap_markers(format: Format) -> (&'static str, &'static str) {
    match format {
        Format::Bold => ("**", "**"),
        Format::Italic => ("__", "__"),
        Format::Underline => ("<u>", "</u>"),
        Format::Strike => ("~~", "~~"),
        Format::Spoiler => ("||", "||"),
        Format::Code => ("`", "`"),
        _ => ("[", "]()"),
    }
}

fn wrap(
    chars: &[char],
    cursor: usize,
    selection: Option<(usize, usize)>,
    format: Format,
) -> Formatted {
    let (from, to) = selection.unwrap_or((cursor, cursor));
    let (open, close) = wrap_markers(format);
    let (open_c, close_c): (Vec<char>, Vec<char>) =
        (open.chars().collect(), close.chars().collect());
    let (o, c) = (open_c.len(), close_c.len());
    let (before, inner, after) = (&chars[..from], &chars[from..to], &chars[to..]);
    if selection.is_some() && format != Format::Link {
        if inner.len() >= o + c && inner.starts_with(&open_c) && inner.ends_with(&close_c) {
            let kept = &inner[o..inner.len() - c];
            return Formatted {
                text: joined(&[before, kept, after]),
                anchor: Some(from),
                cursor: from + kept.len(),
            };
        }
        if before.ends_with(&open_c) && after.starts_with(&close_c) {
            return Formatted {
                text: joined(&[&before[..from - o], inner, &after[c..]]),
                anchor: Some(from - o),
                cursor: to - o,
            };
        }
    }
    let text = joined(&[before, &open_c, inner, &close_c, after]);
    match (format, selection) {
        (Format::Link, Some(_)) => Formatted {
            text,
            anchor: None,
            cursor: to + o + 2,
        },
        (_, Some(_)) => Formatted {
            text,
            anchor: Some(from + o),
            cursor: to + o,
        },
        _ => Formatted {
            text,
            anchor: None,
            cursor: from + o,
        },
    }
}

fn wrap_block(chars: &[char], cursor: usize, selection: Option<(usize, usize)>) -> Formatted {
    let (from, to) = selection.unwrap_or((cursor, cursor));
    let (before, inner, after) = (&chars[..from], &chars[from..to], &chars[to..]);
    let fence: Vec<char> = "```".chars().collect();
    let close_fence: Vec<char> = "\n```".chars().collect();
    if selection.is_some()
        && inner.starts_with(&fence)
        && inner.ends_with(&close_fence)
        && let Some(nl) = inner.iter().position(|c| *c == '\n')
        && nl + 5 <= inner.len()
    {
        let kept = &inner[nl + 1..inner.len() - 4];
        return Formatted {
            text: joined(&[before, kept, after]),
            anchor: Some(from),
            cursor: from + kept.len(),
        };
    }
    let lead: Vec<char> = if from > 0 && chars[from - 1] != '\n' {
        "\n```\n".chars().collect()
    } else {
        "```\n".chars().collect()
    };
    let trail: Vec<char> = if to < chars.len() && chars[to] != '\n' {
        "\n```\n".chars().collect()
    } else {
        "\n```".chars().collect()
    };
    Formatted {
        text: joined(&[before, &lead, inner, &trail, after]),
        anchor: selection.map(|_| from + lead.len()),
        cursor: to + lead.len(),
    }
}

fn quote_lines(chars: &[char], cursor: usize, selection: Option<(usize, usize)>) -> Formatted {
    let (from, to) = selection.unwrap_or((cursor, cursor));
    let last = if to > from && chars[to - 1] == '\n' {
        to - 1
    } else {
        to
    };
    let first = line_start(chars, from);
    let mut starts = vec![first];
    for (i, c) in chars.iter().enumerate().take(last).skip(first) {
        if *c == '\n' {
            starts.push(i + 1);
        }
    }
    let prefix = ['>', ' '];
    let remove = starts.iter().all(|&s| chars[s..].starts_with(&prefix));
    let mut out: Vec<char> = Vec::with_capacity(chars.len() + 2 * starts.len());
    let mut at = 0;
    for &s in &starts {
        out.extend_from_slice(&chars[at..s]);
        if remove {
            at = s + 2;
        } else {
            out.extend_from_slice(&prefix);
            at = s;
        }
    }
    out.extend_from_slice(&chars[at..]);
    let map = |p: usize| {
        if remove {
            p - starts
                .iter()
                .map(|&s| p.saturating_sub(s).min(2))
                .sum::<usize>()
        } else {
            p + 2 * starts.iter().filter(|&&s| s <= p).count()
        }
    };
    Formatted {
        text: out.into_iter().collect(),
        anchor: selection.map(|_| map(from)),
        cursor: map(to),
    }
}

fn clear_markers(chars: &[char], cursor: usize, selection: Option<(usize, usize)>) -> Formatted {
    let (from, to) = selection.unwrap_or_else(|| {
        let at = line_start(chars, cursor);
        (at, line_end(chars, cursor))
    });
    let source: String = chars.iter().collect();
    let mut removed = vec![false; chars.len()];
    for r in parse(&source).markers {
        if r.start >= to || r.end <= from {
            continue;
        }
        let (mut a, mut b) = (r.start, r.end);
        let whole_line = chars[a..b].starts_with(&['`', '`', '`'])
            && (a == 0 || chars[a - 1] == '\n')
            && (b == chars.len() || chars[b] == '\n');
        if whole_line {
            if b < chars.len() {
                b += 1;
            } else {
                a = a.saturating_sub(1);
            }
        }
        removed[a..b].iter_mut().for_each(|x| *x = true);
    }
    let map = |p: usize| p - removed[..p].iter().filter(|x| **x).count();
    let text: String = chars
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| !**gone)
        .map(|(c, _)| *c)
        .collect();
    Formatted {
        text,
        anchor: selection.map(|_| map(from)),
        cursor: if selection.is_some() {
            map(to)
        } else {
            map(cursor)
        },
    }
}

pub fn apply(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    format: Format,
) -> Formatted {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let selection = selection
        .map(|(a, b)| (a.min(b).min(chars.len()), a.max(b).min(chars.len())))
        .filter(|(a, b)| a < b);
    match format {
        Format::Quote => quote_lines(&chars, cursor, selection),
        Format::Clear => clear_markers(&chars, cursor, selection),
        Format::Pre => wrap_block(&chars, cursor, selection),
        _ => wrap(&chars, cursor, selection, format),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(offset: usize, length: usize, kind: EntityKind) -> Entity {
        Entity {
            offset,
            length,
            kind,
        }
    }

    fn rich(text: &str, entities: Vec<Entity>) -> RichText {
        RichText {
            text: text.to_owned(),
            entities,
        }
    }

    fn assert_round_trip(rt: &RichText) {
        let source = to_source(rt);
        assert_eq!(&parse(&source).rich, rt, "source was {source:?}");
    }

    fn assert_plain(text: &str) {
        let parsed = parse(text);
        assert_eq!(parsed.rich, RichText::plain(text), "parsing {text:?}");
        assert!(parsed.markers.is_empty(), "markers for {text:?}");
    }

    #[test]
    fn bold_italic_strike_spoiler_parse() {
        let p = parse("**a** __b__ ~~c~~ ||d||").rich;
        assert_eq!(p.text, "a b c d");
        assert_eq!(
            p.entities,
            vec![
                ent(0, 1, EntityKind::Bold),
                ent(2, 1, EntityKind::Italic),
                ent(4, 1, EntityKind::Strike),
                ent(6, 1, EntityKind::Spoiler),
            ]
        );
    }

    #[test]
    fn underline_and_code_parse() {
        let p = parse("<u>under</u> and `code`").rich;
        assert_eq!(p.text, "under and code");
        assert_eq!(
            p.entities,
            vec![
                ent(0, 5, EntityKind::Underline),
                ent(10, 4, EntityKind::Code)
            ]
        );
    }

    #[test]
    fn link_and_mention_parse() {
        let p = parse("[site](https://e.com/x) [me](tg://user?id=42)").rich;
        assert_eq!(p.text, "site me");
        assert_eq!(
            p.entities,
            vec![
                ent(0, 4, EntityKind::Link(Some("https://e.com/x".to_owned()))),
                ent(5, 2, EntityKind::MentionName(42)),
            ]
        );
    }

    #[test]
    fn link_url_may_contain_balanced_parentheses() {
        let p = parse("[w](https://e.com/a_(b))").rich;
        assert_eq!(
            p.entities,
            vec![ent(
                0,
                1,
                EntityKind::Link(Some("https://e.com/a_(b)".to_owned()))
            )]
        );
    }

    #[test]
    fn ordinary_text_is_not_mangled() {
        for text in [
            "my__var__name",
            "C:\\Users\\me\\file",
            "a * b * c",
            "2 ** 3 and 4 ** 5",
            "a lone ~~",
            "~~",
            "https://example.com/a__b__c",
            "an unclosed **bold",
            "[not a link] (really)",
            "x < y > z",
            "a | b || c",
            "``",
            "`",
        ] {
            assert_plain(text);
        }
    }

    #[test]
    fn backslash_escapes_marker_characters_only() {
        let p = parse("\\*\\*not bold\\*\\* and \\n").rich;
        assert_eq!(p, RichText::plain("**not bold** and \\n"));
        assert_eq!(parse("\\\\").rich, RichText::plain("\\"));
    }

    #[test]
    fn escapes_are_reported_separately_from_markers() {
        let p = parse("\\*a");
        assert_eq!(p.escapes, vec![0..1]);
        assert!(p.markers.is_empty());
    }

    #[test]
    fn nothing_is_parsed_inside_code() {
        let p = parse("`**x** __y__`").rich;
        assert_eq!(p.text, "**x** __y__");
        assert_eq!(p.entities, vec![ent(0, 11, EntityKind::Code)]);
    }

    #[test]
    fn nesting_bold_inside_italic() {
        let p = parse("__a **b** c__").rich;
        assert_eq!(p.text, "a b c");
        assert_eq!(
            p.entities,
            vec![ent(0, 5, EntityKind::Italic), ent(2, 1, EntityKind::Bold)]
        );
    }

    #[test]
    fn italic_needs_word_boundaries() {
        assert_plain("a__b__c");
        let p = parse("a __b__ c").rich;
        assert_eq!(p.entities, vec![ent(2, 1, EntityKind::Italic)]);
    }

    #[test]
    fn fenced_block_with_language() {
        let p = parse("hi\n```rust\nfn x() {}\n```\nbye").rich;
        assert_eq!(p.text, "hi\nfn x() {}\nbye");
        assert_eq!(
            p.entities,
            vec![ent(3, 9, EntityKind::Pre(Some("rust".to_owned())))]
        );
    }

    #[test]
    fn fenced_block_without_language_ignores_markup() {
        let p = parse("```\n**x**\n```").rich;
        assert_eq!(p.text, "**x**");
        assert_eq!(p.entities, vec![ent(0, 5, EntityKind::Pre(None))]);
    }

    #[test]
    fn unclosed_fence_is_plain_text() {
        assert_plain("```rust\nfn x() {}");
    }

    #[test]
    fn quote_run_is_one_entity_without_prefixes() {
        let p = parse("> one\n> two\nafter").rich;
        assert_eq!(p.text, "one\ntwo\nafter");
        assert_eq!(p.entities, vec![ent(0, 7, EntityKind::Quote)]);
    }

    #[test]
    fn quote_allows_inline_formatting() {
        let p = parse("> a **b**\n> `c`").rich;
        assert_eq!(p.text, "a b\nc");
        assert_eq!(
            p.entities,
            vec![
                ent(0, 5, EntityKind::Quote),
                ent(2, 1, EntityKind::Bold),
                ent(4, 1, EntityKind::Code),
            ]
        );
    }

    #[test]
    fn separate_quotes_are_separate_entities() {
        let p = parse("> a\nmid\n> b").rich;
        assert_eq!(p.text, "a\nmid\nb");
        assert_eq!(
            p.entities,
            vec![ent(0, 1, EntityKind::Quote), ent(6, 1, EntityKind::Quote)]
        );
    }

    #[test]
    fn offsets_count_utf16_units() {
        let p = parse("😀 **жир** 😀 __курсив__").rich;
        assert_eq!(p.text, "😀 жир 😀 курсив");
        assert_eq!(
            p.entities,
            vec![ent(3, 3, EntityKind::Bold), ent(10, 6, EntityKind::Italic)]
        );
    }

    #[test]
    fn marker_and_styled_ranges_use_source_chars() {
        let p = parse("😀**hi** there");
        assert_eq!(p.markers, vec![1..3, 5..7]);
        assert_eq!(p.styled, vec![(3..5, EntityKind::Bold)]);
    }

    #[test]
    fn quote_prefixes_and_fences_are_markers() {
        let p = parse("> q\n```c\nx\n```");
        assert_eq!(p.markers, vec![0..2, 4..8, 11..14]);
    }

    #[test]
    fn round_trip_each_kind() {
        let kinds = [
            EntityKind::Bold,
            EntityKind::Italic,
            EntityKind::Underline,
            EntityKind::Strike,
            EntityKind::Spoiler,
            EntityKind::Code,
            EntityKind::Link(Some("https://e.com/a(b)".to_owned())),
            EntityKind::MentionName(7),
            EntityKind::Quote,
            EntityKind::Pre(None),
            EntityKind::Pre(Some("python".to_owned())),
        ];
        for kind in kinds {
            if !matches!(kind, EntityKind::Quote | EntityKind::Pre(_)) {
                assert_round_trip(&rich("x word y", vec![ent(2, 4, kind.clone())]));
            }
            assert_round_trip(&rich("word", vec![ent(0, 4, kind.clone())]));
        }
    }

    #[test]
    fn round_trip_combinations() {
        assert_round_trip(&rich(
            "all of it",
            vec![
                ent(0, 9, EntityKind::Bold),
                ent(0, 9, EntityKind::Italic),
                ent(4, 2, EntityKind::Underline),
            ],
        ));
        assert_round_trip(&rich(
            "a b\nc d\ne",
            vec![
                ent(0, 7, EntityKind::Quote),
                ent(2, 1, EntityKind::Bold),
                ent(4, 1, EntityKind::Code),
            ],
        ));
        assert_round_trip(&rich(
            "intro\nlet x = 1;\nend",
            vec![ent(6, 10, EntityKind::Pre(Some("rust".to_owned())))],
        ));
        assert_round_trip(&rich(
            "go there now",
            vec![
                ent(3, 5, EntityKind::Bold),
                ent(3, 5, EntityKind::Link(Some("https://e.com".to_owned()))),
            ],
        ));
    }

    #[test]
    fn round_trip_with_emoji_and_cyrillic() {
        assert_round_trip(&rich(
            "😀 привет мир 😀 ok",
            vec![ent(3, 6, EntityKind::Bold), ent(10, 3, EntityKind::Spoiler)],
        ));
    }

    #[test]
    fn round_trip_plain_text_full_of_markers() {
        for text in [
            "**x**",
            "__init__",
            "~~gone~~ ||secret||",
            "`code` and ```",
            "[a](b)",
            "[[a](b)",
            "> not a quote",
            "<u>x</u>",
            "\\*",
            "\\\\*",
            "C:\\Users\\me\\",
            "a * b ** c *** d",
            "*_~|`[]<>\\",
            "line\n> next\n```\nend",
            "a < b </u> c",
        ] {
            assert_round_trip(&RichText::plain(text));
        }
    }

    #[test]
    fn round_trip_markers_inside_formatted_text() {
        assert_round_trip(&rich("a*b* **c**", vec![ent(0, 10, EntityKind::Bold)]));
        assert_round_trip(&rich("x ]( y", vec![ent(0, 6, EntityKind::Italic)]));
    }

    #[test]
    fn to_source_escapes_literal_markers() {
        assert_eq!(to_source(&RichText::plain("**x**")), "\\*\\*x\\*\\*");
        assert_eq!(to_source(&RichText::plain("a * b * c")), "a * b * c");
        assert_eq!(to_source(&RichText::plain("> q")), "\\> q");
    }

    #[test]
    fn to_source_hoists_whitespace_out_of_markers() {
        let rt = rich("bold  next", vec![ent(0, 5, EntityKind::Bold)]);
        assert_eq!(to_source(&rt), "**bold**  next");
    }

    #[test]
    fn partially_overlapping_entities_are_split_into_nested_markers() {
        let rt = rich(
            "abcdef",
            vec![ent(0, 4, EntityKind::Bold), ent(2, 4, EntityKind::Strike)],
        );
        let back = parse(&to_source(&rt)).rich;
        assert_eq!(back.text, "abcdef");
        let covered = |kind: EntityKind| -> Vec<bool> {
            (0..6)
                .map(|p| {
                    back.entities
                        .iter()
                        .any(|e| e.kind == kind && p >= e.offset && p < e.offset + e.length)
                })
                .collect()
        };
        assert_eq!(
            covered(EntityKind::Bold),
            [true, true, true, true, false, false]
        );
        assert_eq!(
            covered(EntityKind::Strike),
            [false, false, true, true, true, true]
        );
    }

    #[test]
    fn link_without_url_entity_is_plain_in_source() {
        let rt = rich(
            "see https://e.com",
            vec![ent(4, 13, EntityKind::Link(None))],
        );
        assert_eq!(to_source(&rt), "see https://e.com");
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse("").rich, RichText::default());
        assert_eq!(to_source(&RichText::default()), "");
    }

    fn applied(
        text: &str,
        cursor: usize,
        sel: Option<(usize, usize)>,
        f: Format,
    ) -> (String, Option<usize>, usize) {
        let out = apply(text, cursor, sel, f);
        (out.text, out.anchor, out.cursor)
    }

    #[test]
    fn bold_wraps_the_selection() {
        assert_eq!(
            applied("say hello now", 9, Some((4, 9)), Format::Bold),
            ("say **hello** now".to_owned(), Some(6), 11)
        );
    }

    #[test]
    fn bold_toggles_off_when_markers_are_inside_the_selection() {
        assert_eq!(
            applied("say **hello** now", 13, Some((4, 13)), Format::Bold),
            ("say hello now".to_owned(), Some(4), 9)
        );
    }

    #[test]
    fn bold_toggles_off_when_markers_surround_the_selection() {
        assert_eq!(
            applied("say **hello** now", 11, Some((6, 11)), Format::Bold),
            ("say hello now".to_owned(), Some(4), 9)
        );
    }

    #[test]
    fn other_markers_do_not_toggle() {
        assert_eq!(
            applied("__hi__", 6, Some((0, 6)), Format::Bold).0,
            "**__hi__**"
        );
    }

    #[test]
    fn bold_without_selection_puts_the_cursor_between_markers() {
        assert_eq!(
            applied("ab", 1, None, Format::Bold),
            ("a****b".to_owned(), None, 3)
        );
    }

    #[test]
    fn every_inline_format_wraps_with_its_own_markers() {
        for (f, want) in [
            (Format::Italic, "__x__"),
            (Format::Underline, "<u>x</u>"),
            (Format::Strike, "~~x~~"),
            (Format::Spoiler, "||x||"),
            (Format::Code, "`x`"),
        ] {
            assert_eq!(applied("x", 1, Some((0, 1)), f).0, want);
        }
    }

    #[test]
    fn link_leaves_the_cursor_inside_the_parentheses() {
        assert_eq!(
            applied("see site", 8, Some((4, 8)), Format::Link),
            ("see [site]()".to_owned(), None, 11)
        );
        assert_eq!(
            applied("", 0, None, Format::Link),
            ("[]()".to_owned(), None, 1)
        );
    }

    #[test]
    fn quote_prefixes_every_selected_line() {
        assert_eq!(
            applied("a\nb\nc", 3, Some((0, 3)), Format::Quote),
            ("> a\n> b\nc".to_owned(), Some(2), 7)
        );
    }

    #[test]
    fn quote_skips_the_line_where_the_selection_only_touches_the_start() {
        assert_eq!(applied("a\nb", 2, Some((0, 2)), Format::Quote).0, "> a\nb");
    }

    #[test]
    fn quote_toggles_off_and_works_without_selection() {
        assert_eq!(
            applied("> a\n> b", 8, Some((0, 7)), Format::Quote).0,
            "a\nb"
        );
        assert_eq!(
            applied("one\ntwo", 5, None, Format::Quote),
            ("one\n> two".to_owned(), None, 7)
        );
    }

    #[test]
    fn code_block_goes_on_its_own_lines() {
        assert_eq!(
            applied("a b c", 3, Some((2, 3)), Format::Pre).0,
            "a \n```\nb\n```\n c"
        );
        assert_eq!(
            applied("", 0, None, Format::Pre),
            ("```\n\n```".to_owned(), None, 4)
        );
        assert_eq!(applied("```\nx\n```", 9, Some((0, 9)), Format::Pre).0, "x");
    }

    #[test]
    fn clear_removes_markers_in_the_selection_only() {
        let (text, anchor, cursor) = applied("**a** and __b__", 15, Some((0, 7)), Format::Clear);
        assert_eq!(text, "a and __b__");
        assert_eq!((anchor, cursor), (Some(0), 3));
    }

    #[test]
    fn clear_removes_quote_prefixes_and_fences() {
        assert_eq!(
            applied("> **q**\n```c\nx\n```", 0, Some((0, 20)), Format::Clear).0,
            "q\nx"
        );
    }

    #[test]
    fn clear_keeps_escaped_characters() {
        assert_eq!(
            applied("\\*a\\* **b**", 0, Some((0, 11)), Format::Clear).0,
            "\\*a\\* b"
        );
    }

    #[test]
    fn clear_without_selection_works_on_the_current_line() {
        assert_eq!(
            applied("**a**\n**b**", 8, None, Format::Clear).0,
            "**a**\nb"
        );
    }

    #[test]
    fn formatting_survives_emoji_and_wide_text() {
        assert_eq!(
            applied("😀 привет", 9, Some((2, 8)), Format::Italic).0,
            "😀 __привет__"
        );
    }
}
