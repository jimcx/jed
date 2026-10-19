//! Indent / outdent lines at every selection: Cmd+] / Cmd+[ (`indent`, `outdent`), Shift+Tab
//! (`outdent`) and Tab over lines (`tab`), VS Code's `ShiftCommand` (with `useTabStops`).
//!
//! - Each touched line's indentation (its leading spaces and tabs) becomes the next (indent) or
//!   previous (outdent) multiple of the tab width, written as the language's indentation
//!   (`Settings::for_language`: spaces, or tabs for a Makefile / `insert_spaces = false`). So a
//!   line with fewer spaces than a level outdents to column 0, and mixed indentation comes out
//!   normalised (as VS Code). Only the part of the indentation that differs is replaced: a line
//!   indented with spaces gets the new spaces at its indentation's end.
//! - The lines are those each selection touches; a non-empty selection ending at a later line's
//!   start doesn't take that line, and a line shared by two selections belongs to the first
//!   (`comment.rs`'s rules). Empty lines are skipped, unless the selection touches only that
//!   line (VS Code: Cmd+] at a cursor on an empty line indents it); lines of only whitespace
//!   count as indentation. Outdent leaves a line without indentation alone.
//! - Tab (`tab`) decides per selection, as VS Code's `TypeOperations.tab`: a selection over
//!   several lines, or over one whole line, indents its lines; others (cursors, part of a
//!   line) get the usual tab inserted in their place. When no selection indents, `tab` does
//!   nothing and the caller inserts tabs as before (an `EditKind::Insert` step). A selection of
//!   the second kind that starts inside an indentation already changed by an earlier selection
//!   in the same edit gets no tab of its own.
//! - Selections keep their text: a cursor at the indentation's end stays before the line's
//!   text (moved after inserted indentation), a non-empty selection whose start is in its first
//!   line's indentation keeps its start column there (so it grows over added indentation and
//!   shrinks with removed indentation, VS Code's `_selectionStartColumnStaysPut`), and its end
//!   stays before indentation inserted where it ends. Directions are kept.
//! - One undo step (`EditKind::Other`); nothing to change = no step. Lines are read with a
//!   `LineWalker` and the edit is one `ChangeSet` built in order, so it stays fast at 60k
//!   selections.
//!
//! Not done (VS Code differences): no indentation detection (the tab width is the indent
//! size); lines aligned past a level by an `onEnterRules` continuation (doc comments) aren't
//! kept aligned; Tab at a cursor on a whitespace-only line doesn't jump to the line's expected
//! indentation.

use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{ByteIdx, Line, LineWalker};
use crate::buffer::undo::EditKind;
use crate::edit::Settings;
use crate::selection::{Selection, Selections};
use crate::view::View;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Indent,
    Outdent,
    /// Indent the lines of selections over lines, insert a tab at the others.
    Tab,
}

/// Where a selection lands after the edit.
#[derive(Clone, Copy)]
enum Land {
    /// Replaced by a tab: a cursor after it.
    Replaced,
    /// On shifted lines: keeps its text. `stays`: the start of its first line, when the
    /// selection is non-empty and starts in that line's indentation (its column stays put).
    Shifted { stays: Option<usize> },
}

/// Cmd+]: indent the lines of every selection by one level.
pub fn indent(buf: &mut Buffer, view: &mut View, s: &Settings) {
    shift(buf, view, s, Mode::Indent);
}

/// Cmd+[ and Shift+Tab: outdent the lines of every selection by one level.
pub fn outdent(buf: &mut Buffer, view: &mut View, s: &Settings) {
    shift(buf, view, s, Mode::Outdent);
}

/// Tab: when some selection covers several lines or a whole line, indent those selections'
/// lines and put a tab in place of the others, as one step; false (nothing done) when none
/// does, for the caller to insert tabs.
pub fn tab(buf: &mut Buffer, view: &mut View, s: &Settings) -> bool {
    let mut walk = LineWalker::new(&buf.rope);
    let any = view
        .selections
        .iter()
        .any(|sel| !sel.is_empty() && shifts(sel, walk.line_at(sel.start())));
    if any {
        shift(buf, view, s, Mode::Tab);
    }
    any
}

/// Whether Tab indents a selection's lines (`line`: its start's): a selection over lines or a
/// whole line (VS Code). Cursors never do.
fn shifts(sel: &Selection, line: &Line) -> bool {
    !sel.is_empty() && (!line.contains(sel.end()) || (sel.start() == line.start() && sel.end() == line.end()))
}

