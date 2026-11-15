//! Line commands at every selection: move lines up / down (Alt+Up / Down) and copy lines up /
//! down (Shift+Alt+Up / Down), VS Code's `MoveLinesCommand` and `CopyLinesCommand`.
//!
//! Each runs as one undo step. Selections stay on the moved (or, copying down, the new lower)
//! text. Line breaks are the file's own (CRLF, LF, lone CR); the last line needs none.

use std::ops::Range;

use ropey::Rope;

use crate::buffer::Buffer;
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx, LineWalker};
use crate::buffer::undo::EditKind;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// Lines `first..=last`, where they are, and the selections (indices into the sorted list) on
/// them.
struct Block {
    first: usize,
    last: usize,
    /// Start of `first`.
    start: usize,
    /// End of `last`'s content, and the start of the line after it (the text's end if none).
    end: usize,
    next: usize,
    sels: Range<usize>,
}

/// Groups the sorted selections into blocks of lines: selections sharing a line join one block,
/// and with `touching`, so do selections on neighbouring lines. A non-empty selection that ends
/// at a line start doesn't take that line (VS Code): selecting whole lines with Shift+Down moves
/// just those. Lines are looked up only for a selection reaching past the block so far (at 60k
/// selections the lookups are most of the cost).
fn blocks(rope: &Rope, sels: &[Selection], touching: bool) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut walk = LineWalker::new(rope);
    for (i, s) in sels.iter().enumerate() {
        let line = walk.line_at(s.start());
        let (first, start) = (line.index, line.start().0);
        // Ends on this line, or (non-empty) at the next one's start, which it doesn't take.
        let line = if line.contains(s.end()) || (!s.is_empty() && s.end() == line.next()) {
            line
        } else {
            let end = walk.line_at(s.end());
            let (index, at_start) = (end.index, end.start() == s.end());
            walk.line(index - usize::from(at_start))
        };
        let (last, end, next) = (line.index, line.end().0, line.next().0);
        match out.last_mut() {
            Some(b) if first <= b.last + usize::from(touching) => {
                if last > b.last {
                    (b.last, b.end, b.next) = (last, end, next);
                }
                b.sels.end = i + 1;
            }
            _ => out.push(Block {
                first,
                last,
                start,
                end,
                next,
                sels: i..i + 1,
            }),
        }
    }
    out
}

fn push_slice(out: &mut String, rope: &Rope, r: Range<usize>) {
    rope.slice(r).chunks().for_each(|c| out.push_str(c));
}

fn shift(s: &Selection, f: impl Fn(ByteIdx) -> ByteIdx) -> Selection {
    Selection {
        anchor: f(s.anchor),
        head: f(s.head),
        goal: s.goal,
    }
}

/// Alt+Up / Down: each block of lines swaps with the line above / below it. A block already at
/// the top / bottom stays; if none can move, nothing changes (no undo step).
pub fn move_lines(buf: &mut Buffer, view: &mut View, up: bool) {
    let rope = &buf.rope;
    let last_line = pos::line_count(rope) - 1;
    let before = view.selections.clone();
    let sels = before.as_slice();
    let blocks = blocks(rope, sels, true);
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(blocks.len(), 0);
    let mut out = Vec::with_capacity(sels.len());
    // The swapped region copied once, then reassembled into `text`.
    let (mut region, mut text) = (String::new(), String::new());
    for b in blocks {
        let moved = &sels[b.sels.clone()];
        let (x_start, x_end) = (b.start, b.end);
        text.clear();
        if up && b.first > 0 {
            // "P brk X" becomes "X brk P": the block's own last line break stays put.
            let p = walk.line(b.first - 1);
            let (p_start, p_end) = (p.start().0, p.end().0);
            let brk = p_end..x_start;
            let after = p_start + (x_end - x_start) + brk.len();
            region.clear();
            push_slice(&mut region, rope, p_start..x_end);
            let at = |r: Range<usize>| &region[r.start - p_start..r.end - p_start];
            text.extend([at(x_start..x_end), at(brk), at(p_start..p_end)]);
            changes.push(ByteIdx(p_start), ByteIdx(x_end), &text);
            let d = x_start - p_start;
            // A selection ending at the next line's start ends at the start of P now.
            out.extend(
                moved
                    .iter()
                    .map(|s| shift(s, |p| ByteIdx(if p.0 <= x_end { p.0 - d } else { after }))),
            );
        } else if !up && b.last < last_line {
            // "X brk N" becomes "N brk X".
            let n = walk.line(b.last + 1);
            let (n_end, after) = (n.end().0, n.next().0);
            let brk = x_end..b.next;
            region.clear();
            push_slice(&mut region, rope, x_start..n_end);
            let at = |r: Range<usize>| &region[r.start - x_start..r.end - x_start];
            text.extend([at(b.next..n_end), at(brk), at(x_start..x_end)]);
            changes.push(ByteIdx(x_start), ByteIdx(n_end), &text);
            let e = n_end - x_end;
            out.extend(
                moved
                    .iter()
                    .map(|s| shift(s, |p| ByteIdx(if p.0 <= x_end { p.0 + e } else { after }))),
            );
        } else {
            out.extend_from_slice(moved);
        }
    }
    finish(buf, view, before, out, changes);
}

