//! The selection list: every view holds one, a single cursor is a list of length 1.

use std::sync::Arc;

use crate::buffer::pos::{ByteIdx, DisplayCol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: ByteIdx,
    pub head: ByteIdx,
    /// Remembered column for up/down movement.
    pub goal: Option<DisplayCol>,
}

impl Selection {
    pub fn cursor(pos: ByteIdx) -> Self {
        Self {
            anchor: pos,
            head: pos,
            goal: None,
        }
    }

    pub fn new(anchor: ByteIdx, head: ByteIdx) -> Self {
        Self {
            anchor,
            head,
            goal: None,
        }
    }

    pub fn start(&self) -> ByteIdx {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> ByteIdx {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn is_backward(&self) -> bool {
        self.head < self.anchor
    }

    /// VS Code's rule: overlapping, or touching when either is an empty cursor. Two selections
    /// that only touch are kept apart (Cmd+D on `a` in `aaaa` gives four).
    fn should_merge(&self, other: &Selection) -> bool {
        if self.is_empty() || other.is_empty() {
            self.start() <= other.end() && other.start() <= self.end()
        } else {
            self.start() < other.end() && other.start() < self.end()
        }
    }
}

/// Sorted by position and merged. Besides the primary it remembers the *newest* selection, the
/// one added last (VS Code keeps its cursors in the order they were added; Cmd+D searches after
/// the last one and Cmd+Alt+D replaces it).
///
/// The list is shared on clone (copied on the next change): undo steps, the Cmd+U history and
/// edits all keep copies of lists that can hold 60k selections, and comparing two copies of one
/// list is a pointer check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selections {
    list: Arc<Vec<Selection>>,
    primary: usize,
    newest: usize,
}

impl Selections {
    pub fn single(sel: Selection) -> Self {
        Self {
            list: Arc::new(vec![sel]),
            primary: 0,
            newest: 0,
        }
    }

    /// Sorts and merges. `primary` indexes into `list`; the last entry of `list` is the newest.
    pub fn new(list: Vec<Selection>, primary: usize) -> Self {
        let newest = list.len().saturating_sub(1);
        Self::with_newest(list, primary, newest)
    }

    /// `new`, naming the newest selection too (both index into `list`).
    pub fn with_newest(list: Vec<Selection>, primary: usize, newest: usize) -> Self {
        assert!(primary < list.len(), "selection list must not be empty");
        assert!(newest < list.len());
        let mut s = Self {
            list: Arc::new(list),
            primary,
            newest,
        };
        s.normalize();
        s
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.list.iter()
    }

    /// The selections, sorted.
    pub fn as_slice(&self) -> &[Selection] {
        &self.list
    }

    /// The selections that end at or after `from` and start at or before `to`, e.g. the visible
    /// ones (binary search: rendering doesn't walk every selection).
    pub fn overlapping(&self, from: ByteIdx, to: ByteIdx) -> &[Selection] {
        // Merged selections don't overlap, so both starts and ends are sorted.
        let first = self.list.partition_point(|s| s.end() < from);
        let last = self.list.partition_point(|s| s.start() <= to);
        &self.list[first..last.max(first)]
    }

    /// Whether exactly `start..end` is one of the selections (either direction).
    pub fn contains_range(&self, start: ByteIdx, end: ByteIdx) -> bool {
        self.list
            .binary_search_by_key(&(start, end), |s| (s.start(), s.end()))
            .is_ok()
    }

    pub fn newest(&self) -> &Selection {
        &self.list[self.newest]
    }

