//! The Search view's model (no drawing; `ui::search` draws it): the query input and its
//! options, the debounce that restarts the search as the query changes, the error of an invalid
//! regex, and the results list's scroll position. The results themselves are `App::search`
//! (`workspace_search::Results`).
//!
//! - The options are the view's own (VS Code keeps the Search view's apart from the find
//!   widget's). Only case and regex: whole word is later (PLAN.md), so it stays off.
//! - Typing schedules a search `DEBOUNCE` after the last edit (`edited`); the event loop calls
//!   `take_due` when the deadline passes. Times are passed in, so tests need no clock.
//! - The list scrolls by rows: each file is a row followed by one row per matching line. The top
//!   row is kept as (file, row in file), so files streaming in above it don't move what's shown.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::pos::{self, DisplayCol};
use crate::field::TextField;
use crate::search::{Options, SearchError};
use crate::workspace_search::{FileMatches, LineMatch, Query, Results};

/// How long after the last edit to the query the search restarts (VS Code waits ~300 ms).
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// Cells of text kept before the first match when a result line has to be cut to show it.
const CONTEXT_BEFORE: usize = 12;

#[derive(Debug, Default)]
pub struct SearchView {
    pub query: TextField,
    /// Case and regex (whole word stays off in v1).
    pub options: Options,
    /// The query is an invalid regex: shown under the input, nothing runs.
    pub error: Option<SearchError>,
    /// When to restart the search after the last edit.
    deadline: Option<Instant>,
    /// The list's top row: a file (by path) and the row within it (0 = the file's own row);
    /// `None` is the top of the list.
    top: Option<(PathBuf, usize)>,
    /// Rows the list showed in the last frame (set by drawing).
    pub height: usize,
}

/// One row of the results list.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    File(&'a FileMatches),
    Line(&'a LineMatch),
}

impl SearchView {
    /// The query as the search engine takes it.
    pub fn query(&self) -> Query {
        Query {
            pattern: self.query.text.clone(),
            options: self.options,
        }
    }

    /// The query changed at `now`: restart the search `DEBOUNCE` later (later edits push it back).
    pub fn edited(&mut self, now: Instant) {
        self.deadline = Some(now + DEBOUNCE);
    }

    /// When the pending restart is due, if one is.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// True (once) when a restart is pending and due at `now`.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(d) if d <= now => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }

    /// The search is starting now: nothing pending, the list back at its top.
    pub fn started(&mut self) {
        self.deadline = None;
        self.top = None;
    }

    /// The row index at the top of the list.
    pub fn scroll(&self, results: &Results) -> usize {
        let Some((path, offset)) = &self.top else { return 0 };
        let i = results.files.partition_point(|f| f.path < *path);
        let before: usize = results.files[..i].iter().map(file_rows).sum();
        match results.files.get(i) {
            Some(f) if f.path == *path => before + (*offset).min(f.lines.len()),
            _ => before,
        }
    }

    /// Puts row `row` (clamped so the last row can reach the bottom) at the top.
    pub fn set_scroll(&mut self, results: &Results, row: usize) {
        let row = row.min(row_count(results).saturating_sub(self.height.max(1)));
        self.top = None;
        if row == 0 {
            return;
        }
        let mut skip = row;
        for f in &results.files {
            let n = file_rows(f);
            if skip < n {
                self.top = Some((f.path.clone(), skip));
                return;
            }
            skip -= n;
        }
    }

    pub fn scroll_by(&mut self, results: &Results, delta: isize) {
        let row = self.scroll(results).saturating_add_signed(delta);
        self.set_scroll(results, row);
    }
}

fn file_rows(f: &FileMatches) -> usize {
    1 + f.lines.len()
}

/// Rows in the whole list.
pub fn row_count(results: &Results) -> usize {
    results.files.iter().map(file_rows).sum()
}

/// Up to `n` rows starting at row `from` (only those are looked at past the files skipped).
pub fn rows(results: &Results, from: usize, n: usize) -> Vec<Row<'_>> {
    let mut out = Vec::with_capacity(n);
    let mut skip = from;
    for f in &results.files {
        if out.len() >= n {
            break;
        }
        let rows = file_rows(f);
        if skip >= rows {
            skip -= rows;
            continue;
        }
        if skip == 0 {
            out.push(Row::File(f));
        }
        let first = skip.saturating_sub(1);
        skip = 0;
        out.extend(f.lines[first..].iter().take(n - out.len()).map(Row::Line));
    }
    out
}

/// What a result line shows: its text from `start` (leading whitespace dropped, or from a little
/// before the first match when that wouldn't fit in the width), whether text was cut off before
/// it (drawn as "…"), and the matches as offsets into the shown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview<'a> {
    pub text: &'a str,
    pub cut: bool,
    pub ranges: Vec<Range<usize>>,
}

/// Cells a piece of a result line takes (tabs as one cell; drawn the same way).
pub fn cells(text: &str) -> usize {
    text.graphemes(true)
        .map(|g| pos::display_grapheme(g, DisplayCol(0), 1).1)
        .sum()
}

