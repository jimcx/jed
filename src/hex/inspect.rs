//! The data inspector's model: the bytes at an offset as typed values, one `Row` per type.
//! Pure functions over up to 8 bytes (`rows`); the panel (`ui::hex`) and Copy Value ask for
//! them, so a row is named by `RowKind`, never by its label. `ByteOrder` is shared with the
//! digit grouping (`hex_group_bytes`).

use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::{Cell, HexTab};

/// The byte order multi-byte values are read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ByteOrder {
    #[default]
    Little,
    Big,
}

impl ByteOrder {
    /// `LE` / `BE`, as the panel's header shows it.
    pub fn label(self) -> &'static str {
        match self {
            ByteOrder::Little => "LE",
            ByteOrder::Big => "BE",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            ByteOrder::Little => ByteOrder::Big,
            ByteOrder::Big => ByteOrder::Little,
        }
    }
}

/// Which value a row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Binary,
    I8,
    U8,
    Char,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Utf8,
    Utf16,
}

impl RowKind {
    /// A whole-number row (Copy Value's pick among a selection's equally wide rows).
    pub fn is_integer(self) -> bool {
        match self {
            RowKind::I8
            | RowKind::U8
            | RowKind::I16
            | RowKind::U16
            | RowKind::I32
            | RowKind::U32
            | RowKind::I64
            | RowKind::U64 => true,
            RowKind::Binary | RowKind::Char | RowKind::F32 | RowKind::F64 | RowKind::Utf8 | RowKind::Utf16 => false,
        }
    }
}

/// The shown text of a value that can't be read (past the end of the file).
pub const BLANK: &str = "—";
/// A value whose bytes aren't loaded yet.
pub const PENDING: &str = "…";

/// One inspector row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub label: &'static str,
    /// The bytes the value covers.
    pub width: usize,
    pub value: String,
}

/// The rows in the order the panel lists them: kind, label, width in bytes (UTF-8 and UTF-16
/// vary with the value).
const KINDS: [(RowKind, &str, usize); 14] = [
    (RowKind::Binary, "binary", 1),
    (RowKind::I8, "i8", 1),
    (RowKind::U8, "u8", 1),
    (RowKind::Char, "char", 1),
    (RowKind::I16, "i16 short", 2),
    (RowKind::U16, "u16", 2),
    (RowKind::I32, "i32 int", 4),
    (RowKind::U32, "u32", 4),
    (RowKind::I64, "i64 long", 8),
    (RowKind::U64, "u64", 8),
    (RowKind::F32, "f32 float", 4),
    (RowKind::F64, "f64 double", 8),
    (RowKind::Utf8, "UTF-8", 1),
    (RowKind::Utf16, "UTF-16", 2),
];

/// The rows for `bytes` (up to 8 from the inspected offset, fewer at the end of the file).
pub fn rows(bytes: &[u8], order: ByteOrder) -> Vec<Row> {
    KINDS
        .iter()
        .map(|&(kind, label, fixed)| {
            let (value, width) = value(kind, bytes, order).unwrap_or_else(|| (BLANK.into(), fixed));
            Row {
                kind,
                label,
                width,
                value,
            }
        })
        .collect()
}

/// The text and byte width of one row, or `None` when `bytes` is too short.
fn value(kind: RowKind, b: &[u8], order: ByteOrder) -> Option<(String, usize)> {
    /// The first `N` bytes, reversed for big-endian so `from_le_bytes` reads them.
    fn arr<const N: usize>(b: &[u8], order: ByteOrder) -> Option<[u8; N]> {
        let mut a: [u8; N] = b.get(..N)?.try_into().ok()?;
        if order == ByteOrder::Big {
            a.reverse();
        }
        Some(a)
    }
    let text = match kind {
        RowKind::Binary => (format!("{:08b}", b.first()?), 1),
        RowKind::I8 => ((*b.first()? as i8).to_string(), 1),
        RowKind::U8 => (b.first()?.to_string(), 1),
        RowKind::Char => (char_text(*b.first()?), 1),
        RowKind::I16 => (i16::from_le_bytes(arr(b, order)?).to_string(), 2),
        RowKind::U16 => (u16::from_le_bytes(arr(b, order)?).to_string(), 2),
        RowKind::I32 => (i32::from_le_bytes(arr(b, order)?).to_string(), 4),
        RowKind::U32 => (u32::from_le_bytes(arr(b, order)?).to_string(), 4),
        RowKind::I64 => (i64::from_le_bytes(arr(b, order)?).to_string(), 8),
        RowKind::U64 => (u64::from_le_bytes(arr(b, order)?).to_string(), 8),
        RowKind::F32 => (f32::from_le_bytes(arr(b, order)?).to_string(), 4),
        RowKind::F64 => (f64::from_le_bytes(arr(b, order)?).to_string(), 8),
        RowKind::Utf8 => return utf8(b),
        RowKind::Utf16 => return utf16(b, order),
    };
    Some(text)
}

