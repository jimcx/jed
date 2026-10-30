//! LSP semantic tokens as the editor keeps them: `Semantic`, one buffer's tokens and the regions a
//! server marked inactive (clangd's compiled-out code), byte ranges in its current text that
//! `Buffer::apply` maps through every change set (as `diagnostics::Marks`). The requests and the
//! app side are `app::semantic`; drawing is not here.
//!
//! - `Legend` turns a server's legend (types and modifiers by index, names may repeat) into `Kind`
//!   and a modifier bit set; `decode` reads the relative `data` array into sorted `Token`s with
//!   one `pos::LspReader` pass (UTF-8 or UTF-16 columns); `apply_delta` applies a
//!   `semanticTokens/full/delta` result's edits to the previous raw array.
//! - Tokens never overlap (the client says it doesn't support that), so they are sorted by start
//!   and by end alike and `overlapping` is two binary searches.

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{ByteIdx, LspPos, LspReader, PosEncoding};

/// The standard token types (LSP 3.17); a name that isn't one is `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Namespace,
    Type,
    Class,
    Enum,
    Interface,
    Struct,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    EnumMember,
    Event,
    Function,
    Method,
    Macro,
    Keyword,
    Modifier,
    Comment,
    String,
    Number,
    Regexp,
    Operator,
    Decorator,
    Unknown,
}

impl Kind {
    /// The standard names, in `Kind` order (`Unknown` has none).
    pub const NAMES: [&'static str; 23] = [
        "namespace",
        "type",
        "class",
        "enum",
        "interface",
        "struct",
        "typeParameter",
        "parameter",
        "variable",
        "property",
        "enumMember",
        "event",
        "function",
        "method",
        "macro",
        "keyword",
        "modifier",
        "comment",
        "string",
        "number",
        "regexp",
        "operator",
        "decorator",
    ];

    const ALL: [Kind; 23] = [
        Kind::Namespace,
        Kind::Type,
        Kind::Class,
        Kind::Enum,
        Kind::Interface,
        Kind::Struct,
        Kind::TypeParameter,
        Kind::Parameter,
        Kind::Variable,
        Kind::Property,
        Kind::EnumMember,
        Kind::Event,
        Kind::Function,
        Kind::Method,
        Kind::Macro,
        Kind::Keyword,
        Kind::Modifier,
        Kind::Comment,
        Kind::String,
        Kind::Number,
        Kind::Regexp,
        Kind::Operator,
        Kind::Decorator,
    ];

    pub fn from_name(name: &str) -> Kind {
        Self::NAMES
            .iter()
            .position(|n| *n == name)
            .map_or(Kind::Unknown, |i| Self::ALL[i])
    }

    /// The token type's name as servers (and themes) say it; `Unknown` has none.
    #[allow(dead_code)] // the theme keys arrive in semantic_tokens-b
    pub fn name(self) -> &'static str {
        Self::NAMES.get(self as usize).copied().unwrap_or("")
    }
}

/// Modifier bits: the standard modifiers plus the ones clangd sends.
#[allow(dead_code)] // the named bits are for drawing (semantic_tokens-b)
pub mod mods {
    pub const NAMES: [&str; 18] = [
        "declaration",
        "definition",
        "readonly",
        "static",
        "deprecated",
        "abstract",
        "async",
        "modification",
        "documentation",
        "defaultLibrary",
        "constructorOrDestructor",
        "dependentName",
        "functionScope",
        "classScope",
        "fileScope",
        "globalScope",
        "virtual",
        "usedAsMutableReference",
    ];

    /// The bit of the modifier called `name` (0: not one jed knows).
    pub fn bit(name: &str) -> u32 {
        NAMES.iter().position(|n| *n == name).map_or(0, |i| 1 << i)
    }

    pub const DECLARATION: u32 = 1;
    pub const DEFINITION: u32 = 1 << 1;
    pub const READONLY: u32 = 1 << 2;
    pub const STATIC: u32 = 1 << 3;
    pub const DEPRECATED: u32 = 1 << 4;
    pub const DEFAULT_LIBRARY: u32 = 1 << 9;
    pub const CONSTRUCTOR_OR_DESTRUCTOR: u32 = 1 << 10;
}

