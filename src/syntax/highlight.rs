//! Syntax highlighting: each language's highlight queries (`Language::highlight_queries`) run over
//! a range of lines, giving `Span`s that name a theme key (`Highlight::key`, Dark+ colours in
//! `Theme::dark_plus`).
//!
//! `Highlights` (one per `Syntax`) caches spans for a few regions of lines: a request for any line
//! range (the editor's viewport, later the Minimap's) is served from a region that covers it, else
//! the range plus `MARGIN` lines either side is queried. Regions belong to one tree (the `version`
//! `Syntax` bumps whenever a new current tree is in place), so nothing is queried again until the
//! tree changes or the view leaves the cached lines.
//!
//! While there is no current tree (an edit's re-parse runs in the background), the last spans are
//! kept, mapped through each change set (`Highlights::edit`) so they stay on the same text; text
//! typed at a span's edge stays plain until the new tree arrives. Positions are only compared
//! with, never used to index the rope, so stale spans can't panic.

use std::cmp::Reverse;
use std::ops::Range;
use std::sync::OnceLock;

use ropey::Rope;
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

use super::Language;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};

/// Theme keys a capture can map to (`Theme::dark_plus` has each). A capture's dotted name is
/// looked up, then its prefixes (`function.method.builtin` → `function.method` → `function`),
/// each also in `ALIASES`.
pub const KEYS: &[&str] = &[
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "diff.deleted",
    "diff.header",
    "diff.hunk",
    "diff.inserted",
    "escape",
    "function",
    "invalid",
    "keyword",
    "keyword.control",
    "label",
    "markup.heading",
    "markup.link",
    "markup.list",
    "markup.quote",
    "markup.raw",
    "number",
    "operator",
    "overflow",
    "property",
    "punctuation",
    "punctuation.special",
    "punctuation.tag",
    "string",
    "tag",
    "tag.attribute",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// Capture names the grammars' own queries use for what a key in `KEYS` colours (Neovim's older
/// and newer names): capture (or prefix) → key.
pub(super) const ALIASES: &[(&str, &str)] = &[
    ("boolean", "constant.builtin"),
    ("string.special.key", "property"),
    ("string.escape", "escape"),
    ("keyword.conditional", "keyword.control"),
    ("keyword.repeat", "keyword.control"),
    ("keyword.return", "keyword.control"),
    ("conditional", "keyword.control"),
    ("repeat", "keyword.control"),
    ("include", "keyword.control"),
    ("exception", "keyword.control"),
    ("storageclass", "keyword"),
    ("namespace", "type"),
    ("method", "function"),
    ("parameter", "variable.parameter"),
    ("text.title", "markup.heading"),
    ("text.literal", "markup.raw"),
    ("text.uri", "markup.link"),
    ("text.reference", "markup.link"),
];

/// Lines queried beyond the requested ones, so scrolling a little needs no new query.
pub const MARGIN: usize = 50;
/// The most bytes highlighted from the start of the requested lines, and queried beyond either
/// end of them (instead of `MARGIN` lines when those are longer), so one query stays under ~7 ms
/// (dense C++ on a slow x86: ~0.3 ms per KB) even on a 1 MB line, which is coloured only for its
/// first `MAX_QUERY_BYTES` (VS Code also stops tokenizing long lines). A screen of code is far
/// less: 80 rows of 200 bytes is 16 KB.
const MAX_QUERY_BYTES: usize = 16 << 10;
const MARGIN_BYTES: usize = 4 << 10;
/// Cached regions per buffer (several views on one buffer, the Minimap).
const MAX_REGIONS: usize = 4;

/// A theme key from `KEYS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight(u8);

impl Highlight {
    #[cfg(test)]
    pub fn key(self) -> &'static str {
        KEYS[self.0 as usize]
    }

    /// Its position in `KEYS` (for a style table indexed the same way).
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The theme key for a capture name: the name itself, else its longest dotted prefix, in `KEYS`
/// or `ALIASES`; `None` if there is none (the text keeps the default colour; for the same node an
/// earlier pattern's colour stays).
pub fn highlight_for(capture: &str) -> Option<Highlight> {
    let index = |key: &str| KEYS.iter().position(|k| *k == key).map(|i| Highlight(i as u8));
    let mut name = capture;
    loop {
        if let Some(h) = index(name) {
            return Some(h);
        }
        if let Some((_, key)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
            return index(key);
        }
        name = &name[..name.rfind('.')?];
    }
}

/// Coloured text: `start..end` in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: ByteIdx,
    pub end: ByteIdx,
    pub highlight: Highlight,
}

#[derive(Debug)]
pub(super) struct HighlightQuery {
    query: Query,
    /// Per capture index.
    highlights: Vec<Option<Highlight>>,
}

impl HighlightQuery {
    /// Compiles `source` for `grammar` and maps its captures to theme keys; captures that colour
    /// nothing are logged once, at debug (`language` labels the log). The error carries its row
    /// and column (tree-sitter's `QueryError`).
    /// Blocking: a few ms for a big query; call off the UI thread.
    pub(super) fn compile(
        language: &str,
        grammar: &tree_sitter::Language,
        source: &str,
    ) -> Result<Self, tree_sitter::QueryError> {
        let query = Query::new(grammar, source)?;
        let highlights: Vec<Option<Highlight>> = query.capture_names().iter().map(|n| highlight_for(n)).collect();
        for (name, h) in query.capture_names().iter().zip(&highlights) {
            if h.is_none() && !name.starts_with('_') {
                tracing::debug!(language, capture = *name, "capture colours nothing");
            }
        }
        Ok(Self { query, highlights })
    }
}

/// The language's compiled highlight query (a built-in's is compiled once, on first use, shared by
/// all buffers; a plugin's is compiled by `plugin::load`, off the UI thread, so none until then).
pub(super) fn query(language: Language) -> Option<&'static HighlightQuery> {
    static QUERIES: [OnceLock<Option<HighlightQuery>>; Language::BUILT_IN.len()] =
        [const { OnceLock::new() }; Language::BUILT_IN.len()];
    if let Language::Plugin(p) = language {
        return p.get().query.get();
    }
    let i = Language::BUILT_IN.iter().position(|l| *l == language)?;
    QUERIES[i]
        .get_or_init(|| {
            let grammar = language.grammar()?;
            let source = language.highlight_queries().join("\n");
            match HighlightQuery::compile(language.id(), &grammar, &source) {
                Ok(q) => Some(q),
                Err(e) => {
                    tracing::error!(language = language.id(), %e, "bad highlight query");
                    None
                }
            }
        })
        .as_ref()
}

