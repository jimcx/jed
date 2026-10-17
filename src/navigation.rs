//! Back / forward navigation's history, the model (no app state; `app/navigation.rs` records
//! places and goes to them): VS Code's Go Back / Go Forward with one history for the window.
//!
//! Entries are places — a group, a file and, for a text tab, its primary selection and scroll
//! — oldest first, at most `MAX`. `index` is where the user is: `len()` while not navigating
//! (live), else the entry Go Back / Go Forward last went to. `record` (before a jump) drops
//! the entries from `index` on — the forward ones, and the one navigated to, which the
//! recorded place replaces — and appends the place unless it equals the last one. `back`
//! first stores where the user is (appended when live, else replacing the entry navigated to,
//! so moves made there are kept) so `forward` returns to it.
//!
//! Positions are byte offsets kept current by `map` (every applied change set of the file's
//! buffer, like other views'), and clamped when used: a file not open may change on disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::ByteIdx;
use crate::layout::GroupId;

/// How many places the history keeps (VS Code's limit).
pub const MAX: usize = 50;

/// A move of the primary cursor by this many lines or more in one step is recorded as a jump.
pub const MIN_LINES: usize = 10;

/// A text tab's place in its file: the primary selection and the scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub anchor: ByteIdx,
    pub head: ByteIdx,
    /// The first shown line's start.
    pub top: ByteIdx,
    pub left_col: usize,
}

/// A place to go back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub group: GroupId,
    /// The file, absolute.
    pub path: PathBuf,
    /// `None`: a diff tab of the file (it counts as the file, without a position).
    pub pos: Option<Pos>,
}

impl Entry {
    /// The same place: group, file and selection (the scroll doesn't count).
    pub fn same(&self, other: &Entry) -> bool {
        self.group == other.group
            && self.path == other.path
            && self.pos.map(|p| (p.anchor, p.head)) == other.pos.map(|p| (p.anchor, p.head))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Back,
    Forward,
}

#[derive(Debug, Default)]
pub struct History {
    entries: Vec<Entry>,
    /// `entries.len()`: live; else the entry navigated to.
    index: usize,
}

impl History {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Where the user was before a jump. Drops the forward entries (and the one navigated to,
    /// which `place` replaces).
    pub fn record(&mut self, place: Entry) {
        self.entries.truncate(self.index);
        self.push(place);
        self.index = self.entries.len();
    }

    /// The place before the current one, storing `current` (where the user is: `None` with no
    /// editor tab) so `forward` returns to it. `None`: nothing further back.
    pub fn back(&mut self, current: Option<Entry>) -> Option<Entry> {
        if self.index >= self.entries.len() {
            if let Some(c) = current {
                self.push(c);
                self.index = self.entries.len() - 1;
            } else {
                self.index = self.entries.len();
            }
        } else if let Some(c) = current {
            self.entries[self.index] = c;
        }
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].clone())
    }

    /// The place `back` left, storing `current` in the entry navigated to. `None`: none (live,
    /// or at the newest entry).
    pub fn forward(&mut self, current: Option<Entry>) -> Option<Entry> {
        if self.index + 1 >= self.entries.len() {
            return None;
        }
        if let Some(c) = current {
            self.entries[self.index] = c;
        }
        self.index += 1;
        Some(self.entries[self.index].clone())
    }

    /// The entry `back` / `forward` (going `dir`) just returned can't be shown (its file is gone):
    /// removed, so going `dir` again goes on from where the user still is.
    pub fn skip(&mut self, dir: Dir) {
        if self.index >= self.entries.len() {
            return;
        }
        self.entries.remove(self.index);
        if dir == Dir::Forward {
            // The place navigated from is just before it.
            self.index = self.index.saturating_sub(1);
        }
        // Back: the place navigated from moved down into `index`.
    }

