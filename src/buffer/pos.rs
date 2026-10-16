//! Text position types and every conversion between them.
//!
//! `ByteIdx` is the canonical unit. Nothing outside this module converts
//! between bytes, lines, graphemes and screen columns.

use std::borrow::Cow;
use std::ops::ControlFlow;

use ropey::{LineType, Rope};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Line breaks: LF, CRLF and lone CR.
pub const LINE_TYPE: LineType = LineType::LF_CR;

/// UTF-8 byte offset into a buffer. Always on a char boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteIdx(pub usize);

/// tree-sitter's position of `pos`: the row counts LFs only (as tree-sitter does, so a lone CR
/// doesn't start a row, unlike `LINE_TYPE`), the column is bytes since the last LF.
pub fn ts_point(rope: &Rope, pos: ByteIdx) -> tree_sitter::Point {
    let row = rope.byte_to_line_idx(pos.0, LineType::LF);
    let start = rope.line_to_byte_idx(row, LineType::LF);
    tree_sitter::Point {
        row,
        column: pos.0 - start,
    }
}

/// The text from byte `at` to the end of the chunk holding it (empty at the end): how tree-sitter
/// reads a rope without copying it.
pub fn chunk_from(rope: &Rope, at: usize) -> &[u8] {
    if at >= rope.len() {
        return &[];
    }
    let (chunk, start) = rope.chunk(at);
    &chunk.as_bytes()[at - start..]
}

/// The bytes from `start` to `end` (clamped to the text), chunk by chunk without copying: how
/// tree-sitter's query predicates (`#match?`) read a node's text. Byte offsets, no boundary needed.
pub fn bytes_in(rope: &Rope, start: usize, end: usize) -> impl Iterator<Item = &[u8]> {
    let end = end.min(rope.len());
    let mut at = start;
    std::iter::from_fn(move || {
        if at >= end {
            return None;
        }
        let chunk = chunk_from(rope, at);
        let chunk = &chunk[..chunk.len().min(end - at)];
        at += chunk.len();
        (!chunk.is_empty()).then_some(chunk)
    })
}

/// `bytes_in` backwards: the chunks of `start..end` (clamped to the text) from the last to the
/// first (bracket matching scans back from a closing bracket).
pub fn bytes_in_rev(rope: &Rope, start: usize, end: usize) -> impl Iterator<Item = &[u8]> {
    let mut at = end.min(rope.len());
    std::iter::from_fn(move || {
        if at <= start {
            return None;
        }
        let (chunk, chunk_start) = rope.chunk(at - 1);
        let from = chunk_start.max(start);
        let bytes = &chunk.as_bytes()[from - chunk_start..at - chunk_start];
        at = from;
        Some(bytes)
    })
}

/// The unit of an LSP position's `character`, negotiated at `initialize` (`positionEncoding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PosEncoding {
    /// Bytes: no conversion beyond the line lookup.
    Utf8,
    /// UTF-16 code units: LSP's default when a server names no encoding.
    #[default]
    Utf16,
}

/// An LSP position: 0-based line (LSP's line breaks are LF, CRLF and lone CR, as `LINE_TYPE`)
/// and the column in the server's `PosEncoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LspPos {
    pub line: u32,
    pub character: u32,
}

/// The LSP position of `pos`.
#[cfg_attr(not(test), allow(dead_code))] // document sync converts many with `LspWalker`
pub fn lsp_pos(rope: &Rope, pos: ByteIdx, encoding: PosEncoding) -> LspPos {
    LspWalker::new(rope, encoding).pos(pos)
}

/// `lsp_pos` for many positions, cheapest in increasing order (a change set's): a position on
/// the last one's line needs no line lookup, and UTF-16 counts only the chars since the last
/// position, so a long (minified) line with many changes stays linear.
pub struct LspWalker<'a> {
    rope: &'a Rope,
    encoding: PosEncoding,
    /// The last position's line: index, start, the next line's start, whether it's the last line.
    line: Option<(usize, usize, usize, bool)>,
    /// The last position and its column.
    last: usize,
    character: usize,
}

impl<'a> LspWalker<'a> {
    pub fn new(rope: &'a Rope, encoding: PosEncoding) -> Self {
        Self {
            rope,
            encoding,
            line: None,
            last: 0,
            character: 0,
        }
    }

    pub fn pos(&mut self, pos: ByteIdx) -> LspPos {
        let pos = pos.0.min(self.rope.len());
        match self.line {
            Some((_, start, next, last_line))
                if pos >= start && pos >= self.last && (pos < next || (last_line && pos == next)) =>
            {
                self.character += self.units(self.last, pos);
            }
            _ => {
                let line = line_of(self.rope, ByteIdx(pos));
                let (start, _, next) = line_bounds(self.rope, line);
                self.line = Some((line, start.0, next.0, line + 1 >= line_count(self.rope)));
                self.character = self.units(start.0, pos);
            }
        }
        self.last = pos;
        LspPos {
            line: self.line.map_or(0, |l| l.0) as u32,
            character: self.character as u32,
        }
    }

