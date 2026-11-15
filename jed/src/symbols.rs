//! Symbols a language server answers with (no app state, draws nothing): a file's
//! `textDocument/documentSymbol` as a tree and `workspace/symbol` as a flat list. Claude's
//! `get_document_symbols` / `find_workspace_symbols` (`app/claude_lsp.rs`) are built on them.
//!
//! - `document` reads both shapes: `DocumentSymbol[]` (a tree already) and flat
//!   `SymbolInformation[]`, nested by `containerName`: a symbol goes under the nearest earlier
//!   symbol of that name whose range contains it, else at the top.
//! - `workspace` reads `SymbolInformation[]` and `WorkspaceSymbol[]` (whose location may be just a
//!   `uri`, so `start` is `None`); places in anything but a `file:` URI are skipped.
//! - Malformed entries are skipped; null or empty answers give nothing.

use std::path::PathBuf;

use lsp_types::SymbolKind;
use serde_json::Value;

use crate::buffer::pos::LspPos;
use crate::lsp::uri;

/// One symbol of a file, with the symbols inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: Option<String>,
    /// The whole symbol, and the part that names it.
    pub range: (LspPos, LspPos),
    pub selection: (LspPos, LspPos),
    pub children: Vec<Symbol>,
}

impl Symbol {
    /// This symbol and all inside it.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Symbol::count).sum::<usize>()
    }
}

/// One `workspace/symbol` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceItem {
    pub name: String,
    pub kind: SymbolKind,
    pub container: Option<String>,
    pub path: PathBuf,
    /// `None` when the server left the range out (a `WorkspaceSymbol` with only a `uri`).
    pub start: Option<LspPos>,
}

fn pos(p: &Value) -> Option<LspPos> {
    Some(LspPos {
        line: p.get("line")?.as_u64()? as u32,
        character: p.get("character")?.as_u64()? as u32,
    })
}

fn range(r: &Value) -> Option<(LspPos, LspPos)> {
    let start = pos(r.get("start")?)?;
    Some((start, pos(r.get("end")?)?.max(start)))
}

fn kind(v: &Value) -> Option<SymbolKind> {
    serde_json::from_value(v.get("kind")?.clone()).ok()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// A `DocumentSymbol` and its children.
fn tree(v: &Value) -> Option<Symbol> {
    let whole = range(v.get("range")?)?;
    let children = match v.get("children") {
        Some(Value::Array(a)) => a.iter().filter_map(tree).collect(),
        _ => Vec::new(),
    };
    Some(Symbol {
        name: v.get("name")?.as_str()?.to_string(),
        kind: kind(v)?,
        detail: text(v, "detail"),
        selection: v.get("selectionRange").and_then(range).unwrap_or(whole),
        range: whole,
        children,
    })
}

/// A flat `SymbolInformation` and its container's name.
fn flat(v: &Value) -> Option<(Symbol, Option<String>)> {
    let whole = range(v.get("location")?.get("range")?)?;
    let symbol = Symbol {
        name: v.get("name")?.as_str()?.to_string(),
        kind: kind(v)?,
        detail: None,
        range: whole,
        selection: whole,
        children: Vec::new(),
    };
    Some((symbol, text(v, "containerName")))
}

/// Reads a `documentSymbol` answer (`DocumentSymbol[] | SymbolInformation[] | null`).
pub fn document(result: &Value) -> Vec<Symbol> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    // A `DocumentSymbol` has `range`; a `SymbolInformation` has `location`.
    if items.iter().any(|v| v.get("range").is_some()) {
        return items.iter().filter_map(tree).collect();
    }
    let flats: Vec<(Symbol, Option<String>)> = items.iter().filter_map(flat).collect();
    // Each symbol's parent: the nearest earlier one named as its container that contains it.
    let parents: Vec<Option<usize>> = (0..flats.len())
        .map(|i| {
            let (s, container) = &flats[i];
            let container = container.as_ref()?;
            (0..i).rev().find(|&j| {
                let p = &flats[j].0;
                p.name == *container && p.range.0 <= s.range.0 && s.range.1 <= p.range.1
            })
        })
        .collect();
    let mut slots: Vec<Option<Symbol>> = flats.into_iter().map(|(s, _)| Some(s)).collect();
    // Later symbols first: a child is complete before it moves into its parent.
    for i in (0..slots.len()).rev() {
        if let Some(p) = parents[i]
            && let Some(child) = slots[i].take()
            && let Some(parent) = slots[p].as_mut()
        {
            parent.children.insert(0, child);
        }
    }
    slots.into_iter().flatten().collect()
}

/// Reads a `workspace/symbol` answer (`SymbolInformation[] | WorkspaceSymbol[] | null`).
pub fn workspace(result: &Value) -> Vec<WorkspaceItem> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let location = v.get("location")?;
            let path = uri::to_path(&location.get("uri")?.as_str()?.parse().ok()?)?;
            Some(WorkspaceItem {
                name: v.get("name")?.as_str()?.to_string(),
                kind: kind(v)?,
                container: text(v, "containerName"),
                path,
                start: location.get("range").and_then(range).map(|r| r.0),
            })
        })
        .collect()
}

