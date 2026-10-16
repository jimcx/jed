//! Document sync: which buffers each server has open, and the `textDocument/didOpen` /
//! `didChange` / `didSave` / `didClose` notifications that keep its copy equal to the buffer.
//!
//! `Documents::sync` runs once per event-loop turn (`App::sync_servers`): it opens every buffer
//! with a file path whose language's server is running and wants open / close notifications,
//! closes documents whose buffer is gone or moved (renamed: another URI; language changed:
//! another `languageId` or server), forgets those of a server that stopped, and sends each open
//! document's changes since the last turn as one `didChange` (version + 1). The changes come
//! from `Buffer::take_tracked` (every change set `Buffer::apply` applied, edits and undo / redo
//! alike, with the text from before it), converted by `content_changes`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncSaveOptions, Uri,
};
use ropey::Rope;
use serde_json::{Value, json};

use super::{Server, ServerId, State, uri};
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{ByteIdx, LspPos, LspWalker, PosEncoding};
use crate::buffer::{BufferId, Buffers, Tracked};
use crate::syntax::Language;

/// A change set with more changes than this goes out as one change spanning them all (the text
/// between its first and last change, as it is after it): 60k cursors' typing as 60k ranges
/// would be megabytes of JSON per key. `syntax` coalesces its tree edits from the same size.
pub const COALESCE_AT: usize = 256;

/// How many versions of a document are kept for diagnostics published for an older version
/// (`Documents::since`). A server is rarely more than a few behind (one version per loop turn
/// with an edit); a publish for a version no longer kept is read against the current text.
pub const KEEP_VERSIONS: usize = 32;

/// How a server wants `didChange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSync {
    None,
    /// The whole text each time (servers that can't take ranges; jed never prefers it).
    Full,
    Incremental,
}

/// The server's `textDocumentSync` capability. A bare kind (the older form) means open / close,
/// that kind of change and save without text, as VS Code's client reads it; `None` (or the
/// capability missing) means no document sync at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    pub open_close: bool,
    pub change: ChangeSync,
    /// `didSave` wanted; `Some(true)`: with the text.
    pub save: Option<bool>,
}

impl SyncOptions {
    pub fn of(caps: &ServerCapabilities) -> Self {
        let kind = |k: Option<TextDocumentSyncKind>| match k {
            Some(TextDocumentSyncKind::FULL) => ChangeSync::Full,
            Some(TextDocumentSyncKind::INCREMENTAL) => ChangeSync::Incremental,
            _ => ChangeSync::None,
        };
        match &caps.text_document_sync {
            None => Self::NONE,
            Some(TextDocumentSyncCapability::Kind(k)) if *k == TextDocumentSyncKind::NONE => Self::NONE,
            Some(TextDocumentSyncCapability::Kind(k)) => Self {
                open_close: true,
                change: kind(Some(*k)),
                save: Some(false),
            },
            Some(TextDocumentSyncCapability::Options(o)) => Self {
                open_close: o.open_close.unwrap_or(false),
                change: kind(o.change),
                save: match &o.save {
                    None | Some(TextDocumentSyncSaveOptions::Supported(false)) => None,
                    Some(TextDocumentSyncSaveOptions::Supported(true)) => Some(false),
                    Some(TextDocumentSyncSaveOptions::SaveOptions(s)) => Some(s.include_text.unwrap_or(false)),
                },
            },
        }
    }

    const NONE: Self = Self {
        open_close: false,
        change: ChangeSync::None,
        save: None,
    };
}

/// One buffer open on a server.
#[derive(Debug)]
struct Document {
    server: ServerId,
    path: PathBuf,
    uri: Uri,
    language: Language,
    version: i32,
    /// The versions a diagnostic may still be published for, oldest first, the newest (`version`)
    /// last; with incremental or full sync only (else the server keeps version 1).
    versions: VecDeque<Version>,
}