    /// The encoding's units in `from..to`.
    fn units(&self, from: usize, to: usize) -> usize {
        match self.encoding {
            PosEncoding::Utf8 => to - from,
            PosEncoding::Utf16 => self
                .rope
                .slice(from..to)
                .chunks()
                .map(|c| c.chars().map(char::len_utf16).sum::<usize>())
                .sum(),
        }
    }
}

/// The byte offset of an LSP position, clamped as the protocol says: a line past the end is the
/// text's end, a column past the line's content is its end (before the line break). A column
/// inside a char (a UTF-8 byte or the second half of a surrogate pair) rounds down to its start.
pub fn from_lsp_pos(rope: &Rope, pos: LspPos, encoding: PosEncoding) -> ByteIdx {
    let line = pos.line as usize;
    if line >= line_count(rope) {
        return ByteIdx(rope.len());
    }
    let (start, end, _) = line_bounds(rope, line);
    let want = pos.character as usize;
    match encoding {
        PosEncoding::Utf8 => ByteIdx(rope.floor_char_boundary((start.0 + want).min(end.0))),
        PosEncoding::Utf16 => {
            let mut units = 0;
            let mut at = start.0;
            for chunk in rope.slice(start.0..end.0).chunks() {
                for c in chunk.chars() {
                    units += c.len_utf16();
                    if units > want {
                        return ByteIdx(at);
                    }
                    at += c.len_utf8();
                }
            }
            ByteIdx(at)
        }
    }
}

/// `from_lsp_pos` for many positions (a server's diagnostics): lines come from a `LineWalker`,
/// so positions in increasing line order need no line lookup each; any order is correct.
pub struct LspReader<'a> {
    rope: &'a Rope,
    encoding: PosEncoding,
    lines: usize,
    walker: LineWalker<'a>,
}

impl<'a> LspReader<'a> {
    pub fn new(rope: &'a Rope, encoding: PosEncoding) -> Self {
        Self {
            rope,
            encoding,
            lines: line_count(rope),
            walker: LineWalker::new(rope),
        }
    }

    /// `from_lsp_pos(rope, pos, encoding)`, with the same clamping.
    pub fn byte(&mut self, pos: LspPos) -> ByteIdx {
        let line = pos.line as usize;
        if line >= self.lines {
            return ByteIdx(self.rope.len());
        }
        let line = self.walker.line(line);
        let text = line.text();
        let want = pos.character as usize;
        let offset = match self.encoding {
            PosEncoding::Utf8 => text.floor_char_boundary(want.min(text.len())),
            PosEncoding::Utf16 => {
                let mut units = 0;
                let mut at = text.len();
                for (i, c) in text.char_indices() {
                    units += c.len_utf16();
                    if units > want {
                        at = i;
                        break;
                    }
                }
                at
            }
        };
        ByteIdx(line.start().0 + offset)
    }
}

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

/// The line's start, its content end (before the line break) and the next line's start (the
/// text's end for the last line), in one lookup.
pub fn line_bounds(rope: &Rope, line: usize) -> (ByteIdx, ByteIdx, ByteIdx) {
    let start = rope.line_to_byte_idx(line, LINE_TYPE);
    let slice = rope.line(line, LINE_TYPE);
    let end = start + slice.trailing_line_break_idx(LINE_TYPE).unwrap_or(slice.len());
    (ByteIdx(start), ByteIdx(end), ByteIdx(start + slice.len()))
}

/// The line's content without its line break.
pub fn line_text(rope: &Rope, line: usize) -> String {
    rope.slice(line_start(rope, line).0..line_end(rope, line).0).to_string()
}

/// Calls `f(start, text)` with each line's content (no line break), from `line` to the end, or
/// from `line` back to the first line when `backward`. Stops at the first `Break` and returns its
/// value. `text` borrows the rope when the line sits in one chunk; otherwise it is copied into a
/// scratch string reused across lines, so the whole buffer is never copied.
pub fn each_line<T>(
    rope: &Rope,
    line: usize,
    backward: bool,
    mut f: impl FnMut(ByteIdx, &str) -> ControlFlow<T>,
) -> Option<T> {
    let mut scratch = String::new();
    let mut visit = |start: usize, slice: ropey::RopeSlice| {
        let content = slice.slice(..slice.trailing_line_break_idx(LINE_TYPE).unwrap_or(slice.len()));
        let text = match content.as_str() {
            Some(s) => s,
            None => {
                scratch.clear();
                content.chunks().for_each(|c| scratch.push_str(c));
                &scratch
            }
        };
        f(ByteIdx(start), text)
    };
    let last = line_count(rope) - 1;
    if backward {
        // Line by line index: ropey's `Lines::prev()` can repeat a line (and stop) when a line
        // break ends a chunk, e.g. "<2047 bytes>\r\r".
        for line in (0..=line.min(last)).rev() {
            let start = rope.line_to_byte_idx(line, LINE_TYPE);
            if let ControlFlow::Break(t) = visit(start, rope.line(line, LINE_TYPE)) {
                return Some(t);
            }
        }
    } else if line <= last {
        let mut start = rope.line_to_byte_idx(line, LINE_TYPE);
        for slice in rope.lines_at(line, LINE_TYPE) {
            if let ControlFlow::Break(t) = visit(start, slice) {
                return Some(t);
            }
            start += slice.len();
        }
    }
    None
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
    col_in(&text, text.len(), tab_width)
}

