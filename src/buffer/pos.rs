//! Text position types and every conversion between them.
//!
//! `ByteIdx` is the canonical unit. Nothing outside this module converts
//! between bytes, lines, graphemes and screen columns.

use std::borrow::Cow;

use ropey::{LineType, Rope};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Line breaks: LF, CRLF and lone CR.
pub const LINE_TYPE: LineType = LineType::LF_CR;

/// UTF-8 byte offset into a buffer. Always on a char boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteIdx(pub usize);

/// Screen column: tabs expanded, wide chars count 2, a grapheme cluster is one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DisplayCol(pub usize);

pub fn line_count(rope: &Rope) -> usize {
    rope.len_lines(LINE_TYPE)
}

pub fn line_of(rope: &Rope, pos: ByteIdx) -> usize {
    rope.byte_to_line_idx(pos.0, LINE_TYPE)
}

pub fn line_start(rope: &Rope, line: usize) -> ByteIdx {
    ByteIdx(rope.line_to_byte_idx(line, LINE_TYPE))
}

/// End of the line's content, before its line break.
pub fn line_end(rope: &Rope, line: usize) -> ByteIdx {
    let start = rope.line_to_byte_idx(line, LINE_TYPE);
    let slice = rope.line(line, LINE_TYPE);
    ByteIdx(start + slice.trailing_line_break_idx(LINE_TYPE).unwrap_or(slice.len()))
}

/// The line's content without its line break.
pub fn line_text(rope: &Rope, line: usize) -> String {
    rope.slice(line_start(rope, line).0..line_end(rope, line).0).to_string()
}

/// How a grapheme is drawn: the text to put on screen and its width.
/// Rendering and column maths both go through this, so they always agree.
pub fn display_grapheme(g: &str, col: DisplayCol, tab_width: usize) -> (Cow<'_, str>, usize) {
    if g == "\t" {
        let w = tab_width - col.0 % tab_width;
        return (Cow::Owned(" ".repeat(w)), w);
    }
    let first = g.chars().next().unwrap_or(' ');
    if first.is_control() {
        // C0/DEL as caret notation, e.g. ^@
        let shown = char::from_u32((first as u32) ^ 0x40).unwrap_or('?');
        return (Cow::Owned(format!("^{shown}")), 2);
    }
    match g.width() {
        0 => (Cow::Borrowed("·"), 1),
        w => (Cow::Borrowed(g), w),
    }
}

/// Screen column of `pos` within its line.
pub fn display_col(rope: &Rope, pos: ByteIdx, tab_width: usize) -> DisplayCol {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    let end = pos.0.min(line_end(rope, line).0);
    let text = rope.slice(start..end).to_string();
    let mut col = DisplayCol(0);
    for g in text.graphemes(true) {
        col.0 += display_grapheme(g, col, tab_width).1;
    }
    col
}

/// The grapheme boundary on `line` closest to screen column `target`.
/// Past the end of the line gives the line end; inside a wide char gives its start.
pub fn byte_at_col(rope: &Rope, line: usize, target: DisplayCol, tab_width: usize) -> ByteIdx {
    let start = line_start(rope, line).0;
    let text = line_text(rope, line);
    let mut col = DisplayCol(0);
    for (off, g) in text.grapheme_indices(true) {
        let w = display_grapheme(g, col, tab_width).1;
        if col.0 + w > target.0 {
            return ByteIdx(start + off);
        }
        col.0 += w;
    }
    ByteIdx(start + text.len())
}

/// Next grapheme boundary; a line break counts as one step.
pub fn next_grapheme(rope: &Rope, pos: ByteIdx) -> ByteIdx {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    let end = line_end(rope, line).0;
    if pos.0 >= end {
        return if line + 1 < line_count(rope) {
            line_start(rope, line + 1)
        } else {
            pos
        };
    }
    let text = line_text(rope, line);
    let off = pos.0 - start;
    let next = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .find(|&i| i > off)
        .unwrap_or(text.len());
    ByteIdx(start + next)
}

