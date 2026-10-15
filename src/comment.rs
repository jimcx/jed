//! Comment line (Cmd+/), VS Code's `editor.action.commentLine` (`LineCommentCommand`).
//!
//! Each selection toggles its own lines: if every non-blank one starts (after its indentation)
//! with the language's line comment token, the token and one space after it are removed;
//! otherwise every non-blank line gets the token and a space at the block's smallest
//! indentation (a selection of only blank lines comments them all). A selection ending at the
//! start of a later line doesn't take that line. A line shared by two selections belongs to
//! the first. One undo step for all of them; selections stay on their text. A language without
//! a line comment token (plain text) does nothing.
//!
//! Unlike VS Code the smallest indentation isn't rounded down to a multiple of the indent size
//! (red has no indentation detection, so a 2-space file with tab width 4 would get `//` at
//! column 0); with detection on VS Code usually gives the same result.
//!
//! Lines are read with a `LineWalker` and the edit is one `ChangeSet` built in order, so it
//! stays fast at 60k lines or selections.

use std::ops::Range;

use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{ByteIdx, LineWalker};
use crate::buffer::undo::EditKind;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// One line of a selection's block.
struct LineInfo {
    start: usize,
    /// Its leading spaces and tabs, as a range of `Scan::prefixes`.
    prefix: Range<usize>,
    blank: bool,
    commented: bool,
    /// The token is followed by a space (removed with it).
    space: bool,
}

pub fn toggle(buf: &mut Buffer, view: &mut View, tab_width: usize) {
    let Some(token) = buf.syntax.language().config().line_comment else {
        buf.seal_undo();
        return;
    };
    let rope = &buf.rope;
    let before = view.selections.clone();
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(before.len(), 0);
    let mut insert = String::from(token);
    insert.push(' ');
    let mut lines: Vec<LineInfo> = Vec::new();
    let mut prefixes = String::new();
    let mut prev_last = None;
    for s in before.iter() {
        let first = walk.line_at(s.start()).index;
        let end = walk.line_at(s.end());
        let mut last = end.index;
        if !s.is_empty() && last > first && end.start() == s.end() {
            last -= 1;
        }
        let first = if prev_last == Some(first) { first + 1 } else { first };
        if first > last {
            continue;
        }
        prev_last = Some(last);
        lines.clear();
        prefixes.clear();
        for i in first..=last {
            let line = walk.line(i);
            let text = line.text();
            let indent = text.len() - text.trim_start_matches([' ', '\t']).len();
            let rest = &text[indent..];
            let at = prefixes.len();
            prefixes.push_str(&text[..indent]);
            lines.push(LineInfo {
                start: line.start().0,
                prefix: at..prefixes.len(),
                blank: rest.is_empty(),
                commented: rest.starts_with(token),
                space: rest.strip_prefix(token).is_some_and(|r| r.starts_with(' ')),
            });
        }
        let all_blank = lines.iter().all(|l| l.blank);
        if !all_blank && lines.iter().all(|l| l.blank || l.commented) {
            for l in lines.iter().filter(|l| !l.blank) {
                let at = l.start + l.prefix.len();
                let len = token.len() + usize::from(l.space);
                changes.push(ByteIdx(at), ByteIdx(at + len), "");
            }
        } else {
            let take = |l: &&LineInfo| all_blank || !l.blank;
            let min = lines
                .iter()
                .filter(take)
                .map(|l| width(&prefixes[l.prefix.clone()], usize::MAX, tab_width).1)
                .min()
                .unwrap_or(0);
            for l in lines.iter().filter(take) {
                let at = l.start + width(&prefixes[l.prefix.clone()], min, tab_width).0;
                changes.push(ByteIdx(at), ByteIdx(at), &insert);
            }
        }
    }
    if changes.is_empty() {
        buf.seal_undo();
        return;
    }
    // A selection keeps text inserted at its edges (VS Code tracks it as growing), a cursor at
    // the insertion point lands after the token.
    let m = changes.mapper();
    let out: Vec<Selection> = before
        .iter()
        .map(|s| {
            if s.is_empty() {
                return Selection::cursor(m.map(s.head, Assoc::After));
            }
            let (start, end) = (m.map(s.start(), Assoc::Before), m.map(s.end(), Assoc::After));
            if s.head < s.anchor {
                Selection::new(end, start)
            } else {
                Selection::new(start, end)
            }
        })
        .collect();
    let after = Selections::with_newest(out, before.primary_index(), before.newest_index());
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
}

