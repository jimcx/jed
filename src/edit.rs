//! Movement and editing commands, applied at every selection.

use ropey::Rope;

use crate::buffer::Buffer;
use crate::buffer::change::{Change, ChangeSet};
use crate::buffer::file::Eol;
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::undo::EditKind;
use crate::command::{Command, Motion};
use crate::selection::{Selection, Selections};
use crate::view::View;

#[derive(Debug, Clone)]
pub struct Settings {
    pub tab_width: usize,
    pub insert_spaces: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
        }
    }
}

/// Runs a movement or editing command. Returns false if `cmd` isn't one.
/// The caller checks the buffer is writable before calling with an edit.
pub fn run(cmd: Command, buf: &mut Buffer, view: &mut View, s: &Settings) -> bool {
    match cmd {
        Command::Move { motion, select } => move_cursors(buf, view, motion, select, s),
        Command::DeleteLeft => delete(buf, view, pos::prev_grapheme),
        Command::DeleteRight => delete(buf, view, pos::next_grapheme),
        Command::DeleteWordLeft => delete(buf, view, pos::word_left),
        Command::DeleteWordRight => delete(buf, view, pos::word_right),
        Command::DeleteToLineStart => delete(buf, view, |r, p| {
            let start = pos::line_start(r, pos::line_of(r, p));
            if p == start { pos::prev_grapheme(r, p) } else { start }
        }),
        Command::InsertNewline => insert_newline(buf, view),
        Command::InsertTab => insert_tab(buf, view, s),
        Command::SelectAll => {
            view.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(buf.rope.len())));
            buf.seal_undo();
        }
        Command::CancelSelection => {
            if view.selections.len() > 1 {
                view.selections.keep_primary();
            } else {
                view.selections.transform(|s| Selection::cursor(s.head));
            }
            buf.seal_undo();
        }
        Command::Undo => restore(view, buf.undo()),
        Command::Redo => restore(view, buf.redo()),
        _ => return false,
    }
    true
}

pub fn is_edit(cmd: Command) -> bool {
    matches!(
        cmd,
        Command::DeleteLeft
            | Command::DeleteRight
            | Command::DeleteWordLeft
            | Command::DeleteWordRight
            | Command::DeleteToLineStart
            | Command::InsertNewline
            | Command::InsertTab
            | Command::Undo
            | Command::Redo
    )
}

/// Typed text: one char from a key press.
pub fn insert_char(buf: &mut Buffer, view: &mut View, c: char) {
    let text = c.to_string();
    replace_each(buf, view, EditKind::Insert, |_, sel| {
        (sel.start(), sel.end(), text.clone())
    });
}

/// Pasted text: line endings converted to the buffer's. `whole_line` (from a copy of
/// empty selections) puts it above each empty cursor's line, the cursor staying on its text.
pub fn paste(buf: &mut Buffer, view: &mut View, text: &str, whole_line: bool) {
    let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
    if buf.eol == Eol::Crlf {
        text = text.replace('\n', "\r\n");
    }
    replace_each_landing(buf, view, EditKind::Other, |rope, sel| {
        if whole_line && sel.is_empty() {
            let start = pos::line_start(rope, pos::line_of(rope, sel.head));
            (start, start, text.clone(), Land::Keep(sel.head))
        } else {
            (sel.start(), sel.end(), text.clone(), Land::AfterText)
        }
    });
}

/// What copy puts on the clipboard, newlines as `\n`, and whether it's whole lines.
/// Selections are joined with newlines; if they're all empty, each cursor's line
/// is taken whole with its line break, once per line.
pub fn copy_text(buf: &Buffer, view: &View) -> (String, bool) {
    let rope = &buf.rope;
    if view.selections.iter().all(|s| s.is_empty()) {
        let mut text = String::new();
        let mut last = None;
        for sel in view.selections.iter() {
            let line = pos::line_of(rope, sel.head);
            if last != Some(line) {
                last = Some(line);
                text.push_str(&pos::line_text(rope, line));
                text.push('\n');
            }
        }
        (text, true)
    } else {
        let parts: Vec<String> = view
            .selections
            .iter()
            .map(|s| rope.slice(s.start().0..s.end().0).to_string())
            .collect();
        (parts.join("\n").replace("\r\n", "\n"), false)
    }
}

