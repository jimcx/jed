//! A one-line text field, used by the prompts and the palette.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not a field key.
    Ignored,
    Moved,
    Edited,
}

#[derive(Debug, Clone, Default)]
pub struct TextField {
    pub text: String,
    /// Byte offset, on a char boundary.
    pub cursor: usize,
}

impl TextField {
    /// Cursor at the end.
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
        }
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let prev = |s: &str, i: usize| s[..i].char_indices().last().map_or(0, |(j, _)| j);
        let next = |s: &str, i: usize| s[i..].chars().next().map_or(i, |c| i + c.len_utf8());
        match key.code {
            KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.insert(&c.to_string());
                Outcome::Edited
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let p = prev(&self.text, self.cursor);
                self.text.replace_range(p..self.cursor, "");
                self.cursor = p;
                Outcome::Edited
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                let n = next(&self.text, self.cursor);
                self.text.replace_range(self.cursor..n, "");
                Outcome::Edited
            }
            KeyCode::Left => {
                self.cursor = prev(&self.text, self.cursor);
                Outcome::Moved
            }
            KeyCode::Right => {
                self.cursor = next(&self.text, self.cursor);
                Outcome::Moved
            }
            KeyCode::Home => {
                self.cursor = 0;
                Outcome::Moved
            }
            KeyCode::End => {
                self.cursor = self.text.len();
                Outcome::Moved
            }
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn editing() {
        let mut f = TextField::new("ab".into());
        assert_eq!(f.key(key(KeyCode::Left)), Outcome::Moved);
        assert_eq!(f.key(key(KeyCode::Char('é'))), Outcome::Edited);
        assert_eq!((f.text.as_str(), f.cursor), ("aéb", 3));
        f.key(key(KeyCode::Backspace));
        f.key(key(KeyCode::Delete));
        assert_eq!((f.text.as_str(), f.cursor), ("a", 1));
        f.key(key(KeyCode::Home));
        f.key(key(KeyCode::Char('x')));
        assert_eq!(f.text, "xa");
        assert_eq!(f.key(key(KeyCode::Up)), Outcome::Ignored);
        assert_eq!(
            f.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Outcome::Ignored
        );
    }
}
