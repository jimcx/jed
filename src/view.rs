//! A view onto a buffer: its selections and scroll position.

use std::sync::Arc;

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::column::ColumnBox;
use crate::pairs::AutoClosed;
use crate::selection::{Selection, Selections};
use crate::wrap::Wrap;

#[derive(Debug, Clone)]
pub struct View {
    pub selections: Selections,
    top_line: usize,
    /// The sub-row within `top_line` at the top of the view: 0 unless lines are wrapped.
    top_row: usize,
    /// Start of `top_line`, so the viewport can follow edits made in other views.
    top_byte: ByteIdx,
    pub left_col: usize,
    /// The widest line drawn so far, written by drawing: how far sideways scrolling goes.
    pub scroll_width: crate::scrollbar::ScrollWidth,
    /// Text area size from the last render, used by page movement.
    pub height: usize,
    pub width: usize,
    /// Whether the last frame wrapped lines, and the revision / tab width its rows were laid
    /// out for: written by drawing, read by scrolling (`scroll_lines`, `TextScroll`).
    pub wrapping: bool,
    pub revision: u64,
    pub tab_width: usize,
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
    /// The lines' screen rows at the view's width, kept across edits (`wrap::Wrap`).
    pub wrap: Wrap,
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
        if self.view.wrapping {
            let (revision, width, tab) = (self.view.revision, self.view.width, self.view.tab_width);
            let total = self.view.wrap.cached_total(revision, width, tab);
            let line = self
                .view
                .wrap
                .cached_row_of_line(revision, self.view.top_line(), width, tab);
            // Drawing primes the counts; without a frame yet, scroll by lines as before.
            if let (Some(total), Some(line)) = (total, line) {
                return crate::scrollbar::Scroll::text(total, self.view.height, line + self.view.top_row());
            }
        }
        crate::scrollbar::Scroll::text(pos::line_count(self.rope), self.view.height, self.view.top_line())
    }

    /// Drops pending reveals: the pointer decides where the view is.
    fn set_top(&mut self, top: usize) {
        let (revision, width, tab) = (self.view.revision, self.view.width, self.view.tab_width);
        // Rows only while the counts are current for this key, matching `scroll`'s fallback to
        // lines: otherwise `top` is a line index (from the line-based `Scroll`), not a row.
        if self.view.wrapping && self.view.wrap.cached_total(revision, width, tab).is_some() {
            let (line, row) = self.view.wrap.line_at(self.rope, revision, top, width, tab);
            self.view.set_top(self.rope, line, row);
        } else {
            self.view.set_top_line(self.rope, top);
        }
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
            top_row: 0,
            top_byte: ByteIdx(0),
            left_col: 0,
            scroll_width: Default::default(),
            height: 1,
            width: 1,
            wrapping: false,
            revision: 0,
            tab_width: 4,
            reveal_cursor: true,
            reveal_range: None,
            reveal_pos: None,
            column: None,
            auto_closed: AutoClosed::default(),
            brackets: Default::default(),
            wrap: Wrap::default(),
        }
    }

    pub fn top_line(&self) -> usize {
        self.top_line
    }

    /// The sub-row of `top_line` shown first: 0 unless lines are wrapped.
    pub fn top_row(&self) -> usize {
        self.top_row
    }

    /// The first shown line's start (kept with `top_line`).
    pub fn top_byte(&self) -> ByteIdx {
        self.top_byte
    }

    pub fn set_top_line(&mut self, rope: &Rope, line: usize) {
        self.top_line = line.min(pos::line_count(rope) - 1);
        self.top_row = 0;
        self.top_byte = pos::line_start(rope, self.top_line);
    }

    /// Sets the first shown (line, sub-row): wrapped scrolling. Callers take `row` from a layout
    /// or the row walkers, which clamp it.
    pub fn set_top(&mut self, rope: &Rope, line: usize, row: usize) {
        self.set_top_line(rope, line);
        self.top_row = row;
    }

    /// Scrolls `delta` screen rows (the wheel, PageUp / PageDown), or lines while not wrapping;
    /// stops at the first row and the text's last row.
    pub fn scroll_lines(&mut self, rope: &Rope, delta: isize) {
        if !self.wrapping {
            self.set_top_line(rope, self.top_line.saturating_add_signed(delta));
            return;
        }
        let (line, row) = if delta >= 0 {
            self.rows_down(rope, self.top_line, self.top_row, delta.unsigned_abs())
        } else {
            self.rows_up(rope, self.top_line, self.top_row, delta.unsigned_abs())
        };
        self.set_top(rope, line, row);
    }

    /// The (line, row) `count` rows below (`at`, `row`), clamped to the text's last row. Walks
    /// only the lines crossed, not the whole text.
    fn rows_down(&mut self, rope: &Rope, at: usize, row: usize, count: usize) -> (usize, usize) {
        let (revision, width, tab) = (self.revision, self.width, self.tab_width);
        let total = pos::line_count(rope);
        let (mut at, mut remaining) = (at, row + count);
        while at < total {
            let rows = self.wrap.line_rows(rope, revision, at, width, tab);
            if remaining < rows {
                return (at, remaining);
            }
            remaining -= rows;
            at += 1;
        }
        let last = total - 1;
        (last, self.wrap.line_rows(rope, revision, last, width, tab) - 1)
    }

    /// The (line, row) `count` rows above (`at`, `row`), clamped to the first row.
    fn rows_up(&mut self, rope: &Rope, at: usize, row: usize, count: usize) -> (usize, usize) {
        let (revision, width, tab) = (self.revision, self.width, self.tab_width);
        let (mut at, mut row, mut up) = (at, row, count);
        while up > 0 {
            if row >= up {
                row -= up;
                break;
            }
            up -= row;
            if at == 0 {
                row = 0;
                break;
            }
            at -= 1;
            row = self.wrap.line_rows(rope, revision, at, width, tab) - 1;
            up -= 1;
        }
        (at, row)
    }

    /// `head`'s row within its line, from the line's wrap layout.
    fn wrapped_row(&mut self, rope: &Rope, line: usize, head: ByteIdx, tab_width: usize) -> usize {
        let within = head.0.saturating_sub(pos::line_start(rope, line).0);
        self.wrap
            .layout(rope, self.revision, line, self.width, tab_width)
            .row_of(ByteIdx(within))
            .0
    }

    /// Scrolls sideways by `delta` columns (the wheel), stopping when the widest line drawn so
    /// far ends in view; nothing while wrapping (the rows already fit the text width). The
    /// selections stay put and nothing is revealed (the next reveal scrolls back).
    pub fn scroll_columns(&mut self, delta: isize) {
        if self.wrapping {
            return;
        }
        self.left_col = self.scroll_width.scroll(self.left_col, delta, self.width);
    }

    /// Scrolls just enough to show the primary cursor.
    pub fn scroll_to_cursor(&mut self, rope: &Rope, tab_width: usize) {
        self.scroll_to(rope, self.selections.primary().head, tab_width);
    }

    /// Scrolls just enough to show `head` (clamped to the text: it may be from before an edit).
    /// While wrapping that is the cursor's screen row, and nothing scrolls sideways.
    pub fn scroll_to(&mut self, rope: &Rope, head: ByteIdx, tab_width: usize) {
        let head = ByteIdx(rope.floor_char_boundary(head.0.min(rope.len())));
        let line = pos::line_of(rope, head);
        if self.wrapping {
            let row = self.wrapped_row(rope, line, head, tab_width);
            if (line, row) < (self.top_line, self.top_row) {
                self.set_top(rope, line, row);
            } else {
                let bottom = self.rows_down(rope, self.top_line, self.top_row, self.height.saturating_sub(1));
                if (line, row) > bottom {
                    let (at, sub) = self.rows_up(rope, line, row, self.height.saturating_sub(1));
                    self.set_top(rope, at, sub);
                }
            }
            return;
        }
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

    /// Shows `start..end`: if its first row is off screen, centres it vertically; scrolls
    /// sideways just enough to show its end (nothing while wrapping).
    pub fn reveal_in_center(&mut self, rope: &Rope, start: ByteIdx, end: ByteIdx, tab_width: usize) {
        let line = pos::line_of(rope, start);
        if self.wrapping {
            let row = self.wrapped_row(rope, line, start, tab_width);
            let bottom = self.rows_down(rope, self.top_line, self.top_row, self.height.saturating_sub(1));
            if (line, row) < (self.top_line, self.top_row) || (line, row) > bottom {
                let (at, sub) = self.rows_up(rope, line, row, self.height / 2);
                self.set_top(rope, at, sub);
            }
            return;
        }
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
    /// viewport move with the text. `changes` are in the order applied; `rope` is the result
    /// and `revision` its buffer revision (for the wrap cache).
    pub fn map_through(&mut self, changes: &[Arc<ChangeSet>], rope: &Rope, revision: u64) {
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
        self.wrap.map(changes, rope, revision);
        self.cursor_history.reset(&self.selections);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::edit::{self, Settings};
    use crate::scrollbar::Scrollable;

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
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());
        let heads: Vec<usize> = b.selections.iter().map(|s| s.head.0).collect();
        assert_eq!(heads, vec![9, 18], "both cursors after the insert moved");
        assert_eq!(b.top_line(), 2, "viewport still starts at `two`");

        edit::run(crate::command::Command::Undo, &mut buf, &mut a, &Settings::default());
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());
        let got: Vec<(usize, usize)> = b.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(4, 4), (8, 13)]);
        assert_eq!(b.top_line(), 1);
    }

    /// `map_through` keeps the wrap cache current across edits too, and incrementally: after the
    /// edit `line_rows` has the new count without laying the whole text out again (one sweep).
    #[test]
    fn map_through_maps_the_wrap_cache() {
        let mut buf = Buffer::from_str("aaaa bbbb\ncc\n");
        let mut a = View::new();
        let mut b = View::new();
        a.selections = cursor(0);
        b.selections = cursor(0);
        let rev0 = buf.revision();
        // Lay out b's wrap cache: line 0 is two rows at width 5.
        assert_eq!(b.wrap.line_rows(&buf.rope, rev0, 0, 5, 4), 2);
        assert_eq!(b.wrap.sweeps(), 1);

        // Type at the start of line 0 so it grows to three rows, without changing the line count.
        edit::paste(&mut buf, &mut a, "zzzz", false, &[]);
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());

        assert_eq!(b.wrap.line_rows(&buf.rope, buf.revision(), 0, 5, 4), 3);
        assert_eq!(
            b.wrap.sweeps(),
            1,
            "map_through mapped the wrap cache, no whole-text layout"
        );
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
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());
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
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());
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
        b.map_through(&buf.take_applied(), &buf.rope, buf.revision());
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

    /// The wheel's sideways scroll: the selections untouched, stopping one column past the
    /// widest line drawn so far.
    #[test]
    fn scroll_columns_stops_at_the_widest_line() {
        let mut v = View::new();
        v.width = 20;
        v.scroll_columns(4);
        assert_eq!(v.left_col, 0, "nothing wider than the view drawn yet");
        v.scroll_width.measure(30);
        for _ in 0..5 {
            v.scroll_columns(4);
        }
        assert_eq!(
            v.left_col, 11,
            "the widest line's end in view, one blank column after it"
        );
        v.scroll_columns(-4);
        assert_eq!(v.left_col, 7);
        assert_eq!(v.selections.primary().head, ByteIdx(0));
        assert!(v.reveal_cursor, "scrolling sideways doesn't touch the reveal");
    }

    /// A view wrapping lines at `width` columns and `height` rows (the fields drawing writes).
    fn wrapped(width: usize, height: usize) -> View {
        let mut v = View::new();
        (v.width, v.height, v.wrapping, v.tab_width) = (width, height, true, 4);
        v
    }

    /// The wheel and PageUp / PageDown scroll screen rows while wrapping, over line breaks, and
    /// stop at the first and last rows.
    #[test]
    fn scroll_lines_walks_wrapped_rows_and_stops_at_the_ends() {
        // At width 5: two rows, one row, two rows.
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut v = wrapped(5, 2);
        v.scroll_lines(&rope, 1);
        assert_eq!((v.top_line(), v.top_row()), (0, 1));
        v.scroll_lines(&rope, 1);
        assert_eq!((v.top_line(), v.top_row()), (1, 0), "over to the next line");
        v.scroll_lines(&rope, 2);
        assert_eq!((v.top_line(), v.top_row()), (2, 1), "the last row");
        v.scroll_lines(&rope, 10);
        assert_eq!((v.top_line(), v.top_row()), (3, 0), "clamped at the last row");
        v.scroll_lines(&rope, -1);
        assert_eq!((v.top_line(), v.top_row()), (2, 1));
        v.scroll_lines(&rope, -3);
        assert_eq!((v.top_line(), v.top_row()), (0, 1), "back over a whole line");
        v.scroll_lines(&rope, -10);
        assert_eq!((v.top_line(), v.top_row()), (0, 0), "clamped at the top");
    }

    /// Revealing a cursor below the view scrolls to its row (the last visible row is its row),
    /// above to its row at the top; nothing scrolls sideways while wrapping.
    #[test]
    fn scroll_to_reveals_the_cursor_row_without_sideways_scroll() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut v = wrapped(5, 2);
        v.left_col = 3;
        v.scroll_to(&rope, ByteIdx(pos::line_end(&rope, 2).0), 4);
        assert_eq!((v.top_line(), v.top_row()), (2, 0), "the end's row at the bottom");
        assert_eq!(v.left_col, 3, "no sideways scroll while wrapping");
        v.scroll_to(&rope, ByteIdx(pos::line_start(&rope, 1).0), 4);
        assert_eq!((v.top_line(), v.top_row()), (1, 0), "above: it becomes the top row");
        v.scroll_to(&rope, ByteIdx(0), 4);
        assert_eq!((v.top_line(), v.top_row()), (0, 0));
        v.scroll_to(&rope, ByteIdx(pos::line_end(&rope, 0).0), 4);
        assert_eq!(
            (v.top_line(), v.top_row()),
            (0, 0),
            "a row already on screen: no scroll"
        );
        v.scroll_width.measure(30);
        v.scroll_columns(4);
        assert_eq!(v.left_col, 3, "the sideways wheel does nothing while wrapping");
    }

    /// Cmd+D's centre reveal works on rows: an off-screen row is centred, a row above goes as
    /// far up as it can.
    #[test]
    fn reveal_in_center_centres_off_screen_rows() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut v = wrapped(5, 2);
        v.left_col = 3;
        let at = |line: usize, col: usize| ByteIdx(pos::line_start(&rope, line).0 + col);
        v.reveal_in_center(&rope, at(0, 0), at(0, 0), 4);
        assert_eq!((v.top_line(), v.top_row()), (0, 0), "on screen: no scroll");
        v.reveal_in_center(&rope, at(2, 0), at(2, 0), 4);
        assert_eq!((v.top_line(), v.top_row()), (1, 0), "below: one row up");
        assert_eq!(v.left_col, 3, "no sideways scroll while wrapping");
        v.reveal_in_center(&rope, at(0, 1), at(0, 1), 4);
        assert_eq!(
            (v.top_line(), v.top_row()),
            (0, 0),
            "above, near the top: as far as it goes"
        );
    }

    /// The scroll bar is over rows while wrapping: its top is the row (not the line), and a
    /// drag lands on the (line, row) of that global row.
    #[test]
    fn the_text_scroll_bar_uses_rows() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut v = wrapped(5, 2);
        assert_eq!(v.wrap.total_rows(&rope, 0, 5, 4), 6, "drawing primes the counts");
        {
            let scroll = TextScroll {
                view: &mut v,
                rope: &rope,
            }
            .scroll();
            assert_eq!((scroll.top, scroll.max_top, scroll.height), (0, 5, 2));
        }
        v.set_top(&rope, 1, 0); // line 1's first row is global row 2
        {
            let scroll = TextScroll {
                view: &mut v,
                rope: &rope,
            }
            .scroll();
            assert_eq!(scroll.top, 2, "the top is the row, not the line");
        }
        {
            let mut bar = TextScroll {
                view: &mut v,
                rope: &rope,
            };
            crate::scrollbar::Scrollable::set_top(&mut bar, 4);
        }
        assert_eq!((v.top_line(), v.top_row()), (2, 1), "the last row");
        {
            let mut bar = TextScroll {
                view: &mut v,
                rope: &rope,
            };
            crate::scrollbar::Scrollable::set_top(&mut bar, 99);
        }
        assert_eq!((v.top_line(), v.top_row()), (3, 0), "clamped");
        assert!(!v.reveal_cursor, "the pointer decides where the view is");
    }

    /// Before the first wrapped frame primes the counts, the scroll bar reads lines and a drag
    /// sets the line (not a row), so `scroll` and `set_top` agree on the units.
    #[test]
    fn the_scroll_bar_falls_back_to_lines_before_the_first_wrapped_frame() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut v = wrapped(5, 2);
        {
            let scroll = TextScroll {
                view: &mut v,
                rope: &rope,
            }
            .scroll();
            assert_eq!((scroll.max_top, scroll.top), (3, 0), "four lines, no rows primed yet");
        }
        {
            let mut bar = TextScroll {
                view: &mut v,
                rope: &rope,
            };
            crate::scrollbar::Scrollable::set_top(&mut bar, 2);
        }
        assert_eq!((v.top_line(), v.top_row()), (2, 0), "a line index, not a global row");
    }
}
