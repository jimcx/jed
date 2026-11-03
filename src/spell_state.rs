//! One buffer's spell-check bookkeeping: which parts of the text changed since they were last
//! checked (byte ranges in the current text, kept there by `Buffer::apply` through `map`), when
//! the last edit was, and what a check in flight is for. The app's side is `app/spell.rs`.

use std::ops::Range;
use std::time::Instant;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::ByteIdx;

/// More changed ranges than this collapse into one covering them all.
const MAX_RANGES: usize = 64;

#[derive(Debug)]
pub struct SpellState {
    /// "Spelling: Toggle Spell Check" turns it off for this buffer (it reverts at close).
    pub enabled: bool,
    /// The whole text needs checking (open, a settings or dictionary change, a dropped result).
    pub full: bool,
    /// The app's check generation this buffer was last checked under.
    pub generation: u64,
    /// Changed ranges, sorted, not overlapping, not yet checked.
    dirty: Vec<Range<ByteIdx>>,
    /// When the buffer was last edited.
    pub edited_at: Option<Instant>,
    /// The revision a background check was started for.
    pub running: Option<u64>,
    /// Words held back while the caret was in them (the half-typed word rule): re-checked
    /// once it leaves. Mapped through edits like `dirty`.
    pub held: Vec<Range<ByteIdx>>,
    /// Spell checking doesn't apply to this buffer (settings, size, language, toggled off).
    pub off: bool,
    /// "Spelling: Ignore Word in This File": lower-case words accepted until the buffer closes.
    pub ignored: std::collections::HashSet<String>,
    /// Whether the buffer has spell marks (so turning off knows to clear them).
    pub marked: bool,
}

impl Default for SpellState {
    fn default() -> Self {
        SpellState {
            enabled: true,
            full: true,
            generation: 0,
            dirty: Vec::new(),
            edited_at: None,
            running: None,
            held: Vec::new(),
            off: false,
            ignored: Default::default(),
            marked: false,
        }
    }
}

impl SpellState {
    /// Follows a change set (`Buffer::apply`): old ranges move with the text, and each change's
    /// new text is dirty.
    pub fn map(&mut self, changes: &ChangeSet) {
        if changes.is_empty() {
            return;
        }
        self.edited_at = Some(Instant::now());
        // Many selections (Cmd+Shift+L): more ranges than are kept anyway. Not O(n log n) per edit.
        if changes.len() > MAX_RANGES {
            self.want_full();
        }
        if self.full {
            return;
        }
        let mapper = changes.mapper();
        for r in self.dirty.iter_mut().chain(self.held.iter_mut()) {
            r.start = mapper.map(r.start, Assoc::Before);
            r.end = mapper.map(r.end, Assoc::After).max(r.start);
        }
        let mut delta = 0isize;
        for (start, end, len) in changes.iter() {
            let s = (start.0 as isize + delta) as usize;
            self.dirty.push(ByteIdx(s)..ByteIdx(s + len));
            delta += len as isize - (end.0 - start.0) as isize;
        }
        self.normalize();
    }

    fn normalize(&mut self) {
        self.dirty.sort_by_key(|r| (r.start, r.end));
        let mut out: Vec<Range<ByteIdx>> = Vec::with_capacity(self.dirty.len());
        for r in self.dirty.drain(..) {
            match out.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => out.push(r),
            }
        }
        if out.len() > MAX_RANGES {
            let all = out[0].start..out[out.len() - 1].end;
            out = vec![all];
        }
        self.dirty = out;
    }

    /// Marks a range as needing a check.
    pub fn touch(&mut self, range: Range<ByteIdx>) {
        if !self.full {
            self.dirty.push(range);
            self.normalize();
        }
    }

    #[cfg(test)]
    pub fn dirty(&self) -> &[Range<ByteIdx>] {
        &self.dirty
    }

    /// Takes the changed ranges.
    pub fn take_dirty(&mut self) -> Vec<Range<ByteIdx>> {
        std::mem::take(&mut self.dirty)
    }

    /// Whether anything needs checking.
    pub fn wants(&self) -> bool {
        self.full || !self.dirty.is_empty()
    }

    /// The whole text is to be checked: the dirty ranges are moot.
    pub fn want_full(&mut self) {
        self.full = true;
        self.dirty.clear();
        self.held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    fn set(start: usize, end: usize, text: &str) -> ChangeSet {
        ChangeSet::new(vec![Change {
            start: ByteIdx(start),
            end: ByteIdx(end),
            text: text.into(),
        }])
    }

    fn idle() -> SpellState {
        SpellState {
            full: false,
            ..Default::default()
        }
    }

    #[test]
    fn spell_check_dirty_ranges_follow_edits() {
        let mut s = idle();
        s.map(&set(10, 10, "abc"));
        assert_eq!(s.dirty(), [ByteIdx(10)..ByteIdx(13)]);
        s.map(&set(0, 0, "zz"));
        assert_eq!(s.dirty(), [ByteIdx(0)..ByteIdx(2), ByteIdx(12)..ByteIdx(15)]);
        s.map(&set(1, 13, ""));
        assert_eq!(s.dirty(), [ByteIdx(0)..ByteIdx(3)]);
    }

    #[test]
    fn spell_check_many_dirty_ranges_collapse() {
        let mut s = idle();
        for i in 0..100 {
            s.map(&set(i * 10, i * 10, "x"));
        }
        assert!(s.dirty().len() <= MAX_RANGES);
    }
}