/// Previous grapheme boundary; a line break counts as one step.
pub fn prev_grapheme(rope: &Rope, pos: ByteIdx) -> ByteIdx {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    if pos.0 <= start {
        return if line > 0 { line_end(rope, line - 1) } else { pos };
    }
    let end = line_end(rope, line).0;
    if pos.0 > end {
        // Inside a CRLF; step to the content end.
        return ByteIdx(end);
    }
    let text = line_text(rope, line);
    let off = pos.0 - start;
    let prev = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i < off)
        .last()
        .unwrap_or(0);
    ByteIdx(start + prev)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn class_of(g: &str) -> CharClass {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// VS Code word-right: skip spaces, then one run of word or punctuation chars.
/// At the end of a line, moves to the start of the next.
pub fn word_right(rope: &Rope, pos: ByteIdx) -> ByteIdx {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    if pos.0 >= line_end(rope, line).0 {
        return next_grapheme(rope, pos);
    }
    let text = line_text(rope, line);
    let off = pos.0 - start;
    let mut gs = text.grapheme_indices(true).skip_while(|&(i, _)| i < off).peekable();
    while gs.next_if(|&(_, g)| class_of(g) == CharClass::Space).is_some() {}
    if let Some(&(_, g)) = gs.peek() {
        let class = class_of(g);
        while gs.next_if(|&(_, g)| class_of(g) == class).is_some() {}
    }
    ByteIdx(start + gs.peek().map_or(text.len(), |&(i, _)| i))
}

/// VS Code word-left: skip spaces backwards, then one run of word or punctuation chars.
/// At the start of a line, moves to the end of the previous.
pub fn word_left(rope: &Rope, pos: ByteIdx) -> ByteIdx {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    if pos.0 <= start {
        return prev_grapheme(rope, pos);
    }
    let text = line_text(rope, line);
    let off = (pos.0 - start).min(text.len());
    let mut gs = text
        .grapheme_indices(true)
        .rev()
        .skip_while(|&(i, _)| i >= off)
        .peekable();
    while gs.next_if(|&(_, g)| class_of(g) == CharClass::Space).is_some() {}
    let mut result = gs.peek().map_or(0, |&(i, g)| i + g.len());
    if let Some(&(_, g)) = gs.peek() {
        let class = class_of(g);
        while let Some((i, _)) = gs.next_if(|&(_, g)| class_of(g) == class) {
            result = i;
        }
    }
    ByteIdx(start + result)
}

/// First non-whitespace position on the line (line end if the line is blank).
pub fn first_non_blank(rope: &Rope, line: usize) -> ByteIdx {
    let text = line_text(rope, line);
    let indent = text.len() - text.trim_start_matches([' ', '\t']).len();
    ByteIdx(line_start(rope, line).0 + indent)
}

/// The run of same-class characters around `pos` (double-click selection).
/// At a class boundary, the run to the right wins; at the line end, the run to the left.
pub fn word_range(rope: &Rope, pos: ByteIdx) -> (ByteIdx, ByteIdx) {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    let text = line_text(rope, line);
    let off = (pos.0 - start).min(text.len());
    let gs: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let Some(i) = gs
        .iter()
        .position(|&(o, g)| off < o + g.len())
        .or(gs.len().checked_sub(1))
    else {
        return (pos, pos);
    };
    let class = class_of(gs[i].1);
    let mut a = i;
    while a > 0 && class_of(gs[a - 1].1) == class {
        a -= 1;
    }
    let mut b = i + 1;
    while b < gs.len() && class_of(gs[b].1) == class {
        b += 1;
    }
    let end = gs.get(b).map_or(text.len(), |&(o, _)| o);
    (ByteIdx(start + gs[a].0), ByteIdx(start + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn line_end_excludes_crlf() {
        let r = rope("ab\r\ncd\nef");
        assert_eq!(line_count(&r), 3);
        assert_eq!(line_end(&r, 0), ByteIdx(2));
        assert_eq!(line_start(&r, 1), ByteIdx(4));
        assert_eq!(line_end(&r, 1), ByteIdx(6));
        assert_eq!(line_end(&r, 2), ByteIdx(9));
    }

    #[test]
    fn grapheme_steps_over_crlf_and_clusters() {
        let r = rope("e\u{301}x\r\ny");
        assert_eq!(next_grapheme(&r, ByteIdx(0)), ByteIdx(3));
        assert_eq!(next_grapheme(&r, ByteIdx(4)), ByteIdx(6));
        assert_eq!(prev_grapheme(&r, ByteIdx(6)), ByteIdx(4));
        assert_eq!(prev_grapheme(&r, ByteIdx(3)), ByteIdx(0));
    }

    #[test]
    fn display_columns() {
        let r = rope("\tab界c");
        assert_eq!(display_col(&r, ByteIdx(1), 4), DisplayCol(4));
        assert_eq!(display_col(&r, ByteIdx(3), 4), DisplayCol(6));
        assert_eq!(display_col(&r, ByteIdx(6), 4), DisplayCol(8));
        assert_eq!(byte_at_col(&r, 0, DisplayCol(7), 4), ByteIdx(3));
        assert_eq!(byte_at_col(&r, 0, DisplayCol(99), 4), ByteIdx(7));
    }

    #[test]
    fn word_ranges() {
        let r = rope("foo.bar  baz\n");
        assert_eq!(word_range(&r, ByteIdx(1)), (ByteIdx(0), ByteIdx(3)));
        assert_eq!(word_range(&r, ByteIdx(3)), (ByteIdx(3), ByteIdx(4)));
        assert_eq!(word_range(&r, ByteIdx(8)), (ByteIdx(7), ByteIdx(9)));
        assert_eq!(word_range(&r, ByteIdx(12)), (ByteIdx(9), ByteIdx(12)));
        assert_eq!(word_range(&r, ByteIdx(13)), (ByteIdx(13), ByteIdx(13)));
    }

    #[test]
    fn word_moves() {
        let r = rope("foo.bar  baz\nx");
        assert_eq!(word_right(&r, ByteIdx(0)), ByteIdx(3));
        assert_eq!(word_right(&r, ByteIdx(3)), ByteIdx(4));
        assert_eq!(word_right(&r, ByteIdx(7)), ByteIdx(12));
        assert_eq!(word_right(&r, ByteIdx(12)), ByteIdx(13));
        assert_eq!(word_left(&r, ByteIdx(12)), ByteIdx(9));
        assert_eq!(word_left(&r, ByteIdx(9)), ByteIdx(4));
        assert_eq!(word_left(&r, ByteIdx(13)), ByteIdx(12));
    }
}
