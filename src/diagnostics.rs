//! Diagnostics as the editor shows them: severities, and `Marks`, the squiggles of one buffer
//! (byte ranges in its current text, kept there by `Buffer::apply` mapping them through every
//! change set). Where they come from (`textDocument/publishDiagnostics`, the store of every
//! file's diagnostics, converting positions) is `lsp::diagnostics`.

use std::sync::Arc;

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx, Peek};
use crate::lsp::ServerId;

/// Most severe first (`Ord`: `Error` is the least).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    pub const ALL: [Severity; 4] = [Severity::Error, Severity::Warning, Severity::Info, Severity::Hint];

    /// An LSP severity; a missing one is an error (as VS Code's client reads it).
    pub fn from_lsp(s: Option<lsp_types::DiagnosticSeverity>) -> Self {
        use lsp_types::DiagnosticSeverity as S;
        match s {
            Some(S::WARNING) => Self::Warning,
            Some(S::INFORMATION) => Self::Info,
            Some(S::HINT) => Self::Hint,
            _ => Self::Error,
        }
    }

    /// The theme key of its squiggle (VS Code's names).
    pub fn theme_key(self) -> &'static str {
        match self {
            Self::Error => "editorError.foreground",
            Self::Warning => "editorWarning.foreground",
            Self::Info => "editorInfo.foreground",
            Self::Hint => "editorHint.foreground",
        }
    }

    /// The theme key of its icon in the Problems list (VS Code's names; hints have none there).
    pub fn icon_key(self) -> &'static str {
        match self {
            Self::Error => "problemsErrorIcon.foreground",
            Self::Warning => "problemsWarningIcon.foreground",
            Self::Info => "problemsInfoIcon.foreground",
            Self::Hint => "problemsHintIcon.foreground",
        }
    }

    /// The status bar's symbol (VS Code's codicons, as near as a terminal gets).
    pub fn icon(self) -> &'static str {
        match self {
            Self::Error => "⊗",
            Self::Warning => "⚠",
            Self::Info => "ⓘ",
            Self::Hint => "…",
        }
    }
}

/// One diagnostic's content (its position is kept apart: LSP positions in the store, byte
/// ranges in a buffer's `Marks`; both share this through an `Arc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// Who says so (`clang`, `ty`, …).
    pub source: Option<String>,
    pub code: Option<String>,
    /// The diagnostic as its server published it (range, `code`, `data`, …): code action
    /// requests send it back in `context.diagnostics` (with the range where it is now), as
    /// rust-analyzer and clangd find their fixes by it. `Null` when it didn't come from a server.
    pub raw: serde_json::Value,
}

/// A diagnostic placed in a buffer's text. `start == end` is an empty range (drawn under the
/// char there, or the one before at a line end).
#[derive(Debug, Clone)]
pub struct Mark {
    pub start: ByteIdx,
    pub end: ByteIdx,
    pub server: ServerId,
    pub diag: Arc<Diagnostic>,
}

/// A buffer's diagnostics from every server, sorted by (start, end). Mapped through each change
/// set the buffer applies until the server publishes again: text inserted at a range's edges
/// stays outside it, a range whose text is deleted becomes empty where it was.
#[derive(Debug, Default)]
pub struct Marks {
    marks: Vec<Mark>,
    /// `max_end[i]`: the largest end among `marks[..=i]` (ends aren't sorted): with it,
    /// `overlapping` finds its first candidate by binary search too.
    max_end: Vec<ByteIdx>,
}

impl Marks {
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Every mark, sorted by (start, end).
    pub fn all(&self) -> &[Mark] {
        &self.marks
    }

    /// Replaces `server`'s marks with `marks` (any order).
    pub fn replace(&mut self, server: ServerId, marks: Vec<Mark>) {
        self.marks.retain(|m| m.server != server);
        self.marks.extend(marks);
        self.marks.sort_by_key(|m| (m.start, m.end));
        self.index();
    }

    /// Drops `server`'s marks.
    pub fn clear(&mut self, server: ServerId) {
        if self.marks.iter().any(|m| m.server == server) {
            self.marks.retain(|m| m.server != server);
            self.index();
        }
    }

    /// Drops every server's marks (the buffer's file changed: they were for another path).
    pub fn clear_all(&mut self) {
        self.marks.clear();
        self.index();
    }

    /// Follows a change set the buffer applied. Mapping keeps the order, so nothing is sorted.
    pub fn map(&mut self, changes: &ChangeSet) {
        if self.marks.is_empty() || changes.is_empty() {
            return;
        }
        map_marks(&mut self.marks, changes);
        self.index();
    }

