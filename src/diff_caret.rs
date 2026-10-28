//! The caret and text selection of a diff tab (task diff_edit): a line and a byte into one side's
//! `Lines`, one side active at a time, a selection staying in that side. The row cursor
//! (`DiffTab::cursor`) is always the row of the caret's head: `sync_caret` puts the caret back on
//! the cursor's row when something else moved the cursor (next change, a reload, a layout switch).
//! Nothing here edits; `DiffTab::editable_sides` says which sides later subtasks may edit.

use std::ops::Range;

use crate::buffer::pos::{self, DisplayCol};
use crate::command::Motion;
use crate::diff_view::{DiffTab, Row};

/// A position in a side: line, byte in the line (a char boundary).
pub type At = (usize, usize);

/// The caret: the active side, the selection's anchor and its head (the caret), and the display
/// column Up / Down aim for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub right: bool,
    pub anchor: At,
    pub head: At,
    pub col: Option<usize>,
}

impl Default for Caret {
    fn default() -> Caret {
        Caret {
            right: true,
            anchor: (0, 0),
            head: (0, 0),
            col: None,
        }
    }
}

impl Caret {
    /// The selection's ends, first then last.
    pub fn ordered(&self) -> (At, At) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected bytes of line `line` of side `right` (of `len` bytes), and whether the
    /// selection goes on past the line's end (its line break is selected). `None`: nothing of it.
    pub fn on_line(&self, right: bool, line: usize, len: usize) -> Option<(Range<usize>, bool)> {
        let (lo, hi) = self.ordered();
        if right != self.right || self.is_empty() || line < lo.0 || line > hi.0 {
            return None;
        }
        let start = if line == lo.0 { lo.1 } else { 0 };
        let end = if line == hi.0 { hi.1 } else { len };
        let past = line < hi.0;
        (start < end || past).then_some((start..end, past))
    }
}

/// The line `row` shows on a side: side by side that side's; inline the one drawn (a removed
/// line on the left, every other row's on the right), so the other side has none.
pub fn side_line(row: &Row, right: bool, inline: bool) -> Option<usize> {
    if inline {
        match (right, row.right) {
            (true, r) => r,
            (false, None) => row.left,
            (false, Some(_)) => None,
        }
    } else if right {
        row.right
    } else {
        row.left
    }
}

/// The side and line an inline row draws.
fn drawn(row: &Row) -> (bool, usize) {
    match (row.right, row.left) {
        (Some(r), _) => (true, r),
        (None, l) => (false, l.unwrap_or_default()),
    }
}

impl DiffTab {
    /// The text of line `line` of a side.
    fn side_text(&self, right: bool, line: usize) -> &str {
        match &self.doc {
            Some(d) if right => d.right.line(line),
            Some(d) => d.left.line(line),
            None => "",
        }
    }

    fn side_len(&self, right: bool) -> usize {
        match &self.doc {
            Some(d) if right => d.right.len(),
            Some(d) => d.left.len(),
            None => 0,
        }
    }

    /// Puts the caret back on the cursor's row when it isn't there (the cursor moved by other
    /// means, or the diff reloaded or changed layout), and inside its line's text.
    pub fn sync_caret(&mut self) {
        let Some(row) = self.rows().get(self.cursor).copied() else {
            return;
        };
        let c = self.caret;
        // Inline, a Same row also holds its left line (Select All can leave the cursor there).
        if side_line(&row, c.right, false) == Some(c.head.0) && c.anchor.0 < self.side_len(c.right) {
            let clamp = |t: &Self, (l, b): At| (l, t.side_text(c.right, l).floor_char_boundary(b));
            let (anchor, head) = (clamp(self, c.anchor), clamp(self, c.head));
            self.caret.anchor = anchor;
            self.caret.head = head;
            return;
        }
        let (right, line) = match side_line(&row, c.right, self.inline) {
            Some(l) => (c.right, l),
            None => match side_line(&row, !c.right, self.inline) {
                Some(l) => (!c.right, l),
                None => return,
            },
        };
        self.caret = Caret {
            right,
            anchor: (line, 0),
            head: (line, 0),
            col: None,
        };
    }

    /// The nearest row after (`dir` 1) or before the row `row` showing a line on `right`
    /// (inline: any row, unless `strict`), as (row, side, line).
    fn step_row(&self, row: usize, dir: isize, right: bool, strict: bool) -> Option<(usize, bool, usize)> {
        let rows = self.rows();
        let mut i = row as isize + dir;
        while i >= 0 && (i as usize) < rows.len() {
            let r = &rows[i as usize];
            if self.inline && !strict {
                let (side, line) = drawn(r);
                return Some((i as usize, side, line));
            }
            if let Some(l) = side_line(r, right, self.inline) {
                return Some((i as usize, right, l));
            }
            i += dir;
        }
        None
    }