/// Shift+Alt+Up / Down: each block of lines (selections sharing a line) is duplicated. Copying
/// up, the selections stay on the upper copy; copying down, they go to the lower one.
pub fn copy_lines(buf: &mut Buffer, view: &mut View, down: bool) {
    let rope = &buf.rope;
    let eol = buf.eol.as_str();
    let before = view.selections.clone();
    let sels = before.as_slice();
    let blocks = blocks(rope, sels, false);
    let mut changes = ChangeSet::with_capacity(blocks.len(), 0);
    let mut out = Vec::with_capacity(sels.len());
    let (mut region, mut text) = (String::new(), String::new());
    // How far everything after the previous blocks' inserts has moved.
    let mut delta = 0;
    for b in blocks {
        let (x_start, x_end) = (b.start, b.end);
        text.clear();
        // The lines and their break, in one copy; the last line has no break of its own, so
        // the copy is joined with the file's.
        region.clear();
        push_slice(&mut region, rope, x_start..b.next);
        if b.next == x_end {
            region.push_str(eol);
        }
        let (lines, brk) = region.split_at(x_end - x_start);
        if down {
            text.extend([brk, lines]);
            changes.push(ByteIdx(x_end), ByteIdx(x_end), &text);
        } else {
            text.extend([lines, brk]);
            changes.push(ByteIdx(x_start), ByteIdx(x_start), &text);
        }
        let by = delta + if down { text.len() } else { 0 };
        out.extend(sels[b.sels].iter().map(|s| shift(s, |p| ByteIdx(p.0 + by))));
        delta += text.len();
    }
    finish(buf, view, before, out, changes);
}

fn finish(buf: &mut Buffer, view: &mut View, before: Selections, out: Vec<Selection>, changes: ChangeSet) {
    let (primary, newest) = (before.primary_index(), before.newest_index());
    finish_with(buf, view, before, out, changes, primary, newest);
}

/// `finish` for commands whose result has a different number of selections than before.
fn finish_with(
    buf: &mut Buffer,
    view: &mut View,
    before: Selections,
    out: Vec<Selection>,
    changes: ChangeSet,
    primary: usize,
    newest: usize,
) {
    if changes.is_empty() {
        buf.seal_undo();
        return;
    }
    let after = Selections::with_newest(out, primary, newest);
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
}

/// Cmd+Shift+K: deletes the lines of every selection (a block of neighbouring lines at once).
/// Each block leaves a cursor at the same column of the line that followed it (of the one
/// before for the last line, whose preceding break goes too).
pub fn delete_lines(buf: &mut Buffer, view: &mut View, tab_width: usize) {
    let rope = &buf.rope;
    let last_line = pos::line_count(rope) - 1;
    let before = view.selections.clone();
    let sels = before.as_slice();
    let blocks = blocks(rope, sels, true);
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(blocks.len(), 0);
    let mut out = Vec::with_capacity(blocks.len());
    let (mut primary, mut newest) = (0, 0);
    let (p, n) = (before.primary_index(), before.newest_index());
    // Bytes removed by the blocks so far.
    let mut removed = 0;
    for b in blocks {
        let head = sels[b.sels.end - 1].head;
        let col = walk.line_at(head).col(head, tab_width);
        let (cut_start, land) = if b.last < last_line {
            let at = walk.line(b.last + 1).byte_at_col(col, tab_width).0;
            (b.start, at - (b.next - b.start) - removed)
        } else if b.first > 0 {
            let prev = walk.line(b.first - 1);
            (prev.end().0, prev.byte_at_col(col, tab_width).0 - removed)
        } else {
            (b.start, 0)
        };
        changes.push(ByteIdx(cut_start), ByteIdx(b.next), "");
        removed += b.next - cut_start;
        if b.sels.contains(&p) {
            primary = out.len();
        }
        if b.sels.contains(&n) {
            newest = out.len();
        }
        out.push(Selection::cursor(ByteIdx(land)));
    }
    finish_with(buf, view, before, out, changes, primary, newest);
}

