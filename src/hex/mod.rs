//! The hex view's model (PLAN.md "Binary files (hex view)"): a file's bytes as `hexdump -C` prints
//! them, and byte search. No UI and no app state: the hex tab (`Tab::Hex`) holds a `HexDoc` (a
//! byte `Source`, resident or paged, and its page cache), fits a `Layout` to its width and
//! formats only the visible rows with `row` / `row_in` (a byte whose page isn't loaded yet is a
//! placeholder); find on a hex tab builds a `Query` and calls `find_next` / `find_prev` / `find_in`
//! on the UI thread for a resident doc, `stream_next` / `stream_prev` / `stream_count` off it for a
//! paged one, and `count` off it.
//!
//! Rows: offset `Layout::offset_digits` wide, two spaces, each byte as `xx ` with one extra space
//! after every 8th (and after the last when a row has fewer than 8), then `|`, the bytes as ASCII
//! (`ascii_char`), `|`. A short last row pads its hex part so the `|` lines up; one more row holds
//! only the end offset (the file's size); an empty file has no rows, as hexdump prints nothing.
//! No `*` squeezing (hexdump `-v`), so row `i` starts at byte `i * bytes_per_row`. One deviation:
//! every row's offset is as wide as the file's size needs (hexdump widens each offset on its own
//! past 8 digits), so the columns stay put; it only shows for files of 4 GiB and more.
//!
//! Matches are byte ranges `start..start + query.len()` and may overlap (`aa` is found 3 times in
//! `aaaa`): that makes the matches in a window independent of where the window starts, so a frame
//! can search only its visible bytes and agree with `count` over the whole file.
//!
//! `page` (a file's bytes resident or paged, and the page cache) and `stream` (`find_next` and
//! friends over a paged file) are the model for files too big to hold: a frame asks the doc for
//! its visible bytes (`HexDoc::window`), which queues the pages it lacks; the app reads them off
//! the UI thread (`plan_reads`, `finish_reads`).

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{self, Write as _};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use memchr::memmem;

use crate::command::Motion;

mod inspect;
mod page;
mod stream;

pub use inspect::*;
pub use page::*;
pub use stream::*;

// Byte offsets are `u64` (files of any size) while `HexTab`'s cursor and scroll stay `usize`:
// on a 64-bit target the casts between them are lossless.
const _: () = assert!(usize::BITS == 64);

/// `count` stops here: the find widget then says "100000+".
pub const HEX_MAX_COUNT: usize = 100_000;
/// The largest `hex_bytes_per_line` (the setting is a power of two from 1 to this).
pub const MAX_BYTES_PER_ROW: usize = 64;
/// Cmd+C copies at most this many bytes of a selection (the clipboard text is three times it).
pub const HEX_COPY_MAX: u64 = 16 << 20;
/// Pages one background read job takes.
const JOB_PAGES: usize = 4;

/// A file's bytes (`Source`) and the pages read of it so far, shared (cloning is an `Arc` clone:
/// a tab and its splits share one cache and one set of reads in flight).
#[derive(Debug, Clone)]
pub struct HexDoc {
    source: Source,
    pages: Arc<Mutex<Pages>>,
}

/// What a doc's frames and page reads share (the UI thread's: never held across a read).
#[derive(Debug)]
struct Pages {
    cache: PageCache,
    /// Pages being read by a job.
    inflight: HashSet<u64>,
    /// Pages that failed to read (the file shrank under us): drawn unreadable, tried again after
    /// the next disk check (`retry_failed`).
    failed: HashSet<u64>,
    /// Pages the frames drawn since the last `plan_reads` need, and ones near them.
    visible: Vec<u64>,
    prefetch: Vec<u64>,
}

/// How one byte of a `Window` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Byte(u8),
    /// Its page is being read.
    Pending,
    /// Its page can't be read.
    Unreadable,
}

/// A run of bytes as a frame draws them: loaded ones as they are, the rest as placeholders.
#[derive(Debug, Clone)]
pub struct Window {
    pub start: u64,
    pub cells: Vec<Cell>,
}

impl Window {
    pub fn get(&self, offset: u64) -> Option<Cell> {
        let i = usize::try_from(offset.checked_sub(self.start)?).ok()?;
        self.cells.get(i).copied()
    }
}

