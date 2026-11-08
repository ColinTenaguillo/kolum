//! A one-line text field: the picker's prompt, the `:` and `/` lines.
//! Its editing keys are fixed (readline's), and only see what the keymap
//! left unhandled, so a binding like `<C-u>` in the picker wins over them.

use crate::keymap::Key;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    /// In characters, not bytes.
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.chars().count();
    }

    fn byte(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(b, _)| b)
    }

    /// True when the text changed.
    pub fn handle(&mut self, key: Key) -> bool {
        let ctrl = key.mods.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) if key.mods.is_empty() => self.insert(c),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.delete()
            }
            KeyCode::Delete if self.cursor < self.text.chars().count() => self.delete(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char('u') if ctrl => self.delete_to_start(),
            code => self.move_cursor(code),
        }
    }

    fn insert(&mut self, c: char) -> bool {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
        true
    }

    /// The character under the cursor.
    fn delete(&mut self) -> bool {
        let at = self.byte(self.cursor);
        self.text.remove(at);
        true
    }

    /// The word before the cursor, and the spaces after it.
    fn delete_word(&mut self) -> bool {
        let before: Vec<char> = self.text.chars().take(self.cursor).collect();
        let trimmed = before.iter().rposition(|c| !c.is_whitespace()).map_or(0, |i| i + 1);
        let start = before[..trimmed].iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
        let (a, b) = (self.byte(start), self.byte(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = start;
        a != b
    }

    fn delete_to_start(&mut self) -> bool {
        let b = self.byte(self.cursor);
        self.text.replace_range(..b, "");
        self.cursor = 0;
        b > 0
    }

    /// Moves change no text: false, for every key.
    fn move_cursor(&mut self, code: KeyCode) -> bool {
        let len = self.text.chars().count();
        self.cursor = match code {
            KeyCode::Left => self.cursor.saturating_sub(1),
            KeyCode::Right => (self.cursor + 1).min(len),
            KeyCode::Home => 0,
            KeyCode::End => len,
            _ => self.cursor,
        };
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::parse_keys;

    fn typed(keys: &str) -> Input {
        let mut input = Input::default();
        for k in parse_keys(keys, Key::char(' ')).unwrap() {
            input.handle(k);
        }
        input
    }

    #[test]
    fn typing_editing_and_moving() {
        assert_eq!(typed("héllo").text(), "héllo");
        assert_eq!(typed("abc<BS>").text(), "ab");
        assert_eq!(typed("ac<Left>b").text(), "abc");
        assert_eq!(typed("abc<Home><Del>").text(), "bc");
        assert_eq!(typed("abc<Home><End>d").text(), "abcd");
        assert_eq!(typed("<BS><Del>x").text(), "x");
    }

    #[test]
    fn ctrl_w_deletes_a_word_and_ctrl_u_the_line_before_the_cursor() {
        let w = typed("login time  <C-w>");
        assert_eq!((w.text(), w.cursor()), ("login ", 6));
        assert_eq!(typed("ab cd<Left><Left><C-u>").text(), "cd");
    }
}
