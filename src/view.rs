//! A view onto a buffer: its selections and scroll position.

use std::sync::Arc;

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::column::ColumnBox;
use crate::pairs::AutoClosed;
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
    /// Set by Cmd+D and friends: the next render shows this range, centred if it's off screen
    /// (VS Code's `revealRangeInCenterIfOutsideViewport`). Wins over `reveal_cursor`.
    pub reveal_range: Option<(ByteIdx, ByteIdx)>,
    /// Set by add cursor above / below and column select: the next render scrolls just enough
    /// to show this position (the newest cursor, not the primary). Wins over `reveal_cursor`.
    pub reveal_pos: Option<ByteIdx>,
    /// The last column (box) select, so the next one grows the same box.
    pub column: Option<ColumnBox>,
    /// Cmd+U: earlier selection lists, separate from text undo.
    pub cursor_history: CursorHistory,
    /// Closers this view auto-inserted that typing can still step over (`pairs`).
    pub auto_closed: AutoClosed,
    /// The brackets drawn highlighted, cached between frames (`brackets::Matches`).
    pub brackets: crate::brackets::Matches,
}

/// VS Code's cursor undo stack. Every selection change that isn't part of a text change pushes
/// the list from before it (consecutive duplicates once, at most `CURSOR_HISTORY` entries); any
/// text change, in this view or another on the same buffer, clears it. The app records changes
/// once per input event (`App::sync_views`), comparing against `seen`.
#[derive(Debug, Clone)]
pub struct CursorHistory {
    undo: Vec<CursorState>,
    /// The selections at the end of the last recorded event.
    seen: Selections,
}

/// What Cmd+U restores: the selections and the scroll position that went with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub selections: Selections,
    pub top_line: usize,
    pub left_col: usize,
}

/// VS Code keeps 50.
const CURSOR_HISTORY: usize = 50;

impl CursorHistory {
    fn new(selections: &Selections) -> Self {
        Self {
            undo: Vec::new(),
            seen: selections.clone(),
        }
    }

    /// The text changed: nothing before it can be restored.
    pub fn reset(&mut self, current: &Selections) {
        self.undo.clear();
        self.seen.clone_from(current);
    }

    /// The first half of a `reset` around a text change: drops the history's share of the
    /// selection list, so the change can edit the list in place instead of copying it.
    fn release(&mut self) {
        self.undo.clear();
        self.seen = Selections::single(Selection::cursor(ByteIdx(0)));
    }

    /// Records a selection change since the last call, if any; `top_line`/`left_col` are the
    /// scroll position from before it. Returns whether the selections changed.
    pub fn record(&mut self, current: &Selections, top_line: usize, left_col: usize) -> bool {
        if self.seen == *current {
            return false;
        }
        let prev = std::mem::replace(&mut self.seen, current.clone());
        if self.undo.last().is_none_or(|s| s.selections != prev) {
            self.undo.push(CursorState {
                selections: prev,
                top_line,
                left_col,
            });
            if self.undo.len() > CURSOR_HISTORY {
                self.undo.remove(0);
            }
        }
        true
    }

    /// Pops the last state; it becomes `seen`, so restoring it isn't recorded as a change.
    pub fn pop(&mut self) -> Option<CursorState> {
        let state = self.undo.pop()?;
        self.seen.clone_from(&state.selections);
        Some(state)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.undo.len()
    }
}

/// A text view's scroll bar: the view with its buffer's text.
pub struct TextScroll<'a> {
    pub view: &'a mut View,
    pub rope: &'a Rope,
}

impl crate::scrollbar::Scrollable for TextScroll<'_> {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::text(pos::line_count(self.rope), self.view.height, self.view.top_line())
    }

    /// Drops pending reveals: the pointer decides where the view is.
    fn set_top(&mut self, top: usize) {
        self.view.set_top_line(self.rope, top);
        self.view.reveal_cursor = false;
        self.view.reveal_range = None;
        self.view.reveal_pos = None;
    }
}

