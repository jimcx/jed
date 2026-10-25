//! Soft wrap: where one line's text breaks into screen rows at the editor's width.
//!
//! No drawing and no app state: [`Wrap`] is a per-view cache that the editor, scrolling and
//! movement read (soft wrap's later steps). A line wraps after the last space that fits a row,
//! else between graphemes; a wide char or a tab is never split (a grapheme wider than the row
//! gets a row to itself). Continuation rows start at the line's indentation width (VS Code's
//! `wrappingIndent: same`); a space that doesn't fit stays at the end of the row before it (the
//! next row starts at the word), so a continuation row never begins with a break space.
//!
//! [`Wrap`] keeps each line's row count for the scroll bar: laid out for every line once per
//! (revision, width, tab width), then [`Wrap::map`]ped through edits so only the edited lines
//! are laid out again. [`Wrap::layout`] lays out one line (break points, byte ↔ row and column)
//! and caches it by (buffer revision, width, tab width), so only lines that are drawn pay for
//! their break points. Offsets in a [`Layout`] are within the line's content (no line break):
//! add the line's start for a buffer position.

// The model is read by drawing (`ui::editor`) and, in later steps, scrolling and movement
// (soft_wrap-d, -e) and the mouse (soft_wrap-f).
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};

/// Lines whose break points are kept between frames: a screen draws fewer, and clearing is
/// cheap, so an unbounded map isn't worth an LRU.
const MAX_LAYOUTS: usize = 512;

/// One screen row of a wrapped line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The row's first byte, within the line's content.
    pub start: ByteIdx,
    /// The display column the row's first byte is drawn at: 0 on the first row, the line's
    /// indentation width on continuation rows.
    pub col: DisplayCol,
}

/// One line's rows at a width, with the text they were laid out from.
#[derive(Debug, Clone)]
pub struct Layout {
    text: String,
    rows: Vec<Row>,
    tab_width: usize,
}

impl Layout {
    fn compute(text: &str, width: usize, tab_width: usize) -> Self {
        let width = width.max(1);
        let continuation = continuation_col(text, width, tab_width);
        let mut rows = Vec::new();
        let mut start = 0;
        let mut col = 0;
        loop {
            rows.push(Row {
                start: ByteIdx(start),
                col: DisplayCol(col),
            });
            let (end, _) = row_end(text, start, col, width, tab_width);
            if end >= text.len() {
                break;
            }
            start = end;
            col = continuation;
        }
        Self {
            text: text.to_string(),
            rows,
            tab_width,
        }
    }

    /// The line's content (no line break), as the rows were laid out from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The rows, top first (never empty: an empty line has one empty row).
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The byte just past `row`'s last grapheme: the next row's start, or the line's end.
    pub fn row_end(&self, row: usize) -> ByteIdx {
        self.rows
            .get(row + 1)
            .map_or(ByteIdx(self.text.len()), |next| next.start)
    }

    /// The row `pos` (a byte offset within the line) is on, and its display column from that
    /// row's start. A byte at a continuation row's first position is on that row; the line's
    /// end is on the last row.
    pub fn row_of(&self, pos: ByteIdx) -> (usize, DisplayCol) {
        let pos = pos.0.min(self.text.len());
        let row = self.rows.partition_point(|r| r.start.0 <= pos).saturating_sub(1);
        let (start, row_col) = (self.rows[row].start.0, self.rows[row].col);
        let mut at = start;
        let mut col = DisplayCol(0);
        while at < pos {
            let next = pos::next_grapheme_in(&self.text, at);
            if next > pos {
                break;
            }
            col.0 += self.width_of(&self.text[at..next], row_col.0 + col.0);
            at = next;
        }
        (row, col)
    }

    /// The nearest byte boundary on `row` at or before display column `target`: the start of
    /// the grapheme holding it, or the row's end past the last one.
    pub fn byte_at(&self, row: usize, target: DisplayCol) -> ByteIdx {
        let row = row.min(self.rows.len() - 1);
        let at = self.rows[row];
        let end = self.row_end(row).0;
        let mut i = at.start.0;
        let mut col = DisplayCol(0);
        while i < end {
            let next = pos::next_grapheme_in(&self.text, i);
            let w = self.width_of(&self.text[i..next], at.col.0 + col.0);
            if col.0 + w > target.0 {
                return ByteIdx(i);
            }
            col.0 += w;
            i = next;
        }
        ByteIdx(i)
    }