/// The grapheme boundary on `line` closest to screen column `target`.
/// Past the end of the line gives the line end; inside a wide char gives its start.
pub fn byte_at_col(rope: &Rope, line: usize, target: DisplayCol, tab_width: usize) -> ByteIdx {
    let start = line_start(rope, line).0;
    ByteIdx(start + offset_at_col(&line_text(rope, line), target, tab_width))
}

/// One line looked up once, for many conversions on it.
pub struct Line {
    pub index: usize,
    start: ByteIdx,
    end: ByteIdx,
    next: ByteIdx,
    text: String,
}

impl Line {
    pub fn new(rope: &Rope, index: usize) -> Self {
        let (start, end, next) = line_bounds(rope, index);
        Self {
            index,
            start,
            end,
            next,
            text: rope.slice(start.0..end.0).to_string(),
        }
    }

    /// Whether `pos` is on this line (its line break included; the last line's end too).
    pub fn contains(&self, pos: ByteIdx) -> bool {
        self.start <= pos && (pos < self.next || (pos == self.end && self.end == self.next))
    }

    pub fn start(&self) -> ByteIdx {
        self.start
    }

    /// End of the content, before the line break.
    pub fn end(&self) -> ByteIdx {
        self.end
    }

    /// Start of the next line (the text's end for the last line).
    pub fn next(&self) -> ByteIdx {
        self.next
    }

    /// `display_col` of a position on this line.
    pub fn col(&self, pos: ByteIdx, tab_width: usize) -> DisplayCol {
        col_in(
            &self.text,
            pos.0.clamp(self.start.0, self.end.0) - self.start.0,
            tab_width,
        )
    }

    /// `byte_at_col` on this line.
    pub fn byte_at_col(&self, target: DisplayCol, tab_width: usize) -> ByteIdx {
        ByteIdx(self.start.0 + offset_at_col(&self.text, target, tab_width))
    }

    /// The grapheme boundary nearest to screen column `target`, the one before on a tie (VS
    /// Code's `columnFromVisibleColumn`, for column select: a box edge inside a tab or wide char
    /// takes it if it covers most of it). Past the end of the line gives the line end.
    pub fn byte_near_col(&self, target: DisplayCol, tab_width: usize) -> ByteIdx {
        ByteIdx(self.start.0 + offset_near_col(&self.text, target, tab_width))
    }

    /// The line's content (no line break).
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The chars next to positions, for checks at thousands of cursors (auto-close): keeps the
/// chunk of the last lookup, so positions in increasing order mostly need no rope search.
pub struct Peek<'a> {
    rope: &'a Rope,
    chunk: &'a str,
    start: usize,
}

impl<'a> Peek<'a> {
    pub fn new(rope: &'a Rope) -> Self {
        Self {
            rope,
            chunk: "",
            start: 0,
        }
    }

    /// The chunk holding byte `at` (< len) as text from `at` on, and `at`'s offset in it.
    fn at(&mut self, at: usize) -> (&'a str, usize) {
        if at < self.start || at >= self.start + self.chunk.len() {
            (self.chunk, self.start) = self.rope.chunk(at);
        }
        (self.chunk, at - self.start)
    }

    /// The char starting at `pos` (`None` at the end of the text).
    pub fn after(&mut self, pos: ByteIdx) -> Option<char> {
        if pos.0 >= self.rope.len() {
            return None;
        }
        let (chunk, off) = self.at(pos.0);
        chunk[off..].chars().next()
    }

    /// The char ending at `pos` (`None` at the start). Chunks split on char boundaries, so it
    /// sits wholly in the chunk holding the byte before `pos`.
    pub fn before(&mut self, pos: ByteIdx) -> Option<char> {
        if pos.0 == 0 {
            return None;
        }
        let (chunk, off) = self.at(pos.0 - 1);
        chunk[..=off].chars().next_back()
    }

    pub fn byte(&mut self, at: usize) -> u8 {
        let (chunk, off) = self.at(at);
        chunk.as_bytes()[off]
    }

    /// The start of `pos`'s line if only spaces and tabs come before `pos` on it, else `None`.
    /// Looks back only over those blanks (no line lookup: a closing bracket typed at 60k cursors).
    pub fn blank_line_start(&mut self, pos: ByteIdx) -> Option<ByteIdx> {
        let mut i = pos.0;
        while i > 0 {
            match self.byte(i - 1) {
                b' ' | b'\t' => i -= 1,
                b'\n' | b'\r' => break,
                _ => return None,
            }
        }
        Some(ByteIdx(i))
    }

    /// The leading spaces and tabs of `pos`'s line (up to `pos`), looking back at most `limit`
    /// bytes for the line's start: `None` if it is further.
    pub fn line_indent(&mut self, pos: ByteIdx, limit: usize) -> Option<String> {
        let mut start = pos.0;
        while start > 0 && !matches!(self.byte(start - 1), b'\n' | b'\r') {
            if pos.0 - start >= limit {
                return None;
            }
            start -= 1;
        }
        let mut indent = String::new();
        for i in start..pos.0 {
            match self.byte(i) {
                b @ (b' ' | b'\t') => indent.push(b as char),
                _ => break,
            }
        }
        Some(indent)
    }
}