impl HexDoc {
    /// A resident doc over `bytes`: hashes them for the version, blocking for a big one (call
    /// from `spawn_blocking`).
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::from_source(Source::resident(bytes))
    }

    pub fn from_source(source: Source) -> Self {
        let cache = PageCache::new(source.version());
        Self {
            source,
            pages: Arc::new(Mutex::new(Pages {
                cache,
                inflight: HashSet::new(),
                failed: HashSet::new(),
                visible: Vec::new(),
                prefetch: Vec::new(),
            })),
        }
    }

    fn pages(&self) -> MutexGuard<'_, Pages> {
        self.pages.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// What the bytes were read from (a disk check compares it, find's cache key holds it).
    pub fn version(&self) -> Version {
        self.source.version()
    }

    pub fn len(&self) -> usize {
        self.source.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// The whole file, for a resident doc (it is searched and copied on the UI thread).
    pub fn resident_bytes(&self) -> Option<&[u8]> {
        self.source.resident_bytes()
    }

    /// The whole file (empty for a paged doc), for tests.
    #[cfg(test)]
    pub fn bytes(&self) -> &[u8] {
        self.resident_bytes().unwrap_or(&[])
    }

    /// Bytes the page cache holds now (a resident doc has none).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cached_bytes(&self) -> usize {
        self.pages().cache.bytes()
    }

    /// `range` (clipped to the file) as a frame draws it. A paged doc's missing pages are
    /// placeholders and are queued for `plan_reads` (the visible ones, and the pages up to one
    /// range's length either side); never reads the file.
    pub fn window(&self, range: Range<u64>) -> Window {
        let len = self.source.len();
        let end = range.end.min(len);
        let start = range.start.min(end);
        if let Some(bytes) = self.resident_bytes() {
            return Window {
                start,
                cells: bytes[start as usize..end as usize]
                    .iter()
                    .map(|&b| Cell::Byte(b))
                    .collect(),
            };
        }
        let n = end - start;
        let mut buf = vec![0u8; n as usize];
        let version = self.version();
        let mut pages = self.pages();
        let missing = pages.cache.copy_range(version, start, &mut buf);
        let mut cells: Vec<Cell> = buf.into_iter().map(Cell::Byte).collect();
        for &page in &missing {
            let bytes = page_bytes(page, len);
            let cell = if pages.failed.contains(&page) {
                Cell::Unreadable
            } else {
                pages.visible.push(page);
                Cell::Pending
            };
            let lo = (bytes.start.max(start) - start) as usize;
            let hi = (bytes.end.min(end) - start) as usize;
            cells[lo..hi].fill(cell);
        }
        for page in pages_for(start.saturating_sub(n)..end.saturating_add(n).min(len)) {
            if !pages.cache.contains(version, page) && !pages.failed.contains(&page) && !missing.contains(&page) {
                pages.prefetch.push(page);
            }
        }
        Window { start, cells }
    }

    /// The byte at `offset` when it is loaded.
    pub fn byte(&self, offset: u64) -> Option<u8> {
        if let Some(bytes) = self.resident_bytes() {
            return bytes.get(offset as usize).copied();
        }
        self.pages().cache.byte(self.version(), offset)
    }

    /// `range`'s bytes when every one is loaded (nothing is read or queued).
    pub fn loaded_bytes(&self, range: Range<u64>) -> Option<Vec<u8>> {
        if let Some(bytes) = self.resident_bytes() {
            return bytes.get(range.start as usize..range.end as usize).map(<[u8]>::to_vec);
        }
        let mut out = vec![0u8; (range.end - range.start) as usize];
        let missing = self.pages().cache.copy_range(self.version(), range.start, &mut out);
        missing.is_empty().then_some(out)
    }

    /// The reads to start for the pages frames asked for since the last call: jobs of a few
    /// pages, the visible ones first, none for pages loaded, failed or already being read.
    pub fn plan_reads(&self) -> Vec<Vec<u64>> {
        let mut pages = self.pages();
        let version = self.version();
        let mut want = std::mem::take(&mut pages.visible);
        want.extend(std::mem::take(&mut pages.prefetch));
        let mut seen = HashSet::new();
        want.retain(|&p| {
            seen.insert(p)
                && !pages.inflight.contains(&p)
                && !pages.failed.contains(&p)
                && !pages.cache.contains(version, p)
        });
        pages.inflight.extend(want.iter().copied());
        want.chunks(JOB_PAGES).map(<[u64]>::to_vec).collect()
    }

    /// A job's pages arrived: loaded ones cached, failed ones remembered as unreadable. Dropped
    /// (false) when they were read for another version.
    pub fn finish_reads(&self, version: Version, read: Vec<(u64, io::Result<Arc<[u8]>>)>) -> bool {
        if version != self.version() {
            return false;
        }
        let mut pages = self.pages();
        for (page, result) in read {
            pages.inflight.remove(&page);
            match result {
                Ok(bytes) => pages.cache.insert(version, page, bytes),
                Err(e) => {
                    tracing::warn!(page, "can't read a page of the file: {e}");
                    pages.failed.insert(page);
                }
            }
        }
        true
    }

    /// Pages that failed are tried again (after a disk check found the file as it was).
    pub fn retry_failed(&self) -> bool {
        let mut pages = self.pages();
        let any = !pages.failed.is_empty();
        pages.failed.clear();
        any
    }
}

