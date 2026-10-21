//! The hex view's model (PLAN.md "Binary files (hex view)"): a file's bytes as `hexdump -C` prints
//! them, and byte search. No UI and no app state: the hex tab (`Tab::Hex`, hex-b) holds a
//! `HexDoc`, fits a `Layout` to its width and formats only the visible rows with `row`; find on a
//! hex tab (hex-c) builds a `Query` and calls `find_next` / `find_prev` / `find_in` on the UI
//! thread (each bounded by the distance to a match or the window) and `count` off it.
//!
//! Rows: offset `Layout::offset_digits` wide, two spaces, each byte as `xx ` with one extra space
//! after every 8th (and after the last when a row has fewer than 8), then `|`, the bytes as ASCII
//! (`ascii_char`), `|`. A short last row pads its hex part so the `|` lines up; one more row holds
//! only the end offset (the file's size); an empty file has no rows, as hexdump prints nothing.
//! No `*` squeezing (hexdump `-v`), so row `i` starts at byte `i * bytes_per_row`. One deviation:
//! every row's offset is as wide as the file's size needs (hexdump widens each offset on its own
//! past 8 digits), so the columns stay put; it only shows for files of 4 GiB and more, which jed
//! doesn't open (`HEX_MAX_BYTES`).
//!
//! Matches are byte ranges `start..start + query.len()` and may overlap (`aa` is found 3 times in
//! `aaaa`): that makes the matches in a window independent of where the window starts, so a frame
//! can search only its visible bytes and agree with `count` over the whole file.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use memchr::memmem;

use crate::buffer::file::Stamp;
use crate::command::Motion;

/// Files bigger than this aren't opened in a hex tab (paged reads are a TODO).
pub const HEX_MAX_BYTES: usize = 256 << 20;
/// `count` stops here: the find widget then says "100000+".
pub const HEX_MAX_COUNT: usize = 100_000;
/// The largest `hex_bytes_per_line` (the setting is a power of two from 1 to this).
pub const MAX_BYTES_PER_ROW: usize = 64;

/// Whether a file of `len` bytes opens in a hex tab; else why not (the message's end).
pub fn check_size(len: u64) -> Result<(), String> {
    if len > HEX_MAX_BYTES as u64 {
        Err(format!(
            "too big for the hex view ({} MB, at most {} MB)",
            len >> 20,
            HEX_MAX_BYTES >> 20
        ))
    } else {
        Ok(())
    }
}

/// A file's bytes as read, shared (cloning is an `Arc` clone: a tab and its splits share one),
/// and the `Stamp` they were read with, for the changed-on-disk check.
#[derive(Debug, Clone)]
pub struct HexDoc {
    bytes: Arc<[u8]>,
    #[cfg_attr(not(test), allow(dead_code))] // Used by disk checks (hex-d).
    stamp: Stamp,
}

impl HexDoc {
    /// Hashes the bytes for the stamp: blocking for a big file (call from `spawn_blocking`, or
    /// use `with_stamp` with the one the load computed).
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        let stamp = Stamp::of(&bytes);
        Self { bytes, stamp }
    }

    #[cfg_attr(not(test), allow(dead_code))] // Used by disk checks (hex-d).
    pub fn with_stamp(bytes: impl Into<Arc<[u8]>>, stamp: Stamp) -> Self {
        Self {
            bytes: bytes.into(),
            stamp,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[cfg_attr(not(test), allow(dead_code))] // Used by disk checks (hex-d).
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))] // Used by disk checks (hex-d).
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }
}

/// Which column a byte is shown (or the cursor is) in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Hex,
    Ascii,
}

/// A screen column inside a row: the byte under it (its index in the row, which may be past a
/// short last row's bytes: the caller clamps) and which column it's in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub column: Column,
}

/// How rows are laid out at some width: bytes per row and offset digits, and from them every
/// part's screen columns (relative to the row's first column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    bytes_per_row: usize,
    offset_digits: usize,
}

impl Layout {
    /// The largest power of two up to `max` (itself floored to a power of two in
    /// 1..=`MAX_BYTES_PER_ROW`) whose row fits `width` columns, or 1 when none does (the row is
    /// then cut); offsets as wide as `len` (the file's size, which the last row prints) needs,
    /// at least 8 digits.
    pub fn fit(width: usize, max: usize, len: usize) -> Self {
        let digits = offset_digits(len);
        let mut n = floor_pow2(max.clamp(1, MAX_BYTES_PER_ROW));
        while n > 1 && row_width(n, digits) > width {
            n /= 2;
        }
        Self {
            bytes_per_row: n,
            offset_digits: digits,
        }
    }

    pub fn bytes_per_row(&self) -> usize {
        self.bytes_per_row
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn offset_digits(&self) -> usize {
        self.offset_digits
    }

    /// A full row's width in columns (16 bytes: hexdump's 78).
    pub fn width(&self) -> usize {
        row_width(self.bytes_per_row, self.offset_digits)
    }

    /// Rows in a `len`-byte file: one per `bytes_per_row` bytes plus the end offset's; none when
    /// it's empty.
    pub fn rows(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            len.div_ceil(self.bytes_per_row) + 1
        }
    }

    /// The offset row `row` prints: its first byte, or `len` for the last (end) row.
    pub fn row_offset(&self, row: usize, len: usize) -> usize {
        (row * self.bytes_per_row).min(len)
    }

    /// The row a byte is on.
    pub fn row_of(&self, offset: usize) -> usize {
        offset / self.bytes_per_row
    }

    /// The offset's columns.
    pub fn offset_cols(&self) -> Range<usize> {
        0..self.offset_digits
    }

    /// The column of the first hex digit of the row's `index`th byte (two digits wide);
    /// `hex_col(bytes_per_row)` is just past the hex part (its trailing spaces included).
    pub fn hex_col(&self, index: usize) -> usize {
        let extra = if index == self.bytes_per_row && self.bytes_per_row < 8 {
            1
        } else {
            index / 8
        };
        self.offset_digits + 2 + 3 * index + extra
    }

    /// The column of the opening `|`.
    pub fn bar_col(&self) -> usize {
        self.hex_col(self.bytes_per_row)
    }

    /// The column of the row's `index`th byte in the ASCII column; `ascii_col(bytes_per_row)` is
    /// the closing `|`'s (in a full row).
    pub fn ascii_col(&self, index: usize) -> usize {
        self.bar_col() + 1 + index
    }

