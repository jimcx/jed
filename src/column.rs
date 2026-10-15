//! Cursor commands that work by line and screen column: add cursor above / below (Cmd+Alt+Up /
//! Down), a cursor at each selected line's end (Shift+Alt+I) and column (box) select, from the
//! keyboard (Cmd+Shift+Alt+arrows) and the mouse (Shift+Alt+drag). VS Code's
//! `insertCursorAbove`/`Below`, `insertCursorAtEndOfEachLineSelected` and `ColumnSelection`.
//!
//! Columns are display columns (tabs expanded, wide chars two cells) through `buffer::pos`.

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx, DisplayCol, Line, LineWalker};
use crate::command::ColumnMotion;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// A line and a display column on it (which may lie past the line's end).
pub type Cell = (usize, DisplayCol);

/// The last box selection of a view: the corner it started from, the moving corner, and the
/// selections it made. The next column select grows the same box only while the view still has
/// exactly those selections (VS Code's `columnSelectData`); any other change starts afresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnBox {
    pub from: Cell,
    pub to: Cell,
    made: Selections,
}

/// The box to grow: the view's last one if its selections are unchanged since, else from the
/// primary selection's anchor to its head.
pub fn current(rope: &Rope, view: &View, tab_width: usize) -> (Cell, Cell) {
    if let Some(b) = &view.column
        && b.made == view.selections
    {
        return (b.from, b.to);
    }
    let p = view.selections.primary();
    (cell_of(rope, p.anchor, tab_width), cell_of(rope, p.head, tab_width))
}

fn cell_of(rope: &Rope, at: ByteIdx, tab_width: usize) -> Cell {
    (pos::line_of(rope, at), pos::display_col(rope, at, tab_width))
}

/// Selects the box from `from` to `to`: on each line from `from.0` to `to.0`, the text between
/// the two columns (each at the nearest grapheme boundary), anchored at `from`'s column. Lines
/// too short to reach into the box are left out; if that leaves none, every line gets a cursor at
/// its end (VS Code). The selection on `from`'s line (or the nearest kept one) is primary, the
/// one on `to`'s line newest and revealed.
pub fn select(rope: &Rope, view: &mut View, from: Cell, to: Cell, tab_width: usize) {
    let (fc, tc) = (from.1, to.1);
    let lines = from.0.min(to.0)..=from.0.max(to.0);
    let mut walk = LineWalker::new(rope);
    let mut list = Vec::new();
    for line in lines.clone() {
        let line = walk.line(line);
        let start = line.byte_near_col(fc, tab_width);
        let end = line.byte_near_col(tc, tab_width);
        let (vs, ve) = (line.col(start, tab_width), line.col(end, tab_width));
        if fc < tc && (vs > tc || ve < fc) || fc > tc && (ve > fc || vs < tc) {
            continue;
        }
        list.push(Selection::new(start, end));
    }
    if list.is_empty() {
        let mut walk = LineWalker::new(rope);
        list = lines.map(|l| Selection::cursor(walk.line(l).end())).collect();
    }
    // Listed top down; `from`'s end of the box is primary, `to`'s newest.
    let (primary, newest) = if from.0 <= to.0 {
        (0, list.len() - 1)
    } else {
        (list.len() - 1, 0)
    };
    let head = list[newest].head;
    view.selections = Selections::with_newest(list, primary, newest);
    view.column = Some(ColumnBox {
        from,
        to,
        made: view.selections.clone(),
    });
    view.reveal_pos = Some(head);
}

/// Cmd+Shift+Alt+arrow / PageUp / PageDown: moves the box's moving corner. Left and right step one
/// display column (right no further than the longest line in the box), up and down one line or a
/// page.
pub fn step(rope: &Rope, view: &mut View, motion: ColumnMotion, tab_width: usize) {
    let (from, (mut line, mut col)) = current(rope, view, tab_width);
    let last = pos::line_count(rope) - 1;
    let page = view.height.max(1);
    match motion {
        ColumnMotion::Left => col.0 = col.0.saturating_sub(1),
        ColumnMotion::Right => {
            let max = (from.0.min(line)..=from.0.max(line))
                .map(|l| pos::display_col(rope, pos::line_end(rope, l), tab_width))
                .max()
                .unwrap_or_default();
            if col < max {
                col.0 += 1;
            }
        }
        ColumnMotion::Up => line = line.saturating_sub(1),
        ColumnMotion::Down => line = (line + 1).min(last),
        ColumnMotion::PageUp => line = line.saturating_sub(page),
        ColumnMotion::PageDown => line = (line + page).min(last),
    }
    select(rope, view, from, (line, col), tab_width);
}