/// Whether `sel` takes no line after `line`: it ends on it, or (non-empty) at the next one's
/// start.
fn ends_on(sel: &Selection, line: &Line) -> bool {
    line.contains(sel.end()) || (!sel.is_empty() && sel.end() == line.next())
}

/// `line`'s indentation one level on (`mode`), written into `new`: the change as (start, end,
/// bytes of `new` it keeps — the part both share — so the new text is `new[kept..]`). `None`:
/// outdent of a line without indentation.
fn reindent(
    line: &Line,
    mode: Mode,
    spaces: bool,
    tab_width: usize,
    new: &mut String,
) -> Option<(usize, usize, usize)> {
    let text = line.text();
    let len = indent_len(text);
    if mode == Mode::Outdent && len == 0 {
        return None;
    }
    let col = indent_col(&text[..len], tab_width);
    let level = if mode == Mode::Outdent {
        (col - 1) / tab_width
    } else {
        col / tab_width + 1
    };
    new.clear();
    for _ in 0..level {
        if spaces {
            (0..tab_width).for_each(|_| new.push(' '));
        } else {
            new.push('\t');
        }
    }
    let common = text
        .bytes()
        .zip(new.bytes())
        .take(len)
        .take_while(|(a, b)| a == b)
        .count();
    let start = line.start().0;
    Some((start + common, start + len, common))
}

/// The leading spaces and tabs of a line's text, in bytes.
fn indent_len(text: &str) -> usize {
    text.len() - text.trim_start_matches([' ', '\t']).len()
}

/// The screen column an indentation of spaces and tabs reaches.
fn indent_col(indent: &str, tab_width: usize) -> usize {
    indent.bytes().fold(0, |col, b| {
        if b == b'\t' {
            (col / tab_width + 1) * tab_width
        } else {
            col + 1
        }
    })
}

