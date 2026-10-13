//! A view onto a buffer: its selections and scroll position.

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};
use crate::selection::{Selection, Selections};

#[derive(Debug)]
pub struct View {
    pub selections: Selections,
    pub top_line: usize,
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
            left_col: 0,
            height: 1,
            width: 1,
            reveal_cursor: true,
        }
    }

    pub fn scroll_lines(&mut self, rope: &Rope, delta: isize) {
        let max_top = pos::line_count(rope).saturating_sub(1);
        self.top_line = self.top_line.saturating_add_signed(delta).min(max_top);
    }

    /// Scrolls just enough to show the primary cursor.
    pub fn scroll_to_cursor(&mut self, rope: &Rope, tab_width: usize) {
        let head = self.selections.primary().head;
        let line = pos::line_of(rope, head);
        if line < self.top_line {
            self.top_line = line;
        } else if line >= self.top_line + self.height {
            self.top_line = line + 1 - self.height;
        }
        let col = pos::display_col(rope, head, tab_width).0;
        if col < self.left_col {
            self.left_col = col;
        } else if col >= self.left_col + self.width {
            self.left_col = col + 1 - self.width;
        }
    }
}
