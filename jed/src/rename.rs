//! Rename symbol's model (no drawing; `app/rename.rs` drives it): reading a
//! `textDocument/prepareRename` answer and a `WorkspaceEdit`, and turning a file's `TextEdit`s
//! into one `ChangeSet` (format document reuses `text_edits` and `changes`).
//!
//! - `parse_prepare` → `Prepared::{Range, Default, Refused}` (`Range | { range, placeholder } |
//!   { defaultBehavior } | null`).
//! - `parse_workspace_edit` → one `FileEdit` per file (sorted by path; a file named twice gets
//!   its edits merged): `documentChanges` (`TextDocumentEdit`s with their version) when present,
//!   else `changes`. Resource operations (create / rename / delete a file) make it an `Err`: jed
//!   doesn't advertise them, and applying only part of such an edit would leave a broken rename.
//! - `changes(rope, edits, encoding)`: positions through `buffer::pos` (clamped as
//!   `from_lsp_pos`), sorted (insertions at one place keep their order), overlapping edits → `Err`.
//! - `follow(selections, changes)`: where a view's selections land: a position inside a replaced
//!   range keeps its offset into the new text (clamped), any other maps as other views do.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ropey::Rope;
use serde_json::Value;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{ByteIdx, LspPos, LspReader, PosEncoding};
use crate::lsp::uri;
use crate::selection::{Selection, Selections};

/// What `textDocument/prepareRename` said about a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// The symbol's range, and the text to offer (else the range's text).
    Range {
        start: LspPos,
        end: LspPos,
        placeholder: Option<String>,
    },
    /// `{ defaultBehavior: true }`: the word at the position, as without prepare support.
    Default,
    /// `null`: nothing to rename there.
    Refused,
}

fn lsp_pos(v: &Value) -> Option<LspPos> {
    Some(LspPos {
        line: v.get("line")?.as_u64()? as u32,
        character: v.get("character")?.as_u64()? as u32,
    })
}

/// A `Range`'s start and end (end never before start).
fn range(v: &Value) -> Option<(LspPos, LspPos)> {
    let start = lsp_pos(v.get("start")?)?;
    let end = lsp_pos(v.get("end")?)?.max(start);
    Some((start, end))
}

pub fn parse_prepare(value: &Value) -> Prepared {
    if value.get("defaultBehavior").and_then(Value::as_bool) == Some(true) {
        return Prepared::Default;
    }
    let (r, placeholder) = match value.get("range") {
        Some(r) => (r, value.get("placeholder").and_then(Value::as_str).map(String::from)),
        None => (value, None),
    };
    match range(r) {
        Some((start, end)) => Prepared::Range {
            start,
            end,
            placeholder,
        },
        None => Prepared::Refused,
    }
}

/// One `TextEdit`: replace `start..end` (LSP positions) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start: LspPos,
    pub end: LspPos,
    pub text: String,
}

/// A file's edits, and the document version they were computed for (a `TextDocumentEdit`'s;
/// `None`: any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

/// A `TextEdit[]` (a list of `{ range, newText }`).
pub fn text_edits(v: &Value) -> Result<Vec<TextEdit>, String> {
    let items = v.as_array().ok_or("edits aren't a list")?;
    items
        .iter()
        .map(|e| {
            let (start, end) = e.get("range").and_then(range).ok_or("an edit without a range")?;
            // An `AnnotatedTextEdit` is a `TextEdit` with an `annotationId` (ignored).
            let text = e
                .get("newText")
                .and_then(Value::as_str)
                .ok_or("an edit without newText")?;
            Ok(TextEdit {
                start,
                end,
                text: text.to_string(),
            })
        })
        .collect()
}

fn file_path(u: &Value) -> Result<PathBuf, String> {
    let s = u.as_str().ok_or("a document without a URI")?;
    s.parse()
        .ok()
        .and_then(|u| uri::to_path(&u))
        .ok_or_else(|| format!("not a file: {s}"))
}