/// Walks lines in increasing order for commands at thousands of cursors: steps a line iterator
/// forward instead of looking each line up (a lookup costs a few hundred ns, which dominated at
/// 60k cursors). Asking for an earlier line, or one far ahead, looks it up afresh.
pub struct LineWalker<'a> {
    rope: &'a Rope,
    /// Yields line `next`, which starts at `next_start`.
    lines: Option<ropey::iter::Lines<'a>>,
    next: usize,
    next_start: usize,
    line: Option<Line>,
}

/// How far ahead stepping beats a fresh lookup.
const WALK_LINES: usize = 32;
const WALK_BYTES: usize = 4096;

impl<'a> LineWalker<'a> {
    pub fn new(rope: &'a Rope) -> Self {
        Self {
            rope,
            lines: None,
            next: 0,
            next_start: 0,
            line: None,
        }
    }

    /// Line `index`.
    pub fn line(&mut self, index: usize) -> &Line {
        if !self.line.as_ref().is_some_and(|l| l.index == index) {
            if self.lines.is_none() || index < self.next || index - self.next > WALK_LINES {
                self.seek(index);
            }
            while self.next <= index && self.step() {}
            if !self.line.as_ref().is_some_and(|l| l.index == index) {
                self.line = Some(Line::new(self.rope, index));
            }
        }
        self.line.as_ref().expect("line is set")
    }

    /// The line `pos` is on.
    pub fn line_at(&mut self, pos: ByteIdx) -> &Line {
        if !self.line.as_ref().is_some_and(|l| l.contains(pos)) {
            if self.lines.is_none() || pos.0 < self.next_start || pos.0 - self.next_start > WALK_BYTES {
                self.seek(line_of(self.rope, pos));
            }
            while self.step() && !self.line.as_ref().is_some_and(|l| l.contains(pos)) {}
            if !self.line.as_ref().is_some_and(|l| l.contains(pos)) {
                self.line = Some(Line::new(self.rope, line_of(self.rope, pos)));
            }
        }
        self.line.as_ref().expect("line is set")
    }

    fn seek(&mut self, index: usize) {
        self.lines = Some(self.rope.lines_at(index, LINE_TYPE));
        self.next = index;
        self.next_start = line_start(self.rope, index).0;
    }

    /// Reads line `next` into `line` (reusing its text buffer); false at the end.
    fn step(&mut self) -> bool {
        let Some(slice) = self.lines.as_mut().and_then(|l| l.next()) else {
            return false;
        };
        let start = self.next_start;
        let content = slice.trailing_line_break_idx(LINE_TYPE).unwrap_or(slice.len());
        let mut text = self.line.take().map(|l| l.text).unwrap_or_default();
        text.clear();
        slice.slice(..content).chunks().for_each(|c| text.push_str(c));
        self.line = Some(Line {
            index: self.next,
            start: ByteIdx(start),
            end: ByteIdx(start + content),
            next: ByteIdx(start + slice.len()),
            text,
        });
        self.next += 1;
        self.next_start += slice.len();
        true
    }
}