/// One version of a document as sent: its text (a cheap rope clone) and the change sets that
/// lead from it to the next version.
#[derive(Debug)]
struct Version {
    version: i32,
    text: Rope,
    then: Vec<Arc<ChangeSet>>,
}

/// The documents servers have open, by buffer.
#[derive(Debug, Default)]
pub struct Documents {
    open: HashMap<BufferId, Document>,
}

/// The running server a buffer belongs to, if it wants documents opened, and its options.
fn server_for(servers: &[Server], language: Language) -> Option<(&Server, SyncOptions, PosEncoding)> {
    let s = servers.iter().find(|s| s.languages.contains(&language))?;
    let State::Running { encoding, capabilities } = &s.state else {
        return None;
    };
    let options = SyncOptions::of(capabilities);
    options.open_close.then_some((s, options, *encoding))
}

impl Documents {
    /// Brings every server's open documents in line with the buffers and sends what changed
    /// (see the module doc). Cheap when nothing did: a lookup per buffer.
    pub fn sync(&mut self, servers: &[Server], buffers: &mut Buffers) {
        // Close (or forget) what no longer matches; send the rest's changes.
        let ids: Vec<BufferId> = self.open.keys().copied().collect();
        for id in ids {
            let doc = &self.open[&id];
            let server = doc.server;
            let running = servers
                .iter()
                .find(|s| s.id == server)
                .filter(|s| matches!(s.state, State::Running { .. }));
            // Still the same file, language and server?
            let current = running.and_then(|_| {
                let b = buffers.get(id)?;
                let (s, options, encoding) = server_for(servers, b.syntax.language())?;
                (s.id == server && b.path == doc.path && b.syntax.language() == doc.language)
                    .then_some((options, encoding))
            });
            if let (Some(server), Some((options, encoding)), Some(buf)) = (running, current, buffers.get_mut(id)) {
                let doc = self.open.get_mut(&id).expect("open");
                send_changes(server, doc, buf.take_tracked(), &buf.rope, options.change, encoding);
                continue;
            }
            let doc = self.open.remove(&id).expect("open");
            if let Some(buf) = buffers.get_mut(id) {
                buf.track_changes(false);
            }
            match running {
                Some(server) => {
                    tracing::debug!(server = %server.config.name, uri = doc.uri.as_str(), "didClose");
                    server.notify(
                        "textDocument/didClose",
                        json!({ "textDocument": { "uri": doc.uri.as_str() } }),
                    );
                }
                // Stopped or failed: nothing to tell it.
                None => tracing::debug!(uri = doc.uri.as_str(), "document dropped with its server"),
            }
        }
        // Open what should be.
        for (id, buf) in buffers.iter_mut() {
            if self.open.contains_key(&id) || !buf.path.is_absolute() {
                continue;
            }
            let language = buf.syntax.language();
            let Some((server, ..)) = server_for(servers, language) else {
                continue;
            };
            let doc = Document {
                server: server.id,
                path: buf.path.clone(),
                uri: uri::from_path(&buf.path),
                language,
                version: 1,
                versions: VecDeque::from([Version {
                    version: 1,
                    text: buf.rope.clone(),
                    then: Vec::new(),
                }]),
            };
            tracing::debug!(server = %server.config.name, uri = doc.uri.as_str(), language = language.id(), "didOpen");
            server.notify(
                "textDocument/didOpen",
                json!({ "textDocument": {
                    "uri": doc.uri.as_str(),
                    "languageId": language.id(),
                    "version": doc.version,
                    "text": buf.rope.to_string(),
                }}),
            );
            buf.track_changes(true);
            self.open.insert(id, doc);
        }
    }