/// The file's size as the status bar shows it: `N bytes` below 1 MiB, else binary units with one
/// decimal and the exact count (`1.2 GB (1288490188 bytes)`).
pub fn size_text(len: u64) -> String {
    if len < 1 << 20 {
        return format!("{len} bytes");
    }
    let mut v = len as f64 / (1u64 << 20) as f64;
    let mut unit = "MB";
    for next in ["GB", "TB"] {
        if v >= 1024.0 {
            v /= 1024.0;
            unit = next;
        }
    }
    format!("{v:.1} {unit} ({len} bytes)")
}

/// Opens `path`'s bytes for a hex tab, whatever they are (resident up to `RESIDENT_MAX`, else
/// paged: nothing read): `Ok(None)` when it's missing.
/// Blocking: call from spawn_blocking.
pub fn read(path: &Path) -> io::Result<Option<HexDoc>> {
    Ok(open(path)?.map(HexDoc::from_source))
}

/// Whether `path` is too big to read whole (over `RESIDENT_MAX`) and looks binary
/// (`file::looks_binary`, reading only its start): it then opens in a hex tab paged, never read
/// into a text buffer first. False for a missing file.
/// Blocking: call from spawn_blocking.
pub fn is_big_binary(path: &Path) -> io::Result<bool> {
    use std::io::Read;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > RESIDENT_MAX => {}
        _ => return Ok(false),
    }
    let mut start = Vec::new();
    std::fs::File::open(path)?
        .take(crate::buffer::file::BINARY_SNIFF_LEN as u64)
        .read_to_end(&mut start)?;
    Ok(crate::buffer::file::looks_binary(&start))
}

/// What a hex tab's file holds now, against the `Version` its bytes were read with (`check`).
#[derive(Debug)]
pub enum OnDisk {
    /// No file there (deleted, or renamed away).
    Missing,
    /// The bytes the tab shows.
    Unchanged,
    /// Other bytes, opened (a new doc: its own empty cache).
    Changed(HexDoc),
}

/// Compares `path` with `known` (its hex tabs' `HexDoc::version`; `None`: they differ, so any
/// bytes are `Changed`), for the changed-on-disk check: `probe` (a tab's source) takes the file's
/// version the way the tabs' did (metadata for a paged file, a hash for a resident one), and a
/// changed file is opened again (a paged one isn't read).
/// Blocking: call from spawn_blocking.
pub fn check(path: &Path, probe: &Source, known: Option<Version>) -> io::Result<OnDisk> {
    let Some(now) = probe.current_version(path)? else {
        return Ok(OnDisk::Missing);
    };
    if Some(now) == known {
        return Ok(OnDisk::Unchanged);
    }
    Ok(match open(path)? {
        Some(source) => OnDisk::Changed(HexDoc::from_source(source)),
        None => OnDisk::Missing,
    })
}