/// Reads a `WorkspaceEdit` into its files' edits (empty: nothing to change). `Err` holds why it
/// can't be applied.
pub fn parse_workspace_edit(value: &Value) -> Result<Vec<FileEdit>, String> {
    let mut files: BTreeMap<PathBuf, FileEdit> = BTreeMap::new();
    let mut add = |path: PathBuf, version: Option<i32>, edits: Vec<TextEdit>| {
        let f = files.entry(path.clone()).or_insert_with(|| FileEdit {
            path,
            version,
            edits: Vec::new(),
        });
        f.edits.extend(edits);
    };
    if let Some(changes) = value.get("documentChanges").filter(|v| !v.is_null()) {
        for c in changes.as_array().ok_or("documentChanges isn't a list")? {
            if let Some(kind) = c.get("kind").and_then(Value::as_str) {
                return Err(format!("file operations ({kind}) aren't supported"));
            }
            let doc = c.get("textDocument").ok_or("a document change without a document")?;
            let path = file_path(doc.get("uri").unwrap_or(&Value::Null))?;
            let version = doc.get("version").and_then(Value::as_i64).map(|v| v as i32);
            add(path, version, text_edits(c.get("edits").unwrap_or(&Value::Null))?);
        }
    } else if let Some(changes) = value.get("changes").filter(|v| !v.is_null()) {
        for (u, edits) in changes.as_object().ok_or("changes isn't a map")? {
            add(file_path(&Value::String(u.clone()))?, None, text_edits(edits)?);
        }
    }
    Ok(files.into_values().filter(|f| !f.edits.is_empty()).collect())
}

/// `edits` (in `encoding`) as one change set on `rope`; `Err` when two overlap.
pub fn changes(rope: &Rope, edits: &[TextEdit], encoding: PosEncoding) -> Result<ChangeSet, String> {
    let mut order: Vec<&TextEdit> = edits.iter().collect();
    // Stable: insertions at one place keep their order (LSP's rule).
    order.sort_by_key(|e| (e.start, e.end));
    let mut reader = LspReader::new(rope, encoding);
    let mut bytes: Vec<(ByteIdx, ByteIdx, &str)> = Vec::with_capacity(order.len());
    for e in order {
        let start = reader.byte(e.start);
        let end = reader.byte(e.end).max(start);
        if let Some((_, prev_end, _)) = bytes.last()
            && start < *prev_end
        {
            return Err("overlapping edits".into());
        }
        bytes.push((start, end, e.text.as_str()));
    }
    let mut set = ChangeSet::with_capacity(bytes.len(), bytes.iter().map(|(_, _, t)| t.len()).sum());
    for (start, end, text) in bytes {
        set.push(start, end, text);
    }
    Ok(set)
}