    /// The byte and column under screen column `col` of a row: a hex cell's two digits and the
    /// space(s) after it are its byte's, an ASCII char its byte's; the offset, the gap before the
    /// first byte, the `|`s and anything past the row are `None`.
    pub fn hit(&self, col: usize) -> Option<Hit> {
        let n = self.bytes_per_row;
        let start = self.hex_col(0);
        if col < start {
            return None;
        }
        if col < self.bar_col() {
            // The 8-byte groups are 25 columns apart (with fewer than 8 bytes there's one group).
            let rel = col - start;
            let (group, within) = (rel / 25, rel % 25);
            let index = (group * 8 + (within / 3).min(7)).min(n - 1);
            return Some(Hit {
                index,
                column: Column::Hex,
            });
        }
        let ascii = self.ascii_col(0);
        (col >= ascii && col < ascii + n).then(|| Hit {
            index: col - ascii,
            column: Column::Ascii,
        })
    }
}

/// Columns of a row of `n` bytes with `digits`-wide offsets.
fn row_width(n: usize, digits: usize) -> usize {
    // offset, 2 spaces, `xx ` per byte, the group spaces, `|`, n chars, `|`.
    digits + 2 + 3 * n + n.div_ceil(8) + 1 + n + 1
}

fn offset_digits(len: usize) -> usize {
    let bits = usize::BITS - len.leading_zeros();
    (bits as usize).div_ceil(4).max(8)
}

fn floor_pow2(n: usize) -> usize {
    1 << (usize::BITS - 1 - n.leading_zeros())
}

/// How a byte shows in the ASCII column: itself if printable ASCII (0x20–0x7e), else `.`.
pub fn ascii_char(b: u8) -> char {
    if is_printable(b) { b as char } else { '.' }
}

pub fn is_printable(b: u8) -> bool {
    (0x20..=0x7e).contains(&b)
}

/// The row starting at `offset` (a multiple of `bytes_per_row`, or `doc.len()` for the end row)
/// exactly as `hexdump -C -v` prints it, without the newline.
pub fn row(doc: &HexDoc, layout: &Layout, offset: usize) -> String {
    let len = doc.len();
    debug_assert!(offset == len || (offset < len && offset.is_multiple_of(layout.bytes_per_row)));
    let digits = layout.offset_digits;
    let mut s = String::with_capacity(layout.width());
    let _ = write!(s, "{offset:0digits$x}");
    if offset >= len {
        return s;
    }
    let bytes = &doc.bytes()[offset..len.min(offset + layout.bytes_per_row)];
    s.push_str("  ");
    for (i, b) in bytes.iter().enumerate() {
        let _ = write!(s, "{b:02x} ");
        if (i + 1) % 8 == 0 {
            s.push(' ');
        }
    }
    // All ASCII, so bytes are columns.
    let pad = layout.bar_col().saturating_sub(s.len());
    s.extend(std::iter::repeat_n(' ', pad));
    s.push('|');
    s.extend(bytes.iter().map(|&b| ascii_char(b)));
    s.push('|');
    s
}

/// The bytes as hex pairs separated by spaces (`7f 45 4c 46`): what Cmd+C copies from the hex
/// column, and a hex query seeded from a selection.
pub fn hex_pairs(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The bytes as the ASCII column shows them (`.` for the rest): what Cmd+C copies from it.
pub fn ascii_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| ascii_char(b)).collect()
}

/// The bytes as text when every one is printable ASCII.
pub fn printable_text(bytes: &[u8]) -> Option<String> {
    bytes
        .iter()
        .all(|&b| is_printable(b))
        .then(|| bytes.iter().map(|&b| b as char).collect())
}

/// Go to Offset's input: hex as `0x1f0` / `1f0h` (either case), else decimal (`496`); spaces
/// around it are ignored. `None` if it's neither. Past the end is the caller's to clamp.
pub fn parse_offset(text: &str) -> Option<usize> {
    let t = text.trim();
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .or_else(|| t.strip_suffix('h'))
        .or_else(|| t.strip_suffix('H'));
    match hex {
        Some(h) if !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()) => usize::from_str_radix(h, 16).ok(),
        Some(_) => None,
        None if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => t.parse().ok(),
        None => None,
    }
}

/// The find widget's mode on a hex tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ascii,
    Hex,
}

/// Why a hex query doesn't parse; `Display` is the widget's error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HexError {
    /// An odd number of hex digits.
    Odd,
    /// The first char that is neither a hex digit nor whitespace.
    NotHex(char),
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HexError::Odd => write!(f, "hex bytes come in pairs"),
            HexError::NotHex(c) => write!(f, "not a hex digit: {c}"),
        }
    }
}

/// Hex digit pairs, whitespace ignored anywhere (`7f454c46`, `7f 45 4c 46`, `7F 45`).
pub fn parse_hex(text: &str) -> Result<Vec<u8>, HexError> {
    let mut digits = Vec::with_capacity(text.len());
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        match c.to_digit(16) {
            Some(d) => digits.push(d as u8),
            None => return Err(HexError::NotHex(c)),
        }
    }
    if !digits.len().is_multiple_of(2) {
        return Err(HexError::Odd);
    }
    Ok(digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect())
}

/// The query text after switching the widget to `to`: ASCII → its UTF-8 bytes as hex pairs;
/// hex → the text when it parses and every byte is printable ASCII. `None`: keep the text as is
/// (it's re-read in the new mode).
pub fn convert(text: &str, to: Mode) -> Option<String> {
    match to {
        Mode::Hex => Some(hex_pairs(text.as_bytes())),
        Mode::Ascii => parse_hex(text).ok().and_then(|b| printable_text(&b)),
    }
}

/// A byte search: always a fixed number of bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// The text's UTF-8 bytes; not case-sensitive: ASCII letters match either case.
    Ascii {
        text: String,
        case_sensitive: bool,
    },
    Hex(Vec<u8>),
}

impl Query {
    /// The widget's text read in `mode` (`case_sensitive` only matters for ASCII).
    pub fn parse(text: &str, mode: Mode, case_sensitive: bool) -> Result<Query, HexError> {
        Ok(match mode {
            Mode::Ascii => Query::Ascii {
                text: text.to_string(),
                case_sensitive,
            },
            Mode::Hex => Query::Hex(parse_hex(text)?),
        })
    }

    /// The bytes looked for (as typed: not folded).
    pub fn bytes(&self) -> &[u8] {
        match self {
            Query::Ascii { text, .. } => text.as_bytes(),
            Query::Hex(b) => b,
        }
    }

    /// Every match's length.
    pub fn len(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes().is_empty()
    }

    fn folds(&self) -> bool {
        matches!(
            self,
            Query::Ascii {
                case_sensitive: false,
                ..
            }
        )
    }

    /// A searcher for this query; `None` when it's empty (it matches nothing).
    fn searcher(&self) -> Option<Searcher> {
        (!self.is_empty()).then(|| Searcher::new(self))
    }
}

/// Bytes folded per chunk for a case-insensitive search (a copy this size at a time).
const FOLD_CHUNK: usize = 1 << 16;