    /// How wide `grapheme` is drawn at display column `at` (tabs depend on the column).
    fn width_of(&self, grapheme: &str, at: usize) -> usize {
        pos::display_grapheme(grapheme, DisplayCol(at), self.tab_width).1
    }
}

/// Where the row starting at byte `start` (display column `col`) ends: its content end and the
/// display column after its last grapheme.
fn row_end(text: &str, start: usize, col: usize, width: usize, tab_width: usize) -> (usize, usize) {
    let mut i = start;
    let mut at = col;
    // The end of the last space that fits, where a row can break after a word.
    let mut space: Option<(usize, usize)> = None;
    while i < text.len() {
        let next = pos::next_grapheme_in(text, i);
        let grapheme = &text[i..next];
        let w = pos::display_grapheme(grapheme, DisplayCol(at), tab_width).1;
        if at + w > width {
            if i == start {
                // A grapheme wider than the whole row (a wide char or tab) gets it alone.
                return (next, at + w);
            }
            if grapheme == " " {
                // Break after the space, even though it doesn't fit: it stays at the end of
                // this row, so the next row starts at the word.
                return (next, at + w);
            }
            return space.unwrap_or((i, at));
        }
        at += w;
        i = next;
        if grapheme == " " {
            space = Some((i, at));
        }
    }
    (i, at)
}

/// The display width of the line's leading spaces and tabs: where its continuation rows start.
fn indentation(text: &str, tab_width: usize) -> usize {
    let mut col = 0;
    let mut i = 0;
    while i < text.len() {
        let next = pos::next_grapheme_in(text, i);
        let grapheme = &text[i..next];
        if grapheme != " " && grapheme != "\t" {
            break;
        }
        col += pos::display_grapheme(grapheme, DisplayCol(col), tab_width).1;
        i = next;
    }
    col
}

/// The column continuation rows start at: the line's indentation, or none when the indentation
/// doesn't leave a column to draw text in.
fn continuation_col(text: &str, width: usize, tab_width: usize) -> usize {
    let indent = indentation(text, tab_width);
    if indent < width { indent } else { 0 }
}

/// How many rows `text` wraps to at `width`.
fn row_count(text: &str, width: usize, tab_width: usize) -> usize {
    let width = width.max(1);
    let continuation = continuation_col(text, width, tab_width);
    let mut rows = 0;
    let mut start = 0;
    let mut col = 0;
    loop {
        rows += 1;
        let (end, _) = row_end(text, start, col, width, tab_width);
        if end >= text.len() {
            break;
        }
        start = end;
        col = continuation;
    }
    rows
}

/// One line's row count at a width, and the revision / width / tab width they were computed for.
#[derive(Debug, Clone)]
struct Counts {
    revision: u64,
    width: usize,
    tab_width: usize,
    /// Rows each line wraps to, by 0-based line.
    rows: Vec<usize>,
    /// `rows`' sum: the scroll bar's total.
    total: usize,
}

/// A view's wrapped-row cache: the text's total row count and each line's row count (kept
/// current through edits), plus the break points of the lines drawn.
#[derive(Debug, Default, Clone)]
pub struct Wrap {
    counts: Option<Counts>,
    layouts: HashMap<usize, Layout>,
    /// The (revision, width, tab width) `layouts` are for.
    layouts_key: Option<(u64, usize, usize)>,
    /// How many times every line was laid out (tests: `map` must not).
    #[cfg(test)]
    sweeps: usize,
}

impl Wrap {
    /// How many screen rows the whole text takes at `width`.
    pub fn total_rows(&mut self, rope: &Rope, revision: u64, width: usize, tab_width: usize) -> usize {
        self.counts(rope, revision, width, tab_width).total
    }