/// Deletes what `copy_text` takes: the selections, or if they're all empty, each cursor's
/// whole line (the last line takes the line break before it). One undo step.
pub fn cut(buf: &mut Buffer, view: &mut View) {
    let whole_lines = view.selections.iter().all(|s| s.is_empty());
    replace_each(buf, view, EditKind::Other, |rope, sel| {
        if !whole_lines {
            return (sel.start(), sel.end(), String::new());
        }
        let line = pos::line_of(rope, sel.head);
        let (start, end) = if line + 1 < pos::line_count(rope) {
            (pos::line_start(rope, line), pos::line_start(rope, line + 1))
        } else if line > 0 {
            (pos::line_end(rope, line - 1), pos::line_end(rope, line))
        } else {
            (ByteIdx(0), pos::line_end(rope, line))
        };
        (start, end, String::new())
    });
}

fn restore(view: &mut View, sel: Option<Selections>) {
    if let Some(sel) = sel {
        view.selections = sel;
        view.reveal_cursor = true;
    }
}

fn move_cursors(buf: &mut Buffer, view: &mut View, motion: Motion, select: bool, s: &Settings) {
    let rope = &buf.rope;
    let page = view.height.max(1) as isize;
    view.selections.transform(|sel| {
        if !select && !sel.is_empty() {
            match motion {
                Motion::Left => return Selection::cursor(sel.start()),
                Motion::Right => return Selection::cursor(sel.end()),
                _ => {}
            }
        }
        let (head, goal) = match motion {
            Motion::Left => (pos::prev_grapheme(rope, sel.head), None),
            Motion::Right => (pos::next_grapheme(rope, sel.head), None),
            Motion::Up => vertical(rope, sel, -1, s.tab_width),
            Motion::Down => vertical(rope, sel, 1, s.tab_width),
            Motion::PageUp => vertical(rope, sel, -page, s.tab_width),
            Motion::PageDown => vertical(rope, sel, page, s.tab_width),
            Motion::WordLeft => (pos::word_left(rope, sel.head), None),
            Motion::WordRight => (pos::word_right(rope, sel.head), None),
            Motion::LineStart => {
                let line = pos::line_of(rope, sel.head);
                let first = pos::first_non_blank(rope, line);
                (
                    if sel.head == first {
                        pos::line_start(rope, line)
                    } else {
                        first
                    },
                    None,
                )
            }
            Motion::LineEnd => (pos::line_end(rope, pos::line_of(rope, sel.head)), None),
            Motion::DocStart => (ByteIdx(0), None),
            Motion::DocEnd => (ByteIdx(rope.len()), None),
        };
        let anchor = if select { sel.anchor } else { head };
        Selection { anchor, head, goal }
    });
    match motion {
        Motion::PageUp => view.scroll_lines(rope, -page),
        Motion::PageDown => view.scroll_lines(rope, page),
        _ => {}
    }
    view.reveal_cursor = true;
    buf.seal_undo();
}

/// Up/down by `delta` lines, keeping the remembered column.
/// Past the first line goes to the start of the file, past the last to the end.
fn vertical(rope: &Rope, sel: &Selection, delta: isize, tab_width: usize) -> (ByteIdx, Option<pos::DisplayCol>) {
    let goal = sel.goal.unwrap_or_else(|| pos::display_col(rope, sel.head, tab_width));
    let line = pos::line_of(rope, sel.head);
    let last = pos::line_count(rope) - 1;
    match line.checked_add_signed(delta) {
        None if line == 0 => (ByteIdx(0), None),
        Some(t) if t > last && line == last => (ByteIdx(rope.len()), None),
        target => {
            let target = target.unwrap_or(0).min(last);
            (pos::byte_at_col(rope, target, goal, tab_width), Some(goal))
        }
    }
}

/// Deletes each selection, or for an empty one, the range from `to(cursor)` to the cursor.
fn delete(buf: &mut Buffer, view: &mut View, to: impl Fn(&Rope, ByteIdx) -> ByteIdx) {
    replace_each(buf, view, EditKind::Delete, |rope, sel| {
        if sel.is_empty() {
            let other = to(rope, sel.head);
            (sel.head.min(other), sel.head.max(other), String::new())
        } else {
            (sel.start(), sel.end(), String::new())
        }
    });
}