fn shift(buf: &mut Buffer, view: &mut View, s: &Settings, mode: Mode) {
    let tab_width = s.tab_width.max(1);
    let rope = &buf.rope;
    let before = view.selections.clone();
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(before.len(), 0);
    let mut lands = Vec::with_capacity(before.len());
    // The last line shifted, and the end of the last change (later ones must start there or
    // after it).
    let mut prev_last: Option<usize> = None;
    let mut prev_end = 0;
    let mut new_indent = String::new();
    let spaces = " ".repeat(tab_width);
    for sel in before.iter() {
        let line = walk.line_at(sel.start());
        let (first, line_start) = (line.index, line.start().0);
        if mode == Mode::Tab && !shifts(sel, line) {
            if sel.start().0 < prev_end {
                lands.push(Land::Shifted { stays: None });
                continue;
            }
            let text = if s.insert_spaces {
                let col = line.col(sel.start(), tab_width).0;
                &spaces[..tab_width - col % tab_width]
            } else {
                "\t"
            };
            changes.push(sel.start(), sel.end(), text);
            prev_end = sel.end().0;
            lands.push(Land::Replaced);
            continue;
        }
        let in_indent = sel.start().0 <= line_start + indent_len(line.text());
        lands.push(Land::Shifted {
            stays: (!sel.is_empty() && in_indent).then_some(line_start),
        });
        let single = ends_on(sel, line);
        let from = if prev_last == Some(first) { first + 1 } else { first };
        // Forward only (no lookup of the end's line, which a backward step would redo).
        let mut i = first;
        loop {
            let line = walk.line(i);
            let done = ends_on(sel, line) || line.next().0 >= rope.len();
            let text = line.text();
            let skip = i < from || (text.is_empty() && (mode == Mode::Outdent || !single));
            if !skip
                && let Some((at, end, kept)) = reindent(line, mode, s.insert_spaces, tab_width, &mut new_indent)
                && at >= prev_end
            {
                changes.push(ByteIdx(at), ByteIdx(end), &new_indent[kept..]);
                prev_end = end;
            }
            if done {
                break;
            }
            i += 1;
        }
        prev_last = Some(i);
    }
    if changes.is_empty() {
        buf.seal_undo();
        return;
    }
    let m = changes.mapper();
    let out: Vec<Selection> = before
        .iter()
        .zip(lands)
        .map(|(sel, land)| match land {
            Land::Replaced => Selection::cursor(m.map(sel.end(), Assoc::After)),
            Land::Shifted { .. } if sel.is_empty() => Selection::cursor(m.map(sel.head, Assoc::After)),
            Land::Shifted { stays } => {
                let kept = stays.map(|ls| m.map(ByteIdx(ls), Assoc::Before).0 + (sel.start().0 - ls));
                let mut start = m.map(sel.start(), Assoc::After);
                if let Some(kept) = kept {
                    start = start.min(ByteIdx(kept));
                }
                let end = m.map(sel.end(), Assoc::Before);
                if sel.head < sel.anchor {
                    Selection::new(end, start)
                } else {
                    Selection::new(start, end)
                }
            }
        })
        .collect();
    let after = Selections::with_newest(out, before.primary_index(), before.newest_index());
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::edit;
    use crate::pairs::tests::{setup, show};
    use crate::syntax::Language;

    fn spaces() -> Settings {
        Settings::default()
    }

    fn tabs() -> Settings {
        Settings {
            insert_spaces: false,
            ..Settings::default()
        }
    }

    fn run(cmd: Command, marked: &str, s: &Settings) -> String {
        let (mut b, mut v) = setup(Language::PlainText, marked, false);
        edit::run(cmd, &mut b, &mut v, s);
        show(&b, &v)
    }

    fn indented(marked: &str) -> String {
        run(Command::IndentLines, marked, &spaces())
    }

    fn outdented(marked: &str) -> String {
        run(Command::OutdentLines, marked, &spaces())
    }

    fn tabbed(marked: &str) -> String {
        run(Command::InsertTab, marked, &spaces())
    }

    #[test]
    fn indents_each_touched_line_to_the_next_stop() {
        assert_eq!(indented("a|b"), "    a|b");
        assert_eq!(
            indented("|ab"),
            "    |ab",
            "a cursor at the text's start stays before it"
        );
        assert_eq!(indented("  a|b"), "    a|b", "to the next multiple of the tab width");
        assert_eq!(indented("    |ab"), "        |ab");
        assert_eq!(
            indented("  |  ab"),
            "  |      ab",
            "a cursor in the indentation keeps its place"
        );
        assert_eq!(indented("«a\nb\nc»"), "«    a\n    b\n    c»");
        assert_eq!(indented("«a\n\nb»"), "«    a\n\n    b»", "empty lines aren't indented");
        assert_eq!(
            indented("«a\n  \nb»"),
            "«    a\n    \n    b»",
            "whitespace-only ones are"
        );
        assert_eq!(indented("|"), "    |", "a cursor alone on an empty line indents it");
        assert_eq!(indented("a\n|\nb"), "a\n    |\nb");
        assert_eq!(indented("\t|x"), "        |x", "spaces: tabs normalised");
    }

    #[test]
    fn outdents_to_the_previous_stop() {
        assert_eq!(outdented("    a|b"), "a|b");
        assert_eq!(outdented("  a|b"), "a|b", "fewer spaces than a level");
        assert_eq!(outdented("      a|b"), "    a|b");
        assert_eq!(outdented("        |ab"), "    |ab");
        assert_eq!(outdented("a|b"), "a|b", "nothing to remove");
        assert_eq!(
            outdented("    |    ab"),
            "    |ab",
            "a cursor in the removed part goes to its start"
        );
        assert_eq!(outdented("«    a\n\n  b\nc»"), "«a\n\nb\nc»");
        assert_eq!(outdented("\t\ta|"), "    a|", "spaces: tabs normalised");
        assert_eq!(outdented("   \n|"), "   \n|", "another line's indentation untouched");
        assert_eq!(outdented("  |"), "|", "a whitespace-only line");
    }

    #[test]
    fn tabs_when_not_inserting_spaces() {
        let t = tabs();
        assert_eq!(run(Command::IndentLines, "a|", &t), "\ta|");
        assert_eq!(run(Command::IndentLines, "«\ta\n\t\tb»", &t), "«\t\ta\n\t\t\tb»");
        assert_eq!(
            run(Command::IndentLines, "    a|", &t),
            "\t\ta|",
            "spaces normalised to tabs"
        );
        assert_eq!(run(Command::OutdentLines, "\t\ta|", &t), "\ta|");
        assert_eq!(run(Command::OutdentLines, "  \ta|", &t), "a|");
        assert_eq!(run(Command::InsertTab, "«a\nb»", &t), "«\ta\n\tb»");
        // A Makefile always indents with tabs.
        let (mut b, mut v) = setup(Language::Makefile, "«all:\nx»", false);
        edit::run(Command::IndentLines, &mut b, &mut v, &spaces());
        assert_eq!(show(&b, &v), "«\tall:\n\tx»");
        // The tab width sets the level.
        let two = Settings {
            tab_width: 2,
            ..Settings::default()
        };
        assert_eq!(run(Command::IndentLines, "a|", &two), "  a|");
        assert_eq!(run(Command::OutdentLines, "   a|", &two), "  a|");
    }

    #[test]
    fn selections_keep_their_text() {
        // A start at the line's start (or in its indentation) stays put: the selection grows.
        assert_eq!(indented("«ab\ncd»"), "«    ab\n    cd»");
        assert_eq!(indented("  «ab\ncd»"), "  «  ab\n    cd»");
        assert_eq!(
            indented("a«b\ncd»"),
            "    a«b\n    cd»",
            "a start in the text moves with it"
        );
        assert_eq!(outdented("«    ab\n    cd»"), "«ab\ncd»", "shrinks");
        assert_eq!(outdented("    a«b\n    c»d"), "a«b\nc»d");
        // An end at a later line's start doesn't take that line.
        assert_eq!(indented("«a\nb\n»c"), "«    a\n    b\n»c");
        // An end in the last line's indentation stays before the inserted part.
        assert_eq!(indented("«a\n  »b"), "«    a\n  »  b");
        // Backwards selections stay backwards.
        let (mut b, mut v) = setup(Language::PlainText, "|ab\ncd", false);
        v.selections = Selections::single(Selection::new(ByteIdx(4), ByteIdx(1)));
        edit::run(Command::IndentLines, &mut b, &mut v, &spaces());
        assert_eq!(b.rope.to_string(), "    ab\n    cd");
        let s = v.selections.primary();
        assert_eq!((s.anchor.0, s.head.0), (12, 5));
    }

    #[test]
    fn many_selections_and_shared_lines() {
        assert_eq!(indented("a|b|c\nd|"), "    a|b|c\n    d|", "a line shifts once");
        assert_eq!(indented("«a\nb»«b\nc»"), "«    a\n    b»«b\n    c»");
        assert_eq!(outdented("    |a\n\n  b|\nc|"), "|a\n\nb|\nc|");
        assert_eq!(indented("a\r\n|b\r\n|c"), "a\r\n    |b\r\n    |c", "CRLF");
        assert_eq!(indented("«界\n  é»"), "«    界\n    é»");
    }

    #[test]
    fn tab_indents_selections_over_lines_and_inserts_at_the_others() {
        assert_eq!(tabbed("a|b"), "a   |b", "a cursor: a tab, as before");
        assert_eq!(tabbed("  «a»b"), "    |b", "part of a line: replaced");
        assert_eq!(tabbed("«ab»\ncd"), "«    ab»\ncd", "a whole line: indented");
        assert_eq!(tabbed("«a\nb»"), "«    a\n    b»");
        assert_eq!(tabbed("«a\n\nb»"), "«    a\n\n    b»");
        // Per selection (VS Code): the cursor gets its tab in the same step.
        assert_eq!(tabbed("«a\nb» x|y"), "«    a\n    b» x |y");
        assert_eq!(tabbed("x|y\n«a\nb»"), "x   |y\n«    a\n    b»");
        // A cursor in the indentation the line's shift changes gets no tab of its own.
        assert_eq!(tabbed("«a\n » |  b"), "«    a\n » |      b");
        // Part of a line next to a selection over lines is still replaced by a tab.
        assert_eq!(tabbed("«a\nb» x«y»z"), "«    a\n    b» x |z");
        // A tab inserted before a later selection's first line keeps that line's indentation
        // (its change would come before the insertion): only the selection's other lines shift.
        assert_eq!(tabbed("x|y«z\nw»"), "x   |y«z\n    w»");
    }

    #[test]
    fn nothing_to_do_is_no_step_and_one_step_otherwise() {
        let s = spaces();
        let (mut b, mut v) = setup(Language::PlainText, "a|\nb", false);
        edit::run(Command::OutdentLines, &mut b, &mut v, &s);
        assert!(!b.is_dirty());
        let (mut b, mut v) = setup(Language::PlainText, "«a\n  b»", false);
        edit::run(Command::IndentLines, &mut b, &mut v, &s);
        edit::run(Command::IndentLines, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "«        a\n        b»");
        edit::run(Command::Undo, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "«    a\n    b»");
        edit::run(Command::Undo, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "«a\n  b»");
        edit::run(Command::Redo, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "«    a\n    b»");
    }
}
