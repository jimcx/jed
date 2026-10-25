//! Selecting a terminal's text with the mouse: the drag's cells, the text they copy and the
//! first row a taller screen is drawn from. No drawing and no app state.
//!
//! A `Selection`'s cells are in the screen's *visible viewport* coordinates — the space
//! `vt100::Screen::cell` / `contents_between` use, so scrollback is folded in — and `copy_text`
//! is `contents_between` over the range the drag spans, with trailing spaces trimmed per line.
//! Only a program that hasn't asked for the mouse gets a selection (`tasks::mouse` keeps every
//! event when it has), which the mouse wiring decides.

use vt100::Screen;

/// A left-drag over a terminal's screen: `anchor` is where the button went down and `head`
/// where the drag is now, as `(row, col)` cells in the screen's visible viewport. The cell at
/// `head` is not part of the text: its column is the end `contents_between` excludes, so a
/// click without a drag (`anchor == head`) selects nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
}

/// The text `selection` covers on `screen`: from the anchor to the head in row order (a drag
/// upwards or leftwards copies the same text as the opposite one), soft-wrapped rows joined
/// without a line break, and trailing spaces trimmed off each line. Empty when it covers no
/// cell.
///
/// Only the tests call this until the mouse wiring copies through it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn copy_text(screen: &Screen, selection: &Selection) -> String {
    let (start, end) = selection.rows();
    screen
        .contents_between(start.0, start.1, end.0, end.1)
        .split('\n')
        .map(|line| line.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Selection {
    /// The drag's `(start, end)` in row order: `contents_between` takes the first cell first,
    /// whichever way the drag went (tuples compare row first).
    pub fn rows(&self) -> ((u16, u16), (u16, u16)) {
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

    fn selection(anchor: (u16, u16), head: (u16, u16)) -> Selection {
        Selection { anchor, head }
    }

    /// A drag that ends left of / above its anchor covers the same cells as the forward one,
    /// and only `head`'s own cell is left out.
    #[test]
    fn copy_is_the_same_both_ways() {
        let p = screen(4, 20, b"one\r\ntwo\r\nthree");
        let forward = selection((0, 0), (1, 1));
        assert_eq!(copy_text(p.screen(), &forward), "one\nt");
        assert_eq!(
            copy_text(p.screen(), &selection((1, 1), (0, 0))),
            copy_text(p.screen(), &forward)
        );
        assert_eq!(
            copy_text(p.screen(), &selection((0, 0), (0, 3))),
            "one",
            "a same-row drag stops before the head's column"
        );
        assert_eq!(
            copy_text(p.screen(), &selection((0, 3), (0, 0))),
            "one",
            "a same-row drag leftwards covers the same cells"
        );
        assert_eq!(
            copy_text(p.screen(), &selection((1, 1), (1, 1))),
            "",
            "a click without a drag"
        );
    }

    #[test]
    fn trailing_spaces_are_trimmed_per_line() {
        let p = screen(4, 20, b"a   \r\nb  \r\nc d ");
        assert_eq!(
            copy_text(p.screen(), &selection((0, 0), (2, 4))),
            "a\nb\nc d",
            "inner spaces stay"
        );
        assert_eq!(copy_text(p.screen(), &selection((2, 0), (2, 4))), "c d", "a single row");
    }

    /// Only trailing spaces are trimmed: leading spaces are part of the line (the trim is
    /// `trim_end_matches(' ')`, not `trim`).
    #[test]
    fn leading_spaces_are_kept() {
        let p = screen(2, 5, b"  x\r\n  y");
        assert_eq!(copy_text(p.screen(), &selection((0, 0), (1, 5))), "  x\n  y");
    }

    /// A line wider than the screen wraps: `contents_between` joins the rows with no line
    /// break, and the joined line is what gets trimmed.
    #[test]
    fn wrapped_rows_are_joined() {
        let mut p = vt100::Parser::new(2, 5, 0);
        p.process(b"abcdefgh");
        assert!(p.screen().row_wrapped(0));
        assert_eq!(copy_text(p.screen(), &selection((0, 0), (1, 3))), "abcdefgh");
        let mut padded = vt100::Parser::new(2, 5, 0);
        padded.process(b"abcdef  ");
        assert_eq!(copy_text(padded.screen(), &selection((0, 0), (1, 3))), "abcdef");
    }

    /// The cells are visible-viewport cells: scrolled back, the same cell reads the text that
    /// is on screen then.
    #[test]
    fn copy_reads_the_scrolled_back_view() {
        let mut p = vt100::Parser::new(3, 20, 10);
        p.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        assert_eq!(p.screen().contents(), "three\nfour\nfive");
        p.screen_mut().set_scrollback(1);
        assert_eq!(p.screen().scrollback(), 1);
        assert_eq!(copy_text(p.screen(), &selection((0, 0), (1, 6))), "two\nthree");
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