/// A kind's lower case name ("function", "struct", "enum member").
pub fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "number",
        SymbolKind::BOOLEAN => "boolean",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "enum member",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type parameter",
        _ => "symbol",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn r(a: u32, b: u32, c: u32, d: u32) -> Value {
        json!({"start": {"line": a, "character": b}, "end": {"line": c, "character": d}})
    }

    #[test]
    fn document_symbol_tree_keeps_children_detail_and_selection() {
        let answer = json!([
            {"name": "Shape", "kind": 23, "detail": "pub", "range": r(0, 0, 9, 1), "selectionRange": r(0, 11, 0, 16),
             "children": [
                {"name": "sides", "kind": 8, "range": r(1, 4, 1, 14), "selectionRange": r(1, 4, 1, 9)},
                {"bogus": true}
             ]},
            {"name": "run", "kind": 12, "range": r(11, 0, 13, 1)}
        ]);
        let got = document(&answer);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Shape");
        assert_eq!(got[0].kind, SymbolKind::STRUCT);
        assert_eq!(got[0].detail.as_deref(), Some("pub"));
        assert_eq!(got[0].selection.0, LspPos { line: 0, character: 11 });
        assert_eq!(got[0].children.len(), 1);
        assert_eq!(got[0].children[0].name, "sides");
        assert_eq!(got[0].count(), 2);
        // No selectionRange: the whole range.
        assert_eq!(got[1].selection, got[1].range);
    }

    #[test]
    fn flat_symbols_nest_by_container() {
        let loc = |a, b, c, d| json!({"uri": "file:///w/a.rs", "range": r(a, b, c, d)});
        let answer = json!([
            {"name": "Shape", "kind": 23, "location": loc(0, 0, 9, 1)},
            {"name": "sides", "kind": 8, "containerName": "Shape", "location": loc(1, 4, 1, 14)},
            {"name": "area", "kind": 6, "containerName": "Shape", "location": loc(3, 4, 5, 5)},
            {"name": "sides", "kind": 8, "containerName": "Other", "location": loc(20, 0, 20, 5)},
            {"name": "run", "kind": 12, "location": loc(11, 0, 13, 1)},
            {"name": "broken", "kind": 12}
        ]);
        let got = document(&answer);
        let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["Shape", "sides", "run"],
            "a container that isn't there: top level"
        );
        let kids: Vec<&str> = got[0].children.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(kids, ["sides", "area"]);
    }

    #[test]
    fn flat_symbols_with_equal_names_nest_under_the_one_that_contains_them() {
        let loc = |a, b, c, d| json!({"uri": "file:///w/a.rs", "range": r(a, b, c, d)});
        let answer = json!([
            {"name": "Shape", "kind": 5, "location": loc(0, 0, 4, 1)},
            {"name": "Shape", "kind": 5, "location": loc(10, 0, 14, 1)},
            {"name": "late", "kind": 6, "containerName": "Shape", "location": loc(12, 4, 12, 9)},
            {"name": "early", "kind": 6, "containerName": "Shape", "location": loc(1, 4, 1, 9)},
            // Before its container in the list: nothing earlier holds it, so the top level.
            {"name": "orphan", "kind": 6, "containerName": "Later", "location": loc(20, 0, 20, 3)},
            {"name": "Later", "kind": 5, "location": loc(19, 0, 22, 1)}
        ]);
        let got = document(&answer);
        let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Shape", "Shape", "orphan", "Later"]);
        assert_eq!(got[0].children.len(), 1);
        assert_eq!(got[0].children[0].name, "early");
        assert_eq!(got[1].children.len(), 1);
        assert_eq!(got[1].children[0].name, "late");
        assert_eq!(got.iter().map(Symbol::count).sum::<usize>(), 6);
    }

    #[test]
    fn empty_and_null_answers_are_empty() {
        assert!(document(&Value::Null).is_empty());
        assert!(document(&json!([])).is_empty());
        assert!(workspace(&Value::Null).is_empty());
    }

    #[test]
    fn workspace_symbols_both_shapes_skip_other_uris() {
        let answer = json!([
            {"name": "Shape", "kind": 23, "containerName": "geom",
             "location": {"uri": "file:///w/a.rs", "range": r(4, 11, 4, 16)}},
            {"name": "Circle", "kind": 5, "location": {"uri": "file:///w/b.rs"}},
            {"name": "Remote", "kind": 5, "location": {"uri": "https://example.com/x"}},
            {"name": "NoKind", "location": {"uri": "file:///w/c.rs"}}
        ]);
        let got = workspace(&answer);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].path, PathBuf::from("/w/a.rs"));
        assert_eq!(got[0].container.as_deref(), Some("geom"));
        assert_eq!(got[0].start, Some(LspPos { line: 4, character: 11 }));
        assert_eq!(got[1].name, "Circle");
        assert_eq!(got[1].start, None);
    }

    #[test]
    fn kind_names() {
        assert_eq!(kind_name(SymbolKind::STRUCT), "struct");
        assert_eq!(kind_name(SymbolKind::ENUM_MEMBER), "enum member");
        assert_eq!(kind_name(SymbolKind::FUNCTION), "function");
    }
}
