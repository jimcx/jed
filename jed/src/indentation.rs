//! Guessing a file's indentation from its text (VS Code's `editor.detectIndentation`): a port of
//! VS Code's `guessIndentation` (`textModel.ts` / `indentationGuesser.ts`).
//!
//! - The first `MAX_LINES` lines are scanned; blank and whitespace-only lines are skipped.
//! - Spaces vs tabs: lines whose indentation holds a tab against lines indented with two or more
//!   spaces only; a tie (none of either, say) keeps the default.
//! - The size (used only with spaces): each line's indentation is compared with the previous
//!   non-blank line's (`spaces_diff`: the spaces added or removed, or spaces per tab when both
//!   change), and every difference up to 8 is counted; the best scoring of 2, 4, 6, 8, 3, 5, 7
//!   (in that order of preference) wins, and 2 also wins over 4 when it scores at least half as
//!   much. No difference counted: the default size.
//! - A line that looks aligned rather than indented (`const a = 1,` / `      b = 2;`) is
//!   skipped, unless the defaults are spaces and the difference is the default size.
//!
//! `guess` gives what the text says (`Guess`, either half possibly unknown); `Guess::apply` fills
//! the rest in from the settings, as VS Code's result does. It runs where the file is read (load
//! and disk reload, off the UI thread) and the result is kept on the `Buffer`
//! (`Buffer::guess`); `edit::Settings::for_buffer` applies it.
//!
//! "Convert Indentation to Spaces / Tabs" (`changes`, `convert`) rewrite every line's leading
//! whitespace (spaces and tabs; a whitespace-only line is all leading whitespace) at the
//! buffer's tab width — the drawn width of a tab, which with tabs is also the indent size:
//! - The leading whitespace is measured in columns, each tab going to the next tab stop.
//! - To spaces: that many spaces. To tabs: `cols / tab_width` tabs and `cols % tab_width`
//!   spaces.
//!
//! VS Code (`getIndentationEditOperations`, at the model's `tabSize`) replaces each tab with
//! `tabSize` spaces and each run of `tabSize` spaces with a tab, as strings. That's the same
//! for indentation that is tabs followed by spaces; for spaces before a tab (`"  \t"`) it
//! changes the drawn width, measuring columns doesn't (`"  \t"` at 4 is 4 spaces, not 6).

use std::ops::ControlFlow;

use ropey::Rope;

use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::undo::EditKind;
use crate::selection::Selection;
use crate::view::View;

/// Lines looked at (VS Code's).
pub const MAX_LINES: usize = 10_000;

/// Sizes guessed, in order of preference (even first).
const GUESSES: [usize; 7] = [2, 4, 6, 8, 3, 5, 7];
const MAX_GUESS: usize = 8;

/// How much of the previous line is kept beyond its indentation (the alignment check reads one
/// byte at the next line's space count): positions past it count as not a space.
const KEEP: usize = 256;

/// Spaces (`size` of them per level) or tabs (drawn `size` wide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indentation {
    pub spaces: bool,
    pub size: usize,
}

/// What a text says about its indentation: `None` where it says nothing (a tie between tab- and
/// space-indented lines, no indentation differences between lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guess {
    pub spaces: Option<bool>,
    pub size: Option<usize>,
}

impl Guess {
    /// The indentation to use: the guess, the rest from `defaults`. The guessed size only goes
    /// with spaces (tabs keep the default's drawn width), as in VS Code.
    pub fn apply(self, defaults: Indentation) -> Indentation {
        let spaces = self.spaces.unwrap_or(defaults.spaces);
        let size = match self.size {
            Some(size) if spaces => size,
            _ => defaults.size,
        };
        Indentation { spaces, size }
    }
}

/// One line's indentation compared with the previous one's (VS Code's `SpacesDiffResult`).
#[derive(Default)]
struct Diff {
    spaces: usize,
    looks_like_alignment: bool,
}

/// The previous non-blank line, as much as `spaces_diff` reads of it: no allocation per line.
#[derive(Default)]
struct Prev {
    /// Its first bytes: the indentation and up to `KEEP` more.
    head: Vec<u8>,
    /// Its whole length in bytes.
    len: usize,
    indent: usize,
    last: Option<u8>,
}

impl Prev {
    fn byte(&self, i: usize) -> Option<u8> {
        self.head.get(i).copied()
    }
}