/// Where `selections` land after `changes`: a position strictly inside a replaced range keeps
/// its offset into the new text (clamped to it: a cursor in the middle of a renamed word stays
/// there), others map as other views' do (`Assoc::Before`).
pub fn follow(selections: &Selections, changes: &ChangeSet) -> Selections {
    let spans: Vec<(ByteIdx, ByteIdx, &str)> = changes.changes().collect();
    let m = changes.mapper();
    let map = |p: ByteIdx| {
        let i = spans.partition_point(|(_, e, _)| *e <= p);
        if let Some(&(s, e, text)) = spans.get(i)
            && s < p
            && p < e
        {
            let start = m.map(s, Assoc::Before);
            return ByteIdx(start.0 + text.floor_char_boundary((p.0 - s.0).min(text.len())));
        }
        m.map(p, Assoc::Before)
    };
    let mut after = selections.clone();
    after.transform(|s| Selection {
        anchor: map(s.anchor),
        head: map(s.head),
        goal: None,
    });
    after
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn p(line: u32, character: u32) -> LspPos {
        LspPos { line, character }
    }

    fn r(l1: u32, c1: u32, l2: u32, c2: u32) -> Value {
        json!({ "start": { "line": l1, "character": c1 }, "end": { "line": l2, "character": c2 } })
    }

    fn edit(l1: u32, c1: u32, l2: u32, c2: u32, text: &str) -> TextEdit {
        TextEdit {
            start: p(l1, c1),
            end: p(l2, c2),
            text: text.into(),
        }
    }

    #[test]
    fn prepare_answers() {
        assert_eq!(
            parse_prepare(&r(1, 2, 1, 5)),
            Prepared::Range {
                start: p(1, 2),
                end: p(1, 5),
                placeholder: None
            }
        );
        assert_eq!(
            parse_prepare(&json!({ "range": r(0, 0, 0, 3), "placeholder": "foo" })),
            Prepared::Range {
                start: p(0, 0),
                end: p(0, 3),
                placeholder: Some("foo".into())
            }
        );
        assert_eq!(parse_prepare(&json!({ "defaultBehavior": true })), Prepared::Default);
        assert_eq!(parse_prepare(&Value::Null), Prepared::Refused);
        assert_eq!(parse_prepare(&json!({ "nonsense": 1 })), Prepared::Refused);
    }

    #[test]
    fn workspace_edits() {
        // `changes`: by URI, sorted by path.
        let got = parse_workspace_edit(&json!({ "changes": {
            "file:///w/b.cpp": [{ "range": r(0, 0, 0, 1), "newText": "x" }],
            "file:///w/a%20b.cpp": [{ "range": r(1, 0, 1, 2), "newText": "y" }, { "range": r(0, 0, 0, 0), "newText": "z" }],
            "file:///w/empty.cpp": [],
        }}))
        .unwrap();
        assert_eq!(
            got,
            [
                FileEdit {
                    path: "/w/a b.cpp".into(),
                    version: None,
                    edits: vec![edit(1, 0, 1, 2, "y"), edit(0, 0, 0, 0, "z")]
                },
                FileEdit {
                    path: "/w/b.cpp".into(),
                    version: None,
                    edits: vec![edit(0, 0, 0, 1, "x")]
                }
            ]
        );
        // `documentChanges` win over `changes`, with versions; a file named twice is merged.
        let got = parse_workspace_edit(&json!({
            "changes": { "file:///w/ignored.cpp": [{ "range": r(0, 0, 0, 1), "newText": "x" }] },
            "documentChanges": [
                { "textDocument": { "uri": "file:///w/a.cpp", "version": 4 },
                  "edits": [{ "range": r(0, 0, 0, 1), "newText": "1" }] },
                { "textDocument": { "uri": "file:///w/c.cpp", "version": null },
                  "edits": [{ "range": r(2, 0, 2, 1), "newText": "2", "annotationId": "a" }] },
                { "textDocument": { "uri": "file:///w/a.cpp", "version": 4 },
                  "edits": [{ "range": r(3, 0, 3, 1), "newText": "3" }] },
            ],
        }))
        .unwrap();
        let summary: Vec<_> = got
            .iter()
            .map(|f| (f.path.to_str().unwrap(), f.version, f.edits.len()))
            .collect();
        assert_eq!(summary, [("/w/a.cpp", Some(4), 2), ("/w/c.cpp", None, 1)]);
        // File operations are refused; so is nonsense.
        let err = parse_workspace_edit(&json!({ "documentChanges": [
            { "kind": "rename", "oldUri": "file:///w/a.cpp", "newUri": "file:///w/b.cpp" },
        ]}))
        .unwrap_err();
        assert!(err.contains("rename"), "{err}");
        assert!(parse_workspace_edit(&json!({ "changes": { "untitled:1": [] } })).is_err());
        assert!(parse_workspace_edit(&json!({ "changes": { "file:///w/a": [{ "newText": "x" }] } })).is_err());
        assert_eq!(parse_workspace_edit(&json!({})), Ok(vec![]));
    }

    #[test]
    fn edits_become_one_change_set() {
        // UTF-16: the 😀 is two units.
        let mut rope = Rope::from_str("😀 foo(foo);\r\nfoo\n");
        let edits = [
            edit(1, 0, 1, 3, "barbaz"),
            edit(0, 3, 0, 6, "barbaz"),
            edit(0, 7, 0, 10, "barbaz"),
            edit(0, 0, 0, 0, "A"),
            edit(0, 0, 0, 0, "B"),
        ];
        let set = changes(&rope, &edits, PosEncoding::Utf16).unwrap();
        set.apply(&mut rope);
        assert_eq!(rope.to_string(), "AB😀 barbaz(barbaz);\r\nbarbaz\n");
        // In UTF-8 the same columns are bytes.
        let mut rope = Rope::from_str("é foo\n");
        let set = changes(&rope, &[edit(0, 3, 0, 6, "x")], PosEncoding::Utf8).unwrap();
        set.apply(&mut rope);
        assert_eq!(rope.to_string(), "é x\n");
        let rope = Rope::from_str("abcdef\n");
        let err = changes(
            &rope,
            &[edit(0, 0, 0, 3, "x"), edit(0, 2, 0, 4, "y")],
            PosEncoding::Utf8,
        );
        assert!(err.is_err());
    }

    #[test]
    fn selections_follow() {
        let rope = Rope::from_str("foo + foo + x\n");
        let set = changes(
            &rope,
            &[edit(0, 0, 0, 3, "ab"), edit(0, 6, 0, 9, "longer")],
            PosEncoding::Utf8,
        )
        .unwrap();
        let sels = Selections::new(
            vec![
                Selection::cursor(ByteIdx(2)),
                Selection::cursor(ByteIdx(4)),
                Selection::cursor(ByteIdx(7)),
                Selection::new(ByteIdx(9), ByteIdx(12)),
            ],
            0,
        );
        let after = follow(&sels, &set);
        let got: Vec<_> = after.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        // "ab + longer + x": in the first word clamped to its end, then shifted, kept inside the
        // second, the end of the second after it.
        assert_eq!(got, [(2, 2), (3, 3), (6, 6), (11, 14)]);
    }
}