/// A byte as ASCII, or an escape.
fn char_text(b: u8) -> String {
    match b {
        0 => "\\0".into(),
        b'\t' => "\\t".into(),
        b'\n' => "\\n".into(),
        b'\r' => "\\r".into(),
        0x20..=0x7e => (b as char).to_string(),
        _ => format!("\\x{b:02x}"),
    }
}

/// `U+00E9 é` (just the number for a control character).
fn code_point(c: char) -> String {
    let shown = if c.is_control() { String::new() } else { format!(" {c}") };
    format!("U+{:04X}{shown}", c as u32)
}

/// The code point starting at `b[0]`; `invalid` for a bad sequence; `None` for no bytes, or a
/// valid start cut off by the end of the file.
fn utf8(b: &[u8]) -> Option<(String, usize)> {
    let first = *b.first()?;
    let n = match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Some(("invalid".into(), 1)),
    };
    match b.get(..n) {
        Some(s) => match std::str::from_utf8(s) {
            Ok(s) => Some((code_point(s.chars().next()?), n)),
            Err(_) => Some(("invalid".into(), 1)),
        },
        None if b[1..].iter().all(|c| c & 0xc0 == 0x80) => None,
        None => Some(("invalid".into(), 1)),
    }
}

/// The code unit at `b[0..2]` in `order`, a surrogate pair combined; a lone surrogate is `invalid`.
fn utf16(b: &[u8], order: ByteOrder) -> Option<(String, usize)> {
    let unit = |at: usize| -> Option<u16> {
        let p: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(match order {
            ByteOrder::Little => u16::from_le_bytes(p),
            ByteOrder::Big => u16::from_be_bytes(p),
        })
    };
    let hi = unit(0)?;
    // A high surrogate needs the next unit; past the end of the file it's cut off, not invalid.
    let pair;
    let units: &[u16] = if (0xd800..=0xdbff).contains(&hi) {
        pair = [hi, unit(2)?];
        &pair
    } else {
        std::slice::from_ref(&hi)
    };
    Some(match char::decode_utf16(units.iter().copied()).next() {
        Some(Ok(c)) => (code_point(c), c.len_utf16() * 2),
        _ => ("invalid".into(), 2),
    })
}

/// What the data inspector shows for a tab as it is now.
#[derive(Debug, Clone)]
pub struct Inspected {
    /// Where the values are read from: the selection's start, else the cursor.
    pub offset: usize,
    pub rows: Vec<Row>,
    /// Indices into `rows` that are highlighted and whose bytes are outlined: the clicked row, else
    /// every row as wide as a 1 / 2 / 4 / 8 byte selection.
    pub marked: Vec<usize>,
}

impl Inspected {
    /// The bytes a marked row covers, outlined in both columns.
    pub fn outlines(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.marked
            .iter()
            .map(|&i| self.offset..self.offset + self.rows[i].width.max(1))
    }

    /// Copy Value's text: the first marked integer row's value (a clicked row is the only one
    /// marked, whatever its kind), when it has one.
    pub fn value(&self) -> Option<&str> {
        let i = self
            .marked
            .iter()
            .find(|&&i| self.rows[i].kind.is_integer())
            .or(self.marked.first())?;
        let r = &self.rows[*i];
        (r.value != BLANK && r.value != PENDING).then_some(r.value.as_str())
    }
}