    /// Follows edits to `path`'s buffer: `changes` in the order applied.
    pub fn map(&mut self, path: &Path, changes: &[Arc<ChangeSet>]) {
        if changes.is_empty() || !self.entries.iter().any(|e| e.pos.is_some() && e.path == path) {
            return;
        }
        let mappers: Vec<_> = changes.iter().map(|c| c.mapper()).collect();
        let map = |p: ByteIdx| mappers.iter().fold(p, |p, m| m.map(p, Assoc::Before));
        for e in &mut self.entries {
            if e.path == path
                && let Some(p) = &mut e.pos
            {
                p.anchor = map(p.anchor);
                p.head = map(p.head);
                p.top = map(p.top);
            }
        }
    }

    /// Appends `place` unless it's the last entry's place (then that entry takes its scroll),
    /// dropping the oldest beyond `MAX`.
    fn push(&mut self, place: Entry) {
        match self.entries.last_mut() {
            Some(last) if last.same(&place) => *last = place,
            _ => self.entries.push(place),
        }
        if self.entries.len() > MAX {
            let extra = self.entries.len() - MAX;
            self.entries.drain(..extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    fn at(path: &str, head: usize) -> Entry {
        Entry {
            group: 1,
            path: PathBuf::from(path),
            pos: Some(Pos {
                anchor: ByteIdx(head),
                head: ByteIdx(head),
                top: ByteIdx(0),
                left_col: 0,
            }),
        }
    }

    fn heads(h: &History) -> Vec<(String, usize)> {
        h.entries()
            .iter()
            .map(|e| (e.path.display().to_string(), e.pos.map_or(usize::MAX, |p| p.head.0)))
            .collect()
    }

    fn head(e: Option<Entry>) -> Option<(String, usize)> {
        e.map(|e| (e.path.display().to_string(), e.pos.unwrap().head.0))
    }

    fn s(path: &str, head: usize) -> Option<(String, usize)> {
        Some((path.to_string(), head))
    }

    #[test]
    fn back_returns_to_recorded_places_and_forward_comes_back() {
        let mut h = History::default();
        assert_eq!(h.back(Some(at("a", 0))), None, "nothing recorded");
        // `back` with nothing before still stored where we are; a jump replaces it.
        h.record(at("a", 0));
        h.record(at("b", 5));
        // Now at c.
        assert_eq!(head(h.back(Some(at("c", 7)))), s("b", 5));
        assert_eq!(head(h.back(Some(at("b", 5)))), s("a", 0));
        assert_eq!(h.back(Some(at("a", 0))), None, "the oldest");
        assert_eq!(head(h.forward(Some(at("a", 0)))), s("b", 5));
        assert_eq!(head(h.forward(Some(at("b", 5)))), s("c", 7));
        assert_eq!(h.forward(Some(at("c", 7))), None, "the newest");
        assert_eq!(heads(&h), [s("a", 0), s("b", 5), s("c", 7)].map(Option::unwrap));
    }

    #[test]
    fn going_back_keeps_moves_made_at_the_place_navigated_to() {
        let mut h = History::default();
        h.record(at("a", 0));
        h.back(Some(at("b", 3)));
        // Moved a little within a (no jump), then forward and back again.
        assert_eq!(head(h.forward(Some(at("a", 2)))), s("b", 3));
        assert_eq!(head(h.back(Some(at("b", 4)))), s("a", 2));
        assert_eq!(head(h.forward(None)), s("b", 4));
    }

    #[test]
    fn a_jump_after_going_back_drops_the_forward_entries() {
        let mut h = History::default();
        h.record(at("a", 0));
        h.record(at("b", 0));
        h.back(Some(at("c", 0)));
        h.back(Some(at("b", 0)));
        // At a (moved to 1): jump to d.
        h.record(at("a", 1));
        assert_eq!(heads(&h), [s("a", 1)].map(Option::unwrap));
        assert_eq!(h.index(), 1);
        assert_eq!(h.forward(Some(at("d", 0))), None);
        assert_eq!(head(h.back(Some(at("d", 0)))), s("a", 1));
        assert_eq!(head(h.forward(None)), s("d", 0));
    }

    #[test]
    fn consecutive_equal_places_are_one_entry() {
        let mut h = History::default();
        h.record(at("a", 0));
        let mut scrolled = at("a", 0);
        scrolled.pos.as_mut().unwrap().top = ByteIdx(9);
        h.record(scrolled.clone());
        assert_eq!(h.entries(), [scrolled], "the newer scroll kept");
        // Another group is another place.
        let mut other = at("a", 0);
        other.group = 2;
        h.record(other);
        assert_eq!(h.entries().len(), 2);
        // A diff tab of the file is the file without a position.
        let diff = Entry {
            group: 2,
            path: PathBuf::from("a"),
            pos: None,
        };
        h.record(diff.clone());
        h.record(diff.clone());
        assert_eq!(h.entries().len(), 3);
        // Going back from the last place doesn't add it again.
        let back = h.back(Some(diff)).unwrap();
        assert_eq!((back.group, back.pos.is_some()), (2, true));
        assert_eq!(h.entries().len(), 3);
        assert_eq!(h.index(), 1);
    }

    #[test]
    fn the_history_keeps_the_newest_max_entries() {
        let mut h = History::default();
        for i in 0..MAX + 7 {
            h.record(at("a", i));
        }
        assert_eq!(h.entries().len(), MAX);
        assert_eq!(h.entries()[0].pos.unwrap().head, ByteIdx(7));
        // Going back from the head stores the current place, dropping one more.
        assert_eq!(head(h.back(Some(at("b", 0)))), s("a", MAX + 6));
        assert_eq!(h.entries().len(), MAX);
        assert_eq!(h.entries()[0].pos.unwrap().head, ByteIdx(8));
        let mut back = 1;
        while h.back(None).is_some() {
            back += 1;
        }
        assert_eq!(back, MAX - 1);
        assert_eq!(h.index(), 0);
    }

    #[test]
    fn a_skipped_entry_is_removed_and_navigation_goes_on() {
        let mut h = History::default();
        h.record(at("a", 0));
        h.record(at("gone", 0));
        assert_eq!(head(h.back(Some(at("c", 0)))), s("gone", 0));
        h.skip(Dir::Back);
        assert_eq!(head(h.back(Some(at("c", 1)))), s("a", 0));
        assert_eq!(heads(&h), [s("a", 0), s("c", 1)].map(Option::unwrap));
        // Forward over a gone one.
        let mut h = History::default();
        h.record(at("a", 0));
        h.record(at("gone", 0));
        h.back(Some(at("c", 0)));
        h.back(None);
        assert_eq!(h.index(), 0);
        assert_eq!(head(h.forward(None)), s("gone", 0));
        h.skip(Dir::Forward);
        assert_eq!(head(h.forward(None)), s("c", 0));
        assert_eq!(heads(&h), [s("a", 0), s("c", 0)].map(Option::unwrap));
        // Nothing to skip while live.
        let mut h = History::default();
        h.record(at("a", 0));
        h.skip(Dir::Back);
        assert_eq!(h.entries().len(), 1);
    }

    #[test]
    fn positions_follow_edits_to_their_file() {
        let mut h = History::default();
        h.record(at("a", 10));
        h.record(at("b", 10));
        let mut sel = at("a", 20);
        sel.pos.as_mut().unwrap().anchor = ByteIdx(15);
        sel.pos.as_mut().unwrap().top = ByteIdx(12);
        h.record(sel);
        // Insert 5 bytes at 0, then delete 2..4 (in the text after the first).
        let insert = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(0),
            text: "hello".into(),
        }]));
        let delete = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(2),
            end: ByteIdx(4),
            text: String::new(),
        }]));
        h.map(Path::new("a"), &[insert, delete]);
        let e = h.entries();
        assert_eq!(e[0].pos.unwrap().head, ByteIdx(13));
        assert_eq!(e[1].pos.unwrap().head, ByteIdx(10), "another file");
        let p = e[2].pos.unwrap();
        assert_eq!((p.anchor, p.head, p.top), (ByteIdx(18), ByteIdx(23), ByteIdx(15)));
    }
}