/// Finds a query's matches by their starts. A case-insensitive one folds the needle once and the
/// haystack a chunk at a time (with the needle's length − 1 of overlap), so memmem does the work
/// either way.
struct Searcher {
    forward: memmem::Finder<'static>,
    reverse: memmem::FinderRev<'static>,
    len: usize,
    fold: bool,
}

impl Searcher {
    fn new(q: &Query) -> Self {
        let needle = if q.folds() {
            Cow::Owned(q.bytes().to_ascii_lowercase())
        } else {
            Cow::Borrowed(q.bytes())
        };
        Self {
            forward: memmem::Finder::new(&needle).into_owned(),
            reverse: memmem::FinderRev::new(&needle).into_owned(),
            len: needle.len(),
            fold: q.folds(),
        }
    }

    /// The pieces `starts` is searched in, in order (reversed for `rev`): the whole range unless
    /// folding.
    fn pieces(&self, starts: Range<usize>, rev: bool) -> Vec<Range<usize>> {
        if !self.fold || starts.len() <= FOLD_CHUNK {
            return vec![starts];
        }
        let mut v: Vec<Range<usize>> = starts
            .clone()
            .step_by(FOLD_CHUNK)
            .map(|s| s..(s + FOLD_CHUNK).min(starts.end))
            .collect();
        if rev {
            v.reverse();
        }
        v
    }

    /// The haystack for matches starting in `starts`: through the last such match's end.
    fn hay<'a>(&self, bytes: &'a [u8], starts: &Range<usize>, buf: &'a mut Vec<u8>) -> &'a [u8] {
        let end = (starts.end + self.len - 1).min(bytes.len());
        let hay = &bytes[starts.start..end.max(starts.start)];
        if self.fold {
            buf.clear();
            buf.extend(hay.iter().map(u8::to_ascii_lowercase));
            buf
        } else {
            hay
        }
    }

    /// The first match starting in `starts`.
    fn first(&self, bytes: &[u8], starts: Range<usize>) -> Option<usize> {
        let mut buf = Vec::new();
        for piece in self.pieces(starts, false) {
            let hay = self.hay(bytes, &piece, &mut buf);
            if let Some(i) = self.forward.find(hay) {
                return Some(piece.start + i);
            }
        }
        None
    }

    /// The last match starting in `starts`.
    fn last(&self, bytes: &[u8], starts: Range<usize>) -> Option<usize> {
        let mut buf = Vec::new();
        for piece in self.pieces(starts, true) {
            let hay = self.hay(bytes, &piece, &mut buf);
            if let Some(i) = self.reverse.rfind(hay) {
                return Some(piece.start + i);
            }
        }
        None
    }

    /// Calls `f` with every match starting in `starts`, in order, until it returns false.
    fn each(&self, bytes: &[u8], starts: Range<usize>, mut f: impl FnMut(usize) -> bool) {
        let mut buf = Vec::new();
        for piece in self.pieces(starts, false) {
            let hay = self.hay(bytes, &piece, &mut buf);
            let mut at = 0;
            while let Some(i) = self.forward.find(&hay[at..]) {
                if !f(piece.start + at + i) {
                    return;
                }
                at += i + 1;
            }
        }
    }
}

/// The first match starting at or after `from`, else (wrapping) the first one before it.
pub fn find_next(bytes: &[u8], q: &Query, from: usize) -> Option<Range<usize>> {
    let s = q.searcher()?;
    let from = from.min(bytes.len());
    let start = s.first(bytes, from..bytes.len()).or_else(|| s.first(bytes, 0..from))?;
    Some(start..start + q.len())
}

/// The last match starting before `before`, else (wrapping) the last one at or after it.
pub fn find_prev(bytes: &[u8], q: &Query, before: usize) -> Option<Range<usize>> {
    let s = q.searcher()?;
    let before = before.min(bytes.len());
    let start = s
        .last(bytes, 0..before)
        .or_else(|| s.last(bytes, before..bytes.len()))?;
    Some(start..start + q.len())
}

/// Every match overlapping `range` (a frame's visible bytes), in order: only `range` and the
/// query's length − 1 bytes each side are read.
pub fn find_in(bytes: &[u8], q: &Query, range: Range<usize>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let Some(s) = q.searcher() else { return found };
    let end = range.end.min(bytes.len());
    let start = range.start.saturating_sub(q.len() - 1).min(end);
    s.each(bytes, start..end, |m| {
        found.push(m..m + q.len());
        true
    });
    found
}

/// What `count` found: every match's start in order, stopping at the limit (`capped`: there
/// may be more). The starts give the "3 of 41" index of a selected match (`index_of`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Count {
    pub starts: Vec<usize>,
    pub capped: bool,
}

impl Count {
    pub fn found(&self) -> usize {
        self.starts.len()
    }

    /// The 0-based index of the match starting at `start`, when it's one of those counted.
    pub fn index_of(&self, start: usize) -> Option<usize> {
        self.starts.binary_search(&start).ok()
    }
}

/// Counts the matches in the whole file, up to `limit` (`HEX_MAX_COUNT` for the widget),
/// keeping their starts. Blocking: call from spawn_blocking (a whole-file scan).
pub fn count(bytes: &[u8], q: &Query, limit: usize) -> Count {
    let mut c = Count::default();
    let Some(s) = q.searcher() else { return c };
    if limit == 0 {
        c.capped = s.first(bytes, 0..bytes.len()).is_some();
        return c;
    }
    s.each(bytes, 0..bytes.len(), |m| {
        c.starts.push(m);
        if c.starts.len() == limit {
            c.capped = true;
            return false;
        }
        true
    });
    c
}

/// A hex tab (`group::Tab::Hex`): a file's bytes, a byte cursor and selection, which column the
/// cursor is in, and the scroll. Read only. `ui::hex` fits it to the area drawn each frame
/// (`fit`), which also scrolls a moved cursor into view (`reveal`); `app/hex.rs` runs its
/// commands and mouse. A split copies it (`Clone`: the bytes are shared).
#[derive(Debug, Clone)]
pub struct HexTab {
    /// Unique per tab opened (a process-wide counter; a split's copy keeps it: same bytes), for results
    /// computed off the UI thread (find's count, `AppEvent::HexCount`).
    pub id: u64,
    pub path: PathBuf,
    pub doc: HexDoc,
    /// The cursor's byte (0 in an empty file, which has none).
    pub cursor: usize,
    /// Where a selection started (Shift + movement, a drag, Cmd+A): the selection is every byte
    /// from it to the cursor, both included. `None`: no selection.
    pub anchor: Option<usize>,
    /// The column the cursor is in: what Cmd+C copies, where a click put it.
    pub column: Column,
    /// The top row's first byte (a multiple of the layout's bytes per row).
    pub top: usize,
    /// The layout drawn last (16 bytes per row until the first frame).
    pub layout: Layout,
    /// Rows on screen, as drawn last.
    pub height: usize,
    /// The next frame scrolls the cursor into view.
    pub reveal: bool,
    /// The file is gone from disk (the explorer deleted it): `(deleted)` in the tab bar.
    pub deleted: bool,
}

