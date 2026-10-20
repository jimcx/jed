//! The Search view's model (no drawing; `ui::search` draws it): the query input and its
//! options, the debounce that restarts the search as the query changes, the error of an invalid
//! regex, and the results list's scroll position. The results themselves are `App::search`
//! (`workspace_search::Results`).
//!
//! - The options (case, whole word, regex) are the view's own (VS Code keeps the Search view's
//!   apart from the find widget's); each means what it means in find (`search::Matcher`).
//! - Typing schedules a search `DEBOUNCE` after the last edit (`edited`); the event loop calls
//!   `take_due` when the deadline passes. Times are passed in, so tests need no clock.
//! - The list scrolls by rows: each file is a row followed by one row per matching line. The top
//!   row is kept as a `RowKey` (file, row in file), so files streaming in above it don't move
//!   what's shown.
//! - The list has a selected row (also a `RowKey`, so streaming doesn't move it either) and,
//!   for F4 / Shift+F4, which match on that line: `step` goes to the next / previous match
//!   (wrapping), `target` is the match Enter opens. Up/Down move it (`move_selection`), keeping it
//!   on screen; `list_focused` says whether the list or an input has the keys.
//! - Search details (VS Code's "Toggle Search Details", the `…` button): the "files to include"
//!   and "files to exclude" inputs under the query (`path_filter` reads them), shown while
//!   `details`. Hidden, they still apply (as VS Code). `input` is the input that has the keys
//!   while the list doesn't; Tab / Shift+Tab go query → include → exclude → list → query
//!   (`cycle`: hidden inputs and an empty list left out). Edits to them are debounced like the
//!   query's.
//! - A file row collapses / expands (`set_collapsed`, `collapse_all`, `collapse_key`: the flag
//!   lives on the results' `FileMatches`, so a new search shows everything again); a collapsed
//!   file is a single row, and a selection or top row inside it moves to that row. F4 /
//!   Shift+F4 step into a collapsed file and expand it (VS Code's `selectNextMatch`).
//!   `dismiss` takes a line or a file out of the results (`Results::dismiss_line` /
//!   `dismiss_file`); a selection on it moves to the row that takes its place.
//! - `hover` is the results row under the pointer (its `×` dismiss button is drawn).
//! - Replace (VS Code's `>` toggle left of the query, Cmd+Shift+H): a second input under the
//!   query, shown while `replacing` (`toggle_replace`), between the query and the include input
//!   in the Tab order. Its text is a `search::Replacement` template parsed for the search's mode
//!   (`replacement`); while it's non-empty (`previewing`) each result line draws its matches
//!   struck through followed by their replacements, and opening a result opens the file's
//!   replace preview diff tab instead (`workspace_search::FileReplace`). Editing it re-searches
//!   nothing.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::pos::{self, DisplayCol};
use crate::field::TextField;
use crate::grouped::{self, Group, ListState};
use crate::search::{Matcher, Options, Replacement, SearchError};
use crate::workspace_search::{FileMatches, LineMatch, Query, Results};

pub use crate::grouped::RowKey;

/// How long after the last edit to the query the search restarts (VS Code waits ~300 ms).
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// Cells of text kept before the first match when a result line has to be cut to show it.
const CONTEXT_BEFORE: usize = 12;

/// One of the view's inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Input {
    #[default]
    Query,
    Replace,
    Include,
    Exclude,
}

#[derive(Debug, Default)]
pub struct SearchView {
    pub query: TextField,
    /// The replace input's text (a `search::Replacement` template).
    pub replace: TextField,
    /// The replace input is shown.
    pub replacing: bool,
    /// Files to include / exclude: comma-separated globs (`path_filter`).
    pub include: TextField,
    pub exclude: TextField,
    /// The include / exclude inputs are shown (they apply either way).
    pub details: bool,
    /// The input that has the keys while the list doesn't.
    pub input: Input,
    /// Case, whole word and regex.
    pub options: Options,
    /// The query is an invalid regex: shown under the input, nothing runs.
    pub error: Option<SearchError>,
    /// When to restart the search after the last edit.
    deadline: Option<Instant>,
    /// The results list's scroll, selected row and height (rows by `RowKey`).
    pub list: ListState,
    /// Which match on the selected line F4 is on (0 for a file row).
    selected_match: usize,
    /// The results list (not the query input) has the keys (context `searchResults`).
    pub list_focused: bool,
    /// The results row under the pointer (its dismiss button shows).
    pub hover: Option<RowKey>,
}

