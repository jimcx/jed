//! "Hex: Copy As…": the formats a hex tab's selection can be copied in, as pure functions of its bytes.

use super::inspect::{ByteOrder, RowKind, rows};
use super::{DIGITS, Grouping, is_printable};

/// Bytes per line of a C / Rust array that doesn't fit on one.
pub const ARRAY_PER_LINE: usize = 16;

/// A way to copy bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopyFormat {
    /// `7f454c46`
    HexPlain,
    /// `7f 45 4c 46` (Cmd+C's)
    HexSpaced,
    /// `{ 0x7f, 0x45 }`
    CArray,
    /// `[0x7f, 0x45]`
    RustArray,
    /// `b'\x7fELF'`
    Python,
    Base64,
    /// The inspector's value of a 1 / 2 / 4 / 8 byte selection.
    Value,
}

impl CopyFormat {
    /// The quick pick's order.
    pub const ALL: [CopyFormat; 7] = [
        CopyFormat::HexPlain,
        CopyFormat::HexSpaced,
        CopyFormat::CArray,
        CopyFormat::RustArray,
        CopyFormat::Python,
        CopyFormat::Base64,
        CopyFormat::Value,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CopyFormat::HexPlain => "Hex without spaces",
            CopyFormat::HexSpaced => "Hex with spaces",
            CopyFormat::CArray => "C array",
            CopyFormat::RustArray => "Rust array",
            CopyFormat::Python => "Python bytes",
            CopyFormat::Base64 => "Base64",
            CopyFormat::Value => "Value",
        }
    }

    /// A short example, dimmed after the title.
    pub fn example(self) -> &'static str {
        match self {
            CopyFormat::HexPlain => "7f454c46",
            CopyFormat::HexSpaced => "7f 45 4c 46",
            CopyFormat::CArray => "{ 0x7f, 0x45, ... }",
            CopyFormat::RustArray => "[0x7f, 0x45, ...]",
            CopyFormat::Python => "b'\\x7fELF'",
            CopyFormat::Base64 => "f0VMRg==",
            CopyFormat::Value => "the inspector's integer",
        }
    }

    /// Whether `len` selected bytes can be copied this way (a value needs 1 / 2 / 4 / 8).
    pub fn allows(self, len: usize) -> bool {
        self != CopyFormat::Value || matches!(len, 1 | 2 | 4 | 8)
    }
}

/// `bytes` in `format` (`order` reads a value). `None`: a value of a width that has none.
pub fn format_as(bytes: &[u8], format: CopyFormat, order: ByteOrder) -> Option<String> {
    Some(match format {
        CopyFormat::HexPlain => {
            let mut out = String::with_capacity(bytes.len() * 2);
            bytes.iter().for_each(|&b| push_hex(&mut out, b));
            out
        }
        CopyFormat::HexSpaced => Grouping::default().format(bytes, 0),
        CopyFormat::CArray => array(bytes, '{', '}', " "),
        CopyFormat::RustArray => array(bytes, '[', ']', ""),
        CopyFormat::Python => python(bytes),
        CopyFormat::Base64 => crate::clipboard::base64(bytes),
        CopyFormat::Value => {
            if !matches!(bytes.len(), 1 | 2 | 4 | 8) {
                return None;
            }
            let all = rows(bytes, order);
            let row = all
                .iter()
                .find(|r| r.width == bytes.len() && r.kind.is_integer())
                .or_else(|| all.iter().find(|r| r.width == bytes.len()))?;
            row.value.clone()
        }
    })
}

/// The bytes Follow Pointer reads for an inspector row: 8 for a 64-bit row, else 4.
pub fn pointer_width(kind: RowKind) -> usize {
    match kind {
        RowKind::I64 | RowKind::U64 | RowKind::F64 => 8,
        _ => 4,
    }
}

/// An unsigned pointer of 4 or 8 `bytes` in `order` (other lengths: the first 4 / all of them
/// up to 8).
pub fn pointer_value(bytes: &[u8], order: ByteOrder) -> u64 {
    let n = if bytes.len() >= 8 { 8 } else { 4.min(bytes.len()) };
    let bytes = &bytes[..n];
    match order {
        ByteOrder::Little => bytes.iter().rev().fold(0, |v, &b| v << 8 | u64::from(b)),
        ByteOrder::Big => bytes.iter().fold(0, |v, &b| v << 8 | u64::from(b)),
    }
}

/// Two lowercase hex digits of `b`, through the shared nibble table.
fn push_hex(out: &mut String, b: u8) {
    out.push(char::from(DIGITS[usize::from(b >> 4)]));
    out.push(char::from(DIGITS[usize::from(b & 15)]));
}

/// `0x7f, 0x45`: the bytes joined by commas.
fn push_items(out: &mut String, bytes: &[u8]) {
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str("0x");
        push_hex(out, b);
    }
}