/// A server's legend, resolved by name: `kinds[i]` is the `Kind` of legend type `i`, `mods[i]` the
/// bit of legend modifier `i`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Legend {
    kinds: Vec<Kind>,
    mods: Vec<u32>,
}

impl Legend {
    pub fn new<'a>(types: impl IntoIterator<Item = &'a str>, modifiers: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            kinds: types.into_iter().map(Kind::from_name).collect(),
            mods: modifiers.into_iter().map(mods::bit).collect(),
        }
    }

    pub fn from_lsp(legend: &lsp_types::SemanticTokensLegend) -> Self {
        Self::new(
            legend.token_types.iter().map(|t| t.as_str()),
            legend.token_modifiers.iter().map(|t| t.as_str()),
        )
    }

    fn kind(&self, index: u32) -> Kind {
        self.kinds.get(index as usize).copied().unwrap_or(Kind::Unknown)
    }

    fn mods(&self, set: u32) -> u32 {
        let mut out = 0;
        let mut rest = set;
        while rest != 0 {
            let i = rest.trailing_zeros() as usize;
            out |= self.mods.get(i).copied().unwrap_or(0);
            rest &= rest - 1;
        }
        out
    }
}

/// One highlighted run: a byte range inside a line, its type and its modifier bits (`mods`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: ByteIdx,
    pub end: ByteIdx,
    pub kind: Kind,
    pub mods: u32,
}

/// Code a server marked inactive (clangd's `inactiveRegions`): a byte range, whole lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: ByteIdx,
    pub end: ByteIdx,
}

/// Reads a relative `data` array (5 numbers per token: line delta, start delta, length, type,
/// modifiers) in `text`, which must be the version the server saw. Sorted, no empty tokens, and
/// a token that would cross its line's end is cut there. Stops at an incomplete tuple.
pub fn decode(data: &[u32], text: &Rope, legend: &Legend, encoding: PosEncoding) -> Vec<Token> {
    let mut reader = LspReader::new(text, encoding);
    let mut out = Vec::with_capacity(data.len() / 5);
    let (mut line, mut col) = (0u32, 0u32);
    for t in data.as_chunks::<5>().0 {
        let (dl, ds, len, ty, md) = (t[0], t[1], t[2], t[3], t[4]);
        line = line.saturating_add(dl);
        col = if dl == 0 { col.saturating_add(ds) } else { ds };
        let start = reader.byte(LspPos { line, character: col });
        let end = reader.byte(LspPos {
            line,
            character: col.saturating_add(len),
        });
        if end > start {
            out.push(Token {
                start,
                end,
                kind: legend.kind(ty),
                mods: legend.mods(md),
            });
        }
    }
    normalize(&mut out);
    out
}

/// Sorts a hostile server's overlapping tokens into non-overlapping ones (a later token cuts the
/// earlier): only when it isn't already in order.
fn normalize(tokens: &mut Vec<Token>) {
    if tokens.windows(2).all(|w| w[0].end <= w[1].start) {
        return;
    }
    tokens.sort_by_key(|t| (t.start, t.end));
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for t in tokens.drain(..) {
        if let Some(last) = out.last_mut()
            && last.end > t.start
        {
            last.end = t.start;
            if last.end <= last.start {
                out.pop();
            }
        }
        out.push(t);
    }
    *tokens = out;
}

/// One edit of a `full/delta` result: replace `delete` numbers at `start` of the previous array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEdit {
    pub start: usize,
    pub delete: usize,
    pub data: Vec<u32>,
}

/// Applies a delta's edits (all against the previous array, so they go last first). False, with
/// `data` untouched, when an edit reaches past the array or edits overlap.
pub fn apply_delta(data: &mut Vec<u32>, edits: &[DeltaEdit]) -> bool {
    let mut order: Vec<&DeltaEdit> = edits.iter().collect();
    order.sort_by_key(|e| e.start);
    let mut limit = data.len();
    for e in order.iter().rev() {
        match e.start.checked_add(e.delete) {
            Some(end) if end <= limit => limit = e.start,
            _ => return false,
        }
    }
    for e in order.iter().rev() {
        data.splice(e.start..e.start + e.delete, e.data.iter().copied());
    }
    true
}