/// Cmd+L: each selection becomes the whole lines it touches, line break included. One already
/// covering whole lines grows by the next line. A selection that starts mid-line and ends at a
/// line start doesn't take that last line.
pub fn select_lines(rope: &Rope, view: &mut View) {
    let mut walk = LineWalker::new(rope);
    view.selections.transform(|s| {
        let first = walk.line_at(s.start());
        let (start, whole_start) = (first.start(), first.start() == s.start());
        let end = walk.line_at(s.end());
        let head = if !s.is_empty() && end.start() == s.end() && !whole_start {
            end.start()
        } else {
            end.next()
        };
        Selection::new(start, head)
    });
    view.reveal_cursor = true;
}

/// Ctrl+J: joins each selection's lines with the next one (a cursor, or a selection within one
/// line) or all the lines it spans: the break and the next line's leading whitespace become one
/// space, none if either side is empty or the next line starts with `)`. The selections of a
/// block become one cursor at its last join.
pub fn join_lines(buf: &mut Buffer, view: &mut View) {
    let rope = &buf.rope;
    let last_line = pos::line_count(rope) - 1;
    let before = view.selections.clone();
    let sels = before.as_slice();
    let blocks = blocks(rope, sels, true);
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(blocks.len(), 0);
    let mut out = Vec::with_capacity(sels.len());
    let (mut primary, mut newest) = (0, 0);
    let (p, n) = (before.primary_index(), before.newest_index());
    let mut removed = 0;
    for b in blocks {
        let single = sels[b.sels.end - 1].start().0 >= walk.line(b.last).start().0;
        let hi = (b.last + usize::from(single)).min(last_line);
        if hi <= b.first {
            // Nothing to join: the selections just shift.
            if b.sels.contains(&p) {
                primary = out.len() + p - b.sels.start;
            }
            if b.sels.contains(&n) {
                newest = out.len() + n - b.sels.start;
            }
            out.extend(
                sels[b.sels.clone()]
                    .iter()
                    .map(|s| shift(s, |p| ByteIdx(p.0 - removed))),
            );
            continue;
        }
        let first = walk.line(b.first);
        let (mut cur_end, mut blank) = (first.end().0, first.text().is_empty());
        let mut ends_ws = first.text().ends_with([' ', '\t']);
        let mut join_at = 0;
        for k in b.first + 1..=hi {
            let l = walk.line(k);
            let text = l.text();
            let trimmed = text.trim_start_matches([' ', '\t']);
            let to = l.start().0 + text.len() - trimmed.len();
            let space = !trimmed.is_empty() && !blank && !ends_ws && !trimmed.starts_with(')');
            changes.push(ByteIdx(cur_end), ByteIdx(to), if space { " " } else { "" });
            join_at = cur_end - removed;
            removed += to - cur_end - usize::from(space);
            cur_end = l.end().0;
            if !trimmed.is_empty() {
                blank = false;
                ends_ws = text.ends_with([' ', '\t']);
            }
        }
        if b.sels.contains(&p) {
            primary = out.len();
        }
        if b.sels.contains(&n) {
            newest = out.len();
        }
        out.push(Selection::cursor(ByteIdx(join_at)));
    }
    finish_with(buf, view, before, out, changes, primary, newest);
}

/// Cmd+Enter, step one: every selection becomes a cursor at the end of the last line it
/// touches (a selection ending at a line start doesn't take that line), for Enter to run.
pub fn to_line_ends(rope: &Rope, view: &mut View) {
    let mut walk = LineWalker::new(rope);
    view.selections.transform(|s| {
        let end = walk.line_at(s.end());
        let at = if !s.is_empty() && end.start() == s.end() && end.index > 0 {
            let index = end.index - 1;
            walk.line(index).end()
        } else {
            end.end()
        };
        Selection::cursor(at)
    });
}