    /// The marks that may touch `from..=to` (an empty one at either end too), in order: found by
    /// binary search, then filtered; cheap with thousands of marks when few are on screen.
    pub fn overlapping(&self, from: ByteIdx, to: ByteIdx) -> impl Iterator<Item = &Mark> {
        let first = self.max_end.partition_point(|&e| e < from);
        let last = self.marks.partition_point(|m| m.start <= to);
        self.marks[first..last.max(first)].iter().filter(move |m| m.end >= from)
    }

    /// The diagnostic the status bar shows for a cursor at `head` on the line `start..=end`
    /// (content bounds): the most severe touching the line, among equals one under the cursor,
    /// then the first.
    pub fn for_line(&self, start: ByteIdx, end: ByteIdx, head: ByteIdx) -> Option<&Mark> {
        self.overlapping(start, end)
            .filter(|m| m.end > start || m.start >= start)
            .min_by_key(|m| (m.diag.severity, !(m.start <= head && head <= m.end), m.start))
    }

    /// The marks whose squiggle is drawn on the cell of the char at `at` (`extent`; at an empty
    /// line's end, the cell past it), most severe first: what the mouse resting there hovers.
    pub fn drawn_at(&self, rope: &Rope, at: ByteIdx) -> Vec<&Mark> {
        self.around(rope, at, |(s, e)| s <= at && at < e)
    }

    /// The marks a cursor at `at` touches (`extent` from its start to its end, both included:
    /// a range ending at the cursor too), most severe first: what Show Hover shows there.
    pub fn touching(&self, rope: &Rope, at: ByteIdx) -> Vec<&Mark> {
        self.around(rope, at, |(s, e)| s <= at && at <= e)
    }

    /// The marks near `at` (an empty one's extent reaches a grapheme away) whose extent `keep`
    /// takes, sorted by severity (then position).
    fn around(&self, rope: &Rope, at: ByteIdx, keep: impl Fn((ByteIdx, ByteIdx)) -> bool) -> Vec<&Mark> {
        let mut peek = Peek::new(rope);
        let from = pos::prev_grapheme(rope, at);
        let to = pos::next_grapheme(rope, at);
        let mut found: Vec<&Mark> = self
            .overlapping(from, to)
            .filter(|m| keep(extent(rope, &mut peek, m)))
            .collect();
        found.sort_by_key(|m| m.diag.severity);
        found
    }

    fn index(&mut self) {
        self.max_end.clear();
        let mut max = ByteIdx(0);
        self.max_end.extend(self.marks.iter().map(|m| {
            max = max.max(m.end);
            max
        }));
    }
}

/// Maps marks (sorted by start) through `changes`, in order (the mapper steps forward).
pub fn map_marks(marks: &mut [Mark], changes: &ChangeSet) {
    let m = changes.mapper();
    for mark in marks {
        mark.start = m.map(mark.start, Assoc::After);
        mark.end = m.map(mark.end, Assoc::Before).max(mark.start);
    }
}

/// Where a mark's squiggle is drawn: its range, or for an empty one the char there, the one
/// before at a line end, or the cell past an empty line's end (`start..start + 1`). A range over
/// a line break alone (ty's "Expected an expression" after `y = 1 +`) is drawn as an empty one at
/// its start: a line break has no cell of its own.
pub fn extent(rope: &Rope, peek: &mut Peek, m: &Mark) -> (ByteIdx, ByteIdx) {
    let line_break = |c: Option<char>| matches!(c, None | Some('\n' | '\r'));
    if m.start < m.end {
        let len = m.end.0 - m.start.0;
        let only_break = match peek.after(m.start) {
            Some('\n') => len == 1,
            Some('\r') => len == 1 || (len == 2 && peek.after(ByteIdx(m.start.0 + 1)) == Some('\n')),
            _ => false,
        };
        if !only_break {
            return (m.start, m.end);
        }
    }
    match (line_break(peek.after(m.start)), line_break(peek.before(m.start))) {
        (false, _) => (m.start, pos::next_grapheme(rope, m.start)),
        (true, false) => (pos::prev_grapheme(rope, m.start), m.start),
        (true, true) => (m.start, ByteIdx(m.start.0 + 1)),
    }
}

/// A stretch of text drawn with one severity's squiggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub start: ByteIdx,
    pub end: ByteIdx,
    pub severity: Severity,
}