/// Compiles the language's query now (a few ms for C++): `ParseJob::run` calls it off the UI thread.
pub fn warm(language: Language) {
    let _ = query(language);
}

/// The spans for the bytes `start..end` of `rope` (which `tree` must match): sorted,
/// non-overlapping, clipped to the range. Nodes that start before the range (a block comment
/// opened above it) colour the part inside.
///
/// Overlaps: an inner node's capture wins over an enclosing one (an escape inside a string), and
/// for the same node a later pattern wins over an earlier one, as the grammars' queries expect
/// (`(identifier) @variable` comes first, the more specific patterns after it; C++'s and jed's
/// additions come after C's).
pub fn compute(language: Language, tree: &Tree, rope: &Rope, start: ByteIdx, end: ByteIdx) -> Vec<Span> {
    let Some(q) = query(language) else {
        return Vec::new();
    };
    if start >= end {
        return Vec::new();
    }
    let mut captures = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(start.0..end.0);
    let text = |node: Node| pos::bytes_in(rope, node.start_byte(), node.end_byte());
    let mut matches = cursor.matches(&q.query, tree.root_node(), text);
    while let Some(m) = matches.next() {
        for c in m.captures() {
            let Some(h) = q.highlights[c.index as usize] else {
                continue;
            };
            let (s, e) = (c.node.start_byte(), c.node.end_byte());
            if s.max(start.0) < e.min(end.0) {
                captures.push((s, e, m.pattern_index, h));
            }
        }
    }
    // Paint outer nodes first, then inner ones over them; for the same node, later patterns last.
    // Tree nodes nest or are disjoint, so length order is nesting order.
    captures.sort_unstable_by_key(|&(s, e, p, _)| (Reverse(e - s), p));
    const NONE: u8 = u8::MAX;
    let mut paint = vec![NONE; end.0 - start.0];
    for (s, e, _, h) in captures {
        paint[s.max(start.0) - start.0..e.min(end.0) - start.0].fill(h.0);
    }
    if language == Language::GitCommit {
        git_commit_lines(tree, rope, start, end, &mut paint);
    }
    let mut spans = Vec::new();
    let mut i = 0;
    while i < paint.len() {
        let h = paint[i];
        let run = paint[i..].iter().take_while(|&&x| x == h).count();
        if h != NONE {
            spans.push(Span {
                start: ByteIdx(start.0 + i),
                end: ByteIdx(start.0 + i + run),
                highlight: Highlight(h),
            });
        }
        i += run;
    }
    spans
}

/// The subject's length (in graphemes) past which `overflow` colours it.
const SUBJECT_WIDTH: usize = 50;

/// The line pass for a git commit message, painted over the query's colours in `paint` (one
/// highlight per byte of `start..end`): the subject's text past column 50 (`overflow`), text on
/// line 2 (`invalid`: git wants it blank) and, below the scissors line of `git commit -v`, the diff
/// by line kinds (the grammar has one `diff` node there; jed has no injections). O(the range).
fn git_commit_lines(tree: &Tree, rope: &Rope, start: ByteIdx, end: ByteIdx, paint: &mut [u8]) {
    let key = |k: &str| highlight_for(k).expect("key in KEYS").0;
    let mut put = |from: usize, to: usize, h: u8| {
        let (from, to) = (from.max(start.0), to.min(end.0));
        if from < to {
            paint[from - start.0..to - start.0].fill(h);
        }
    };
    let head = |from: ByteIdx, to: ByteIdx, max: usize| {
        let to = floor(rope, to.0.min(from.0 + max));
        rope.slice(from.0..to.0).to_string()
    };
    let lines = pos::line_count(rope);
    let (l0, e0, _) = pos::line_bounds(rope, 0);
    let subject = head(l0, e0, 512);
    if !subject.starts_with('#') {
        let mut off = 0;
        for _ in 0..SUBJECT_WIDTH {
            off = pos::next_grapheme_in(&subject, off);
        }
        if l0.0 + off < e0.0 {
            put(l0.0 + off, e0.0, key("overflow"));
        }
    }
    if lines > 1 {
        let (l1, e1, _) = pos::line_bounds(rope, 1);
        let text = head(l1, e1, 512);
        if !text.starts_with('#') && !text.trim().is_empty() {
            put(l1.0, e1.0, key("invalid"));
        }
    }
    // Below the scissors line.
    let mut cur = tree.root_node().walk();
    let from = tree
        .root_node()
        .children(&mut cur)
        .find(|n| n.kind() == "scissor")
        .map(|n| n.end_byte());
    let Some(from) = from.filter(|&f| f < end.0) else {
        return;
    };
    let (hdr, hunk, ins, del) = (
        key("diff.header"),
        key("diff.hunk"),
        key("diff.inserted"),
        key("diff.deleted"),
    );
    const HEADERS: &[&str] = &[
        "--- a/",
        "+++ b/",
        "--- /dev/null",
        "+++ /dev/null",
        "diff --git",
        "index ",
        "new file mode",
        "deleted file mode",
        "old mode",
        "new mode",
        "similarity index",
        "rename ",
        "Binary files",
    ];
    let mut line = pos::line_of(rope, ByteIdx(from.max(start.0)));
    if pos::line_start(rope, line).0 < from {
        line += 1;
    }
    while line < lines {
        let (ls, le, _) = pos::line_bounds(rope, line);
        if ls >= end {
            break;
        }
        let t = head(ls, le, 24);
        let kind = if t.starts_with("@@") {
            Some(hunk)
        } else if HEADERS.iter().any(|p| t.starts_with(p)) {
            Some(hdr)
        } else if t.starts_with('+') {
            Some(ins)
        } else if t.starts_with('-') {
            Some(del)
        } else {
            None
        };
        if let Some(h) = kind {
            put(ls.0, le.0, h);
        }
        line += 1;
    }
}

fn floor(rope: &Rope, at: usize) -> ByteIdx {
    ByteIdx(rope.floor_char_boundary(at.min(rope.len())))
}

/// Spans for `start..end`, computed from the tree of `version`.
struct Region {
    version: u64,
    start: ByteIdx,
    end: ByteIdx,
    spans: Vec<Span>,
}