/// Cmd+Enter with auto-indent off, after `to_line_ends`: a line break and a copy of the line's
/// indentation at every cursor (what VS Code does whatever the setting).
pub fn insert_line_below_keep_indent(buf: &mut Buffer, view: &mut View) {
    let rope = &buf.rope;
    let eol = buf.eol.as_str();
    let before = view.selections.clone();
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(before.len(), 0);
    let mut out = Vec::with_capacity(before.len());
    let mut text = String::new();
    let mut delta = 0;
    for s in before.iter() {
        let line = walk.line_at(s.head);
        let content = line.text();
        let indent = &content[..content.len() - content.trim_start_matches([' ', '\t']).len()];
        text.clear();
        text.extend([eol, indent]);
        changes.push(s.head, s.head, &text);
        delta += text.len();
        out.push(Selection::cursor(ByteIdx(s.head.0 + delta)));
    }
    finish(buf, view, before, out, changes);
}

/// Cmd+Shift+Enter: a new line above each selection's first line, with that line's indentation,
/// and a cursor on it.
pub fn insert_line_above(buf: &mut Buffer, view: &mut View) {
    let rope = &buf.rope;
    let eol = buf.eol.as_str();
    let before = view.selections.clone();
    let sels = before.as_slice();
    let mut walk = LineWalker::new(rope);
    let mut changes = ChangeSet::with_capacity(sels.len(), 0);
    let mut out = Vec::with_capacity(sels.len());
    let mut text = String::new();
    // Bytes inserted so far, and the line last given a new one above (cursors sharing a line).
    let mut delta = 0;
    let mut done: Option<(usize, usize)> = None;
    for s in sels {
        let line = walk.line_at(s.start());
        if let Some((index, land)) = done
            && index == line.index
        {
            out.push(Selection::cursor(ByteIdx(land)));
            continue;
        }
        let start = line.start().0;
        let content = line.text();
        let indent = &content[..content.len() - content.trim_start_matches([' ', '\t']).len()];
        text.clear();
        text.extend([indent, eol]);
        changes.push(ByteIdx(start), ByteIdx(start), &text);
        let land = start + delta + indent.len();
        done = Some((line.index, land));
        delta += text.len();
        out.push(Selection::cursor(ByteIdx(land)));
    }
    finish(buf, view, before, out, changes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(text: &str, sels: Vec<(usize, usize)>) -> (Buffer, View) {
        let buf = Buffer::from_str(text);
        let mut view = View::new();
        let list = sels
            .into_iter()
            .map(|(a, h)| Selection::new(ByteIdx(a), ByteIdx(h)))
            .collect();
        view.selections = Selections::new(list, 0);
        (buf, view)
    }

    fn state(b: &Buffer, v: &View) -> (String, Vec<(usize, usize)>) {
        let r = v.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        (b.rope.to_string(), r)
    }

    fn st(text: &str, r: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
        (text.to_string(), r.to_vec())
    }

    #[test]
    fn move_line_up_and_down_keeps_the_cursor_on_its_text() {
        let (mut b, mut v) = setup("one\ntwo\nthree", vec![(5, 5)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("two\none\nthree", &[(1, 1)]));
        move_lines(&mut b, &mut v, false);
        move_lines(&mut b, &mut v, false);
        assert_eq!(
            state(&b, &v),
            st("one\nthree\ntwo", &[(11, 11)]),
            "onto the last line, no break"
        );
        move_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("one\nthree\ntwo", &[(11, 11)]), "already last");
        // Each move was its own undo step (consecutive moves don't join).
        b.undo();
        assert_eq!(b.rope.to_string(), "one\ntwo\nthree");
        b.undo();
        assert_eq!(b.rope.to_string(), "two\none\nthree");
        b.undo();
        assert_eq!(b.rope.to_string(), "one\ntwo\nthree");
        assert!(b.undo().is_none());
    }

    #[test]
    fn whole_line_selection_with_mixed_line_breaks() {
        // "b\n" selected whole, below a CRLF line: it ends at the start of the line after it,
        // which after the move is "a", past the CRLF (not inside it).
        let (mut b, mut v) = setup("a\r\nb\nc", vec![(3, 5)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("b\r\na\nc", &[(0, 3)]));
        move_lines(&mut b, &mut v, false);
        assert_eq!(
            state(&b, &v),
            st("a\r\nb\nc", &[(3, 5)]),
            "the breaks stay where they were"
        );
    }

    #[test]
    fn delete_join_and_insert_above_with_crlf() {
        let (mut b, mut v) = setup("a\r\nb\r\nc", vec![(0, 0)]);
        delete_lines(&mut b, &mut v, 4);
        assert_eq!(state(&b, &v), st("b\r\nc", &[(0, 0)]));
        let (mut b, mut v) = setup("a  \r\n  b\r\nc", vec![(0, 0)]);
        join_lines(&mut b, &mut v);
        assert_eq!(
            state(&b, &v),
            st("a  b\r\nc", &[(3, 3)]),
            "no space after trailing blanks"
        );
        let (mut b, mut v) = setup(" a\r\nb", vec![(2, 2)]);
        insert_line_above(&mut b, &mut v);
        assert_eq!(state(&b, &v), st(" \r\n a\r\nb", &[(1, 1)]));
    }

    #[test]
    fn move_at_the_top_adds_no_undo_step() {
        let (mut b, mut v) = setup("one\ntwo", vec![(1, 1)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("one\ntwo", &[(1, 1)]));
        assert!(b.undo().is_none());
        assert!(!b.is_dirty());
    }

    #[test]
    fn move_crlf_lines_and_the_last_line_without_a_break() {
        let (mut b, mut v) = setup("a\r\nbb\r\ncc", vec![(8, 8)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("a\r\ncc\r\nbb", &[(4, 4)]));
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("cc\r\na\r\nbb", &[(1, 1)]));
    }

    #[test]
    fn whole_line_selection_moves_just_its_lines() {
        // Lines 1-2 selected, ending at the start of line 3.
        let (mut b, mut v) = setup("a\nb\nc\nd\n", vec![(2, 6)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("b\nc\na\nd\n", &[(0, 4)]));
        move_lines(&mut b, &mut v, false);
        move_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("a\nd\nb\nc\n", &[(4, 8)]));
        move_lines(&mut b, &mut v, false);
        assert_eq!(
            state(&b, &v),
            st("a\nd\n\nb\nc", &[(5, 8)]),
            "past the empty last line: the block ends the file now"
        );
    }

    #[test]
    fn neighbouring_cursors_move_as_one_block_and_others_alone() {
        // Cursors on lines 1, 2 (a block) and 4; line 0 is above the block, line 3 above 4.
        let (mut b, mut v) = setup("0\n1\n2\n3\n4", vec![(2, 2), (4, 4), (8, 8)]);
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("1\n2\n0\n4\n3", &[(0, 0), (2, 2), (6, 6)]));
        // Again: the block is at the top and stays; the lone cursor still moves.
        move_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("1\n2\n4\n0\n3", &[(0, 0), (2, 2), (4, 4)]));
        // Down: they now form one block of three lines.
        move_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("0\n1\n2\n4\n3", &[(2, 2), (4, 4), (6, 6)]));
    }

    #[test]
    fn backward_selection_keeps_its_direction() {
        let (mut b, mut v) = setup("ab\ncd\nef", vec![(5, 3)]);
        move_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("ab\nef\ncd", &[(8, 6)]));
    }

    #[test]
    fn copy_down_moves_the_selection_to_the_copy() {
        let (mut b, mut v) = setup("ab\ncd", vec![(1, 1)]);
        copy_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("ab\nab\ncd", &[(4, 4)]));
        b.undo();
        assert_eq!(b.rope.to_string(), "ab\ncd");
    }

    #[test]
    fn copy_up_keeps_the_selection_on_the_upper_copy() {
        let (mut b, mut v) = setup("ab\ncd", vec![(4, 4)]);
        copy_lines(&mut b, &mut v, false);
        assert_eq!(
            state(&b, &v),
            st("ab\ncd\ncd", &[(4, 4)]),
            "last line: joined with the file's break"
        );
    }

    #[test]
    fn copy_crlf_and_several_blocks() {
        // Two cursors on line 0 (one block), one on line 2.
        let (mut b, mut v) = setup("ab\r\ncd\r\nef", vec![(0, 0), (2, 2), (9, 9)]);
        copy_lines(&mut b, &mut v, true);
        assert_eq!(
            state(&b, &v),
            st("ab\r\nab\r\ncd\r\nef\r\nef", &[(4, 4), (6, 6), (17, 17)])
        );
        // Neighbouring lines copy separately (VS Code), not as one block.
        let (mut b, mut v) = setup("a\nb", vec![(0, 0), (2, 2)]);
        copy_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("a\na\nb\nb", &[(0, 0), (4, 4)]));
    }

    #[test]
    fn copy_whole_line_selection() {
        let (mut b, mut v) = setup("a\nb\nc", vec![(2, 4)]);
        copy_lines(&mut b, &mut v, true);
        assert_eq!(state(&b, &v), st("a\nb\nb\nc", &[(4, 6)]));
        let (mut b, mut v) = setup("a\nb\nc", vec![(2, 4)]);
        copy_lines(&mut b, &mut v, false);
        assert_eq!(state(&b, &v), st("a\nb\nb\nc", &[(2, 4)]));
    }
}