/// `{ 0x7f, 0x45 }` (`pad` inside the brackets: a space for C, none for Rust) on one line up to
/// `ARRAY_PER_LINE` bytes; more: one line per 16, indented four spaces, each ending in a comma
/// (valid C and Rust), the brackets on their own lines.
fn array(bytes: &[u8], open: char, close: char, pad: &str) -> String {
    let mut out = String::with_capacity(bytes.len() * 6 + bytes.len() / ARRAY_PER_LINE * 6 + 8);
    out.push(open);
    if bytes.len() <= ARRAY_PER_LINE {
        if !bytes.is_empty() {
            out.push_str(pad);
            push_items(&mut out, bytes);
            out.push_str(pad);
        }
    } else {
        out.push('\n');
        for line in bytes.chunks(ARRAY_PER_LINE) {
            out.push_str("    ");
            push_items(&mut out, line);
            out.push_str(",\n");
        }
    }
    out.push(close);
    out
}

/// `b'…'`: printable ASCII as itself (`\\` and `\'` escaped), the rest `\xNN`.
fn python(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push_str("b'");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            _ if is_printable(b) => out.push(b as char),
            _ => {
                out.push_str("\\x");
                push_hex(&mut out, b);
            }
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELF: [u8; 4] = [0x7f, 0x45, 0x4c, 0x46];

    fn f(bytes: &[u8], format: CopyFormat) -> String {
        format_as(bytes, format, ByteOrder::Little).unwrap()
    }

    #[test]
    fn every_format_of_the_elf_magic() {
        assert_eq!(f(&ELF, CopyFormat::HexPlain), "7f454c46");
        assert_eq!(f(&ELF, CopyFormat::HexSpaced), "7f 45 4c 46");
        assert_eq!(f(&ELF, CopyFormat::CArray), "{ 0x7f, 0x45, 0x4c, 0x46 }");
        assert_eq!(f(&ELF, CopyFormat::RustArray), "[0x7f, 0x45, 0x4c, 0x46]");
        assert_eq!(f(&ELF, CopyFormat::Python), "b'\\x7fELF'");
        assert_eq!(f(&ELF, CopyFormat::Base64), "f0VMRg==");
        assert_eq!(f(&ELF, CopyFormat::Value), "1179403647");
        assert_eq!(
            format_as(&ELF, CopyFormat::Value, ByteOrder::Big).unwrap(),
            "2135247942"
        );
    }

    #[test]
    fn empty_and_one_byte() {
        assert_eq!(f(&[], CopyFormat::HexPlain), "");
        assert_eq!(f(&[], CopyFormat::CArray), "{}");
        assert_eq!(f(&[], CopyFormat::RustArray), "[]");
        assert_eq!(f(&[], CopyFormat::Python), "b''");
        assert_eq!(f(&[], CopyFormat::Base64), "");
        assert_eq!(f(&[0xab], CopyFormat::HexPlain), "ab");
        assert_eq!(f(&[0xab], CopyFormat::CArray), "{ 0xab }");
        assert_eq!(f(&[0xab], CopyFormat::Value), "-85");
        assert_eq!(format_as(&[], CopyFormat::Value, ByteOrder::Little), None);
        assert_eq!(format_as(&[1, 2, 3], CopyFormat::Value, ByteOrder::Little), None);
    }

    #[test]
    fn arrays_wrap_at_sixteen_bytes() {
        let bytes: Vec<u8> = (0..17).collect();
        let c = f(&bytes, CopyFormat::CArray);
        assert_eq!(c.lines().count(), 4);
        assert_eq!(c.lines().nth(1).unwrap().matches("0x").count(), 16);
        assert!(c.starts_with("{\n    0x00, 0x01,"));
        assert!(c.ends_with("    0x10,\n}"));
        assert!(f(&bytes[..16], CopyFormat::RustArray).starts_with("[0x00, "));
        assert!(f(&bytes, CopyFormat::RustArray).ends_with(",\n]"));
    }

    #[test]
    fn python_escapes_quotes_backslashes_and_unprintables() {
        assert_eq!(f(b"it's a\\b", CopyFormat::Python), "b'it\\'s a\\\\b'");
        assert_eq!(
            f(&[0x00, b'A', 0x0a, 0xff, b' ', b'~'], CopyFormat::Python),
            "b'\\x00A\\x0a\\xff ~'"
        );
    }

    #[test]
    fn base64_pads_one_two_and_three_bytes() {
        assert_eq!(f(b"f", CopyFormat::Base64), "Zg==");
        assert_eq!(f(b"fo", CopyFormat::Base64), "Zm8=");
        assert_eq!(f(b"foo", CopyFormat::Base64), "Zm9v");
    }

    #[test]
    fn pointers_decode_in_both_orders() {
        let four = [0x10, 0, 0, 0];
        assert_eq!(pointer_value(&four, ByteOrder::Little), 0x10);
        assert_eq!(pointer_value(&four, ByteOrder::Big), 0x1000_0000);
        assert_eq!(pointer_value(&[0xff; 4], ByteOrder::Little), 0xffff_ffff);
        let eight = [0x20, 0x01, 0, 0, 0, 0, 0, 0];
        assert_eq!(pointer_value(&eight, ByteOrder::Little), 0x120);
        assert_eq!(pointer_value(&eight, ByteOrder::Big), 0x2001_0000_0000_0000);
        assert_eq!((pointer_width(RowKind::U64), pointer_width(RowKind::U32)), (8, 4));
    }

    #[test]
    fn value_widths() {
        assert!(CopyFormat::Value.allows(8) && !CopyFormat::Value.allows(3) && CopyFormat::CArray.allows(3));
        assert_eq!(f(&[0xff, 0xff], CopyFormat::Value), "-1");
    }
}
