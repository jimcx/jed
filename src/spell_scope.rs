//! Which byte ranges of a buffer the spell checker looks at: pure, no app state. `-c` calls
//! [`check_ranges`] for a full scan and [`check_ranges_in`] for the lines an edit touched, and
//! passes the result to `Checker::check`.
//!
//! The setting is [`SpellScope`] (`spell_check_scope`: `all` / `comments_and_strings` / `off`);
//! [`enabled`] adds the other switches (a language in `spell_check_off_languages`, a file over
//! `syntax_max_bytes`).
//!
//! **With a parse tree** the walk finds the nodes `Language::scope_of` calls `Comment` or `String`
//! (the whole node, minus the parts that are `Code` again: interpolations) and takes their text
//! minus escapes and format fields: the highlight query's `escape` captures, plus a textual pass
//! over each string for `\n` `\x41` `\u{1F600}` `\101`, `%s` `%-5.2f` and `{name}` / `{0:>5}`
//! (grammars differ in what they capture as an escape, so neither alone is enough).
//! **Identifiers** (scope `all` only) are not found by a second list of node kinds per language:
//! the language's own highlight query is run over the range ([`syntax::highlight_spans`]) and the
//! captures that mean a name are taken: `variable`, `variable.parameter`, `function`, `type`,
//! `property`, `constructor`, `constant` (not `.builtin`), `label`, `tag` and `tag.attribute`
//! ([`IDENTIFIER_KEYS`]). Keywords, numbers, operators, punctuation, escapes and the builtin
//! variants (`self`, `i32`, `true`) are never taken. So a language gets identifier checking exactly
//! when its highlight query colours names, and a fix to that query fixes this.
//!
//! **No grammar** (plain text, a plugin language without an installed grammar): the whole range.
//! A language that has a grammar but no tree yet gives nothing (the parse is pending or over the
//! size limit). **Markdown** has only its block grammar (inline text is one leaf): prose is the
//! text outside fenced / indented code blocks, HTML blocks and front matter and outside inline
//! `` `code` `` spans (found textually) and link destinations `](url)`; the code is checked only
//! with scope `all`.
//!
//! The result is sorted and merged: ranges that overlap are joined, ranges that merely touch are
//! not (two names side by side stay two words).

use std::ops::Range;

use ropey::Rope;
use tree_sitter::{Node, Tree};

use crate::buffer::pos::{self, ByteIdx};
use crate::edit::SpellScope;
use crate::syntax::{self, KEYS, Language, Scope};

/// Highlight keys whose captures are names (checked with scope `all`).
pub const IDENTIFIER_KEYS: &[&str] = &[
    "constant",
    "constructor",
    "function",
    "label",
    "property",
    "tag",
    "tag.attribute",
    "type",
    "variable",
    "variable.parameter",
];

/// Whether a buffer in `language` of `len` bytes is spell-checked at all: the scope isn't `off`,
/// the language isn't in `off_languages` (ids, `Language::id`), and the file is within
/// `syntax_max_bytes` (as `Syntax::update` applies it: 0 means no parse tree, so a language with a
/// grammar is off; plain text has no tree to lack).
pub fn enabled(
    scope: SpellScope,
    language: Language,
    off_languages: &[String],
    len: usize,
    syntax_max_bytes: usize,
) -> bool {
    scope != SpellScope::Off
        && !off_languages.iter().any(|id| id == language.id())
        && if syntax_max_bytes == 0 {
            language.grammar().is_none()
        } else {
            len <= syntax_max_bytes
        }
}

/// The ranges of the whole text to check. `tree` must match `rope`.
#[cfg_attr(not(test), allow(dead_code))] // the app uses `check_ranges_in`
pub fn check_ranges(language: Language, tree: Option<&Tree>, rope: &Rope, scope: SpellScope) -> Vec<Range<ByteIdx>> {
    check_ranges_in(language, tree, rope, scope, ByteIdx(0)..ByteIdx(rope.len()))
}

/// [`check_ranges`] clipped to `within` (char boundaries, such as whole lines), walking only the
/// nodes that overlap it: O(range) plus the tree's depth, so a one-line re-check is cheap in a big
/// file. A comment or string that starts above `within` counts for the part inside it.
pub fn check_ranges_in(
    language: Language,
    tree: Option<&Tree>,
    rope: &Rope,
    scope: SpellScope,
    within: Range<ByteIdx>,
) -> Vec<Range<ByteIdx>> {
    let (start, end) = (within.start.0, within.end.0.min(rope.len()));
    if scope == SpellScope::Off || start >= end {
        return Vec::new();
    }
    let ranges = if language.grammar().is_none() {
        std::iter::once(start..end).collect()
    } else if let Some(tree) = tree {
        if language == Language::Markdown {
            markdown(tree, rope, scope, start, end)
        } else {
            code(language, tree, rope, scope, start, end)
        }
    } else {
        Vec::new()
    };
    merge(ranges)
}

