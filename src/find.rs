//! The find / replace widget's model (no drawing): its two inputs, which one has the cursor,
//! and the matches of the query in the active buffer, cached per (buffer, revision, query,
//! options) so the count and the highlights cost one `find_all` per change, not per frame.
//! The options themselves live on the app (`App::find_options`), which Cmd+D borrows.

use std::ops::Range;

use ropey::Rope;

use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::field::TextField;
use crate::search::{Match, Matcher, Options, Replacement, SearchError};

/// Which input has the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    #[default]
    Find,
    Replace,
}

#[derive(Debug, Default)]
pub struct FindWidget {
    /// Shown over the active editor. The query outlives closing it (Cmd+F reopens with it).
    pub visible: bool,
    pub query: TextField,
    pub replace: TextField,
    /// The replace row is shown (Cmd+Alt+F).
    pub replace_visible: bool,
    pub field: Field,
    cache: Option<Cache>,
}

#[derive(Debug)]
struct Cache {
    key: Key,
    found: Result<Found, SearchError>,
}

#[derive(Debug)]
struct Found {
    matcher: Matcher,
    matches: Vec<Match>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    buffer: BufferId,
    revision: u64,
    query: String,
    options: Options,
}

impl FindWidget {
    /// The input with the cursor.
    pub fn field_mut(&mut self) -> &mut TextField {
        match self.field {
            Field::Find => &mut self.query,
            Field::Replace => &mut self.replace,
        }
    }

    /// Searches `rope` (buffer `buffer` at `revision`) unless the cached result is for exactly that.
    pub fn refresh(&mut self, buffer: BufferId, revision: u64, rope: &Rope, options: Options) {
        let key = Key {
            buffer,
            revision,
            query: self.query.text.clone(),
            options,
        };
        if self.cache.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let t = std::time::Instant::now();
        let found = Matcher::new(&key.query, options).map(|matcher| Found {
            matches: matcher.find_all(rope),
            matcher,
        });
        if let Ok(f) = &found {
            tracing::debug!(
                matches = f.matches.len(),
                elapsed_us = t.elapsed().as_micros() as u64,
                "find all"
            );
        }
        self.cache = Some(Cache { key, found });
    }

    /// Whether the cache holds the matches for this buffer at this revision.
    pub fn is_current(&self, buffer: BufferId, revision: u64) -> bool {
        self.cache
            .as_ref()
            .is_some_and(|c| c.key.buffer == buffer && c.key.revision == revision)
    }

    /// The matcher for the cached query, unless it's empty or invalid.
    pub fn matcher(&self) -> Option<&Matcher> {
        let found = self.cache.as_ref()?.found.as_ref().ok()?;
        (!found.matcher.is_empty()).then_some(&found.matcher)
    }

    pub fn error(&self) -> Option<&SearchError> {
        self.cache.as_ref()?.found.as_ref().err()
    }

    /// Every match, in order (empty without a valid query).
    pub fn matches(&self) -> &[Match] {
        match self.cache.as_ref().map(|c| &c.found) {
            Some(Ok(f)) => &f.matches,
            _ => &[],
        }
    }

    /// The matches overlapping `range`, for drawing the visible lines.
    pub fn matches_in(&self, range: Range<ByteIdx>) -> &[Match] {
        let all = self.matches();
        let from = all.partition_point(|m| m.end < range.start);
        let to = from + all[from..].partition_point(|m| m.start <= range.end);
        &all[from..to]
    }

    /// The 0-based index of the match that is exactly `start..end`.
    pub fn index_of(&self, start: ByteIdx, end: ByteIdx) -> Option<usize> {
        let all = self.matches();
        let i = all.partition_point(|m| m.start < start);
        all.get(i).filter(|m| m.start == start && m.end == end).map(|_| i)
    }

    /// "3 of 41" when the selection `sel` is a match, "? of 41" when it isn't, "No results",
    /// or nothing without a query. An invalid regex shows in `error` instead.
    pub fn count_label(&self, sel: (ByteIdx, ByteIdx)) -> String {
        if self.query.text.is_empty() || self.error().is_some() {
            return String::new();
        }
        let n = self.matches().len();
        match (n, self.index_of(sel.0, sel.1)) {
            (0, _) => "No results".into(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) => format!("? of {n}"),
        }
    }

    /// What replaces `m` (a cached match): the replace field, with groups expanded for a regex.
    pub fn replacement_for(&self, rope: &Rope, m: Match, regex: bool) -> Option<String> {
        let matcher = self.matcher()?;
        let with = Replacement::new(&self.replace.text, regex);
        let mut out = None;
        matcher.replace_in(rope, m.start..m.end, &with, |found, text| {
            if found == m {
                out = Some(text.to_string());
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn widget(query: &str, text: &str, options: Options) -> (FindWidget, Rope) {
        let rope = Buffer::from_str(text).rope;
        let mut w = FindWidget {
            query: TextField::new(query.into()),
            ..FindWidget::default()
        };
        w.refresh(1, 0, &rope, options);
        (w, rope)
    }

    #[test]
    fn counts_and_caches() {
        let (mut w, rope) = widget("ab", "ab xab\nab", Options::default());
        assert_eq!(w.matches().len(), 3);
        assert_eq!(w.count_label((ByteIdx(4), ByteIdx(6))), "2 of 3");
        assert_eq!(w.count_label((ByteIdx(0), ByteIdx(0))), "? of 3");
        assert!(w.is_current(1, 0));
        assert!(!w.is_current(1, 1));
        assert!(!w.is_current(2, 0));

        // Same key: no new search (the old result stays even for a different rope).
        let other = Buffer::from_str("nothing").rope;
        w.refresh(1, 0, &other, Options::default());
        assert_eq!(w.matches().len(), 3);
        w.refresh(1, 1, &other, Options::default());
        assert_eq!(w.count_label((ByteIdx(0), ByteIdx(0))), "No results");
        w.query = TextField::default();
        w.refresh(1, 1, &rope, Options::default());
        assert_eq!(w.count_label((ByteIdx(0), ByteIdx(0))), "");
        assert!(w.matcher().is_none());
    }

    #[test]
    fn visible_slice_and_errors() {
        let (w, _) = widget("a", "a\na\na\na\na", Options::default());
        let vis = w.matches_in(ByteIdx(2)..ByteIdx(5));
        assert_eq!(vis.iter().map(|m| m.start.0).collect::<Vec<_>>(), vec![2, 4]);
        assert_eq!(w.index_of(ByteIdx(4), ByteIdx(5)), Some(2));
        assert_eq!(w.index_of(ByteIdx(4), ByteIdx(4)), None);

        let regex = Options {
            regex: true,
            ..Options::default()
        };
        let (w, _) = widget("(", "(", regex);
        assert!(w.error().is_some());
        assert!(w.matches().is_empty());
        assert_eq!(w.count_label((ByteIdx(0), ByteIdx(0))), "");
    }

    #[test]
    fn replacement_expands_groups() {
        let regex = Options {
            regex: true,
            ..Options::default()
        };
        let (mut w, rope) = widget(r"(\w+)=(\w+)", "a=b c=d", regex);
        w.replace = TextField::new("$2=$1".into());
        let m = w.matches()[1];
        assert_eq!(w.replacement_for(&rope, m, true).as_deref(), Some("d=c"));
        assert_eq!(w.replacement_for(&rope, m, false).as_deref(), Some("$2=$1"));
    }
}
