//! Merge conflicts: git's conflict markers in a buffer's text, the blocks between them and the
//! edit that accepts one side. No drawing (the editor draws the blocks and the action row) and no
//! app state. See [docs/dev/modules.md](../docs/dev/modules.md)'s data flow for where a scan
//! runs.
//!
//! The scan is a line scan of the rope (never the tree): `<<<<<<<` … (`|||||||` base …)
//! `=======` … `>>>>>>>` at line starts. A marker line that is never closed, or that has another
//! `<<<<<<<` before its `>>>>>>>`, is not a conflict. The result is kept on the buffer
//! ([`Conflicts`], `buf.conflicts`) and cached by revision; every change set applied invalidates
//! it (`map`, called by `Buffer::apply`), so the next `scan` rescans the text.

use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use ropey::Rope;

use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx};

const START: &str = "<<<<<<<";
const BASE: &str = "|||||||";
const MIDDLE: &str = "=======";
const END: &str = ">>>>>>>";

/// One conflict: where it is and the text between its markers, in the buffer's coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The `<<<<<<<` line's start.
    pub start: ByteIdx,
    /// The start of the line after the `>>>>>>>` line (the text's end when that line has no
    /// line break).
    pub end: ByteIdx,
    /// The current side: from the line after `<<<<<<<` to the `|||||||` / `=======` line.
    pub current: Range<ByteIdx>,
    /// The base, from the line after `|||||||` to the `=======` line (three-way markers only).
    pub base: Option<Range<ByteIdx>>,
    /// The incoming side: from the line after `=======` to the `>>>>>>>` line.
    pub incoming: Range<ByteIdx>,
}

/// What an accept keeps of a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Current,
    Incoming,
    /// Current, then incoming, each with its own line breaks.
    Both,
    /// The base (three-way markers); a two-way conflict has none, so this removes it (VS Code's
    /// "Accept None").
    Base,
}

/// A buffer's conflicts and the revision they were scanned at.
#[derive(Debug, Default)]
pub struct Conflicts {
    /// Sorted by `start`, in the buffer's current coordinates.
    conflicts: Vec<Conflict>,
    /// The revision the scan was for; `None` before the first scan and after an edit.
    computed: Option<u64>,
}

impl Conflicts {
    /// The conflicts in `rope` at `revision`, scanning again when the revision changed (or an
    /// edit invalidated the cache).
    pub fn scan(&mut self, rope: &Rope, revision: u64) -> &[Conflict] {
        if self.computed != Some(revision) {
            self.conflicts = scan(rope);
            self.computed = Some(revision);
        }
        &self.conflicts
    }

    /// Follows a change set applied to the buffer (`Buffer::apply`): the text changed, so the
    /// next `scan` rescans (a scan is cheap).
    pub fn map(&mut self, _changes: &Arc<ChangeSet>) {
        self.computed = None;
    }

    /// Forgets the conflicts and the revision they were scanned at.
    #[cfg(test)]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The conflicts of the last `scan`.
    #[cfg(test)]
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// How many conflicts the last `scan` found (`None` before the first scan and after an
    /// edit, until the next one). The editor scans the active buffer every frame, so the status
    /// bar, drawn after it, reads the frame's count.
    pub fn count(&self) -> Option<usize> {
        self.computed.map(|_| self.conflicts.len())
    }

    /// The next conflict starting after `at`, wrapping around to the first; `None` without
    /// conflicts.
    pub fn next(&mut self, rope: &Rope, revision: u64, at: ByteIdx) -> Option<&Conflict> {
        let conflicts = self.scan(rope, revision);
        conflicts.iter().find(|c| c.start > at).or_else(|| conflicts.first())
    }

    /// The previous conflict ending at or before `at`, wrapping around to the last; `None`
    /// without conflicts.
    pub fn previous(&mut self, rope: &Rope, revision: u64, at: ByteIdx) -> Option<&Conflict> {
        let conflicts = self.scan(rope, revision);
        conflicts
            .iter()
            .rev()
            .find(|c| c.end <= at)
            .or_else(|| conflicts.last())
    }
}

