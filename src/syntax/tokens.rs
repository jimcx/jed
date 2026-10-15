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
}

impl<'t> Tokens<'t> {
    pub fn new(tree: &'t Tree) -> Self {
        Self {
            tree,
            cursor: tree.walk(),
            last: 0,
            done: false,
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
}