/// Guesses `rope`'s indentation from its first `MAX_LINES` lines. `defaults` (the settings) only
/// decide whether a line that looks aligned still counts: with spaces, when its difference is the
/// default size (a nested Markdown list item). Blocking for a big file: call it where the file is
/// read. O(bytes of the lines' indentation), no allocation per line.
pub fn guess(rope: &Rope, defaults: Indentation) -> Guess {
    let mut tab_lines = 0usize; // lines with a tab in their indentation
    let mut space_lines = 0usize; // lines indented with two or more spaces, no tab
    let mut counts = [0usize; MAX_GUESS + 1];
    let mut prev = Prev::default();
    let mut diff = Diff::default();
    let mut lines = 0;
    pos::each_line(rope, 0, false, |_, text| {
        if lines == MAX_LINES {
            return ControlFlow::Break(());
        }
        lines += 1;
        let bytes = text.as_bytes();
        let (mut tabs, mut spaces) = (0, 0);
        let Some(indent) = bytes.iter().position(|&c| match c {
            b'\t' => {
                tabs += 1;
                false
            }
            b' ' => {
                spaces += 1;
                false
            }
            _ => true,
        }) else {
            // Blank or only whitespace.
            return ControlFlow::Continue(());
        };
        if tabs > 0 {
            tab_lines += 1;
        } else if spaces > 1 {
            space_lines += 1;
        }
        spaces_diff(&prev, bytes, indent, &mut diff);
        if diff.looks_like_alignment && !(defaults.spaces && defaults.size == diff.spaces) {
            // const a = 1,
            //       b = 2;
            // (but with spaces at the default size it may be indentation: `- item` / `  - item`)
            return ControlFlow::Continue(());
        }
        if diff.spaces <= MAX_GUESS {
            counts[diff.spaces] += 1;
        }
        prev.head.clear();
        prev.head.extend_from_slice(&bytes[..bytes.len().min(indent + KEEP)]);
        prev.len = bytes.len();
        prev.indent = indent;
        prev.last = bytes.last().copied();
        ControlFlow::Continue(())
    });

    let spaces = (tab_lines != space_lines).then_some(tab_lines < space_lines);
    let mut size = None;
    let mut score = 0;
    for g in GUESSES {
        if counts[g] > score {
            score = counts[g];
            size = Some(g);
        }
    }
    // 2 wins over 4 when it scores at least half as much.
    if size == Some(4) && counts[4] > 0 && counts[2] > 0 && 2 * counts[2] >= counts[4] {
        size = Some(2);
    }
    Guess { spaces, size }
}

/// VS Code's `spacesDiff`: how `b`'s indentation (`b_indent` bytes) differs from `a`'s.
fn spaces_diff(a: &Prev, b: &[u8], b_indent: usize, out: &mut Diff) {
    out.spaces = 0;
    out.looks_like_alignment = false;
    // This can go both ways: "\t" → "\t    " is one tab and four spaces.
    let mut i = 0;
    while i < a.indent && i < b_indent && a.byte(i) == Some(b[i]) {
        i += 1;
    }
    let (mut a_spaces, mut a_tabs) = (0usize, 0usize);
    for j in i..a.indent {
        if a.byte(j) == Some(b' ') {
            a_spaces += 1;
        } else {
            a_tabs += 1;
        }
    }
    let (mut b_spaces, mut b_tabs) = (0usize, 0usize);
    for &c in &b[i..b_indent] {
        if c == b' ' {
            b_spaces += 1;
        } else {
            b_tabs += 1;
        }
    }
    if (a_spaces > 0 && a_tabs > 0) || (b_spaces > 0 && b_tabs > 0) {
        return;
    }
    let tabs_diff = a_tabs.abs_diff(b_tabs);
    let spaces_diff = a_spaces.abs_diff(b_spaces);
    if tabs_diff == 0 {
        // The difference may be alignment, which shouldn't count as a hint.
        out.spaces = spaces_diff;
        if spaces_diff > 0
            && b_spaces >= 1
            && b_spaces - 1 < a.len
            && b_spaces < b.len()
            && b[b_spaces] != b' '
            && a.byte(b_spaces - 1) == Some(b' ')
            && a.last == Some(b',')
        {
            // const a = b + c,
            //       d = b || c;
            out.looks_like_alignment = true;
        }
        return;
    }
    if spaces_diff % tabs_diff == 0 {
        out.spaces = spaces_diff / tabs_diff;
    }
}

