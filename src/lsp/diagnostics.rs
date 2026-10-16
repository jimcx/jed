//! Pushed diagnostics (`textDocument/publishDiagnostics`): `parse` reads a publish, `Store`
//! keeps every file's as published (LSP positions, per server: a publish replaces only that
//! server's set; files not open in a buffer too, for the Problems list and the status bar's
//! counts), and `resolve` places them in a buffer's text as `diagnostics::Mark`s.
//!
//! Versions: a publish names the document version it was computed for. `Documents` keeps the
//! text of the last `KEEP_VERSIONS` versions sent and the change sets between them, so the
//! positions are read against the text the server saw and then mapped through the edits made
//! since (`Documents::since`). From then on the buffer maps its marks through each change set it
//! applies (`Buffer::apply`), until the next publish replaces them. Without a version, or one no
//! longer kept, positions are read against the current text (what VS Code does).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{NumberOrString, PublishDiagnosticsParams};
use ropey::Rope;
use serde_json::Value;

use super::{ServerId, uri};
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{LspPos, LspReader, PosEncoding};
use crate::diagnostics::{Diagnostic, Mark, Severity, map_marks};

/// One published diagnostic: its range as the server sent it.
#[derive(Debug, Clone)]
pub struct Item {
    pub start: LspPos,
    pub end: LspPos,
    pub diag: Arc<Diagnostic>,
}

/// A `publishDiagnostics` notification, read.
#[derive(Debug)]
pub struct Published {
    pub path: PathBuf,
    pub version: Option<i32>,
    /// Sorted by start.
    pub items: Vec<Item>,
}

/// Reads `publishDiagnostics` params; `Err` for something that isn't one, or a URI that isn't
/// a local file.
pub fn parse(params: Value) -> Result<Published, String> {
    let p: PublishDiagnosticsParams = serde_json::from_value(params).map_err(|e| e.to_string())?;
    let path = uri::to_path(&p.uri).ok_or_else(|| format!("not a file URI: {}", p.uri.as_str()))?;
    let pos = |p: lsp_types::Position| LspPos {
        line: p.line,
        character: p.character,
    };
    let mut items: Vec<Item> = p
        .diagnostics
        .into_iter()
        .map(|d| {
            let (start, end) = (pos(d.range.start), pos(d.range.end));
            Item {
                start,
                end: end.max(start),
                diag: Arc::new(Diagnostic {
                    severity: Severity::from_lsp(d.severity),
                    message: d.message,
                    source: d.source,
                    code: d.code.map(|c| match c {
                        NumberOrString::Number(n) => n.to_string(),
                        NumberOrString::String(s) => s,
                    }),
                }),
            }
        })
        .collect();
    items.sort_by_key(|i| i.start);
    Ok(Published {
        path,
        version: p.version,
        items,
    })
}

/// Places `items` (positions in `text`, the document as the server saw it, in `encoding`) in
/// the current text: read with one `LspReader` pass, then mapped through `since` (the change
/// sets applied after `text`, in order). Sorted by start.
pub fn resolve(
    server: ServerId,
    items: &[Item],
    text: &Rope,
    encoding: PosEncoding,
    since: &[Arc<ChangeSet>],
) -> Vec<Mark> {
    let mut reader = LspReader::new(text, encoding);
    let mut marks: Vec<Mark> = items
        .iter()
        .map(|i| {
            let start = reader.byte(i.start);
            Mark {
                start,
                end: reader.byte(i.end).max(start),
                server,
                diag: i.diag.clone(),
            }
        })
        .collect();
    for set in since {
        map_marks(&mut marks, set);
    }
    marks
}

/// Diagnostics counted by severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    by_severity: [usize; 4],
}

impl Counts {
    pub fn of(&self, severity: Severity) -> usize {
        self.by_severity[severity as usize]
    }

    fn add(&mut self, items: &[Item], sign: isize) {
        for i in items {
            let n = &mut self.by_severity[i.diag.severity as usize];
            *n = n.wrapping_add_signed(sign);
        }
    }
}

/// One server's diagnostics for one file, as last published.
#[derive(Debug)]
pub struct FileSet {
    pub server: ServerId,
    pub encoding: PosEncoding,
    pub items: Vec<Item>,
}

/// Every file's diagnostics, by path (sorted: the Problems list's order), then by server.
#[derive(Debug, Default)]
pub struct Store {
    files: BTreeMap<PathBuf, Vec<FileSet>>,
    counts: Counts,
}

impl Store {
    /// Replaces `server`'s diagnostics for the file (an empty publish removes them).
    pub fn publish(&mut self, server: ServerId, encoding: PosEncoding, p: Published) {
        let sets = self.files.entry(p.path.clone()).or_default();
        if let Some(i) = sets.iter().position(|s| s.server == server) {
            let old = sets.remove(i);
            self.counts.add(&old.items, -1);
        }
        if !p.items.is_empty() {
            self.counts.add(&p.items, 1);
            sets.push(FileSet {
                server,
                encoding,
                items: p.items,
            });
        }
        if sets.is_empty() {
            self.files.remove(&p.path);
        }
    }

    /// Drops everything `server` published (it stopped); true if there was something.
    pub fn clear_server(&mut self, server: ServerId) -> bool {
        let mut any = false;
        self.files.retain(|_, sets| {
            sets.retain(|s| {
                let keep = s.server != server;
                if !keep {
                    self.counts.add(&s.items, -1);
                    any = true;
                }
                keep
            });
            !sets.is_empty()
        });
        any
    }