/// The text `choice` keeps of `conflict`: the chosen block, markers left out (the base of a
/// two-way conflict is empty: accepting it removes the conflict).
pub fn block(rope: &Rope, conflict: &Conflict, choice: Choice) -> String {
    let block = |range: &Range<ByteIdx>| rope.slice(range.start.0..range.end.0).to_string();
    match choice {
        Choice::Current => block(&conflict.current),
        Choice::Incoming => block(&conflict.incoming),
        Choice::Both => {
            let mut text = block(&conflict.current);
            text.push_str(&block(&conflict.incoming));
            text
        }
        Choice::Base => match &conflict.base {
            Some(base) => block(base),
            None => String::new(),
        },
    }
}

/// The changes that accept `choice` of every conflict in `conflicts` (sorted and not
/// overlapping): each whole span (markers included) replaced with its chosen block's text, in
/// the text's coordinates before the edit. Applying them is one `EditKind::Other` undo step.
pub fn accept_all(rope: &Rope, conflicts: &[Conflict], choice: Choice) -> ChangeSet {
    let mut set = ChangeSet::with_capacity(conflicts.len(), 0);
    for conflict in conflicts {
        let text = block(rope, conflict, choice);
        set.push(conflict.start, conflict.end, &text);
    }
    set
}

/// The change that accepts `choice` of `conflict`; see [`accept_all`].
#[cfg_attr(not(test), allow(dead_code))] // the commands use `accept_all`
pub fn accept(rope: &Rope, conflict: &Conflict, choice: Choice) -> ChangeSet {
    accept_all(rope, std::slice::from_ref(conflict), choice)
}

/// What a marker line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    Base,
    Middle,
    End,
}

impl Kind {
    fn of(text: &str) -> Option<Self> {
        if text.starts_with(START) {
            Some(Kind::Start)
        } else if text.starts_with(BASE) {
            Some(Kind::Base)
        } else if text.starts_with(MIDDLE) {
            Some(Kind::Middle)
        } else if text.starts_with(END) {
            Some(Kind::End)
        } else {
            None
        }
    }
}

/// A marker line: what it is, its start, and the start of the line after it (the text's end for
/// the last line).
#[derive(Debug, Clone, Copy)]
struct Marker {
    kind: Kind,
    start: ByteIdx,
    after: ByteIdx,
}

