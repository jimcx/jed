//! Walking a parse tree's tokens (its leaves) in text order, for checks at many increasing
//! positions: auto-indent and bracket matching ask whether a bracket is a real token (not inside a
//! string or comment, which are single leaves or hold their text in content leaves).

use tree_sitter::{Node, Tree, TreeCursor};

/// A forward walk over a tree's leaves. Positions asked for should not decrease (a sorted
/// selection list): each node is then stepped over at most once, so many lookups cost one pass
/// over the nodes between them instead of a descent from the root each (a root with 60k children
/// makes a descent slow). Asking for an earlier position starts again from the root.
pub struct Tokens<'t> {
    tree: &'t Tree,
    cursor: TreeCursor<'t>,
    /// The last position asked for.
    last: usize,
    /// Every leaf has been passed.
    done: bool,
    /// The innermost node holding the last position that the walk climbed back into (the
    /// position is in a gap between its children); for `node_at`.
    gap: Option<Node<'t>>,
}

impl<'t> Tokens<'t> {
    pub fn new(tree: &'t Tree) -> Self {
        Self {
            tree,
            cursor: tree.walk(),
            last: 0,
            done: false,
            gap: None,
        }
    }

    /// The first leaf ending after byte `b` (holding `b`, or the next one after a gap); `None`
    /// past the last. Zero-width (missing) leaves are skipped.
    pub fn leaf_from(&mut self, b: usize) -> Option<Node<'t>> {
        if b < self.last {
            self.cursor.reset(self.tree.root_node());
            self.done = false;
        }
        self.last = b;
        self.gap = None;
        if self.done {
            return None;
        }
        // `forward`: this node's leaves are all behind; move to the next sibling or climb.
        let mut forward = false;
        loop {
            let n = self.cursor.node();
            if forward || n.end_byte() <= b || n.end_byte() == n.start_byte() {
                if self.cursor.goto_next_sibling() {
                    forward = false;
                    continue;
                }
                if !self.cursor.goto_parent() {
                    self.done = true;
                    return None;
                }
                let p = self.cursor.node();
                if self.gap.is_none() && p.start_byte() <= b && b < p.end_byte() {
                    self.gap = Some(p);
                }
                forward = true;
                continue;
            }
            if n.child_count() == 0 {
                return Some(n);
            }
            self.cursor.goto_first_child();
        }
    }

    /// The leaf holding byte `b`, `None` if `b` is between tokens (whitespace).
    pub fn leaf_at(&mut self, b: usize) -> Option<Node<'t>> {
        self.leaf_from(b).filter(|n| n.start_byte() <= b)
    }

    /// The leaf holding byte `b`, else the innermost node holding it in a gap between its
    /// children: the text of a comment or string whose node has only its delimiters as children
    /// (Rust's `line_comment` / `block_comment`), or whitespace in a block. Same order rules as
    /// `leaf_from`: the walk notes a node it climbs back into around `b`, else the next leaf's
    /// ancestors are climbed; only after the last leaf is passed does it descend from the root.
    pub fn node_at(&mut self, b: usize) -> Option<Node<'t>> {
        let leaf = self.leaf_from(b);
        if let Some(leaf) = leaf.filter(|n| n.start_byte() <= b) {
            return Some(leaf);
        }
        if let Some(gap) = self.gap {
            return Some(gap);
        }
        if leaf.is_none() {
            return self.tree.root_node().descendant_for_byte_range(b, b + 1);
        }
        // The cursor is on the next leaf: its innermost ancestor starting by `b`.
        let mut up = self.cursor.clone();
        while up.goto_parent() {
            if up.node().start_byte() <= b {
                return Some(up.node());
            }
        }
        None
    }

    /// Whether the char at byte `b` is a token of its own spelled `text` (a bracket that isn't in
    /// a string or comment).
    pub fn is_token(&mut self, b: usize, text: &str) -> bool {
        self.leaf_at(b)
            .is_some_and(|n| n.start_byte() == b && n.end_byte() == b + text.len() && n.kind() == text)
    }

    /// Calls `f(start, kind)` for each leaf starting in `lo..hi`, in order.
    pub fn each_leaf(&mut self, lo: usize, hi: usize, mut f: impl FnMut(usize, &str)) {
        let mut at = lo;
        while let Some(n) = self.leaf_from(at) {
            if n.start_byte() >= hi {
                break;
            }
            if n.start_byte() >= lo {
                f(n.start_byte(), n.kind());
            }
            at = n.end_byte();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    fn parse(language: Language, text: &str) -> Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&language.grammar().unwrap()).unwrap();
        p.parse(text, None).unwrap()
    }

    #[test]
    fn leaves_in_order_skip_strings_and_comments() {
        let text = "int f() { g(\"{\"); } // }\n";
        let tree = parse(Language::Cpp, text);
        let mut t = Tokens::new(&tree);
        let mut brackets = Vec::new();
        t.each_leaf(0, text.len(), |at, kind| {
            if matches!(kind, "{" | "}" | "(" | ")") {
                brackets.push((at, kind.to_string()));
            }
        });
        let want: Vec<(usize, String)> = [(5, "("), (6, ")"), (8, "{"), (11, "("), (15, ")"), (18, "}")]
            .into_iter()
            .map(|(a, k)| (a, k.to_string()))
            .collect();
        assert_eq!(brackets, want);
    }

    #[test]
    fn leaf_at_increasing_and_restarting() {
        let text = "x = (1, \"(\")  # (\n";
        let tree = parse(Language::Python, text);
        let mut t = Tokens::new(&tree);
        assert!(t.is_token(4, "("));
        assert!(t.leaf_at(6).is_some_and(|n| n.kind() == ","));
        assert_eq!(t.leaf_at(7), None, "a space between tokens");
        assert!(!t.is_token(9, "("), "in a string");
        assert!(!t.is_token(16, "("), "in a comment");
        // Going back starts over.
        assert!(t.is_token(4, "("));
        assert_eq!(t.leaf_from(text.len()), None);
        assert_eq!(t.leaf_from(text.len() + 5), None);
    }

    /// Rust's comments hold their delimiters as children and their text in no leaf: `node_at`
    /// gives the comment there, and the enclosing node in whitespace.
    #[test]
    fn node_at_in_gaps() {
        let text = "fn f() {\n    /* a */\n    x; // b\n}";
        let tree = parse(Language::Rust, text);
        let mut t = Tokens::new(&tree);
        let at = |s: &str| text.find(s).unwrap();
        assert_eq!(t.leaf_at(at("a */")), None);
        assert_eq!(t.node_at(at("a */")).unwrap().kind(), "block_comment");
        assert_eq!(t.node_at(at("x;")).unwrap().kind(), "identifier");
        assert_eq!(t.node_at(at("x;") - 1).unwrap().kind(), "block", "whitespace");
        assert_eq!(t.node_at(at("b")).unwrap().kind(), "line_comment");
        assert_eq!(t.node_at(at("}")).unwrap().kind(), "}");
        // Past the last leaf: the end of the last comment.
        let text = "x; // b";
        let tree = parse(Language::Rust, text);
        let mut t = Tokens::new(&tree);
        assert_eq!(t.leaf_at(6), None);
        assert_eq!(t.node_at(6).unwrap().kind(), "line_comment");
    }
}
