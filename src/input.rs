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
            KeyCode::Backspace if alt => bool_edit(self.delete_word()),
            KeyCode::Char('u') if ctrl => {
                let b = self.byte_at(self.cursor);
                self.text.replace_range(..b, "");
                self.cursor = 0;
                Edit::Changed
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                Edit::Moved
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = self.len();
                Edit::Moved
            }
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
            KeyCode::Home => {
                self.cursor = 0;
                Edit::Moved
            }
            KeyCode::End => {
                self.cursor = self.len();
                Edit::Moved
            }
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
    fn visual_lines_wrap() {
        let t = TextInput::with_text("abcdef\ngh");
        assert_eq!(t.visual_lines(4), vec!["abcd", "ef", "gh"]);
        assert_eq!(t.cursor_position(4), (2, 2));
    }
}