impl HexTab {
    /// The inspector's rows for the bytes at the inspected offset, from cached bytes only
    /// (`HexDoc::window` queues a page it lacks; a value over one that isn't loaded is `…`).
    /// `None` in an empty file.
    pub fn inspect(&self, order: ByteOrder) -> Option<Inspected> {
        let len = self.len();
        if len == 0 {
            return None;
        }
        let selection = self.selection();
        let offset = selection.as_ref().map_or(self.cursor, |s| s.start).min(len - 1);
        let end = (offset + 8).min(len);
        let window = self.doc.window(offset as u64..end as u64);
        let loaded: Vec<u8> = window
            .cells
            .iter()
            .map_while(|c| match c {
                Cell::Byte(b) => Some(*b),
                _ => None,
            })
            .collect();
        let missing = loaded.len() < end - offset;
        let mut rows = rows(&loaded, order);
        if missing {
            // Blank because a page isn't loaded, unless the row would run past the end anyway.
            for r in rows.iter_mut().filter(|r| r.value == BLANK && offset + r.width <= len) {
                r.value = PENDING.into();
            }
        }
        let clicked = self
            .inspected_row
            .filter(|&(_, at)| at == offset)
            .and_then(|(k, _)| rows.iter().position(|r| r.kind == k));
        let marked = match (clicked, &selection) {
            (Some(i), _) => vec![i],
            (None, Some(s)) if matches!(s.len(), 1 | 2 | 4 | 8) => rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.width == s.len() && r.value != BLANK && r.value != PENDING)
                .map(|(i, _)| i)
                .collect(),
            _ => Vec::new(),
        };
        Some(Inspected { offset, rows, marked })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ByteOrder::{Big, Little};

    fn get(bytes: &[u8], order: ByteOrder, kind: RowKind) -> String {
        rows(bytes, order).into_iter().find(|r| r.kind == kind).unwrap().value
    }

    const SEQ: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn integers_in_both_orders() {
        assert_eq!(get(&SEQ, Little, RowKind::U16), "513");
        assert_eq!(get(&SEQ, Little, RowKind::U32), "67305985");
        assert_eq!(get(&SEQ, Little, RowKind::U64), "578437695752307201");
        assert_eq!(get(&SEQ, Big, RowKind::U16), "258");
        assert_eq!(get(&SEQ, Big, RowKind::U32), "16909060");
        assert_eq!(get(&SEQ, Big, RowKind::U64), "72623859790382856");
        assert_eq!(get(&SEQ, Little, RowKind::I64), "578437695752307201");
        assert_eq!(get(&SEQ, Big, RowKind::I32), "16909060");
    }

    #[test]
    fn negative_values_and_byte_rows() {
        let b = [0xff, 0xfe, 0x80, 0x00];
        assert_eq!(get(&b, Little, RowKind::I8), "-1");
        assert_eq!(get(&b, Little, RowKind::U8), "255");
        assert_eq!(get(&b, Little, RowKind::I16), "-257");
        assert_eq!(get(&b, Big, RowKind::I16), "-2");
        assert_eq!(get(&b, Little, RowKind::Binary), "11111111");
        assert_eq!(get(&[0x41], Little, RowKind::Binary), "01000001");
    }

    #[test]
    fn floats() {
        assert_eq!(get(&[0, 0, 0x80, 0x3f], Little, RowKind::F32), "1");
        assert_eq!(get(&[0x3f, 0x80, 0, 0], Big, RowKind::F32), "1");
        assert_eq!(get(&[0, 0, 0, 0, 0, 0, 0xf0, 0x3f], Little, RowKind::F64), "1");
        assert_eq!(get(&[0, 0, 0xc0, 0x7f], Little, RowKind::F32), "NaN");
        assert_eq!(get(&[0, 0, 0x80, 0x7f], Little, RowKind::F32), "inf");
        assert_eq!(get(&[0, 0, 0x80, 0xff], Little, RowKind::F32), "-inf");
        assert_eq!(get(&[0, 0, 0, 0, 0, 0, 0xf0, 0x7f], Little, RowKind::F64), "inf");
    }

    #[test]
    fn char_escapes() {
        assert_eq!(get(b"A", Little, RowKind::Char), "A");
        assert_eq!(get(&[0], Little, RowKind::Char), "\\0");
        assert_eq!(get(b"\n", Little, RowKind::Char), "\\n");
        assert_eq!(get(&[0x7f], Little, RowKind::Char), "\\x7f");
        assert_eq!(get(&[0xe9], Little, RowKind::Char), "\\xe9");
    }