/// A buffer's semantic tokens and inactive regions.
#[derive(Debug, Clone, Default)]
pub struct Semantic {
    tokens: Vec<Token>,
    inactive: Vec<Region>,
}

impl Semantic {
    /// Replaces the tokens (sorted, not overlapping: `decode`'s).
    pub fn replace(&mut self, tokens: Vec<Token>) {
        self.tokens = tokens;
    }

    pub fn clear(&mut self) {
        self.tokens.clear();
        self.inactive.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.inactive.is_empty()
    }

    /// Follows a change set the buffer applied. Mapping keeps the order; tokens and regions the
    /// edit emptied are dropped.
    pub fn map(&mut self, changes: &ChangeSet) {
        if changes.is_empty() || self.is_empty() {
            return;
        }
        map_tokens(&mut self.tokens, changes);
        let m = changes.mapper();
        self.inactive.retain_mut(|r| {
            r.start = m.map(r.start, Assoc::After);
            r.end = m.map(r.end, Assoc::Before);
            r.end > r.start
        });
    }
}

/// What drawing reads (semantic_tokens-b); tests use it now.
#[allow(dead_code)]
impl Semantic {
    /// Replaces the inactive regions (any order; overlapping ones are merged).
    pub fn replace_inactive(&mut self, mut regions: Vec<Region>) {
        regions.sort_by_key(|r| (r.start, r.end));
        let mut out: Vec<Region> = Vec::with_capacity(regions.len());
        for r in regions {
            match out.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ if r.end > r.start => out.push(r),
                _ => {}
            }
        }
        self.inactive = out;
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn inactive(&self) -> &[Region] {
        &self.inactive
    }

    /// The tokens that touch `from..to` (a range of the text, e.g. the viewport), as a slice.
    pub fn overlapping(&self, from: ByteIdx, to: ByteIdx) -> &[Token] {
        let first = self.tokens.partition_point(|t| t.end <= from);
        let last = self.tokens.partition_point(|t| t.start < to);
        &self.tokens[first..last.max(first)]
    }

    /// The inactive regions that touch `from..to`.
    pub fn overlapping_inactive(&self, from: ByteIdx, to: ByteIdx) -> &[Region] {
        let first = self.inactive.partition_point(|r| r.end <= from);
        let last = self.inactive.partition_point(|r| r.start < to);
        &self.inactive[first..last.max(first)]
    }
}