    /// How many rows `line` wraps to at `width` (0-based line index).
    pub fn line_rows(&mut self, rope: &Rope, revision: u64, line: usize, width: usize, tab_width: usize) -> usize {
        let counts = self.counts(rope, revision, width, tab_width);
        counts.rows.get(line).copied().unwrap_or(1)
    }

    /// The global screen row `line`'s first row is at: the rows of every line before it. Walks
    /// the counts, so O(lines) — for the scroll bar's marks and a drag.
    pub fn row_of_line(&mut self, rope: &Rope, revision: u64, line: usize, width: usize, tab_width: usize) -> usize {
        let counts = self.counts(rope, revision, width, tab_width);
        let line = line.min(counts.rows.len());
        counts.rows[..line].iter().sum()
    }

    /// The (line, sub-row) of global screen row `row`, clamped to the text's last row. Walks the
    /// counts, so O(lines) — for a scroll bar drag.
    pub fn line_at(
        &mut self,
        rope: &Rope,
        revision: u64,
        row: usize,
        width: usize,
        tab_width: usize,
    ) -> (usize, usize) {
        let counts = self.counts(rope, revision, width, tab_width);
        let mut row = row;
        for (line, &rows) in counts.rows.iter().enumerate() {
            if row < rows {
                return (line, row);
            }
            row -= rows;
        }
        let last = counts.rows.len().saturating_sub(1);
        (last, counts.rows.get(last).copied().unwrap_or(1).saturating_sub(1))
    }

    /// `total_rows`' count when the counts are already for this key (drawing primes them), for
    /// callers that can't lay the text out (`TextScroll::scroll` takes `&self`).
    pub fn cached_total(&self, revision: u64, width: usize, tab_width: usize) -> Option<usize> {
        self.counts_for(revision, width, tab_width).map(|c| c.total)
    }

    /// `row_of_line`'s row when the counts are already for this key (`cached_total`).
    pub fn cached_row_of_line(&self, revision: u64, line: usize, width: usize, tab_width: usize) -> Option<usize> {
        let counts = self.counts_for(revision, width, tab_width)?;
        Some(counts.rows[..line.min(counts.rows.len())].iter().sum())
    }

    /// `line`'s break points at `width`, laid out on first use and kept while the buffer
    /// revision, width and tab width stay.
    pub fn layout(&mut self, rope: &Rope, revision: u64, line: usize, width: usize, tab_width: usize) -> &Layout {
        let width = width.max(1);
        let key = (revision, width, tab_width);
        if self.layouts_key != Some(key) {
            self.layouts.clear();
            self.layouts_key = Some(key);
        }
        if !self.layouts.contains_key(&line) {
            if self.layouts.len() >= MAX_LAYOUTS {
                self.layouts.clear();
            }
            let text = pos::line_text(rope, line);
            self.layouts.insert(line, Layout::compute(&text, width, tab_width));
        }
        &self.layouts[&line]
    }

    /// Follows change sets applied to the buffer (with `revision` the buffer's revision after
    /// them), as `View::map_through` does: the row counts of the edited lines are laid out
    /// again and the total adjusted, instead of rescanning the whole text. A change in the
    /// number of lines drops the counts; the next use lays everything out again.
    pub fn map(&mut self, changes: &[Arc<ChangeSet>], rope: &Rope, revision: u64) {
        let Some(counts) = &mut self.counts else {
            return;
        };
        if counts.rows.len() != pos::line_count(rope) {
            self.counts = None;
            return;
        }
        // A line break inserted while the line count stays means one was also removed somewhere
        // else (net zero), so lines between the two changes shifted index: `counts.rows` is
        // keyed by line index and the shifted lines aren't all re-laid out, so drop and lay out
        // again on the next use.
        if changes.iter().any(|set| set.inserts_line_break()) {
            self.counts = None;
            return;
        }
        let mappers: Vec<_> = changes.iter().map(|set| set.mapper()).collect();
        let len = rope.len();
        for (k, set) in changes.iter().enumerate() {
            for (start, end, _) in set.iter() {
                let map = |pos: ByteIdx, assoc| mappers[k..].iter().fold(pos, |pos, mapper| mapper.map(pos, assoc));
                let from = map(start, Assoc::Before).0.min(len);
                let to = map(end, Assoc::After).0.min(len);
                let first = pos::line_of(rope, ByteIdx(from));
                let last = pos::line_of(rope, ByteIdx(to));
                for line in first..=last {
                    let rows = row_count(&pos::line_text(rope, line), counts.width, counts.tab_width);
                    counts.total = counts.total + rows - counts.rows[line];
                    counts.rows[line] = rows;
                }
            }
        }
        counts.revision = revision;
    }