/// Printable ASCII only: every char is one grapheme one column wide, so columns are offsets.
fn is_plain(text: &str) -> bool {
    text.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// Screen column at byte `off` of a line's `text`.
fn col_in(text: &str, off: usize, tab_width: usize) -> DisplayCol {
    let text = &text[..off];
    if is_plain(text) {
        return DisplayCol(off);
    }
    let mut col = DisplayCol(0);
    for g in text.graphemes(true) {
        col.0 += display_grapheme(g, col, tab_width).1;
    }
    col
}

/// The offset in a line's `text` of the grapheme boundary at or before column `target`.
fn offset_at_col(text: &str, target: DisplayCol, tab_width: usize) -> usize {
    if is_plain(text) {
        return target.0.min(text.len());
    }
    let mut col = DisplayCol(0);
    for (off, g) in text.grapheme_indices(true) {
        let w = display_grapheme(g, col, tab_width).1;
        if col.0 + w > target.0 {
            return off;
        }
        col.0 += w;
    }
    text.len()
}

/// The offset in a line's `text` of the grapheme boundary nearest column `target`.
fn offset_near_col(text: &str, target: DisplayCol, tab_width: usize) -> usize {
    if is_plain(text) {
        return target.0.min(text.len());
    }
    let mut col = DisplayCol(0);
    for (off, g) in text.grapheme_indices(true) {
        let after = col.0 + display_grapheme(g, col, tab_width).1;
        if after >= target.0 {
            let take = after - target.0 < target.0 - col.0;
            return off + if take { g.len() } else { 0 };
        }
        col.0 = after;
    }
    text.len()
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

/// Word characters for word movement, double-click and whole-word search: alphanumeric or `_`.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn class_of(g: &str) -> CharClass {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        CharClass::Space
    } else if is_word_char(c) {
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

/// The word (run of word characters) touching `pos`, as VS Code's `getWordAtPosition`: the one
/// under it, else the one ending at it. `None` when `pos` has no word on either side (Cmd+D on
/// whitespace or punctuation does nothing).
pub fn word_at(rope: &Rope, pos: ByteIdx) -> Option<(ByteIdx, ByteIdx)> {
    let line = line_of(rope, pos);
    let start = line_start(rope, line).0;
    let text = line_text(rope, line);
    let off = (pos.0 - start).min(text.len());
    let gs: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let is_word = |g: &str| class_of(g) == CharClass::Word;
    // The grapheme at `pos` if it's a word char, else the one before it.
    let i = match gs.iter().position(|&(o, _)| o >= off) {
        Some(i) if is_word(gs[i].1) && gs[i].0 == off => i,
        Some(i) if i > 0 && is_word(gs[i - 1].1) => i - 1,
        None if gs.last().is_some_and(|&(_, g)| is_word(g)) => gs.len() - 1,
        _ => return None,
    };
    let mut a = i;
    while a > 0 && is_word(gs[a - 1].1) {
        a -= 1;
    }
    let mut b = i + 1;
    while b < gs.len() && is_word(gs[b].1) {
        b += 1;
    }
    let end = gs.get(b).map_or(text.len(), |&(o, _)| o);
    Some((ByteIdx(start + gs[a].0), ByteIdx(start + end)))
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

    fn lines_from(r: &Rope, line: usize, backward: bool) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        each_line::<()>(r, line, backward, |start, text| {
            out.push((start.0, text.to_string()));
            ControlFlow::Continue(())
        });
        out
    }

    #[test]
    fn each_line_both_ways() {
        let r = rope("ab\r\ncd\ref\n");
        let fwd = vec![
            (0, "ab".into()),
            (4, "cd".into()),
            (7, "ef".into()),
            (10, String::new()),
        ];
        assert_eq!(lines_from(&r, 0, false), fwd);
        assert_eq!(lines_from(&r, 2, false), fwd[2..]);
        assert_eq!(lines_from(&r, 9, false), vec![]);
        let back: Vec<_> = fwd.iter().rev().cloned().collect();
        assert_eq!(lines_from(&r, 3, true), back);
        assert_eq!(lines_from(&r, 99, true), back);
        assert_eq!(lines_from(&r, 1, true), back[2..]);
        assert_eq!(lines_from(&rope(""), 0, true), vec![(0, String::new())]);
        // Lines spanning chunks come out whole; `Break` stops early with its value.
        let long = format!("{}\n{}", "x".repeat(5000), "y".repeat(5000));
        let r = rope(&long);
        assert_eq!(
            lines_from(&r, 0, false)
                .iter()
                .map(|(_, t)| t.len())
                .collect::<Vec<_>>(),
            [5000, 5000]
        );
        let found = each_line(&r, 0, false, |start, text| match text.starts_with('y') {
            true => ControlFlow::Break(start),
            false => ControlFlow::Continue(()),
        });
        assert_eq!(found, Some(ByteIdx(5001)));
    }

    #[test]
    fn each_line_matches_line_helpers_around_chunk_edges() {
        // Lines ending on or near a chunk boundary, with every kind of line break, checked
        // against `line_start` / `line_text` from every line in both directions.
        for n in 2030..2070 {
            for tail in ["\r\r", "\r\n\r", "\n\n", "\r\n\r\n", "a\rb\n"] {
                let r = rope(&format!("{}{tail}", "é".repeat(n / 2) + &"x".repeat(n % 2)));
                let want: Vec<_> = (0..line_count(&r))
                    .map(|l| (line_start(&r, l).0, line_text(&r, l)))
                    .collect();
                for line in 0..want.len() {
                    assert_eq!(lines_from(&r, line, false), want[line..], "{n} {tail:?} from {line}");
                    let back: Vec<_> = want[..=line].iter().rev().cloned().collect();
                    assert_eq!(lines_from(&r, line, true), back, "{n} {tail:?} back from {line}");
                }
            }
        }
    }

    #[test]
    fn word_chars() {
        assert!(is_word_char('a') && is_word_char('_') && is_word_char('é') && is_word_char('9'));
        assert!(!is_word_char('.') && !is_word_char(' ') && !is_word_char('-'));
    }

    #[test]
    fn word_at_prefers_the_word_under_then_before() {
        let r = rope("foo.bar  héllo\nx");
        let w = |p: usize| word_at(&r, ByteIdx(p)).map(|(a, b)| (a.0, b.0));
        assert_eq!(w(0), Some((0, 3)));
        assert_eq!(w(3), Some((0, 3)), "at the end of foo, before the dot");
        assert_eq!(w(4), Some((4, 7)));
        assert_eq!(w(7), Some((4, 7)));
        assert_eq!(w(8), None, "between two spaces");
        assert_eq!(w(10), Some((9, 15)), "multi-byte word");
        assert_eq!(w(15), Some((9, 15)), "line end");
        assert_eq!(w(16), Some((16, 17)));
        assert_eq!(word_at(&rope(""), ByteIdx(0)), None);
    }

    #[test]
    fn line_cache_agrees_with_the_rope_functions() {
        let rope = Rope::from_str("plain text\r\n\tx界\u{301}y\nlast");
        for index in 0..line_count(&rope) {
            let line = Line::new(&rope, index);
            let (start, _, next) = line_bounds(&rope, index);
            for p in start.0..=next.0.min(rope.len()) {
                let p = ByteIdx(p);
                if rope.floor_char_boundary(p.0) != p.0 || line_of(&rope, p) != index {
                    continue;
                }
                assert!(line.contains(p), "line {index} at {p:?}");
                assert_eq!(line.col(p, 4), display_col(&rope, p, 4), "line {index} at {p:?}");
            }
            for c in 0..14 {
                let c = DisplayCol(c);
                assert_eq!(line.byte_at_col(c, 4), byte_at_col(&rope, index, c, 4));
            }
        }
        // The walker gives the same lines, stepping or seeking, by index or by position.
        let text: String = (0..300).map(|i| format!("{i}\t界\r\n")).collect::<String>() + "\r\r\n";
        let rope = Rope::from_str(&text);
        let mut walk = LineWalker::new(&rope);
        for index in [0, 1, 2, 5, 40, 41, 299, 300, 301, 302, 3] {
            let l = walk.line(index);
            assert_eq!(
                (l.index, l.start(), l.end(), l.next()),
                (
                    index,
                    line_start(&rope, index),
                    line_end(&rope, index),
                    line_bounds(&rope, index).2
                )
            );
            assert_eq!(l.col(l.end(), 4), display_col(&rope, line_end(&rope, index), 4));
        }
        let mut walk = LineWalker::new(&rope);
        for p in (0..=rope.len()).step_by(7).chain([rope.len(), 3, rope.len()]) {
            let p = ByteIdx(rope.floor_char_boundary(p));
            assert_eq!(walk.line_at(p).index, line_of(&rope, p), "at {p:?}");
        }

        // Plain ASCII takes the shortcut; the answers are the same.
        let rope = Rope::from_str("abc");
        assert_eq!(byte_at_col(&rope, 0, DisplayCol(9), 4), ByteIdx(3));
        assert_eq!(display_col(&rope, ByteIdx(2), 4), DisplayCol(2));
    }

    #[test]
    fn line_bounds_and_nearest_column() {
        let rope = Rope::from_str("a\tb\r\n界x");
        assert_eq!(line_bounds(&rope, 0), (ByteIdx(0), ByteIdx(3), ByteIdx(5)));
        assert_eq!(line_bounds(&rope, 1), (ByteIdx(5), ByteIdx(9), ByteIdx(9)));
        // The tab covers columns 1..4: column 2 is nearer its start, 3 nearer its end.
        let near = |line, col| Line::new(&rope, line).byte_near_col(DisplayCol(col), 4).0;
        assert_eq!([0, 1, 2, 3, 4, 5, 9].map(|c| near(0, c)), [0, 1, 1, 2, 2, 3, 3]);
        // The wide char covers columns 0..2: a tie at 1 takes the start.
        assert_eq!([0, 1, 2, 3, 4].map(|c| near(1, c)), [5, 5, 8, 9, 9]);
    }

    /// Seeded xorshift: `next(n)` is in `0..n`.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Reference column maths straight from `display_grapheme`, with no ASCII shortcut.
    fn ref_cols(text: &str, tab_width: usize) -> Vec<(usize, usize, usize)> {
        // (offset, column before, column after) per grapheme.
        let mut col = 0;
        let mut out = Vec::new();
        for (off, g) in text.grapheme_indices(true) {
            let w = display_grapheme(g, DisplayCol(col), tab_width).1;
            out.push((off, col, col + w));
            col += w;
        }
        out
    }

    fn ref_col(text: &str, off: usize, tab_width: usize) -> usize {
        let g = ref_cols(text, tab_width);
        g.iter()
            .find(|x| x.0 >= off)
            .map_or(g.last().map_or(0, |x| x.2), |x| x.1)
    }

    fn ref_at_col(text: &str, target: usize, tab_width: usize) -> usize {
        let g = ref_cols(text, tab_width);
        g.iter().find(|x| x.2 > target).map_or(text.len(), |x| x.0)
    }

    fn ref_near_col(text: &str, target: usize, tab_width: usize) -> usize {
        for (off, before, after) in ref_cols(text, tab_width) {
            if after >= target {
                let len = text[off..].graphemes(true).next().unwrap().len();
                return if after - target < target - before {
                    off + len
                } else {
                    off
                };
            }
        }
        text.len()
    }

    /// `Line`, `LineWalker` and the printable-ASCII shortcut against a plain grapheme walk and the
    /// rope's own line lookups, on random text: mostly-ASCII lines (the shortcut), tabs, wide
    /// chars, combining marks, emoji, controls, and every kind of line break, over several chunks.
    #[test]
    fn line_and_walker_match_the_reference_on_random_text() {
        const PIECES: [&str; 17] = [
            "a", "b", "z", " ", "~", "\t", "界", "e\u{301}", "\u{301}", "😀", "\u{1}", "\u{7f}", "é", "\n", "\r\n",
            "\r", "xy",
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for round in 0..24 {
            let len = if round % 12 == 0 { 2500 } else { 1 + rng.next(60) };
            let mut text = String::new();
            while text.len() < len {
                // Every other round leans to plain ASCII lines, so the shortcut is taken often.
                let i = if round % 2 == 0 && rng.next(4) > 0 {
                    [0, 1, 3, 4, 16, 13][rng.next(6)]
                } else {
                    rng.next(PIECES.len())
                };
                text.push_str(PIECES[i]);
            }
            let rope = Rope::from_str(&text);
            let tab = [1, 2, 4, 8][rng.next(4)];
            for index in 0..line_count(&rope) {
                let line = Line::new(&rope, index);
                assert_eq!(
                    (line.start(), line.end()),
                    (line_start(&rope, index), line_end(&rope, index))
                );
                let content = line_text(&rope, index);
                let s = line.start().0;
                for (off, _) in content.grapheme_indices(true).chain([(content.len(), "")]) {
                    let want = DisplayCol(ref_col(&content, off, tab));
                    assert_eq!(line.col(ByteIdx(s + off), tab), want, "{content:?} at {off}");
                    assert_eq!(display_col(&rope, ByteIdx(s + off), tab), want, "{content:?} at {off}");
                }
                let width = ref_col(&content, content.len(), tab);
                for c in 0..width + 3 {
                    let at = ByteIdx(s + ref_at_col(&content, c, tab));
                    assert_eq!(line.byte_at_col(DisplayCol(c), tab), at, "{content:?} col {c}");
                    assert_eq!(byte_at_col(&rope, index, DisplayCol(c), tab), at, "{content:?} col {c}");
                    let near = ByteIdx(s + ref_near_col(&content, c, tab));
                    assert_eq!(line.byte_near_col(DisplayCol(c), tab), near, "{content:?} col {c}");
                }
            }
            // The walker, by index (mostly forward, sometimes back or far ahead) and by position.
            let count = line_count(&rope);
            let mut walk = LineWalker::new(&rope);
            let mut index = 0;
            for _ in 0..60 {
                index = match rng.next(10) {
                    0 => rng.next(count),
                    1 => (index + 40).min(count - 1),
                    _ => (index + rng.next(3)).min(count - 1),
                };
                let l = walk.line(index);
                let (start, end, next) = line_bounds(&rope, index);
                assert_eq!((l.index, l.start(), l.end(), l.next()), (index, start, end, next));
                assert_eq!(l.text, line_text(&rope, index));
            }
            let mut walk = LineWalker::new(&rope);
            let mut p = 0;
            for _ in 0..60 {
                p = match rng.next(10) {
                    0 => rng.next(rope.len() + 1),
                    1 => p + 5000,
                    _ => p + rng.next(20),
                }
                .min(rope.len());
                let at = ByteIdx(rope.floor_char_boundary(p));
                let l = walk.line_at(at);
                let index = line_of(&rope, at);
                assert_eq!(
                    (l.index, l.start(), l.next()),
                    (index, line_start(&rope, index), line_bounds(&rope, index).2)
                );
                assert!(l.contains(at));
            }
        }
    }

    /// `Peek` against the string, at every char boundary of a text spanning many chunks,
    /// forwards and then backwards (a fresh chunk lookup each time).
    #[test]
    fn peek_reads_the_chars_around_positions() {
        let text = "a界😀\r\n(é)".repeat(2000);
        let rope = Rope::from_str(&text);
        let mut peek = Peek::new(&rope);
        let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
        let check = |peek: &mut Peek, i: usize| {
            assert_eq!(peek.after(ByteIdx(i)), text[i..].chars().next(), "after {i}");
            assert_eq!(peek.before(ByteIdx(i)), text[..i].chars().next_back(), "before {i}");
        };
        for &i in &bounds {
            check(&mut peek, i);
        }
        for &i in bounds.iter().rev() {
            check(&mut peek, i);
        }
    }

    #[test]
    fn peek_blank_line_start_and_line_indent() {
        let text = "a\n  \t  x\r\n\t y\rz\n    ";
        let rope = Rope::from_str(text);
        let mut peek = Peek::new(&rope);
        let at = |s: &str| ByteIdx(text.find(s).unwrap());
        assert_eq!(peek.blank_line_start(at("x")), Some(at("  \t")));
        assert_eq!(peek.blank_line_start(ByteIdx(at("x").0 - 2)), Some(at("  \t")));
        assert_eq!(peek.blank_line_start(ByteIdx(at("x").0 + 1)), None, "after x");
        assert_eq!(
            peek.blank_line_start(at("y")),
            Some(ByteIdx(at("y").0 - 2)),
            "after CRLF"
        );
        assert_eq!(peek.blank_line_start(at("z")), Some(at("z")), "after a lone CR");
        assert_eq!(peek.blank_line_start(ByteIdx(0)), Some(ByteIdx(0)));
        assert_eq!(peek.blank_line_start(ByteIdx(1)), None);
        assert_eq!(
            peek.blank_line_start(ByteIdx(text.len())),
            Some(ByteIdx(text.len() - 4))
        );
        assert_eq!(peek.line_indent(at("x"), 100).as_deref(), Some("  \t  "));
        assert_eq!(peek.line_indent(at("y"), 100).as_deref(), Some("\t "));
        assert_eq!(peek.line_indent(at("a"), 100).as_deref(), Some(""));
        assert_eq!(peek.line_indent(ByteIdx(text.len()), 100).as_deref(), Some("    "));
        // The indentation stops at the first other char, the line start is looked for `limit` back.
        assert_eq!(peek.line_indent(ByteIdx(at("x").0 + 1), 100).as_deref(), Some("  \t  "));
        assert_eq!(peek.line_indent(at("x"), 4), None);
        assert_eq!(peek.line_indent(at("x"), 5).as_deref(), Some("  \t  "));
    }

    #[test]
    fn lsp_positions_in_both_encodings() {
        // "é" is 2 bytes / 1 unit, "😀" 4 bytes / 2 units; CRLF and a lone CR end lines.
        let text = "aé😀b\r\nx\ry";
        let rope = Rope::from_str(text);
        let at = |s: &str| ByteIdx(text.find(s).unwrap());
        let p = |line, character| LspPos { line, character };
        use PosEncoding::*;
        assert_eq!(lsp_pos(&rope, at("b"), Utf8), p(0, 7));
        assert_eq!(lsp_pos(&rope, at("b"), Utf16), p(0, 4));
        assert_eq!(lsp_pos(&rope, at("😀"), Utf16), p(0, 2));
        assert_eq!(lsp_pos(&rope, at("x"), Utf16), p(1, 0));
        assert_eq!(lsp_pos(&rope, at("y"), Utf8), p(2, 0));
        assert_eq!(lsp_pos(&rope, ByteIdx(text.len()), Utf16), p(2, 1));
        for enc in [Utf8, Utf16] {
            for (i, _) in text.char_indices() {
                let pos = ByteIdx(i);
                if text[..i].ends_with('\r') && text[i..].starts_with('\n') {
                    continue; // inside CRLF: not a cursor position
                }
                assert_eq!(
                    from_lsp_pos(&rope, lsp_pos(&rope, pos, enc), enc),
                    pos,
                    "{enc:?} at {i}"
                );
            }
        }
        // Clamping: past the line's content, past the last line, inside a char.
        assert_eq!(from_lsp_pos(&rope, p(0, 99), Utf16), at("\r\n"));
        assert_eq!(from_lsp_pos(&rope, p(9, 0), Utf8), ByteIdx(text.len()));
        assert_eq!(from_lsp_pos(&rope, p(0, 2), Utf8), at("é"));
        assert_eq!(from_lsp_pos(&rope, p(0, 3), Utf16), at("😀"), "inside a surrogate pair");
    }

    /// `LspReader` agrees with `from_lsp_pos` for positions in and past every line, in order,
    /// backwards and out of range, on text spanning many chunks.
    #[test]
    fn lsp_reader_matches_from_lsp_pos() {
        for tail in ["", "\n", "z"] {
            let text = format!("{}{tail}", "a界😀\r\nb\ré\n\n".repeat(150));
            let rope = Rope::from_str(&text);
            let lines = line_count(&rope) as u32;
            for enc in [PosEncoding::Utf8, PosEncoding::Utf16] {
                let mut positions = Vec::new();
                for line in 0..lines + 2 {
                    for character in 0..12 {
                        positions.push(LspPos { line, character });
                    }
                }
                let mut reader = LspReader::new(&rope, enc);
                for &p in &positions {
                    assert_eq!(reader.byte(p), from_lsp_pos(&rope, p, enc), "{enc:?} {p:?}");
                }
                for &p in positions.iter().rev().step_by(5) {
                    assert_eq!(reader.byte(p), from_lsp_pos(&rope, p, enc), "{enc:?} back to {p:?}");
                }
            }
        }
    }

    /// `LspWalker` agrees with a fresh `lsp_pos` at every char boundary visited in increasing
    /// order (also repeating and skipping positions), on text spanning many chunks, ending with
    /// and without a line break.
    #[test]
    fn lsp_walker_matches_lsp_pos() {
        for tail in ["", "\n", "\r\n", "z"] {
            let text = format!("{}{tail}", "a界😀\r\nb\ré\n\n".repeat(150));
            let rope = Rope::from_str(&text);
            let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
            for enc in [PosEncoding::Utf8, PosEncoding::Utf16] {
                let fresh = |i: usize| lsp_pos(&rope, ByteIdx(i), enc);
                let mut walker = LspWalker::new(&rope, enc);
                for &i in &bounds {
                    assert_eq!(walker.pos(ByteIdx(i)), fresh(i), "{enc:?} at {i}");
                    assert_eq!(walker.pos(ByteIdx(i)), fresh(i), "{enc:?} again at {i}");
                }
                let mut walker = LspWalker::new(&rope, enc);
                for &i in bounds.iter().step_by(7) {
                    assert_eq!(walker.pos(ByteIdx(i)), fresh(i), "{enc:?} skipping to {i}");
                }
                // Backwards works too (a fresh lookup each time), also within a line.
                for &i in bounds.iter().rev().step_by(13) {
                    assert_eq!(walker.pos(ByteIdx(i)), fresh(i), "{enc:?} back to {i}");
                }
                for i in [8, 4, 1, 0] {
                    assert_eq!(walker.pos(ByteIdx(i)), fresh(i), "{enc:?} back on line 0 to {i}");
                }
            }
        }
    }
}
