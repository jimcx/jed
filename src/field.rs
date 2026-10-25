//! A one-line text field, used by the prompts, the palette, the find widget and the Search
//! view's inputs: a cursor and an optional selection, edited as the editor edits one line.
//! Offsets are bytes into `text`, on grapheme boundaries (word and grapheme stops come from
//! `buffer::pos`'s `*_in` helpers, the editor's rules). The keys are the editor's: `App::field_key`
//! looks unbound keys up in the editor context and runs `FieldCommand::from_command`'s here.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use crate::buffer::pos;
use crate::command::{Command, Motion};

/// What a key did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not a field key (or it changed nothing).
    Ignored,
    Moved,
    Edited,
}

/// Where a field move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldMotion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Start,
    End,
}

/// What a field does (an editor command's meaning on one line: `from_command`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCommand {
    Move { motion: FieldMotion, select: bool },
    DeleteLeft,
    DeleteRight,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToStart,
    SelectAll,
}

impl FieldCommand {
    /// The field meaning of an editor command: moves along the line (line / document start and
    /// end are the field's ends; up / down / page moves have none), deletes and select all.
    /// Copy / cut / paste need the clipboard: `App::field_key` handles them.
    pub fn from_command(cmd: Command) -> Option<Self> {
        Some(match cmd {
            Command::Move { motion, select } => {
                let motion = match motion {
                    Motion::Left => FieldMotion::Left,
                    Motion::Right => FieldMotion::Right,
                    Motion::WordLeft => FieldMotion::WordLeft,
                    Motion::WordRight => FieldMotion::WordRight,
                    Motion::LineStart | Motion::DocStart => FieldMotion::Start,
                    Motion::LineEnd | Motion::DocEnd => FieldMotion::End,
                    Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => return None,
                };
                FieldCommand::Move { motion, select }
            }
            Command::DeleteLeft => FieldCommand::DeleteLeft,
            Command::DeleteRight => FieldCommand::DeleteRight,
            Command::DeleteWordLeft => FieldCommand::DeleteWordLeft,
            Command::DeleteWordRight => FieldCommand::DeleteWordRight,
            Command::DeleteToLineStart => FieldCommand::DeleteToStart,
            Command::SelectAll => FieldCommand::SelectAll,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextField {
    pub text: String,
    /// Byte offset, on a grapheme boundary.
    pub cursor: usize,
    /// The selection's other end (`selection()`): typing, pasting and deletes replace the
    /// selected text. `None` (or equal to `cursor`) is no selection.
    pub anchor: Option<usize>,
}

impl TextField {
    /// Cursor at the end.
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            anchor: None,
        }
    }

    /// Selects all the text, with the cursor at its end (the find widget on opening: typing
    /// replaces the query).
    pub fn select_all(&mut self) {
        self.cursor = self.text.len();
        self.anchor = (!self.text.is_empty()).then_some(0);
    }

    /// The selected byte range, if any text is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor.filter(|&a| a != self.cursor)?;
        Some(a.min(self.cursor)..a.max(self.cursor))
    }

    /// The byte offset at screen column `col` of the field's text, drawn scrolled sideways by
    /// `scroll` columns (`ui::find::draw_field`'s layout): the offset of the grapheme there, its
    /// start when the column falls inside a wide one, the text's end past it. `col` is relative
    /// to the input's left edge and may go past either end (a drag there scrolls the field): a
    /// negative `col` maps left of the first drawn column, a large one past the text's end.
    /// Grapheme boundaries, so a click never lands inside a character.
    pub fn offset_at_col(&self, col: i64, scroll: usize) -> usize {
        let target = (scroll as i64 + col).max(0);
        let mut at: i64 = 0;
        for (off, grapheme) in self.text.grapheme_indices(true) {
            if at >= target {
                return off;
            }
            let width = grapheme.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() as i64;
            if target < at + width {
                return off;
            }
            at += width;
        }
        self.text.len()
    }

    /// A click at byte offset `at`: `count` 1 places the cursor, 2 selects the word there (the
    /// editor's classifier, `pos::word_range_in`) and 3 all the text; `shift` with one click
    /// extends from the existing anchor (or the cursor) instead, as the editor does.
    pub fn click(&mut self, at: usize, count: u8, shift: bool) {
        let at = at.min(self.text.len());
        if shift && count == 1 {
            let anchor = self.anchor.unwrap_or(self.cursor);
            self.cursor = at;
            self.anchor = (anchor != at).then_some(anchor);
            return;
        }
        let (start, end) = match count {
            2 => pos::word_range_in(&self.text, at),
            3 => (0, self.text.len()),
            _ => (at, at),
        };
        self.cursor = end;
        self.anchor = (start != end).then_some(start);
    }

    /// A drag to byte offset `at`: extends the selection from the click's anchor (the cursor
    /// when there was none); back at the anchor the selection goes.
    pub fn drag_to(&mut self, at: usize) {
        let anchor = self.anchor.unwrap_or(self.cursor);
        let at = at.min(self.text.len());
        self.cursor = at;
        self.anchor = (anchor != at).then_some(anchor);
    }

    /// Inserts `s` at the cursor, replacing the selection.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Types a plain character (Shift allowed); every other key is `Ignored` (the editor
    /// context's commands do the rest, through `run`).
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.insert(c.encode_utf8(&mut [0; 4]));
                Outcome::Edited
            }
            _ => Outcome::Ignored,
        }
    }

    /// Runs a command with the editor's rules: a move without `select` collapses a selection
    /// (Left to its start, Right to its end; the others move from the cursor), with `select`
    /// extends it from the anchor; a delete with a selection deletes just that.
    pub fn run(&mut self, cmd: FieldCommand) -> Outcome {
        let (text, at) = (self.text.as_str(), self.cursor);
        match cmd {
            FieldCommand::Move { motion, select } => {
                let sel = self.selection();
                let to = match (motion, &sel) {
                    (FieldMotion::Left, Some(r)) if !select => r.start,
                    (FieldMotion::Right, Some(r)) if !select => r.end,
                    (FieldMotion::Left, _) => pos::prev_grapheme_in(text, at),
                    (FieldMotion::Right, _) => pos::next_grapheme_in(text, at),
                    (FieldMotion::WordLeft, _) => pos::word_left_in(text, at),
                    (FieldMotion::WordRight, _) => pos::word_right_in(text, at),
                    (FieldMotion::Start, _) => 0,
                    (FieldMotion::End, _) => text.len(),
                };
                self.anchor = if select { Some(self.anchor.unwrap_or(at)) } else { None };
                self.cursor = to;
                if self.anchor == Some(to) {
                    self.anchor = None;
                }
                Outcome::Moved
            }
            FieldCommand::SelectAll => {
                self.select_all();
                Outcome::Moved
            }
            FieldCommand::DeleteLeft
            | FieldCommand::DeleteRight
            | FieldCommand::DeleteWordLeft
            | FieldCommand::DeleteWordRight
            | FieldCommand::DeleteToStart => {
                if self.delete_selection() {
                    return Outcome::Edited;
                }
                let text = self.text.as_str();
                let to = match cmd {
                    FieldCommand::DeleteLeft => pos::prev_grapheme_in(text, at),
                    FieldCommand::DeleteRight => pos::next_grapheme_in(text, at),
                    FieldCommand::DeleteWordLeft => pos::word_left_in(text, at),
                    FieldCommand::DeleteWordRight => pos::word_right_in(text, at),
                    _ => 0,
                };
                if to == at {
                    return Outcome::Ignored;
                }
                self.text.replace_range(to.min(at)..to.max(at), "");
                self.cursor = to.min(at);
                Outcome::Edited
            }
        }
    }

    /// The selected text (`None` without a selection: unlike the editor, never the whole line).
    pub fn copy(&self) -> Option<String> {
        self.selection().map(|r| self.text[r].to_string())
    }

    /// The selected text, removed.
    pub fn cut(&mut self) -> Option<String> {
        let copied = self.copy()?;
        self.delete_selection();
        Some(copied)
    }

    /// Removes the selected text, if any; the cursor lands at its start. False without one.
    fn delete_selection(&mut self) -> bool {
        let Some(r) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(r.clone(), "");
        self.cursor = r.start;
        self.anchor = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mv(motion: FieldMotion) -> FieldCommand {
        FieldCommand::Move { motion, select: false }
    }

    fn sel(motion: FieldMotion) -> FieldCommand {
        FieldCommand::Move { motion, select: true }
    }

    /// A field with `|` marking the cursor and `[` the anchor.
    fn field(marked: &str) -> TextField {
        let anchor = marked.find('[');
        let plain: String = marked.replace('[', "");
        let cursor = plain.find('|').unwrap();
        let text = plain.replace('|', "");
        let anchor = anchor.map(|a| if a > cursor { a - 1 } else { a });
        TextField { text, cursor, anchor }
    }

    /// The field marked as `field` takes it (`[` only when something is selected).
    fn show(f: &TextField) -> String {
        let mut out = f.text.clone();
        let a = f.selection().map(|_| f.anchor.unwrap());
        let (c, a) = (f.cursor, a);
        match a {
            Some(a) if a > c => {
                out.insert(a, '[');
                out.insert(c, '|');
            }
            Some(a) => {
                out.insert(c, '|');
                out.insert(a, '[');
            }
            None => out.insert(c, '|'),
        }
        out
    }

    fn run(marked: &str, cmd: FieldCommand) -> (String, Outcome) {
        let mut f = field(marked);
        let out = f.run(cmd);
        (show(&f), out)
    }

    #[test]
    fn typing_and_other_keys() {
        let mut f = TextField::new("ab".into());
        f.cursor = 1;
        assert_eq!(f.key(key(KeyCode::Char('é'))), Outcome::Edited);
        assert_eq!(
            f.key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)),
            Outcome::Edited
        );
        assert_eq!(show(&f), "aéX|b");
        for k in [KeyCode::Left, KeyCode::Home, KeyCode::Backspace, KeyCode::Up] {
            assert_eq!(f.key(key(k)), Outcome::Ignored, "{k:?}: the editor context's");
        }
        let ctrl = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(f.key(ctrl), Outcome::Ignored);
        assert_eq!(show(&f), "aéX|b");
    }

    #[test]
    fn moves_by_grapheme_and_to_the_ends() {
        use FieldMotion::*;
        assert_eq!(run("aé|b", mv(Left)).0, "a|éb");
        assert_eq!(run("a|éb", mv(Right)).0, "aé|b");
        // A combining accent stays with its letter.
        assert_eq!(run("xe\u{301}|", mv(Left)).0, "x|e\u{301}");
        assert_eq!(run("|ab", mv(Left)), ("|ab".into(), Outcome::Moved));
        assert_eq!(run("ab|", mv(Right)).0, "ab|");
        assert_eq!(run("aé|b", mv(Start)).0, "|aéb");
        assert_eq!(run("a|éb", mv(End)).0, "aéb|");
    }

    #[test]
    fn word_moves_use_the_editors_stops() {
        use FieldMotion::*;
        assert_eq!(run("foo.bär  baz|", mv(WordLeft)).0, "foo.bär  |baz");
        assert_eq!(run("foo.bär  |baz", mv(WordLeft)).0, "foo.|bär  baz");
        assert_eq!(run("foo.|bär  baz", mv(WordLeft)).0, "foo|.bär  baz");
        assert_eq!(run("|foo.bär", mv(WordLeft)).0, "|foo.bär");
        assert_eq!(run("|foo.bär  baz", mv(WordRight)).0, "foo|.bär  baz");
        assert_eq!(run("foo.bär|  baz", mv(WordRight)).0, "foo.bär  baz|");
        assert_eq!(run("foo|", mv(WordRight)).0, "foo|");
    }

    #[test]
    fn a_move_collapses_the_selection_to_its_side() {
        use FieldMotion::*;
        assert_eq!(run("a[bé|c", mv(Left)).0, "a|béc");
        assert_eq!(run("a|bé[c", mv(Right)).0, "abé|c");
        assert_eq!(run("a[bé|c", mv(Start)).0, "|abéc");
        assert_eq!(run("a|bé[c", mv(End)).0, "abéc|");
        // Word moves go from the cursor.
        assert_eq!(run("ab [cd|", mv(WordLeft)).0, "ab |cd");
    }

    #[test]
    fn shift_moves_extend_from_the_anchor() {
        use FieldMotion::*;
        assert_eq!(run("ab|é", sel(Left)).0, "a|b[é");
        assert_eq!(run("a|b[é", sel(Left)).0, "|ab[é");
        assert_eq!(run("a|b[é", sel(Right)).0, "ab|é", "back to the anchor");
        assert_eq!(run("ab|é", sel(Start)).0, "|ab[é");
        assert_eq!(run("a[b|é", sel(End)).0, "a[bé|");
        assert_eq!(run("é[b|", sel(WordLeft)).0, "|é[b");
        // Back to the anchor: no selection.
        let mut f = field("a[b|");
        f.run(sel(Left));
        assert_eq!((f.selection(), show(&f)), (None, "a|b".into()));
    }

    #[test]
    fn deletes_without_a_selection() {
        use FieldCommand::*;
        assert_eq!(run("aé|b", DeleteLeft), ("a|b".into(), Outcome::Edited));
        assert_eq!(run("a|éb", DeleteRight), ("a|b".into(), Outcome::Edited));
        assert_eq!(run("|ab", DeleteLeft), ("|ab".into(), Outcome::Ignored));
        assert_eq!(run("ab|", DeleteRight), ("ab|".into(), Outcome::Ignored));
        assert_eq!(run("foo bär|", DeleteWordLeft).0, "foo |");
        assert_eq!(run("foo| bär", DeleteWordRight).0, "foo|");
        assert_eq!(run("fé o|x", DeleteToStart).0, "|x");
    }

    #[test]
    fn deletes_and_typing_replace_the_selection() {
        use FieldCommand::*;
        for cmd in [DeleteLeft, DeleteRight, DeleteWordLeft, DeleteWordRight, DeleteToStart] {
            assert_eq!(run("a[bé|c", cmd), ("a|c".into(), Outcome::Edited), "{cmd:?}");
        }
        let mut f = field("a|bé[c");
        f.insert("xy");
        assert_eq!(show(&f), "axy|c");
        let mut f = field("a|bé[c");
        assert_eq!(f.key(key(KeyCode::Char('ü'))), Outcome::Edited);
        assert_eq!(show(&f), "aü|c");
    }

    #[test]
    fn select_all_copy_and_cut() {
        let mut f = TextField::new("héllo".into());
        assert_eq!(f.copy(), None, "nothing selected: nothing copied");
        assert_eq!(f.run(FieldCommand::SelectAll), Outcome::Moved);
        assert_eq!((f.selection(), f.cursor), (Some(0..6), 6));
        assert_eq!(f.copy().as_deref(), Some("héllo"));
        (f.anchor, f.cursor) = (Some(0), 1);
        assert_eq!(f.cut().as_deref(), Some("h"));
        assert_eq!(show(&f), "|éllo");
        assert_eq!(f.cut(), None);
        let mut empty = TextField::default();
        empty.select_all();
        assert_eq!(empty.selection(), None);
    }

    #[test]
    fn offset_at_col_walks_the_drawn_widths() {
        let f = TextField::new("ab日c".into());
        assert_eq!(f.offset_at_col(0, 0), 0);
        assert_eq!(f.offset_at_col(1, 0), 1);
        assert_eq!(f.offset_at_col(2, 0), 2);
        assert_eq!(f.offset_at_col(3, 0), 2, "inside the wide char: its start");
        assert_eq!(f.offset_at_col(4, 0), 5, "after it: the next char");
        assert_eq!(f.offset_at_col(99, 0), 6, "past the end: the text's end");
        // The scroll is the first drawn character's display column.
        assert_eq!(f.offset_at_col(0, 2), 2);
        assert_eq!(f.offset_at_col(1, 2), 2, "inside the wide char through the scroll");
        assert_eq!(f.offset_at_col(2, 2), 5);
        // Negative columns (a drag past the left end) scroll left; past the start clamps to 0.
        assert_eq!(f.offset_at_col(-1, 2), 1);
        assert_eq!(f.offset_at_col(-99, 2), 0);
        // A combining accent is part of its grapheme, not a column of its own.
        let f = TextField::new("e\u{301}x".into());
        assert_eq!(f.offset_at_col(0, 0), 0);
        assert_eq!(f.offset_at_col(1, 0), 3);
    }

    #[test]
    fn clicks_place_select_a_word_or_everything() {
        let mut f = TextField::new("foo.bar baz".into());
        f.click(5, 1, false);
        assert_eq!((show(&f), f.selection()), ("foo.b|ar baz".into(), None));
        // Shift+click extends from the click's anchor.
        f.click(8, 1, true);
        assert_eq!((show(&f), f.selection()), ("foo.b[ar |baz".into(), Some(5..8)));
        // Shift+click again moves the head, keeping the anchor.
        f.click(2, 1, true);
        assert_eq!((show(&f), f.selection()), ("fo|o.b[ar baz".into(), Some(2..5)));
        // A double click takes the word's run, a triple the whole text.
        f.click(5, 2, false);
        assert_eq!((show(&f), f.selection()), ("foo.[bar| baz".into(), Some(4..7)));
        f.click(8, 2, false);
        assert_eq!((show(&f), f.selection()), ("foo.bar [baz|".into(), Some(8..11)));
        f.click(3, 3, false);
        assert_eq!((show(&f), f.selection()), ("[foo.bar baz|".into(), Some(0..11)));
        // A click past the end goes to the end; a shift click back to the anchor clears it.
        f.click(99, 1, false);
        assert_eq!((f.cursor, f.anchor), (11, None));
        let mut f = TextField::new("ab".into());
        f.click(0, 1, false);
        f.click(2, 1, true);
        assert_eq!((f.selection(), show(&f)), (Some(0..2), "[ab|".into()));
    }

    #[test]
    fn dragging_extends_the_selection_whichever_way() {
        let mut f = TextField::new("abcdef".into());
        f.click(2, 1, false);
        f.drag_to(5);
        assert_eq!((f.selection(), show(&f)), (Some(2..5), "ab[cde|f".into()));
        f.drag_to(1);
        assert_eq!((f.selection(), show(&f)), (Some(1..2), "a|b[cdef".into()));
        f.drag_to(2);
        assert_eq!((f.selection(), show(&f)), (None, "ab|cdef".into()));
        f.drag_to(99);
        assert_eq!((f.selection(), show(&f)), (Some(2..6), "ab[cdef|".into()));
        // A word double click drags the whole word's range from its start.
        let mut f = TextField::new("foo bar".into());
        f.click(1, 2, false);
        f.drag_to(5);
        assert_eq!((f.selection(), show(&f)), (Some(0..5), "[foo b|ar".into()));
    }

    #[test]
    fn editor_commands_with_a_field_meaning() {
        let m = |motion, select| Command::Move { motion, select };
        let f = |motion, select| Some(FieldCommand::Move { motion, select });
        assert_eq!(
            FieldCommand::from_command(m(Motion::LineStart, true)),
            f(FieldMotion::Start, true)
        );
        assert_eq!(
            FieldCommand::from_command(m(Motion::DocEnd, false)),
            f(FieldMotion::End, false)
        );
        assert_eq!(
            FieldCommand::from_command(m(Motion::WordLeft, false)),
            f(FieldMotion::WordLeft, false)
        );
        assert_eq!(FieldCommand::from_command(m(Motion::Up, false)), None);
        assert_eq!(FieldCommand::from_command(m(Motion::PageDown, true)), None);
        assert_eq!(
            FieldCommand::from_command(Command::DeleteToLineStart),
            Some(FieldCommand::DeleteToStart)
        );
        assert_eq!(FieldCommand::from_command(Command::Undo), None);
        assert_eq!(FieldCommand::from_command(Command::InsertNewline), None);
    }
}