    /// After `buffer` was saved: `didSave` (with the text if the server asked for it), once
    /// the document is current (`sync` first).
    pub fn saved(&mut self, servers: &[Server], buffers: &mut Buffers, buffer: BufferId) {
        self.sync(servers, buffers);
        let (Some(doc), Some(buf)) = (self.open.get(&buffer), buffers.get(buffer)) else {
            return;
        };
        let Some((server, options, _)) = server_for(servers, doc.language) else {
            return;
        };
        let Some(include_text) = options.save else {
            return;
        };
        let mut params = json!({ "textDocument": { "uri": doc.uri.as_str() } });
        if include_text {
            params["text"] = Value::String(buf.rope.to_string());
        }
        server.notify("textDocument/didSave", params);
    }

    /// The version of the document open for `buffer`.
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn version(&self, buffer: BufferId) -> Option<i32> {
        self.open.get(&buffer).map(|d| d.version)
    }

    /// The server `buffer`'s document is open on, its URI and version (requests about it).
    pub fn document(&self, buffer: BufferId) -> Option<(ServerId, &Uri, i32)> {
        self.open.get(&buffer).map(|d| (d.server, &d.uri, d.version))
    }

    /// For diagnostics `server` published for `version` of `buffer`'s document: the text of that
    /// version and the change sets applied since, in order (`sync` first, so the newest version
    /// is the buffer's text). Versions before it are dropped (a server publishes in order).
    /// `None` when the document isn't open on `server` or that version is no longer kept.
    pub fn since(&mut self, buffer: BufferId, server: ServerId, version: i32) -> Option<(Rope, Vec<Arc<ChangeSet>>)> {
        let doc = self.open.get_mut(&buffer).filter(|d| d.server == server)?;
        let i = doc.versions.iter().position(|v| v.version == version)?;
        doc.versions.drain(..i);
        let text = doc.versions[0].text.clone();
        let sets = doc.versions.iter().flat_map(|v| v.then.iter().cloned()).collect();
        Some((text, sets))
    }
}

fn send_changes(
    server: &Server,
    doc: &mut Document,
    tracked: Vec<Tracked>,
    current: &Rope,
    change: ChangeSync,
    encoding: PosEncoding,
) {
    if tracked.is_empty() {
        return;
    }
    let changes = match change {
        ChangeSync::None => {
            // The server's copy stays at version 1, the text as opened; its diagnostics are read
            // against the current text (the edits since aren't kept).
            doc.versions.clear();
            return;
        }
        ChangeSync::Full => vec![json!({ "text": current.to_string() })],
        ChangeSync::Incremental => content_changes(&tracked, current, encoding),
    };
    doc.version += 1;
    if let Some(last) = doc.versions.back_mut() {
        last.then.extend(tracked.iter().map(|t| t.changes.clone()));
    }
    doc.versions.push_back(Version {
        version: doc.version,
        text: current.clone(),
        then: Vec::new(),
    });
    while doc.versions.len() > KEEP_VERSIONS {
        doc.versions.pop_front();
    }
    tracing::trace!(server = %server.config.name, uri = doc.uri.as_str(), version = doc.version, changes = changes.len(), "didChange");
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": doc.uri.as_str(), "version": doc.version },
            "contentChanges": changes,
        }),
    );
}