/// Walks an indentation prefix of spaces and tabs up to screen column `upto`: (bytes taken,
/// column reached). Stops before a tab that would pass `upto` (VS Code's
/// `_normalizeInsertionPoint`), so the token never lands inside a tab.
fn width(prefix: &str, upto: usize, tab_width: usize) -> (usize, usize) {
    let mut col = 0;
    for (i, b) in prefix.bytes().enumerate() {
        if col >= upto {
            return (i, col);
        }
        let next = if b == b'\t' {
            (col / tab_width + 1) * tab_width
        } else {
            col + 1
        };
        if next > upto {
            return (i, col);
        }
        col = next;
    }
    (prefix.len(), col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::edit::{self, Settings};
    use crate::pairs::tests::{setup, show};
    use crate::syntax::Language;

    fn toggled(language: Language, marked: &str) -> String {
        let (mut b, mut v) = setup(language, marked, false);
        toggle(&mut b, &mut v, 4);
        show(&b, &v)
    }

    #[test]
    fn comments_at_the_smallest_indent_skipping_blank_lines() {
        let cpp = Language::Cpp;
        assert_eq!(toggled(cpp, "a|b\n"), "// a|b\n");
        assert_eq!(
            toggled(cpp, "|ab"),
            "// |ab",
            "a cursor at the insertion point lands after it"
        );
        assert_eq!(
            toggled(cpp, "«  if (x) {\n\n      y;\n  }»\n"),
            "«  // if (x) {\n\n  //     y;\n  // }»\n"
        );
        // Blank lines (spaces only) are left alone, unless that's all there is.
        assert_eq!(toggled(cpp, "«a\n   \nb»"), "«// a\n   \n// b»");
        assert_eq!(toggled(cpp, "x\n  |\n"), "x\n  // |\n");
        // Tabs: the token goes before a tab that would pass the smallest indent.
        assert_eq!(toggled(cpp, "«\ta\n  b»"), "«// \ta\n  // b»");
        assert_eq!(toggled(cpp, "«\ta\n\t\tb»"), "«\t// a\n\t// \tb»");
    }

    #[test]
    fn uncomments_when_every_line_is_commented() {
        let cpp = Language::Cpp;
        assert_eq!(toggled(cpp, "«  // a\n  //b\n\n// c»"), "«  a\n  b\n\nc»");
        // One line without: comment them all (again).
        assert_eq!(toggled(cpp, "«// a\nb»"), "«// // a\n// b»");
        assert_eq!(toggled(cpp, "// a|b"), "a|b");
        assert_eq!(toggled(cpp, "// |"), "|");
        assert_eq!(
            toggled(cpp, "//| a"),
            "|a",
            "a cursor inside the token goes to its start"
        );
        assert_eq!(
            toggled(Language::Python, "«def f():\n    # x\n    return 1»"),
            "«# def f():\n#     # x\n#     return 1»"
        );
        assert_eq!(
            toggled(Language::Python, "«# def f():\n#     return 1»"),
            "«def f():\n    return 1»"
        );
    }

    #[test]
    fn line_ends_and_selection_ends() {
        let cpp = Language::Cpp;
        assert_eq!(
            toggled(cpp, "«a\r\nb\r\n»c"),
            "«// a\r\n// b\r\n»c",
            "CRLF; ends at c's line start"
        );
        assert_eq!(toggled(cpp, "a\nb|"), "a\n// b|", "last line without a line break");
        // A line starting with a multi-byte char (once sliced inside it and panicked).
        assert_eq!(toggled(cpp, "«界\n  é»"), "«// 界\n//   é»");
        assert_eq!(toggled(Language::Python, "界|"), "# 界|");
        assert_eq!(toggled(Language::Python, "#界|"), "界|");
        assert_eq!(toggled(cpp, "a«\n»b"), "// a«\n»b");
        // Backwards selections stay backwards.
        let (mut b, mut v) = setup(cpp, "|ab\ncd", false);
        v.selections = Selections::single(Selection::new(ByteIdx(5), ByteIdx(0)));
        toggle(&mut b, &mut v, 4);
        assert_eq!(b.rope.to_string(), "// ab\n// cd");
        let s = v.selections.primary();
        assert_eq!((s.anchor.0, s.head.0), (11, 0));
    }

    #[test]
    fn many_selections_toggle_their_own_lines() {
        let cpp = Language::Cpp;
        // Two cursors on one line: commented once.
        assert_eq!(toggled(cpp, "a|b|c\nd"), "// a|b|c\nd");
        // Each selection decides for its own lines (VS Code).
        assert_eq!(toggled(cpp, "a|\n// b|\nc"), "// a|\nb|\nc");
        // A line shared by two selections belongs to the first.
        assert_eq!(toggled(cpp, "«a\nb»«b\nc»"), "«// a\n// b»«b\n// c»");
    }

    #[test]
    fn plain_text_does_nothing_and_it_is_one_undo_step() {
        let (mut b, mut v) = setup(Language::PlainText, "a|", false);
        toggle(&mut b, &mut v, 4);
        assert_eq!(show(&b, &v), "a|");
        assert!(!b.is_dirty());
        let s = Settings::default();
        let (mut b, mut v) = setup(Language::Cpp, "a|\nb|", false);
        edit::insert_char(&mut b, &mut v, 'x', &s);
        edit::run(Command::CommentLine, &mut b, &mut v, &s);
        edit::insert_char(&mut b, &mut v, 'y', &s);
        assert_eq!(show(&b, &v), "// axy|\n// bxy|");
        edit::run(Command::Undo, &mut b, &mut v, &s);
        edit::run(Command::Undo, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "ax|\nbx|");
        edit::run(Command::Redo, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "// ax|\n// bx|");
    }

    /// Each language's line comment token; none for JSON, Markdown and XML (block comments only).
    #[test]
    fn tokens_per_language() {
        use Language::*;
        for l in [Rust, Jsonc] {
            assert_eq!(toggled(l, "«a\n  b»"), "«// a\n//   b»", "{l:?}");
        }
        for l in [Yaml, Toml, ShellScript, CMake, Makefile, Dockerfile] {
            assert_eq!(toggled(l, "  a|"), "  # a|", "{l:?}");
            assert_eq!(toggled(l, "  # a|"), "  a|", "{l:?}");
        }
        for l in [Json, Markdown, Xml] {
            assert_eq!(toggled(l, "a|"), "a|", "{l:?}");
        }
    }
}