impl HexTab {
    pub fn new(path: PathBuf, doc: HexDoc) -> Self {
        let layout = Layout::fit(row_width(16, 8), 16, doc.len());
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            path,
            doc,
            cursor: 0,
            anchor: None,
            column: Column::Hex,
            top: 0,
            layout,
            height: 1,
            reveal: false,
            deleted: false,
        }
    }

    /// The tab's title: the file's name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    pub fn len(&self) -> usize {
        self.doc.len()
    }

    /// The last byte's offset (0 in an empty file).
    fn last(&self) -> usize {
        self.len().saturating_sub(1)
    }

    /// Rows at the current layout, the end offset's row included.
    pub fn rows(&self) -> usize {
        self.layout.rows(self.len())
    }

    pub fn top_row(&self) -> usize {
        self.top / self.layout.bytes_per_row()
    }

    /// Puts row `row` at the top, clamped so the last page stays full.
    fn set_top_row(&mut self, row: usize) {
        let max = self.rows().saturating_sub(self.height);
        self.top = row.min(max) * self.layout.bytes_per_row();
    }

    /// Scrolls by `rows` (the wheel), the cursor staying where it is.
    pub fn scroll_by(&mut self, rows: isize) {
        self.set_top_row(self.top_row().saturating_add_signed(rows));
    }

    /// Fits the tab to the area drawn (`max`: the `hex_bytes_per_line` setting): a new layout
    /// snaps the top down to its row's start, so the top row's first byte stays in view, and
    /// reveals the cursor; the top is clamped to the rows; a pending `reveal` is done.
    pub fn fit(&mut self, width: usize, height: usize, max: usize) {
        self.height = height.max(1);
        let layout = Layout::fit(width, max, self.len());
        if layout != self.layout {
            self.layout = layout;
            self.top -= self.top % layout.bytes_per_row();
            self.reveal = true;
        }
        self.set_top_row(self.top_row());
        if std::mem::take(&mut self.reveal) {
            let row = self.layout.row_of(self.cursor);
            // On the last bytes' row, the end offset's row below it comes into view too.
            let bottom = if row + 2 == self.rows() { row + 1 } else { row };
            let top = self.top_row();
            if row < top {
                self.set_top_row(row);
            } else if bottom >= top + self.height {
                self.set_top_row(bottom + 1 - self.height);
            }
        }
    }

    /// Moves the cursor to `offset` (clamped to the last byte), extending the selection when
    /// `select` (else dropping it), and scrolls it into view at the next frame.
    pub fn move_to(&mut self, offset: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset.min(self.last());
        self.reveal = true;
    }

    /// The editor's movement keys: Left / Right a byte, Up / Down a row, Home / End the row's
    /// first / last byte, Alt (Ctrl) + Left / Right to the previous / next 8-byte group, page up
    /// / down (scrolling the view a page too), the file start / end.
    pub fn motion(&mut self, motion: Motion, select: bool) {
        let n = self.layout.bytes_per_row();
        let c = self.cursor;
        let page = self.height.saturating_sub(1).max(1);
        let to = match motion {
            Motion::Left => c.saturating_sub(1),
            Motion::Right => c + 1,
            Motion::Up => c.saturating_sub(n),
            Motion::Down => c + n,
            Motion::LineStart => c - c % n,
            Motion::LineEnd => c - c % n + n - 1,
            Motion::WordLeft if !c.is_multiple_of(8) => c - c % 8,
            Motion::WordLeft => c.saturating_sub(8),
            Motion::WordRight => c - c % 8 + 8,
            Motion::DocStart => 0,
            Motion::DocEnd => self.last(),
            Motion::PageUp => {
                self.scroll_by(-(page as isize));
                c.saturating_sub(page * n)
            }
            Motion::PageDown => {
                self.scroll_by(page as isize);
                c + page * n
            }
        };
        self.move_to(to, select);
    }

    /// Selects `range` (a find match: not empty), the cursor on its last byte, and scrolls it
    /// into view at the next frame: its first row too when the whole of it fits.
    pub fn select(&mut self, range: Range<usize>) {
        if range.is_empty() || range.end > self.len() {
            return;
        }
        self.anchor = Some(range.start);
        self.cursor = range.end - 1;
        self.reveal = true;
        let first = self.layout.row_of(range.start);
        if first < self.top_row() {
            self.set_top_row(first);
        }
    }

    /// Cmd+A: every byte.
    pub fn select_all(&mut self) {
        if self.len() > 0 {
            self.anchor = Some(0);
            self.cursor = self.last();
            self.reveal = true;
        }
    }

    /// Tab: the cursor to the other column.
    pub fn toggle_column(&mut self) {
        self.column = match self.column {
            Column::Hex => Column::Ascii,
            Column::Ascii => Column::Hex,
        };
    }

    /// The selected bytes; `None` without a selection (or bytes).
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor.filter(|_| self.len() > 0)?;
        Some(a.min(self.cursor)..a.max(self.cursor) + 1)
    }

    /// What Cmd+C copies: the selection, else the cursor's byte, as the cursor's column shows
    /// them (hex pairs, or the ASCII text with `.`s); `None` in an empty file.
    pub fn copy_text(&self) -> Option<String> {
        let range = self
            .selection()
            .or((self.len() > 0).then(|| self.cursor..self.cursor + 1))?;
        let bytes = &self.doc.bytes()[range];
        Some(match self.column {
            Column::Hex => hex_pairs(bytes),
            Column::Ascii => ascii_text(bytes),
        })
    }

    /// The byte (and column) at screen row `row` (from the first drawn) and column `col` (from
    /// the rows' left edge), as a click picks it: a column that isn't a byte's (the offset, the
    /// `|`s, past the row) is `None`, unless `near`: then it's the row's first byte left of the
    /// bytes and its last one right of them, in the cursor's column (a drag past the edges).
    /// A row past the bytes (a short last row's empty cells, the end row, below it) gives the
    /// last byte. `None` in an empty file.
    pub fn offset_at(&self, row: usize, col: usize, near: bool) -> Option<(usize, Column)> {
        if self.len() == 0 {
            return None;
        }
        let l = &self.layout;
        let hit = l.hit(col).or_else(|| {
            near.then(|| Hit {
                index: if col < l.hex_col(0) { 0 } else { l.bytes_per_row() - 1 },
                column: self.column,
            })
        })?;
        let offset = (self.top_row() + row) * l.bytes_per_row() + hit.index;
        Some((offset.min(self.last()), hit.column))
    }
}