    #[test]
    fn rows_past_the_end_are_blank() {
        let b = [1, 2, 3];
        for kind in [
            RowKind::I32,
            RowKind::U32,
            RowKind::F32,
            RowKind::I64,
            RowKind::U64,
            RowKind::F64,
        ] {
            assert_eq!(get(&b, Little, kind), BLANK, "{kind:?}");
        }
        assert_eq!(get(&b, Little, RowKind::U16), "513");
        assert!(rows(&[], Little).iter().all(|r| r.value == BLANK));
        assert_eq!(get(&[1], Little, RowKind::U16), BLANK);
    }

    #[test]
    fn blanks_at_each_of_the_last_eight_offsets() {
        // `left` bytes remain: a row of width w is blank exactly when w > left.
        for left in 0..=8usize {
            let b = &SEQ[..left];
            for (kind, w) in [
                (RowKind::Binary, 1),
                (RowKind::I8, 1),
                (RowKind::U8, 1),
                (RowKind::Char, 1),
                (RowKind::I16, 2),
                (RowKind::U16, 2),
                (RowKind::I32, 4),
                (RowKind::U32, 4),
                (RowKind::F32, 4),
                (RowKind::I64, 8),
                (RowKind::U64, 8),
                (RowKind::F64, 8),
            ] {
                for order in [Little, Big] {
                    assert_eq!(
                        get(b, order, kind) == BLANK,
                        w > left,
                        "{kind:?} with {left} left, {order:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn utf8_code_points() {
        assert_eq!(get("é".as_bytes(), Little, RowKind::Utf8), "U+00E9 é");
        assert_eq!(get(b"A", Little, RowKind::Utf8), "U+0041 A");
        assert_eq!(get("€".as_bytes(), Little, RowKind::Utf8), "U+20AC €");
        assert_eq!(get("😀".as_bytes(), Little, RowKind::Utf8), "U+1F600 😀");
        assert_eq!(get(&[0], Little, RowKind::Utf8), "U+0000");
        assert_eq!(get(&[0xff], Little, RowKind::Utf8), "invalid");
        assert_eq!(get(&[0x80, 0x80], Little, RowKind::Utf8), "invalid");
        assert_eq!(get(&[0xc3, 0x41], Little, RowKind::Utf8), "invalid");
        assert_eq!(
            get(&[0xed, 0xa0, 0x80], Little, RowKind::Utf8),
            "invalid",
            "a surrogate"
        );
        // Cut by the end of the file: blank. The width is the sequence's length.
        assert_eq!(get(&[0xe2, 0x82], Little, RowKind::Utf8), BLANK);
        let r = rows("é".as_bytes(), Little);
        assert_eq!(r.iter().find(|r| r.kind == RowKind::Utf8).unwrap().width, 2);
    }

    #[test]
    fn utf16_units_and_pairs() {
        assert_eq!(get(&[0xe9, 0x00], Little, RowKind::Utf16), "U+00E9 é");
        assert_eq!(get(&[0x00, 0xe9], Big, RowKind::Utf16), "U+00E9 é");
        // U+1F600 = D83D DE00.
        assert_eq!(get(&[0x3d, 0xd8, 0x00, 0xde], Little, RowKind::Utf16), "U+1F600 😀");
        assert_eq!(get(&[0xd8, 0x3d, 0xde, 0x00], Big, RowKind::Utf16), "U+1F600 😀");
        assert_eq!(get(&[0x3d, 0xd8, 0x41, 0x00], Little, RowKind::Utf16), "invalid");
        assert_eq!(get(&[0x00, 0xde, 0x41, 0x00], Little, RowKind::Utf16), "invalid");
        assert_eq!(get(&[0x3d, 0xd8], Little, RowKind::Utf16), BLANK, "pair cut by the end");
        assert_eq!(get(&[0x41], Little, RowKind::Utf16), BLANK);
    }

    #[test]
    fn labels_use_both_names() {
        let r = rows(&SEQ, Little);
        let labels: Vec<_> = r.iter().map(|r| r.label).collect();
        assert!(labels.contains(&"i16 short") && labels.contains(&"i32 int") && labels.contains(&"i64 long"));
    }

    #[test]
    fn byte_order_toggle_and_labels() {
        assert_eq!(ByteOrder::default(), Little);
        assert_eq!(Little.toggle(), Big);
        assert_eq!(Big.label(), "BE");
    }
}
