//! Selecting a terminal's text with the mouse: the drag's cells, the text they copy and the
//! first row a taller screen is drawn from. No drawing and no app state.
//!
//! A `Selection`'s cells are scroll-independent (`Cell`: line 0 is the live screen's first row,
//! scrollback is negative), so a scroll leaves it on the same text; `copy_text` is
//! `contents_between` over the range the drag spans, window by window, with trailing spaces
//! trimmed per line. A resize clears it (the rows reflow). New output keeps it while it stays on
//! its text (`Snapshot` before a chunk, `follow` after): a chunk that scrolled `n` lines into the
//! scrollback (`Screen::scrolled_lines` of the jed vt100 fork) shifts the selection up by `n`
//! (`Selection::shift`: a part older than the kept scrollback clamps to its oldest row, one
//! wholly gone is dropped); it is cleared when the text of a live-screen row it covered changed
//! where that row scrolled to (output rewrote it in place: `\r`, a redraw, the alternate screen,
//! `clear`), or the scrollback is not what scrolling alone makes it (cleared by the program).
//! Scrollback rows never change, so only the live rows are compared, row by row (a row the cap
//! pushed out is skipped). While the user holds a drag only the
//! anchor follows its text: the head stays on its screen cell and nothing is compared.
//! Only a program that hasn't asked for the mouse gets a selection (`tasks::mouse` keeps every
//! event when it has), which the mouse wiring decides.

use vt100::Screen;

/// `screen`'s `(rows, cols)` as plain numbers (`vt100` 0.18 returns `NonZeroU16`s).
pub fn size(screen: &Screen) -> (u16, u16) {
    let (rows, cols) = screen.size();
    (rows.get(), cols.get())
}

/// A cell of the terminal's text, independent of the scroll: `(line, col)` where `line` counts
/// from the live screen's first row (0), so older output in the scrollback is negative. The
/// viewport row `r` shown `back` rows scrolled back is line `r - back`.
pub type Cell = (isize, u16);

/// The cell at viewport `row` / `col` of a view scrolled `back` rows back.
pub fn cell_at(row: u16, col: u16, back: usize) -> Cell {
    (row as isize - back as isize, col)
}

/// The viewport row `line` is at when scrolled `back` rows back: negative or past the screen
/// when it is out of view.
pub fn viewport_row(line: isize, back: usize) -> isize {
    line + back as isize
}

/// A left-drag over a terminal's screen: `anchor` is where the button went down and `head`
/// where the drag is now. The cell at `head` is not part of the text: its column is the end
/// `contents_between` excludes, so a click without a drag (`anchor == head`) selects nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cell,
    pub head: Cell,
}

