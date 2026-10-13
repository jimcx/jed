//! A view onto a buffer: its selections and scroll position.

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::selection::{Selection, Selections};

#[derive(Debug, Clone)]
pub struct View {
    pub selections: Selections,
    top_line: usize,
    /// Start of `top_line`, so the viewport can follow edits made in other views.
    top_byte: ByteIdx,
    pub left_col: usize,
    /// Text area size from the last render, used by page movement.
    pub height: usize,
    pub width: usize,
    /// Set by commands that move the cursor; the next render scrolls it into view.
    pub reveal_cursor: bool,
}

impl View {
    pub fn new() -> Self {
        Self {
            selections: Selections::single(Selection::cursor(ByteIdx(0))),
            top_line: 0,
            top_byte: ByteIdx(0),
            left_col: 0,
            height: 1,
            width: 1,
            reveal_cursor: true,
        }
    }

    pub fn top_line(&self) -> usize {
        self.top_line
    }

    pub fn set_top_line(&mut self, rope: &Rope, line: usize) {
        self.top_line = line.min(pos::line_count(rope) - 1);
        self.top_byte = pos::line_start(rope, self.top_line);
    }

    pub fn scroll_lines(&mut self, rope: &Rope, delta: isize) {
        self.set_top_line(rope, self.top_line.saturating_add_signed(delta));
    }

    /// Scrolls just enough to show the primary cursor.
    pub fn scroll_to_cursor(&mut self, rope: &Rope, tab_width: usize) {
        let head = self.selections.primary().head;
        let line = pos::line_of(rope, head);
        if line < self.top_line {
            self.set_top_line(rope, line);
        } else if line >= self.top_line + self.height {
            self.set_top_line(rope, line + 1 - self.height);
        }
        let col = pos::display_col(rope, head, tab_width).0;
        if col < self.left_col {
            self.left_col = col;
        } else if col >= self.left_col + self.width {
            self.left_col = col + 1 - self.width;
        }
    }

    /// Follows edits made through another view on the same buffer: selections and the
    /// viewport move with the text. `changes` are in the order applied; `rope` is the result.
    pub fn map_through(&mut self, changes: &[ChangeSet], rope: &Rope) {
        let len = rope.len();
        let map = |p: ByteIdx| {
            let p = changes.iter().fold(p, |p, c| c.map(p, Assoc::Before));
            ByteIdx(rope.floor_char_boundary(p.0.min(len)))
        };
        self.selections.transform(|s| Selection {
            anchor: map(s.anchor),
            head: map(s.head),
            goal: s.goal,
        });
        let top = map(self.top_byte);
        self.set_top_line(rope, pos::line_of(rope, top));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::edit::{self, Settings};

    fn cursor(at: usize) -> Selections {
        Selections::single(Selection::cursor(ByteIdx(at)))
    }

    /// Two views on one buffer: edits (and undo) through `a` keep `b` on the same text.
    #[test]
    fn other_view_follows_edits_and_undo() {
        let mut buf = Buffer::from_str("one\ntwo\nthree\n");
        let mut a = View::new();
        let mut b = View::new();
        a.selections = cursor(0);
        b.selections = Selections::new(
            vec![Selection::cursor(ByteIdx(4)), Selection::new(ByteIdx(8), ByteIdx(13))],
            0,
        );
        b.set_top_line(&buf.rope, 1);

        edit::paste(&mut buf, &mut a, "zero\n");
        b.map_through(&buf.take_applied(), &buf.rope);
        let heads: Vec<usize> = b.selections.iter().map(|s| s.head.0).collect();
        assert_eq!(heads, vec![9, 18], "both cursors after the insert moved");
        assert_eq!(b.top_line(), 2, "viewport still starts at `two`");

        edit::run(crate::command::Command::Undo, &mut buf, &mut a, &Settings::default());
        b.map_through(&buf.take_applied(), &buf.rope);
        let got: Vec<(usize, usize)> = b.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(4, 4), (8, 13)]);
        assert_eq!(b.top_line(), 1);
    }

    #[test]
    fn deleting_text_under_another_cursor_keeps_it_valid() {
        let mut buf = Buffer::from_str("héllo world");
        let mut a = View::new();
        let mut b = View::new();
        b.selections = cursor(3); // after "hé"
        a.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(5)));
        edit::run(
            crate::command::Command::DeleteLeft,
            &mut buf,
            &mut a,
            &Settings::default(),
        );
        b.map_through(&buf.take_applied(), &buf.rope);
        assert_eq!(b.selections.primary().head, ByteIdx(0));
        assert_eq!(buf.rope.to_string(), "o world");
    }

    #[test]
    fn insert_at_another_cursor_leaves_it_before() {
        let mut buf = Buffer::from_str("ab");
        let mut a = View::new();
        let mut b = View::new();
        a.selections = cursor(1);
        b.selections = cursor(1);
        edit::insert_char(&mut buf, &mut a, 'X');
        b.map_through(&buf.take_applied(), &buf.rope);
        assert_eq!(b.selections.primary().head, ByteIdx(1));
        assert_eq!(a.selections.primary().head, ByteIdx(2));
    }
}
