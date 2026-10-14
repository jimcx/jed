//! A set of replacements applied to a buffer as one unit.

use std::cell::Cell;

use ropey::{Rope, RopeBuilder};

use super::pos::ByteIdx;

/// From this many changes, `apply` rebuilds the rope instead of editing it in place.
const REBUILD_AT: usize = 256;

/// Replace `start..end` (in the text before the edit) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub start: ByteIdx,
    pub end: ByteIdx,
    pub text: String,
}

/// Non-overlapping changes, sorted by position, all in pre-edit coordinates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    changes: Vec<Change>,
}

/// Which side of an insertion at the same position a mapped position ends up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    /// Not used by the editor yet (other views keep cursors before inserts).
    #[cfg_attr(not(test), allow(dead_code))]
    After,
}

impl ChangeSet {
    pub fn new(changes: Vec<Change>) -> Self {
        let changes: Vec<Change> = changes
            .into_iter()
            .filter(|c| c.start != c.end || !c.text.is_empty())
            .collect();
        debug_assert!(
            changes.windows(2).all(|w| w[0].end <= w[1].start),
            "changes must be sorted and non-overlapping"
        );
        Self { changes }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Applies the changes and returns the inverse set.
    pub fn apply(&self, rope: &mut Rope) -> ChangeSet {
        if self.changes.len() >= REBUILD_AT {
            return self.rebuild(rope);
        }
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for c in &self.changes {
            let removed = rope.slice(c.start.0..c.end.0).to_string();
            inverse.push(self.inverse_of(c, delta, removed));
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
        }
        // Last to first, so earlier offsets stay valid.
        for c in self.changes.iter().rev() {
            if c.end > c.start {
                rope.remove(c.start.0..c.end.0);
            }
            if !c.text.is_empty() {
                rope.insert(c.start.0, &c.text);
            }
        }
        ChangeSet { changes: inverse }
    }

    fn inverse_of(&self, c: &Change, delta: isize, removed: String) -> Change {
        let start = (c.start.0 as isize + delta) as usize;
        Change {
            start: ByteIdx(start),
            end: ByteIdx(start + c.text.len()),
            text: removed,
        }
    }

    /// `apply` for thousands of changes (an edit at every Cmd+Shift+L selection): one pass over
    /// the text builds the new rope and collects the removed text, instead of a tree edit and a
    /// slice lookup per change.
    fn rebuild(&self, rope: &mut Rope) -> ChangeSet {
        let mut b = RopeBuilder::new();
        let mut text = Walk::new(rope);
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for c in &self.changes {
            text.take(c.start.0, |s| b.append(s));
            let mut removed = String::new();
            text.take(c.end.0, |s| removed.push_str(s));
            b.append(&c.text);
            inverse.push(self.inverse_of(c, delta, removed));
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
        }
        text.take(rope.len(), |s| b.append(s));
        *rope = b.finish();
        ChangeSet { changes: inverse }
    }

    /// Where a pre-edit position ends up after the edit. O(changes); to map many positions,
    /// build a `mapper` once.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn map(&self, pos: ByteIdx, assoc: Assoc) -> ByteIdx {
        self.mapper().map(pos, assoc)
    }

    /// Maps positions in O(log changes) each, after an O(changes) setup: other views with
    /// thousands of cursors follow an edit at thousands of cursors without going quadratic.
    pub fn mapper(&self) -> Mapper<'_> {
        let mut deltas = Vec::with_capacity(self.changes.len() + 1);
        let mut delta: isize = 0;
        deltas.push(0);
        for c in &self.changes {
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
            deltas.push(delta);
        }
        Mapper {
            changes: &self.changes,
            deltas,
            hint: Cell::new(0),
        }
    }
}

/// Reads a rope front to back in pieces, borrowing its chunks.
struct Walk<'a> {
    chunks: ropey::iter::Chunks<'a>,
    /// The rest of the current chunk, starting at byte `at` of the rope.
    rest: &'a str,
    at: usize,
}

impl<'a> Walk<'a> {
    fn new(rope: &'a Rope) -> Self {
        Self {
            chunks: rope.chunks(),
            rest: "",
            at: 0,
        }
    }

    /// Hands over the text from the current position up to byte `to` (a char boundary).
    fn take(&mut self, to: usize, mut f: impl FnMut(&str)) {
        while self.at < to {
            if self.rest.is_empty() {
                match self.chunks.next() {
                    Some(c) => self.rest = c,
                    None => return,
                }
            }
            let n = (to - self.at).min(self.rest.len());
            let (head, tail) = self.rest.split_at(n);
            f(head);
            self.rest = tail;
            self.at += n;
        }
    }
}

