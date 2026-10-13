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
        Command::Save | Command::Quit => return false,
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

/// Pasted text: line endings converted to the buffer's.
pub fn paste(buf: &mut Buffer, view: &mut View, text: &str) {
    let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
    if buf.eol == Eol::Crlf {
        text = text.replace('\n', "\r\n");
    }
    replace_each(buf, view, EditKind::Other, |_, sel| {
        (sel.start(), sel.end(), text.clone())
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

/// One transaction: replace a range per selection, cursors land after the new text.
fn replace_each(
    buf: &mut Buffer,
    view: &mut View,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> (ByteIdx, ByteIdx, String),
) {
    let before = view.selections.clone();
    let mut changes = Vec::with_capacity(before.len());
    let mut prev_end = ByteIdx(0);
    for sel in before.iter() {
        let (start, end, text) = f(&buf.rope, sel);
        // Expanded ranges (e.g. backspace next to a selection) can overlap; clip them.
        let start = start.max(prev_end);
        let end = end.max(start);
        prev_end = end;
        changes.push(Change { start, end, text });
    }

    let mut delta: isize = 0;
    let cursors = changes
        .iter()
        .map(|c| {
            let at = (c.start.0 as isize + delta) as usize + c.text.len();
            delta += c.text.len() as isize - (c.end.0 - c.start.0) as isize;
            Selection::cursor(ByteIdx(at))
        })
        .collect();
    let after = Selections::new(cursors, before.primary_index());

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
        paste(&mut b, &mut v, "1\n2");
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
    fn home_toggles_first_non_blank() {
        let (mut b, mut v, s) = setup("    foo", 6);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 4);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0);
    }
}