/// Which column a byte is shown (or the cursor is) in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub fn fit(width: usize, max: usize, len: u64) -> Self {
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
    pub fn rows(&self, len: u64) -> usize {
        if len == 0 {
            0
        } else {
            len.div_ceil(self.bytes_per_row as u64) as usize + 1
        }
    }

    /// The offset row `row` prints: its first byte, or `len` for the last (end) row.
    pub fn row_offset(&self, row: usize, len: u64) -> u64 {
        (row as u64 * self.bytes_per_row as u64).min(len)
    }

    /// The row a byte is on.
    pub fn row_of(&self, offset: u64) -> usize {
        (offset / self.bytes_per_row as u64) as usize
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

fn offset_digits(len: u64) -> usize {
    let bits = u64::BITS - len.leading_zeros();
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
/// exactly as `hexdump -C -v` prints it, without the newline (a byte not loaded yet as `··`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn row(doc: &HexDoc, layout: &Layout, offset: u64) -> String {
    let window = doc.window(offset..offset + layout.bytes_per_row as u64);
    row_in(&window, layout, offset, doc.len() as u64)
}

/// `row` for a file of `len` bytes, its bytes taken from `window` (which must cover the row).
pub fn row_in(window: &Window, layout: &Layout, offset: u64, len: u64) -> String {
    debug_assert!(offset == len || (offset < len && offset.is_multiple_of(layout.bytes_per_row as u64)));
    let digits = layout.offset_digits;
    let mut s = String::with_capacity(layout.width());
    let _ = write!(s, "{offset:0digits$x}");
    if offset >= len {
        return s;
    }
    let count = len.min(offset + layout.bytes_per_row as u64) - offset;
    let cells = || (offset..offset + count).map(|o| window.get(o).unwrap_or(Cell::Pending));
    s.push_str("  ");
    for (i, c) in cells().enumerate() {
        match c {
            Cell::Byte(b) => {
                let _ = write!(s, "{b:02x} ");
            }
            Cell::Pending => s.push_str("\u{b7}\u{b7} "),
            Cell::Unreadable => s.push_str("?? "),
        }
        if (i + 1) % 8 == 0 {
            s.push(' ');
        }
    }
    // The hex part is spaces and 3-byte placeholder dots: pad by columns, not bytes.
    let cols = s.chars().count();
    let pad = layout.bar_col().saturating_sub(cols);
    s.extend(std::iter::repeat_n(' ', pad));
    s.push('|');
    s.extend(cells().map(|c| match c {
        Cell::Byte(b) => ascii_char(b),
        Cell::Pending => '\u{b7}',
        Cell::Unreadable => '?',
    }));
    s.push('|');
    s
}

/// Bytes as `column` shows them: what Cmd+C copies.
pub fn format_bytes(column: Column, bytes: &[u8]) -> String {
    match column {
        Column::Hex => hex_pairs(bytes),
        Column::Ascii => ascii_text(bytes),
    }
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
pub fn parse_offset(text: &str) -> Option<u64> {
    let t = text.trim();
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .or_else(|| t.strip_suffix('h'))
        .or_else(|| t.strip_suffix('H'));
    match hex {
        Some(h) if !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()) => u64::from_str_radix(h, 16).ok(),
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
    /// computed off the UI thread (find's count, `AppEvent::View(ViewEvent::HexCount)`).
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
    /// The data inspector row a click selected, and the offset it read from (valid only while the
    /// inspected offset is still that one: `HexTab::inspect`).
    pub inspected_row: Option<(RowKind, usize)>,
}

impl HexTab {
    pub fn new(path: PathBuf, doc: HexDoc) -> Self {
        let layout = Layout::fit(row_width(16, 8), 16, doc.len() as u64);
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
            inspected_row: None,
        }
    }

    /// The file's new bytes (changed on disk): the cursor keeps its offset and the selection
    /// its anchor, both clamped to the last byte (dropped in an empty file), the top stays at
    /// its row clamped to the rows; the next frame fits the layout (the offset digits may have
    /// changed) and reveals the cursor.
    pub fn set_doc(&mut self, doc: HexDoc) {
        let empty = doc.is_empty();
        self.doc = doc;
        self.deleted = false;
        let last = self.last();
        self.cursor = self.cursor.min(last);
        self.anchor = self.anchor.filter(|_| !empty).map(|a| a.min(last));
        self.set_top_row(self.top_row());
        self.reveal = true;
    }

    /// Puts the cursor and the top where a session left them (clamped: the file may have
    /// changed), the cursor in `column`, not revealed (the scroll is as it was).
    pub fn place(&mut self, cursor: usize, column: Column, top: usize) {
        self.cursor = cursor.min(self.last());
        self.column = column;
        self.anchor = None;
        self.top = top.min(self.len());
        self.set_top_row(self.top_row());
        self.reveal = false;
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
        self.layout.rows(self.len() as u64)
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
        let layout = Layout::fit(width, max, self.len() as u64);
        if layout != self.layout {
            self.layout = layout;
            self.top -= self.top % layout.bytes_per_row();
            self.reveal = true;
        }
        self.set_top_row(self.top_row());
        if std::mem::take(&mut self.reveal) {
            let row = self.layout.row_of(self.cursor as u64);
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
        let first = self.layout.row_of(range.start as u64);
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

    /// What Cmd+C copies: the selection, else the cursor's byte (`None` in an empty file).
    pub fn copy_range(&self) -> Option<Range<usize>> {
        self.selection()
            .or((self.len() > 0).then(|| self.cursor..self.cursor + 1))
    }

    /// What Cmd+C copies, for a resident doc (a paged one's bytes are read off the UI thread:
    /// `None`; the app does that).
    pub fn copy_text(&self) -> Option<String> {
        let bytes = self.doc.resident_bytes()?;
        Some(format_bytes(self.column, &bytes[self.copy_range()?]))
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
        for r in 0..layout.rows(doc.len() as u64) {
            out.push_str(&row(doc, layout, layout.row_offset(r, doc.len() as u64)));
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
        let l = Layout::fit(78, 32, len as u64);
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
    fn layout_and_offsets_past_4_gib() {
        let len: u64 = (6 << 30) + 5;
        let l = Layout::fit(200, 16, len);
        assert_eq!((l.offset_digits(), l.bytes_per_row()), (9, 16));
        assert_eq!(l.rows(len), (len.div_ceil(16) + 1) as usize);
        assert_eq!(l.row_offset(l.rows(len) - 1, len), len);
        assert_eq!(l.row_offset(1 << 28, len), 1 << 32);
        assert_eq!(l.row_of(len - 1), (len / 16) as usize);
        assert_eq!(parse_offset("0x180000005"), Some(len));
        assert_eq!(parse_offset("6442450949"), Some(len));
        assert_eq!(parse_offset("ffffffffffffffffh"), Some(u64::MAX));
        assert_eq!(parse_offset("10000000000000000h"), None);
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
        let l = Layout::fit(144, 64, doc.len() as u64);
        assert_eq!((l.bytes_per_row(), l.width()), (32, 144));
        let full = "00000000  41 42 43 44 45 46 47 48  49 4a 4b 4c 4d 4e 4f 50  51 52 53 54 55 56 57 58  \
                    59 5a 30 31 32 33 34 35  |ABCDEFGHIJKLMNOPQRSTUVWXYZ012345|";
        let short = format!("00000020  36 37 38 39{}|6789|", " ".repeat(110 - 21));
        assert_eq!(dump(&doc, &l), format!("{full}\n{short}\n00000024\n"));
        let l = Layout::fit(77, 64, doc.len() as u64);
        assert_eq!((l.bytes_per_row(), l.width()), (8, 45));
        assert_eq!(row(&doc, &l, 8), "00000008  49 4a 4b 4c 4d 4e 4f 50  |IJKLMNOP|");
        let l = Layout::fit(43, 64, doc.len() as u64);
        assert_eq!((l.bytes_per_row(), l.width()), (4, 29));
        assert_eq!(row(&doc, &l, 4), "00000004  45 46 47 48  |EFGH|");
        assert_eq!(row(&doc, &l, 36), "00000024");
        let l = Layout::fit(28, 64, doc.len() as u64);
        assert_eq!((l.bytes_per_row(), l.width()), (2, 21));
        assert_eq!(row(&doc, &l, 2), "00000002  43 44  |CD|");
        let l = Layout::fit(10, 64, doc.len() as u64);
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
            let l = Layout::fit(1000, max, doc.len() as u64);
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
        assert_eq!(doc.version(), copy.version());
        assert_eq!((doc.len(), doc.is_empty()), (3, false));
        assert!(HexDoc::new(Vec::new()).is_empty());
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

    #[test]
    fn check_compares_the_version_and_opens_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        let probe = Source::resident(b"\0ab".to_vec());
        let known = Some(probe.version());
        assert!(matches!(check(&path, &probe, known).unwrap(), OnDisk::Missing));
        assert!(read(&path).unwrap().is_none());
        std::fs::write(&path, b"\0ab").unwrap();
        assert!(matches!(check(&path, &probe, known).unwrap(), OnDisk::Unchanged));
        assert_eq!(Some(read(&path).unwrap().unwrap().version()), known);
        assert!(matches!(check(&path, &probe, None).unwrap(), OnDisk::Changed(_)));
        std::fs::write(&path, b"\0abc").unwrap();
        match check(&path, &probe, known).unwrap() {
            OnDisk::Changed(doc) => assert_eq!(doc.bytes(), b"\0abc"),
            other => panic!("{other:?}"),
        }
        // Grown past the resident limit (a sparse file, so nothing is written): opened paged,
        // not read.
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_len(RESIDENT_MAX + 1).unwrap();
        match check(&path, &probe, known).unwrap() {
            OnDisk::Changed(doc) => assert!(doc.resident_bytes().is_none() && doc.len() as u64 == RESIDENT_MAX + 1),
            other => panic!("{other:?}"),
        }
        let paged = read(&path).unwrap().unwrap();
        assert!(matches!(
            check(&path, paged.source(), Some(paged.version())).unwrap(),
            OnDisk::Unchanged
        ));
    }

    #[test]
    fn is_big_binary_needs_both_size_and_binary_content() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, len: u64, head: &[u8]| {
            let p = dir.path().join(name);
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(head).unwrap();
            f.set_len(len).unwrap();
            p
        };
        // Sparse zero tails: a "big text file" is spaces, so it is built from real text bytes.
        let text = dir.path().join("big.txt");
        std::fs::write(&text, vec![b'a'; RESIDENT_MAX as usize + 1]).unwrap();
        assert!(!is_big_binary(&text).unwrap(), "big text stays text");
        assert!(is_big_binary(&mk("big.bin", RESIDENT_MAX + 1, b"\0\x01\x02")).unwrap());
        assert!(
            !is_big_binary(&mk("small.bin", 1000, b"\0\x01\x02")).unwrap(),
            "small: resident path"
        );
        assert!(
            !is_big_binary(&dir.path().join("none")).unwrap(),
            "missing: left to the caller"
        );
    }

    #[test]
    fn size_text_units() {
        assert_eq!(size_text(0), "0 bytes");
        assert_eq!(size_text((1 << 20) - 1), "1048575 bytes");
        assert_eq!(size_text(1 << 20), "1.0 MB (1048576 bytes)");
        assert_eq!(size_text(1_288_490_188), "1.2 GB (1288490188 bytes)");
        assert_eq!(size_text(6 << 30), "6.0 GB (6442450944 bytes)");
        assert_eq!(size_text(3 << 40), "3.0 TB (3298534883328 bytes)");
    }

    #[test]
    fn window_has_placeholders_until_the_pages_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let mut data = vec![0u8; RESIDENT_MAX as usize + 1000];
        data[PAGE as usize + 5] = 0x41;
        std::fs::write(&path, &data).unwrap();
        let doc = read(&path).unwrap().unwrap();
        assert!(doc.resident_bytes().is_none());
        let layout = Layout::fit(row_width(16, 8), 16, doc.len() as u64);
        let at = PAGE;
        let pending = row(&doc, &layout, at);
        assert!(
            pending.contains("\u{b7}\u{b7} ")
                && pending.ends_with(
                    "|\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}|"
                ),
            "{pending}"
        );
        // The visible page first, then the ones around it, in jobs of a few.
        let jobs = doc.plan_reads();
        assert_eq!(jobs[0][0], 1);
        let all: Vec<u64> = jobs.concat();
        assert!(all.contains(&0), "the page above is prefetched: {all:?}");
        assert!(doc.plan_reads().is_empty(), "in flight: not asked for twice");
        let read = read_pages(doc.source(), all.iter().copied());
        assert!(doc.finish_reads(doc.version(), read));
        assert!(row(&doc, &layout, at).contains("00 00"));
        assert_eq!(doc.byte(PAGE + 5), Some(0x41));
        assert!(doc.cached_bytes() > 0);
        assert!(doc.plan_reads().is_empty(), "loaded: nothing to ask for");
        // Another version's pages are dropped.
        let other = Version::Meta {
            len: 1,
            mtime: 0,
            mtime_nsec: 0,
            ino: 0,
            dev: 0,
        };
        assert!(!doc.finish_reads(other, Vec::new()));
        // A failed page draws unreadable and isn't asked for again until `retry_failed`.
        let far = doc.len() as u64 - 3;
        let _ = doc.window(far..far + 3);
        let jobs = doc.plan_reads();
        let last = (doc.len() as u64 - 1) / PAGE;
        assert!(jobs.concat().contains(&last));
        let err = std::io::Error::other("gone");
        assert!(doc.finish_reads(doc.version(), vec![(last, Err(err))]));
        assert!(matches!(doc.window(far..far + 3).cells[0], Cell::Unreadable));
        assert!(doc.plan_reads().iter().all(|j| !j.contains(&last)));
        assert!(doc.retry_failed());
        let _ = doc.window(far..far + 3);
        assert!(doc.plan_reads().concat().contains(&last));
    }

    #[test]
    fn reads_are_neither_stuck_nor_repeated_every_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        std::fs::write(&path, vec![7u8; RESIDENT_MAX as usize + 1000]).unwrap();
        let doc = read(&path).unwrap().unwrap();
        let wanted = |doc: &HexDoc| -> Vec<u64> { doc.plan_reads().concat() };
        // Many frames over the same rows: the pages are asked for once, not once per frame.
        let _ = doc.window(0..256);
        assert!(wanted(&doc).contains(&0));
        for _ in 0..5 {
            let _ = doc.window(0..256);
            assert!(wanted(&doc).is_empty(), "in flight");
        }
        // A failed read frees its in-flight slot and is not asked for every frame either.
        let err = std::io::Error::other("gone");
        assert!(doc.finish_reads(doc.version(), vec![(0, Err(err))]));
        for _ in 0..5 {
            let _ = doc.window(0..256);
            assert!(!wanted(&doc).contains(&0), "failed: waits for a disk check");
        }
        // A disk check that finds the file as it was retries it, once.
        assert!(doc.retry_failed());
        let _ = doc.window(0..256);
        assert_eq!(wanted(&doc), vec![0], "the failed page again, nothing else");
        let _ = doc.window(0..256);
        assert!(wanted(&doc).is_empty());
        // A changed version is a new doc with its own state: its pages are asked for afresh, and
        // the old doc's reads (still in flight) are dropped when they arrive.
        let newer = read(&path).unwrap().unwrap();
        let _ = newer.window(0..256);
        assert!(
            wanted(&newer).contains(&0),
            "not blocked by the old doc's reads in flight"
        );
        let other = Version::Meta {
            len: 1,
            mtime: 0,
            mtime_nsec: 0,
            ino: 0,
            dev: 0,
        };
        let stale = read_pages(doc.source(), [0]);
        assert!(!doc.finish_reads(other, stale), "a read for another version is dropped");
        assert_eq!(doc.cached_bytes(), 0);
    }

    #[test]
    fn set_doc_clamps_the_cursor_selection_and_top() {
        let mut tab = HexTab::new("a.bin".into(), HexDoc::new(vec![0u8; 1000]));
        tab.fit(row_width(16, 8), 10, 16);
        tab.anchor = Some(900);
        tab.move_to(990, true);
        tab.fit(row_width(16, 8), 10, 16);
        assert!(tab.top > 0);
        tab.set_doc(HexDoc::new(vec![1u8; 400]));
        assert_eq!((tab.cursor, tab.anchor), (399, Some(399)));
        tab.fit(row_width(16, 8), 10, 16);
        assert_eq!(tab.top_row(), tab.rows() - 10, "the last page");
        assert!(tab.top_row() <= tab.layout.row_of(tab.cursor as u64));
        tab.deleted = true;
        tab.set_doc(HexDoc::new(Vec::new()));
        assert_eq!((tab.cursor, tab.anchor, tab.top, tab.deleted), (0, None, 0, false));
        // Grown: the cursor stays where it was.
        tab.cursor = 0;
        tab.set_doc(HexDoc::new(vec![0u8; 5000]));
        assert_eq!(tab.cursor, 0);
    }
}
