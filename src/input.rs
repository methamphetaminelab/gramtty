use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

pub enum Edit {
    Changed,
    Moved,
    Ignored,
}

impl TextInput {
    pub fn with_text(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.chars().count(),
            anchor: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor && anchor <= self.len())
            .then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn select(&mut self, anchor: usize, cursor: usize) {
        let len = self.len();
        self.anchor = Some(anchor.min(len));
        self.cursor = cursor.min(len);
    }

    pub fn splice(&mut self, from: usize, to: usize, with: &str, cursor: usize) {
        let (a, b) = (self.byte_at(from), self.byte_at(to.max(from)));
        self.text.replace_range(a..b, with);
        self.anchor = None;
        self.cursor = cursor.min(self.len());
    }

    fn delete_selection(&mut self) -> bool {
        let selection = self.selection();
        self.anchor = None;
        match selection {
            Some((from, to)) => self.remove_range(from, to),
            None => false,
        }
    }

    pub fn slice(&self, from: usize, to: usize) -> String {
        self.text
            .chars()
            .skip(from)
            .take(to.saturating_sub(from))
            .collect()
    }

    pub fn move_visual(&mut self, down: bool, width: usize) {
        let (start, end) = (self.line_start(), self.line_end());
        let w = width.max(1);
        if down {
            if self.cursor + w <= end {
                self.cursor += w;
            } else if (self.cursor - start) / w < (end - start) / w {
                self.cursor = end;
            } else {
                self.move_line(true);
            }
        } else if self.cursor - start >= w {
            self.cursor -= w;
        } else {
            self.move_line(false);
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn at_end(&self) -> bool {
        self.cursor >= self.len()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(b, _)| b)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn insert(&mut self, ch: char) {
        self.delete_selection();
        let b = self.byte_at(self.cursor);
        self.text.insert(b, ch);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        let clean: String = s.chars().filter(|&ch| ch != '\r').collect();
        let b = self.byte_at(self.cursor);
        self.text.insert_str(b, &clean);
        self.cursor += clean.chars().count();
    }

    fn prev_boundary(&self) -> usize {
        let b = self.byte_at(self.cursor);
        let step = self.text[..b]
            .graphemes(true)
            .next_back()
            .map_or(0, |g| g.chars().count());
        self.cursor - step
    }

    fn next_boundary(&self) -> usize {
        let b = self.byte_at(self.cursor);
        let step = self.text[b..]
            .graphemes(true)
            .next()
            .map_or(0, |g| g.chars().count());
        self.cursor + step
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        self.remove_range(self.prev_boundary(), self.cursor)
    }

    pub fn delete(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        self.remove_range(self.cursor, self.next_boundary())
    }

    pub fn delete_word(&mut self) -> bool {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == self.cursor {
            return false;
        }
        let (a, b) = (self.byte_at(start), self.byte_at(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = start;
        true
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    fn line_start(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1] != '\n' {
            i -= 1;
        }
        i
    }

    fn line_end(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && chars[i] != '\n' {
            i += 1;
        }
        i
    }

    fn remove_range(&mut self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        self.text.replace_range(a..b, "");
        self.cursor = from;
        true
    }

    fn move_to(&mut self, to: usize) -> Edit {
        if to == self.cursor {
            return Edit::Ignored;
        }
        self.cursor = to;
        Edit::Moved
    }

    fn move_line(&mut self, down: bool) -> Edit {
        let start = self.line_start();
        let col = self.cursor - start;
        let target = if down {
            let end = self.line_end();
            if end >= self.len() {
                return Edit::Ignored;
            }
            end + 1
        } else {
            if start == 0 {
                return Edit::Ignored;
            }
            self.cursor = start - 1;
            self.line_start()
        };
        self.cursor = target;
        self.cursor = (target + col).min(self.line_end());
        Edit::Moved
    }

    fn mention_start(&self) -> Option<usize> {
        let chars: Vec<char> = self.text.chars().take(self.cursor).collect();
        let start = chars
            .iter()
            .rposition(|c| c.is_whitespace())
            .map_or(0, |i| i + 1);
        (chars.get(start) == Some(&'@')).then_some(start)
    }

    pub fn mention_prefix(&self) -> Option<String> {
        let start = self.mention_start()?;
        Some(
            self.text
                .chars()
                .skip(start + 1)
                .take(self.cursor - start - 1)
                .collect(),
        )
    }

    pub fn complete_mention(&mut self, username: &str) {
        self.complete_mention_with(&format!("@{username}"));
    }

    pub fn complete_mention_with(&mut self, source: &str) {
        let Some(start) = self.mention_start() else {
            return;
        };
        self.remove_range(start, self.cursor);
        self.insert_str(&format!("{source} "));
    }

    pub fn handle_key(&mut self, key: &KeyEvent, allow_newline: bool) -> Edit {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let motion = matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
        );
        if allow_newline && shift && motion {
            let anchor = self.anchor.unwrap_or(self.cursor);
            let mut plain = *key;
            plain.modifiers.remove(KeyModifiers::SHIFT);
            let edit = self.apply_key(&plain, allow_newline);
            self.anchor = (self.cursor != anchor).then_some(anchor);
            return edit;
        }
        if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) && self.delete_selection() {
            return Edit::Changed;
        }
        let edit = self.apply_key(key, allow_newline);
        if motion || !matches!(edit, Edit::Ignored) {
            self.anchor = None;
        }
        edit
    }

    fn apply_key(&mut self, key: &KeyEvent, allow_newline: bool) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('j') if ctrl && allow_newline => {
                self.insert('\n');
                Edit::Changed
            }
            KeyCode::Enter
                if allow_newline
                    && key.modifiers.intersects(
                        KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL,
                    ) =>
            {
                self.insert('\n');
                Edit::Changed
            }
            KeyCode::Char('w') if ctrl => bool_edit(self.delete_word()),
            KeyCode::Char('h') if ctrl => bool_edit(self.remove_range(self.word_left(), self.cursor)),
            KeyCode::Backspace if alt || ctrl => {
                bool_edit(self.remove_range(self.word_left(), self.cursor))
            }
            KeyCode::Delete if alt || ctrl => {
                bool_edit(self.remove_range(self.cursor, self.word_right()))
            }
            KeyCode::Char('d') if alt => bool_edit(self.remove_range(self.cursor, self.word_right())),
            KeyCode::Char('u') if ctrl => bool_edit(self.remove_range(self.line_start(), self.cursor)),
            KeyCode::Char('a') if ctrl => self.move_to(self.line_start()),
            KeyCode::Char('e') if ctrl => self.move_to(self.line_end()),
            KeyCode::Left if ctrl || alt => self.move_to(self.word_left()),
            KeyCode::Right if ctrl || alt => self.move_to(self.word_right()),
            KeyCode::Char('b') if alt => self.move_to(self.word_left()),
            KeyCode::Char('f') if alt => self.move_to(self.word_right()),
            KeyCode::Home if ctrl => self.move_to(0),
            KeyCode::End if ctrl => self.move_to(self.len()),
            KeyCode::Char(_) if alt => Edit::Ignored,
            KeyCode::Char(c) if !ctrl => {
                self.insert(c);
                Edit::Changed
            }
            KeyCode::Backspace => bool_edit(self.backspace()),
            KeyCode::Delete => bool_edit(self.delete()),
            KeyCode::Left => {
                self.cursor = self.prev_boundary();
                Edit::Moved
            }
            KeyCode::Right => {
                self.cursor = self.next_boundary();
                Edit::Moved
            }
            KeyCode::Home => self.move_to(self.line_start()),
            KeyCode::End => self.move_to(self.line_end()),
            KeyCode::Up => self.move_line(false),
            KeyCode::Down => self.move_line(true),
            _ => Edit::Ignored,
        }
    }

