//! Searching a terminal's text, scrollback included: the matches of a query on a `vt100` screen
//! as `select::Cell` ranges (line 0 is the live screen's first row, scrollback is negative), so
//! a scroll leaves them on their text. No drawing and no app state; `app/term_search.rs` drives
//! it and `ui::task` draws the bar and the highlights.
//!
//! A match lies within one screen row (a soft-wrapped line is searched row by row), is literal
//! (no regex or whole word) and is case-insensitive unless asked. Wide characters count their
//! two columns; empty cells read as spaces.

use vt100::Screen;

use super::RunId;
use super::select::{self, Cell};
use crate::field::TextField;

/// The search bar's state over one run (`App::term_search`): the query, the case option and the
/// match the bar stands on, with the count as of the last search ("3 of 12"; output arriving
/// afterwards doesn't change it until the next query edit or step).
pub struct TermSearch {
    pub run: RunId,
    pub field: TextField,
    pub case: bool,
    pub current: Option<Match>,
    pub count: usize,
    pub index: Option<usize>,
}

impl TermSearch {
    pub fn new(run: RunId) -> Self {
        Self {
            run,
            field: TextField::default(),
            case: false,
            current: None,
            count: 0,
            index: None,
        }
    }

    /// "3 of 12", "No results", or nothing for an empty query.
    pub fn label(&self) -> String {
        match (self.field.text.is_empty(), self.index) {
            (true, _) => String::new(),
            (false, Some(i)) => format!("{} of {}", i + 1, self.count),
            (false, None) => "No results".into(),
        }
    }
}

/// One match: `line` as in `select::Cell`, columns `start..end` (end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub line: isize,
    pub start: u16,
    pub end: u16,
}

impl Match {
    pub fn cell(&self) -> Cell {
        (self.line, self.start)
    }
}

/// One row as `(char, column)` pairs; a wide character's continuation cell has no entry.
fn row_chars(screen: &Screen, row: u16, cols: u16) -> Vec<(char, u16)> {
    let mut out = Vec::with_capacity(cols as usize);
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            out.push((' ', col));
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let text = cell.contents();
        match text.chars().next() {
            Some(c) => out.push((c, col)),
            None => out.push((' ', col)),
        }
    }
    out
}

fn fold(c: char, case: bool) -> char {
    if case { c } else { c.to_lowercase().next().unwrap_or(c) }
}

/// The matches of `query` (non-overlapping, left to right) on `row` of the screen as drawn.
fn row_matches(screen: &Screen, row: u16, cols: u16, line: isize, query: &[char], case: bool, out: &mut Vec<Match>) {
    let chars = row_chars(screen, row, cols);
    if chars.len() < query.len() {
        return;
    }
    let mut i = 0;
    while i + query.len() <= chars.len() {
        if query.iter().enumerate().all(|(k, &q)| fold(chars[i + k].0, case) == q) {
            let last = chars[i + query.len() - 1].1;
            let wide = screen.cell(row, last).is_some_and(|c| c.is_wide());
            out.push(Match {
                line,
                start: chars[i].1,
                end: last + 1 + u16::from(wide),
            });
            i += query.len();
        } else {
            i += 1;
        }
    }
}

fn folded(query: &str, case: bool) -> Vec<char> {
    query.chars().map(|c| fold(c, case)).collect()
}

/// Every match in the scrollback and on the live screen, oldest first. Scrolls `screen` a window
/// at a time and restores its offset.
pub fn find_all(screen: &mut Screen, query: &str, case: bool) -> Vec<Match> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let query = folded(query, case);
    let (rows, cols) = select::size(screen);
    let back = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let max = screen.scrollback();
    // Window by window from the oldest line, as `select::copy_text` reads: the window at `line`
    // (the live screen once `line` is 0 or below it) shows lines `top..top + rows`.
    let mut line = -(max as isize);
    while line < rows as isize {
        let k = (-line).clamp(0, max as isize) as usize;
        screen.set_scrollback(k);
        let top = -(k as isize);
        for row in (line - top) as u16..rows {
            row_matches(screen, row, cols, top + row as isize, &query, case, &mut out);
        }
        line = top + rows as isize;
    }
    screen.set_scrollback(back);
    out
}

/// The matches on the rows currently shown (`screen` at its own scroll offset), for the
/// highlights; lines are in `select::Cell` space.
pub fn find_visible(screen: &Screen, query: &str, case: bool) -> Vec<Match> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let query = folded(query, case);
    let (rows, cols) = select::size(screen);
    let back = screen.scrollback();
    for row in 0..rows {
        row_matches(
            screen,
            row,
            cols,
            select::cell_at(row, 0, back).0,
            &query,
            case,
            &mut out,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::select::test_parser;

    fn m(line: isize, start: u16, end: u16) -> Match {
        Match { line, start, end }
    }

    /// Matches come oldest first across the scrollback and the live screen, each window read once.
    #[test]
    fn find_all_reads_scrollback_and_screen() {
        let mut p = test_parser(3, 20, 100);
        for i in 0..8 {
            p.process(format!("line {i} foo\r\n").as_bytes());
        }
        p.process(b"foo foo");
        let screen = p.screen_mut();
        screen.set_scrollback(2);
        let all = find_all(screen, "foo", false);
        assert_eq!(screen.scrollback(), 2, "the view is restored");
        // 8 lines + the last row = 9 lines scrolled/live; screen rows are lines 0..3
        assert_eq!(all.len(), 10);
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(all[0], m(-(screen.scrollback_len() as isize), 7, 10));
        assert_eq!(&all[8..], &[m(2, 0, 3), m(2, 4, 7)]);
    }

    #[test]
    fn case_folding_and_wide_characters() {
        let mut p = test_parser(2, 20, 0);
        p.process("中Foo bar FOO".as_bytes());
        let s = p.screen();
        assert_eq!(find_visible(s, "foo", false), vec![m(0, 2, 5), m(0, 10, 13)]);
        assert_eq!(find_visible(s, "foo", true), vec![]);
        assert_eq!(find_visible(s, "中", true), vec![m(0, 0, 2)]);
        assert_eq!(find_visible(s, "", false), vec![]);
    }

    /// `find_visible` gives lines in the scroll-independent space.
    #[test]
    fn visible_matches_follow_the_scroll() {
        let mut p = test_parser(2, 10, 10);
        p.process(b"a x\r\nb\r\nc\r\nd");
        p.screen_mut().set_scrollback(2);
        assert_eq!(find_visible(p.screen(), "x", false), vec![m(-2, 2, 3)]);
    }
}
