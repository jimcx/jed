//! Bracket matching (VS Code's `editor.matchBrackets`): the bracket next to a cursor and its
//! partner, drawn highlighted; and the unclosed opening bracket before a position (auto-indent's
//! outdent when a closing bracket is typed).
//!
//! The pairs are the language's bracket pairs (`LanguageConfig::brackets`: `{}` `[]` `()`).
//! Like VS Code, only brackets of the same kind count when matching (`(]` doesn't close).
//! - With a current parse tree a bracket counts only when it is a token of its own: brackets in
//!   strings and comments are ignored. Its partner is found among its siblings in the tree (an
//!   opening and closing bracket of one pair share a parent node), however far away; a pair the
//!   parser completed with a zero-width "missing" bracket doesn't match.
//! - Without one (plain text, a file over the size limit, a re-parse running) the text is
//!   scanned, every bracket counting, at most `WINDOW` bytes each way.
//!
//! Drawing asks `Matches::get` every frame; it recomputes only when the text, the tree's
//! availability or the cursors on screen change.

use ropey::Rope;
use tree_sitter::Tree;

use crate::buffer::pos::{self, ByteIdx, Peek};
use crate::syntax::{Language, Syntax, Tokens};

/// How far a scan without a tree (and the outdent's opener search) looks, each way.
pub const WINDOW: usize = 16 << 10;
/// Only the first this many cursors on screen get their brackets highlighted.
pub const MAX_CURSORS: usize = 64;

/// The bracket pair `c` belongs to in `language`: (open, close, whether `c` opens).
pub fn bracket(language: Language, c: char) -> Option<(char, char, bool)> {
    language
        .config()
        .brackets()
        .find(|&(o, cl)| c == o || c == cl)
        .map(|(o, cl)| (o, cl, c == o))
}

/// The bracket at `pos` and its partner: the bracket after the cursor first, else the one
/// before it (VS Code: the bracket right of the cursor wins when both touch it). `None` when
/// neither is a bracket with a partner.
pub fn match_at(rope: &Rope, syntax: &Syntax, pos: ByteIdx) -> Option<(ByteIdx, ByteIdx)> {
    let language = syntax.language();
    let tree = syntax.tree();
    let mut peek = Peek::new(rope);
    let candidates = [(peek.after(pos), pos.0), (peek.before(pos), pos.0.wrapping_sub(1))];
    for (c, at) in candidates {
        let Some((open, close, opens)) = c.and_then(|c| bracket(language, c)) else {
            continue;
        };
        let partner = match tree {
            Some(tree) => partner_in_tree(tree, at, open, close, opens),
            None => partner_in_text(rope, at, open as u8, close as u8, opens),
        };
        if let Some(p) = partner {
            return Some((ByteIdx(at), ByteIdx(p)));
        }
    }
    None
}

fn is(kind: &str, c: char) -> bool {
    let mut chars = kind.chars();
    chars.next() == Some(c) && chars.next().is_none()
}

