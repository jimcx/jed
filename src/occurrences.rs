//! Selection highlight (VS Code's `editor.selectionHighlight`): every other place the primary
//! selection's text occurs in the buffer is drawn highlighted (`editor.selectionHighlight`), and
//! the status bar shows how many there are (the selection itself included) while the find
//! widget is closed.
//!
//! The text qualifies when it is non-empty, within one line, at most `MAX_LEN` bytes and not
//! only whitespace (`needle`). Matching is literal and case-sensitive, through the same
//! `search::Matcher` as find; matches never span lines.
//!
//! Drawing calls `Occurrences::refresh` every frame; it searches the whole buffer only when the
//! revision or the text changes (as the find widget's cache), so a frame otherwise costs a
//! binary search (`matches_in`).

use std::ops::Range;

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};
use crate::search::{Match, Matcher, Options};
use crate::selection::Selection;

/// The longest selection (in bytes) whose occurrences are highlighted (VS Code's 200).
pub const MAX_LEN: usize = 200;

/// The text of `sel` to look for, unless it doesn't qualify (see the module docs).
pub fn needle(rope: &Rope, sel: &Selection) -> Option<String> {
    let (start, end) = (sel.start(), sel.end());
    if sel.is_empty() || end.0 - start.0 > MAX_LEN || pos::line_of(rope, start) != pos::line_of(rope, end) {
        return None;
    }
    let text = rope.slice(start.0..end.0).to_string();
    // A selection ending at a line break's middle (CR LF) or holding one isn't on one line.
    (!text.contains(['\n', '\r']) && !text.trim().is_empty()).then_some(text)
}

/// A view's occurrences of its selection's text, cached between frames.
#[derive(Debug, Default, Clone)]
pub struct Occurrences {
    /// The revision and text the matches are for; `None` while nothing is highlighted.
    key: Option<(u64, String)>,
    matches: Vec<Match>,
}

impl Occurrences {
    /// Finds `needle` in `rope` (at `revision`) unless the cache is already for exactly that;
    /// `None` clears it.
    pub fn refresh(&mut self, rope: &Rope, revision: u64, needle: Option<String>) {
        let Some(needle) = needle else {
            self.key = None;
            self.matches.clear();
            return;
        };
        if self.key.as_ref().is_some_and(|(r, n)| *r == revision && *n == needle) {
            return;
        }
        let t = std::time::Instant::now();
        let options = Options {
            case_sensitive: true,
            ..Options::default()
        };
        // Plain text always compiles.
        self.matches = Matcher::new(&needle, options).map_or_else(|_| Vec::new(), |m| m.find_all(rope));
        tracing::debug!(
            matches = self.matches.len(),
            elapsed_us = t.elapsed().as_micros() as u64,
            "selection occurrences"
        );
        self.key = Some((revision, needle));
    }

    /// How many times the text occurs (the selection included), while something is highlighted.
    pub fn count(&self) -> Option<usize> {
        self.key.as_ref().map(|_| self.matches.len())
    }

    /// The occurrences overlapping `range`, for drawing the visible lines.
    pub fn matches_in(&self, range: Range<ByteIdx>) -> &[Match] {
        let all = &self.matches;
        let from = all.partition_point(|m| m.end < range.start);
        let to = from + all[from..].partition_point(|m| m.start <= range.end);
        &all[from..to]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, b: usize) -> Selection {
        Selection::new(ByteIdx(a), ByteIdx(b))
    }

    #[test]
    fn needle_wants_one_line_of_non_blank_text() {
        let rope = Rope::from_str("foo bar foo\n  \nfoo");
        assert_eq!(needle(&rope, &sel(0, 3)).as_deref(), Some("foo"));
        // Backwards selections too.
        assert_eq!(needle(&rope, &sel(3, 0)).as_deref(), Some("foo"));
        assert_eq!(needle(&rope, &sel(2, 2)), None, "empty");
        assert_eq!(needle(&rope, &sel(12, 14)), None, "blank");
        assert_eq!(needle(&rope, &sel(8, 13)), None, "spans lines");
        assert_eq!(needle(&rope, &sel(8, 12)), None, "takes the line break");
        let long = Rope::from_str(&"x".repeat(MAX_LEN + 1));
        assert_eq!(needle(&long, &sel(0, MAX_LEN + 1)), None, "too long");
        assert!(needle(&long, &sel(0, MAX_LEN)).is_some());
    }

    #[test]
    fn finds_case_sensitive_literal_occurrences_and_caches_them() {
        let rope = Rope::from_str("a.b A.B a.b axb\na.b");
        let mut o = Occurrences::default();
        assert_eq!(o.count(), None);
        o.refresh(&rope, 1, Some("a.b".into()));
        assert_eq!(o.count(), Some(3), "literal and case-sensitive");
        assert_eq!(
            o.matches_in(ByteIdx(4)..ByteIdx(12)),
            &[Match {
                start: ByteIdx(8),
                end: ByteIdx(11)
            }]
        );
        // Same key: not searched again (a different rope would give a different count).
        o.refresh(&Rope::from_str("a.b"), 1, Some("a.b".into()));
        assert_eq!(o.count(), Some(3));
        o.refresh(&Rope::from_str("a.b"), 2, Some("a.b".into()));
        assert_eq!(o.count(), Some(1));
        o.refresh(&rope, 2, None);
        assert_eq!(o.count(), None);
        assert!(o.matches_in(ByteIdx(0)..ByteIdx(20)).is_empty());
    }
}