    pub fn newest_index(&self) -> usize {
        self.newest
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn primary(&self) -> &Selection {
        &self.list[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// Replaces each selection, then sorts and merges.
    pub fn transform(&mut self, mut f: impl FnMut(&Selection) -> Selection) {
        for sel in Arc::make_mut(&mut self.list) {
            *sel = f(sel);
        }
        self.normalize();
    }

    /// Keeps only the primary selection.
    pub fn keep_primary(&mut self) {
        self.list = Arc::new(vec![self.list[self.primary]]);
        self.primary = 0;
        self.newest = 0;
    }

    /// Sorts (stably; skipped when already sorted, as after mapping or an edit) and merges in
    /// place, keeping `primary` and `newest` on the selections they named.
    fn normalize(&mut self) {
        let list = Arc::make_mut(&mut self.list);
        let key = |s: &Selection| (s.start(), s.end());
        if !list.is_sorted_by_key(key) {
            let mut order: Vec<(Selection, usize)> = list.iter().copied().zip(0..).collect();
            order.sort_by_key(|(s, _)| key(s));
            let (mut primary, mut newest) = (0, 0);
            for (k, (sel, i)) in order.into_iter().enumerate() {
                list[k] = sel;
                if i == self.primary {
                    primary = k;
                }
                if i == self.newest {
                    newest = k;
                }
            }
            (self.primary, self.newest) = (primary, newest);
        }

        let (mut primary, mut newest) = (0, 0);
        let mut w: usize = 0;
        for r in 0..list.len() {
            let sel = list[r];
            match w.checked_sub(1).map(|l| &mut list[l]) {
                Some(last) if last.should_merge(&sel) => {
                    let (start, end) = (last.start().min(sel.start()), last.end().max(sel.end()));
                    *last = if sel.is_backward() || (sel.is_empty() && last.is_backward()) {
                        Selection::new(end, start)
                    } else {
                        Selection::new(start, end)
                    };
                }
                _ => {
                    list[w] = sel;
                    w += 1;
                }
            }
            if r == self.primary {
                primary = w - 1;
            }
            if r == self.newest {
                newest = w - 1;
            }
        }
        list.truncate(w);
        (self.primary, self.newest) = (primary, newest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, h: usize) -> Selection {
        Selection::new(ByteIdx(a), ByteIdx(h))
    }

    fn ranges(s: &Selections) -> Vec<(usize, usize)> {
        s.iter().map(|s| (s.anchor.0, s.head.0)).collect()
    }

    #[test]
    fn merges_overlapping_and_equal() {
        let s = Selections::new(vec![sel(5, 8), sel(0, 2), sel(6, 10), sel(1, 1)], 2);
        let got: Vec<_> = s.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(0, 2), (5, 10)]);
        assert_eq!(s.primary_index(), 1);

        let s = Selections::new(vec![sel(3, 3), sel(3, 3)], 1);
        assert_eq!(s.len(), 1);

        // A cursor touching a selection joins it (as VS Code); two touching cursors can't exist.
        let s = Selections::new(vec![sel(3, 3), sel(3, 6), sel(9, 6), sel(9, 9)], 0);
        let got: Vec<_> = s.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(3, 6), (9, 6)]);
    }

    #[test]
    fn touching_selections_stay_apart() {
        // Cmd+D on `a` in `aaaa` must give four selections.
        let s = Selections::new(vec![sel(0, 1), sel(1, 2), sel(2, 3), sel(3, 4)], 0);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn newest_and_primary_follow_sorting_and_merging() {
        // Given in the order added: primary first, newest last.
        let s = Selections::new(vec![sel(6, 8), sel(0, 2), sel(3, 4)], 0);
        assert_eq!((s.primary().start().0, s.newest().start().0), (6, 3));
        assert_eq!((s.primary_index(), s.newest_index()), (2, 1));
        // The newest merges into another selection: that one is newest now.
        let s = Selections::with_newest(vec![sel(0, 4), sel(8, 9), sel(2, 6)], 1, 2);
        assert_eq!((s.newest().start().0, s.newest().end().0), (0, 6));
        assert_eq!(s.primary().start().0, 8);
    }

    #[test]
    fn overlapping_finds_only_the_range() {
        let s = Selections::new((0..100).map(|i| sel(i * 10, i * 10 + 3)).collect(), 0);
        let got: Vec<usize> = s
            .overlapping(ByteIdx(13), ByteIdx(40))
            .iter()
            .map(|s| s.start().0)
            .collect();
        assert_eq!(got, vec![10, 20, 30, 40]);
        assert!(s.overlapping(ByteIdx(4), ByteIdx(9)).is_empty());
        assert_eq!(s.overlapping(ByteIdx(995), ByteIdx(2000)).len(), 0);
        assert!(s.contains_range(ByteIdx(20), ByteIdx(23)));
        assert!(!s.contains_range(ByteIdx(20), ByteIdx(22)));
    }

    /// Clones share the list until one changes: the other keeps its selections.
    #[test]
    fn clones_are_independent() {
        let a = Selections::new(vec![sel(0, 2), sel(5, 8)], 1);
        let mut b = a.clone();
        b.transform(|s| Selection::new(s.anchor, ByteIdx(s.head.0 + 4)));
        assert_eq!(ranges(&a), vec![(0, 2), (5, 8)]);
        assert_eq!(ranges(&b), vec![(0, 12)], "transformed, then merged");
        assert_eq!(a, a.clone());
        assert_ne!(a, b);
    }

    /// Already sorted lists skip the sort but still merge, keeping primary and newest.
    #[test]
    fn sorted_lists_still_merge() {
        let s = Selections::with_newest(vec![sel(0, 0), sel(0, 0), sel(3, 3), sel(3, 5), sel(9, 9)], 3, 1);
        assert_eq!(ranges(&s), vec![(0, 0), (3, 5), (9, 9)]);
        assert_eq!((s.primary_index(), s.newest_index()), (1, 0));
    }

    #[test]
    fn merge_keeps_backward_direction() {
        let s = Selections::new(vec![sel(4, 2), sel(6, 3)], 0);
        assert_eq!((s.primary().anchor.0, s.primary().head.0), (6, 2));
    }
}