    /// The row nearest `row` (that one first, then alternately above and below) showing a line
    /// on `right`: (row, line).
    fn nearest_row(&self, row: usize, right: bool) -> Option<(usize, usize)> {
        let rows = self.rows();
        let row = row.min(rows.len().checked_sub(1)?);
        if let Some(l) = side_line(&rows[row], right, self.inline) {
            return Some((row, l));
        }
        for d in 1..rows.len() {
            for i in [row.checked_sub(d), Some(row + d).filter(|&i| i < rows.len())]
                .into_iter()
                .flatten()
            {
                if let Some(l) = side_line(&rows[i], right, self.inline) {
                    return Some((i, l));
                }
            }
        }
        None
    }

    /// The row showing line `line` of side `right`, searched from the cursor.
    fn find_row(&self, right: bool, line: usize) -> usize {
        let rows = self.rows();
        let dir: isize = if line >= self.caret.head.0 { 1 } else { -1 };
        let mut i = self.cursor as isize;
        while i >= 0 && (i as usize) < rows.len() {
            // Raw columns: inline, a Same row holds its left line too.
            if side_line(&rows[i as usize], right, false) == Some(line) {
                return i as usize;
            }
            i += dir;
        }
        self.cursor
    }

    /// The caret to `at` of side `right`, on row `row`; `select` keeps the anchor.
    fn go_row(&mut self, row: usize, right: bool, at: At, select: bool, keep_col: bool) {
        self.cursor = row;
        self.reveal = true;
        let c = &mut self.caret;
        c.right = right;
        c.head = at;
        if !select {
            c.anchor = at;
        }
        if !keep_col {
            c.col = None;
        }
    }

    fn go(&mut self, at: At, select: bool) {
        let right = self.caret.right;
        let row = self.find_row(right, at.0);
        self.go_row(row, right, at, select, false);
    }