/// Lays out `m` for `width` cells: the first match is kept in view.
pub fn preview(m: &LineMatch, width: usize) -> Preview<'_> {
    let text = m.text.as_str();
    let first = m.text_ranges().next();
    let indent = text.len() - text.trim_start().len();
    let mut start = first.as_ref().map_or(indent, |r| indent.min(r.start));
    let mut cut = m.text_start > 0;
    if let Some(r) = &first {
        // Room for the text up to the end of the first match (one cell kept for a leading "…").
        let room = width.saturating_sub(1);
        if cells(&text[start..r.end]) > room {
            let keep = CONTEXT_BEFORE.min(room / 3);
            let mut s = r.start;
            for (i, _) in text[start..r.start].grapheme_indices(true).rev() {
                if cells(&text[start + i..r.start]) > keep {
                    break;
                }
                s = start + i;
            }
            start = s;
            cut = true;
        }
    }
    let ranges = m
        .text_ranges()
        .filter(|r| r.end > start || (r.start == r.end && r.start >= start))
        .map(|r| r.start.max(start) - start..r.end - start)
        .collect();
    Preview {
        text: &text[start..],
        cut,
        ranges,
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn line(text: &str, ranges: Vec<Range<usize>>) -> LineMatch {
        LineMatch {
            line: 0,
            ranges,
            text: text.into(),
            text_start: 0,
        }
    }

    fn results(files: &[(&str, usize)]) -> Results {
        let files = files
            .iter()
            .map(|(p, n)| FileMatches {
                path: PathBuf::from(p),
                lines: (0..*n)
                    .map(|i| LineMatch {
                        line: i,
                        ..line("x", vec![0..1])
                    })
                    .collect(),
            })
            .collect();
        Results {
            files,
            ..Results::default()
        }
    }

    #[test]
    fn debounce_restarts_once_after_the_last_edit() {
        let t = Instant::now();
        let mut v = SearchView::default();
        assert!(!v.take_due(t));
        v.edited(t);
        v.edited(t + Duration::from_millis(100));
        assert!(
            !v.take_due(t + Duration::from_millis(200)),
            "pushed back by the second edit"
        );
        assert_eq!(v.deadline(), Some(t + Duration::from_millis(100) + DEBOUNCE));
        assert!(v.take_due(t + Duration::from_millis(250)));
        assert!(!v.take_due(t + Duration::from_millis(300)), "only once");
        v.edited(t);
        v.started();
        assert_eq!(v.deadline(), None, "starting a search clears it");
    }

    #[test]
    fn rows_and_a_stable_scroll() {
        let mut r = results(&[("b", 2), ("d", 3)]);
        assert_eq!(row_count(&r), 7);
        let shown: Vec<String> = rows(&r, 2, 3)
            .iter()
            .map(|row| match row {
                Row::File(f) => f.path.display().to_string(),
                Row::Line(l) => l.line.to_string(),
            })
            .collect();
        assert_eq!(shown, ["1", "d", "0"]);
        assert_eq!(rows(&r, 6, 10).len(), 1);
        assert!(rows(&r, 7, 10).is_empty());

        let mut v = SearchView {
            height: 2,
            ..SearchView::default()
        };
        v.set_scroll(&r, 4);
        assert_eq!(v.scroll(&r), 4, "row 1 of d");
        // A file arriving above keeps the same row on top.
        r.files.insert(0, results(&[("a", 4)]).files.remove(0));
        assert_eq!(v.scroll(&r), 9);
        assert!(matches!(rows(&r, 9, 1)[0], Row::Line(l) if l.line == 0));
        v.scroll_by(&r, -100);
        assert_eq!(v.scroll(&r), 0);
        v.scroll_by(&r, 100);
        assert_eq!(v.scroll(&r), row_count(&r) - 2, "the last row reaches the bottom");
        v.started();
        assert_eq!(v.scroll(&r), 0);
    }

    #[test]
    fn previews_keep_the_first_match_in_view() {
        // Leading whitespace goes; ranges follow.
        let l = line("    let x = 1;", vec![8..9]);
        let p = preview(&l, 40);
        assert_eq!((p.text, p.cut, p.ranges.clone()), ("let x = 1;", false, vec![4..5]));

        // Too far right to fit: cut to a little context before the match.
        let long = format!("{}needle tail", "a".repeat(100));
        let l = line(&long, vec![100..106]);
        let p = preview(&l, 30);
        assert!(p.cut);
        assert_eq!(p.text, format!("{}needle tail", "a".repeat(9)));
        assert_eq!(p.ranges, vec![9..15]);
        assert!(cells(&p.text[..p.ranges[0].end]) < 30);

        // A line 7a already cut shows the ellipsis even when it fits.
        let l = LineMatch {
            text_start: 50,
            ..line("abc", vec![50..51])
        };
        assert!(preview(&l, 40).cut);

        // A match inside the indent keeps it.
        let l = line("  x", vec![0..1]);
        assert_eq!(preview(&l, 40).text, "  x");

        // Wide and multibyte text before the match: cut on a grapheme boundary, measured in
        // cells, the match still fits (with the leading "…").
        let long = format!("{}e\u{301}needle", "日本".repeat(30));
        let at = long.find("needle").unwrap();
        let l = line(&long, vec![at..at + 6]);
        for width in [8, 13, 20, 30] {
            let p = preview(&l, width);
            assert!(p.cut, "{width}");
            assert!(p.text.ends_with("needle"));
            assert_eq!(&p.text[p.ranges[0].clone()], "needle");
            assert!(cells(&p.text[..p.ranges[0].end]) < width, "{width}: {:?}", p.text);
            assert!(!p.text.starts_with('\u{301}'), "not inside a grapheme");
        }
        assert_eq!(
            preview(&l, 30).text,
            "日本日本e\u{301}needle",
            "12 cells of context at most"
        );
    }
}