    /// How many times every line was laid out from scratch (tests: `map` must keep it at one
    /// across edits, `map_through` too).
    #[cfg(test)]
    pub fn sweeps(&self) -> usize {
        self.sweeps
    }

    /// The counts when they are for this key (nothing to lay out).
    fn counts_for(&self, revision: u64, width: usize, tab_width: usize) -> Option<&Counts> {
        let width = width.max(1);
        self.counts
            .as_ref()
            .filter(|c| (c.revision, c.width, c.tab_width) == (revision, width, tab_width))
    }

    /// The counts, laid out for the whole text when they aren't for this key.
    fn counts(&mut self, rope: &Rope, revision: u64, width: usize, tab_width: usize) -> &Counts {
        let width = width.max(1);
        let key = (revision, width, tab_width);
        let stale = self
            .counts
            .as_ref()
            .is_none_or(|c| (c.revision, c.width, c.tab_width) != key);
        if stale {
            #[cfg(test)]
            {
                self.sweeps += 1;
            }
            let mut rows = Vec::with_capacity(pos::line_count(rope));
            let mut total = 0;
            let _: Option<()> = pos::each_line(rope, 0, false, |_, text| {
                let count = row_count(text, width, tab_width);
                rows.push(count);
                total += count;
                ControlFlow::Continue(())
            });
            self.counts = Some(Counts {
                revision,
                width,
                tab_width,
                rows,
                total,
            });
        }
        self.counts.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    /// A row's text, and the rows as (start byte, start column).
    fn texts(layout: &Layout) -> Vec<&str> {
        (0..layout.len())
            .map(|row| &layout.text()[layout.rows()[row].start.0..layout.row_end(row).0])
            .collect()
    }

    fn starts(layout: &Layout) -> Vec<(usize, usize)> {
        layout.rows().iter().map(|row| (row.start.0, row.col.0)).collect()
    }

    /// Grapheme boundaries of `text`, the end included.
    fn boundaries(text: &str) -> Vec<usize> {
        let mut out = vec![0];
        let mut i = 0;
        while i < text.len() {
            i = pos::next_grapheme_in(text, i);
            out.push(i);
        }
        out
    }

    #[test]
    fn breaks_after_the_last_space_that_fits() {
        let layout = Layout::compute("aaaa bbbb cccc", 10, 4);
        assert_eq!(texts(&layout), ["aaaa bbbb ", "cccc"]);
        assert_eq!(starts(&layout), [(0, 0), (10, 0)]);

        // A single long word breaks between graphemes.
        let layout = Layout::compute("abcdefghij", 4, 4);
        assert_eq!(texts(&layout), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wide_chars_and_tabs_are_never_split() {
        // A wide char that would straddle the edge moves to the next row whole.
        let layout = Layout::compute("a日bb", 3, 4);
        assert_eq!(texts(&layout), ["a日", "bb"]);
        assert_eq!(starts(&layout), [(0, 0), (4, 0)]);

        // Wider than the row: it gets a row of its own.
        let layout = Layout::compute("日日日", 3, 4);
        assert_eq!(texts(&layout), ["日", "日", "日"]);

        // A tab is a whole tab stop: one wider than the row gets its own row.
        let layout = Layout::compute("\tx", 2, 4);
        assert_eq!(texts(&layout), ["\t", "x"]);
        assert_eq!(starts(&layout), [(0, 0), (1, 0)], "no indentation room");

        // The tab fills the rest of a row (a tab stop at column 1 is 3 wide).
        let layout = Layout::compute("a\tb", 4, 4);
        assert_eq!(texts(&layout), ["a\t", "b"]);
    }

    #[test]
    fn continuation_rows_keep_the_indentation() {
        let layout = Layout::compute("    aaa bb cc", 9, 4);
        assert_eq!(texts(&layout), ["    aaa ", "bb cc"]);
        assert_eq!(starts(&layout), [(0, 0), (8, 4)]);

        // A space that doesn't fit ends the row before it, so the word starts the next one.
        let layout = Layout::compute("    aaaa bbbb", 8, 4);
        assert_eq!(texts(&layout), ["    aaaa ", "bbbb"]);
        assert_eq!(starts(&layout), [(0, 0), (9, 4)]);

        // An indentation with no room for text keeps none: pathological, but a row must hold
        // at least one grapheme (the space that overflows the first row ends it).
        let layout = Layout::compute("        x", 4, 4);
        assert_eq!(starts(&layout), [(0, 0), (5, 0)]);
        assert_eq!(texts(&layout), ["     ", "   x"]);
    }

    #[test]
    fn rows_and_columns_map_both_ways() {
        let layout = Layout::compute("    aaa bb cc", 9, 4);
        // The break space ends the first row; the word's first byte is the second row's start.
        assert_eq!(layout.row_of(ByteIdx(8)), (1, DisplayCol(0)));
        assert_eq!(layout.row_of(ByteIdx(9)), (1, DisplayCol(1)));
        assert_eq!(layout.row_of(ByteIdx(13)), (1, DisplayCol(5)));
        assert_eq!(layout.row_of(ByteIdx(100)), (1, DisplayCol(5)), "clamped");
        assert_eq!(layout.byte_at(1, DisplayCol(0)), ByteIdx(8));
        assert_eq!(layout.byte_at(1, DisplayCol(1)), ByteIdx(9));
        assert_eq!(layout.byte_at(1, DisplayCol(9)), ByteIdx(13));
        assert_eq!(layout.row_end(0), ByteIdx(8));
        assert_eq!(layout.row_end(1), ByteIdx(13));

        // A column inside a wide char gives its start.
        let layout = Layout::compute("a日bb", 3, 4);
        assert_eq!(layout.byte_at(0, DisplayCol(1)), ByteIdx(1));
        assert_eq!(layout.byte_at(0, DisplayCol(2)), ByteIdx(1));
        assert_eq!(layout.byte_at(0, DisplayCol(3)), ByteIdx(4));

        // Tabs expand from the row's starting column.
        let layout = Layout::compute("ab\tcd", 10, 4);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout.row_of(ByteIdx(2)), (0, DisplayCol(2)));
        assert_eq!(layout.row_of(ByteIdx(3)), (0, DisplayCol(4)));
        assert_eq!(layout.byte_at(0, DisplayCol(3)), ByteIdx(2), "inside the tab");
        assert_eq!(layout.byte_at(0, DisplayCol(4)), ByteIdx(3));
        assert_eq!(layout.row_of(ByteIdx(4)), (0, DisplayCol(5)));
    }

    #[test]
    fn every_grapheme_boundary_round_trips() {
        for text in [
            "",
            "    aaa bb cc",
            "a日bb",
            "\tx y\tz",
            "longwordlongword",
            "a          b",
            "  \t  x",
            "日本語 語",
            "a b 日\tc  ",
        ] {
            for width in 1..=12 {
                let layout = Layout::compute(text, width, 4);
                let all = boundaries(text);
                for (row, start) in layout.rows().iter().enumerate() {
                    if row > 0 {
                        assert!(start.start.0 > layout.rows()[row - 1].start.0, "{text:?}");
                    }
                    assert_eq!(
                        layout.row_end(row),
                        if row + 1 < layout.len() {
                            layout.rows()[row + 1].start
                        } else {
                            ByteIdx(text.len())
                        }
                    );
                }
                for off in all {
                    let (row, col) = layout.row_of(ByteIdx(off));
                    assert!(row < layout.len(), "{text:?} {width} at {off}");
                    assert_eq!(layout.byte_at(row, col), ByteIdx(off), "{text:?} at {off}");
                }
            }
        }
    }

    /// A slow, straightforward wrap of one line: for every row, try every grapheme prefix to
    /// find the longest that fits, then scan that prefix for the last space to break after.
    /// O(n²) and only for the reference check.
    fn reference(text: &str, width: usize, tab_width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let indent = indentation(text, tab_width);
        let continuation = if indent < width { indent } else { 0 };
        let all = boundaries(text);
        let mut rows = Vec::new();
        let (mut start, mut col) = (0, 0);
        loop {
            rows.push((start, col));
            if start == text.len() {
                break;
            }
            // The longest prefix of whole graphemes that fits the row.
            let mut limit = start;
            for &boundary in all.iter().filter(|&&b| b > start) {
                let w = col_of(text, start, boundary, col, tab_width);
                if w > width {
                    break;
                }
                limit = boundary;
            }
            let row_end = if limit == start {
                // Not even one grapheme fits: take the first alone.
                *all.iter().find(|&&b| b > start).unwrap()
            } else if limit == text.len() {
                limit
            } else if &text[limit..pos::next_grapheme_in(text, limit)] == " " {
                // The space after the fitting text ends the row (it doesn't fit, but a break
                // after a space keeps it there).
                pos::next_grapheme_in(text, limit)
            } else {
                // The last space in the fitting text, else the fitting text itself.
                let mut space = None;
                for &boundary in all.iter().filter(|&&b| b > start && b <= limit) {
                    if text[start..boundary].ends_with(' ') {
                        space = Some(boundary);
                    }
                }
                space.unwrap_or(limit)
            };
            debug_assert!(row_end > start, "{text:?} {width}");
            if row_end >= text.len() {
                break;
            }
            start = row_end;
            col = continuation;
        }
        rows
    }

    /// `text[from..to]` drawn from column `start`, in display columns.
    fn col_of(text: &str, from: usize, to: usize, start: usize, tab_width: usize) -> usize {
        let mut col = start;
        let mut i = from;
        while i < to {
            let next = pos::next_grapheme_in(text, i);
            col += pos::display_grapheme(&text[i..next], DisplayCol(col), tab_width).1;
            i = next;
        }
        col
    }

    /// A deterministic set of strings: words, spaces, tabs and wide chars, some indented.
    fn generated(seed: u64, len: usize) -> String {
        const PIECES: &[&str] = &["a", "b", "e", " ", " ", " ", "\t", "日", "語", "。"];
        let mut s = String::new();
        let mut x = seed;
        for _ in 0..len {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            s.push_str(PIECES[(x >> 33) as usize % PIECES.len()]);
        }
        s
    }

    #[test]
    fn matches_a_naive_reference_wrap() {
        let mut texts: Vec<String> = [
            "",
            "a",
            " ",
            "aaaa bbbb cccc",
            "    aaa bb cc",
            "a日bb 日 語",
            "\tx y\tz",
            "longwordlongwordlongword",
            "a          b",
            "  \t  x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for seed in 0..24 {
            texts.push(generated(seed, (seed as usize) % 31));
        }
        // A capped set of widths and tab widths: a failure is one assertion away.
        for text in &texts {
            for width in 1..=16 {
                for tab_width in [2, 4, 8] {
                    let layout = Layout::compute(text, width, tab_width);
                    assert_eq!(
                        starts(&layout),
                        reference(text, width, tab_width),
                        "{text:?} at width {width}, tabs {tab_width}"
                    );
                    assert_eq!(row_count(text, width, tab_width), layout.len());
                }
            }
        }
    }

    #[test]
    fn layout_cache_follows_the_revision_and_width() {
        let mut wrap = Wrap::default();
        let rope = Rope::from_str("aaaa bbbb\ncccc");
        {
            let layout = wrap.layout(&rope, 1, 0, 5, 4);
            assert_eq!(layout.text(), "aaaa bbbb");
            assert_eq!(layout.len(), 2);
        }
        // A wider view at the same revision lays the line out again.
        assert_eq!(wrap.layout(&rope, 1, 0, 40, 4).len(), 1);
        // An edit is a new revision: the cached text is gone even if the caller forgot to map.
        let edited = Rope::from_str("aaaa bbbb cccc\ndd");
        let layout = wrap.layout(&edited, 2, 0, 5, 4);
        assert_eq!(layout.text(), "aaaa bbbb cccc");
        assert_eq!(layout.len(), 3);
        assert_eq!(wrap.layout(&edited, 2, 1, 5, 4).text(), "dd");
    }

    #[test]
    fn total_and_line_row_counts_match_the_layouts() {
        let rope = Rope::from_str("aaaa bbbb cccc\ncc\n\n    dddd eeee\n");
        let mut wrap = Wrap::default();
        let total = wrap.total_rows(&rope, 3, 5, 4);
        let mut sum = 0;
        for line in 0..pos::line_count(&rope) {
            let rows = wrap.line_rows(&rope, 3, line, 5, 4);
            assert_eq!(rows, wrap.layout(&rope, 3, line, 5, 4).len(), "line {line}");
            sum += rows;
        }
        assert_eq!(sum, total);
        assert_eq!(wrap.line_rows(&rope, 3, 99, 5, 4), 1, "past the end");
        assert_eq!(wrap.sweeps, 1, "one layout of the whole text for the key");
    }

    /// Global rows: a line's first row from `row_of_line`, and back from `line_at` (a multi-row
    /// line's rows all map to it, the end clamps to the last row).
    #[test]
    fn global_rows_map_to_lines_and_back() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let mut wrap = Wrap::default();
        assert_eq!(wrap.total_rows(&rope, 0, 5, 4), 6);
        let starts: Vec<usize> = (0..=3).map(|line| wrap.row_of_line(&rope, 0, line, 5, 4)).collect();
        assert_eq!(starts, [0, 2, 3, 5], "line 0 wraps to two rows, line 2 to two");
        assert_eq!(wrap.row_of_line(&rope, 0, 99, 5, 4), 6, "past the end: the total");
        let rows = [(0, 0), (0, 1), (1, 0), (2, 0), (2, 1), (3, 0)];
        for (row, at) in rows.iter().enumerate() {
            assert_eq!(wrap.line_at(&rope, 0, row, 5, 4), *at, "row {row}");
        }
        assert_eq!(wrap.line_at(&rope, 0, 99, 5, 4), (3, 0), "clamped to the last row");
    }

    fn fresh_total(rope: &Rope, revision: u64, width: usize, tab_width: usize) -> usize {
        Wrap::default().total_rows(rope, revision, width, tab_width)
    }

    /// Applies `change` to `rope`, `map`s the wrap through it and checks the counts against a
    /// wrap that lays everything out again. The counts' first layout is the one sweep; `map`
    /// lays out only the edited lines, unless the line count changed or a line break was
    /// inserted while it stayed (the line indices shifted).
    fn mapped(rope: &Rope, change: Change, revision: u64, width: usize, tab_width: usize) -> (Rope, Wrap, usize) {
        let set = Arc::new(ChangeSet::new(vec![change]));
        let mut edited = rope.clone();
        set.apply(&mut edited);
        let same_lines = pos::line_count(rope) == pos::line_count(&edited);
        let inserts_break = set.inserts_line_break();
        let mut wrap = Wrap::default();
        let before = wrap.total_rows(rope, revision - 1, width, tab_width);
        assert_eq!(before, fresh_total(rope, revision - 1, width, tab_width));
        wrap.map(&[set], &edited, revision);
        let after = wrap.total_rows(&edited, revision, width, tab_width);
        assert_eq!(after, fresh_total(&edited, revision, width, tab_width));
        assert_eq!(
            wrap.sweeps,
            if same_lines && !inserts_break { 1 } else { 2 },
            "map lays out only the edited lines unless the line count changes or a line break is inserted"
        );
        (edited, wrap, after)
    }

    #[test]
    fn mapping_through_edits_keeps_the_row_counts() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        // 2 + 1 + 2 rows, and the empty line after the last line break.
        assert_eq!(fresh_total(&rope, 0, 5, 4), 6);

        // Typing inside a line: the row count can grow without the line count changing.
        let (edited, mut wrap, _) = mapped(
            &rope,
            Change {
                start: ByteIdx(9),
                end: ByteIdx(9),
                text: " x y".into(),
            },
            1,
            5,
            4,
        );
        assert_eq!(wrap.line_rows(&edited, 1, 0, 5, 4), 3);

        // A line break: the counts are laid out again on the next use.
        let (edited, mut wrap, _) = mapped(
            &rope,
            Change {
                start: ByteIdx(4),
                end: ByteIdx(4),
                text: "\n".into(),
            },
            2,
            5,
            4,
        );
        assert_eq!(wrap.line_rows(&edited, 2, 0, 5, 4), 1);
        assert_eq!(wrap.line_rows(&edited, 2, 1, 5, 4), 1);

        // A wide char spanning a row edge.
        let (edited, mut wrap, _) = mapped(
            &rope,
            Change {
                start: ByteIdx(5),
                end: ByteIdx(5),
                text: "日".into(),
            },
            3,
            5,
            4,
        );
        assert_eq!(wrap.line_rows(&edited, 3, 0, 5, 4), 3);

        // A change across two lines that keeps the line count: both rows are laid out again.
        let (edited, mut wrap, _) = mapped(
            &rope,
            Change {
                start: ByteIdx(5),
                end: ByteIdx(12),
                text: "b\ncccccccc".into(),
            },
            4,
            5,
            4,
        );
        assert_eq!(wrap.line_rows(&edited, 4, 0, 5, 4), 2, "aaaa b");
        assert_eq!(wrap.line_rows(&edited, 4, 1, 5, 4), 2, "cccccccc");
    }