/// A comment or string found by the walk, with the code inside it.
struct Text {
    range: Range<usize>,
    string: bool,
    holes: Vec<Range<usize>>,
}

fn code(
    language: Language,
    tree: &Tree,
    rope: &Rope,
    scope: SpellScope,
    start: usize,
    end: usize,
) -> Vec<Range<usize>> {
    let mut texts: Vec<Text> = Vec::new();
    // (node, the `texts` entry it is inside as text, not code)
    let mut stack: Vec<(Node, Option<usize>)> = vec![(tree.root_node(), None)];
    while let Some((node, inside)) = stack.pop() {
        let mut child_ctx = inside;
        match (language.scope_of(node.kind()), inside) {
            (Some(kind @ (Scope::Comment | Scope::String)), None) => {
                texts.push(Text {
                    range: node.start_byte().max(start)..node.end_byte().min(end),
                    string: kind == Scope::String,
                    holes: Vec::new(),
                });
                child_ctx = Some(texts.len() - 1);
            }
            (Some(Scope::Code), Some(i)) => {
                texts[i].holes.push(node.start_byte()..node.end_byte());
                child_ctx = None;
            }
            _ => {}
        }
        for child in children_in(node, start, end) {
            stack.push((child, child_ctx));
        }
    }
    let wants_spans = scope == SpellScope::All || texts.iter().any(|t| t.string);
    let spans = if wants_spans {
        syntax::highlight_spans(language, tree, rope, ByteIdx(start), ByteIdx(end))
    } else {
        Vec::new()
    };
    let escapes: Vec<Range<usize>> = spans
        .iter()
        .filter(|s| KEYS[s.highlight.index()] == "escape")
        .map(|s| s.start.0..s.end.0)
        .collect();
    let mut out = Vec::new();
    for mut text in texts {
        text.holes.sort_by_key(|h| h.start);
        let mut holes = text.holes;
        if text.string {
            holes.extend(
                escapes
                    .iter()
                    .filter(|e| e.start < text.range.end && e.end > text.range.start)
                    .cloned(),
            );
            if let Some(s) = text_of(rope, &text.range) {
                holes.extend(
                    string_noise(&s)
                        .into_iter()
                        .map(|r| r.start + text.range.start..r.end + text.range.start),
                );
            }
        }
        out.extend(subtract(text.range, holes));
    }
    if scope == SpellScope::All {
        out.extend(
            spans
                .iter()
                .filter(|s| IDENTIFIER_KEYS.contains(&KEYS[s.highlight.index()]))
                .map(|s| s.start.0..s.end.0),
        );
    }
    out
}