/// Cmd+Alt+Up / Down: adds a copy of every selection one line up / down, at the same display
/// column (the remembered one if the cursor has it; the copy remembers it too). A cursor on the
/// first / last line has nowhere to go. The primary stays; the copy furthest in that direction
/// is the newest and is scrolled into view.
pub fn add_cursor(rope: &Rope, view: &mut View, up: bool, tab_width: usize) {
    let last = pos::line_count(rope) - 1;
    let target = |line: usize| {
        if up {
            line.checked_sub(1)
        } else {
            (line < last).then_some(line + 1)
        }
    };
    let sels = view.selections.as_slice();
    let mut list = sels.to_vec();
    // Lines in order, each looked up once (60k cursors).
    let (mut from, mut to) = (LineWalker::new(rope), LineWalker::new(rope));
    for s in sels {
        let line = from.line_at(s.head);
        let Some(t) = target(line.index) else {
            continue;
        };
        let goal = s.goal.unwrap_or_else(|| line.col(s.head, tab_width));
        let anchor_cell = if s.is_empty() {
            None
        } else if line.contains(s.anchor) {
            Some((line.index, line.col(s.anchor, tab_width)))
        } else {
            Some(cell_of(rope, s.anchor, tab_width))
        };
        let head = to.line(t).byte_at_col(goal, tab_width);
        let anchor = match anchor_cell {
            None => head,
            Some((al, col)) => match target(al).unwrap_or(al) {
                al if al == t => to.line(t).byte_at_col(col, tab_width),
                al => Line::new(rope, al).byte_at_col(col, tab_width),
            },
        };
        list.push(Selection {
            anchor,
            head,
            goal: Some(goal),
        });
    }
    if list.len() == sels.len() {
        return;
    }
    // The copy of the first selection is furthest up, of the last furthest down: the newest.
    let newest = if up { sels.len() } else { list.len() - 1 };
    let head = list[newest].head;
    view.selections = Selections::with_newest(list, view.selections.primary_index(), newest);
    view.reveal_pos = Some(head);
}