fn insert_newline(buf: &mut Buffer, view: &mut View) {
    let eol = buf.eol.as_str();
    replace_each(buf, view, EditKind::Insert, |rope, sel| {
        // Carry over the current line's indentation, up to the cursor.
        let line = pos::line_of(rope, sel.start());
        let line_start = pos::line_start(rope, line);
        let text = pos::line_text(rope, line);
        let upto = (sel.start().0 - line_start.0).min(text.len());
        let indent: String = text[..upto].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        (sel.start(), sel.end(), format!("{eol}{indent}"))
    });
}

fn insert_tab(buf: &mut Buffer, view: &mut View, s: &Settings) {
    replace_each(buf, view, EditKind::Insert, |rope, sel| {
        let text = if s.insert_spaces {
            let col = pos::display_col(rope, sel.start(), s.tab_width).0;
            " ".repeat(s.tab_width - col % s.tab_width)
        } else {
            "\t".to_string()
        };
        (sel.start(), sel.end(), text)
    });
}

/// Where a cursor ends up after its selection's change.
enum Land {
    AfterText,
    /// Stays on the same text: a position at or after the change, before the edit.
    Keep(ByteIdx),
}

/// One transaction: replace a range per selection, cursors land after the new text.
fn replace_each(
    buf: &mut Buffer,
    view: &mut View,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> (ByteIdx, ByteIdx, String),
) {
    replace_each_landing(buf, view, kind, |rope, sel| {
        let (start, end, text) = f(rope, sel);
        (start, end, text, Land::AfterText)
    });
}