/// `didChange`'s `contentChanges` for `tracked` (in order; `current` is the text after the
/// last): LSP applies them one after another, each in the text the previous ones left. Within a
/// set they go last change first, so every range stays in the set's pre-edit coordinates (changes
/// that touch are merged into one, see below); a set
/// over `COALESCE_AT` changes is one range from its first change's start to its last one's end,
/// replaced by that stretch of the text after it. Positions in `encoding`, via `LspWalker` (one
/// forward pass per set): O(changes · log n) in all, never the whole text.
pub fn content_changes(tracked: &[Tracked], current: &Rope, encoding: PosEncoding) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, t) in tracked.iter().enumerate() {
        let Some((first, last, delta)) = t.changes.extent() else {
            continue;
        };
        let mut walker = LspWalker::new(&t.before, encoding);
        if t.changes.len() > COALESCE_AT {
            let after = tracked.get(i + 1).map_or(current, |next| &next.before);
            let end = (last.0 as isize + delta) as usize;
            let text = after.slice(first.0..end).to_string();
            out.push(change(walker.pos(first), walker.pos(last), text));
        } else {
            // Changes that touch go out as one: a later one applied first could otherwise turn a
            // lone CR just before the earlier one's end into a CRLF (an LF inserted, or the char
            // between them removed), moving that end to another line.
            let at = out.len();
            let mut pending: Option<(ByteIdx, ByteIdx, String)> = None;
            for (start, end, text) in t.changes.changes() {
                match &mut pending {
                    Some((_, e, t)) if *e == start => {
                        *e = end;
                        t.push_str(text);
                    }
                    _ => {
                        if let Some((s, e, t)) = pending.replace((start, end, text.to_string())) {
                            out.push(change(walker.pos(s), walker.pos(e), t));
                        }
                    }
                }
            }
            if let Some((s, e, t)) = pending {
                out.push(change(walker.pos(s), walker.pos(e), t));
            }
            out[at..].reverse();
        }
    }
    out
}