/// Shift+Alt+I: replaces each non-empty selection with a cursor at the end of every line it
/// covers, the last one at the selection's end (none there if that's a line start). Empty
/// selections are dropped; if all are empty nothing changes (VS Code).
pub fn cursors_at_line_ends(rope: &Rope, view: &mut View) {
    let mut walk = LineWalker::new(rope);
    let mut list = Vec::new();
    for s in view.selections.iter().filter(|s| !s.is_empty()) {
        let mut l = walk.line_at(s.start()).index;
        loop {
            let line = walk.line(l);
            if line.contains(s.end()) {
                if line.start() != s.end() {
                    list.push(Selection::cursor(s.end()));
                }
                break;
            }
            list.push(Selection::cursor(line.end()));
            l += 1;
        }
    }
    if list.is_empty() {
        return;
    }
    view.selections = Selections::new(list, 0);
    view.reveal_cursor = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_with(sels: Vec<Selection>) -> View {
        let mut v = View::new();
        v.selections = Selections::new(sels, 0);
        v
    }

    fn ranges(v: &View) -> Vec<(usize, usize)> {
        v.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect()
    }

    fn cur(at: usize) -> Selection {
        Selection::cursor(ByteIdx(at))
    }

    #[test]
    fn add_cursor_below_keeps_the_column_and_remembers_it() {
        // Line starts: 0, 7, 10, 17.
        let rope = Rope::from_str("abcdef\nab\nabcdef\n");
        let mut v = view_with(vec![cur(5)]);
        add_cursor(&rope, &mut v, false, 4);
        assert_eq!(ranges(&v), vec![(5, 5), (9, 9)], "clamped to the short line's end");
        add_cursor(&rope, &mut v, false, 4);
        assert_eq!(
            ranges(&v),
            vec![(5, 5), (9, 9), (15, 15)],
            "back at column 5 on the long line"
        );
        assert_eq!(v.selections.primary().head.0, 5, "the primary stays");
        assert_eq!(v.selections.newest().head.0, 15);
        assert_eq!(v.reveal_pos, Some(ByteIdx(15)));
        // Last line (empty, after the final newline): nowhere further down.
        let mut v = view_with(vec![cur(17)]);
        add_cursor(&rope, &mut v, false, 4);
        assert_eq!(ranges(&v), vec![(17, 17)]);
        assert_eq!(v.reveal_pos, None);
    }

    #[test]
    fn add_cursor_above_on_the_first_line_does_nothing() {
        let rope = Rope::from_str("ab\ncd");
        let mut v = view_with(vec![cur(1)]);
        add_cursor(&rope, &mut v, true, 4);
        assert_eq!(ranges(&v), vec![(1, 1)]);
        // From two lines: each gets a copy above; the top one is new and newest.
        assert_eq!(v.reveal_pos, None);
        let rope = Rope::from_str("ab\ncd\nef");
        let mut v = view_with(vec![cur(4), cur(7)]);
        add_cursor(&rope, &mut v, true, 4);
        assert_eq!(ranges(&v), vec![(1, 1), (4, 4), (7, 7)]);
        assert_eq!(v.selections.newest().head.0, 1);
        // One cursor on the first line gets no copy: the newest is the other one's copy.
        let mut v = view_with(vec![cur(1), cur(7)]);
        add_cursor(&rope, &mut v, true, 4);
        assert_eq!(ranges(&v), vec![(1, 1), (4, 4), (7, 7)]);
        assert_eq!(v.selections.newest().head.0, 4);
        assert_eq!(v.reveal_pos, Some(ByteIdx(4)));
    }

    #[test]
    fn add_cursor_uses_display_columns() {
        // A tab (to column 4) and a wide char (two columns) line up under "abcd".
        let rope = Rope::from_str("\tx\nabcdx\n界界x");
        let mut v = view_with(vec![cur(1)]); // after the tab: column 4
        add_cursor(&rope, &mut v, false, 4);
        add_cursor(&rope, &mut v, false, 4);
        assert_eq!(ranges(&v), vec![(1, 1), (7, 7), (15, 15)], "column 4 on each line");
    }

    #[test]
    fn add_cursor_copies_a_selection() {
        let rope = Rope::from_str("abcd\nabcd");
        let mut v = view_with(vec![Selection::new(ByteIdx(1), ByteIdx(3))]);
        add_cursor(&rope, &mut v, false, 4);
        assert_eq!(ranges(&v), vec![(1, 3), (6, 8)]);
    }

    #[test]
    fn line_ends_of_each_selection() {
        // Lines: "abc" 0..3, "de" 4..6, "fgh" 7..10, "ij" 11..13.
        let rope = Rope::from_str("abc\nde\nfgh\nij");
        // Ends mid-line: that position is the last cursor.
        let mut v = view_with(vec![Selection::new(ByteIdx(1), ByteIdx(8))]);
        cursors_at_line_ends(&rope, &mut v);
        assert_eq!(ranges(&v), vec![(3, 3), (6, 6), (8, 8)]);
        // Ends at a line start: no cursor there. Empty selections go.
        let mut v = view_with(vec![Selection::new(ByteIdx(0), ByteIdx(7)), cur(12)]);
        cursors_at_line_ends(&rope, &mut v);
        assert_eq!(ranges(&v), vec![(3, 3), (6, 6)]);
        // Only cursors: nothing changes.
        let mut v = view_with(vec![cur(1), cur(5)]);
        cursors_at_line_ends(&rope, &mut v);
        assert_eq!(ranges(&v), vec![(1, 1), (5, 5)]);
    }

    #[test]
    fn box_select_skips_short_lines_and_grows_from_the_same_corner() {
        // Line starts: 0, 7, 10, 17.
        let rope = Rope::from_str("abcdef\nab\nabcdef\nabcdef");
        let mut v = view_with(vec![cur(1)]);
        select(&rope, &mut v, (0, DisplayCol(1)), (2, DisplayCol(4)), 4);
        assert_eq!(
            ranges(&v),
            vec![(1, 4), (8, 9), (11, 14)],
            "the short line reaches column 2"
        );
        // A box wholly past the short line's end leaves it out.
        select(&rope, &mut v, (0, DisplayCol(3)), (2, DisplayCol(5)), 4);
        assert_eq!(ranges(&v), vec![(3, 5), (13, 15)]);
        assert_eq!(v.selections.primary().head.0, 5, "primary on the starting line");
        // The next keyboard step grows that box.
        step(&rope, &mut v, ColumnMotion::Down, 4);
        assert_eq!(ranges(&v), vec![(3, 5), (13, 15), (20, 22)]);
        step(&rope, &mut v, ColumnMotion::Left, 4);
        assert_eq!(ranges(&v), vec![(3, 4), (13, 14), (20, 21)]);
        // Changed selections: start again from the primary.
        v.selections = Selections::single(cur(2));
        step(&rope, &mut v, ColumnMotion::Up, 4);
        assert_eq!(ranges(&v), vec![(2, 2)], "on the first line already");
        step(&rope, &mut v, ColumnMotion::Down, 4);
        assert_eq!(ranges(&v), vec![(2, 2), (9, 9)]);
    }

    #[test]
    fn box_select_upwards_and_right_to_left() {
        let rope = Rope::from_str("abcdef\nabcdef");
        let mut v = view_with(vec![cur(0)]);
        select(&rope, &mut v, (1, DisplayCol(4)), (0, DisplayCol(1)), 4);
        assert_eq!(ranges(&v), vec![(4, 1), (11, 8)], "anchored at the starting column");
        assert_eq!(v.selections.primary().start().0, 8, "primary on the starting line");
        assert_eq!(v.selections.newest().start().0, 1);
        assert_eq!(v.reveal_pos, Some(ByteIdx(1)));
    }

    #[test]
    fn box_past_every_line_end_puts_cursors_at_the_ends() {
        let rope = Rope::from_str("ab\nabc\na");
        let mut v = view_with(vec![cur(0)]);
        select(&rope, &mut v, (0, DisplayCol(5)), (2, DisplayCol(8)), 4);
        assert_eq!(ranges(&v), vec![(2, 2), (6, 6), (8, 8)]);
    }

    #[test]
    fn box_select_right_stops_at_the_longest_line() {
        let rope = Rope::from_str("ab\na");
        let mut v = view_with(vec![cur(0)]);
        step(&rope, &mut v, ColumnMotion::Down, 4);
        for _ in 0..5 {
            step(&rope, &mut v, ColumnMotion::Right, 4);
        }
        assert_eq!(v.column.as_ref().unwrap().to, (1, DisplayCol(2)));
        assert_eq!(ranges(&v), vec![(0, 2), (3, 4)]);
    }

    #[test]
    fn box_select_counts_tabs_and_wide_chars_as_columns() {
        let rope = Rope::from_str("\tab\nabcdef\n界界ab");
        let mut v = view_with(vec![cur(0)]);
        select(&rope, &mut v, (0, DisplayCol(4)), (2, DisplayCol(5)), 4);
        // Column 4..5: after the tab "a"; "e"; after both wide chars "a".
        assert_eq!(ranges(&v), vec![(1, 2), (8, 9), (17, 18)]);
        // A box edge inside a tab or wide char snaps to its nearer side, at either end: column 3
        // is in the tab's last quarter (after it) and on the second wide char's first half (before
        // it); column 1 is on the first wide char's tie (before it).
        let rope = Rope::from_str("a\tbc\n界界ab");
        select(&rope, &mut v, (0, DisplayCol(0)), (1, DisplayCol(3)), 4);
        assert_eq!(ranges(&v), vec![(0, 2), (5, 8)]);
        select(&rope, &mut v, (1, DisplayCol(3)), (0, DisplayCol(1)), 4);
        assert_eq!(ranges(&v), vec![(2, 1), (8, 5)]);
    }
}
