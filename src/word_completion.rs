//! Word-based completion (VS Code's `editor.wordBasedSuggestions: matchingDocuments`): the words
//! of the open buffers, offered when no language server answers. This module only collects
//! words and caches them per buffer by revision; `app/completion.rs` drives it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ropey::Rope;

use crate::buffer::BufferId;
use crate::buffer::pos;

/// The most text of one buffer that is scanned.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// The most words kept of one buffer.
pub const MAX_WORDS: usize = 20_000;
/// Shortest and longest word kept (in bytes).
pub const MIN_LEN: usize = 3;
pub const MAX_LEN: usize = 64;

/// The unique words of `rope` (runs of word chars, `MIN_LEN..=MAX_LEN` bytes) in first-occurrence
/// order, scanning at most `max_bytes` of its text and keeping at most `max_words`.
/// Blocking: call from spawn_blocking.
pub fn collect(rope: &Rope, max_bytes: usize, max_words: usize) -> Vec<String> {
    let end = rope.floor_char_boundary(rope.len().min(max_bytes));
    let mut seen: HashSet<String> = HashSet::new();
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut too_long = false;
    // Ends the current run: keeps it if new; true once the word cap is reached.
    let mut flush = |cur: &mut String, too_long: &mut bool, words: &mut Vec<String>| {
        if !*too_long && cur.len() >= MIN_LEN && !seen.contains(cur.as_str()) {
            seen.insert(cur.clone());
            words.push(cur.clone());
        }
        cur.clear();
        *too_long = false;
        words.len() >= max_words
    };
    for chunk in rope.slice(0..end).chunks() {
        for c in chunk.chars() {
            if pos::is_word_char(c) {
                if cur.len() + c.len_utf8() > MAX_LEN {
                    too_long = true;
                } else if !too_long {
                    cur.push(c);
                }
            } else if (!cur.is_empty() || too_long) && flush(&mut cur, &mut too_long, &mut words) {
                return words;
            }
        }
    }
    flush(&mut cur, &mut too_long, &mut words);
    words
}

/// Each buffer's words as of a revision.
#[derive(Default)]
pub struct WordCache {
    map: HashMap<BufferId, (u64, Arc<Vec<String>>)>,
}

impl WordCache {
    /// The words of `buffer` if they were collected at `revision`.
    pub fn get(&self, buffer: BufferId, revision: u64) -> Option<Arc<Vec<String>>> {
        self.map
            .get(&buffer)
            .filter(|(r, _)| *r == revision)
            .map(|(_, w)| w.clone())
    }

    /// The words of `buffer` as of the last collection, whatever its revision.
    pub fn latest(&self, buffer: BufferId) -> Option<Arc<Vec<String>>> {
        self.map.get(&buffer).map(|(_, w)| w.clone())
    }

    pub fn put(&mut self, buffer: BufferId, revision: u64, words: Arc<Vec<String>>) {
        self.map.insert(buffer, (revision, words));
    }

    /// Forgets the buffers `open` rejects (closed ones).
    pub fn prune(&mut self, open: impl Fn(BufferId) -> bool) {
        self.map.retain(|&id, _| open(id));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        collect(&Rope::from_str(s), MAX_BYTES, MAX_WORDS)
    }

    #[test]
    fn collects_unique_words_in_first_occurrence_order() {
        assert_eq!(words("beta alpha, beta; gamma.alpha"), ["beta", "alpha", "gamma"]);
    }

    #[test]
    fn skips_short_and_long_words() {
        let long = "x".repeat(MAX_LEN + 1);
        let ok = "y".repeat(MAX_LEN);
        assert_eq!(words(&format!("a ab abc {long} {ok}")), vec!["abc".to_string(), ok]);
    }

    #[test]
    fn keeps_underscores_digits_and_unicode_words() {
        assert_eq!(words("foo_bar x1y2 café"), ["foo_bar", "x1y2", "café"]);
    }

    #[test]
    fn caps_the_words_and_the_text() {
        assert_eq!(
            collect(&Rope::from_str("aaa bbb ccc ddd"), MAX_BYTES, 2),
            ["aaa", "bbb"]
        );
        assert_eq!(collect(&Rope::from_str("aaa bbb ccc"), 9, MAX_WORDS), ["aaa", "bbb"]);
    }

    #[test]
    fn the_cache_reuses_a_revision_and_prunes() {
        let mut c = WordCache::default();
        c.put(1, 5, Arc::new(vec!["abc".into()]));
        c.put(2, 1, Arc::new(vec![]));
        assert!(c.get(1, 5).is_some());
        assert!(c.get(1, 6).is_none());
        c.prune(|id| id == 2);
        assert!(c.get(1, 5).is_none() && c.len() == 1);
    }
}