/// See `ChangeSet::mapper`.
pub struct Mapper<'a> {
    changes: &'a [Change],
    /// `deltas[i]`: total length change of the changes before `i`.
    deltas: Vec<isize>,
    /// Where the last lookup ended: positions mapped in order (a sorted selection list) are
    /// found by stepping forward from here instead of searching.
    hint: Cell<usize>,
}

impl Mapper<'_> {
    pub fn map(&self, pos: ByteIdx, assoc: Assoc) -> ByteIdx {
        // The first change that isn't wholly before `pos`: one ending after it, or a pure
        // insertion exactly at it. Ends are sorted, so this is a binary search.
        let before = |c: &Change| !(c.end > pos || (c.start == pos && c.end == pos));
        let h = self.hint.get();
        let i = if h == 0 || before(&self.changes[h - 1]) {
            let mut i = h;
            while i < self.changes.len() && i < h + 8 && before(&self.changes[i]) {
                i += 1;
            }
            if i == h + 8 {
                i += self.changes[i..].partition_point(before);
            }
            i
        } else {
            self.changes.partition_point(before)
        };
        self.hint.set(i);
        let delta = self.deltas[i];
        let shift = |p: usize| ByteIdx((p as isize + delta) as usize);
        match self.changes.get(i) {
            Some(c) if pos >= c.start => match assoc {
                // Inside a replaced range, or at a pure insertion point.
                Assoc::Before => shift(c.start.0),
                Assoc::After => shift(c.start.0 + c.text.len()),
            },
            _ => shift(pos.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(start: usize, end: usize, text: &str) -> Change {
        Change {
            start: ByteIdx(start),
            end: ByteIdx(end),
            text: text.into(),
        }
    }

    #[test]
    fn apply_and_invert() {
        let mut r = Rope::from_str("hello world");
        let cs = ChangeSet::new(vec![ch(0, 5, "bye"), ch(6, 6, "big "), ch(11, 11, "!")]);
        let inv = cs.apply(&mut r);
        assert_eq!(r.to_string(), "bye big world!");
        inv.apply(&mut r);
        assert_eq!(r.to_string(), "hello world");
    }

    /// Many changes take the rebuild path; it gives the same text and inverse as small edits.
    #[test]
    fn large_sets_rebuild_the_same_text() {
        // 7-byte lines with a 2-byte char; each change starts a line and removes 0 to 2 bytes.
        let text = "abcdé\n".repeat(2000);
        let changes: Vec<Change> = (0..REBUILD_AT * 2)
            .map(|i| {
                let at = i * 7;
                ch(at, at + i % 3, if i % 2 == 0 { "XY" } else { "" })
            })
            .collect();
        let mut expected = text.clone();
        for c in changes.iter().rev() {
            expected.replace_range(c.start.0..c.end.0, &c.text);
        }
        let mut r = Rope::from_str(&text);
        let inv = ChangeSet::new(changes).apply(&mut r);
        assert_eq!(r.to_string(), expected);
        inv.apply(&mut r);
        assert_eq!(r.to_string(), text);
    }

    #[test]
    fn map_positions() {
        let cs = ChangeSet::new(vec![ch(2, 4, "x"), ch(6, 6, "yy")]);
        assert_eq!(cs.map(ByteIdx(1), Assoc::After), ByteIdx(1));
        assert_eq!(cs.map(ByteIdx(3), Assoc::Before), ByteIdx(2));
        assert_eq!(cs.map(ByteIdx(3), Assoc::After), ByteIdx(3));
        assert_eq!(cs.map(ByteIdx(5), Assoc::After), ByteIdx(4));
        assert_eq!(cs.map(ByteIdx(6), Assoc::Before), ByteIdx(5));
        assert_eq!(cs.map(ByteIdx(6), Assoc::After), ByteIdx(7));
        assert_eq!(cs.map(ByteIdx(8), Assoc::After), ByteIdx(9));
    }

    /// The pre-`mapper` implementation of `map`: a walk over every change.
    fn linear(cs: &[Change], pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for c in cs {
            let shift = |p: usize| (p as isize + delta) as usize;
            if pos < c.start.0 {
                return shift(pos);
            }
            if pos < c.end.0 || (pos == c.start.0 && c.start == c.end) {
                return match assoc {
                    Assoc::Before => shift(c.start.0),
                    Assoc::After => shift(c.start.0 + c.text.len()),
                };
            }
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
        }
        (pos as isize + delta) as usize
    }

    /// The binary-search mapper agrees with a straightforward walk over the changes.
    #[test]
    fn mapper_matches_linear_walk() {
        // Deletions, insertions (also right after a deletion) and replacements.
        let changes = vec![
            ch(1, 3, ""),
            ch(3, 3, "abc"),
            ch(5, 9, "z"),
            ch(12, 12, "q"),
            ch(14, 20, ""),
        ];
        let cs = ChangeSet::new(changes.clone());
        let m = cs.mapper();
        // In order (the hint path), backwards and jumping about (the search path).
        let order = (0..25).chain((0..25).rev()).chain((0..25).map(|i| (i * 7) % 25));
        for pos in order {
            for assoc in [Assoc::Before, Assoc::After] {
                assert_eq!(
                    m.map(ByteIdx(pos), assoc).0,
                    linear(&changes, pos, assoc),
                    "{pos} {assoc:?}"
                );
            }
        }
    }

    /// Seeded xorshift: `next(n)` is in `0..n`.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Random text over several rope chunks (multi-byte chars, so chunk ends fall inside runs of
    /// them) and a random sorted change set on char boundaries: replacements, deletions,
    /// insertions, several insertions at one position, changes touching each other.
    fn random_case(rng: &mut Rng, n_changes: usize) -> (String, Vec<Change>) {
        const PIECES: &[&str] = &["a", "bc", "é", "界", "😀", "\n", "\r\n", "xyz"];
        let mut text = String::new();
        while text.len() < n_changes * 12 + 3000 {
            text.push_str(PIECES[rng.next(PIECES.len())]);
        }
        let bounds: Vec<usize> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
        let mut changes = Vec::new();
        let mut at = 0; // index into `bounds`
        for _ in 0..n_changes {
            at += rng.next(8);
            if at >= bounds.len() {
                break;
            }
            let end = (at + rng.next(4)).min(bounds.len() - 1);
            let ins = ["", "Q", "é界", "\n"][rng.next(4)];
            let (s, e) = (bounds[at], bounds[end]);
            if s == e && ins.is_empty() {
                continue; // dropped by `ChangeSet::new` anyway
            }
            changes.push(ch(s, e, ins));
            at = end;
        }
        (text, changes)
    }

    fn reference_apply(text: &str, changes: &[Change]) -> String {
        let mut out = text.to_string();
        for c in changes.iter().rev() {
            out.replace_range(c.start.0..c.end.0, &c.text);
        }
        out
    }

    /// Both `apply` paths (tree edits below `REBUILD_AT`, the one-pass rebuild from it) give
    /// the reference text and an inverse that restores the original, and agree on the inverse;
    /// the mapper agrees with the linear walk for positions in order and out of order.
    #[test]
    fn random_change_sets_agree_with_references() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let sizes = [1, 2, 9, 40, REBUILD_AT - 1, REBUILD_AT, REBUILD_AT + 1, 700];
        for round in 0..24 {
            let n = sizes[round % sizes.len()];
            let (text, changes) = random_case(&mut rng, n);
            let cs = ChangeSet::new(changes.clone());
            let expected = reference_apply(&text, &changes);

            let mut r = Rope::from_str(&text);
            let inv = cs.apply(&mut r);
            assert_eq!(r.to_string(), expected, "round {round}: {n} changes");
            // The rebuild directly, whatever the size: the same text and inverse.
            let mut r2 = Rope::from_str(&text);
            let inv2 = cs.rebuild(&mut r2);
            assert_eq!(r2.to_string(), expected, "round {round}: rebuild");
            assert_eq!(inv.changes, inv2.changes, "round {round}: inverse");
            inv.apply(&mut r);
            assert_eq!(r.to_string(), text, "round {round}: undo");

            let m = cs.mapper();
            let len = text.len();
            let ordered = (0..=len).step_by(1 + rng.next(3));
            let jumping = (0..400).map(|_| rng.next(len + 1));
            for pos in ordered.chain(jumping) {
                for assoc in [Assoc::Before, Assoc::After] {
                    assert_eq!(
                        m.map(ByteIdx(pos), assoc).0,
                        linear(&changes, pos, assoc),
                        "{pos} {assoc:?}"
                    );
                }
            }
        }
    }
}