/// Scans `rope` for conflicts: markers at line starts only. An unterminated `<<<<<<<`, or one
/// with another `<<<<<<<` before its `>>>>>>>`, isn't a conflict; the scan resumes after the
/// `>>>>>>>` that would have closed it.
fn scan(rope: &Rope) -> Vec<Conflict> {
    // The marker lines only: the text is visited without copying (a 1 MB file's lines are not
    // all collected), and `line_bounds` runs just for the few that match.
    let mut found: Vec<(usize, Kind)> = Vec::new();
    let mut line = 0;
    pos::each_line::<()>(rope, 0, false, |_, text| {
        if let Some(kind) = Kind::of(text) {
            found.push((line, kind));
        }
        line += 1;
        ControlFlow::Continue(())
    });
    let markers: Vec<Marker> = found
        .iter()
        .map(|&(line, kind)| {
            let (start, _, after) = pos::line_bounds(rope, line);
            Marker { kind, start, after }
        })
        .collect();

    let mut conflicts = Vec::new();
    let mut i = 0;
    while i < markers.len() {
        if markers[i].kind != Kind::Start {
            i += 1;
            continue;
        }
        let (mut base, mut middle, mut end) = (None, None, None);
        let mut nested = false;
        let mut j = i + 1;
        while j < markers.len() {
            match markers[j].kind {
                Kind::Start => {
                    nested = true;
                    break;
                }
                Kind::Base if base.is_none() && middle.is_none() => base = Some(j),
                Kind::Middle if middle.is_none() => middle = Some(j),
                Kind::End => {
                    end = Some(j);
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        if nested {
            // Both the outer and the `<<<<<<<` inside it are refused: carry on after the
            // `>>>>>>>` that would have closed the outer.
            i = markers[j..]
                .iter()
                .position(|m| m.kind == Kind::End)
                .map_or(markers.len(), |k| j + k + 1);
            continue;
        }
        match (middle, end) {
            (Some(middle_at), Some(end_at)) => {
                let (start, middle, end) = (markers[i], markers[middle_at], markers[end_at]);
                let current_end = base.map_or(middle.start, |b| markers[b].start);
                conflicts.push(Conflict {
                    start: start.start,
                    end: end.after,
                    current: start.after..current_end,
                    base: base.map(|b| markers[b].after..middle.start),
                    incoming: middle.after..end.start,
                });
                i = end_at + 1;
            }
            // A `>>>>>>>` with no `=======` before it: refused, carry on after it.
            (None, Some(end_at)) => i = end_at + 1,
            // No `>>>>>>>` from here on: nothing later can be closed either.
            (_, None) => break,
        }
    }
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    /// The conflict markers `git merge-file -p` writes for `base` / `ours` / `theirs`, with a
    /// `|||||||` base block when `diff3` (real git output, not hand-written markers).
    fn merge_file(base: &str, ours: &str, theirs: &str, diff3: bool) -> String {
        // `git merge-file` exits with the number of conflicts (1 here), so `git::test_repo::git`
        // (which asserts success) can't run it.
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [("ours", ours), ("base", base), ("theirs", theirs)] {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["merge-file", "-p"]);
        if diff3 {
            cmd.arg("--diff3");
        }
        let out = cmd.args(["ours", "base", "theirs"]).output().expect("git runs");
        assert_eq!(
            out.status.code(),
            Some(1),
            "one conflict: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn conflicts(text: &str) -> Vec<Conflict> {
        let rope = Rope::from_str(text);
        let mut conflicts = Conflicts::default();
        conflicts.scan(&rope, 0);
        conflicts.conflicts().to_vec()
    }

    fn slice(rope: &Rope, range: &Range<ByteIdx>) -> String {
        rope.slice(range.start.0..range.end.0).to_string()
    }

    /// The text left after accepting a choice of the first conflict (hand-checked against what
    /// `git checkout --ours` / `--theirs` gives for that hunk).
    fn accepted(text: &str, choice: Choice) -> String {
        let mut rope = Rope::from_str(text);
        let list = conflicts(text);
        let set = accept(&rope, &list[0], choice);
        set.apply(&mut rope);
        rope.to_string()
    }

    const TWO_CONFLICTS: &str = "a\n\
<<<<<<< HEAD\n\
b\n\
=======\n\
c\n\
>>>>>>> main\n\
d\n\
<<<<<<< HEAD\n\
e\n\
=======\n\
f\n\
>>>>>>> main\n\
g\n";

    #[test]
    fn a_two_way_merge_has_current_and_incoming_blocks() {
        let text = merge_file("a\nb\nc\n", "a\nB\nc\n", "a\nX\nc\n", false);
        assert_eq!(text, "a\n<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\nc\n");
        let rope = Rope::from_str(&text);
        let list = conflicts(&text);
        assert_eq!(list.len(), 1);
        let conflict = &list[0];
        assert_eq!(conflict.start, ByteIdx(2), "the <<<<<<< line's start");
        assert_eq!(
            slice(&rope, &(conflict.start..conflict.end)),
            "<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\n",
            "the whole span, markers included"
        );
        assert_eq!(slice(&rope, &conflict.current), "B\n");
        assert_eq!(slice(&rope, &conflict.incoming), "X\n");
        assert_eq!(conflict.base, None);
        assert_eq!(pos::line_of(&rope, conflict.start), 1);
        assert_eq!(
            pos::line_of(&rope, ByteIdx(conflict.end.0 - 1)),
            5,
            "its last byte is the >>>>>>> line's line break"
        );
        assert_eq!(
            accept(&rope, conflict, Choice::Current).len(),
            1,
            "an accept is one whole-span change"
        );
    }

    #[test]
    fn diff3_merges_have_a_base_block() {
        let text = merge_file("a\nb\nc\n", "a\nB\nc\n", "a\nX\nc\n", true);
        assert_eq!(
            text,
            "a\n<<<<<<< ours\nB\n||||||| base\nb\n=======\nX\n>>>>>>> theirs\nc\n"
        );
        let rope = Rope::from_str(&text);
        let list = conflicts(&text);
        assert_eq!(list.len(), 1);
        let base = list[0].base.as_ref().expect("a three-way conflict");
        assert_eq!(slice(&rope, base), "b\n");
    }

    #[test]
    fn accepting_a_choice_gives_what_checkout_ours_or_theirs_would() {
        let merged = merge_file("a\nb\nc\n", "a\nB\nc\n", "a\nX\nc\n", false);
        assert_eq!(accepted(&merged, Choice::Current), "a\nB\nc\n");
        assert_eq!(accepted(&merged, Choice::Incoming), "a\nX\nc\n");
        assert_eq!(accepted(&merged, Choice::Both), "a\nB\nX\nc\n", "current then incoming");
        assert_eq!(
            accepted(&merged, Choice::Base),
            "a\nc\n",
            "no base in a two-way conflict: it removes the conflict"
        );

        let merged = merge_file("a\nb\nc\n", "a\nB\nc\n", "a\nX\nc\n", true);
        assert_eq!(accepted(&merged, Choice::Current), "a\nB\nc\n");
        assert_eq!(accepted(&merged, Choice::Incoming), "a\nX\nc\n");
        assert_eq!(accepted(&merged, Choice::Base), "a\nb\nc\n");
    }

    #[test]
    fn accept_all_takes_every_conflict_in_one_set() {
        let list = conflicts(TWO_CONFLICTS);
        assert_eq!(list.len(), 2);
        let applied = |choice| {
            let mut rope = Rope::from_str(TWO_CONFLICTS);
            let set = accept_all(&rope, &list, choice);
            assert_eq!(set.len(), list.len(), "one change per conflict");
            set.apply(&mut rope);
            rope.to_string()
        };
        assert_eq!(applied(Choice::Current), "a\nb\nd\ne\ng\n");
        assert_eq!(applied(Choice::Incoming), "a\nc\nd\nf\ng\n");
        assert_eq!(applied(Choice::Both), "a\nb\nc\nd\ne\nf\ng\n");
        assert_eq!(applied(Choice::Base), "a\nd\ng\n", "no base block: both are removed");
        let rope = Rope::from_str(TWO_CONFLICTS);
        assert_eq!(block(&rope, &list[0], Choice::Current), "b\n");
        assert_eq!(block(&rope, &list[1], Choice::Both), "e\nf\n");
    }

    #[test]
    fn unterminated_markers_are_not_conflicts() {
        assert!(conflicts("a\n<<<<<<< ours\nB\n=======\nX\n").is_empty());
        assert!(conflicts("<<<<<<< ours\nB\n=======\nX\n").is_empty());
        assert!(conflicts("<<<<<<< ours\nB\n>>>>>>> theirs\n").is_empty(), "no =======");
    }

    #[test]
    fn nested_markers_refuse_the_outer_conflict() {
        let nested = "a\n<<<<<<< A\nx\n<<<<<<< B\ny\n=======\nz\n>>>>>>> C\nb\n";
        assert!(
            conflicts(nested).is_empty(),
            "the outer is refused and so is the <<<<<<< inside it"
        );
        // The scan resumes after the `>>>>>>>` that would have closed the outer.
        let after = format!("{nested}<<<<<<< D\nq\n=======\nr\n>>>>>>> E\n");
        let list = conflicts(&after);
        assert_eq!(list.len(), 1);
        let rope = Rope::from_str(&after);
        assert_eq!(pos::line_of(&rope, list[0].start), 9);
        assert_eq!(accepted(&after, Choice::Incoming), format!("{nested}r\n"));
    }

    #[test]
    fn markers_only_count_at_line_starts() {
        assert!(conflicts(" <<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\n").is_empty());
        assert!(conflicts("a <<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\n").is_empty());
        // Stray separators and ends alone are content.
        assert!(conflicts("=====\n=======\n>>>>>>>\n").is_empty());
    }

    #[test]
    fn blocks_keep_their_line_breaks_and_empty_sides() {
        // CRLF markers and blocks.
        let crlf = "<<<<<<< a\r\nx\r\n=======\r\ny\r\n>>>>>>> b\r\n";
        assert_eq!(accepted(crlf, Choice::Current), "x\r\n");
        assert_eq!(accepted(crlf, Choice::Incoming), "y\r\n");
        assert_eq!(accepted(crlf, Choice::Both), "x\r\ny\r\n");
        // No line break after the end marker: the conflict ends at the text's end.
        let last = "<<<<<<< a\nx\n=======\ny\n>>>>>>> b";
        assert_eq!(conflicts(last)[0].end, ByteIdx(last.len()));
        assert_eq!(accepted(last, Choice::Current), "x\n");
        // An empty current side (git writes none when a side deleted the lines).
        let empty = "a\n<<<<<<< ours\n=======\nX\n>>>>>>> theirs\n";
        assert_eq!(accepted(empty, Choice::Current), "a\n");
        assert_eq!(accepted(empty, Choice::Incoming), "a\nX\n");
        // An empty incoming side, the mirror case.
        let empty_incoming = "a\n<<<<<<< ours\nB\n=======\n>>>>>>> theirs\n";
        assert_eq!(accepted(empty_incoming, Choice::Incoming), "a\n");
        assert_eq!(accepted(empty_incoming, Choice::Current), "a\nB\n");
    }

    #[test]
    fn next_and_previous_wrap_between_conflicts() {
        let rope = Rope::from_str(TWO_CONFLICTS);
        let mut conflicts = Conflicts::default();
        let list = conflicts.scan(&rope, 0).to_vec();
        assert_eq!(list.len(), 2);
        let (first, second) = (list[0].start, list[1].start);
        assert!(first < second);
        assert_eq!(conflicts.next(&rope, 0, ByteIdx(0)).map(|c| c.start), Some(first));
        assert_eq!(conflicts.next(&rope, 0, first).map(|c| c.start), Some(second));
        assert_eq!(conflicts.next(&rope, 0, second).map(|c| c.start), Some(first), "wraps");
        let end = ByteIdx(rope.len());
        assert_eq!(conflicts.previous(&rope, 0, end).map(|c| c.start), Some(second));
        assert_eq!(conflicts.previous(&rope, 0, second).map(|c| c.start), Some(first));
        assert_eq!(
            conflicts.previous(&rope, 0, first).map(|c| c.start),
            Some(second),
            "wraps"
        );
        let plain = Rope::from_str("plain\n");
        assert_eq!(conflicts.next(&plain, 1, ByteIdx(0)), None);
        assert_eq!(conflicts.previous(&plain, 1, ByteIdx(0)), None);
    }

    #[test]
    fn count_is_the_last_scans_and_none_before_one() {
        let rope = Rope::from_str(TWO_CONFLICTS);
        let mut conflicts = Conflicts::default();
        assert_eq!(conflicts.count(), None, "nothing scanned yet");
        assert_eq!(conflicts.scan(&rope, 0).len(), 2);
        assert_eq!(conflicts.count(), Some(2));
        let plain = Rope::from_str("plain\n");
        conflicts.scan(&plain, 1);
        assert_eq!(conflicts.count(), Some(0));
        // An edit invalidates it until the next scan.
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(rope.len()),
            text: "x\n".into(),
        }]));
        conflicts.map(&set);
        assert_eq!(conflicts.count(), None);
    }

    #[test]
    fn the_scan_is_cached_by_revision_and_edits_invalidate_it() {
        let rope = Rope::from_str(TWO_CONFLICTS);
        let mut conflicts = Conflicts::default();
        assert_eq!(conflicts.scan(&rope, 0).len(), 2);
        let plain = Rope::from_str("plain\n");
        assert_eq!(conflicts.scan(&plain, 0).len(), 2, "the same revision: the cached scan");
        assert_eq!(conflicts.scan(&plain, 1).len(), 0, "a new revision: scanned again");
        // An edit applied to the buffer (`Buffer::apply` calls `map`) invalidates it even at the
        // same revision.
        let mut rope = Rope::from_str(TWO_CONFLICTS);
        assert_eq!(conflicts.scan(&rope, 2).len(), 2);
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(rope.len()),
            text: "plain\n".into(),
        }]));
        set.apply(&mut rope);
        conflicts.map(&set);
        assert_eq!(conflicts.scan(&rope, 2).len(), 0);
        conflicts.reset();
        assert!(conflicts.conflicts().is_empty());
        assert_eq!(conflicts.scan(&rope, 2).len(), 0);
    }
}
