//! Forward walkers over sorted data for the editor's per-cell overlays (syntax tokens, selections,
//! find matches, squiggles, semantic tokens, merge conflicts, bracket marks, secondary cursors).
//! The editor visits bytes in increasing order, so one index per overlay replaces a search per
//! cell: O(items + cells) for a frame, however many selections or matches there are.
//!
//! A query that goes back (a continuation row asking for its line's start again) is answered
//! correctly as long as the item the index sits at still covers it: the index only moves past
//! items that end before a query.

use crate::buffer::pos::ByteIdx;

/// Items sorted by start and non-overlapping, each covering `start..end` (half open).
pub struct SortedWalk<'a, T, S, E> {
    items: &'a [T],
    start: S,
    end: E,
    i: usize,
}

impl<'a, T, S: Fn(&T) -> ByteIdx, E: Fn(&T) -> ByteIdx> SortedWalk<'a, T, S, E> {
    pub fn new(items: &'a [T], start: S, end: E) -> Self {
        SortedWalk {
            items,
            start,
            end,
            i: 0,
        }
    }

    /// The item covering `b`: advances past the items ending at or before it, then returns the next
    /// one if it starts at or before `b`. An empty item (`start == end`) covers nothing here; use
    /// `PointWalk` for points.
    pub fn at(&mut self, b: ByteIdx) -> Option<&'a T> {
        while self.i < self.items.len() && (self.end)(&self.items[self.i]) <= b {
            self.i += 1;
        }
        self.items.get(self.i).filter(|t| (self.start)(t) <= b)
    }
}

/// Items sorted by a byte position (matched brackets, selection heads), asked whether one is at a byte.
pub struct PointWalk<'a, T, K> {
    items: &'a [T],
    key: K,
    i: usize,
}

impl<'a, T, K: Fn(&T) -> ByteIdx> PointWalk<'a, T, K> {
    pub fn new(items: &'a [T], key: K) -> Self {
        PointWalk { items, key, i: 0 }
    }

    /// Whether an item sits at `b`: advances past the items before it.
    pub fn at(&mut self, b: ByteIdx) -> bool {
        while self.i < self.items.len() && (self.key)(&self.items[self.i]) < b {
            self.i += 1;
        }
        self.items.get(self.i).is_some_and(|t| (self.key)(t) == b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = (usize, usize);

    fn ranges(v: &[R]) -> SortedWalk<'_, R, impl Fn(&R) -> ByteIdx, impl Fn(&R) -> ByteIdx> {
        SortedWalk::new(v, |r| ByteIdx(r.0), |r| ByteIdx(r.1))
    }

    fn points(v: &[ByteIdx]) -> PointWalk<'_, ByteIdx, impl Fn(&ByteIdx) -> ByteIdx> {
        PointWalk::new(v, |x| *x)
    }

    #[test]
    fn empty_input_finds_nothing() {
        let mut w = ranges(&[]);
        assert!(w.at(ByteIdx(0)).is_none());
        assert!(w.at(ByteIdx(10)).is_none());
        assert!(!points(&[]).at(ByteIdx(3)));
    }

    #[test]
    fn ranges_are_half_open_and_touching_ranges_hand_over() {
        let v = [(2, 4), (4, 6), (7, 8)];
        let mut w = ranges(&v);
        let got: Vec<Option<usize>> = (0..9).map(|n| w.at(ByteIdx(n)).map(|r| r.0)).collect();
        assert_eq!(
            got,
            [None, None, Some(2), Some(2), Some(4), Some(4), None, Some(7), None]
        );
    }

    #[test]
    fn an_empty_range_covers_nothing_but_a_point_walk_finds_a_point() {
        let v = [(3, 3)];
        let mut w = ranges(&v);
        assert!(w.at(ByteIdx(2)).is_none());
        assert!(w.at(ByteIdx(3)).is_none());
        let pts = [ByteIdx(1), ByteIdx(3), ByteIdx(7)];
        let mut p = points(&pts);
        let got: Vec<bool> = (0..9).map(|n| p.at(ByteIdx(n))).collect();
        assert_eq!(got, [false, true, false, true, false, false, false, true, false]);
    }

    #[test]
    fn a_query_past_the_end_finds_nothing_and_stays_there() {
        let v = [(0, 2), (3, 5)];
        let mut w = ranges(&v);
        assert!(w.at(ByteIdx(100)).is_none());
        assert!(w.at(ByteIdx(101)).is_none());
        assert!(!points(&[ByteIdx(1)]).at(ByteIdx(9)));
    }

    #[test]
    fn a_query_can_skip_several_items() {
        let v = [(0, 1), (2, 3), (4, 5), (6, 9), (10, 11)];
        let mut w = ranges(&v);
        assert_eq!(w.at(ByteIdx(7)), Some(&(6, 9)));
        assert_eq!(w.at(ByteIdx(10)), Some(&(10, 11)));
        let pts = [ByteIdx(1), ByteIdx(2), ByteIdx(3), ByteIdx(8)];
        assert!(points(&pts).at(ByteIdx(8)));
    }

    #[test]
    fn the_same_byte_twice_and_a_step_back_within_an_item() {
        let v = [(2, 8)];
        let mut w = ranges(&v);
        assert!(w.at(ByteIdx(5)).is_some());
        assert!(w.at(ByteIdx(5)).is_some());
        assert!(w.at(ByteIdx(3)).is_some(), "back inside the same item");
        let pts = [ByteIdx(4)];
        let mut p = points(&pts);
        assert!(p.at(ByteIdx(4)));
        assert!(p.at(ByteIdx(4)));
    }
}
