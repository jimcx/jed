//! The find / replace widget's model (no drawing): its two inputs, which one has the cursor,
//! and the matches of the query in the active buffer, cached per (buffer, revision, query,
//! options) so the count and the highlights cost one `find_all` per change, not per frame.
//! The options themselves live on the app (`App::find_options`), which Cmd+D borrows.
//!
//! On a hex tab the same inputs search bytes instead (`HexFind`): the query read as a
//! `hex::Query` (ASCII text or hex pairs, by `Options::hex`), and the whole file's count, which
//! the app computes off the UI thread per query generation. Highlights and next / previous search
//! from the bytes directly (`hex::find_in`, `find_next`), so nothing else is cached.

use std::ops::Range;

use ropey::Rope;

use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::field::TextField;
use crate::hex::{self, HexError, Mode};
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
    /// The byte search's state while the active tab is a hex view.
    pub hex: HexFind,
}

/// Find on a hex tab: the query as read last and the whole file's count for it.
#[derive(Debug, Default)]
pub struct HexFind {
    key: Option<HexKey>,
    query: Option<Result<hex::Query, HexError>>,
    /// Bumped by every new key: a count computed for an older one is dropped.
    generation: u64,
    /// `None` while the count runs ("…").
    count: Option<hex::Count>,
}

/// What a `HexFind` was computed for: the tab, its bytes (a re-read is new bytes), the query.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HexKey {
    tab: u64,
    bytes: (usize, usize),
    text: String,
    mode: Mode,
    case_sensitive: bool,
}

/// A count to compute off the UI thread: the generation it's for, and the query.
#[derive(Debug)]
pub struct CountJob {
    pub generation: u64,
    pub query: hex::Query,
}

impl HexFind {
    /// Reads `text` again when the tab (`tab`, `bytes`), the text or the options changed since
    /// last time: a new generation, its count unknown. Returns the count to start, if the new
    /// query is valid and not empty.
    pub fn refresh(&mut self, tab: u64, bytes: &[u8], text: &str, options: Options) -> Option<CountJob> {
        let key = HexKey {
            tab,
            bytes: (bytes.as_ptr() as usize, bytes.len()),
            text: text.to_string(),
            mode: if options.hex { Mode::Hex } else { Mode::Ascii },
            case_sensitive: options.case_sensitive,
        };
        if self.key.as_ref() == Some(&key) {
            return None;
        }
        let query = hex::Query::parse(&key.text, key.mode, key.case_sensitive);
        self.generation += 1;
        self.count = None;
        self.key = Some(key);
        let job = match &query {
            Ok(q) if !q.is_empty() => Some(CountJob {
                generation: self.generation,
                query: q.clone(),
            }),
            _ => None,
        };
        self.query = Some(query);
        job
    }

    /// The current query's generation (what its count must carry).
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The tab the state is for.
    pub fn tab(&self) -> Option<u64> {
        self.key.as_ref().map(|k| k.tab)
    }

    /// A count arrived: kept when it's for the current tab and generation, else dropped (false).
    pub fn counted(&mut self, tab: u64, generation: u64, count: hex::Count) -> bool {
        if self.tab() != Some(tab) || generation != self.generation {
            return false;
        }
        self.count = Some(count);
        true
    }

    /// The query, unless it's empty or invalid.
    pub fn query(&self) -> Option<&hex::Query> {
        self.query.as_ref()?.as_ref().ok().filter(|q| !q.is_empty())
    }

    pub fn error(&self) -> Option<&HexError> {
        self.query.as_ref()?.as_ref().err()
    }

    /// The count, once it arrived.
    #[cfg(test)]
    pub fn count(&self) -> Option<&hex::Count> {
        self.count.as_ref()
    }

    /// "3 of 41" when the selection `sel` is a match (counted), "? of 41" when it isn't (or lies
    /// past the cap: "? of 100000+"), "…" while counting, "No results", or nothing without a
    /// valid query.
    pub fn count_label(&self, sel: Option<Range<usize>>) -> String {
        let Some(q) = self.query() else {
            return String::new();
        };
        let Some(c) = &self.count else {
            return "…".into();
        };
        let total = if c.capped {
            format!("{}+", c.found())
        } else {
            c.found().to_string()
        };
        let index = sel.filter(|s| s.len() == q.len()).and_then(|s| c.index_of(s.start));
        match (c.found(), index) {
            (0, _) if !c.capped => "No results".into(),
            (_, Some(i)) => format!("{} of {total}", i + 1),
            _ => format!("? of {total}"),
        }
    }
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
    fn hex_find_reads_counts_and_drops_stale_counts() {
        let bytes = b"\x00ELF elf";
        let mut h = HexFind::default();
        let mut opts = Options::default();
        let job = h.refresh(7, bytes, "elf", opts).expect("a count to start");
        assert_eq!(job.query.len(), 3);
        assert_eq!(h.count_label(Some(1..4)), "…");
        assert!(h.refresh(7, bytes, "elf", opts).is_none(), "unchanged: no new count");
        let count = hex::count(bytes, &job.query, hex::HEX_MAX_COUNT);
        assert!(!h.counted(8, job.generation, count.clone()), "another tab");
        assert!(!h.counted(7, job.generation - 1, count.clone()), "an older query");
        assert!(h.counted(7, job.generation, count));
        assert_eq!(h.count_label(Some(5..8)), "2 of 2");
        assert_eq!(h.count_label(Some(5..7)), "? of 2", "not a whole match");
        assert_eq!(h.count_label(None), "? of 2");

        opts.hex = true;
        assert!(h.refresh(7, bytes, "4", opts).is_none(), "invalid: nothing to count");
        assert_eq!(h.error(), Some(&HexError::Odd));
        assert_eq!(h.count_label(None), "");
        let job = h.refresh(7, bytes, "aa", opts).unwrap();
        assert!(h.counted(7, job.generation, hex::count(bytes, &job.query, 10)));
        assert_eq!(h.count_label(None), "No results");
        let job = h.refresh(7, b"aaaa", "61", opts).unwrap();
        assert!(h.counted(7, job.generation, hex::count(b"aaaa", &job.query, 2)));
        assert_eq!(h.count_label(Some(1..2)), "2 of 2+");
        assert_eq!(h.count_label(Some(3..4)), "? of 2+", "past the cap");
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