impl View {
    pub fn new() -> Self {
        let selections = Selections::single(Selection::cursor(ByteIdx(0)));
        Self {
            cursor_history: CursorHistory::new(&selections),
            selections,
            top_line: 0,
            top_byte: ByteIdx(0),
            left_col: 0,
            height: 1,
            width: 1,
            reveal_cursor: true,
            reveal_range: None,
            reveal_pos: None,
            column: None,
            auto_closed: AutoClosed::default(),
            brackets: Default::default(),
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
        self.scroll_to(rope, self.selections.primary().head, tab_width);
    }

    /// Scrolls just enough to show `head` (clamped to the text: it may be from before an edit).
    pub fn scroll_to(&mut self, rope: &Rope, head: ByteIdx, tab_width: usize) {
        let head = ByteIdx(rope.floor_char_boundary(head.0.min(rope.len())));
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

    /// Shows `start..end`: if its first line is off screen, centres it vertically; scrolls
    /// sideways just enough to show its end.
    pub fn reveal_in_center(&mut self, rope: &Rope, start: ByteIdx, end: ByteIdx, tab_width: usize) {
        let line = pos::line_of(rope, start);
        if line < self.top_line || line >= self.top_line + self.height {
            self.set_top_line(rope, line.saturating_sub(self.height / 2));
        }
        let (start_col, end_col) = (
            pos::display_col(rope, start, tab_width).0,
            pos::display_col(rope, end, tab_width).0,
        );
        if end_col >= self.left_col + self.width {
            self.left_col = end_col + 1 - self.width;
        }
        if start_col < self.left_col {
            self.left_col = start_col;
        }
    }

    /// Follows edits made through another view on the same buffer: selections and the
    /// viewport move with the text. `changes` are in the order applied; `rope` is the result.
    pub fn map_through(&mut self, changes: &[Arc<ChangeSet>], rope: &Rope) {
        let len = rope.len();
        let mappers: Vec<_> = changes.iter().map(|c| c.mapper()).collect();
        // Changes start and end on char boundaries and insert whole strings, so a position on
        // a boundary maps to one: no rope lookup per position (it was most of the cost at 60k
        // selections).
        let map = |p: ByteIdx| {
            let p = mappers.iter().fold(p, |p, m| m.map(p, Assoc::Before)).0.min(len);
            debug_assert_eq!(rope.floor_char_boundary(p), p, "mapped off a char boundary");
            ByteIdx(p)
        };
        self.cursor_history.release();
        // Start before end: positions in order let each mapper step from its last hit instead of
        // searching. Mapping is a function of the position, so a cursor is looked up once.
        self.selections.transform(|s| {
            let start = map(s.start());
            let end = if s.is_empty() { start } else { map(s.end()) };
            let (anchor, head) = if s.head < s.anchor { (end, start) } else { (start, end) };
            Selection {
                anchor,
                head,
                goal: s.goal,
            }
        });
        let top = map(self.top_byte);
        self.set_top_line(rope, pos::line_of(rope, top));
        self.cursor_history.reset(&self.selections);
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

        edit::paste(&mut buf, &mut a, "zero\n", false, &[]);
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

    /// Backward selections keep their direction when mapped; the other view's Cmd+U history
    /// is cleared by the edit.
    #[test]
    fn mapping_keeps_direction_and_clears_cursor_history() {
        let mut buf = Buffer::from_str("abc def ghi");
        let mut a = View::new();
        let mut b = View::new();
        a.selections = cursor(0);
        b.selections = Selections::new(
            vec![
                Selection::new(ByteIdx(7), ByteIdx(4)),
                Selection::new(ByteIdx(8), ByteIdx(11)),
            ],
            0,
        );
        b.cursor_history.record(&cursor(1), 0, 0);
        assert_eq!(b.cursor_history.len(), 1);
        edit::insert_char(&mut buf, &mut a, 'X', &Settings::default());
        b.map_through(&buf.take_applied(), &buf.rope);
        let got: Vec<(usize, usize)> = b.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(8, 5), (9, 12)]);
        assert_eq!(b.cursor_history.len(), 0);
        assert!(
            !b.cursor_history.record(&b.selections, 0, 0),
            "the mapped list is the one seen"
        );
    }

    #[test]
    fn insert_at_another_cursor_leaves_it_before() {
        let mut buf = Buffer::from_str("ab");
        let mut a = View::new();
        let mut b = View::new();
        a.selections = cursor(1);
        b.selections = cursor(1);
        edit::insert_char(&mut buf, &mut a, 'X', &Settings::default());
        b.map_through(&buf.take_applied(), &buf.rope);
        assert_eq!(b.selections.primary().head, ByteIdx(1));
        assert_eq!(a.selections.primary().head, ByteIdx(2));
    }

    #[test]
    fn cursor_history_records_changes_once_and_caps() {
        let mut v = View::new();
        let h = &mut v.cursor_history;
        assert!(!h.record(&cursor(0), 0, 0), "unchanged");
        assert!(h.record(&cursor(3), 0, 0));
        assert!(h.record(&cursor(0), 5, 0));
        assert!(h.record(&cursor(3), 0, 0));
        assert_eq!(h.len(), 3);
        assert_eq!(h.pop().unwrap().selections, cursor(0));
        assert!(!h.record(&cursor(0), 0, 0), "restoring isn't a change");
        let s = h.pop().unwrap();
        assert_eq!((s.selections, s.top_line), (cursor(3), 5), "with the scroll from then");
        for i in 0..100 {
            h.record(&cursor(10 + i), i, 0);
        }
        assert_eq!(h.len(), CURSOR_HISTORY);
        // The oldest entries were dropped: the newest 50 pop back in order.
        let mut copy = h.clone();
        for i in (50..100).rev() {
            let s = copy.pop().unwrap();
            assert_eq!((s.selections, s.top_line), (cursor(10 + i - 1), i));
        }
        assert!(copy.pop().is_none());
        h.reset(&cursor(1));
        assert_eq!(h.len(), 0);
        assert!(h.pop().is_none());
    }

    /// Cmd+D's reveal: a range already on screen doesn't scroll; one above or below is centred;
    /// sideways it scrolls just enough to show the range's end, then its start.
    #[test]
    fn reveal_in_center_centres_off_screen_ranges() {
        let text: String = (0..100).map(|i| format!("{i:<3}{}\n", "x".repeat(i))).collect();
        let rope = Rope::from_str(&text);
        let at = |line: usize, col: usize| ByteIdx(pos::line_start(&rope, line).0 + col);
        let mut v = View::new();
        (v.height, v.width) = (10, 20);
        v.set_top_line(&rope, 20);
        v.reveal_in_center(&rope, at(29, 0), at(29, 2), 4);
        assert_eq!((v.top_line(), v.left_col), (20, 0), "on screen: no scroll");
        v.reveal_in_center(&rope, at(30, 0), at(30, 2), 4);
        assert_eq!(v.top_line(), 25, "below: centred");
        v.reveal_in_center(&rope, at(5, 0), at(5, 2), 4);
        assert_eq!(v.top_line(), 0, "above, near the top: as far as it goes");
        v.reveal_in_center(&rope, at(50, 0), at(50, 1), 4);
        assert_eq!(v.top_line(), 45);
        // Sideways: the end shows at the right edge; a range wider than the view shows its start.
        v.reveal_in_center(&rope, at(50, 30), at(50, 35), 4);
        assert_eq!(v.left_col, 16);
        v.reveal_in_center(&rope, at(50, 5), at(50, 40), 4);
        assert_eq!(v.left_col, 5);
        assert_eq!(v.top_line(), 45, "same line: no vertical scroll");
    }
}
