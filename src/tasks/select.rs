//! Selecting a terminal's text with the mouse: the drag's cells, the text they copy and the
//! first row a taller screen is drawn from. No drawing and no app state.
//!
//! A `Selection`'s cells are scroll-independent (`Cell`: line 0 is the live screen's first row,
//! scrollback is negative), so a scroll leaves it on the same text; `copy_text` is
//! `contents_between` over the range the drag spans, window by window, with trailing spaces
//! trimmed per line. New output and a resize clear it (the rows reflow).
//! Only a program that hasn't asked for the mouse gets a selection (`tasks::mouse` keeps every
//! event when it has), which the mouse wiring decides.

use vt100::Screen;

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
    let (rows, cols) = screen.size();
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

/// The first row to draw of a `screen` shown in `height` rows: 0 when it fits, else as low as
/// keeps its last non-blank row and the cursor in view.
pub fn top_row(screen: &Screen, height: u16) -> u16 {
    let (rows, cols) = screen.size();
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
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process(output);
        p
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
        let mut p = vt100::Parser::new(2, 5, 0);
        p.process(b"abcdefgh");
        assert!(p.screen().row_wrapped(0));
        assert_eq!(copy_text(p.screen_mut(), &selection((0, 0), (1, 3))), "abcdefgh");
        let mut padded = vt100::Parser::new(2, 5, 0);
        padded.process(b"abcdef  ");
        assert_eq!(copy_text(padded.screen_mut(), &selection((0, 0), (1, 3))), "abcdef");
    }

    /// Cells are scroll-independent: a selection made scrolled back copies the same text
    /// from any scroll, and the scroll is restored.
    #[test]
    fn copy_reads_the_scrolled_back_text_from_any_view() {
        let mut p = vt100::Parser::new(3, 20, 10);
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
        let mut p = vt100::Parser::new(3, 5, 50);
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
        let mut p = vt100::Parser::new(3, 5, 50);
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
        let mut p = vt100::Parser::new(10, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        assert_eq!(top_row(p.screen(), 10), 0, "fits");
        assert_eq!(top_row(p.screen(), 4), 0, "the output is at the top");
        assert_eq!(top_row(p.screen(), 2), 1);
        p.process(b"\r\n\r\n\r\n\r\nseven\r\n");
        assert_eq!(top_row(p.screen(), 4), 4, "the cursor below the last line");
    }
}