/// "Convert Indentation to Spaces / Tabs" (see the module doc): the changes to `rope`, one per
/// line whose leading whitespace changes, in one pass over the lines (no allocation per line).
/// Nothing to change: an empty set.
pub fn changes(rope: &Rope, spaces: bool, tab_width: usize) -> ChangeSet {
    let width = tab_width.max(1);
    let mut changes = ChangeSet::default();
    let mut new = String::new();
    pos::each_line(rope, 0, false, |start, text| {
        let bytes = text.as_bytes();
        let len = bytes
            .iter()
            .position(|&c| c != b' ' && c != b'\t')
            .unwrap_or(bytes.len());
        let indent = &bytes[..len];
        let mut cols = 0;
        for &c in indent {
            cols = if c == b'\t' {
                (cols / width + 1) * width
            } else {
                cols + 1
            };
        }
        let (tabs, rest) = if spaces {
            (0, cols)
        } else {
            (cols / width, cols % width)
        };
        let same = indent.len() == tabs + rest
            && indent[..tabs].iter().all(|&c| c == b'\t')
            && indent[tabs..].iter().all(|&c| c == b' ');
        if !same {
            new.clear();
            new.extend(std::iter::repeat_n('\t', tabs));
            new.extend(std::iter::repeat_n(' ', rest));
            changes.push(start, ByteIdx(start.0 + len), &new);
        }
        ControlFlow::<()>::Continue(())
    });
    changes
}

