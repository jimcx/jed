//! A set of replacements applied to a buffer as one unit.

use ropey::Rope;

use super::pos::ByteIdx;

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
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for c in &self.changes {
            let removed = rope.slice(c.start.0..c.end.0).to_string();
            let start = (c.start.0 as isize + delta) as usize;
            inverse.push(Change {
                start: ByteIdx(start),
                end: ByteIdx(start + c.text.len()),
                text: removed,
            });
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

    /// Where a pre-edit position ends up after the edit.
    pub fn map(&self, pos: ByteIdx, assoc: Assoc) -> ByteIdx {
        let mut delta: isize = 0;
        for c in &self.changes {
            let shift = |p: usize| ByteIdx((p as isize + delta) as usize);
            if pos < c.start {
                return shift(pos.0);
            }
            if pos < c.end || (pos == c.start && c.start == c.end) {
                // Inside a replaced range, or at a pure insertion point.
                return match assoc {
                    Assoc::Before => shift(c.start.0),
                    Assoc::After => shift(c.start.0 + c.text.len()),
                };
            }
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
        }
        ByteIdx((pos.0 as isize + delta) as usize)
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
}
