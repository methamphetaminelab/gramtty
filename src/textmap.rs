use std::cell::RefCell;

use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;

use crate::app::{Pane, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Pane(Pane),
    Popup,
    Cover,
}

impl Zone {
    fn plain(self) -> bool {
        matches!(self, Zone::Pane(Pane::Info) | Zone::Popup)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Line,
    Space,
    Glue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Folder(usize),
    Chat(Row),
    Message(i64),
    Composer,
}

impl Hit {
    fn zone(self) -> Zone {
        Zone::Pane(match self {
            Hit::Folder(_) => Pane::Folders,
            Hit::Chat(_) => Pane::Chats,
            Hit::Message(_) | Hit::Composer => Pane::Messages,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub zone: Zone,
    pub anchor: Position,
    pub head: Position,
    pub dragged: bool,
}

impl Selection {
    pub fn new(zone: Zone, at: Position) -> Self {
        Self {
            zone,
            anchor: at,
            head: at,
            dragged: false,
        }
    }

    pub fn extend(&mut self, to: Position) {
        self.dragged |= to != self.anchor;
        self.head = to;
    }

    fn columns(&self, y: u16, rect: Rect) -> Option<(u16, u16)> {
        let key = |p: Position| (p.y, p.x);
        let (start, end) = if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        if y < start.y || y > end.y || y < rect.top() || y >= rect.bottom() {
            return None;
        }
        let lo = if y == start.y {
            start.x.max(rect.left())
        } else {
            rect.left()
        };
        let hi = if y == end.y {
            end.x.saturating_add(1).min(rect.right())
        } else {
            rect.right()
        };
        Some((lo, hi))
    }
}

#[derive(Clone, Debug)]
struct Strip {
    area: usize,
    x: u16,
    y: u16,
    width: u16,
    join: Join,
    text: String,
    bounds: Vec<usize>,
}

impl Strip {
    fn pick(&self, sel: &Selection, rect: Rect) -> Option<(usize, usize)> {
        let (lo, hi) = sel.columns(self.y, rect)?;
        if self.width == 0 {
            return (lo <= self.x && self.x < hi).then_some((0, 0));
        }
        let a = lo.max(self.x);
        let b = hi.min(self.x + self.width);
        (a < b).then(|| (usize::from(a - self.x), usize::from(b - self.x)))
    }
}

#[derive(Debug, Default)]
pub struct TextMap {
    areas: Vec<(Zone, Rect)>,
    strips: Vec<Strip>,
    hits: Vec<(Rect, Hit)>,
}

impl TextMap {
    pub fn add_area(&mut self, zone: Zone, rect: Rect) {
        self.areas.push((zone, rect));
    }

    pub fn add_row(&mut self, x: u16, y: u16, width: u16, join: Join) {
        let Some(area) = self.areas.len().checked_sub(1) else {
            return;
        };
        self.strips.push(Strip {
            area,
            x,
            y,
            width,
            join,
            text: String::new(),
            bounds: Vec::new(),
        });
    }

    pub fn add_hit(&mut self, rect: Rect, hit: Hit) {
        self.hits.push((rect, hit));
    }

    fn covered(&self, area: usize, at: Position) -> bool {
        self.areas[area + 1..].iter().any(|(_, r)| r.contains(at))
    }

    pub fn capture(&mut self, buf: &Buffer) {
        let strips = std::mem::take(&mut self.strips);
        self.strips = strips
            .into_iter()
            .filter_map(|s| self.read(s, buf))
            .collect();
    }

    fn read(&self, mut s: Strip, buf: &Buffer) -> Option<Strip> {
        let rect = self.areas[s.area].1.intersection(buf.area);
        if !rect.contains(Position::new(s.x, s.y)) {
            return None;
        }
        s.width = s.width.min(rect.right() - s.x);
        s.text.clear();
        s.bounds = Vec::with_capacity(usize::from(s.width) + 1);
        let mut skip = 0;
        for x in s.x..s.x + s.width {
            s.bounds.push(s.text.len());
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let at = Position::new(x, s.y);
            if self.covered(s.area, at) {
                continue;
            }
            let cell = &buf[at];
            s.text.push_str(cell.symbol());
            skip = cell.cell_width().saturating_sub(1);
        }
        s.bounds.push(s.text.len());
        Some(s)
    }

    pub fn zone_at(&self, at: Position) -> Option<Zone> {
        let (zone, _) = self.areas.iter().rev().find(|(_, r)| r.contains(at))?;
        (*zone != Zone::Cover).then_some(*zone)
    }

    pub fn hit_at(&self, at: Position) -> Option<Hit> {
        let zone = self.zone_at(at)?;
        self.hits
            .iter()
            .find(|(r, h)| h.zone() == zone && r.contains(at))
            .map(|(_, h)| *h)
    }

    fn area_of(&self, zone: Zone) -> Option<usize> {
        self.areas.iter().rposition(|(z, _)| *z == zone)
    }

    pub fn text(&self, sel: &Selection) -> Option<String> {
        let area = self.area_of(sel.zone)?;
        let rect = self.areas[area].1;
        let mut strips: Vec<&Strip> = self.strips.iter().filter(|s| s.area == area).collect();
        strips.sort_by_key(|s| (s.y, s.x));
        let mut out = String::new();
        let mut last = None;
        for (i, s) in strips.iter().enumerate() {
            let Some((a, b)) = s.pick(sel, rect) else {
                continue;
            };
            let piece = &s.text[s.bounds[a]..s.bounds[b]];
            let piece = if sel.zone.plain() {
                piece.trim()
            } else {
                piece.trim_end()
            };
            if !out.is_empty() {
                out.push_str(match s.join {
                    Join::Space if last == Some(i - 1) => " ",
                    Join::Glue if last == Some(i - 1) => "",
                    _ => "\n",
                });
            }
            out.push_str(piece);
            last = Some(i);
        }
        let out = out.trim_end();
        (!out.is_empty()).then(|| out.to_owned())
    }

    pub fn highlight(&self, sel: &Selection, buf: &mut Buffer) {
        let Some(area) = self.area_of(sel.zone) else {
            return;
        };
        let rect = self.areas[area].1;
        for s in self.strips.iter().filter(|s| s.area == area) {
            let Some((a, b)) = s.pick(sel, rect) else {
                continue;
            };
            for x in s.x + a as u16..s.x + b as u16 {
                let at = Position::new(x, s.y);
                if !self.covered(area, at) {
                    buf[at].modifier.toggle(Modifier::REVERSED);
                }
            }
        }
    }
}

thread_local! {
    static FRAME: RefCell<TextMap> = RefCell::new(TextMap::default());
}

fn record(f: impl FnOnce(&mut TextMap)) {
    FRAME.with(|m| f(&mut m.borrow_mut()));
}

pub fn begin() {
    record(|m| *m = TextMap::default());
}

pub fn area(zone: Zone, rect: Rect) {
    record(|m| m.add_area(zone, rect));
}

pub fn cover(rect: Rect) {
    area(Zone::Cover, rect);
}

pub fn row(x: u16, y: u16, width: u16, join: Join) {
    record(|m| m.add_row(x, y, width, join));
}

pub fn hit(rect: Rect, hit: Hit) {
    record(|m| m.add_hit(rect, hit));
}

pub fn panel(rect: Rect, inner: Rect) {
    record(|m| {
        m.add_area(Zone::Popup, rect);
        for y in inner.top()..inner.bottom() {
            m.add_row(inner.x, y, inner.width, Join::Line);
        }
    });
}

pub fn finish(buf: &mut Buffer, sel: Option<&Selection>) -> TextMap {
    let mut map = FRAME.with(|m| std::mem::take(&mut *m.borrow_mut()));
    map.capture(buf);
    if let Some(s) = sel.filter(|s| s.dragged) {
        map.highlight(s, buf);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGES: Zone = Zone::Pane(Pane::Messages);

    fn at(x: u16, y: u16) -> Position {
        Position::new(x, y)
    }

    fn drag(zone: Zone, from: Position, to: Position) -> Selection {
        let mut s = Selection::new(zone, from);
        s.extend(to);
        s
    }

    fn wrapped() -> (TextMap, Buffer) {
        let buf = Buffer::with_lines([
            "│ Alice  12:00      │",
            "│ hello there,      │",
            "│ this wraps        │",
            "│ aroundandaroun    │",
            "│ d          12:01 ✓│",
            "│ second            │",
            "│                   │",
            "│ third para        │",
        ]);
        let mut map = TextMap::default();
        map.add_area(MESSAGES, Rect::new(0, 0, 21, 8));
        map.add_row(2, 1, 12, Join::Line);
        map.add_row(2, 2, 10, Join::Space);
        map.add_row(2, 3, 14, Join::Space);
        map.add_row(2, 4, 1, Join::Glue);
        map.add_row(2, 5, 6, Join::Line);
        map.add_row(2, 6, 0, Join::Line);
        map.add_row(2, 7, 10, Join::Line);
        map.capture(&buf);
        (map, buf)
    }

    #[test]
    fn soft_wrapped_rows_join_back_into_the_original_line() {
        let (map, _) = wrapped();
        let whole = drag(MESSAGES, at(2, 1), at(2, 4));
        assert_eq!(
            map.text(&whole).as_deref(),
            Some("hello there, this wraps aroundandaround")
        );
        let middle = drag(MESSAGES, at(5, 2), at(8, 1));
        assert_eq!(map.text(&middle).as_deref(), Some("there, this"));
    }

    #[test]
    fn messages_and_paragraphs_keep_their_line_breaks_and_drop_chrome() {
        let (map, _) = wrapped();
        let all = drag(MESSAGES, at(0, 0), at(20, 7));
        assert_eq!(
            map.text(&all).as_deref(),
            Some("hello there, this wraps aroundandaround\nsecond\n\nthird para")
        );
        let trailing = drag(MESSAGES, at(2, 5), at(19, 6));
        assert_eq!(map.text(&trailing).as_deref(), Some("second"));
    }

    #[test]
    fn selection_is_clipped_to_the_pane_it_started_in() {
        let buf = Buffer::with_lines(["left one  right one", "left two  right two"]);
        let mut map = TextMap::default();
        map.add_area(Zone::Pane(Pane::Chats), Rect::new(0, 0, 10, 2));
        map.add_row(0, 0, 8, Join::Line);
        map.add_row(0, 1, 8, Join::Line);
        map.add_area(MESSAGES, Rect::new(10, 0, 9, 2));
        map.add_row(10, 0, 9, Join::Line);
        map.add_row(10, 1, 9, Join::Line);
        map.capture(&buf);
        let s = drag(Zone::Pane(Pane::Chats), at(5, 0), at(18, 1));
        assert_eq!(map.text(&s).as_deref(), Some("one\nleft two"));
        let s = drag(MESSAGES, at(16, 0), at(14, 1));
        assert_eq!(map.text(&s).as_deref(), Some("one\nright"));
        let s = drag(MESSAGES, at(16, 0), at(2, 1));
        assert_eq!(map.text(&s).as_deref(), Some("one"));
        let s = drag(MESSAGES, at(16, 1), at(16, 5));
        assert_eq!(map.text(&s).as_deref(), Some("two"));
    }

    #[test]
    fn covered_cells_are_neither_copied_nor_highlighted() {
        let mut buf = Buffer::with_lines(["a message under a popup"]);
        let mut map = TextMap::default();
        map.add_area(MESSAGES, Rect::new(0, 0, 23, 1));
        map.add_row(0, 0, 23, Join::Line);
        map.add_area(Zone::Cover, Rect::new(2, 0, 8, 1));
        map.capture(&buf);
        let s = drag(MESSAGES, at(0, 0), at(22, 0));
        assert_eq!(map.text(&s).as_deref(), Some("a under a popup"));
        map.highlight(&s, &mut buf);
        assert!(buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(4, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(10, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn empty_selections_copy_nothing() {
        let (map, _) = wrapped();
        assert_eq!(map.text(&drag(MESSAGES, at(15, 1), at(19, 1))), None);
        assert_eq!(map.text(&drag(MESSAGES, at(0, 0), at(19, 0))), None);
        assert_eq!(map.text(&drag(MESSAGES, at(3, 6), at(15, 6))), None);
        assert_eq!(map.text(&drag(Zone::Popup, at(2, 1), at(9, 2))), None);
    }

    #[test]
    fn wide_characters_are_copied_whole() {
        let buf = Buffer::with_lines(["你好 ok"]);
        let mut map = TextMap::default();
        map.add_area(MESSAGES, Rect::new(0, 0, 7, 1));
        map.add_row(0, 0, 7, Join::Line);
        map.capture(&buf);
        let copy = |from: u16, to: u16| map.text(&drag(MESSAGES, at(from, 0), at(to, 0)));
        assert_eq!(copy(0, 1).as_deref(), Some("你"));
        assert_eq!(copy(1, 3).as_deref(), Some("好"));
        assert_eq!(copy(0, 6).as_deref(), Some("你好 ok"));
    }

    #[test]
    fn popup_rows_are_trimmed_on_both_sides() {
        let buf = Buffer::with_lines([
            "│  Delete this?   │",
            "│                 │",
            "│ y yes   n no    │",
        ]);
        let mut map = TextMap::default();
        map.add_area(Zone::Popup, Rect::new(0, 0, 19, 3));
        for y in 0..3 {
            map.add_row(1, y, 17, Join::Line);
        }
        map.capture(&buf);
        let s = drag(Zone::Popup, at(0, 0), at(18, 2));
        assert_eq!(
            map.text(&s).as_deref(),
            Some("Delete this?\n\ny yes   n no")
        );
    }

    #[test]
    fn highlight_marks_only_selected_text_cells() {
        let (map, mut buf) = wrapped();
        map.highlight(&drag(MESSAGES, at(0, 1), at(20, 2)), &mut buf);
        let lit = |x: u16, y: u16| buf[(x, y)].modifier.contains(Modifier::REVERSED);
        assert!(!lit(0, 1) && !lit(1, 1));
        assert!((2..14).all(|x| lit(x, 1)));
        assert!(!lit(14, 1));
        assert!((2..12).all(|x| lit(x, 2)) && !lit(12, 2));
        assert!(!lit(2, 3));
    }

    #[test]
    fn zones_and_hits_respect_what_is_drawn_on_top() {
        let mut map = TextMap::default();
        map.add_area(MESSAGES, Rect::new(0, 0, 20, 5));
        map.add_hit(Rect::new(0, 0, 20, 5), Hit::Message(7));
        map.add_area(Zone::Popup, Rect::new(5, 1, 10, 3));
        map.add_area(Zone::Cover, Rect::new(0, 4, 3, 1));
        assert_eq!(map.zone_at(at(1, 1)), Some(MESSAGES));
        assert_eq!(map.hit_at(at(1, 1)), Some(Hit::Message(7)));
        assert_eq!(map.zone_at(at(6, 2)), Some(Zone::Popup));
        assert_eq!(map.hit_at(at(6, 2)), None);
        assert_eq!(map.zone_at(at(1, 4)), None);
        assert_eq!(map.hit_at(at(1, 4)), None);
        assert_eq!(map.zone_at(at(30, 1)), None);
    }

    #[test]
    fn a_press_without_movement_is_not_a_drag() {
        let mut s = Selection::new(MESSAGES, at(3, 3));
        s.extend(at(3, 3));
        assert!(!s.dragged);
        s.extend(at(4, 3));
        s.extend(at(3, 3));
        assert!(s.dragged);
    }

    #[test]
    fn recording_collects_a_frame_and_highlights_a_drag() {
        let mut buf = Buffer::with_lines(["  text  "]);
        begin();
        area(MESSAGES, buf.area);
        row(2, 0, 4, Join::Line);
        let sel = drag(MESSAGES, at(0, 0), at(7, 0));
        let map = finish(&mut buf, Some(&sel));
        assert_eq!(map.text(&sel).as_deref(), Some("text"));
        assert!(buf[(2, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(1, 0)].modifier.contains(Modifier::REVERSED));
        begin();
        let empty = finish(&mut buf, None);
        assert_eq!(empty.text(&sel), None);
    }
}
