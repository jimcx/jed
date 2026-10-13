//! The selection list: every view holds one, a single cursor is a list of length 1.

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

    /// Overlapping or at the same position. Selections that only touch are kept apart.
    fn should_merge(&self, other: &Selection) -> bool {
        (self.start() < other.end() && other.start() < self.end())
            || (self.start() == other.start() && self.end() == other.end())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selections {
    list: Vec<Selection>,
    primary: usize,
}

impl Selections {
    pub fn single(sel: Selection) -> Self {
        Self {
            list: vec![sel],
            primary: 0,
        }
    }

    /// Sorts and merges. `primary` indexes into `list`.
    pub fn new(list: Vec<Selection>, primary: usize) -> Self {
        assert!(primary < list.len(), "selection list must not be empty");
        let mut s = Self { list, primary };
        s.normalize();
        s
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.list.iter()
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
        for sel in &mut self.list {
            *sel = f(sel);
        }
        self.normalize();
    }

    /// Keeps only the primary selection.
    pub fn keep_primary(&mut self) {
        self.list = vec![self.list[self.primary]];
        self.primary = 0;
    }

    fn normalize(&mut self) {
        let mut order: Vec<(Selection, bool)> = self
            .list
            .iter()
            .enumerate()
            .map(|(i, s)| (*s, i == self.primary))
            .collect();
        order.sort_by_key(|(s, _)| (s.start(), s.end()));

        let mut merged: Vec<Selection> = Vec::with_capacity(order.len());
        let mut primary_idx = 0;
        for (sel, is_primary) in order {
            match merged.last_mut() {
                Some(last) if last.should_merge(&sel) => {
                    let (start, end) = (last.start().min(sel.start()), last.end().max(sel.end()));
                    *last = if sel.is_backward() || (sel.is_empty() && last.is_backward()) {
                        Selection::new(end, start)
                    } else {
                        Selection::new(start, end)
                    };
                    if is_primary {
                        primary_idx = merged.len() - 1;
                    }
                }
                _ => {
                    merged.push(sel);
                    if is_primary {
                        primary_idx = merged.len() - 1;
                    }
                }
            }
        }
        self.list = merged;
        self.primary = primary_idx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, h: usize) -> Selection {
        Selection::new(ByteIdx(a), ByteIdx(h))
    }

    #[test]
    fn merges_overlapping_and_equal() {
        let s = Selections::new(vec![sel(5, 8), sel(0, 2), sel(6, 10), sel(1, 1)], 2);
        let got: Vec<_> = s.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, vec![(0, 2), (5, 10)]);
        assert_eq!(s.primary_index(), 1);

        let s = Selections::new(vec![sel(3, 3), sel(3, 3)], 1);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn touching_selections_stay_apart() {
        // Cmd+D on `a` in `aaaa` must give four selections.
        let s = Selections::new(vec![sel(0, 1), sel(1, 2), sel(2, 3), sel(3, 4)], 0);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn merge_keeps_backward_direction() {
        let s = Selections::new(vec![sel(4, 2), sel(6, 3)], 0);
        assert_eq!((s.primary().anchor.0, s.primary().head.0), (6, 2));
    }
}