impl Group for FileMatches {
    fn path(&self) -> &Path {
        &self.path
    }
    fn items(&self) -> usize {
        if self.collapsed { 0 } else { self.lines.len() }
    }
}

/// A match to open: its file (relative to the root), 0-based line and byte offsets in the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: usize,
    pub range: Range<usize>,
}

/// One row of the results list.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    File(&'a FileMatches),
    Line(&'a LineMatch),
}

/// A row on screen: its file and the row within it (0 = the file's own row).
#[derive(Debug, Clone, Copy)]
pub struct Shown<'a> {
    pub file: &'a FileMatches,
    pub offset: usize,
}

impl<'a> Shown<'a> {
    pub fn row(&self) -> Row<'a> {
        match self.offset {
            0 => Row::File(self.file),
            k => Row::Line(&self.file.lines[k - 1]),
        }
    }

    pub fn key(&self) -> RowKey {
        (self.file.path.clone(), self.offset)
    }
}

impl SearchView {
    /// The query as the search engine takes it.
    pub fn query(&self) -> Query {
        Query {
            pattern: self.query.text.clone(),
            options: self.options,
            include: self.include.text.clone(),
            exclude: self.exclude.text.clone(),
        }
    }

    /// The input that has the keys while the list doesn't.
    pub fn input_mut(&mut self) -> &mut TextField {
        match self.input {
            Input::Query => &mut self.query,
            Input::Replace => &mut self.replace,
            Input::Include => &mut self.include,
            Input::Exclude => &mut self.exclude,
        }
    }

    /// Whether the hidden include / exclude inputs hold a filter (it still applies).
    pub fn hidden_filter(&self) -> bool {
        !self.details && !(self.include.text.trim().is_empty() && self.exclude.text.trim().is_empty())
    }

    /// Shows the include / exclude inputs, giving the include input the keys, or hides them,
    /// giving the query the keys if one of them had them.
    pub fn toggle_details(&mut self) {
        self.details = !self.details;
        if self.details {
            self.input = Input::Include;
            self.list_focused = false;
        } else if self.input != Input::Query {
            self.input = Input::Query;
        }
    }

    /// Shows the replace input, giving it the keys, or hides it, giving the query the keys if it
    /// had them.
    pub fn toggle_replace(&mut self) {
        self.replacing = !self.replacing;
        if self.replacing {
            self.input = Input::Replace;
            self.list_focused = false;
        } else if self.input == Input::Replace {
            self.input = Input::Query;
        }
    }

    /// The replace input is shown and non-empty: result lines preview their replacements and
    /// opening one opens the replace preview.
    pub fn previewing(&self) -> bool {
        self.replacing && !self.replace.text.is_empty()
    }

    /// The replace text as a template for `results`' search (its regex mode), while previewing.
    pub fn replacement(&self, results: &Results) -> Option<Replacement> {
        self.previewing()
            .then(|| Replacement::new(&self.replace.text, results.query.options.regex))
    }