    #[test]
    fn mapping_offsetting_line_breaks_relays_out() {
        // Removing a newline in one change and adding one in another keeps the line count but
        // shifts the lines between the two changes up by one: `counts.rows` is keyed by line
        // index, so those lines' counts can't be updated in place and everything must be laid
        // out again.
        let rope = Rope::from_str("aa\nbbbbbbbbbb\ncc\ndddddddddd\nee\n");
        let set = Arc::new(ChangeSet::new(vec![
            Change {
                start: ByteIdx(2),
                end: ByteIdx(3),
                text: "".into(),
            },
            Change {
                start: ByteIdx(30),
                end: ByteIdx(30),
                text: "\n".into(),
            },
        ]));
        let mut edited = rope.clone();
        set.apply(&mut edited);
        assert_eq!(pos::line_count(&rope), pos::line_count(&edited), "line count kept");

        let mut wrap = Wrap::default();
        wrap.total_rows(&rope, 0, 5, 4);
        wrap.map(&[set], &edited, 1);
        assert_eq!(wrap.line_rows(&edited, 1, 1, 5, 4), 1, "cc is one row");
        assert_eq!(wrap.line_rows(&edited, 1, 2, 5, 4), 2, "dddddddddd is two rows");
        assert_eq!(wrap.total_rows(&edited, 1, 5, 4), fresh_total(&edited, 1, 5, 4));
    }

    #[test]
    fn layout_cache_follows_the_tab_width() {
        let mut wrap = Wrap::default();
        let rope = Rope::from_str("a\tb\n");
        assert_eq!(wrap.layout(&rope, 1, 0, 8, 4).len(), 1);
        assert_eq!(wrap.layout(&rope, 1, 0, 8, 8).len(), 2, "a wider tab changes the break");
    }

    #[test]
    fn mapping_several_change_sets_in_one_revision() {
        let rope = Rope::from_str("aaaa bbbb\ncc\ndddd eeee\n");
        let first = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(4),
            text: "aaaaaa".into(),
        }]));
        let second = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(15),
            end: ByteIdx(15),
            text: " zz".into(),
        }]));
        let mut edited = rope.clone();
        first.apply(&mut edited);
        second.apply(&mut edited);

        let mut wrap = Wrap::default();
        wrap.total_rows(&rope, 0, 5, 4);
        wrap.map(&[first, second], &edited, 1);
        assert_eq!(wrap.total_rows(&edited, 1, 5, 4), fresh_total(&edited, 1, 5, 4));
        assert_eq!(wrap.sweeps, 1, "both sets mapped, no layout from scratch");
    }
}