/// The children of `node` that overlap `start..end`, found by seeking rather than visiting all.
fn children_in<'t>(node: Node<'t>, start: usize, end: usize) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    if node.end_byte() <= start || node.start_byte() >= end || node.child_count() == 0 {
        return out;
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child_for_byte(start).is_none() {
        return out;
    }
    loop {
        let child = cursor.node();
        if child.start_byte() >= end {
            break;
        }
        if child.end_byte() > start {
            out.push(child);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    out
}

/// Block kinds of Markdown that are code (or markup), not prose.
const MARKDOWN_CODE: &[&str] = &[
    "fenced_code_block",
    "indented_code_block",
    "html_block",
    "minus_metadata",
    "plus_metadata",
];

fn markdown(tree: &Tree, rope: &Rope, scope: SpellScope, start: usize, end: usize) -> Vec<Range<usize>> {
    let mut blocks: Vec<Range<usize>> = Vec::new();
    // The fence lines (``` and the info string) are neither prose nor code to check.
    let mut fences: Vec<Range<usize>> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if MARKDOWN_CODE.contains(&node.kind()) {
            blocks.push(node.start_byte().max(start)..node.end_byte().min(end));
            let mut cursor = node.walk();
            fences.extend(
                node.children(&mut cursor)
                    .filter(|c| matches!(c.kind(), "fenced_code_block_delimiter" | "info_string"))
                    .map(|c| c.start_byte()..c.end_byte()),
            );
        } else {
            stack.extend(children_in(node, start, end));
        }
    }
    blocks.sort_by_key(|b| b.start);
    let prose = subtract(start..end, blocks.clone());
    let mut inline_code = Vec::new();
    let mut out = Vec::new();
    for piece in prose {
        let Some(text) = text_of(rope, &piece) else {
            out.push(piece);
            continue;
        };
        let (code, noise) = markdown_inline(&text);
        let at = piece.start;
        inline_code.extend(code.iter().map(|r| r.start + at..r.end + at));
        let holes = code.iter().chain(&noise).map(|r| r.start + at..r.end + at).collect();
        out.extend(subtract(piece, holes));
    }
    if scope == SpellScope::All {
        fences.sort_by_key(|f| f.start);
        for block in blocks {
            out.extend(subtract(block, fences.clone()));
        }
        out.extend(inline_code);
    }
    out
}

/// In a run of prose: the inline code spans' contents, and the link destinations in `](...)`.
fn markdown_inline(text: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let b = text.as_bytes();
    let (mut code, mut noise) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'`' => {
                let run = b[i..].iter().take_while(|c| **c == b'`').count();
                let open_end = i + run;
                // The closing run has the same length, before a blank line ends the paragraph.
                let mut j = open_end;
                let mut found = None;
                while j < b.len() {
                    if b[j] == b'`' {
                        let n = b[j..].iter().take_while(|c| **c == b'`').count();
                        if n == run {
                            found = Some(j);
                            break;
                        }
                        j += n;
                    } else if text[j..].starts_with("\n\n") {
                        break;
                    } else {
                        j += 1;
                    }
                }
                match found {
                    Some(close) => {
                        code.push(open_end..close);
                        i = close + run;
                    }
                    None => i = open_end,
                }
            }
            b']' if b.get(i + 1) == Some(&b'(') => {
                let from = i + 2;
                let len = b[from..]
                    .iter()
                    .position(|c| *c == b')' || c.is_ascii_whitespace())
                    .filter(|n| b[from + n] == b')');
                match len {
                    Some(n) => {
                        noise.push(from..from + n);
                        i = from + n;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    (code, noise)
}

/// `range` without `holes` (any order, may overlap), as sorted pieces.
fn subtract(range: Range<usize>, mut holes: Vec<Range<usize>>) -> Vec<Range<usize>> {
    holes.sort_by_key(|h| h.start);
    let mut out = Vec::new();
    let mut at = range.start;
    for h in holes {
        if h.end <= at {
            continue;
        }
        if h.start >= range.end {
            break;
        }
        if h.start > at {
            out.push(at..h.start);
        }
        at = at.max(h.end);
    }
    if at < range.end {
        out.push(at..range.end);
    }
    out
}

/// Sorted by start, overlapping ranges joined, empty ones dropped.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<ByteIdx>> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start < last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out.into_iter().map(|r| ByteIdx(r.start)..ByteIdx(r.end)).collect()
}

/// The text of `range` (char boundaries), `None` if it isn't valid UTF-8 (a cut char).
fn text_of(rope: &Rope, range: &Range<usize>) -> Option<String> {
    let mut bytes = Vec::with_capacity(range.len());
    for chunk in pos::bytes_in(rope, range.start, range.end) {
        bytes.extend_from_slice(chunk);
    }
    String::from_utf8(bytes).ok()
}

/// What in a string literal is not a word: escapes (`\n`, `\x41`, `\u{1F600}`, `\101`), printf
/// conversions (`%s`, `%-5.2f`) and format fields (`{name}`, `{0:>5}`). Byte ranges of `s`.
fn string_noise(s: &str) -> Vec<Range<usize>> {
    let b = s.as_bytes();
    let hex = |from: usize, max: usize| {
        b[from.min(b.len())..]
            .iter()
            .take(max)
            .take_while(|c| c.is_ascii_hexdigit())
            .count()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                let Some(next) = s[i + 1..].chars().next() else {
                    out.push(i..i + 1);
                    break;
                };
                let after = i + 1 + next.len_utf8();
                let end = match next {
                    'x' => after + hex(after, 2),
                    'u' if b.get(after) == Some(&b'{') => match s[after..].find('}') {
                        Some(n) if n <= 9 => after + n + 1,
                        _ => after,
                    },
                    'u' => after + hex(after, 4),
                    'U' => after + hex(after, 8),
                    '0'..='7' => {
                        after
                            + b[after..]
                                .iter()
                                .take(2)
                                .take_while(|c| (b'0'..=b'7').contains(c))
                                .count()
                    }
                    _ => after,
                };
                out.push(i..end);
                i = end;
            }
            b'%' => {
                let mut j = i + 1;
                if b.get(j) == Some(&b'%') {
                    out.push(i..j + 1);
                    i = j + 1;
                    continue;
                }
                while j < b.len() && b"-+#0123456789.*lhzjtqL".contains(&b[j]) {
                    j += 1;
                }
                if j < b.len() && b"diouxXeEfFgGaAcspnSC".contains(&b[j]) {
                    out.push(i..j + 1);
                    i = j + 1;
                } else {
                    i += 1;
                }
            }
            b'{' => {
                let close = b[i + 1..]
                    .iter()
                    .take(40)
                    .position(|c| matches!(c, b'}' | b'{' | b'"' | b'\'' | b'\n') || c.is_ascii_whitespace());
                match close {
                    Some(n) if b[i + 1 + n] == b'}' => {
                        out.push(i..i + n + 2);
                        i += n + 2;
                    }
                    _ => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::syntax::DEFAULT_MAX_BYTES;

    struct Parsed {
        buf: Buffer,
        language: Language,
    }

    fn parsed(language: Language, text: &str) -> Parsed {
        let mut buf = Buffer::from_str(text);
        buf.syntax.set_language(language);
        while let Some(job) = buf.syntax.update(&buf.rope, DEFAULT_MAX_BYTES) {
            buf.syntax.parsed(job.run());
        }
        Parsed { buf, language }
    }

    impl Parsed {
        /// The text of each range to check.
        fn ranges(&self, scope: SpellScope) -> Vec<String> {
            let ranges = check_ranges(self.language, self.buf.syntax.tree(), &self.buf.rope, scope);
            let all = self.buf.rope.to_string();
            for w in ranges.windows(2) {
                assert!(w[0].end <= w[1].start, "sorted and non-overlapping: {ranges:?}");
            }
            ranges.iter().map(|r| all[r.start.0..r.end.0].to_string()).collect()
        }

        /// Whether `needle` is inside some range (all of it).
        fn covers(&self, scope: SpellScope, needle: &str) -> bool {
            let text = self.buf.rope.to_string();
            let ranges = check_ranges(self.language, self.buf.syntax.tree(), &self.buf.rope, scope);
            let mut from = 0;
            let mut found = false;
            while let Some(at) = text[from..].find(needle) {
                let (s, e) = (from + at, from + at + needle.len());
                found |= ranges.iter().any(|r| r.start.0 <= s && e <= r.end.0);
                from = s + 1;
            }
            assert!(text.contains(needle), "{needle} not in the text");
            found
        }
    }

    const ALL: SpellScope = SpellScope::All;
    const CS: SpellScope = SpellScope::CommentsAndStrings;

    #[test]
    fn spell_check_scope_rust_comment_string_identifier() {
        let src = "// a commnet here\nfn recieve_it(argumnt: u32) -> u32 {\n    let strng = \"a sentance\\n and %s {name}\";\n    let n = 12345;\n    argumnt + n\n}\n";
        let p = parsed(Language::Rust, src);
        assert!(p.covers(ALL, "commnet"));
        assert!(p.covers(ALL, "sentance"));
        assert!(p.covers(ALL, "recieve_it"));
        assert!(p.covers(ALL, "argumnt"));
        assert!(p.covers(ALL, "strng"));
        for no in ["fn ", "let", "12345", "\\n", "%s", "{name}"] {
            assert!(
                !p.ranges(ALL).iter().any(|r| r.contains(no)),
                "{no:?} must not be checked: {:?}",
                p.ranges(ALL)
            );
        }
        // The text after the escape is checked.
        assert!(p.covers(ALL, " and "));
    }

    #[test]
    fn spell_check_scope_comments_and_strings_skips_identifiers() {
        let src = "// a commnet\nfn recieve_it(argumnt: u32) {\n    let s = \"sentance\";\n}\n";
        let p = parsed(Language::Rust, src);
        assert!(p.covers(CS, "commnet"));
        assert!(p.covers(CS, "sentance"));
        assert!(!p.covers(CS, "recieve_it"));
        assert!(!p.covers(CS, "argumnt"));
        assert!(p.covers(ALL, "argumnt"));
    }

    #[test]
    fn spell_check_scope_off_and_enabled() {
        let p = parsed(Language::Rust, "// x commnet\n");
        assert!(p.ranges(SpellScope::Off).is_empty());
        let off = vec!["log".to_string()];
        assert!(enabled(ALL, Language::Rust, &off, 100, DEFAULT_MAX_BYTES));
        assert!(!enabled(SpellScope::Off, Language::Rust, &off, 100, DEFAULT_MAX_BYTES));
        assert!(!enabled(
            ALL,
            Language::Rust,
            &["rust".to_string()],
            100,
            DEFAULT_MAX_BYTES
        ));
        assert!(!enabled(ALL, Language::Rust, &off, 101, 100), "over syntax_max_bytes");
        assert!(enabled(ALL, Language::Rust, &off, 100, 100));
        assert!(!enabled(ALL, Language::Rust, &off, 1, 0), "0: no parse tree");
        assert!(enabled(ALL, Language::PlainText, &off, 1, 0), "plain text needs none");
    }

    #[test]
    fn spell_check_scope_python() {
        let src = "# a commnet\ndef recieve(valeu):\n    \"\"\"docstrng here\"\"\"\n    name = f\"hello {valeu} wrold\\t!\"\n    return 42 if valeu else None\n";
        let p = parsed(Language::Python, src);
        assert!(p.covers(ALL, "commnet"));
        assert!(p.covers(ALL, "docstrng"));
        assert!(p.covers(ALL, "wrold"));
        assert!(p.covers(ALL, "recieve"));
        assert!(p.covers(ALL, "valeu"));
        assert!(!p.covers(CS, "recieve"));
        for no in ["def ", "return", "42", "None", "\\t"] {
            assert!(
                !p.ranges(ALL).iter().any(|r| r.contains(no)),
                "{no:?}: {:?}",
                p.ranges(ALL)
            );
        }
        // The interpolation is code: only checked as an identifier, so not in comments_and_strings.
        let s = p.ranges(CS).join("|");
        assert!(!s.contains("valeu"), "{s}");
    }

    #[test]
    fn spell_check_scope_interpolation_is_code_not_string() {
        // `{valeu + 1}` has a space, so the textual format-field pass doesn't hide it: only the
        // Code-hole exclusion keeps it out of the string's ranges.
        let src = "s = f\"hello {valeu + 1} wrold\"\n";
        let p = parsed(Language::Python, src);
        assert_eq!(p.ranges(CS), ["f\"hello ", " wrold\""]);
        assert!(p.covers(ALL, "valeu"), "an identifier in `all`");
        // The shell's `$(…)` is the same for a second grammar.
        let sh = parsed(Language::ShellScript, "echo \"hello $(recieve now) wrold\"\n");
        let r = sh.ranges(CS);
        assert!(r.iter().any(|t| t.contains("wrold")), "{r:?}");
        assert!(!r.iter().any(|t| t.contains("recieve")), "{r:?}");
    }

    #[test]
    fn spell_check_scope_cpp() {
        let src = "// a commnet\nint recieve(int argumnt) {\n  const char *s = \"a sentance\\n\";\n  return argumnt * 2;\n}\n#include <stdio.h>\n";
        let p = parsed(Language::Cpp, src);
        assert!(p.covers(ALL, "commnet"));
        assert!(p.covers(ALL, "sentance"));
        assert!(p.covers(ALL, "recieve"));
        assert!(p.covers(ALL, "argumnt"));
        assert!(!p.covers(CS, "recieve"));
        for no in ["int ", "const", "return", "\\n", "* 2"] {
            assert!(
                !p.ranges(ALL).iter().any(|r| r.contains(no)),
                "{no:?}: {:?}",
                p.ranges(ALL)
            );
        }
    }

    #[test]
    fn spell_check_scope_string_noise() {
        let n = |s: &str| {
            string_noise(s)
                .into_iter()
                .map(|r| s[r].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            n(&format!(r"a\nb\x41c\u{{1F600}}d\{}00e9e\101f\\g\", 'u')),
            [r"\n", r"\x41", r"\u{1F600}", r"\u00e9", r"\101", r"\\", r"\"]
        );
        assert_eq!(n("%s %-5.2f %d%% 100% sure %lu"), ["%s", "%-5.2f", "%d", "%%", "%lu"]);
        assert_eq!(n("{name} {0:>5} {} { not } {a b}"), ["{name}", "{0:>5}", "{}"]);
        assert!(n("plain words only").is_empty());
        assert_eq!(n("é\\né"), ["\\n"]);
    }

    #[test]
    fn spell_check_scope_plain_text_is_whole() {
        let text = "Some plain text\nwith  words.\n";
        let p = parsed(Language::PlainText, text);
        assert_eq!(p.ranges(CS), [text]);
        assert_eq!(p.ranges(ALL), [text]);
        let ranges = check_ranges_in(Language::PlainText, None, &p.buf.rope, ALL, ByteIdx(5)..ByteIdx(9));
        assert_eq!(ranges, [ByteIdx(5)..ByteIdx(9)]);
        assert!(check_ranges_in(Language::PlainText, None, &p.buf.rope, ALL, ByteIdx(5)..ByteIdx(5)).is_empty());
    }

    #[test]
    fn spell_check_scope_markdown_prose_and_code() {
        let src = "# Titel here\n\nSome prose with `codeSpan` and [lnk](http://x.example/pth) text.\n\n```rust\nlet fenced_idnt = 1;\n```\n\n    indented_cod\n\nMore prose.\n";
        let p = parsed(Language::Markdown, src);
        assert!(p.covers(CS, "Titel"));
        assert!(p.covers(CS, "prose with"));
        assert!(p.covers(CS, "lnk"));
        assert!(p.covers(CS, "More prose."));
        for code in ["codeSpan", "fenced_idnt", "indented_cod", "pth", "rust"] {
            assert!(
                !p.covers(CS, code),
                "{code} is code in comments_and_strings: {:?}",
                p.ranges(CS)
            );
        }
        assert!(p.covers(ALL, "codeSpan"));
        assert!(p.covers(ALL, "fenced_idnt"));
        assert!(p.covers(ALL, "indented_cod"));
        assert!(p.covers(ALL, "Titel"));
        assert!(!p.covers(ALL, "```"), "fence lines are not checked");
        assert!(!p.covers(ALL, "pth"), "link destinations are not checked");
    }

    #[test]
    fn spell_check_scope_incremental_range_matches_the_full_scan() {
        let mut src = String::new();
        for i in 0..200 {
            src.push_str(&format!(
                "// commnet {i}\nfn func_{i}(arg: u32) {{ let s = \"strng {i}\"; }}\n"
            ));
        }
        let p = parsed(Language::Rust, &src);
        let rope = &p.buf.rope;
        let tree = p.buf.syntax.tree();
        let starts: Vec<usize> = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        // Lines 100..110, and a single line.
        for scope in [ALL, CS] {
            let full = check_ranges(Language::Rust, tree, rope, scope);
            for (a, b) in [(starts[100], starts[110]), (starts[51], starts[52])] {
                let part = check_ranges_in(Language::Rust, tree, rope, scope, ByteIdx(a)..ByteIdx(b));
                let expected: Vec<_> = full
                    .iter()
                    .filter(|r| r.start.0 >= a && r.end.0 <= b)
                    .cloned()
                    .collect();
                assert!(!part.is_empty());
                assert_eq!(part, expected, "{scope:?} {a}..{b}");
                assert!(part.iter().all(|r| r.start.0 >= a && r.end.0 <= b));
            }
        }
    }

    #[test]
    fn spell_check_scope_block_comment_clipped_to_range() {
        let src = "/* first line\n   second commnet\n   third */\nfn x() {}\n";
        let p = parsed(Language::Rust, src);
        let a = src.find("   second").unwrap();
        let b = src.find("   third").unwrap();
        let r = check_ranges_in(
            Language::Rust,
            p.buf.syntax.tree(),
            &p.buf.rope,
            CS,
            ByteIdx(a)..ByteIdx(b),
        );
        assert_eq!(r, [ByteIdx(a)..ByteIdx(b)]);
    }

    #[test]
    fn spell_check_scope_grammar_without_tree_gives_nothing() {
        let rope = Rope::from_str("// comment\n");
        assert!(check_ranges(Language::Rust, None, &rope, ALL).is_empty());
    }

    #[test]
    fn spell_check_scope_subtract_and_merge() {
        assert_eq!(subtract(0..10, vec![5..7, 1..3, 2..4]), [0..1, 4..5, 7..10]);
        assert_eq!(
            subtract(2..4, std::iter::once(0..10).collect()),
            Vec::<Range<usize>>::new()
        );
        let m = merge(vec![5..8, 0..3, 2..4, 8..9, 7..7]);
        assert_eq!(
            m,
            [ByteIdx(0)..ByteIdx(4), ByteIdx(5)..ByteIdx(8), ByteIdx(8)..ByteIdx(9)]
        );
    }
}