    /// Tab (`forward`) / Shift+Tab: the next / previous of query, replace, include, exclude (while shown)
    /// and the results list (while it has rows: it selects its first row when none is), wrapping.
    pub fn cycle(&mut self, results: Option<&Results>, forward: bool) {
        let list = results.filter(|r| !r.files.is_empty());
        let mut stops = vec![Some(Input::Query)];
        if self.replacing {
            stops.push(Some(Input::Replace));
        }
        if self.details {
            stops.extend([Some(Input::Include), Some(Input::Exclude)]);
        }
        if list.is_some() {
            stops.push(None);
        }
        let here = if self.list_focused { None } else { Some(self.input) };
        let i = stops.iter().position(|s| *s == here).unwrap_or(0);
        let n = stops.len();
        let next = stops[if forward { (i + 1) % n } else { (i + n - 1) % n }];
        match (next, list) {
            (Some(input), _) => {
                self.input = input;
                self.list_focused = false;
            }
            (None, Some(results)) => {
                self.list_focused = true;
                if self.selected_row(results).is_none() {
                    self.select(results, 0);
                }
            }
            (None, None) => {}
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

    /// The search is starting now: nothing pending, the list back at its top, nothing selected.
    pub fn started(&mut self) {
        self.deadline = None;
        self.list.reset();
        self.selected_match = 0;
    }

    /// The row index at the top of the list.
    pub fn scroll(&self, results: &Results) -> usize {
        self.list.scroll(&results.files)
    }

    /// Puts row `row` (clamped so the last row can reach the bottom) at the top.
    pub fn set_scroll(&mut self, results: &Results, row: usize) {
        self.list.set_scroll(&results.files, row);
    }

    pub fn scroll_by(&mut self, results: &Results, delta: isize) {
        self.list.scroll_by(&results.files, delta);
    }

    /// The selected row's index, if a row is selected and its file is still in the results.
    pub fn selected_row(&self, results: &Results) -> Option<usize> {
        self.list.selected_row(&results.files)
    }

    /// Selects row `row` (clamped to the list; nothing when it's empty) and scrolls to it.
    pub fn select(&mut self, results: &Results, row: usize) {
        self.list.select(&results.files, row);
        self.selected_match = 0;
    }

    /// Selects the row `key` names, if it's in the results.
    pub fn select_key(&mut self, results: &Results, key: &RowKey) {
        if self.list.select_key(&results.files, key) {
            self.selected_match = 0;
        }
    }

    /// Moves the selection by `delta` rows (clamped), or selects the first row when nothing is
    /// and `delta` is positive. False when moving up from the first row (or with nothing
    /// selected): the caller gives the keys back to the query input.
    pub fn move_selection(&mut self, results: &Results, delta: isize) -> bool {
        let moved = self.list.move_selection(&results.files, delta);
        if moved {
            self.selected_match = 0;
        }
        moved
    }

    /// The match Enter opens: the selected line's (its match F4 is on), or a file row's first
    /// (which becomes the selection, its file expanded).
    pub fn target(&mut self, results: &mut Results) -> Option<Target> {
        let (path, offset) = self.list.selected()?.clone();
        let i = results.file_index(&path)?;
        let line = offset.max(1).min(results.files[i].lines.len()) - 1;
        Some(self.go(results, i, line, self.selected_match))
    }

    /// F4 (`forward`) / Shift+F4: the match after / before the selected one, wrapping around the
    /// list, which becomes the selection (a collapsed file stepped into is expanded). From a file
    /// row, forward is the file's first match and back is the previous file's last; with nothing
    /// selected, the first or the last match.
    pub fn step(&mut self, results: &mut Results, forward: bool) -> Option<Target> {
        let files = &results.files;
        let n = files.len();
        if n == 0 {
            return None;
        }
        // (file, line, match) indexes.
        let first = |i: usize| (i % n, 0, 0);
        let last = |i: usize| {
            let f = (i + n - 1) % n;
            let line = files[f].lines.len() - 1;
            (f, line, files[f].lines[line].ranges.len() - 1)
        };
        let current = self.list.selected().and_then(|(path, offset)| {
            let (i, _) = grouped::group(files, path)?;
            Some((i, *offset, self.selected_match))
        });
        let (f, line, m) = match current {
            None if forward => first(0),
            None => last(0),
            Some((i, 0, _)) if forward => first(i),
            Some((i, 0, _)) => last(i),
            Some((i, offset, m)) => {
                let lines = &files[i].lines;
                let line = offset.min(lines.len()) - 1;
                let count = lines[line].ranges.len();
                let m = m.min(count - 1);
                if forward {
                    if m + 1 < count {
                        (i, line, m + 1)
                    } else if line + 1 < lines.len() {
                        (i, line + 1, 0)
                    } else {
                        first(i + 1)
                    }
                } else if m > 0 {
                    (i, line, m - 1)
                } else if line > 0 {
                    (i, line - 1, lines[line - 1].ranges.len() - 1)
                } else {
                    last(i)
                }
            }
        };
        Some(self.go(results, f, line, m))
    }

    /// Selects match `m` of line `line` of file `f` (expanded, scrolled into view) and says
    /// where it is.
    fn go(&mut self, results: &mut Results, f: usize, line: usize, m: usize) -> Target {
        let file = &mut results.files[f];
        file.collapsed = false;
        let key = (file.path.clone(), line + 1);
        let l = &file.lines[line];
        let m = m.min(l.ranges.len() - 1);
        let target = Target {
            path: file.path.clone(),
            line: l.line,
            range: l.ranges[m].clone(),
        };
        self.list.select_key(&results.files, &key);
        self.selected_match = m;
        target
    }

    /// Collapses (`collapse`) or expands the file at `path`; collapsing moves a selection or top
    /// row inside it to its file row.
    pub fn set_collapsed(&mut self, results: &mut Results, path: &Path, collapse: bool) {
        let Some(i) = results.file_index(path) else { return };
        results.files[i].collapsed = collapse;
        if collapse && self.list.fold(Some(path)) {
            self.selected_match = 0;
        }
        self.list.clamp(&results.files);
    }

    /// Collapse All / Expand All (files arriving later too).
    pub fn collapse_all(&mut self, results: &mut Results, collapse: bool) {
        results.collapse_all(collapse);
        if collapse && self.list.fold(None) {
            self.selected_match = 0;
        }
        self.list.clamp(&results.files);
    }

    /// Left (`collapse`) / Right in the list: on a file row, collapses / expands it, or when
    /// it's already so, Right goes to its first line; Left on a line goes to its file's row.
    pub fn collapse_key(&mut self, results: &mut Results, collapse: bool) {
        let Some((path, offset)) = self.list.selected().cloned() else {
            return;
        };
        let Some(i) = results.file_index(&path) else { return };
        let collapsed = results.files[i].collapsed;
        match (offset, collapse) {
            (0, true) if !collapsed => self.set_collapsed(results, &path, true),
            (0, false) if collapsed => self.set_collapsed(results, &path, false),
            (0, false) => self.select_key(results, &(path, 1)),
            (_, true) => self.select_key(results, &(path, 0)),
            _ => {}
        }
    }

    /// Dismisses the row `key` names: a line (every match on it; its file's last line takes the
    /// file too) or a file (a collapsed file's every row is its file). A selection on it moves
    /// to the row that takes its place.
    pub fn dismiss(&mut self, results: &mut Results, key: &RowKey) {
        let Ok(row) = grouped::index_of(&results.files, key) else {
            return;
        };
        let Some(i) = results.file_index(&key.0) else { return };
        // Clamped as `index_of` clamps it: a collapsed file's rows are its own.
        let line = key.1.min(grouped::Group::items(&results.files[i]));
        let file_row = row - line;
        let path = key.0.clone();
        let gone = if line == 0 || results.dismiss_line(&path, line - 1) {
            if line == 0 {
                results.dismiss_file(&path);
            }
            (path, 0)
        } else {
            (path, line)
        };
        let at = if gone.1 == 0 { file_row } else { row };
        if self.list.removed(&results.files, &gone, at) {
            self.selected_match = 0;
        }
    }
}

/// Up to `n` rows starting at row `from` (only those are looked at past the files skipped).
pub fn rows(results: &Results, from: usize, n: usize) -> Vec<Shown<'_>> {
    grouped::rows(&results.files, from, n)
        .into_iter()
        .map(|(i, offset)| Shown {
            file: &results.files[i],
            offset,
        })
        .collect()
}

/// What a result line shows: its text from `start` (leading whitespace dropped, or from a little
/// before the first match when that wouldn't fit in the width), whether text was cut off before
/// it (drawn as "…"), and the matches as offsets into the shown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview<'a> {
    pub text: &'a str,
    /// Where `text` starts in the line's text (`LineMatch::text`).
    pub start: usize,
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
        start,
        cut,
        ranges,
    }
}

