use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    text: String,
    cursor: usize,
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
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
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
        let b = self.byte_at(self.cursor);
        self.text.insert(b, ch);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            if ch != '\r' {
                self.insert(ch);
            }
        }
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        let b = self.byte_at(self.cursor);
        self.text.remove(b);
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.len() {
            return false;
        }
        let b = self.byte_at(self.cursor);
        self.text.remove(b);
        true
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
        let Some(start) = self.mention_start() else {
            return;
        };
        self.remove_range(start, self.cursor);
        self.insert_str(&format!("@{username} "));
    }

    pub fn handle_key(&mut self, key: &KeyEvent, allow_newline: bool) -> Edit {
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
                self.cursor = self.cursor.saturating_sub(1);
                Edit::Moved
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.len());
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
        let before: String = self.text.chars().take(self.cursor).collect();
        let mut row = 0;
        let mut col = 0;
        for line in before.split('\n').enumerate() {
            if line.0 > 0 {
                row += 1;
            }
            let w = line.1.width();
            match (w.checked_div(width), w.checked_rem(width)) {
                (Some(r), Some(c)) => {
                    row += r;
                    col = c;
                }
                _ => col = w,
            }
        }
        (row, col)
    }

    pub fn visual_lines(&self, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        for line in self.text.split('\n') {
            if width == 0 || line.width() <= width {
                out.push(line.to_owned());
                continue;
            }
            let mut cur = String::new();
            let mut w = 0;
            for ch in line.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if w + cw > width {
                    out.push(std::mem::take(&mut cur));
                    w = 0;
                }
                cur.push(ch);
                w += cw;
            }
            out.push(cur);
        }
        out
    }
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
}