impl crate::scrollbar::Scrollable for HexTab {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.rows(), self.height, self.top_row())
    }

    /// The pointer decides where the view is: no reveal of the cursor's row.
    fn set_top(&mut self, top: usize) {
        self.set_top_row(top);
        self.reveal = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrollbar::Scrollable;

    fn dump(doc: &HexDoc, layout: &Layout) -> String {
        let mut out = String::new();
        for r in 0..layout.rows(doc.len()) {
            out.push_str(&row(doc, layout, layout.row_offset(r, doc.len())));
            out.push('\n');
        }
        out
    }

    /// Every byte value, then a short last row with a NUL in it.
    fn every_byte() -> Vec<u8> {
        let mut b: Vec<u8> = (0..=255).collect();
        b.extend_from_slice(b"jed!\x00\xff\n");
        b
    }

    /// `LC_ALL=C hexdump -C -v` of `every_byte()`, recorded.
    const EVERY_BYTE_DUMP: &str = r##"00000000  00 01 02 03 04 05 06 07  08 09 0a 0b 0c 0d 0e 0f  |................|
00000010  10 11 12 13 14 15 16 17  18 19 1a 1b 1c 1d 1e 1f  |................|
00000020  20 21 22 23 24 25 26 27  28 29 2a 2b 2c 2d 2e 2f  | !"#$%&'()*+,-./|
00000030  30 31 32 33 34 35 36 37  38 39 3a 3b 3c 3d 3e 3f  |0123456789:;<=>?|
00000040  40 41 42 43 44 45 46 47  48 49 4a 4b 4c 4d 4e 4f  |@ABCDEFGHIJKLMNO|
00000050  50 51 52 53 54 55 56 57  58 59 5a 5b 5c 5d 5e 5f  |PQRSTUVWXYZ[\]^_|
00000060  60 61 62 63 64 65 66 67  68 69 6a 6b 6c 6d 6e 6f  |`abcdefghijklmno|
00000070  70 71 72 73 74 75 76 77  78 79 7a 7b 7c 7d 7e 7f  |pqrstuvwxyz{|}~.|
00000080  80 81 82 83 84 85 86 87  88 89 8a 8b 8c 8d 8e 8f  |................|
00000090  90 91 92 93 94 95 96 97  98 99 9a 9b 9c 9d 9e 9f  |................|
000000a0  a0 a1 a2 a3 a4 a5 a6 a7  a8 a9 aa ab ac ad ae af  |................|
000000b0  b0 b1 b2 b3 b4 b5 b6 b7  b8 b9 ba bb bc bd be bf  |................|
000000c0  c0 c1 c2 c3 c4 c5 c6 c7  c8 c9 ca cb cc cd ce cf  |................|
000000d0  d0 d1 d2 d3 d4 d5 d6 d7  d8 d9 da db dc dd de df  |................|
000000e0  e0 e1 e2 e3 e4 e5 e6 e7  e8 e9 ea eb ec ed ee ef  |................|
000000f0  f0 f1 f2 f3 f4 f5 f6 f7  f8 f9 fa fb fc fd fe ff  |................|
00000100  6a 65 64 21 00 ff 0a                              |jed!...|
00000107
"##;

    /// The real `hexdump -C -v` of `bytes`, or `None` (with a note) when it isn't installed.
    fn hexdump(bytes: &[u8]) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        std::fs::write(&path, bytes).unwrap();
        match std::process::Command::new("hexdump")
            .args(["-C", "-v"])
            .arg(&path)
            .env("LC_ALL", "C")
            .output()
        {
            Ok(out) if out.status.success() => Some(String::from_utf8(out.stdout).unwrap()),
            _ => {
                eprintln!("note: no hexdump CLI, checked against the recorded fixture only");
                None
            }
        }
    }

    fn layout16(len: usize) -> Layout {
        let l = Layout::fit(78, 32, len);
        assert_eq!(l.bytes_per_row(), 16);
        l
    }

    #[test]
    fn rows_match_the_recorded_hexdump() {
        let doc = HexDoc::new(every_byte());
        assert_eq!(dump(&doc, &layout16(doc.len())), EVERY_BYTE_DUMP);
    }

    #[test]
    fn rows_match_the_hexdump_cli() {
        let mut cases = vec![
            every_byte(),
            Vec::new(),
            b"ABCDEFGHIJKLMNOP".to_vec(),
            b"x".to_vec(),
            b"0123456789abcdefg".to_vec(),
        ];
        cases.push((0..1000u32).map(|i| (i * 7 % 251) as u8).collect());
        // Every last-row length (and the 8-byte group's edge: 7, 8, 9, 15 bytes).
        for len in 0..=40u32 {
            cases.push((0..len).map(|i| (i * 37 + 5) as u8).collect());
        }
        for bytes in cases {
            let Some(want) = hexdump(&bytes) else { return };
            let doc = HexDoc::new(bytes);
            assert_eq!(dump(&doc, &layout16(doc.len())), want, "{} bytes", doc.len());
        }
    }

    #[test]
    fn empty_file_has_no_rows_and_a_full_last_row_gets_an_end_row() {
        let empty = HexDoc::new(Vec::new());
        assert_eq!(dump(&empty, &layout16(0)), "", "hexdump prints nothing");
        let doc = HexDoc::new(b"ABCDEFGHIJKLMNOP".to_vec());
        assert_eq!(
            dump(&doc, &layout16(16)),
            "00000000  41 42 43 44 45 46 47 48  49 4a 4b 4c 4d 4e 4f 50  |ABCDEFGHIJKLMNOP|\n00000010\n"
        );
    }

    #[test]
    fn nine_digit_offsets() {
        // A 4 GiB file's offsets need 9 digits: the layout is fitted to that size, the rows come
        // from a small doc.
        let l = Layout::fit(79, 16, 1 << 32);
        assert_eq!((l.offset_digits(), l.bytes_per_row(), l.width()), (9, 16, 79));
        assert_eq!(Layout::fit(78, 16, 1 << 32).bytes_per_row(), 8, "one column short");
        assert_eq!(Layout::fit(80, 16, (1 << 32) - 1).offset_digits(), 8);
        let doc = HexDoc::new(b"ABC".to_vec());
        assert_eq!(
            row(&doc, &l, 0),
            "000000000  41 42 43                                          |ABC|"
        );
        assert_eq!(row(&doc, &l, 3), "000000003");
        assert_eq!(l.hex_col(0), 11);
        assert_eq!(l.bar_col(), 61);
    }

    #[test]
    fn other_widths() {
        let doc = HexDoc::new(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_vec());
        let l = Layout::fit(144, 64, doc.len());
        assert_eq!((l.bytes_per_row(), l.width()), (32, 144));
        let full = "00000000  41 42 43 44 45 46 47 48  49 4a 4b 4c 4d 4e 4f 50  51 52 53 54 55 56 57 58  \
                    59 5a 30 31 32 33 34 35  |ABCDEFGHIJKLMNOPQRSTUVWXYZ012345|";
        let short = format!("00000020  36 37 38 39{}|6789|", " ".repeat(110 - 21));
        assert_eq!(dump(&doc, &l), format!("{full}\n{short}\n00000024\n"));
        let l = Layout::fit(77, 64, doc.len());
        assert_eq!((l.bytes_per_row(), l.width()), (8, 45));
        assert_eq!(row(&doc, &l, 8), "00000008  49 4a 4b 4c 4d 4e 4f 50  |IJKLMNOP|");
        let l = Layout::fit(43, 64, doc.len());
        assert_eq!((l.bytes_per_row(), l.width()), (4, 29));
        assert_eq!(row(&doc, &l, 4), "00000004  45 46 47 48  |EFGH|");
        assert_eq!(row(&doc, &l, 36), "00000024");
        let l = Layout::fit(28, 64, doc.len());
        assert_eq!((l.bytes_per_row(), l.width()), (2, 21));
        assert_eq!(row(&doc, &l, 2), "00000002  43 44  |CD|");
        let l = Layout::fit(10, 64, doc.len());
        assert_eq!((l.bytes_per_row(), l.width()), (1, 17), "below one byte's width: cut");
        assert_eq!(row(&doc, &l, 0), "00000000  41  |A|");
        let l = Layout::fit(1000, 64, 100);
        assert_eq!(l.bytes_per_row(), 64);
        assert_eq!(l.width(), 8 + 2 + 192 + 8 + 66);
        assert_eq!(
            Layout::fit(1000, 48, 100).bytes_per_row(),
            32,
            "max floored to a power of two"
        );
        assert_eq!(Layout::fit(1000, 0, 100).bytes_per_row(), 1);
        assert_eq!(Layout::fit(1000, 1000, 100).bytes_per_row(), 64);
    }

    #[test]
    fn fit_picks_the_largest_power_of_two_that_fits() {
        for max in [1, 2, 4, 8, 16, 32, 64] {
            for width in 0..300 {
                let l = Layout::fit(width, max, 1000);
                let n = l.bytes_per_row();
                assert!(n.is_power_of_two() && n <= max);
                assert!(n == 1 || l.width() <= width, "{width} {max}");
                if n < max {
                    assert!(row_width(n * 2, 8) > width, "{width} {max}: {n} isn't the largest");
                }
            }
        }
        assert_eq!(Layout::fit(78, 32, 1).bytes_per_row(), 16);
        assert_eq!(Layout::fit(143, 32, 1).bytes_per_row(), 16);
        assert_eq!(Layout::fit(144, 32, 1).bytes_per_row(), 32);
    }

    /// Every column of every row width agrees with the formatted row: each byte's hex digits and
    /// ASCII char are where the layout says, and `hit` maps them (and nothing else) back.
    #[test]
    fn columns_and_hits_match_the_row() {
        let bytes: Vec<u8> = (0..64u8).map(|i| b'a' + i % 26).collect();
        let doc = HexDoc::new(bytes.clone());
        for max in [1, 2, 4, 8, 16, 32, 64] {
            let l = Layout::fit(1000, max, doc.len());
            let n = l.bytes_per_row();
            let text: Vec<char> = row(&doc, &l, 0).chars().collect();
            assert_eq!(text.len(), l.width());
            assert_eq!(l.offset_cols(), 0..8);
            assert_eq!(text[l.bar_col()], '|');
            assert_eq!(text[l.ascii_col(n)], '|');
            assert_eq!(l.ascii_col(n) + 1, l.width());
            for i in 0..n {
                let h = l.hex_col(i);
                assert_eq!(text[h..h + 2].iter().collect::<String>(), format!("{:02x}", bytes[i]));
                assert_eq!(text[l.ascii_col(i)], bytes[i] as char);
                for c in [h, h + 1, h + 2] {
                    assert_eq!(
                        l.hit(c),
                        Some(Hit {
                            index: i,
                            column: Column::Hex
                        }),
                        "{n} {c}"
                    );
                }
                assert_eq!(
                    l.hit(l.ascii_col(i)),
                    Some(Hit {
                        index: i,
                        column: Column::Ascii
                    })
                );
            }
            for col in 0..l.hex_col(0) {
                assert_eq!(l.hit(col), None);
            }
            for col in l.hex_col(0)..l.bar_col() {
                let hit = l.hit(col).unwrap();
                assert_eq!(hit.column, Column::Hex);
                assert!(
                    l.hex_col(hit.index) <= col && col < l.hex_col(hit.index + 1),
                    "{n} {col}"
                );
            }
            assert_eq!(l.hit(l.bar_col()), None);
            assert_eq!(l.hit(l.ascii_col(n)), None);
            assert_eq!(l.hit(l.width() + 5), None);
        }
    }

    #[test]
    fn row_arithmetic() {
        let l = layout16(263);
        assert_eq!(l.rows(263), 18);
        assert_eq!(l.rows(256), 17);
        assert_eq!(l.rows(1), 2);
        assert_eq!(l.rows(0), 0);
        assert_eq!(l.row_offset(16, 263), 256);
        assert_eq!(l.row_offset(17, 263), 263, "the end row");
        assert_eq!(l.row_of(255), 15);
        assert_eq!(l.row_of(256), 16);
    }

    #[test]
    fn copy_texts() {
        assert_eq!(hex_pairs(b"\x7fELF"), "7f 45 4c 46");
        assert_eq!(hex_pairs(b""), "");
        assert_eq!(ascii_text(b"\x7fELF\x00 ~"), ".ELF. ~");
        assert_eq!(printable_text(b"ELF ~"), Some("ELF ~".into()));
        assert_eq!(printable_text(b"EL\nF"), None);
        assert_eq!(printable_text("é".as_bytes()), None);
    }

    #[test]
    fn parses_offsets() {
        for (text, want) in [
            ("0x1f0", Some(0x1f0)),
            ("0X1F0", Some(0x1f0)),
            ("1f0h", Some(0x1f0)),
            ("1F0H", Some(0x1f0)),
            ("496", Some(496)),
            ("  0x100 ", Some(0x100)),
            ("0", Some(0)),
            ("0h", Some(0)),
            ("1f0", None),
            ("0x", None),
            ("h", None),
            ("", None),
            ("-1", None),
            ("0xg", None),
            ("12 3", None),
            ("0xffffffffffffffffff", None),
        ] {
            assert_eq!(parse_offset(text), want, "{text:?}");
        }
    }

    #[test]
    fn parses_hex_queries() {
        assert_eq!(parse_hex("7f454c46"), Ok(vec![0x7f, 0x45, 0x4c, 0x46]));
        assert_eq!(parse_hex("7f 45 4c 46"), Ok(vec![0x7f, 0x45, 0x4c, 0x46]));
        assert_eq!(parse_hex(" 7F 4\t5 "), Ok(vec![0x7f, 0x45]));
        assert_eq!(parse_hex(""), Ok(vec![]));
        assert_eq!(parse_hex("7f4"), Err(HexError::Odd));
        assert_eq!(parse_hex("7g"), Err(HexError::NotHex('g')));
        assert_eq!(parse_hex("7f 0x45"), Err(HexError::NotHex('x')));
        assert_eq!(HexError::Odd.to_string(), "hex bytes come in pairs");
        assert_eq!(HexError::NotHex('g').to_string(), "not a hex digit: g");
        assert_eq!(Query::parse("45 4c", Mode::Hex, true), Ok(Query::Hex(vec![0x45, 0x4c])));
        assert_eq!(Query::parse("4", Mode::Hex, true), Err(HexError::Odd));
        assert_eq!(
            Query::parse("4", Mode::Ascii, false),
            Ok(Query::Ascii {
                text: "4".into(),
                case_sensitive: false
            })
        );
    }

    #[test]
    fn converts_between_modes() {
        assert_eq!(convert("ELF", Mode::Hex).as_deref(), Some("45 4c 46"));
        assert_eq!(convert("é", Mode::Hex).as_deref(), Some("c3 a9"));
        assert_eq!(convert("45 4c 46", Mode::Ascii).as_deref(), Some("ELF"));
        assert_eq!(convert("454C46", Mode::Ascii).as_deref(), Some("ELF"));
        assert_eq!(convert("7f 45", Mode::Ascii), None, "not printable: the text stays");
        assert_eq!(convert("4", Mode::Ascii), None, "not valid: the text stays");
        assert_eq!(convert("ELF", Mode::Ascii), None, "`L` isn't hex");
        // Round trip.
        let hex = convert("Hello, world!", Mode::Hex).unwrap();
        assert_eq!(convert(&hex, Mode::Ascii).as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn doc_shares_its_bytes() {
        let doc = HexDoc::new(b"abc".to_vec());
        let copy = doc.clone();
        assert!(std::ptr::eq(doc.bytes(), copy.bytes()));
        assert_eq!(doc.stamp(), Stamp::of(b"abc"));
        assert_eq!((doc.len(), doc.is_empty()), (3, false));
        assert!(HexDoc::with_stamp(Vec::new(), Stamp::of(b"")).is_empty());
    }

    // --- The tab ---

    fn tab(len: usize) -> HexTab {
        let mut t = HexTab::new(
            PathBuf::from("/bin.dat"),
            HexDoc::new((0..len).map(|i| i as u8).collect::<Vec<_>>()),
        );
        t.fit(78, 4, 32);
        t
    }

    #[test]
    fn tab_moves_and_selects() {
        let mut t = tab(100);
        assert_eq!((t.layout.bytes_per_row(), t.rows(), t.height), (16, 8, 4));
        t.motion(Motion::Right, false);
        t.motion(Motion::Down, false);
        assert_eq!(t.cursor, 17);
        t.motion(Motion::LineEnd, false);
        assert_eq!(t.cursor, 31);
        t.motion(Motion::LineStart, false);
        assert_eq!(t.cursor, 16);
        t.motion(Motion::WordRight, true);
        assert_eq!((t.cursor, t.selection()), (24, Some(16..25)));
        t.motion(Motion::WordLeft, true);
        t.motion(Motion::WordLeft, true);
        assert_eq!((t.cursor, t.selection()), (8, Some(8..17)), "back past the anchor");
        t.motion(Motion::Left, false);
        assert_eq!((t.cursor, t.anchor), (7, None));
        t.motion(Motion::Up, false);
        assert_eq!(t.cursor, 0, "the first row: to the start");
        t.motion(Motion::DocEnd, false);
        assert_eq!(t.cursor, 99);
        t.motion(Motion::Right, false);
        t.motion(Motion::Down, false);
        t.motion(Motion::LineEnd, false);
        assert_eq!(t.cursor, 99, "never past the last byte");
        // Its reveal scrolls the last byte's row in, and the end row below it: rows 4..8.
        t.fit(78, 4, 32);
        assert_eq!(t.top_row(), 4);
        t.motion(Motion::PageUp, false);
        assert_eq!((t.cursor, t.top_row()), (99 - 3 * 16, 1));
        t.motion(Motion::PageDown, true);
        assert_eq!((t.cursor, t.top_row()), (99, 4));
        t.motion(Motion::DocStart, false);
        t.select_all();
        assert_eq!(t.selection(), Some(0..100));
        assert_eq!(t.copy_text().unwrap().len(), 100 * 3 - 1);
    }

    #[test]
    fn tab_copies_its_column() {
        let mut t = HexTab::new(PathBuf::from("/x"), HexDoc::new(b"\x7fELF\x00".to_vec()));
        assert_eq!(t.copy_text().as_deref(), Some("7f"), "the cursor's byte");
        t.move_to(1, false);
        t.move_to(3, true);
        assert_eq!(t.copy_text().as_deref(), Some("45 4c 46"));
        t.toggle_column();
        assert_eq!(t.copy_text().as_deref(), Some("ELF"));
        t.select_all();
        assert_eq!(t.copy_text().as_deref(), Some(".ELF."));
        let mut empty = HexTab::new(PathBuf::from("/e"), HexDoc::new(Vec::new()));
        empty.select_all();
        empty.motion(Motion::Right, true);
        assert_eq!((empty.cursor, empty.selection(), empty.copy_text()), (0, None, None));
        assert_eq!(empty.offset_at(0, 12, true), None);
    }

    #[test]
    fn resizing_keeps_the_top_byte_and_the_cursor_in_view() {
        let mut t = tab(1000);
        t.set_top(10);
        assert_eq!(t.top, 160);
        t.move_to(200, false);
        t.reveal = false;
        // 32 per row: the top snaps down to 160 (row 5), the cursor's row 6 is shown.
        t.fit(144, 4, 32);
        assert_eq!((t.layout.bytes_per_row(), t.top), (32, 160));
        // 8 per row: row 20 at the top (byte 160); the cursor's byte 200 is row 25, revealed.
        t.fit(60, 4, 32);
        assert_eq!(t.layout.bytes_per_row(), 8);
        assert_eq!(t.top_row(), 22);
        assert!((t.top..t.top + 4 * 8).contains(&200));
        // The wheel doesn't follow the cursor; clamped to the last full page.
        t.scroll_by(1000);
        assert_eq!(t.top_row(), t.rows() - 4);
        t.fit(60, 4, 32);
        assert_eq!(t.top_row(), t.rows() - 4, "no reveal without a move");
    }

    #[test]
    fn clicks_map_to_bytes() {
        let mut t = tab(40);
        t.fit(78, 2, 32);
        t.set_top(1);
        let l = t.layout;
        assert_eq!(t.offset_at(0, l.hex_col(3), false), Some((19, Column::Hex)));
        assert_eq!(t.offset_at(1, l.ascii_col(2), false), Some((34, Column::Ascii)));
        assert_eq!(
            t.offset_at(1, l.ascii_col(12), false),
            Some((39, Column::Ascii)),
            "a short row's end: the last byte"
        );
        assert_eq!(
            t.offset_at(5, l.hex_col(0), false),
            Some((39, Column::Hex)),
            "below the bytes"
        );
        assert_eq!(t.offset_at(0, 2, false), None, "the offset");
        assert_eq!(
            t.offset_at(0, 2, true),
            Some((16, Column::Hex)),
            "a drag: the row's start"
        );
        assert_eq!(
            t.offset_at(0, 200, true),
            Some((31, Column::Hex)),
            "a drag: the row's end"
        );
    }

    // --- Search, against a naive reference ---

    fn naive_matches(bytes: &[u8], q: &Query) -> Vec<usize> {
        let needle = q.bytes();
        if needle.is_empty() || needle.len() > bytes.len() {
            return Vec::new();
        }
        let fold = q.folds();
        (0..=bytes.len() - needle.len())
            .filter(|&i| {
                let hay = &bytes[i..i + needle.len()];
                if fold {
                    hay.eq_ignore_ascii_case(needle)
                } else {
                    hay == needle
                }
            })
            .collect()
    }

    fn rng(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        }
    }

    /// Small alphabets so matches are many and overlap; sizes past `FOLD_CHUNK` so the folded
    /// search crosses chunk boundaries.
    #[test]
    fn search_matches_a_naive_reference() {
        let mut rand = rng(0x9e37_79b9_7f4a_7c15);
        const ALPHABET: &[u8] = b"aAbB\x00\xff";
        for case in 0..60 {
            let len = match case % 4 {
                0 => rand(40),
                1 => rand(400),
                2 => FOLD_CHUNK + rand(2000),
                _ => 2 * FOLD_CHUNK + rand(100),
            };
            let alphabet = &ALPHABET[..2 + rand(ALPHABET.len() - 1)];
            let bytes: Vec<u8> = (0..len).map(|_| alphabet[rand(alphabet.len())]).collect();
            let qlen = 1 + rand(4);
            let needle: Vec<u8> = (0..qlen).map(|_| alphabet[rand(alphabet.len())]).collect();
            let q = match rand(3) {
                0 => Query::Hex(needle),
                k => match String::from_utf8(needle) {
                    Ok(text) => Query::Ascii {
                        text,
                        case_sensitive: k == 1,
                    },
                    Err(e) => Query::Hex(e.into_bytes()),
                },
            };
            let want = naive_matches(&bytes, &q);
            let ctx = format!("case {case}: {} bytes, {q:?}", bytes.len());

            let c = count(&bytes, &q, usize::MAX);
            assert_eq!(
                c,
                Count {
                    starts: want.clone(),
                    capped: false
                },
                "{ctx}"
            );
            if let Some(&m) = want.get(1) {
                assert_eq!(c.index_of(m), Some(1), "{ctx}");
            }
            if want.len() > 2 {
                let c = count(&bytes, &q, want.len() - 1);
                assert_eq!(
                    c,
                    Count {
                        starts: want[..want.len() - 1].to_vec(),
                        capped: true
                    },
                    "{ctx}"
                );
            }
            assert_eq!(count(&bytes, &q, 0).capped, !want.is_empty(), "{ctx}");

            for _ in 0..20 {
                let from = rand(bytes.len() + 2);
                let next = want
                    .iter()
                    .find(|&&m| m >= from)
                    .or(want.first())
                    .map(|&m| m..m + q.len());
                assert_eq!(find_next(&bytes, &q, from), next, "{ctx} next from {from}");
                let prev = want
                    .iter()
                    .rev()
                    .find(|&&m| m < from)
                    .or(want.last())
                    .map(|&m| m..m + q.len());
                assert_eq!(find_prev(&bytes, &q, from), prev, "{ctx} prev before {from}");

                let a = rand(bytes.len() + 1);
                let b = (a + rand(300)).min(bytes.len() + 3);
                let inside: Vec<Range<usize>> = want
                    .iter()
                    .filter(|&&m| m < b && m + q.len() > a)
                    .map(|&m| m..m + q.len())
                    .collect();
                assert_eq!(find_in(&bytes, &q, a..b), inside, "{ctx} in {a}..{b}");
            }
        }
    }

    #[test]
    fn search_edge_cases() {
        let q = |s: &str, cs: bool| Query::Ascii {
            text: s.into(),
            case_sensitive: cs,
        };
        let bytes = b"\x7fELF elf Elf";
        assert_eq!(find_next(bytes, &q("elf", false), 0), Some(1..4));
        assert_eq!(find_next(bytes, &q("elf", true), 0), Some(5..8));
        assert_eq!(find_next(bytes, &q("elf", true), 6), Some(5..8), "wraps");
        assert_eq!(find_prev(bytes, &q("elf", false), 1), Some(9..12), "wraps");
        assert_eq!(find_next(bytes, &Query::Hex(vec![0x45, 0x4c, 0x46]), 0), Some(1..4));
        assert_eq!(count(bytes, &q("ELF", false), 100).found(), 3);
        // Empty queries and haystacks match nothing.
        assert_eq!(find_next(bytes, &q("", false), 0), None);
        assert!(find_in(bytes, &Query::Hex(vec![]), 0..12).is_empty());
        assert_eq!(count(bytes, &q("", true), 10).found(), 0);
        assert_eq!(find_prev(b"", &q("a", true), 0), None);
        assert_eq!(find_next(b"ab", &q("abc", true), 0), None);
        // Overlapping matches.
        assert_eq!(find_in(b"aaaa", &q("aa", true), 0..4), vec![0..2, 1..3, 2..4]);
        assert_eq!(find_in(b"aaaa", &q("aa", true), 3..4), vec![2..4], "reaches back");
        // Non-ASCII text is matched by its bytes, folding only ASCII letters.
        assert_eq!(find_next("xÉé".as_bytes(), &q("é", false), 0), Some(3..5));
    }
}