/// `replace_each`, with each cursor's landing chosen per selection.
fn replace_each_landing(
    buf: &mut Buffer,
    view: &mut View,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> (ByteIdx, ByteIdx, String, Land),
) {
    let before = view.selections.clone();
    let mut changes = Vec::with_capacity(before.len());
    let mut lands = Vec::with_capacity(before.len());
    let mut prev_end = ByteIdx(0);
    for sel in before.iter() {
        let (start, end, text, land) = f(&buf.rope, sel);
        // Expanded ranges (e.g. backspace next to a selection) can overlap; clip them.
        let start = start.max(prev_end);
        let end = end.max(start);
        prev_end = end;
        changes.push(Change { start, end, text });
        lands.push(land);
    }

    let mut delta: isize = 0;
    let cursors = changes
        .iter()
        .zip(lands)
        .map(|(c, land)| {
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
            let at = match land {
                Land::AfterText => c.end.0,
                Land::Keep(p) => p.0.max(c.end.0),
            };
            Selection::cursor(ByteIdx((at as isize + delta) as usize))
        })
        .collect();
    let after = Selections::with_newest(cursors, before.primary_index(), before.newest_index());

    buf.edit(ChangeSet::new(changes), before, after.clone(), kind);
    view.selections = after;
    view.reveal_cursor = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command as C;

    fn setup(text: &str, cursor: usize) -> (Buffer, View, Settings) {
        let buf = Buffer::from_str(text);
        let mut view = View::new();
        view.selections = Selections::single(Selection::cursor(ByteIdx(cursor)));
        (buf, view, Settings::default())
    }

    fn head(view: &View) -> usize {
        view.selections.primary().head.0
    }

    fn mv(motion: Motion) -> Command {
        C::Move { motion, select: false }
    }

    #[test]
    fn typing_groups_into_one_undo_step() {
        let (mut b, mut v, s) = setup("", 0);
        for c in "hello".chars() {
            insert_char(&mut b, &mut v, c);
        }
        assert_eq!(b.rope.to_string(), "hello");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "");
        assert!(!b.is_dirty());
        run(C::Redo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "hello");
        assert_eq!(head(&v), 5);
    }

    #[test]
    fn cursor_move_and_kind_switch_start_new_steps() {
        let (mut b, mut v, s) = setup("", 0);
        insert_char(&mut b, &mut v, 'a');
        insert_char(&mut b, &mut v, 'b');
        run(C::DeleteLeft, &mut b, &mut v, &s); // kind switch
        run(mv(Motion::Left), &mut b, &mut v, &s); // cursor move
        insert_char(&mut b, &mut v, 'c');
        assert_eq!(b.rope.to_string(), "ca");

        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "");
    }

    #[test]
    fn paste_is_its_own_step_and_converts_eol() {
        let (mut b, mut v, s) = setup("x\r\n", 0);
        insert_char(&mut b, &mut v, 'a');
        paste(&mut b, &mut v, "1\n2", false);
        insert_char(&mut b, &mut v, 'b');
        assert_eq!(b.rope.to_string(), "a1\r\n2bx\r\n");
        run(C::Undo, &mut b, &mut v, &s);
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ax\r\n");
    }

    #[test]
    fn newline_uses_file_eol_and_keeps_indent() {
        let (mut b, mut v, s) = setup("    foo\r\n", 7);
        run(C::InsertNewline, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "    foo\r\n    \r\n");
        assert_eq!(head(&v), 13);
    }

    #[test]
    fn up_down_keep_goal_column() {
        let (mut b, mut v, s) = setup("abcdef\nab\nabcdef", 5);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 9);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 15);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 16);
        run(mv(Motion::Up), &mut b, &mut v, &s);
        assert_eq!(head(&v), 9, "goal column was reset by moving to the end");
    }

    #[test]
    fn backspace_deletes_whole_crlf_and_grapheme() {
        let (mut b, mut v, s) = setup("a\r\ne\u{301}", 6);
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a\r\n");
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a");
    }

    #[test]
    fn edits_apply_at_every_cursor() {
        let (mut b, mut v, s) = setup("ab\ncd\nef", 0);
        v.selections = Selections::new((0..3).map(|l| Selection::cursor(ByteIdx(l * 3 + 1))).collect(), 0);
        insert_char(&mut b, &mut v, 'X');
        assert_eq!(b.rope.to_string(), "aXb\ncXd\neXf");
        run(C::DeleteLeft, &mut b, &mut v, &s);
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "b\nd\nf");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "aXb\ncXd\neXf");
        assert_eq!(v.selections.len(), 3);
    }

    #[test]
    fn copy_joins_selections_and_takes_whole_lines_when_empty() {
        let (b, mut v, _) = setup("ab\r\ncd\r\nef", 0);
        v.selections = Selections::new(
            vec![
                Selection::new(ByteIdx(1), ByteIdx(5)),
                Selection::new(ByteIdx(8), ByteIdx(9)),
            ],
            0,
        );
        assert_eq!(copy_text(&b, &v), ("b\nc\ne".to_string(), false));
        // Two cursors on line 0 copy it once; the last line gets a line break added.
        v.selections = Selections::new([0, 1, 9].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        assert_eq!(copy_text(&b, &v), ("ab\nef\n".to_string(), true));
    }

    #[test]
    fn cut_whole_lines_like_vs_code() {
        // Middle line: the line and its break go, cursor to the start of the next line.
        let (mut b, mut v, s) = setup("ab\ncd\nef", 4);
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "ab\nef");
        assert_eq!(head(&v), 3);
        // Last line: takes the break before it, cursor to the end of the line above.
        v.selections = Selections::single(Selection::cursor(ByteIdx(4)));
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "ab");
        assert_eq!(head(&v), 2);
        // Only line: emptied.
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "");
        // Each cut is one undo step.
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab");
        run(C::Undo, &mut b, &mut v, &s);
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab\ncd\nef");
        assert_eq!(head(&v), 4);
    }

    #[test]
    fn cut_selection_leaves_empty_cursors_alone() {
        let (mut b, mut v, _) = setup("abc\ndef", 0);
        v.selections = Selections::new(
            vec![Selection::new(ByteIdx(0), ByteIdx(2)), Selection::cursor(ByteIdx(5))],
            0,
        );
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "c\ndef");
    }

    #[test]
    fn whole_line_paste_goes_above_and_keeps_cursor_on_its_text() {
        let (mut b, mut v, s) = setup("ab\r\ncd", 6);
        paste(&mut b, &mut v, "x\n", true);
        assert_eq!(b.rope.to_string(), "ab\r\nx\r\ncd");
        assert_eq!(head(&v), 9, "still after 'c'");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab\r\ncd");
        // With a selection, a whole-line paste just replaces it.
        v.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(2)));
        paste(&mut b, &mut v, "x\n", true);
        assert_eq!(b.rope.to_string(), "x\r\n\r\ncd");
    }

    #[test]
    fn home_toggles_first_non_blank() {
        let (mut b, mut v, s) = setup("    foo", 6);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 4);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0);
    }
}