/// Maps `tokens` (sorted) through `changes`, dropping the ones the edit emptied. One pass.
pub fn map_tokens(tokens: &mut Vec<Token>, changes: &ChangeSet) {
    if changes.is_empty() {
        return;
    }
    let m = changes.mapper();
    tokens.retain_mut(|t| {
        t.start = m.map(t.start, Assoc::After);
        t.end = m.map(t.end, Assoc::Before);
        t.end > t.start
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend() -> Legend {
        // Repeated names map to the same kind; unknown ones to `Unknown`.
        Legend::new(
            ["variable", "class", "function", "variable", "weird"],
            ["declaration", "constructorOrDestructor", "nope"],
        )
    }

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    fn tok(start: usize, end: usize, kind: Kind, mods: u32) -> Token {
        Token {
            start: ByteIdx(start),
            end: ByteIdx(end),
            kind,
            mods,
        }
    }

    fn set(changes: &[(usize, usize, &str)]) -> ChangeSet {
        let mut s = ChangeSet::default();
        for (a, b, t) in changes {
            s.push(ByteIdx(*a), ByteIdx(*b), t);
        }
        s
    }

    #[test]
    fn decode_relative_multi_line() {
        let text = rope("int foo;\n  bar();\n");
        // foo (line 0, col 4, len 3, variable + declaration), bar (line 1, col 2, len 3, function)
        let data = [0, 4, 3, 0, 1, 1, 2, 3, 2, 0];
        let t = decode(&data, &text, &legend(), PosEncoding::Utf8);
        assert_eq!(
            t,
            vec![
                tok(4, 7, Kind::Variable, mods::DECLARATION),
                tok(11, 14, Kind::Function, 0)
            ]
        );
    }

    #[test]
    fn decode_same_line_deltas_and_legend_names() {
        let text = rope("a b c\n");
        // index 3 repeats "variable"; index 4 is unknown; modifier bit 2 is unknown (dropped)
        let data = [0, 0, 1, 3, 0, 0, 2, 1, 4, 0b110, 0, 2, 1, 1, 0b10];
        let t = decode(&data, &text, &legend(), PosEncoding::Utf8);
        assert_eq!(
            t,
            vec![
                tok(0, 1, Kind::Variable, 0),
                tok(2, 3, Kind::Unknown, mods::CONSTRUCTOR_OR_DESTRUCTOR),
                tok(4, 5, Kind::Class, mods::CONSTRUCTOR_OR_DESTRUCTOR),
            ]
        );
    }

    #[test]
    fn decode_utf8_and_utf16_columns() {
        // "é" is 2 bytes / 1 unit, "😀" 4 bytes / 2 units.
        let text = rope("é😀x y\n");
        let l = legend();
        // x: UTF-8 col 6 len 1; UTF-16 col 3 len 1
        assert_eq!(
            decode(&[0, 6, 1, 0, 0], &text, &l, PosEncoding::Utf8),
            vec![tok(6, 7, Kind::Variable, 0)]
        );
        assert_eq!(
            decode(&[0, 3, 1, 0, 0], &text, &l, PosEncoding::Utf16),
            vec![tok(6, 7, Kind::Variable, 0)]
        );
        // a token over the emoji: UTF-16 length 2 is 4 bytes
        assert_eq!(
            decode(&[0, 1, 2, 0, 0], &text, &l, PosEncoding::Utf16),
            vec![tok(2, 6, Kind::Variable, 0)]
        );
    }

    #[test]
    fn decode_clamps_and_skips_junk() {
        let text = rope("ab\ncd");
        let l = legend();
        // past the line's end: cut; a line past the text and a trailing partial tuple: ignored
        let t = decode(&[0, 1, 99, 0, 0, 5, 0, 1, 0, 0, 1, 2], &text, &l, PosEncoding::Utf8);
        assert_eq!(t, vec![tok(1, 2, Kind::Variable, 0)]);
    }

    #[test]
    fn delta_edits_apply_against_the_previous_array() {
        let mut d: Vec<u32> = (0..15).collect();
        let edits = [
            DeltaEdit {
                start: 5,
                delete: 5,
                data: vec![100; 5],
            },
            DeltaEdit {
                start: 0,
                delete: 0,
                data: vec![7, 7, 7, 7, 7],
            },
            DeltaEdit {
                start: 10,
                delete: 5,
                data: vec![],
            },
        ];
        assert!(apply_delta(&mut d, &edits));
        let mut want = vec![7; 5];
        want.extend(0..5);
        want.extend([100; 5]);
        assert_eq!(d, want);

        let mut d = vec![1, 2, 3];
        assert!(!apply_delta(
            &mut d,
            &[DeltaEdit {
                start: 2,
                delete: 5,
                data: vec![]
            }]
        ));
        assert_eq!(d, vec![1, 2, 3]);

        // Overlapping edits (2..5 and 4..6) are refused, the array untouched.
        let mut d: Vec<u32> = (0..10).collect();
        let overlapping = [
            DeltaEdit {
                start: 4,
                delete: 2,
                data: vec![9],
            },
            DeltaEdit {
                start: 2,
                delete: 3,
                data: vec![8],
            },
        ];
        assert!(!apply_delta(&mut d, &overlapping));
        assert_eq!(d, (0..10).collect::<Vec<u32>>());
        // Touching ones (2..4 then 4..6) are fine.
        let touching = [
            DeltaEdit {
                start: 4,
                delete: 2,
                data: vec![9],
            },
            DeltaEdit {
                start: 2,
                delete: 2,
                data: vec![8],
            },
        ];
        assert!(apply_delta(&mut d, &touching));
        assert_eq!(d, vec![0, 1, 8, 9, 6, 7, 8, 9]);
    }

    #[test]
    fn mapping_through_edits() {
        let mut s = Semantic::default();
        s.replace(vec![
            tok(0, 3, Kind::Variable, 0),
            tok(4, 7, Kind::Function, 0),
            tok(8, 11, Kind::Class, 0),
        ]);
        s.replace_inactive(vec![Region {
            start: ByteIdx(12),
            end: ByteIdx(20),
        }]);
        // insert "xx" at 0 (before the first token), delete 4..7 (the second), insert in the third
        s.map(&set(&[(0, 0, "xx"), (4, 7, ""), (9, 9, "ZZ")]));
        assert_eq!(s.tokens(), &[tok(2, 5, Kind::Variable, 0), tok(7, 12, Kind::Class, 0)]);
        assert_eq!(
            s.inactive(),
            &[Region {
                start: ByteIdx(13),
                end: ByteIdx(21)
            }]
        );
        // the rest of an emptied region goes
        s.map(&set(&[(0, 100, "")]));
        assert!(s.is_empty());
    }

    #[test]
    fn typing_at_a_token_edge_does_not_grow_it() {
        // VS Code: text typed right before or right after a token isn't part of it until the
        // server says so; a token's whole range replaced is dropped.
        let mut s = Semantic::default();
        s.replace(vec![tok(0, 3, Kind::Variable, 0), tok(4, 7, Kind::Function, 0)]);
        s.map(&set(&[(3, 3, "ZZ"), (4, 4, "Y")]));
        assert_eq!(
            s.tokens(),
            &[tok(0, 3, Kind::Variable, 0), tok(7, 10, Kind::Function, 0)]
        );
        s.map(&set(&[(7, 10, "abc")]));
        assert_eq!(s.tokens(), &[tok(0, 3, Kind::Variable, 0)]);
        let mut toks = vec![tok(0, 3, Kind::Variable, 0)];
        map_tokens(&mut toks, &set(&[(3, 3, "x")]));
        assert_eq!(toks, vec![tok(0, 3, Kind::Variable, 0)]);
    }

    #[test]
    fn overlapping_finds_the_touching_tokens() {
        let mut s = Semantic::default();
        s.replace(vec![
            tok(0, 2, Kind::Variable, 0),
            tok(4, 6, Kind::Variable, 0),
            tok(8, 10, Kind::Variable, 0),
        ]);
        let o = |a, b| s.overlapping(ByteIdx(a), ByteIdx(b)).len();
        assert_eq!(o(0, 100), 3);
        assert_eq!(o(2, 4), 0);
        assert_eq!(o(1, 5), 2);
        assert_eq!(o(5, 9), 2);
        assert_eq!(o(10, 12), 0);
    }

    #[test]
    fn overlapping_tokens_from_a_bad_server_are_cut() {
        let text = rope("abcdef\n");
        // second token starts inside the first
        let t = decode(&[0, 0, 4, 0, 0, 0, 2, 2, 1, 0], &text, &legend(), PosEncoding::Utf8);
        assert_eq!(t, vec![tok(0, 2, Kind::Variable, 0), tok(2, 4, Kind::Class, 0)]);
    }

    #[test]
    fn sixty_thousand_tokens_decode_map_and_query_in_budget() {
        // 60k lines "ab cd\n", two tokens each.
        let text = rope(&"ab cd\n".repeat(60_000));
        let mut data = Vec::new();
        for _ in 0..60_000 {
            data.extend([1, 0, 2, 0, 0, 0, 3, 2, 1, 0]);
        }
        data[0] = 0;
        let t0 = std::time::Instant::now();
        let tokens = decode(&data, &text, &legend(), PosEncoding::Utf16);
        let decoded = t0.elapsed();
        assert_eq!(tokens.len(), 120_000);
        let mut s = Semantic::default();
        s.replace(tokens);
        let t1 = std::time::Instant::now();
        s.map(&set(&[(0, 0, "x"), (3_000, 3_001, ""), (300_000, 300_000, "yy")]));
        let mapped = t1.elapsed();
        let t2 = std::time::Instant::now();
        let mut n = 0;
        for i in 0..1000 {
            n += s.overlapping(ByteIdx(i * 300), ByteIdx(i * 300 + 600)).len();
        }
        let queried = t2.elapsed();
        assert!(n > 0);
        let release = !cfg!(debug_assertions);
        let (d, m, q) = if release { (1, 1, 1) } else { (20, 20, 20) };
        assert!(decoded.as_millis() < 100 * d, "decode {decoded:?}");
        assert!(mapped.as_millis() < 50 * m, "map {mapped:?}");
        assert!(queried.as_millis() < 10 * q, "query {queried:?}");
    }
}