/// Flattens marks into sorted, non-overlapping runs, the most severe winning where they overlap.
/// `extent` gives each mark's drawn range (an empty mark's char); empty extents are skipped.
pub fn runs<'a>(
    marks: impl IntoIterator<Item = &'a Mark>,
    mut extent: impl FnMut(&Mark) -> (ByteIdx, ByteIdx),
) -> Vec<Run> {
    // (position, +1 / -1, severity): ends sort before starts at one position.
    let mut events: Vec<(ByteIdx, i32, Severity)> = Vec::new();
    for m in marks {
        let (s, e) = extent(m);
        if s < e {
            events.push((s, 1, m.diag.severity));
            events.push((e, -1, m.diag.severity));
        }
    }
    events.sort_by_key(|&(p, d, _)| (p, d));
    let mut active = [0i32; 4];
    let mut out: Vec<Run> = Vec::new();
    let mut at = ByteIdx(0);
    for (p, d, sev) in events {
        if p > at
            && let Some(i) = active.iter().position(|&n| n > 0)
        {
            let severity = Severity::ALL[i];
            match out.last_mut() {
                Some(r) if r.end == at && r.severity == severity => r.end = p,
                _ => out.push(Run {
                    start: at,
                    end: p,
                    severity,
                }),
            }
        }
        at = p;
        active[sev as usize] += d;
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::buffer::change::Change;

    pub fn diag(severity: Severity, message: &str) -> Arc<Diagnostic> {
        Arc::new(Diagnostic {
            severity,
            message: message.into(),
            source: None,
            code: None,
            raw: serde_json::Value::Null,
        })
    }

    fn mark(start: usize, end: usize, server: u64, severity: Severity) -> Mark {
        Mark {
            start: ByteIdx(start),
            end: ByteIdx(end),
            server: ServerId::test(server),
            diag: diag(severity, ""),
        }
    }

    fn spans(marks: &Marks) -> Vec<(usize, usize)> {
        marks.all().iter().map(|m| (m.start.0, m.end.0)).collect()
    }

    /// Where the mouse finds a mark (the cell its squiggle is drawn on) and where the cursor does
    /// (its extent, both ends included); most severe first.
    #[test]
    fn marks_drawn_at_and_touching() {
        let rope = Rope::from_str("ab cd\n\nxy\nuv\nst\r\n");
        let mut marks = Marks::default();
        marks.replace(
            ServerId::test(1),
            vec![
                mark(0, 2, 1, Severity::Info),
                mark(2, 3, 1, Severity::Warning),   // the space
                mark(5, 5, 1, Severity::Error),     // "ab cd"'s end: drawn under the d
                mark(6, 6, 1, Severity::Hint),      // the empty line: the cell past its end
                mark(12, 13, 1, Severity::Warning), // "uv"'s line break alone: under the v
                mark(15, 17, 1, Severity::Info),    // "st"'s CRLF alone: under the t
            ],
        );
        let at = |found: Vec<&Mark>| found.iter().map(|m| m.diag.severity).collect::<Vec<_>>();
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(0))), [Severity::Info]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(2))), [Severity::Warning]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(3))), []);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(4))), [Severity::Error]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(5))), []);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(6))), [Severity::Hint]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(7))), []);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(11))), [Severity::Warning]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(12))), []);
        assert_eq!(at(marks.touching(&rope, ByteIdx(12))), [Severity::Warning]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(14))), [Severity::Info]);
        assert_eq!(at(marks.drawn_at(&rope, ByteIdx(15))), []);
        assert_eq!(
            at(marks.touching(&rope, ByteIdx(2))),
            [Severity::Warning, Severity::Info]
        );
        assert_eq!(at(marks.touching(&rope, ByteIdx(3))), [Severity::Warning]);
        assert_eq!(at(marks.touching(&rope, ByteIdx(5))), [Severity::Error]);
        assert_eq!(at(marks.touching(&rope, ByteIdx(8))), []);
    }

    #[test]
    fn replace_is_per_server() {
        let mut marks = Marks::default();
        marks.replace(
            ServerId::test(1),
            vec![mark(5, 8, 1, Severity::Error), mark(0, 2, 1, Severity::Warning)],
        );
        marks.replace(ServerId::test(2), vec![mark(3, 4, 2, Severity::Hint)]);
        assert_eq!(spans(&marks), [(0, 2), (3, 4), (5, 8)]);
        marks.replace(ServerId::test(1), vec![mark(1, 1, 1, Severity::Info)]);
        assert_eq!(spans(&marks), [(1, 1), (3, 4)]);
        marks.clear(ServerId::test(2));
        assert_eq!(spans(&marks), [(1, 1)]);
        marks.clear(ServerId::test(1));
        assert!(marks.is_empty());
    }

    /// Text typed before a mark moves it, at its edges stays outside, inside widens it; a
    /// deleted range collapses where it was.
    #[test]
    fn marks_follow_edits() {
        let mut marks = Marks::default();
        marks.replace(
            ServerId::test(1),
            vec![mark(4, 7, 1, Severity::Error), mark(10, 10, 1, Severity::Error)],
        );
        let edit = |marks: &mut Marks, changes: &[(usize, usize, &str)]| {
            marks.map(&ChangeSet::new(
                changes
                    .iter()
                    .map(|&(s, e, t)| Change {
                        start: ByteIdx(s),
                        end: ByteIdx(e),
                        text: t.into(),
                    })
                    .collect(),
            ));
        };
        edit(&mut marks, &[(0, 0, "ab")]);
        assert_eq!(spans(&marks), [(6, 9), (12, 12)]);
        edit(&mut marks, &[(6, 6, "x"), (9, 9, "y")]);
        assert_eq!(spans(&marks), [(7, 10), (14, 14)], "edges stay outside");
        edit(&mut marks, &[(8, 8, "zz")]);
        assert_eq!(spans(&marks), [(7, 12), (16, 16)]);
        edit(&mut marks, &[(16, 16, "w")]);
        assert_eq!(
            spans(&marks),
            [(7, 12), (17, 17)],
            "an empty mark moves with typing at it"
        );
        edit(&mut marks, &[(6, 13, "q")]);
        assert_eq!(spans(&marks), [(7, 7), (11, 11)], "deleted: empty where it was");
    }

    /// `overlapping` equals a linear filter, also with a long mark early on.
    #[test]
    fn overlapping_matches_a_scan() {
        let mut list = vec![mark(0, 100, 1, Severity::Error)];
        for i in 0..200 {
            list.push(mark(i * 3, i * 3 + (i % 4), 1, Severity::Warning));
        }
        let mut marks = Marks::default();
        marks.replace(ServerId::test(1), list);
        for from in (0..620).step_by(7) {
            for len in [0, 1, 5, 40] {
                let to = from + len;
                let fast: Vec<_> = marks
                    .overlapping(ByteIdx(from), ByteIdx(to))
                    .map(|m| (m.start.0, m.end.0))
                    .collect();
                let slow: Vec<_> = marks
                    .all()
                    .iter()
                    .filter(|m| m.start.0 <= to && m.end.0 >= from)
                    .map(|m| (m.start.0, m.end.0))
                    .collect();
                assert_eq!(fast, slow, "{from}..={to}");
            }
        }
    }

    #[test]
    fn the_cursor_lines_diagnostic() {
        // Lines: 0..10, 11..20, 21..30.
        let mut marks = Marks::default();
        let mut list = vec![
            mark(2, 11, 1, Severity::Error), // ends at line 1's start: not on it
            mark(12, 13, 1, Severity::Warning),
            mark(16, 18, 1, Severity::Warning),
            mark(17, 25, 1, Severity::Info),
            mark(30, 30, 1, Severity::Hint),
            mark(0, 1, 1, Severity::Hint), // under the cursor at 0, but less severe than 0
        ];
        for (i, m) in list.iter_mut().enumerate() {
            m.diag = diag(m.diag.severity, &i.to_string());
        }
        marks.replace(ServerId::test(1), list);
        let msg = |s, e, head| {
            marks
                .for_line(ByteIdx(s), ByteIdx(e), ByteIdx(head))
                .map(|m| m.diag.message.clone())
        };
        assert_eq!(
            msg(0, 10, 0).as_deref(),
            Some("0"),
            "the most severe, not the one under the cursor"
        );
        assert_eq!(msg(11, 20, 11).as_deref(), Some("1"), "the first warning");
        assert_eq!(msg(11, 20, 17).as_deref(), Some("2"), "the one under the cursor");
        assert_eq!(msg(21, 30, 21).as_deref(), Some("3"), "info from line 1 reaches it");
        assert_eq!(msg(31, 31, 31), None);
    }

    #[test]
    fn runs_take_the_most_severe() {
        let list = [
            mark(0, 10, 1, Severity::Warning),
            mark(2, 4, 1, Severity::Error),
            mark(8, 12, 1, Severity::Info),
            mark(20, 20, 1, Severity::Hint),
            mark(30, 32, 1, Severity::Hint),
        ];
        let extent = |m: &Mark| {
            if m.start == m.end {
                (m.start, ByteIdx(m.start.0 + 1))
            } else {
                (m.start, m.end)
            }
        };
        let got: Vec<(usize, usize, Severity)> = runs(&list, extent)
            .iter()
            .map(|r| (r.start.0, r.end.0, r.severity))
            .collect();
        use Severity::*;
        assert_eq!(
            got,
            [
                (0, 2, Warning),
                (2, 4, Error),
                (4, 10, Warning),
                (10, 12, Info),
                (20, 21, Hint),
                (30, 32, Hint)
            ]
        );
    }
}