/// The partner of the bracket token at `at` among its siblings, or `None` if `at` isn't a
/// bracket token (inside a string or comment) or has no real partner.
fn partner_in_tree(tree: &Tree, at: usize, open: char, close: char, opens: bool) -> Option<usize> {
    let mut cursor = tree.walk();
    while cursor.goto_first_child_for_byte(at).is_some() {}
    let me = cursor.node();
    let c = if opens { open } else { close };
    if me.start_byte() != at || me.child_count() != 0 || !is(me.kind(), c) {
        return None;
    }
    if !cursor.goto_parent() {
        return None;
    }
    cursor.goto_first_child();
    // Opening: count forward from it. Closing: openers seen before it on a stack (`None` for
    // a missing one).
    let (mut depth, mut seen) = (0usize, false);
    let mut stack: Vec<Option<usize>> = Vec::new();
    loop {
        let n = cursor.node();
        if opens {
            if seen {
                if is(n.kind(), open) {
                    depth += 1;
                } else if is(n.kind(), close) {
                    depth -= 1;
                    if depth == 0 {
                        return (!n.is_missing()).then(|| n.start_byte());
                    }
                }
            } else if n == me {
                (seen, depth) = (true, 1);
            }
        } else if n == me {
            return stack.pop().flatten();
        } else if is(n.kind(), open) {
            stack.push((!n.is_missing()).then(|| n.start_byte()));
        } else if is(n.kind(), close) {
            stack.pop();
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// The partner of the bracket at `at` by counting brackets in the text, within `WINDOW`.
fn partner_in_text(rope: &Rope, at: usize, open: u8, close: u8, opens: bool) -> Option<usize> {
    let mut depth = 0usize;
    if opens {
        let mut offset = at + 1;
        for chunk in pos::bytes_in(rope, at + 1, at + 1 + WINDOW) {
            for (i, &b) in chunk.iter().enumerate() {
                if b == open {
                    depth += 1;
                } else if b == close {
                    if depth == 0 {
                        return Some(offset + i);
                    }
                    depth -= 1;
                }
            }
            offset += chunk.len();
        }
    } else {
        let mut end = at;
        for chunk in pos::bytes_in_rev(rope, at.saturating_sub(WINDOW), at) {
            let start = end - chunk.len();
            for (i, &b) in chunk.iter().enumerate().rev() {
                if b == close {
                    depth += 1;
                } else if b == open {
                    if depth == 0 {
                        return Some(start + i);
                    }
                    depth -= 1;
                }
            }
            end = start;
        }
    }
    None
}

/// The innermost unclosed `open` bracket before positions asked in increasing order: one pass
/// over the bracket tokens (the tree's, else the text's) with a stack, starting again
/// `WINDOW` before a position that is further than that from the last one.
pub struct Openers<'a> {
    rope: &'a Rope,
    tokens: Option<Tokens<'a>>,
    open: char,
    close: char,
    stack: Vec<usize>,
    /// Bytes before this are on the stack; `None` before the first position.
    scanned: Option<usize>,
}

impl<'a> Openers<'a> {
    pub fn new(rope: &'a Rope, syntax: &'a Syntax, open: char, close: char) -> Self {
        Self {
            rope,
            tokens: syntax.tokens(),
            open,
            close,
            stack: Vec::new(),
            scanned: None,
        }
    }

    pub fn before(&mut self, pos: ByteIdx) -> Option<ByteIdx> {
        let pos = pos.0;
        let from = match self.scanned {
            Some(s) if s <= pos && pos - s <= WINDOW => s,
            _ => {
                self.stack.clear();
                pos.saturating_sub(WINDOW)
            }
        };
        let (open, close, stack) = (self.open, self.close, &mut self.stack);
        match &mut self.tokens {
            Some(tokens) => tokens.each_leaf(from, pos, |at, kind| {
                if is(kind, open) {
                    stack.push(at);
                } else if is(kind, close) {
                    stack.pop();
                }
            }),
            None => {
                let mut offset = from;
                for chunk in pos::bytes_in(self.rope, from, pos) {
                    for (i, &b) in chunk.iter().enumerate() {
                        if b == open as u8 {
                            stack.push(offset + i);
                        } else if b == close as u8 {
                            stack.pop();
                        }
                    }
                    offset += chunk.len();
                }
            }
        }
        self.scanned = Some(pos);
        self.stack.last().map(|&p| ByteIdx(p))
    }
}

/// A view's highlighted brackets, kept between frames.
#[derive(Debug, Clone, Default)]
pub struct Matches {
    key: Option<Key>,
    marks: Vec<ByteIdx>,
}

#[derive(Debug, Clone, PartialEq)]
struct Key {
    revision: u64,
    language: Language,
    tree: bool,
    heads: Vec<ByteIdx>,
}

impl Matches {
    /// The brackets to highlight (sorted) for the cursors at `heads` (sorted; at most
    /// `MAX_CURSORS` are looked at): both brackets of each matched pair. Recomputed only when the
    /// buffer's revision, language, tree availability or the heads changed.
    pub fn get(&mut self, rope: &Rope, syntax: &Syntax, revision: u64, heads: &[ByteIdx]) -> &[ByteIdx] {
        let heads = &heads[..heads.len().min(MAX_CURSORS)];
        let (language, tree) = (syntax.language(), syntax.tree().is_some());
        let fresh = self
            .key
            .as_ref()
            .is_some_and(|k| k.revision == revision && k.language == language && k.tree == tree && k.heads == heads);
        if !fresh {
            self.marks.clear();
            for &h in heads {
                if let Some((a, b)) = match_at(rope, syntax, h) {
                    self.marks.extend([a, b]);
                }
            }
            self.marks.sort_unstable();
            self.marks.dedup();
            self.key = Some(Key {
                revision,
                language,
                tree,
                heads: heads.to_vec(),
            });
        }
        &self.marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::pairs::tests::reparse;

    /// `text` with `|` marking the cursor; returns the matched pair, if any.
    fn matched(language: Language, marked: &str, parse: bool) -> Option<(usize, usize)> {
        let at = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let mut buf = Buffer::from_str(&text);
        buf.syntax.set_language(language);
        if parse {
            reparse(&mut buf);
        }
        match_at(&buf.rope, &buf.syntax, ByteIdx(at)).map(|(a, b)| (a.0, b.0))
    }

    #[test]
    fn nesting_and_cursor_side() {
        for parse in [true, false] {
            let cpp = Language::Cpp;
            // Before an opening bracket, after a closing one.
            assert_eq!(matched(cpp, "int f() |{ g(h(1)); }", parse), Some((8, 19)));
            assert_eq!(matched(cpp, "int f() { g(h(1)); }|", parse), Some((19, 8)));
            // Inner pairs.
            assert_eq!(matched(cpp, "int f() { g|(h(1)); }", parse), Some((11, 16)));
            assert_eq!(matched(cpp, "int f() { g(h(1|)); }", parse), Some((15, 13)));
            // Between two brackets: the one after the cursor wins.
            assert_eq!(matched(cpp, "int f() { g(h(1))|; }", parse), Some((16, 11)));
            assert_eq!(matched(cpp, "int f() { g(h(1)|); }", parse), Some((16, 11)));
            assert_eq!(matched(cpp, "int f() { g(|h(1)); }", parse), Some((11, 16)));
            // Not next to a bracket.
            assert_eq!(matched(cpp, "int f() { g(h(1)); | }", parse), None);
        }
    }

    #[test]
    fn strings_and_comments_dont_count_with_a_tree() {
        let cpp = Language::Cpp;
        // With the tree the bracket in the string is skipped; without it, counted.
        assert_eq!(matched(cpp, "f|(\"(\", 1) // )\n", true), Some((1, 8)));
        assert_eq!(matched(cpp, "f|(\"(\", 1) // )\n", false), Some((1, 13)));
        // A bracket in a string or comment has no match with the tree.
        assert_eq!(matched(cpp, "f(\"|(\", 1) // )\n", true), None);
        assert_eq!(matched(cpp, "f(\"(\", 1) // |)\n", true), None);
        let py = Language::Python;
        assert_eq!(matched(py, "x = |[1, \"]\", 2]  # ]\n", true), Some((4, 14)));
    }

    #[test]
    fn unmatched_brackets() {
        for parse in [true, false] {
            assert_eq!(matched(Language::Cpp, "int f() |{ g(1);\n", parse), None);
            assert_eq!(matched(Language::Cpp, "int x = 1;|)\n", parse), None);
        }
        // Different kinds don't match.
        assert_eq!(matched(Language::PlainText, "|(]", false), None);
        assert_eq!(matched(Language::PlainText, "a |[ (b) ]", false), Some((2, 8)));
    }

    #[test]
    fn text_scan_stops_at_the_window() {
        let far = format!("|({})", "x".repeat(WINDOW));
        assert_eq!(matched(Language::PlainText, &far, false), None);
        let near = format!("|({})", "x".repeat(WINDOW - 2));
        assert!(matched(Language::PlainText, &near, false).is_some());
        let back = format!("({})|", "x".repeat(WINDOW - 1));
        assert!(matched(Language::PlainText, &back, false).is_some());
        let back = format!("({})|", "x".repeat(WINDOW + 1));
        assert_eq!(matched(Language::PlainText, &back, false), None);
    }

    #[test]
    fn openers_before_positions() {
        let text = "void f() {\n  if (c) {\n  x;\n}\n  y;\n  s = \"}\";\n  z;\n}\n";
        let outer = text.find('{').unwrap();
        let inner = text.rfind(") {").unwrap() + 2;
        let at = |c: char| ByteIdx(text.find(c).unwrap());
        for parse in [true, false] {
            let mut buf = Buffer::from_str(text);
            buf.syntax.set_language(Language::Cpp);
            if parse {
                reparse(&mut buf);
            }
            let mut o = Openers::new(&buf.rope, &buf.syntax, '{', '}');
            assert_eq!(o.before(at('x')), Some(ByteIdx(inner)), "parse {parse}");
            assert_eq!(o.before(at('y')), Some(ByteIdx(outer)), "parse {parse}");
            // The `}` in the string closes the outer `{` only when the text is scanned.
            let z = o.before(at('z'));
            assert_eq!(z, if parse { Some(ByteIdx(outer)) } else { None }, "parse {parse}");
        }
    }

    #[test]
    fn cache_recomputes_on_change_only() {
        let buf = Buffer::from_str("(a) [b]");
        let mut m = Matches::default();
        assert_eq!(
            m.get(&buf.rope, &buf.syntax, 0, &[ByteIdx(0)]),
            &[ByteIdx(0), ByteIdx(2)]
        );
        assert_eq!(
            m.get(&buf.rope, &buf.syntax, 0, &[ByteIdx(0), ByteIdx(7)]),
            &[ByteIdx(0), ByteIdx(2), ByteIdx(4), ByteIdx(6)]
        );
        // Other text at the same revision and heads gets the cached answer: only a new revision
        // recomputes.
        let edited = Rope::from_str("(a  [b]");
        assert_eq!(m.get(&edited, &buf.syntax, 0, &[ByteIdx(0), ByteIdx(7)]).len(), 4);
        assert_eq!(m.get(&edited, &buf.syntax, 1, &[ByteIdx(0), ByteIdx(7)]).len(), 2);
    }

    #[test]
    fn cache_recomputes_when_the_tree_arrives() {
        // The text scan counts the `)` in the string; the tree, at the same revision, doesn't.
        let mut buf = Buffer::from_str("f(\")\", 1)");
        buf.syntax.set_language(Language::Cpp);
        let mut m = Matches::default();
        let heads = [ByteIdx(1)];
        assert_eq!(m.get(&buf.rope, &buf.syntax, 0, &heads), &[ByteIdx(1), ByteIdx(3)]);
        reparse(&mut buf);
        assert_eq!(m.get(&buf.rope, &buf.syntax, 0, &heads), &[ByteIdx(1), ByteIdx(8)]);
    }
}