    pub fn cursor_position(&self, width: usize) -> (usize, usize) {
        let cur = self.byte_at(self.cursor);
        let mut row = 0;
        let mut start = 0;
        for line in self.text.split('\n') {
            let end = start + line.len();
            let rows = wrap_line(line, width);
            if cur <= end {
                let mut at = start;
                for (i, r) in rows.iter().enumerate() {
                    let r_end = at + r.len();
                    if cur < r_end || i + 1 == rows.len() {
                        let col = str_width(&self.text[at..cur]);
                        return if width > 0 && col >= width {
                            (row + i + 1, 0)
                        } else {
                            (row + i, col)
                        };
                    }
                    at = r_end;
                }
            }
            row += rows.len();
            start = end + 1;
        }
        (row, 0)
    }

    pub fn visual_lines(&self, width: usize) -> Vec<String> {
        self.text
            .split('\n')
            .flat_map(|line| wrap_line(line, width))
            .map(str::to_owned)
            .collect()
    }
}

fn str_width(s: &str) -> usize {
    s.graphemes(true).map(|g| g.width()).sum()
}

fn wrap_line(line: &str, width: usize) -> Vec<&str> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let mut brk: Option<(usize, usize)> = None;
    for (b, g) in line.grapheme_indices(true) {
        let gw = g.width();
        let space = g.chars().all(char::is_whitespace);
        if width > 0 && !space && used + gw > width {
            if let Some((at, w)) = brk.filter(|&(at, _)| at > start) {
                rows.push(&line[start..at]);
                start = at;
                used -= w;
            }
            if used + gw > width && b > start {
                rows.push(&line[start..b]);
                start = b;
                used = 0;
            }
            brk = None;
        }
        used += gw;
        if space {
            brk = Some((b + g.len(), used));
        }
    }
    rows.push(&line[start..]);
    rows
}