/// Converts `buf`'s indentation (`changes`) as one `EditKind::Other` undo step, `view`'s
/// selections mapped through it (one inside the rewritten whitespace goes to the line start,
/// one at its end stays at the end). Returns whether anything changed (if not, no undo step).
pub fn convert(buf: &mut Buffer, view: &mut View, spaces: bool, tab_width: usize) -> bool {
    let changes = changes(&buf.rope, spaces, tab_width);
    if changes.is_empty() {
        buf.seal_undo();
        return false;
    }
    let before = view.selections.clone();
    let mut after = before.clone();
    {
        let m = changes.mapper();
        // In order (start before end), so each lookup steps on from the last.
        after.transform(|s| {
            let start = m.map(s.start(), Assoc::Before);
            let end = if s.is_empty() {
                start
            } else {
                m.map(s.end(), Assoc::Before)
            };
            let (anchor, head) = if s.head < s.anchor { (end, start) } else { (start, end) };
            Selection {
                anchor,
                head,
                goal: None,
            }
        });
    }
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size VS Code's tests can't guess: `13370` / `13371` as defaults, so a size that comes
    /// back unchanged was kept.
    enum Size {
        Unknown,
        Is(usize),
        /// Guessed only when the defaults are spaces (VS Code's `[n]`).
        WithSpaces(usize),
    }
    use Size::*;

    fn guessed(default_spaces: bool, default_size: usize, lines: &[&str]) -> Indentation {
        let rope = Rope::from_str(&lines.join("\n"));
        let defaults = Indentation {
            spaces: default_spaces,
            size: default_size,
        };
        guess(&rope, defaults).apply(defaults)
    }

    /// VS Code's `testGuessIndentation`.
    #[track_caller]
    fn check_with(default_spaces: bool, default_size: usize, spaces: bool, size: usize, lines: &[&str]) {
        assert_eq!(
            guessed(default_spaces, default_size, lines),
            Indentation { spaces, size },
            "defaults ({default_spaces}, {default_size}): {lines:?}"
        );
    }

    /// VS Code's `assertGuess`: tried with both defaults.
    #[track_caller]
    fn check(spaces: Option<bool>, size: Size, lines: &[&str]) {
        match (spaces, size) {
            (None, Unknown) => {
                check_with(true, 13370, true, 13370, lines);
                check_with(false, 13371, false, 13371, lines);
            }
            (None, Is(n)) => {
                check_with(true, 13370, true, n, lines);
                check_with(false, 13371, false, n, lines);
            }
            (None, WithSpaces(n)) => {
                check_with(true, 13370, true, n, lines);
                check_with(false, 13371, false, 13371, lines);
            }
            (Some(s), Unknown) => {
                check_with(true, 13370, s, 13370, lines);
                check_with(false, 13371, s, 13371, lines);
            }
            (Some(s), Is(n)) => {
                check_with(true, 13370, s, n, lines);
                check_with(false, 13371, s, n, lines);
            }
            (Some(true), WithSpaces(n)) => {
                check_with(true, 13370, true, n, lines);
                check_with(false, 13371, true, n, lines);
            }
            (Some(false), WithSpaces(_)) => {
                check_with(true, 13370, false, 13370, lines);
                check_with(false, 13371, false, 13371, lines);
            }
        }
    }

    // VS Code's `textModel.test.ts`, "guess indentation 1".

    #[test]
    fn no_clues_keep_the_defaults() {
        check(None, Unknown, &["x", "x", "x", "x", "x", "x", "x"]);
        check(None, Unknown, &[]);
        check(None, Unknown, &["", "  ", "\t", "x"]);
    }

    #[test]
    fn one_tab_line_means_tabs() {
        check(Some(false), Unknown, &["\tx", "x", "x", "x", "x", "x", "x"]);
        check(Some(false), Unknown, &["\tx", "\tx", "\tx", "\tx", "\tx", "\tx", "\tx"]);
    }

    #[test]
    fn spaces_and_their_size() {
        check(Some(true), Is(2), &["  x", "x", "x", "x", "x", "x", "x"]);
        check(Some(true), Is(2), &["x", "  x", "  x", "  x", "  x", "  x", "  x"]);
        check(Some(true), Is(2), &["  x", "  x", "  x", "  x", "  x", "  x", "  x"]);
        check(
            Some(true),
            Is(4),
            &["x", "    x", "    x", "    x", "    x", "    x", "    x"],
        );
        check(
            Some(true),
            Is(4),
            &["    x", "    x", "    x", "    x", "    x", "    x", "    x"],
        );
        check(
            Some(true),
            Is(3),
            &["x", "   x", "   x", "   x", "   x", "   x", "   x"],
        );
        check(Some(true), Is(5), &["x", "     x", "     x", "     x", "     x"]);
        check(Some(true), Is(6), &["x", "      x", "      x", "      x"]);
        check(Some(true), Is(7), &["x", "       x", "       x"]);
        check(Some(true), Is(8), &["x", "        x", "        x"]);
        check(
            Some(true),
            Is(2),
            &["x", "  x", "    x", "    x", "      x", "        x"],
        );
        check(
            Some(true),
            Is(4),
            &["x", "    x", "        x", "            x", "        x", "    x"],
        );
    }

    #[test]
    fn one_space_is_not_indentation() {
        // ' x' isn't a space-indented line, and 1 isn't a size guessed.
        check(None, Unknown, &[" x", "x", " x", "x", " x", "x"]);
        check(
            Some(false),
            Unknown,
            &["\tx", " x", "\tx", " x", "\tx", " x", "\tx", " x"],
        );
    }

    #[test]
    fn tabs_against_spaces() {
        // '4x2, 4xTAB': a tie, and a tab against two spaces is a size of 2.
        check(
            None,
            WithSpaces(2),
            &["\tx", "  x", "\tx", "  x", "\tx", "  x", "\tx", "  x"],
        );
        check(
            None,
            WithSpaces(4),
            &["\tx", "    x", "\tx", "    x", "\tx", "    x", "\tx", "    x"],
        );
        // '4x2, 5xTAB'
        check(
            Some(false),
            Unknown,
            &["\tx", "\tx", "  x", "\tx", "  x", "\tx", "  x", "\tx", "  x"],
        );
        check(
            Some(false),
            Unknown,
            &["\tx", "\tx", "x", "\tx", "x", "\tx", "x", "\tx", "x"],
        );
        check(
            Some(true),
            Is(4),
            &["\tx", "    x", "    x", "\tx", "    x", "\tx", "    x", "\tx", "    x"],
        );
    }

    #[test]
    fn two_wins_over_four_at_half_its_score() {
        // 4 scores 3, 2 scores 2: two.
        check(Some(true), Is(2), &["x", "    x", "x", "    x", "      x", "        x"]);
        // 4 scores 4, 2 scores 2: exactly half, two.
        check(Some(true), Is(2), &["x", "    x", "x", "    x", "x", "  x", "    x"]);
        // 4 scores 3, 2 scores 1: four.
        check(Some(true), Is(4), &["x", "    x", "x", "    x", "      x"]);
    }

    // VS Code's issue cases.

    #[test]
    fn issue_44991_wrong_indentation_size() {
        check(
            Some(true),
            Is(4),
            &[
                "a = 10             # 0 space indent",
                "b = 5              # 0 space indent",
                "if a > 10:         # 0 space indent",
                "    a += 1         # 4 space indent      delta 4 spaces",
                "    if b > 5:      # 4 space indent",
                "        b += 1     # 8 space indent      delta 4 spaces",
                "        b += 1     # 8 space indent",
                "        b += 1     # 8 space indent",
                "# comment line 1   # 0 space indent      delta 8 spaces",
                "# comment line 2   # 0 space indent",
                "# comment line 3   # 0 space indent",
                "        b += 1     # 8 space indent      delta 8 spaces",
                "        b += 1     # 8 space indent",
                "        b += 1     # 8 space indent",
            ],
        );
    }

    #[test]
    fn issue_55818_alignment_is_not_indentation() {
        check(
            Some(true),
            Is(2),
            &[
                "",
                "/* REQUIRE */",
                "",
                "const foo = require ( 'foo' ),",
                "      bar = require ( 'bar' );",
                "",
                "/* MY FN */",
                "",
                "function myFn () {",
                "",
                "  const asd = 1,",
                "        dsa = 2;",
                "",
                "  return bar ( foo ( asd ) );",
                "",
                "}",
                "",
                "/* EXPORT */",
                "",
                "module.exports = myFn;",
                "",
            ],
        );
    }

    #[test]
    fn issue_70832_tabs_with_a_stray_space_line() {
        check(
            Some(false),
            Unknown,
            &[
                "x", "x", "x", "x", "\tx", "\t\tx", "    x", "\t\tx", "\tx", "\t\tx", "\tx", "\tx", "\tx", "\tx", "x",
            ],
        );
    }

    #[test]
    fn issue_62143_markdown_lists() {
        check(Some(true), Is(2), &["x", "x", "  x", "  x"]);
        check(Some(true), Is(2), &["x", "  - item2", "  - item3"]);
        check_with(true, 2, true, 2, &["x x", "  x", "  x"]);
        check_with(true, 2, true, 2, &["x x", "  x", "  x", "    x"]);
        check_with(
            true,
            2,
            true,
            2,
            &["<!--test1.md -->", "- item1", "  - item2", "    - item3"],
        );
    }

    #[test]
    fn issue_84217_python() {
        check(Some(true), Is(4), &["def main():", "    print('hello')"]);
        check(
            Some(true),
            Is(4),
            &["def main():", "    with open('foo') as fp:", "        print(fp.read())"],
        );
    }

    // jed's own.

    #[test]
    fn crlf_and_cr_line_breaks_count_as_lines() {
        let rope = Rope::from_str("fn a() {\r\n\tx\r\n}\r\n");
        assert_eq!(guess(&rope, DEFAULT).spaces, Some(false));
        let rope = Rope::from_str("a:\r  b\r    c\r");
        assert_eq!(
            guess(&rope, DEFAULT),
            Guess {
                spaces: Some(true),
                size: Some(2)
            }
        );
    }

    const DEFAULT: Indentation = Indentation { spaces: true, size: 4 };

    #[test]
    fn only_the_first_lines_are_read() {
        let mut text = "x\n".repeat(MAX_LINES - 1);
        text.push_str("  x\n");
        assert_eq!(guess(&Rope::from_str(&text), DEFAULT).size, Some(2));
        let mut text = "x\n".repeat(MAX_LINES);
        text.push_str("  x\n\tx\n");
        assert_eq!(guess(&Rope::from_str(&text), DEFAULT), Guess::default());
    }

    #[test]
    fn long_lines_across_chunks() {
        // Lines longer than a rope chunk, and indentation past what `Prev` keeps.
        let long = "y".repeat(5000);
        let deep = " ".repeat(600);
        let text = format!("{long},\n  {long}\n{deep}x\n{deep}  x\n");
        assert_eq!(
            guess(&Rope::from_str(&text), DEFAULT),
            Guess {
                spaces: Some(true),
                size: Some(2)
            }
        );
    }

    #[test]
    fn alignment_counts_only_at_the_default_size_with_spaces() {
        // The line above ends in a comma and `b` sits under a space: it looks aligned.
        let rope = Rope::from_str("- a,\n  b\n");
        let with = |spaces, size| guess(&rope, Indentation { spaces, size }).size;
        assert_eq!(with(true, 2), Some(2), "the default size with spaces: indentation");
        assert_eq!(with(true, 4), None, "another size: skipped");
        assert_eq!(with(false, 2), None, "tabs: skipped");
        // Without the comma it is plain indentation.
        let rope = Rope::from_str("- a\n  b\n");
        assert_eq!(guess(&rope, Indentation { spaces: true, size: 4 }).size, Some(2));
    }

    #[test]
    fn apply_keeps_the_default_size_with_tabs() {
        let g = Guess {
            spaces: Some(false),
            size: Some(2),
        };
        assert_eq!(g.apply(DEFAULT), Indentation { spaces: false, size: 4 });
        let g = Guess {
            spaces: None,
            size: Some(2),
        };
        assert_eq!(g.apply(DEFAULT), Indentation { spaces: true, size: 2 });
        assert_eq!(Guess::default().apply(DEFAULT), DEFAULT);
    }

    fn converted(text: &str, spaces: bool, width: usize) -> String {
        let mut rope = Rope::from_str(text);
        changes(&rope, spaces, width).apply(&mut rope);
        rope.to_string()
    }

    #[test]
    fn to_spaces_expands_tabs_to_their_stops() {
        assert_eq!(converted("\ta\n\t\tb\n", true, 4), "    a\n        b\n");
        // Partial stops: "  \t" is one stop, "\t  \t" two.
        assert_eq!(converted("  \ta\n\t  \tb\n", true, 4), "    a\n        b\n");
        assert_eq!(converted(" \t x\n", true, 3), "    x\n");
        // Tabs past the text's start are kept.
        assert_eq!(converted("\ta\tb\n", true, 4), "    a\tb\n");
    }

    #[test]
    fn to_tabs_makes_tabs_then_the_remaining_spaces() {
        assert_eq!(
            converted("    a\n        b\n      c\n", false, 4),
            "\ta\n\t\tb\n\t  c\n"
        );
        // Measured in columns, spaces before a tab included.
        assert_eq!(converted("  \ta\n \t  b\n", false, 4), "\ta\n\t  b\n");
        // Less than a tab's worth stays.
        assert_eq!(converted("  a\n", false, 4), "  a\n");
        // Spaces inside the line are kept.
        assert_eq!(converted("    a    b\n", false, 4), "\ta    b\n");
    }

    #[test]
    fn blank_whitespace_only_and_crlf_lines() {
        // Empty lines are left; whitespace-only lines are all leading whitespace.
        assert_eq!(converted("\n    \n\t\n", false, 4), "\n\t\n\t\n");
        assert_eq!(converted("\n    \n\t\n", true, 2), "\n    \n  \n");
        // CRLF and lone CR line breaks are kept, the last line needs none.
        assert_eq!(converted("    a\r\n    b\r    c", false, 4), "\ta\r\n\tb\r\tc");
        assert_eq!(converted("", true, 4), "");
    }

    #[test]
    fn only_changed_lines_get_a_change() {
        let rope = Rope::from_str("\ta\n  b\nc\n\t\td\n");
        let set = changes(&rope, false, 4);
        assert!(set.is_empty(), "already tabs (and less than a tab of spaces)");
        let set = changes(&rope, true, 4);
        let got: Vec<_> = set.changes().map(|(s, e, t)| (s.0, e.0, t.to_string())).collect();
        assert_eq!(got, [(0, 1, "    ".into()), (9, 11, "        ".into())]);
        assert!(changes(&Rope::from_str("    a\n  b\n"), true, 4).is_empty());
    }

    #[test]
    fn convert_is_one_undo_step_and_maps_every_selection() {
        use crate::selection::Selections;
        let text = "    a\n        bc\n  d\n";
        let mut buf = Buffer::from_str(text);
        let mut view = View::new();
        let sel = |a: usize, h: usize| Selection::new(ByteIdx(a), ByteIdx(h));
        // At line 1's text, inside line 2's indentation, a selection over "bc" (reversed), on "d".
        let mut list = vec![sel(4, 4), sel(8, 8), sel(16, 14), sel(19, 20)];
        list[1].goal = Some(crate::buffer::pos::DisplayCol(9));
        let before = Selections::new(list, 2);
        view.selections = before.clone();
        assert!(convert(&mut buf, &mut view, false, 4));
        assert_eq!(buf.rope.to_string(), "\ta\n\t\tbc\n  d\n");
        let got: Vec<_> = view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(got, [(1, 1), (3, 3), (7, 5), (10, 11)]);
        assert_eq!(view.selections.primary_index(), 2);
        assert!(view.selections.iter().all(|s| s.goal.is_none()), "goal columns dropped");
        assert!(buf.is_dirty());
        assert_eq!(buf.undo(), Some(before));
        assert_eq!(buf.rope.to_string(), text);
        assert!(!buf.is_dirty());
        // Nothing to change: no undo step.
        assert!(!convert(&mut buf, &mut view, true, 4));
        assert!(buf.undo().is_none());
    }
}