/// Cached spans for one buffer (see the module docs).
#[derive(Default)]
pub struct Highlights {
    /// Oldest first.
    regions: Vec<Region>,
    /// How many times `compute` ran (tests check the cache).
    #[cfg(test)]
    pub computed: usize,
}

impl Highlights {
    /// Forgets every span (the language changed, or the text went over the size limit).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Follows a change set the buffer applied, so the kept spans stay on the same text until the
    /// new tree replaces them. Text inserted at a span's edge isn't coloured; a span whose text
    /// was deleted goes.
    pub fn edit(&mut self, changes: &ChangeSet) {
        if self.regions.is_empty() || changes.is_empty() {
            return;
        }
        let m = changes.mapper();
        for r in &mut self.regions {
            r.start = m.map(r.start, Assoc::Before);
            r.end = m.map(r.end, Assoc::After);
            for s in &mut r.spans {
                s.start = m.map(s.start, Assoc::After);
                s.end = m.map(s.end, Assoc::Before);
            }
            r.spans.retain(|s| s.start < s.end);
        }
    }

    /// The spans on lines `lines` of `rope` (sorted, non-overlapping; they may reach past the
    /// range's ends). With `tree` (current for `rope`, numbered `version`) they're exact, from the
    /// cache or a new query over the lines plus `MARGIN`. Without one, the last spans, mapped
    /// through the edits since (empty before the first tree).
    pub fn spans(
        &mut self,
        language: Language,
        tree: Option<&Tree>,
        version: u64,
        rope: &Rope,
        lines: Range<usize>,
    ) -> &[Span] {
        let total = pos::line_count(rope);
        let last = lines.end.min(total);
        if lines.start >= last {
            return &[];
        }
        let want = (pos::line_start(rope, lines.start), pos::line_bounds(rope, last - 1).2);
        // Very long lines (minified code, a data table) are coloured only so far.
        let want = (want.0, want.1.min(ByteIdx(want.0.0 + MAX_QUERY_BYTES)));
        let covers = |r: &Region| r.start <= want.0 && want.1 <= r.end;
        let i = match tree {
            Some(tree) => {
                self.regions.retain(|r| r.version == version);
                match self.regions.iter().position(covers) {
                    Some(i) => i,
                    None => {
                        let from = lines.start.saturating_sub(MARGIN);
                        let to = (last + MARGIN).min(total);
                        let start = pos::line_start(rope, from).max(floor(rope, want.0.0.saturating_sub(MARGIN_BYTES)));
                        let end = pos::line_bounds(rope, to - 1)
                            .2
                            .min(floor(rope, want.1.0 + MARGIN_BYTES));
                        let spans = compute(language, tree, rope, start, end);
                        #[cfg(test)]
                        {
                            self.computed += 1;
                        }
                        if self.regions.len() >= MAX_REGIONS {
                            self.regions.remove(0);
                        }
                        self.regions.push(Region {
                            version,
                            start,
                            end,
                            spans,
                        });
                        self.regions.len() - 1
                    }
                }
            }
            None => {
                let overlaps = |r: &&Region| r.start < want.1 && want.0 < r.end;
                let Some(i) = self
                    .regions
                    .iter()
                    .rposition(covers)
                    .or_else(|| self.regions.iter().rposition(|r| overlaps(&r)))
                else {
                    return &[];
                };
                i
            }
        };
        let spans = &self.regions[i].spans;
        let a = spans.partition_point(|s| s.end <= want.0);
        let b = a + spans[a..].partition_point(|s| s.start < want.1);
        &spans[a..b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::buffer::change::Change;
    use crate::buffer::undo::EditKind;
    use crate::selection::{Selection, Selections};

    const MAX: usize = super::super::DEFAULT_MAX_BYTES;

    fn buffer(language: Language, text: &str) -> Buffer {
        let mut buf = Buffer::from_str(text);
        buf.syntax.set_language(language);
        settle(&mut buf);
        buf
    }

    /// As the app: inline re-parses, else background ones (run here).
    fn settle(buf: &mut Buffer) {
        while let Some(job) = buf.syntax.update(&buf.rope, MAX) {
            buf.syntax.parsed(job.run());
        }
        assert!(buf.syntax.tree().is_some());
    }

    fn edit(buf: &mut Buffer, at: usize, end: usize, text: &str) {
        let sel = Selections::single(Selection::cursor(ByteIdx(0)));
        let change = Change {
            start: ByteIdx(at),
            end: ByteIdx(end),
            text: text.into(),
        };
        buf.edit(ChangeSet::new(vec![change]), sel.clone(), sel, EditKind::Other);
    }

    /// Every span of the whole text, as (text, key).
    fn all(language: Language, text: &str) -> Vec<(String, &'static str)> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.grammar().unwrap()).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let rope = Rope::from_str(text);
        let spans = compute(language, &tree, &rope, ByteIdx(0), ByteIdx(text.len()));
        check_sorted(&spans);
        spans
            .iter()
            .map(|s| (text[s.start.0..s.end.0].to_string(), s.highlight.key()))
            .collect()
    }

    fn check_sorted(spans: &[Span]) {
        for w in spans.windows(2) {
            assert!(w[0].end <= w[1].start, "{w:?}");
        }
        assert!(spans.iter().all(|s| s.start < s.end));
    }

    /// `want` (text, key) pairs all appear, in this order: each at the text's next occurrence.
    fn assert_has(language: Language, text: &str, want: &[(&str, &str)]) {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.grammar().unwrap()).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let rope = Rope::from_str(text);
        let spans = compute(language, &tree, &rope, ByteIdx(0), ByteIdx(text.len()));
        let mut from = 0;
        for (needle, key) in want {
            let at = from
                + text[from..]
                    .find(needle)
                    .unwrap_or_else(|| panic!("{needle:?} not in text"));
            from = at + needle.len();
            let range = ByteIdx(at)..ByteIdx(at + needle.len());
            assert!(
                spans
                    .iter()
                    .any(|s| s.start == range.start && s.end == range.end && s.highlight.key() == *key),
                "{needle:?} ({range:?}) as {key}: {:?}",
                all(language, text)
            );
        }
    }

    #[test]
    fn captures_map_to_keys_by_prefix() {
        let key = |c: &str| highlight_for(c).map(Highlight::key);
        assert_eq!(key("keyword"), Some("keyword"));
        assert_eq!(key("keyword.control"), Some("keyword.control"));
        assert_eq!(key("keyword.control.import"), Some("keyword.control"));
        assert_eq!(key("function.method.builtin"), Some("function"));
        assert_eq!(key("function.special"), Some("function"));
        assert_eq!(key("variable.builtin"), Some("variable.builtin"));
        assert_eq!(key("embedded"), None);
        assert_eq!(key("delimiter"), None);
        assert_eq!(key(""), None);
        // Aliases, also as prefixes.
        assert_eq!(key("boolean"), Some("constant.builtin"));
        assert_eq!(key("string.special.key"), Some("property"));
        assert_eq!(key("string.special"), Some("string"));
        assert_eq!(key("string.escape"), Some("escape"));
        assert_eq!(key("keyword.conditional.ternary"), Some("keyword.control"));
        assert_eq!(key("text.title"), Some("markup.heading"));
        assert_eq!(key("text"), None);
        for l in Language::BUILT_IN {
            assert_eq!(query(l).is_some(), l.grammar().is_some(), "{l:?}");
        }
    }

    /// Every language's query compiles, and each capture name colours something unless it is
    /// one the grammars use for text jed leaves plain (or for a predicate: `@_name`).
    #[test]
    fn every_query_compiles_and_its_captures_map() {
        const UNCOLOURED: &[&str] = &[
            "attribute", // Rust's `#[…]` and YAML directives: plain in Dark+.
            "delimiter", // C's `;` `,` `.`
            "preproc",   // Objective-C's `#undef` (the whole directive).
            "none",
            "embedded",
            "spell",
            "nospell",
            "error",
            "markup", // XML text.
            "clean",
            "text.note",
            "text.warning",
            "text.danger",
        ];
        for l in Language::BUILT_IN {
            let Some(grammar) = l.grammar() else { continue };
            let q = Query::new(&grammar, &l.highlight_queries().join("\n")).unwrap_or_else(|e| panic!("{l:?}: {e}"));
            let unmapped: Vec<_> = q
                .capture_names()
                .iter()
                .filter(|n| highlight_for(n).is_none() && !UNCOLOURED.contains(n) && !n.starts_with('_'))
                .collect();
            assert!(unmapped.is_empty(), "{l:?}: no theme key for {unmapped:?}");
            assert!(query(l).is_some(), "{l:?}");
        }
    }

    #[test]
    fn rust_spans() {
        let text = "use std::fmt;\n/// Doc\npub struct P<'a> { x: &'a str }\nconst MAX: u32 = 10;\nfn f(a: i32) -> Option<i32> {\n    let s = \"x\\n\"; // c\n    if a > 0 { return Some(a); }\n    println!(\"{}\", MAX);\n    self.go(1.5, true);\n    None\n}\n";
        assert_has(
            Language::Rust,
            text,
            &[
                ("use", "keyword"),
                ("std", "variable"),
                ("/// Doc\n", "comment"),
                ("pub", "keyword"),
                ("struct", "keyword"),
                ("P", "type"),
                ("'a", "type"),
                ("x", "property"),
                ("str", "type"),
                ("const", "keyword"),
                ("MAX", "constant"),
                ("u32", "type"),
                ("10", "number"),
                ("fn", "keyword"),
                ("f", "function"),
                ("a", "variable.parameter"),
                ("Option", "type"),
                ("let", "keyword"),
                ("s", "variable"),
                ("\"x", "string"),
                ("\\n", "escape"),
                ("// c", "comment"),
                ("if", "keyword.control"),
                ("return", "keyword.control"),
                ("println!", "function"),
                ("MAX", "constant"),
                ("self", "variable.builtin"),
                ("go", "function"),
                ("1.5", "number"),
                ("true", "constant.builtin"),
                ("None", "constructor"),
            ],
        );
    }

    /// The block grammar only: headings, list and quote markers, code blocks, link definitions;
    /// inline text stays plain.
    #[test]
    fn markdown_spans() {
        let text = "# Title\n\nSome *text* with `code`.\n\n- item\n1. one\n\n> quote\n> more\n\n```rust\nfn x() {}\n```\n\n[ref]: http://x.y\n\n---\nSetext\n===\n";
        assert_eq!(
            all(Language::Markdown, text),
            [
                ("# Title\n".into(), "markup.heading"),
                ("- ".into(), "markup.list"),
                ("1. ".into(), "markup.list"),
                ("> ".into(), "markup.quote"),
                ("> ".into(), "markup.quote"),
                ("```rust".into(), "punctuation"),
                ("\nfn x() {}\n".into(), "markup.raw"),
                ("```".into(), "punctuation"),
                ("\n".into(), "markup.raw"),
                ("[ref]".into(), "markup.link"),
                ("http://x.y".into(), "markup.link"),
                ("---\n".into(), "punctuation.special"),
                ("Setext\n===\n".into(), "markup.heading"),
            ]
        );
    }

    #[test]
    fn yaml_spans() {
        let text = "# c\nname: CI\non:\n  push:\n    branches: [main, \"dev\"]\nx: 1.5\ny: true\nz: null\nanchors: &a !!str 'v'\nref: *a\n";
        assert_has(
            Language::Yaml,
            text,
            &[
                ("# c", "comment"),
                ("name", "tag"),
                (":", "punctuation"),
                ("CI", "string"),
                ("push", "tag"),
                ("main", "string"),
                ("\"dev\"", "string"),
                ("1.5", "number"),
                ("true", "constant.builtin"),
                ("null", "constant.builtin"),
                ("anchors", "tag"),
                ("a", "type"),
                ("!!str", "keyword"),
                ("'v'", "string"),
            ],
        );
    }

    #[test]
    fn json_spans() {
        let text = "{\"a\": 1, \"b\": [true, null, \"s\\n\"]}\n";
        assert_eq!(
            all(Language::Json, text),
            [
                ("\"a\"".into(), "property"),
                ("1".into(), "number"),
                ("\"b\"".into(), "property"),
                ("true".into(), "constant.builtin"),
                ("null".into(), "constant.builtin"),
                ("\"s".into(), "string"),
                ("\\n".into(), "escape"),
                ("\"".into(), "string"),
            ]
        );
        assert_eq!(
            all(Language::Jsonc, "// c\n{\"a\": 1 /* b */}\n"),
            [
                ("// c".into(), "comment"),
                ("\"a\"".into(), "property"),
                ("1".into(), "number"),
                ("/* b */".into(), "comment"),
            ]
        );
    }

    #[test]
    fn xml_spans() {
        let text = "<?xml version=\"1.0\"?>\n<!-- c -->\n<root a=\"1\">text &amp; <b/></root>\n";
        assert_has(
            Language::Xml,
            text,
            &[
                ("<?", "punctuation.tag"),
                ("xml", "keyword"),
                ("version", "property"),
                ("<!-- c -->", "comment"),
                ("<", "punctuation.tag"),
                ("root", "tag"),
                ("a", "tag.attribute"),
                ("1", "string"),
                (">", "punctuation.tag"),
                ("&amp;", "constant.builtin"),
                ("b", "tag"),
                ("root", "tag"),
            ],
        );
    }

    #[test]
    fn toml_spans() {
        let text =
            "# c\n[package]\nname = \"jed\"\nversion.workspace = true\n[[bin]]\nx = 1\nd = 1979-05-27\n\"q\" = 'v'\n";
        assert_has(
            Language::Toml,
            text,
            &[
                ("# c", "comment"),
                ("package", "type"),
                ("name", "property"),
                ("=", "operator"),
                ("\"jed\"", "string"),
                ("version", "property"),
                ("workspace", "property"),
                ("true", "constant.builtin"),
                ("bin", "type"),
                ("1", "number"),
                ("1979-05-27", "string"),
                ("\"q\"", "property"),
                ("'v'", "string"),
            ],
        );
    }

    #[test]
    fn shell_spans() {
        let text = "#!/bin/bash\n# c\nlocal x=1\nif [ -f \"$x\" ]; then\n  echo \"a ${x} $(pwd)\" 'r' 42\nfi\nf() { return 0; }\nfor i in 1 2; do ls; done\n";
        assert_has(
            Language::ShellScript,
            text,
            &[
                ("#!/bin/bash", "comment"),
                ("local", "keyword"),
                ("x", "property"),
                ("1", "number"),
                ("if", "keyword.control"),
                ("$", "variable"),
                ("then", "keyword.control"),
                ("echo", "function"),
                ("\"a ", "string"),
                ("${", "variable"),
                ("pwd", "function"),
                ("'r'", "string"),
                ("42", "number"),
                ("fi", "keyword.control"),
                ("f", "function"),
                ("return", "keyword.control"),
                ("for", "keyword.control"),
                ("in", "keyword.control"),
                ("do", "keyword.control"),
                ("ls", "function"),
                ("done", "keyword.control"),
            ],
        );
    }

    #[test]
    fn cmake_spans() {
        let text = "# c\ncmake_minimum_required(VERSION 3.20)\nset(x \"a ${Y}\" ON)\nif(NOT x)\n  message(STATUS \"x\")\nendif()\nfunction(f a)\n  return()\nendfunction()\n";
        assert_eq!(
            all(Language::CMake, text),
            [
                ("# c".into(), "comment"),
                ("cmake_minimum_required".into(), "function"),
                ("VERSION".into(), "constant"),
                ("set".into(), "function"),
                ("\"a ".into(), "string"),
                ("${Y}".into(), "variable"),
                ("\"".into(), "string"),
                ("ON".into(), "constant.builtin"),
                ("if".into(), "keyword.control"),
                ("NOT".into(), "constant"),
                ("message".into(), "function"),
                ("STATUS".into(), "constant"),
                ("\"x\"".into(), "string"),
                ("endif".into(), "keyword.control"),
                ("function".into(), "keyword"),
                ("f".into(), "function"),
                ("return".into(), "keyword.control"),
                ("endfunction".into(), "keyword"),
            ]
        );
    }

    #[test]
    fn makefile_spans() {
        let text = "# c\nCC = gcc\nall: main.o\n\t$(CC) -o $@ $^\nifeq ($(CC),gcc)\ninclude x.mk\nendif\n%.o: %.c\n\techo $(info hi)\n";
        assert_has(
            Language::Makefile,
            text,
            &[
                ("# c", "comment"),
                ("CC", "variable"),
                ("=", "operator"),
                ("gcc", "punctuation"),
                ("all", "function"),
                ("CC", "variable"),
                ("ifeq", "keyword.control"),
                ("include", "keyword.control"),
                ("endif", "keyword.control"),
                ("%.o", "function"),
                ("info", "function"),
            ],
        );
    }

    #[test]
    fn dockerfile_spans() {
        let text = "# c\nFROM ubuntu:22.04 AS build\nENV A=1 B=\"two\"\nRUN echo $A\nEXPOSE 8080\nCMD [\"run\"]\n";
        assert_has(
            Language::Dockerfile,
            text,
            &[
                ("# c\n", "comment"),
                ("FROM", "keyword"),
                ("AS", "keyword"),
                ("ENV", "keyword"),
                ("A", "property"),
                ("B", "property"),
                ("\"two\"", "string"),
                ("RUN", "keyword"),
                ("EXPOSE", "keyword"),
                ("8080", "number"),
                ("CMD", "keyword"),
                ("\"run\"", "string"),
            ],
        );
    }

    const GIT_PLAIN: &str = "feat(ui): add a thing\n\nBody text.\n\nSigned-off-by: A <a@b.c>\n\n# Please enter the commit message for your changes. Lines starting\n# with '#' will be ignored, and an empty message aborts the commit.\n#\n# On branch main\n# Changes to be committed:\n#\tmodified:   src/a.rs\n#\tnew file:   b.txt\n#\n";

    #[test]
    fn git_commit_spans() {
        assert_has(
            Language::GitCommit,
            GIT_PLAIN,
            &[
                ("feat", "keyword"),
                ("ui", "variable.parameter"),
                (" add a thing", "markup.heading"),
                ("Signed-off-by: ", "label"),
                (
                    "# Please enter the commit message for your changes. Lines starting",
                    "comment",
                ),
                ("# On branch ", "comment"),
                ("main", "markup.link"),
                ("Changes to be committed:", "markup.heading"),
                ("modified:", "keyword"),
                ("src/a.rs", "string"),
                ("new file:", "keyword"),
                ("b.txt", "string"),
            ],
        );
    }

    #[test]
    fn git_commit_spans_merge_and_trailers() {
        let merge = "Merge branch 'x' into main\n\n# Conflicts:\n#\tsrc/a.rs\n";
        assert_has(
            Language::GitCommit,
            merge,
            &[
                ("Merge branch 'x' into main", "markup.heading"),
                // A comment ending in `:` stays a comment (jed's query), not the grammar's heading.
                ("# Conflicts:\n#\tsrc/a.rs\n", "comment"),
            ],
        );
        let trailers = "Fix it\n\nSigned-off-by: A <a@b>\nCo-Authored-By: B <b@c>\nBREAKING CHANGE: gone\n";
        assert_has(
            Language::GitCommit,
            trailers,
            &[
                ("Fix it", "markup.heading"),
                ("Signed-off-by: ", "label"),
                ("Co-Authored-By: ", "label"),
                ("BREAKING CHANGE: ", "keyword"),
            ],
        );
    }

    /// The key of every byte of `text` in `lo..hi` (computed over just that range), as runs of
    /// (text, key).
    fn git_range(text: &str, lo: usize, hi: usize) -> Vec<(String, &'static str)> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&Language::GitCommit.grammar().unwrap()).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let rope = Rope::from_str(text);
        let spans = compute(Language::GitCommit, &tree, &rope, ByteIdx(lo), ByteIdx(hi));
        check_sorted(&spans);
        spans
            .iter()
            .map(|s| (text[s.start.0..s.end.0].to_string(), s.highlight.key()))
            .collect()
    }

    #[test]
    fn git_commit_subject_overflow_and_line_two() {
        let ok = "a".repeat(50);
        assert!(all(Language::GitCommit, &ok).iter().all(|(_, k)| *k != "overflow"));
        let long = format!("{}{}", ok, "b".repeat(10));
        let spans = all(Language::GitCommit, &long);
        assert_eq!(spans.last().unwrap(), &("b".repeat(10), "overflow"), "{spans:?}");
        assert_eq!(spans.first().unwrap(), &(ok.clone(), "markup.heading"), "{spans:?}");
        // Graphemes, not bytes: 50 two-byte chars fit.
        let wide = "é".repeat(50);
        assert!(all(Language::GitCommit, &wide).iter().all(|(_, k)| *k != "overflow"));
        // Wide chars count one each (graphemes, as VS Code's subject length check counts chars).
        let cjk = "字".repeat(51);
        assert_eq!(
            all(Language::GitCommit, &cjk).last().unwrap(),
            &("字".into(), "overflow")
        );
        // A comment first line is no subject.
        let c = format!("# {}\n", "c".repeat(60));
        assert!(all(Language::GitCommit, &c).iter().all(|(_, k)| *k != "overflow"));

        let invalid = |t: &str| all(Language::GitCommit, t).iter().any(|(_, k)| *k == "invalid");
        assert!(
            all(Language::GitCommit, "subject\nbody right away\n").contains(&("body right away".into(), "invalid"))
        );
        assert!(!invalid("subject\n\nbody\n"));
        assert!(!invalid("subject\n# comment\n"));
        assert!(!invalid("subject\n   \nbody\n"));
        // Only line 2: text on line 3 or later is fine.
        assert!(!invalid("subject\n\nbody\nmore body\n"));
        assert!(!invalid("subject"));
    }

    const SCISSORS: &str = "msg

- body dash
+ body plus
# ------------------------ >8 ------------------------
# Do not modify or remove the line above.
# Everything below it will be ignored.
diff --git a/x b/x
index 1..2 100644
--- a/x
+++ b/x
@@ -1,3 +1,3 @@
 ctx
-old
+new
";

    #[test]
    fn git_commit_scissors_diff_lines() {
        let spans = all(Language::GitCommit, SCISSORS);
        let has = |t: &str, k: &str| assert!(spans.iter().any(|(s, key)| s == t && *key == k), "{t:?} {k}: {spans:?}");
        has("diff --git a/x b/x", "diff.header");
        has("index 1..2 100644", "diff.header");
        has("--- a/x", "diff.header");
        has("+++ b/x", "diff.header");
        has("@@ -1,3 +1,3 @@", "diff.hunk");
        has("-old", "diff.deleted");
        has("+new", "diff.inserted");
        // Above the scissors line and context lines are not diff lines.
        assert!(
            !spans
                .iter()
                .any(|(t, _)| t.contains("body") || t == " ctx" || t == "ctx"),
            "{spans:?}"
        );
        assert_eq!(spans.iter().filter(|(_, k)| k.starts_with("diff.")).count(), 7);
    }

    #[test]
    fn git_commit_diff_lines_in_a_sub_range() {
        // A range that starts inside the diff (after the scissors) still colours its lines.
        let lo = SCISSORS.find("@@").unwrap();
        let spans = git_range(SCISSORS, lo, SCISSORS.len());
        assert_eq!(
            spans.iter().map(|(_, k)| *k).collect::<Vec<_>>(),
            ["diff.hunk", "diff.deleted", "diff.inserted"],
            "{spans:?}"
        );
        // And one cut mid-line colours the part inside.
        let lo = SCISSORS.find("-old").unwrap() + 2;
        let spans = git_range(SCISSORS, lo, SCISSORS.len());
        assert_eq!(spans[0], ("ld".to_string(), "diff.deleted"), "{spans:?}");
    }

    #[test]
    fn c_spans() {
        let text = "#include <stdio.h>\nint main(void) {\n  /* hi */\n  return printf(\"x\\n\", 42) + 'c';\n}\n";
        assert_has(
            Language::C,
            text,
            &[
                ("#include", "keyword.control"),
                ("<stdio.h>", "string"),
                ("int", "type.builtin"),
                ("main", "function"),
                ("void", "type.builtin"),
                ("/* hi */", "comment"),
                ("return", "keyword.control"),
                ("printf", "function"),
                ("\"x", "string"),
                ("\\n", "escape"),
                ("\"", "string"),
                ("42", "number"),
                ("+", "operator"),
                ("'c'", "string"),
            ],
        );
        // Exact ranges, by offset.
        let spans = all(Language::C, "int x = MAX;");
        assert_eq!(
            spans,
            [
                ("int".into(), "type.builtin"),
                ("x".into(), "variable"),
                ("=".into(), "operator"),
                ("MAX".into(), "constant"),
            ]
        );
    }

    #[test]
    fn objc_spans() {
        let text = "#import <Foundation/Foundation.h>\n@interface Foo : NSObject <Bar>\n@property (nonatomic, copy) NSString *name;\n@end\n@implementation Foo\n- (void)run:(int)count {\n  [self.obj foo:count];\n  NSString *s = @\"hi\";\n  if (count) { return; }\n}\n@end // c\n";
        assert_has(
            Language::ObjC,
            text,
            &[
                ("#import", "keyword.control"),
                ("@interface", "keyword"),
                ("Foo", "type"),
                ("NSObject", "type"),
                ("Bar", "type"),
                ("@property", "keyword"),
                ("@end", "keyword"),
                ("@implementation", "keyword"),
                ("void", "type.builtin"),
                ("run", "function"),
                ("count", "variable.parameter"),
                ("self", "variable.builtin"),
                ("foo", "function"),
                ("count", "variable"),
                ("@\"hi\"", "string"),
                ("if", "keyword.control"),
                ("// c", "comment"),
            ],
        );
    }

    /// The rest of `queries/objc.scm`: conditionals, attributes, doc comments, protocols, `super`,
    /// selectors, control at-keywords; and Objective-C++ colours the same.
    #[test]
    fn objc_spans_more() {
        let text = "#if DEBUG\n/** Doc. */\n@protocol P <Q>\n- (id)init __attribute__((unused));\n@end\n#endif\n@implementation C (Cat)\n- (id)init {\n  @try { self = [super init]; } @finally {}\n  SEL s = @selector(run:);\n  BOOL b = YES;\n  return @[@1];\n}\n@end\n";
        let want: &[(&str, &str)] = &[
            ("#if", "keyword.control"),
            ("DEBUG", "constant"),
            ("/** Doc. */", "comment"),
            ("@protocol", "keyword"),
            ("P", "type"),
            ("Q", "type"),
            ("-", "operator"),
            ("id", "type.builtin"),
            ("init", "function"),
            ("@end", "keyword"),
            ("#endif", "keyword.control"),
            ("@implementation", "keyword"),
            ("C", "type"),
            ("Cat", "type"),
            ("@try", "keyword.control"),
            ("self", "variable.builtin"),
            ("super", "variable.builtin"),
            ("init", "function"),
            ("@finally", "keyword.control"),
            ("SEL", "type.builtin"),
            ("s", "variable"),
            ("@selector", "keyword"),
            ("BOOL", "type.builtin"),
            ("YES", "constant.builtin"),
            ("return", "keyword.control"),
            ("@1", "number"),
        ];
        assert_has(Language::ObjC, text, want);
        assert_eq!(all(Language::ObjCpp, text), all(Language::ObjC, text));
        // An attribute is plain (as in C), its argument not a type.
        let spans = all(Language::ObjC, text);
        assert!(!spans.iter().any(|(t, _)| t.contains("__attribute__")), "{spans:?}");
        assert!(spans.contains(&("unused".to_string(), "variable")), "{spans:?}");
    }

    #[test]
    fn cpp_spans() {
        let text = "namespace ns {\nclass A : public B {\n  int f(const T& v) { return this ? 1 : nullptr; }\n};\n}\nauto s = R\"(raw)\"; // c\n";
        assert_has(
            Language::Cpp,
            text,
            &[
                ("namespace", "keyword"),
                ("ns", "type"),
                ("class", "keyword"),
                ("A", "type"),
                ("public", "keyword"),
                ("f", "function"),
                ("const", "keyword"),
                ("v", "variable.parameter"),
                ("return", "keyword.control"),
                ("this", "variable.builtin"),
                ("1", "number"),
                ("nullptr", "constant.builtin"),
                ("auto", "type.builtin"),
                ("R\"(raw)\"", "string"),
                ("// c", "comment"),
            ],
        );
    }

    #[test]
    fn python_spans() {
        let text = "import os\nclass K:\n    def f(self, x=1):\n        # c\n        return \"s\\t\" + str(x) if x else None\n";
        assert_has(
            Language::Python,
            text,
            &[
                ("import", "keyword.control"),
                ("os", "variable"),
                ("class", "keyword"),
                ("K", "type"),
                ("def", "keyword"),
                ("f", "function"),
                ("self", "variable.builtin"),
                ("x", "variable.parameter"),
                ("1", "number"),
                ("# c", "comment"),
                ("return", "keyword.control"),
                ("\"s", "string"),
                ("\\t", "escape"),
                ("str", "function"),
                ("if", "keyword.control"),
                ("None", "constant.builtin"),
            ],
        );
    }

    /// Text predicates (`#match?`) are checked against the node's text, read from the rope even
    /// when it spans chunks: C's `^[A-Z][A-Z\\d_]*$` constants, Python's `^[A-Z]` constructors
    /// and jed's `self` / `cls`.
    #[test]
    fn match_predicates_read_the_text() {
        let spans = all(Language::C, "int v = MAX_2 + Max;");
        assert_eq!(
            spans,
            [
                ("int".into(), "type.builtin"),
                ("v".into(), "variable"),
                ("=".into(), "operator"),
                ("MAX_2".into(), "constant"),
                ("+".into(), "operator"),
                ("Max".into(), "variable"),
            ]
        );
        let py = all(Language::Python, "x = Foo(BAR, cls, selfish)\n");
        let key = |t: &str| py.iter().find(|(s, _)| s == t).map(|(_, k)| *k);
        assert_eq!(key("x"), Some("variable"));
        assert_eq!(
            key("Foo"),
            Some("function"),
            "a call: the later pattern wins over constructor"
        );
        assert_eq!(key("BAR"), Some("constant"));
        assert_eq!(key("cls"), Some("variable.builtin"));
        assert_eq!(key("selfish"), Some("variable"));
        let py = all(Language::Python, "y = Foo\n");
        assert_eq!(py[2], ("Foo".into(), "constructor"));

        // An identifier longer than a rope chunk: the predicate sees all of it, so a lowercase
        // letter at its end (in a later chunk) means it isn't a constant.
        for (tail, want) in [("", "constant"), ("a", "variable")] {
            let name = format!("{}{tail}", "A".repeat(20_000));
            let text = format!("int v = {name};");
            let rope = Rope::from_str(&text);
            assert!(pos::chunk_from(&rope, 8).len() < name.len(), "spans chunks");
            let spans = all(Language::C, &text);
            assert_eq!(spans[3], (name, want));
        }
    }

    /// A block comment opened above the queried lines colours the lines inside.
    #[test]
    fn nodes_starting_above_the_range_colour_it() {
        let text = "int a;\n/* one\n   two\n   three */\nint b;\n";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&Language::C.grammar().unwrap()).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let rope = Rope::from_str(text);
        let (start, end) = (pos::line_start(&rope, 2), pos::line_bounds(&rope, 2).2);
        let spans = compute(Language::C, &tree, &rope, start, end);
        assert_eq!(
            spans,
            [Span {
                start,
                end, // the comment's line break too
                highlight: highlight_for("comment").unwrap()
            }]
        );
        // Through the cache, asking for lines 2..3 of a long file whose comment starts far above.
        let text = format!("/*\n{}*/\nint x;\n", "text\n".repeat(200));
        let mut buf = buffer(Language::C, &text);
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 150..152).to_vec();
        assert_eq!(spans.len(), 1);
        assert!(spans[0].start <= pos::line_start(&rope, 150));
        assert_eq!(spans[0].highlight.key(), "comment");
        assert!(spans[0].end > pos::line_end(&rope, 151));
    }

    #[test]
    fn spans_are_cached_until_the_tree_changes() {
        let text = "int f(int x) { return x; }\n".repeat(400);
        let mut buf = buffer(Language::C, &text);
        let line = text.find('\n').unwrap() + 1;
        let runs = |buf: &Buffer| buf.syntax.highlight_queries_run();
        let rope = buf.rope.clone();
        let first = buf.syntax.spans(&rope, 0..40).to_vec();
        assert_eq!(runs(&buf), 1);
        assert_eq!(first.first().unwrap().highlight.key(), "type.builtin");
        assert!(first.last().unwrap().end.0 >= 39 * line, "covers the lines asked for");
        // Again, and scrolled within the margin: no query.
        assert_eq!(buf.syntax.spans(&rope, 0..40), first.as_slice());
        let scrolled = buf.syntax.spans(&rope, 30..70).to_vec();
        assert_eq!(runs(&buf), 1);
        assert_eq!(scrolled.first().unwrap().start.0, 30 * line);
        // Far away: a new region; going back finds the first one still cached.
        let far = buf.syntax.spans(&rope, 300..340).to_vec();
        assert_eq!(runs(&buf), 2);
        assert_eq!(far.first().unwrap().start.0, 300 * line);
        buf.syntax.spans(&rope, 0..40);
        assert_eq!(runs(&buf), 2);

        // An edit: until the re-parse the old spans are kept, moved along with the text.
        edit(&mut buf, 0, 0, "unsigned ");
        assert!(buf.syntax.tree().is_none());
        let rope = buf.rope.clone();
        let stale = buf.syntax.spans(&rope, 0..40).to_vec();
        assert_eq!(runs(&buf), 2);
        assert_eq!(stale.len(), first.len());
        assert_eq!(stale[0].start.0, 9, "after the insert");
        assert_eq!(stale[1].start.0, first[1].start.0 + 9);
        // The new tree: queried again, and right.
        settle(&mut buf);
        let fresh = buf.syntax.spans(&rope, 0..40).to_vec();
        assert_eq!(runs(&buf), 3);
        assert_eq!(
            (fresh[0].start.0, fresh[0].end.0, fresh[0].highlight.key()),
            (0, 12, "type.builtin"),
            "`unsigned int`"
        );
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&Language::C.grammar().unwrap()).unwrap();
        let tree = parser.parse(rope.to_string(), None).unwrap();
        let want = compute(Language::C, &tree, &rope, ByteIdx(0), pos::line_bounds(&rope, 39).2);
        assert_eq!(fresh, want);
        buf.syntax.spans(&rope, 0..40);
        assert_eq!(runs(&buf), 3);
    }

    /// Stale spans never panic or leave the text, whatever the edits do to them.
    #[test]
    fn stale_spans_follow_deletes() {
        let mut buf = buffer(Language::Python, "def f():\n    return 'x'\n");
        let rope = buf.rope.clone();
        let before = buf.syntax.spans(&rope, 0..2).len();
        assert!(before > 3);
        edit(&mut buf, 0, rope.len(), "x");
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 0..1).to_vec();
        check_sorted(&spans);
        assert!(spans.iter().all(|s| s.end.0 <= rope.len()));
        // Leaving the language drops them.
        edit(&mut buf, 0, 0, "def g(): pass\n");
        settle(&mut buf);
        let rope = buf.rope.clone();
        assert!(!buf.syntax.spans(&rope, 0..1).is_empty());
        buf.syntax.set_language(Language::PlainText);
        assert!(buf.syntax.spans(&rope, 0..1).is_empty());
    }

    /// Text typed at either edge of a kept span stays plain until the new tree; going over the
    /// size limit drops the kept spans (no tree will replace them).
    #[test]
    fn stale_span_edges_and_the_size_limit() {
        let mut buf = buffer(Language::C, "int x;\n");
        let rope = buf.rope.clone();
        let key = |s: &Span| (s.start.0, s.end.0, s.highlight.key());
        assert_eq!(key(&buf.syntax.spans(&rope, 0..1)[0]), (0, 3, "type.builtin"));
        edit(&mut buf, 3, 3, "eger");
        edit(&mut buf, 0, 0, "u");
        assert!(buf.syntax.tree().is_none());
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 0..1).to_vec();
        assert_eq!(key(&spans[0]), (1, 4, "type.builtin"), "only `int` of `uinteger`");
        assert_eq!(key(&spans[1]), (9, 10, "variable"));
        // Deleting a span's text drops it.
        edit(&mut buf, 9, 10, "");
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 0..1).to_vec();
        assert_eq!(spans.len(), 1, "{spans:?}");
        check_sorted(&spans);

        assert!(buf.syntax.update(&rope, 4).is_none());
        assert!(buf.syntax.too_large());
        assert!(buf.syntax.spans(&rope, 0..1).is_empty());
    }

    /// A very long line is coloured for its first `MAX_QUERY_BYTES` only, and that is cached;
    /// the lines after it are coloured as usual.
    #[test]
    fn long_lines_are_coloured_only_so_far() {
        let text = format!("{}\nint x;\n", "int a; ".repeat(20_000));
        let mut buf = buffer(Language::C, &text);
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 0..1).to_vec();
        check_sorted(&spans);
        assert_eq!(spans[0].highlight.key(), "type.builtin");
        let end = spans.last().unwrap().end.0;
        assert!((MAX_QUERY_BYTES - 8..=MAX_QUERY_BYTES).contains(&end), "{end}");
        buf.syntax.spans(&rope, 0..1);
        assert_eq!(buf.syntax.highlight_queries_run(), 1);
        let line = pos::line_start(&rope, 1);
        let spans = buf.syntax.spans(&rope, 1..2).to_vec();
        assert_eq!((spans[0].start, spans[0].highlight.key()), (line, "type.builtin"));
        assert_eq!(spans.last().unwrap().end.0, line.0 + 5, "`x`");
    }
}