    /// An editor movement: the caret moves (`select`: the anchor stays), `page` rows for Page
    /// Up / Down. A plain Left / Right with a selection collapses it to its edge.
    pub fn caret_move(&mut self, motion: Motion, select: bool, page: usize) {
        self.sync_caret();
        if self.doc.is_none() || self.rows().is_empty() {
            return;
        }
        let c = self.caret;
        let (lo, hi) = c.ordered();
        let (line, byte) = c.head;
        let text_len = self.side_text(c.right, line).len();
        // Whether a step may go to the other side's rows (inline), and a line's end / start.
        let strict = select;
        let end_of = |t: &Self, row: (usize, bool, usize)| (row.2, t.side_text(row.1, row.2).len());
        match motion {
            Motion::Left if !select && !c.is_empty() => self.go(lo, false),
            Motion::Right if !select && !c.is_empty() => self.go(hi, false),
            Motion::Left if byte > 0 => {
                let b = pos::prev_grapheme_in(self.side_text(c.right, line), byte);
                self.go((line, b), select);
            }
            Motion::Right if byte < text_len => {
                let b = pos::next_grapheme_in(self.side_text(c.right, line), byte);
                self.go((line, b), select);
            }
            Motion::Left | Motion::WordLeft if byte == 0 => {
                if let Some(r) = self.step_row(self.cursor, -1, c.right, strict) {
                    let at = end_of(self, r);
                    self.go_row(r.0, r.1, at, select, false);
                }
            }
            Motion::Right | Motion::WordRight if byte == text_len => {
                if let Some(r) = self.step_row(self.cursor, 1, c.right, strict) {
                    self.go_row(r.0, r.1, (r.2, 0), select, false);
                }
            }
            Motion::WordLeft => {
                let b = pos::word_left_in(self.side_text(c.right, line), byte);
                self.go((line, b), select);
            }
            Motion::WordRight => {
                let b = pos::word_right_in(self.side_text(c.right, line), byte);
                self.go((line, b), select);
            }
            Motion::Left | Motion::Right => {}
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let (dir, n) = match motion {
                    Motion::Up => (-1, 1),
                    Motion::Down => (1, 1),
                    Motion::PageUp => (-1, page.max(1)),
                    _ => (1, page.max(1)),
                };
                let tw = self.tab_width;
                let goal = c
                    .col
                    .unwrap_or_else(|| pos::col_before_in(self.side_text(c.right, line), byte, tw).0);
                let (mut row, mut right, mut at_line) = (self.cursor, c.right, line);
                let mut moved = false;
                for _ in 0..n {
                    match self.step_row(row, dir, right, strict) {
                        Some((r, s, l)) => (row, right, at_line, moved) = (r, s, l, true),
                        None => break,
                    }
                }
                let text = self.side_text(right, at_line);
                let b = if moved {
                    pos::byte_at_col_in(text, DisplayCol(goal), tw)
                } else if dir < 0 {
                    0
                } else {
                    text.len()
                };
                self.go_row(row, right, (at_line, b), select, true);
                self.caret.col = Some(goal);
            }
            Motion::LineStart => {
                let text = self.side_text(c.right, line);
                let first = text.len() - text.trim_start_matches([' ', '\t']).len();
                let b = if byte == first { 0 } else { first };
                self.go((line, b), select);
            }
            Motion::LineEnd => self.go((line, text_len), select),
            Motion::DocStart => {
                let row = self.find_row(c.right, 0);
                self.go_row(row, c.right, (0, 0), select, false);
            }
            Motion::DocEnd => {
                let last = self.side_len(c.right).saturating_sub(1);
                let row = self.find_row(c.right, last);
                let len = self.side_text(c.right, last).len();
                self.go_row(row, c.right, (last, len), select, false);
            }
        }
        self.reveal_caret_col();
    }

    /// Scrolls sideways so the caret's column is in view.
    pub fn reveal_caret_col(&mut self) {
        let c = self.caret;
        if self.text_width == 0 {
            return;
        }
        let col = pos::col_before_in(self.side_text(c.right, c.head.0), c.head.1, self.tab_width).0;
        if col < self.left_col {
            self.left_col = col;
        } else if col >= self.left_col + self.text_width {
            self.left_col = col + 1 - self.text_width;
        }
    }

    /// A click or drag at row `row` and display column `col` (counted from the text's start,
    /// scrolled columns included) of side `right`. `select` (Shift, a drag) extends the selection
    /// in the caret's side; a plain click moves the caret to the row's line on that side, else
    /// to the line it shows on the other.
    pub fn caret_click(&mut self, row: usize, right: bool, col: usize, select: bool) {
        self.sync_caret();
        let tw = self.tab_width;
        let right = if select { self.caret.right } else { right };
        let target = if select {
            self.nearest_row(row, right).map(|(r, l)| (r, right, l))
        } else {
            let last = self.rows().len().saturating_sub(1);
            let r = &self.rows()[row.min(last)];
            let side = |s| side_line(r, s, self.inline).map(|l| (row.min(last), s, l));
            side(right).or_else(|| side(!right))
        };
        let Some((row, side, line)) = target else { return };
        let text = self.side_text(side, line);
        let byte = if side == right {
            pos::byte_near_col_in(text, DisplayCol(col), tw)
        } else {
            0
        };
        self.go_row(row, side, (line, byte), select, false);
    }

    /// Selects the word at the caret (double click).
    pub fn select_word(&mut self) {
        let (line, byte) = self.caret.head;
        let (a, b) = pos::word_range_in(self.side_text(self.caret.right, line), byte);
        self.caret.anchor = (line, a);
        self.caret.head = (line, b);
    }

    /// Selects the caret's line (triple click).
    pub fn select_line(&mut self) {
        let line = self.caret.head.0;
        let len = self.side_text(self.caret.right, line).len();
        self.caret.anchor = (line, 0);
        self.caret.head = (line, len);
    }

    /// Selects the active side's whole text (Cmd+A); the caret goes to its end.
    pub fn select_all(&mut self) {
        self.sync_caret();
        let right = self.caret.right;
        let n = self.side_len(right);
        if n == 0 {
            return;
        }
        let last = n - 1;
        let at = (last, self.side_text(right, last).len());
        let row = self.find_row(right, last);
        self.go_row(row, right, at, true, false);
        self.caret.anchor = (0, 0);
        self.reveal_caret_col();
    }

    /// Drops the selection, keeping the caret (Esc). False when there was none.
    pub fn clear_selection(&mut self) -> bool {
        self.sync_caret();
        let had = !self.caret.is_empty();
        self.caret.anchor = self.caret.head;
        had
    }

    /// The selected text (lines joined by `\n`); `None` when nothing is selected.
    pub fn selection_text(&mut self) -> Option<String> {
        self.sync_caret();
        let c = self.caret;
        if c.is_empty() {
            return None;
        }
        let (lo, hi) = c.ordered();
        let mut out = String::new();
        for line in lo.0..=hi.0 {
            let text = self.side_text(c.right, line);
            let (a, b) = (
                if line == lo.0 { lo.1 } else { 0 },
                if line == hi.0 { hi.1 } else { text.len() },
            );
            out.push_str(&text[a.min(text.len())..b.min(text.len())]);
            if line < hi.0 {
                out.push('\n');
            }
        }
        Some(out)
    }

    /// Whether the caret's side may be edited.
    pub fn caret_editable(&self) -> bool {
        self.editable_sides()[self.caret.right as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_view::{Doc, Pair};
    use std::sync::Arc;

    fn tab(old: &str, new: &str) -> DiffTab {
        let mut d = DiffTab::new(1, "f.txt".into(), Pair::UNSTAGED);
        d.doc = Some(Arc::new(Doc::new(old.into(), new.into()).unwrap()));
        d
    }

    #[test]
    fn caret_stays_in_its_side_and_follows_lines() {
        let mut d = tab("one\ntwo\nthree\n", "one\n2\nthree\nfour\n");
        d.caret_move(Motion::Down, false, 1);
        d.caret_move(Motion::LineEnd, false, 1);
        assert_eq!((d.caret.right, d.caret.head), (true, (1, 1)));
        d.caret_move(Motion::Right, false, 1);
        assert_eq!(d.caret.head, (2, 0), "Right at the end goes to the next line");
        assert_eq!(
            d.cursor,
            d.aligned().rows.iter().position(|r| r.right == Some(2)).unwrap()
        );
        d.caret_move(Motion::DocEnd, true, 1);
        assert_eq!((d.caret.anchor, d.caret.head), ((2, 0), (3, 4)));
        assert_eq!(d.selection_text().as_deref(), Some("three\nfour"));
    }

    #[test]
    fn up_down_keep_the_column_and_skip_filler() {
        let mut d = tab("abcdef\nx\nabcdef\n", "abcdef\nabcdef\n");
        d.caret_move(Motion::Right, false, 1);
        d.caret_move(Motion::Right, false, 1);
        d.caret_move(Motion::Right, false, 1);
        d.caret_move(Motion::Down, false, 1);
        assert_eq!(d.caret.head, (1, 3));
        d.caret_move(Motion::Up, false, 1);
        assert_eq!(d.caret.head, (0, 3));
    }

    #[test]
    fn selection_on_a_line() {
        let c = Caret {
            right: true,
            anchor: (1, 2),
            head: (3, 1),
            col: None,
        };
        assert_eq!(c.on_line(true, 0, 5), None);
        assert_eq!(c.on_line(true, 1, 5), Some((2..5, true)));
        assert_eq!(c.on_line(true, 2, 0), Some((0..0, true)));
        assert_eq!(c.on_line(true, 3, 5), Some((0..1, false)));
        assert_eq!(c.on_line(false, 2, 5), None);
    }

    #[test]
    fn only_the_current_text_is_editable() {
        let d = tab("a\n", "b\n");
        assert_eq!(d.editable_sides(), [false, true]);
        let mut staged = tab("a\n", "b\n");
        staged.pair = Pair::STAGED;
        assert_eq!(staged.editable_sides(), [false, false]);
        // A compare tab: each side that names an open buffer.
        use crate::diff_view::{Compare, CompareSide, Source};
        let mut cmp = tab("a\n", "b\n");
        cmp.compare = Some(Compare {
            sides: [
                CompareSide::new(Source::File("/x/a.txt".into())),
                CompareSide::new(Source::Open(7)),
            ],
        });
        assert_eq!(cmp.editable_sides(), [false, true]);
        cmp.compare.as_mut().unwrap().sides.swap(0, 1);
        assert_eq!(cmp.editable_sides(), [true, false]);
    }

    #[test]
    fn plain_left_right_collapse_a_selection_to_its_edge_and_home_toggles() {
        let mut d = tab("x\n", "  indented line\n");
        d.caret_move(Motion::LineEnd, false, 1);
        d.caret_move(Motion::WordLeft, true, 1);
        assert_eq!((d.caret.anchor, d.caret.head), ((0, 15), (0, 11)));
        d.caret_move(Motion::Right, false, 1);
        assert_eq!((d.caret.anchor, d.caret.head), ((0, 15), (0, 15)), "Right: the end");
        d.caret_move(Motion::WordLeft, true, 1);
        d.caret_move(Motion::Left, false, 1);
        assert_eq!((d.caret.anchor, d.caret.head), ((0, 11), (0, 11)), "Left: the start");
        d.caret_move(Motion::LineStart, false, 1);
        assert_eq!(d.caret.head, (0, 2), "Home: the first non-blank");
        d.caret_move(Motion::LineStart, false, 1);
        assert_eq!(d.caret.head, (0, 0), "Home again: the line's start");
    }
}