fn change(start: LspPos, end: LspPos, text: String) -> Value {
    json!({
        "range": {
            "start": { "line": start.line, "character": start.character },
            "end": { "line": end.line, "character": end.character },
        },
        "text": text,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::buffer::change::{Change, ChangeSet};
    use crate::buffer::pos::from_lsp_pos;

    /// Applies `contentChanges` to `text` one after another, as a server does.
    fn apply(text: &str, changes: &[Value], encoding: PosEncoding) -> String {
        let mut rope = Rope::from_str(text);
        for c in changes {
            let Some(range) = c.get("range") else {
                rope = Rope::from_str(c["text"].as_str().unwrap());
                continue;
            };
            let pos = |p: &Value| LspPos {
                line: p["line"].as_u64().unwrap() as u32,
                character: p["character"].as_u64().unwrap() as u32,
            };
            let start = from_lsp_pos(&rope, pos(&range["start"]), encoding).0;
            let end = from_lsp_pos(&rope, pos(&range["end"]), encoding).0;
            rope.remove(start..end);
            rope.insert(start, c["text"].as_str().unwrap());
        }
        rope.to_string()
    }

    fn set(changes: &[(usize, usize, &str)]) -> ChangeSet {
        ChangeSet::new(
            changes
                .iter()
                .map(|&(s, e, t)| Change {
                    start: ByteIdx(s),
                    end: ByteIdx(e),
                    text: t.into(),
                })
                .collect(),
        )
    }

    /// Applies each set in turn, tracking as `Buffer::apply` does.
    fn track(text: &str, sets: Vec<ChangeSet>) -> (Vec<Tracked>, Rope) {
        let mut rope = Rope::from_str(text);
        let mut tracked = Vec::new();
        for s in sets {
            let before = rope.clone();
            s.apply(&mut rope);
            tracked.push(Tracked {
                before,
                changes: Arc::new(s),
            });
        }
        (tracked, rope)
    }

    #[test]
    fn changes_go_last_first_in_pre_edit_coordinates() {
        let text = "aé😀b\r\nx\ry\n";
        let (tracked, after) = track(
            text,
            vec![
                // Two inserts at one position, a replace across the CRLF, a delete at the end.
                set(&[(0, 0, "1"), (0, 0, "2"), (7, 11, "-"), (13, 14, "")]),
                set(&[(1, 2, "")]),
            ],
        );
        for enc in [PosEncoding::Utf8, PosEncoding::Utf16] {
            let changes = content_changes(&tracked, &after, enc);
            assert_eq!(changes.len(), 4, "the two inserts at 0 touch: one change");
            assert_eq!(apply(text, &changes, enc), after.to_string(), "{enc:?}");
        }
        let changes = content_changes(&tracked, &after, PosEncoding::Utf16);
        assert_eq!(
            changes[1],
            json!({ "range": { "start": { "line": 0, "character": 4 }, "end": { "line": 1, "character": 1 } }, "text": "-" }),
            "😀 is two UTF-16 units; the CRLF ends line 0"
        );
    }

    /// A change right after a lone CR, next to a later change in the same set that puts an LF
    /// (or removes a char) right there: applied first, the later change would turn the CR into a
    /// CRLF and move the earlier change's position to another line.
    #[test]
    fn adjacent_changes_after_a_lone_cr() {
        for (text, changes) in [
            ("a\rb", vec![(2, 2, "y"), (2, 3, "\n")]),
            ("a\rxb\n", vec![(0, 2, "z\r"), (2, 3, "")]),
            ("a\rx\nb", vec![(1, 2, "\r\r"), (2, 3, "")]),
        ] {
            let (tracked, after) = track(text, vec![set(&changes)]);
            for enc in [PosEncoding::Utf8, PosEncoding::Utf16] {
                let sent = content_changes(&tracked, &after, enc);
                assert_eq!(apply(text, &sent, enc), after.to_string(), "{text:?} {changes:?}");
            }
        }
    }

    /// Exactly `COALESCE_AT` changes go out one by one; one more, as one range.
    #[test]
    fn coalescing_starts_above_the_limit() {
        let text = "ab\n".repeat(COALESCE_AT + 1);
        for n in [COALESCE_AT, COALESCE_AT + 1] {
            let inserts: Vec<(usize, usize, &str)> = (0..n).map(|i| (3 * i + 1, 3 * i + 1, "x")).collect();
            let (tracked, after) = track(&text, vec![set(&inserts)]);
            let sent = content_changes(&tracked, &after, PosEncoding::Utf16);
            assert_eq!(sent.len(), if n == COALESCE_AT { n } else { 1 });
            assert_eq!(apply(&text, &sent, PosEncoding::Utf16), after.to_string());
        }
    }

    #[test]
    fn big_sets_are_one_range() {
        let text = "let a😀 = 1;\n".repeat(1000);
        let positions: Vec<usize> = text.match_indices('a').map(|(i, _)| i + 1).collect();
        assert!(positions.len() > COALESCE_AT);
        let inserts: Vec<(usize, usize, &str)> = positions.iter().map(|&p| (p, p, "bc")).collect();
        let (tracked, after) = track(&text, vec![set(&inserts), set(&[(0, 3, "var")])]);
        for enc in [PosEncoding::Utf8, PosEncoding::Utf16] {
            let changes = content_changes(&tracked, &after, enc);
            assert_eq!(changes.len(), 2, "one range for the big set, one for the small");
            let first = &changes[0];
            assert_eq!(first["range"]["start"], json!({ "line": 0, "character": 5 }));
            assert_eq!(first["range"]["end"], json!({ "line": 999, "character": 5 }));
            assert_eq!(apply(&text, &changes, enc), after.to_string(), "{enc:?}");
        }
    }

    #[test]
    fn sync_options() {
        let caps = |v: Value| SyncOptions::of(&serde_json::from_value(json!({ "textDocumentSync": v })).unwrap());
        let o = |open_close, change, save| SyncOptions {
            open_close,
            change,
            save,
        };
        assert_eq!(caps(json!(2)), o(true, ChangeSync::Incremental, Some(false)));
        assert_eq!(caps(json!(1)), o(true, ChangeSync::Full, Some(false)));
        assert_eq!(caps(json!(0)), SyncOptions::NONE);
        assert_eq!(SyncOptions::of(&ServerCapabilities::default()), SyncOptions::NONE);
        assert_eq!(
            caps(json!({ "openClose": true, "change": 2, "save": { "includeText": true } })),
            o(true, ChangeSync::Incremental, Some(true))
        );
        assert_eq!(
            caps(json!({ "openClose": true, "change": 1, "save": true })),
            o(true, ChangeSync::Full, Some(false))
        );
        assert_eq!(caps(json!({ "change": 2 })), o(false, ChangeSync::Incremental, None));
    }
}
