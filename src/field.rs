//! A one-line text field, used by the prompts, the palette and the find widget.

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
    /// All the text is selected (the find widget on opening): typing or pasting replaces it,
    /// Backspace / Delete clear it, any other key drops the selection.
    pub all_selected: bool,
}

impl TextField {
    /// Cursor at the end.
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            all_selected: false,
        }
    }

    /// Selects all the text, with the cursor at its end.
    pub fn select_all(&mut self) {
        self.cursor = self.text.len();
        self.all_selected = !self.text.is_empty();
    }

    pub fn insert(&mut self, s: &str) {
        self.clear_selected();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let prev = |s: &str, i: usize| s[..i].char_indices().last().map_or(0, |(j, _)| j);
        let next = |s: &str, i: usize| s[i..].chars().next().map_or(i, |c| i + c.len_utf8());
        if self.all_selected && matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
            self.clear_selected();
            return Outcome::Edited;
        }
        if self.all_selected && matches!(key.code, KeyCode::Left | KeyCode::Home) {
            self.all_selected = false;
            self.cursor = 0;
            return Outcome::Moved;
        }
        if !matches!(key.code, KeyCode::Char(_)) {
            self.all_selected = false;
        }
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

    /// Removes the selected text, if any.
    fn clear_selected(&mut self) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
            self.cursor = 0;
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

    #[test]
    fn selected_text_is_replaced_or_cleared() {
        let mut f = TextField::new("abc".into());
        f.select_all();
        assert_eq!(f.key(key(KeyCode::Char('x'))), Outcome::Edited);
        assert_eq!((f.text.as_str(), f.cursor, f.all_selected), ("x", 1, false));

        f.select_all();
        assert_eq!(f.key(key(KeyCode::Backspace)), Outcome::Edited);
        assert_eq!(f.text, "");

        let mut f = TextField::new("abc".into());
        f.select_all();
        assert_eq!(f.key(key(KeyCode::Left)), Outcome::Moved);
        assert_eq!((f.cursor, f.all_selected), (0, false));
        f.select_all();
        assert_eq!(f.key(key(KeyCode::End)), Outcome::Moved);
        assert!(!f.all_selected);
        f.select_all();
        f.insert("yz");
        assert_eq!(f.text, "yz");
        let mut empty = TextField::default();
        empty.select_all();
        assert!(!empty.all_selected);
    }
}