fn bool_edit(changed: bool) -> Edit {
    if changed {
        Edit::Changed
    } else {
        Edit::Ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn edits_in_the_middle_with_unicode() {
        let mut t = TextInput::with_text("прив");
        t.handle_key(&key(KeyCode::Left, KeyModifiers::NONE), false);
        t.insert('е');
        assert_eq!(t.text(), "приев");
        t.backspace();
        t.delete();
        assert_eq!(t.text(), "при");
    }

    #[test]
    fn word_deletion_and_newlines() {
        let mut t = TextInput::with_text("hello big world");
        t.handle_key(&key(KeyCode::Char('w'), KeyModifiers::CONTROL), true);
        assert_eq!(t.text(), "hello big ");
        t.handle_key(&key(KeyCode::Char('j'), KeyModifiers::CONTROL), true);
        assert_eq!(t.text(), "hello big \n");
        t.handle_key(&key(KeyCode::Char('j'), KeyModifiers::CONTROL), false);
        assert_eq!(t.text(), "hello big \n");
        assert_eq!(t.cursor_position(80), (1, 0));
    }

    #[test]
    fn word_and_line_motions() {
        let ctrl = KeyModifiers::CONTROL;
        let mut t = TextInput::with_text("привет, мир\nвторая строка");
        t.handle_key(&key(KeyCode::Left, ctrl), true);
        t.handle_key(&key(KeyCode::Left, ctrl), true);
        t.insert('X');
        assert_eq!(t.text(), "привет, мир\nXвторая строка");
        t.handle_key(&key(KeyCode::Up, KeyModifiers::NONE), true);
        t.handle_key(&key(KeyCode::End, KeyModifiers::NONE), true);
        t.handle_key(&key(KeyCode::Backspace, ctrl), true);
        assert_eq!(t.text(), "привет, \nXвторая строка");
        t.handle_key(&key(KeyCode::Home, KeyModifiers::NONE), true);
        t.handle_key(&key(KeyCode::Delete, ctrl), true);
        assert_eq!(t.text(), ", \nXвторая строка");
        t.handle_key(&key(KeyCode::Down, KeyModifiers::NONE), true);
        t.handle_key(&key(KeyCode::Right, ctrl), true);
        t.handle_key(&key(KeyCode::Char('u'), ctrl), true);
        assert_eq!(t.text(), ", \n строка");
        t.handle_key(&key(KeyCode::Enter, KeyModifiers::SHIFT), true);
        assert_eq!(t.text(), ", \n\n строка");
    }

    #[test]
    fn mention_prefix_is_the_word_under_the_cursor_and_completes_in_place() {
        let mut t = TextInput::with_text("привет @tr");
        assert_eq!(t.mention_prefix().as_deref(), Some("tr"));
        t.complete_mention("trito");
        assert_eq!(t.text(), "привет @trito ");
        assert_eq!(t.mention_prefix(), None);
        assert_eq!(TextInput::with_text("mail a@b").mention_prefix(), None);
    }

    #[test]
    fn visual_lines_wrap() {
        let t = TextInput::with_text("abcdef\ngh");
        assert_eq!(t.visual_lines(4), vec!["abcd", "ef", "gh"]);
        assert_eq!(t.cursor_position(4), (2, 2));
    }

    #[test]
    fn visual_lines_wrap_by_word_and_keep_the_cursor_in_step() {
        let mut t = TextInput::with_text("hello big world abcdefghij");
        assert_eq!(
            t.visual_lines(8),
            vec!["hello ", "big ", "world ", "abcdefgh", "ij"]
        );
        assert_eq!(t.cursor_position(8), (4, 2));
        t.splice(0, 0, "", 9);
        assert_eq!(t.cursor_position(8), (1, 3));
        t.handle_key(&key(KeyCode::Right, KeyModifiers::NONE), true);
        assert_eq!(t.cursor_position(8), (2, 0));
        let chars: usize = t.visual_lines(8).iter().map(|l| l.chars().count()).sum();
        assert_eq!(chars, t.text().chars().count());
    }

    #[test]
    fn graphemes_move_and_delete_as_one_unit() {
        let flag = "\u{1F1FA}\u{1F1E6}";
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let thumb = "\u{1F44D}\u{1F3FD}";
        let heart = "\u{2764}\u{FE0F}";
        let mut t = TextInput::with_text(&format!("a{flag}{family}{thumb}{heart}"));
        assert_eq!(t.cursor_position(0).1, 1 + 2 + 2 + 2 + 2);
        t.backspace();
        assert_eq!(t.text(), format!("a{flag}{family}{thumb}"));
        press(&mut t, KeyCode::Left, KeyModifiers::NONE);
        press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(selected(&t), Some(family.to_owned()));
        press(&mut t, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(t.text(), format!("a{flag}{thumb}"));
        press(&mut t, KeyCode::Left, KeyModifiers::NONE);
        t.delete();
        assert_eq!(t.text(), format!("a{thumb}"));
        t.insert_str("x\r\ny");
        assert_eq!(t.text(), format!("ax\ny{thumb}"));
        assert_eq!(t.cursor(), 4);
    }

    fn selected(t: &TextInput) -> Option<String> {
        t.selection().map(|(a, b)| t.slice(a, b))
    }

    fn press(t: &mut TextInput, code: KeyCode, mods: KeyModifiers) {
        t.handle_key(&key(code, mods), true);
    }

    #[test]
    fn shift_arrows_extend_the_selection() {
        let mut t = TextInput::with_text("hello world");
        assert_eq!(t.selection(), None);
        press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), Some((9, 11)));
        assert_eq!(selected(&t).as_deref(), Some("ld"));
        press(&mut t, KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), Some((10, 11)));
        press(&mut t, KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn shift_home_end_and_vertical_moves_select() {
        let mut t = TextInput::with_text("ab\ncd");
        press(&mut t, KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), Some((3, 5)));
        press(&mut t, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), Some((0, 5)));
        press(&mut t, KeyCode::Down, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), Some((3, 5)));
        press(&mut t, KeyCode::End, KeyModifiers::SHIFT);
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn ctrl_shift_arrows_select_by_word() {
        let mut t = TextInput::with_text("one two three");
        let mods = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        press(&mut t, KeyCode::Left, mods);
        assert_eq!(selected(&t).as_deref(), Some("three"));
        press(&mut t, KeyCode::Left, mods);
        assert_eq!(selected(&t).as_deref(), Some("two three"));
        press(&mut t, KeyCode::Right, mods);
        assert_eq!(selected(&t).as_deref(), Some(" three"));
    }

    #[test]
    fn plain_moves_clear_the_selection() {
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Up,
        ] {
            let mut t = TextInput::with_text("hello");
            press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
            assert!(t.selection().is_some());
            press(&mut t, code, KeyModifiers::NONE);
            assert_eq!(t.selection(), None, "{code:?}");
        }
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut t = TextInput::with_text("hello world");
        press(
            &mut t,
            KeyCode::Left,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        press(&mut t, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(t.text(), "hello x");
        assert_eq!(t.selection(), None);
        assert_eq!(t.cursor(), 7);
    }

    #[test]
    fn paste_replaces_the_selection() {
        let mut t = TextInput::with_text("abcdef");
        t.select(1, 4);
        t.insert_str("XY");
        assert_eq!(t.text(), "aXYef");
        assert_eq!(t.cursor(), 3);
    }

    #[test]
    fn backspace_and_delete_remove_the_selection() {
        for code in [KeyCode::Backspace, KeyCode::Delete] {
            let mut t = TextInput::with_text("abcdef");
            t.select(4, 1);
            press(&mut t, code, KeyModifiers::NONE);
            assert_eq!(t.text(), "aef", "{code:?}");
            assert_eq!(t.cursor(), 1);
            assert_eq!(t.selection(), None);
        }
    }

    #[test]
    fn newline_replaces_the_selection_and_shift_enter_still_works() {
        let mut t = TextInput::with_text("abcd");
        t.select(1, 3);
        press(&mut t, KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(t.text(), "a\nd");
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn single_line_inputs_never_start_a_selection() {
        let mut t = TextInput::with_text("hello");
        t.handle_key(&key(KeyCode::Left, KeyModifiers::SHIFT), false);
        assert_eq!(t.selection(), None);
        assert_eq!(t.cursor(), 4);
    }

    #[test]
    fn selection_by_unicode_chars() {
        let mut t = TextInput::with_text("привет 😀");
        press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut t, KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(selected(&t).as_deref(), Some(" 😀"));
    }

    #[test]
    fn splice_replaces_text_and_places_the_cursor() {
        let mut t = TextInput::with_text("abcdef");
        t.select(0, 2);
        t.splice(2, 4, "XYZ", 4);
        assert_eq!(t.text(), "abXYZef");
        assert_eq!(t.cursor(), 4);
        assert_eq!(t.selection(), None);
    }
}