/// The text `selection` covers on `screen`: from the anchor to the head in row order (a drag
/// upwards or leftwards copies the same text as the opposite one), soft-wrapped rows joined
/// without a line break, and trailing spaces trimmed off each line. Empty when it covers no
/// cell. Reads wherever the selection is, in view or not, by scrolling `screen` a window at a
/// time (restored afterwards); lines older than the kept scrollback clamp to its oldest row.
pub fn copy_text(screen: &mut Screen, selection: &Selection) -> String {
    let (rows, cols) = size(screen);
    let back = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let max = screen.scrollback() as isize;
    let (mut start, mut end) = selection.rows();
    if start.0 < -max {
        start = (-max, 0);
    }
    let last_line = rows as isize - 1;
    if end.0 > last_line {
        end = (last_line, cols);
    }
    let mut text = String::new();
    let mut line = start.0;
    let mut first = true;
    while line <= end.0 {
        // The window starting at `line` (or the live screen when it starts below its top).
        let k = (-line).clamp(0, max) as usize;
        screen.set_scrollback(k);
        let top = -(k as isize);
        let last = end.0.min(top + rows as isize - 1);
        let from = if first { start.1 } else { 0 };
        let to = if last == end.0 { end.1 } else { cols };
        let r0 = (line - top) as u16;
        let r1 = (last - top) as u16;
        text.push_str(&screen.contents_between(r0, from, r1, to));
        if last < end.0 && !screen.row_wrapped(r1) {
            text.push('\n');
        }
        first = false;
        line = last + 1;
    }
    screen.set_scrollback(back);
    text.split('\n')
        .map(|line| line.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Selection {
    /// The selection after `lines` lines scrolled into a scrollback of `kept` rows: both cells
    /// move up by `lines`; a cell older than the oldest kept row (line `-kept`) clamps to its
    /// start. `None` when the whole selection is older than that row (no text is left).
    pub fn shift(self, lines: usize, kept: usize) -> Option<Selection> {
        let (_, end) = self.rows();
        if end.0.saturating_sub(lines as isize) < -(kept as isize) {
            return None;
        }
        Some(Selection {
            anchor: shift_cell(self.anchor, lines, kept),
            head: shift_cell(self.head, lines, kept),
        })
    }

    /// Like `shift` for a drag in progress: only the anchor follows its text, the head stays
    /// where the pointer put it (clamped like the anchor).
    pub fn shift_anchor(self, lines: usize, kept: usize) -> Selection {
        Selection {
            anchor: shift_cell(self.anchor, lines, kept),
            ..self
        }
    }

    /// The drag's `(start, end)` in row order: `contents_between` takes the first cell first,
    /// whichever way the drag went (tuples compare line first).
    pub fn rows(&self) -> (Cell, Cell) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

fn shift_cell((line, col): Cell, lines: usize, kept: usize) -> Cell {
    let oldest = -(kept as isize);
    let line = line.saturating_sub(lines as isize);
    if line < oldest { (oldest, 0) } else { (line, col) }
}

/// The part of `selection` on each live-screen row (lines 0 and below), one single-row
/// selection per row, top to bottom; empty without one.
fn live_rows(screen: &Screen, selection: &Selection) -> Vec<Selection> {
    let (rows, cols) = size(screen);
    let (start, end) = selection.rows();
    let first = start.0.max(0);
    let last = end.0.min(rows as isize - 1);
    (first..=last)
        .map(|line| Selection {
            anchor: (line, if line == start.0 { start.1 } else { 0 }),
            head: (line, if line == end.0 { end.1 } else { cols }),
        })
        .collect()
}

/// What `follow` needs from before a chunk of output is processed.
pub struct Snapshot {
    scrolled: u64,
    kept: usize,
    /// The selection's part on each live-screen row and its text.
    live: Vec<(Selection, String)>,
}

impl Snapshot {
    /// `Screen::scrolled_lines` when it was taken.
    pub fn scrolled(&self) -> u64 {
        self.scrolled
    }
}

/// Records `screen` (before output) for `follow`.
pub fn snapshot(screen: &mut Screen, selection: &Selection) -> Snapshot {
    let live = live_rows(screen, selection)
        .into_iter()
        .map(|row| (row, copy_text(screen, &row)))
        .collect();
    Snapshot {
        scrolled: screen.scrolled_lines(),
        kept: screen.scrollback_len(),
        live,
    }
}

/// The selection after output was processed, `None` when it no longer covers the same text (see
/// the module doc). `dragging`: only the anchor follows, and nothing is compared.
pub fn follow(screen: &mut Screen, selection: Selection, before: &Snapshot, dragging: bool) -> Option<Selection> {
    let lines = screen.scrolled_lines().saturating_sub(before.scrolled);
    let lines = usize::try_from(lines).unwrap_or(usize::MAX / 2).min(usize::MAX / 2);
    let kept = screen.scrollback_len();
    if dragging {
        return Some(selection.shift_anchor(lines, kept));
    }
    if kept != before.kept.saturating_add(lines).min(screen.scrollback_capacity()) {
        return None;
    }
    // Each live row the selection covered must show the same text where it scrolled to; a row
    // pushed past the kept scrollback is gone (the selection clamps), so it isn't compared.
    for (row, text) in &before.live {
        let line = row.anchor.0.saturating_sub(lines as isize);
        if line < -(kept as isize) {
            continue;
        }
        let moved = Selection {
            anchor: (line, row.anchor.1),
            head: (line, row.head.1),
        };
        if copy_text(screen, &moved) != *text {
            return None;
        }
    }
    selection.shift(lines, kept)
}

/// Whether `c` can be part of a path: ASCII alphanumeric or one of `/ : - _ .~`.
/// ~ is included for home path expansion.
fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '-' | '_' | '.' | '~')
}

/// Whether the viewport cell `(row, col)` of `screen` holds a path character (a wide character,
/// a blank or an empty cell never does).
fn path_cell(screen: &Screen, row: u16, col: u16) -> bool {
    screen.cell(row, col).is_some_and(|cell| {
        let mut chars = cell.contents().chars();
        matches!((chars.next(), chars.next()), (Some(c), None) if is_path_char(c))
    })
}

/// A parser of plain `u16` dimensions, for tests.
#[cfg(test)]
pub fn test_parser(rows: u16, cols: u16, scrollback: usize) -> vt100::Parser {
    let nz = |n| std::num::NonZeroU16::new(n).unwrap();
    vt100::Parser::new(nz(rows), nz(cols), scrollback)
}

/// The run of path characters around viewport cell `(row, col)` of `screen`, as `(start, end)`
/// cells on that row, `end` exclusive (the `Selection` convention): the longest contiguous
/// run of alphanumerics and `/ : - _ . ~` containing the cell, stopped by any other character, a
/// blank cell or the row's edges. `None` when the cell itself is not a path character.
pub fn path_run(screen: &Screen, (row, col): (u16, u16)) -> Option<(Cell, Cell)> {
    let (rows, cols) = size(screen);
    if row >= rows || col >= cols || !path_cell(screen, row, col) {
        return None;
    }
    let mut start = col;
    while start > 0 && path_cell(screen, row, start - 1) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < cols && path_cell(screen, row, end) {
        end += 1;
    }
    let line = row as isize - screen.scrollback() as isize;
    Some(((line, start), (line, end)))
}

/// The first row to draw of a `screen` shown in `height` rows: 0 when it fits, else as low as
/// keeps its last non-blank row and the cursor in view.
pub fn top_row(screen: &Screen, height: u16) -> u16 {
    let (rows, cols) = size(screen);
    if rows <= height {
        return 0;
    }
    let last = screen
        .rows(0, cols)
        .enumerate()
        .filter(|(_, text)| !text.trim_end().is_empty())
        .map(|(i, _)| i as u16)
        .last()
        .unwrap_or(0)
        .max(screen.cursor_position().0);
    (last + 1).saturating_sub(height).min(rows - height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: u16, cols: u16, output: &[u8]) -> vt100::Parser {
        let mut p = test_parser(rows, cols, 0);
        p.process(output);
        p
    }

    fn sel(a: (isize, u16), h: (isize, u16)) -> Selection {
        Selection { anchor: a, head: h }
    }

    #[test]
    fn shift_moves_both_cells_up_and_keeps_their_order() {
        let up = sel((2, 1), (4, 3)).shift(3, 10).unwrap();
        assert_eq!(up, sel((-1, 1), (1, 3)));
        // A backwards drag keeps anchor after head.
        let back = sel((4, 3), (2, 1)).shift(3, 10).unwrap();
        assert_eq!(back, sel((1, 3), (-1, 1)));
        assert_eq!(sel((1, 1), (1, 1)).shift(0, 10), Some(sel((1, 1), (1, 1))));
    }

    #[test]
    fn shift_clamps_to_the_oldest_kept_row_and_drops_what_is_gone() {
        // The start is older than the kept 5 rows: clamps to the oldest row's start.
        assert_eq!(sel((-3, 2), (1, 4)).shift(4, 5), Some(sel((-5, 0), (-3, 4))));
        assert_eq!(sel((1, 4), (-3, 2)).shift(4, 5), Some(sel((-3, 4), (-5, 0))));
        // The end on the oldest row is still partly there; one row older is gone.
        assert_eq!(sel((-1, 0), (-1, 2)).shift(4, 5), Some(sel((-5, 0), (-5, 2))));
        assert_eq!(sel((-1, 0), (-1, 2)).shift(5, 5), None);
        assert_eq!(sel((3, 0), (-2, 2)).shift(100, 0), None);
        assert_eq!(sel((1, 0), (2, 2)).shift(usize::MAX / 2, 5), None);
    }

    #[test]
    fn shift_anchor_leaves_the_head() {
        assert_eq!(sel((2, 1), (4, 3)).shift_anchor(3, 10), sel((-1, 1), (4, 3)));
        assert_eq!(sel((2, 1), (4, 3)).shift_anchor(30, 10), sel((-10, 0), (4, 3)));
    }

    /// Processes `output` after a selection was made, as the reader does.
    fn follow_output(p: &mut vt100::Parser, selection: Selection, output: &[u8]) -> Option<Selection> {
        let before = snapshot(p.screen_mut(), &selection);
        p.process(output);
        follow(p.screen_mut(), selection, &before, false)
    }

    #[test]
    fn follow_scrolls_with_the_text_and_clears_on_in_place_rewrites() {
        let mut p = test_parser(4, 10, 20);
        p.process(b"a\r\nb\r\nc\r\nd");
        // Rows b..c selected (lines 1..2).
        let s = sel((1, 0), (2, 1));
        assert_eq!(copy_text(p.screen_mut(), &s), "b\nc");
        // Two new lines scroll it up by two; the text is the same.
        let moved = follow_output(&mut p, s, b"\r\ne\r\nf").unwrap();
        assert_eq!(moved, sel((-1, 0), (0, 1)));
        assert_eq!(copy_text(p.screen_mut(), &moved), "b\nc");
        // A progress line below the selection (rewritten with \r) leaves it.
        assert_eq!(follow_output(&mut p, moved, b"\rxyz"), Some(moved));
        // Rewriting a selected live row in place clears it.
        let live = sel((0, 0), (0, 1));
        assert_eq!(follow_output(&mut p, live, b"\x1b[1;1HZ"), None);
        // So does erasing the screen, or entering the alternate screen.
        assert_eq!(follow_output(&mut p, live, b"\x1b[2J"), None);
        let mut q = test_parser(4, 10, 20);
        q.process(b"text");
        assert_eq!(follow_output(&mut q, sel((0, 0), (0, 4)), b"\x1b[?1049h"), None);
    }

    #[test]
    fn follow_handles_the_scrollback_cap_and_clearing() {
        let mut p = test_parser(3, 10, 4);
        p.process(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7");
        assert_eq!(p.screen().scrollback_len(), 4);
        // Lines -4..-2 (older rows only): pushed one line, the oldest falls off the cap.
        let s = sel((-4, 0), (-2, 1));
        let moved = follow_output(&mut p, s, b"\r\n8").unwrap();
        assert_eq!(moved, sel((-4, 0), (-3, 1)));
        // Pushed out entirely.
        let all = sel((-4, 0), (-3, 1));
        assert_eq!(follow_output(&mut p, all, b"\r\n9\r\n10\r\n11\r\n12\r\n13"), None);
        // A selection on live rows the cap partly pushes off clamps to the oldest kept row, whose
        // text is still compared; one the cap pushes off wholly is dropped.
        let mut q = test_parser(3, 10, 1);
        q.process(b"0\r\n1\r\n2");
        let clamped = follow_output(&mut q, sel((0, 0), (2, 1)), b"\r\n3\r\n4\r\n5").unwrap();
        assert_eq!(clamped, sel((-1, 0), (-1, 1)));
        assert_eq!(copy_text(q.screen_mut(), &clamped), "2");
        let mut q = test_parser(3, 10, 1);
        q.process(b"0\r\n1\r\n2");
        assert_eq!(follow_output(&mut q, sel((0, 0), (1, 1)), b"\r\n3\r\n4\r\n5"), None);
        // Clamped with a row still live: kept while it is unchanged, cleared when rewritten.
        let mut q = test_parser(3, 10, 1);
        q.process(b"0\r\n1\r\n2");
        let kept = follow_output(&mut q, sel((0, 0), (2, 1)), b"\r\n3\r\n4").unwrap();
        assert_eq!(kept, sel((-1, 0), (0, 1)));
        assert_eq!(copy_text(q.screen_mut(), &kept), "1\n2");
        let mut q = test_parser(3, 10, 1);
        q.process(b"0\r\n1\r\n2");
        assert_eq!(
            follow_output(&mut q, sel((0, 0), (2, 1)), b"\r\n3\r\n4\x1b[1;1HZ\x1b[3;2H"),
            None
        );
        // The program clearing the scrollback is a change too.
        let mut r = test_parser(3, 10, 4);
        r.process(b"0\r\n1\r\n2\r\n3\r\n4");
        assert_eq!(follow_output(&mut r, sel((-2, 0), (-1, 1)), b"\x1b[3J"), None);
        // Also for one reaching into the live rows (unchanged): the clamp alone would keep it.
        let mut r = test_parser(3, 10, 4);
        r.process(b"0\r\n1\r\n2\r\n3\r\n4");
        let s = sel((-1, 0), (1, 1));
        assert_eq!(copy_text(r.screen_mut(), &s), "1\n2\n3");
        assert_eq!(follow_output(&mut r, s, b"\x1b[3J"), None);
        assert_eq!(
            r.screen().contents().lines().next(),
            Some("2"),
            "the live rows didn't change"
        );
    }

    #[test]
    fn follow_while_dragging_moves_only_the_anchor() {
        let mut p = test_parser(3, 10, 10);
        p.process(b"a\r\nb\r\nc");
        let s = sel((0, 0), (2, 1));
        let before = snapshot(p.screen_mut(), &s);
        p.process(b"\r\nd\x1b[1;1HZ");
        assert_eq!(follow(p.screen_mut(), s, &before, true), Some(sel((-1, 0), (2, 1))));
    }

    #[test]
    fn path_run_bounds_and_character_set() {
        let p = screen(3, 20, b"~/src/jed-0.1 x_y:2 \r\nabcdefghijklmnopqrst");
        let s = p.screen();
        // The run includes `~` and stops at the blank; every cell of it gives the same range.
        for col in 0..13 {
            assert_eq!(path_run(s, (0, col)), Some(((0, 0), (0, 13))), "col {col}");
        }
        assert_eq!(path_run(s, (0, 14)), Some(((0, 14), (0, 19))), "`_` and `:` count");
        assert_eq!(path_run(s, (0, 13)), None, "a blank");
        assert_eq!(path_run(s, (0, 19)), None, "a blank");
        // A full row runs to both edges.
        assert_eq!(path_run(s, (1, 7)), Some(((1, 0), (1, 20))));
        // An empty cell and a cell off the screen select nothing.
        assert_eq!(path_run(s, (2, 3)), None);
        assert_eq!(path_run(s, (5, 3)), None);
        assert_eq!(path_run(s, (0, 25)), None);
    }

    #[test]
    fn path_run_excludes_other_characters() {
        // Each of these stops the run `a/b` and selects nothing itself: shell punctuation,
        // quotes, brackets, and a non-ASCII letter (alphanumeric, but not ASCII).
        for c in [',', '@', '\'', '"', '(', ')', '[', '=', '+', '*', '\\', '|', 'é'] {
            let p = screen(1, 10, format!("{c}a/b{c}").as_bytes());
            let s = p.screen();
            assert_eq!(path_run(s, (0, 2)), Some(((0, 1), (0, 4))), "{c:?} bounds the run");
            assert_eq!(path_run(s, (0, 0)), None, "{c:?} is not a path character");
        }
    }

    #[test]
    fn path_run_lines_are_scroll_independent() {
        // Scrolled back 2 rows, viewport row 0 is the line 2 above the live screen's first.
        let mut p = test_parser(2, 10, 10);
        p.process(b"ab.c\r\ntwo\r\nthree\r\nfour");
        p.screen_mut().set_scrollback(2);
        let s = p.screen();
        assert_eq!(s.contents_between(0, 0, 0, 4), "ab.c");
        assert_eq!(path_run(s, (0, 1)), Some(((-2, 0), (-2, 4))));
        assert_eq!(cell_at(0, 0, s.scrollback()), (-2, 0), "the same space as `cell_at`");
    }

    #[test]
    fn path_run_stops_at_a_wide_character() {
        let p = screen(1, 12, "ab\u{4f60}cd".as_bytes());
        let s = p.screen();
        assert_eq!(path_run(s, (0, 0)), Some(((0, 0), (0, 2))));
        assert_eq!(path_run(s, (0, 2)), None, "the wide character's cell");
        assert_eq!(path_run(s, (0, 3)), None, "its continuation cell");
        assert_eq!(path_run(s, (0, 5)), Some(((0, 4), (0, 6))));
    }

    fn selection(anchor: (isize, u16), head: (isize, u16)) -> Selection {
        Selection { anchor, head }
    }

    /// A drag that ends left of / above its anchor covers the same cells as the forward one,
    /// and only `head`'s own cell is left out.
    #[test]
    fn copy_is_the_same_both_ways() {
        let mut p = screen(4, 20, b"one\r\ntwo\r\nthree");
        let forward = selection((0, 0), (1, 1));
        assert_eq!(copy_text(p.screen_mut(), &forward), "one\nt");
        assert_eq!(
            copy_text(p.screen_mut(), &selection((1, 1), (0, 0))),
            copy_text(p.screen_mut(), &forward)
        );
        assert_eq!(
            copy_text(p.screen_mut(), &selection((0, 0), (0, 3))),
            "one",
            "a same-row drag stops before the head's column"
        );
        assert_eq!(
            copy_text(p.screen_mut(), &selection((0, 3), (0, 0))),
            "one",
            "a same-row drag leftwards covers the same cells"
        );
        assert_eq!(
            copy_text(p.screen_mut(), &selection((1, 1), (1, 1))),
            "",
            "a click without a drag"
        );
    }

    #[test]
    fn trailing_spaces_are_trimmed_per_line() {
        let mut p = screen(4, 20, b"a   \r\nb  \r\nc d ");
        assert_eq!(
            copy_text(p.screen_mut(), &selection((0, 0), (2, 4))),
            "a\nb\nc d",
            "inner spaces stay"
        );
        assert_eq!(
            copy_text(p.screen_mut(), &selection((2, 0), (2, 4))),
            "c d",
            "a single row"
        );
    }

    /// Only trailing spaces are trimmed: leading spaces are part of the line (the trim is
    /// `trim_end_matches(' ')`, not `trim`).
    #[test]
    fn leading_spaces_are_kept() {
        let mut p = screen(2, 5, b"  x\r\n  y");
        assert_eq!(copy_text(p.screen_mut(), &selection((0, 0), (1, 5))), "  x\n  y");
    }

    /// A line wider than the screen wraps: `contents_between` joins the rows with no line
    /// break, and the joined line is what gets trimmed.
    #[test]
    fn wrapped_rows_are_joined() {
        let mut p = test_parser(2, 5, 0);
        p.process(b"abcdefgh");
        assert!(p.screen().row_wrapped(0));
        assert_eq!(copy_text(p.screen_mut(), &selection((0, 0), (1, 3))), "abcdefgh");
        let mut padded = test_parser(2, 5, 0);
        padded.process(b"abcdef  ");
        assert_eq!(copy_text(padded.screen_mut(), &selection((0, 0), (1, 3))), "abcdef");
    }

    /// Cells are scroll-independent: a selection made scrolled back copies the same text
    /// from any scroll, and the scroll is restored.
    #[test]
    fn copy_reads_the_scrolled_back_text_from_any_view() {
        let mut p = test_parser(3, 20, 10);
        p.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        assert_eq!(p.screen().contents(), "three\nfour\nfive");
        p.screen_mut().set_scrollback(1);
        let sel = Selection {
            anchor: cell_at(0, 0, 1),
            head: cell_at(1, 6, 1),
        };
        assert_eq!(sel.rows().0, (-1, 0));
        assert_eq!(copy_text(p.screen_mut(), &sel), "two\nthree");
        assert_eq!(p.screen().scrollback(), 1, "restored");
        p.screen_mut().set_scrollback(0);
        assert_eq!(copy_text(p.screen_mut(), &sel), "two\nthree");
        assert_eq!(p.screen().scrollback(), 0);
    }

    /// A selection taller than the screen is read in windows, wrapped rows joined across them.
    #[test]
    fn a_selection_taller_than_the_screen() {
        let mut p = test_parser(3, 5, 50);
        p.process(b"l0\r\nl1\r\nl2\r\nabcdefghij\r\nl4\r\nl5\r\nl6\r\nl7");
        p.screen_mut().set_scrollback(usize::MAX);
        let max = p.screen().scrollback() as isize;
        assert_eq!(max, 6);
        let all = Selection {
            anchor: (-max, 0),
            head: (2, 5),
        };
        assert_eq!(
            copy_text(p.screen_mut(), &all),
            "l0\nl1\nl2\nabcdefghij\nl4\nl5\nl6\nl7"
        );
        p.screen_mut().set_scrollback(0);
        let part = Selection {
            anchor: (-3, 2),
            head: (-1, 1),
        };
        assert_eq!(copy_text(p.screen_mut(), &part), "cdefghij\nl");
        assert_eq!(viewport_row(-3, 2), -1);
    }

    /// The windows' seams: a wrapped row that is the last of one window joins the next
    /// window's first without a line break, an unwrapped one gets one, and the head's
    /// exclusive column applies in whichever window holds it (none of a row at column 0).
    #[test]
    fn windows_join_wrapped_rows_and_end_at_the_head() {
        // Rows (5 wide): aa, bb, ccccd (wrapped), ddd, ee, ff, gg: lines -4..=2 on 3 rows, so
        // a selection from line -4 reads -4..=-2, then -1..=1.
        let mut p = test_parser(3, 5, 50);
        p.process(b"aa\r\nbb\r\nccccdddd\r\nee\r\nff\r\ngg");
        p.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(p.screen().scrollback(), 4);
        p.screen_mut().set_scrollback(0);
        let mut from = |head| copy_text(p.screen_mut(), &selection((-4, 0), head));
        assert_eq!(from((0, 1)), "aa\nbb\nccccdddd\ne", "wrapped across the seam");
        assert_eq!(
            from((-1, 0)),
            "aa\nbb\nccccd",
            "the head at the next window's first cell"
        );
        assert_eq!(from((-1, 2)), "aa\nbb\nccccddd");
        assert_eq!(from((-2, 5)), "aa\nbb\nccccd", "the head ending the first window");
        // An unwrapped seam: from line -3 the windows are -3..=-1 and 0..=2.
        assert_eq!(
            copy_text(p.screen_mut(), &selection((-3, 1), (1, 1))),
            "b\nccccdddd\nee\nf"
        );
        assert_eq!(
            copy_text(p.screen_mut(), &selection((-1, 3), (0, 2))),
            "\nee",
            "past ddd's end"
        );
        // Older than the kept scrollback clamps to its oldest row.
        assert_eq!(copy_text(p.screen_mut(), &selection((-40, 3), (-3, 1))), "aa\nb");
        assert_eq!(p.screen().scrollback(), 0, "restored");
    }

    #[test]
    fn a_taller_screen_keeps_its_last_line_in_view() {
        let mut p = test_parser(10, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        assert_eq!(top_row(p.screen(), 10), 0, "fits");
        assert_eq!(top_row(p.screen(), 4), 0, "the output is at the top");
        assert_eq!(top_row(p.screen(), 2), 1);
        p.process(b"\r\n\r\n\r\n\r\nseven\r\n");
        assert_eq!(top_row(p.screen(), 4), 4, "the cursor below the last line");
    }
}