/// What replaces each of a preview's matches (`p.ranges` order): `with` expanded against the
/// match found again in the line (`Matcher::replacement_at`). Only for lines drawn.
pub fn replacements(m: &LineMatch, p: &Preview, matcher: &Matcher, with: &Replacement) -> Vec<String> {
    p.ranges
        .iter()
        .map(|r| matcher.replacement_at(m.text.as_bytes(), p.start + r.start..p.start + r.end, with))
        .collect()
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::workspace_search::Batch;

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
                collapsed: false,
            })
            .collect();
        let mut r = Results::default();
        r.add(Batch {
            generation: 0,
            files,
            done: false,
            truncated: false,
        });
        r
    }

    fn with_height(height: usize) -> SearchView {
        let mut v = SearchView::default();
        v.list.height = height;
        v
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

    /// Tab / Shift+Tab visit the shown inputs and a non-empty list, wrapping; toggling the
    /// details gives the include input the keys and, hiding them, the query back.
    #[test]
    fn tab_cycles_through_the_inputs_and_the_list() {
        let r = results(&[("a", 1)]);
        let empty = results(&[]);
        let mut v = with_height(5);
        let at = |v: &SearchView| if v.list_focused { None } else { Some(v.input) };
        v.cycle(Some(&empty), true);
        assert_eq!(at(&v), Some(Input::Query), "only the query: stays");
        v.cycle(Some(&r), true);
        assert_eq!(at(&v), None, "the list");
        assert_eq!(v.selected_row(&r), Some(0), "its first row selected");
        v.cycle(Some(&r), true);
        assert_eq!(at(&v), Some(Input::Query));

        v.toggle_details();
        assert_eq!(at(&v), Some(Input::Include));
        v.cycle(Some(&r), true);
        assert_eq!(at(&v), Some(Input::Exclude));
        v.cycle(None, true);
        assert_eq!(at(&v), Some(Input::Query), "no results: wraps past the list");
        v.cycle(Some(&r), false);
        assert_eq!(at(&v), None);
        v.cycle(Some(&r), false);
        assert_eq!(at(&v), Some(Input::Exclude));
        assert!(!v.hidden_filter());
        v.exclude.insert("target");
        v.toggle_details();
        assert_eq!(at(&v), Some(Input::Query), "hidden: the query gets the keys back");
        assert!(v.hidden_filter(), "still applies");
        assert_eq!(v.query().exclude, "target");
    }

    /// The replace input: shown by its toggle (taking the keys), between the query and the
    /// include input in the Tab order; hidden, the query gets the keys back; previews only while
    /// it has text, parsed for the search's mode.
    #[test]
    fn the_replace_input() {
        let mut r = results(&[("a", 1)]);
        let mut v = with_height(5);
        let at = |v: &SearchView| if v.list_focused { None } else { Some(v.input) };
        v.toggle_replace();
        assert!(v.replacing);
        assert_eq!(at(&v), Some(Input::Replace));
        v.toggle_details();
        assert_eq!(at(&v), Some(Input::Include));
        let mut order = Vec::new();
        for _ in 0..5 {
            v.cycle(Some(&r), true);
            order.push(at(&v));
        }
        assert_eq!(
            order,
            [
                Some(Input::Exclude),
                None,
                Some(Input::Query),
                Some(Input::Replace),
                Some(Input::Include)
            ],
            "from the include input the details took the keys to"
        );
        v.cycle(Some(&r), false);
        assert_eq!(at(&v), Some(Input::Replace));
        v.cycle(Some(&r), false);
        assert_eq!(at(&v), Some(Input::Query));
        assert!(!v.previewing() && v.replacement(&r).is_none(), "empty");
        v.replace.insert("$1");
        assert!(v.previewing());
        assert_eq!(v.replacement(&r).unwrap().literal(), Some("$1"), "plain text");
        r.query.options.regex = true;
        assert_eq!(v.replacement(&r).unwrap().literal(), None, "a group");
        v.input = Input::Replace;
        v.toggle_replace();
        assert_eq!(at(&v), Some(Input::Query), "hidden: the query gets the keys");
        assert!(!v.previewing() && v.replacement(&r).is_none());
        v.cycle(Some(&r), true);
        assert_eq!(at(&v), Some(Input::Include), "skipped while hidden");
    }

    /// Each drawn match's replacement, expanded against its own match on the line.
    #[test]
    fn replacements_per_shown_match() {
        let o = Options {
            regex: true,
            ..Options::default()
        };
        let matcher = Matcher::new_single_line(r"foo(\d)", o).unwrap();
        let with = Replacement::new("bar$1", true);
        let l = line("  foo1 x foo2", vec![2..6, 9..13]);
        let p = preview(&l, 40);
        assert_eq!(p.start, 2);
        assert_eq!(replacements(&l, &p, &matcher, &with), ["bar1", "bar2"]);
        // A long line's piece: offsets into the piece.
        let l = LineMatch {
            text_start: 100,
            ..line("foo7", vec![100..104])
        };
        let p = preview(&l, 40);
        assert_eq!(replacements(&l, &p, &matcher, &with), ["bar7"]);
    }

    #[test]
    fn rows_and_a_stable_scroll() {
        let mut r = results(&[("b", 2), ("d", 3)]);
        assert_eq!(grouped::row_count(&r.files), 7);
        let shown: Vec<String> = rows(&r, 2, 3)
            .iter()
            .map(|s| match s.row() {
                Row::File(f) => f.path.display().to_string(),
                Row::Line(l) => l.line.to_string(),
            })
            .collect();
        assert_eq!(shown, ["1", "d", "0"]);
        assert_eq!(rows(&r, 6, 10).len(), 1);
        assert!(rows(&r, 7, 10).is_empty());

        let mut v = with_height(2);
        v.set_scroll(&r, 4);
        assert_eq!(v.scroll(&r), 4, "row 1 of d");
        // A file arriving above keeps the same row on top.
        r.files.insert(0, results(&[("a", 4)]).files.remove(0));
        assert_eq!(v.scroll(&r), 9);
        assert!(matches!(rows(&r, 9, 1)[0].row(), Row::Line(l) if l.line == 0));
        v.scroll_by(&r, -100);
        assert_eq!(v.scroll(&r), 0);
        v.scroll_by(&r, 100);
        assert_eq!(
            v.scroll(&r),
            grouped::row_count(&r.files) - 2,
            "the last row reaches the bottom"
        );
        v.started();
        assert_eq!(v.scroll(&r), 0);
    }

    /// Up/Down/paging move the selection (clamped, kept on screen); Up from the first row hands
    /// the keys back; files streaming in don't move it; a new search clears it.
    #[test]
    fn the_selection_moves_and_stays_put() {
        let mut r = results(&[("b", 2), ("d", 3)]);
        let mut v = with_height(3);
        assert!(!v.move_selection(&r, -1), "nothing selected: up leaves the list");
        assert!(v.move_selection(&r, 1));
        assert_eq!(v.selected_row(&r), Some(0));
        assert!(!v.move_selection(&r, -1), "up from the first row");
        assert_eq!(v.selected_row(&r), Some(0));
        v.move_selection(&r, 3);
        assert_eq!(v.selected_row(&r), Some(3), "d's own row");
        assert_eq!(v.scroll(&r), 1, "scrolled the least to show it");
        v.move_selection(&r, 100);
        assert_eq!(v.selected_row(&r), Some(6));
        assert_eq!(v.scroll(&r), 4);
        v.move_selection(&r, -6);
        assert_eq!((v.selected_row(&r), v.scroll(&r)), (Some(0), 0));
        v.select(&r, 5);
        // A file arriving above: the same row stays selected (and on top).
        r.files.insert(0, results(&[("a", 4)]).files.remove(0));
        assert_eq!(v.selected_row(&r), Some(10));
        v.started();
        assert_eq!(v.selected_row(&r), None);
        v.select(&r, 1);
        v.select_key(&r, &(PathBuf::from("nope"), 1));
        assert_eq!(v.selected_row(&r), Some(1), "an unknown key changes nothing");
        v.select_key(&r, &(PathBuf::from("d"), 2));
        assert_eq!(v.selected_row(&r), Some(10));
        let empty = Results::default();
        v.select(&empty, 0);
        assert_eq!(v.selected_row(&empty), None);
    }

    /// F4 / Shift+F4 visit every match (several on a line too) in order and wrap; from a file row
    /// forward is its first match; Enter on a file row opens its first match.
    #[test]
    fn step_visits_every_match_and_wraps() {
        let file = |p: &str, lines: Vec<(usize, Vec<Range<usize>>)>| FileMatches {
            path: PathBuf::from(p),
            lines: lines
                .into_iter()
                .map(|(n, ranges)| LineMatch {
                    line: n,
                    ..line("x", ranges)
                })
                .collect(),
            collapsed: false,
        };
        let mut r = Results {
            files: vec![
                file("a", vec![(3, vec![0..1, 4..6])]),
                file("b", vec![(0, vec![2..3]), (7, vec![1..2])]),
            ],
            ..Results::default()
        };
        let t = |path: &str, line: usize, range: Range<usize>| Target {
            path: PathBuf::from(path),
            line,
            range,
        };
        let all = [t("a", 3, 0..1), t("a", 3, 4..6), t("b", 0, 2..3), t("b", 7, 1..2)];
        let mut v = with_height(10);
        assert!(v.target(&mut r).is_none(), "nothing selected");
        for want in all.iter().chain(&all[..1]) {
            assert_eq!(v.step(&mut r, true).as_ref(), Some(want));
        }
        assert_eq!(v.selected_row(&r), Some(1));
        assert_eq!(v.target(&mut r), Some(all[0].clone()), "Enter opens the match F4 is on");
        // Selecting a row (a click) starts again from its first match.
        assert_eq!(v.step(&mut r, true), Some(all[1].clone()));
        v.select_key(&r, &(PathBuf::from("a"), 1));
        assert_eq!(v.target(&mut r), Some(all[0].clone()), "the line's first match again");
        assert_eq!(v.step(&mut r, false), Some(all[3].clone()), "back wraps to the last");
        for want in all.iter().rev().skip(1) {
            assert_eq!(v.step(&mut r, false).as_ref(), Some(want));
        }
        v.started();
        assert_eq!(
            v.step(&mut r, false),
            Some(all[3].clone()),
            "nothing selected: the last"
        );

        // From b's file row: forward is its first match, back the previous file's last.
        v.select(&r, 2);
        assert_eq!(v.step(&mut r, true), Some(all[2].clone()));
        v.select(&r, 2);
        assert_eq!(v.step(&mut r, false), Some(all[1].clone()));
        v.select(&r, 2);
        assert_eq!(v.target(&mut r), Some(all[2].clone()));
        assert_eq!(v.selected_row(&r), Some(3), "the match row is selected");
        // Stepping keeps the selection on screen.
        v.list.height = 1;
        v.step(&mut r, true);
        assert_eq!(v.scroll(&r), 4);
        assert!(v.step(&mut Results::default(), true).is_none());
    }

    /// A collapsed file is one row (keys inside it fold to it); Left / Right collapse, expand
    /// and move between a file and its lines; F4 / Shift+F4 step into a collapsed file and
    /// expand it; Collapse All covers every file.
    #[test]
    fn collapsing_files() {
        let mut r = results(&[("a", 2), ("b", 3), ("c", 1)]);
        let mut v = with_height(20);
        let key = |p: &str, k: usize| (PathBuf::from(p), k);
        v.select_key(&r, &key("b", 2));
        v.set_collapsed(&mut r, Path::new("b"), true);
        assert_eq!(grouped::row_count(&r.files), 3 + 1 + 2);
        assert_eq!(v.list.selected(), Some(&key("b", 0)), "folded to its row");
        assert_eq!(v.selected_row(&r), Some(3));
        v.collapse_key(&mut r, false);
        assert!(!r.files[1].collapsed, "Right expands");
        v.collapse_key(&mut r, false);
        assert_eq!(v.list.selected(), Some(&key("b", 1)), "Right again: its first line");
        v.collapse_key(&mut r, true);
        assert_eq!(v.list.selected(), Some(&key("b", 0)), "Left on a line: its file");
        v.collapse_key(&mut r, true);
        assert!(r.files[1].collapsed, "Left collapses");
        v.collapse_key(&mut r, true);
        assert!(r.files[1].collapsed && v.list.selected() == Some(&key("b", 0)), "stays");

        // F4 from b's row steps into it, expanding it; Shift+F4 from c into a collapsed b
        // lands on its last match.
        let t = v.step(&mut r, true).unwrap();
        assert_eq!((t.path, t.line), (PathBuf::from("b"), 0));
        assert!(!r.files[1].collapsed);
        v.set_collapsed(&mut r, Path::new("b"), true);
        v.select_key(&r, &key("c", 1));
        let t = v.step(&mut r, false).unwrap();
        assert_eq!((t.path, t.line), (PathBuf::from("b"), 2));
        assert_eq!(v.list.selected(), Some(&key("b", 3)));
        // Enter on a collapsed file's row opens its first match, expanding it.
        v.set_collapsed(&mut r, Path::new("b"), true);
        assert_eq!(v.list.selected(), Some(&key("b", 0)));
        assert_eq!(v.target(&mut r).map(|t| t.line), Some(0));
        assert!(!r.files[1].collapsed);

        v.collapse_all(&mut r, true);
        assert_eq!(grouped::row_count(&r.files), 3);
        assert_eq!(v.list.selected(), Some(&key("b", 0)));
        v.collapse_all(&mut r, false);
        assert_eq!(grouped::row_count(&r.files), 9);
    }

    /// Dismissing a line or a file: counts drop, a selection on it moves to the row in its
    /// place, one below it keeps what it showed, F4 never visits what's gone.
    #[test]
    fn dismissing_rows() {
        let mut r = results(&[("a", 3), ("b", 1), ("c", 2)]);
        let mut v = with_height(20);
        let key = |p: &str, k: usize| (PathBuf::from(p), k);
        v.select_key(&r, &key("a", 3));
        v.dismiss(&mut r, &key("a", 1));
        assert_eq!(r.total, 5);
        assert_eq!(
            v.list.selected(),
            Some(&key("a", 2)),
            "still on line 2 (now a's second)"
        );
        assert_eq!(r.files[0].lines[1].line, 2);
        v.dismiss(&mut r, &key("a", 2));
        assert_eq!(v.list.selected(), Some(&key("b", 0)), "the row in its place");
        // b's only line: b goes, the selection on its row moves to c's.
        v.select_key(&r, &key("b", 1));
        v.dismiss(&mut r, &key("b", 1));
        assert_eq!(r.files.len(), 2);
        assert_eq!(v.list.selected(), Some(&key("c", 0)));
        // A collapsed file's row dismisses the file.
        v.set_collapsed(&mut r, Path::new("a"), true);
        let t = v.step(&mut r, false).unwrap();
        assert_eq!(t.path, PathBuf::from("a"), "back from c's row: a's last match");
        v.set_collapsed(&mut r, Path::new("a"), true);
        v.dismiss(&mut r, &key("a", 5));
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.total, 2);
        assert_eq!(v.selected_row(&r), Some(0), "c's row now first");
        // F4 cycles c's two lines only.
        let lines: Vec<usize> = (0..3).map(|_| v.step(&mut r, true).unwrap().line).collect();
        assert_eq!(lines, [0, 1, 0]);
        v.dismiss(&mut r, &key("c", 0));
        assert!(r.files.is_empty() && v.list.selected().is_none());
        assert!(v.step(&mut r, true).is_none());
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