    /// The file's diagnostics, per server.
    pub fn get(&self, path: &Path) -> &[FileSet] {
        self.files.get(path).map_or(&[], Vec::as_slice)
    }

    /// Every file with diagnostics, sorted by path (for the Problems list).
    #[cfg_attr(not(test), allow(dead_code))] // the Problems list (9d)
    pub fn files(&self) -> impl Iterator<Item = (&Path, &[FileSet])> {
        self.files.iter().map(|(p, s)| (p.as_path(), s.as_slice()))
    }

    /// Workspace-wide totals (the status bar's).
    pub fn counts(&self) -> Counts {
        self.counts
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::buffer::change::Change;
    use crate::buffer::pos::ByteIdx;

    fn publish(uri: &str, version: Option<i32>, diags: Value) -> Published {
        let mut params = json!({ "uri": uri, "diagnostics": diags });
        if let Some(v) = version {
            params["version"] = json!(v);
        }
        parse(params).unwrap()
    }

    fn d(line: u32, from: u32, to: u32, severity: i32, message: &str) -> Value {
        json!({
            "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
            "severity": severity,
            "message": message,
        })
    }

    #[test]
    fn parse_reads_a_publish() {
        let p = parse(json!({
            "uri": "file:///ws/a%20b.cpp",
            "version": 3,
            "diagnostics": [
                d(2, 0, 1, 2, "later"),
                { "range": { "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 1 } },
                  "message": "no severity", "source": "clang", "code": 42 },
            ],
        }))
        .unwrap();
        assert_eq!(p.path, Path::new("/ws/a b.cpp"));
        assert_eq!(p.version, Some(3));
        let first = &p.items[0];
        assert_eq!(
            (first.start, first.end),
            (LspPos { line: 0, character: 4 }, LspPos { line: 0, character: 4 }),
            "sorted; a reversed range is empty"
        );
        assert_eq!(first.diag.severity, Severity::Error);
        assert_eq!(
            (first.diag.source.as_deref(), first.diag.code.as_deref()),
            (Some("clang"), Some("42"))
        );
        assert_eq!(p.items[1].diag.severity, Severity::Warning);
        assert!(parse(json!({ "uri": "untitled:1", "diagnostics": [] })).is_err());
        assert!(parse(json!({ "diagnostics": [] })).is_err());
    }

    #[test]
    fn store_replaces_per_server_and_counts() {
        let (s1, s2) = (ServerId::test(1), ServerId::test(2));
        let mut store = Store::default();
        let enc = PosEncoding::Utf8;
        store.publish(
            s1,
            enc,
            publish("file:///a", Some(1), json!([d(0, 0, 1, 1, "e"), d(1, 0, 1, 2, "w")])),
        );
        store.publish(s2, enc, publish("file:///a", None, json!([d(0, 0, 1, 1, "e2")])));
        store.publish(s1, enc, publish("file:///b", None, json!([d(0, 0, 1, 3, "i")])));
        let count = |store: &Store| Severity::ALL.map(|s| store.counts().of(s));
        assert_eq!(count(&store), [2, 1, 1, 0]);
        assert_eq!(store.get(Path::new("/a")).len(), 2);
        // s1 republishes /a: only its own set changes.
        store.publish(s1, enc, publish("file:///a", Some(2), json!([d(0, 0, 1, 4, "h")])));
        assert_eq!(count(&store), [1, 0, 1, 1]);
        let a = store.get(Path::new("/a"));
        assert_eq!(a.len(), 2);
        assert!(a.iter().any(|s| s.server == s2 && s.items[0].diag.message == "e2"));
        // An empty publish removes the set; a file with none left goes.
        store.publish(s1, enc, publish("file:///b", None, json!([])));
        assert_eq!(
            store.files().map(|(p, _)| p.to_path_buf()).collect::<Vec<_>>(),
            [PathBuf::from("/a")]
        );
        assert!(store.clear_server(s2));
        assert!(!store.clear_server(s2));
        assert_eq!(count(&store), [0, 0, 0, 1]);
        assert!(store.clear_server(s1));
        assert_eq!(store.files().count(), 0);
        assert_eq!(count(&store), [0; 4]);
    }

    /// Positions are read in the published version's text (UTF-16 here), then mapped through the
    /// edits since: the squiggle stays on its word.
    #[test]
    fn resolve_reads_the_old_text_and_maps_forward() {
        let old = Rope::from_str("😀 foo bar\nbaz\n");
        let p = publish(
            "file:///a",
            Some(1),
            json!([
                d(0, 7, 10, 1, "bar"),
                d(1, 0, 3, 2, "baz"),
                d(0, 3, 6, 1, "foo"),
                d(9, 0, 0, 1, "past")
            ]),
        );
        let edit = ChangeSet::new(vec![
            Change {
                start: ByteIdx(0),
                end: ByteIdx(0),
                text: "xx".into(),
            },
            Change {
                start: ByteIdx(9),
                end: ByteIdx(9),
                text: "yy".into(),
            },
        ]);
        let mut new = old.clone();
        edit.apply(&mut new);
        let marks = resolve(ServerId::test(1), &p.items, &old, PosEncoding::Utf16, &[Arc::new(edit)]);
        let text = new.to_string();
        let got: Vec<(&str, &str)> = marks
            .iter()
            .map(|m| (&text[m.start.0..m.end.0], m.diag.message.as_str()))
            .collect();
        assert_eq!(got, [("foo", "foo"), ("bar", "bar"), ("baz", "baz"), ("", "past")]);
        assert_eq!(marks[3].start, ByteIdx(new.len()));
    }
}
