//! Code actions' model (no drawing; `app/code_action.rs` drives it, `ui::code_action` draws the
//! list, `ui::editor` the lightbulb): reading a `textDocument/codeAction` answer, the order the
//! list shows, the list's state, the lightbulb's cache and the hover's quick fixes.
//!
//! - `parse` takes `(Command | CodeAction)[] | null`. A `CodeAction` literal keeps its title,
//!   `kind`, `isPreferred`, `disabled.reason`, `edit` (a `WorkspaceEdit`, as sent) and `command`;
//!   a bare `Command` is an action that only runs it. Each keeps its JSON for
//!   `codeAction/resolve`.
//! - `sort`: quick fixes first, then refactorings, source actions and the rest (VS Code's
//!   groups), the preferred first within each; otherwise the server's order (stable).
//! - `Popup`: the list at the cursor; the selection starts on the first enabled action. Disabled
//!   ones are shown (greyed, with their reason) and can be selected, not applied.
//! - The lightbulb: `Cache` holds, for one buffer revision, whether each range asked about has
//!   an enabled action; a new revision (or a publish for the buffer) empties it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::buffer::BufferId;
use crate::buffer::pos::{ByteIdx, PosEncoding};
use crate::layout::GroupId;
use crate::lsp::ServerId;

/// How long the cursor rests before the lightbulb's request goes out.
pub const BULB_DELAY: Duration = Duration::from_millis(250);
/// The lightbulb drawn in the gutter (two cells wide).
pub const BULB: &str = "💡";
/// The list's most rows before it scrolls.
pub const MAX_ROWS: usize = 12;
/// Its widest content (cells).
pub const MAX_WIDTH: usize = 70;
/// The hover's row that opens the list for its diagnostics (VS Code's).
pub const QUICK_FIX_ROW: &str = "Quick Fix…";

/// One code action (or a bare command).
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    /// Why it can't be applied (`disabled.reason`).
    pub disabled: Option<String>,
    /// Its `WorkspaceEdit`, as sent.
    pub edit: Option<Value>,
    /// The command to run (after the edit): `{ title, command, arguments }`.
    pub command: Option<Value>,
    /// A `CodeAction` literal (it may be resolved), not a bare `Command`.
    pub literal: bool,
    /// As the server sent it (`codeAction/resolve`'s params).
    pub raw: Value,
}

impl Action {
    /// One item of the answer; `None` when it has no title.
    pub fn read(v: &Value) -> Option<Action> {
        let title = v.get("title")?.as_str()?.to_string();
        // A bare `Command` has a string `command`; a `CodeAction`'s is an object.
        if let Some(command) = v.get("command").and_then(Value::as_str) {
            return Some(Action {
                title,
                kind: None,
                preferred: false,
                disabled: None,
                edit: None,
                command: Some(json!({
                    "title": v.get("title").cloned().unwrap_or(Value::Null),
                    "command": command,
                    "arguments": v.get("arguments").cloned().unwrap_or(Value::Null),
                })),
                literal: false,
                raw: v.clone(),
            });
        }
        let present = |key: &str| v.get(key).filter(|x| !x.is_null()).cloned();
        Some(Action {
            title,
            kind: v.get("kind").and_then(Value::as_str).map(String::from),
            preferred: v.get("isPreferred").and_then(Value::as_bool) == Some(true),
            disabled: v.get("disabled").filter(|d| !d.is_null()).map(|d| {
                d.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("Not available")
                    .to_string()
            }),
            edit: present("edit"),
            command: present("command").filter(|c| c.get("command").and_then(Value::as_str).is_some()),
            literal: true,
            raw: v.clone(),
        })
    }

    pub fn is_quick_fix(&self) -> bool {
        self.kind
            .as_deref()
            .is_some_and(|k| k == "quickfix" || k.starts_with("quickfix."))
    }

    /// Its group in the list: quick fixes, refactorings, source actions, the rest.
    fn group(&self) -> u8 {
        let kind = self.kind.as_deref().unwrap_or("");
        let is = |base: &str| kind == base || kind.strip_prefix(base).is_some_and(|r| r.starts_with('.'));
        if is("quickfix") {
            0
        } else if is("refactor") {
            1
        } else if is("source") {
            2
        } else {
            3
        }
    }

    /// `workspace/executeCommand`'s params for its command.
    pub fn execute_params(&self) -> Option<Value> {
        let c = self.command.as_ref()?;
        let mut params = json!({ "command": c.get("command")? });
        if let Some(args) = c.get("arguments").filter(|a| !a.is_null()) {
            params["arguments"] = args.clone();
        }
        Some(params)
    }

    /// The server resolves its edit (`codeAction/resolve`) before it's applied: a literal without
    /// one, when the server can.
    pub fn needs_resolve(&self, server_resolves: bool) -> bool {
        server_resolves && self.literal && self.edit.is_none()
    }
}

/// A `textDocument/codeAction` answer, sorted (`sort`).
pub fn parse(result: &Value) -> Vec<Action> {
    let mut actions: Vec<Action> = result
        .as_array()
        .map(|a| a.iter().filter_map(Action::read).collect())
        .unwrap_or_default();
    sort(&mut actions);
    actions
}

/// Quick fixes first, then refactorings, source actions and the rest; the preferred first in each
/// group; the server's order otherwise.
pub fn sort(actions: &mut [Action]) {
    actions.sort_by_key(|a| (a.group(), !a.preferred));
}

/// Whether any of `actions` can be applied.
pub fn any_enabled(actions: &[Action]) -> bool {
    actions.iter().any(|a| a.disabled.is_none())
}

/// Who asked for code actions, and so where the answer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Cmd+.: the list.
    List,
    /// The cursor's rest: the lightbulb.
    Bulb,
    /// The hover's diagnostics: its "Quick Fix…" row.
    Hover,
}

/// The server an action list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub server: ServerId,
    pub encoding: PosEncoding,
    /// It answers `codeAction/resolve`.
    pub resolve: bool,
}

/// The list on screen.
#[derive(Debug, Clone)]
pub struct Popup {
    pub group: GroupId,
    pub buffer: BufferId,
    /// The buffer's revision when it opened: an edit closes it.
    pub revision: u64,
    /// The primary cursor when it opened: moving it closes the list.
    pub cursor: ByteIdx,
    /// Where it's drawn from (below that row).
    pub anchor: ByteIdx,
    pub source: Source,
    pub actions: Vec<Action>,
    /// Index into `actions`.
    pub selected: usize,
    /// The first row shown.
    pub top: usize,
    /// The rows drawn last (set by drawing).
    pub rows: usize,
}

impl Popup {
    /// The list of `actions` (not empty), the first enabled one selected.
    pub fn new(
        (group, buffer, revision): (GroupId, BufferId, u64),
        cursor: ByteIdx,
        anchor: ByteIdx,
        source: Source,
        actions: Vec<Action>,
    ) -> Self {
        let selected = actions.iter().position(|a| a.disabled.is_none()).unwrap_or(0);
        let mut p = Self {
            group,
            buffer,
            revision,
            cursor,
            anchor,
            source,
            actions,
            selected,
            top: 0,
            rows: 0,
        };
        p.reveal();
        p
    }

    /// Moves the selection `delta` rows, wrapping round.
    pub fn move_by(&mut self, delta: isize) {
        let n = self.actions.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.reveal();
    }

    fn page(&self) -> usize {
        if self.rows > 0 { self.rows } else { MAX_ROWS }
    }

    /// Scrolls the least to show the selection.
    fn reveal(&mut self) {
        let rows = self.page();
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
        self.top = self.top.min(self.actions.len().saturating_sub(rows));
    }

    /// Scrolls the rows (the wheel) without moving the selection.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.actions.len().saturating_sub(self.page());
        self.top = (self.top as isize + delta).clamp(0, max as isize) as usize;
    }
}

/// Where the lightbulb's request is asked: a buffer revision and the primary selection's range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub group: GroupId,
    pub buffer: BufferId,
    pub revision: u64,
    pub range: (ByteIdx, ByteIdx),
}

/// What the lightbulb's requests found, for one buffer revision.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    pub buffer: Option<BufferId>,
    pub revision: u64,
    /// Range → an enabled action exists there.
    pub ranges: HashMap<(ByteIdx, ByteIdx), bool>,
}

impl Cache {
    /// What's known about `range` of `buffer` at `revision`.
    pub fn get(&self, buffer: BufferId, revision: u64, range: (ByteIdx, ByteIdx)) -> Option<bool> {
        if self.buffer != Some(buffer) || self.revision != revision {
            return None;
        }
        self.ranges.get(&range).copied()
    }

    pub fn put(&mut self, buffer: BufferId, revision: u64, range: (ByteIdx, ByteIdx), has: bool) {
        if self.buffer != Some(buffer) || self.revision != revision {
            *self = Self {
                buffer: Some(buffer),
                revision,
                ranges: HashMap::new(),
            };
        }
        self.ranges.insert(range, has);
    }

    /// Forgets `buffer`'s (its diagnostics changed: the actions may have too).
    pub fn forget(&mut self, buffer: BufferId) {
        if self.buffer == Some(buffer) {
            self.ranges.clear();
        }
    }
}

/// The quick fixes the server offers for the hover popup's diagnostics.
#[derive(Debug, Clone)]
pub struct HoverFixes {
    pub group: GroupId,
    pub buffer: BufferId,
    pub revision: u64,
    /// The diagnostics' range (where the list opens).
    pub range: (ByteIdx, ByteIdx),
    pub source: Source,
    pub actions: Vec<Action>,
}

/// Code actions' state: the list, the lightbulb's rest, request and cache, the hover's fixes.
#[derive(Debug, Default)]
pub struct CodeActions {
    pub popup: Option<Popup>,
    /// The cursor resting: the lightbulb's request goes out at the deadline.
    pub rest: Option<(Instant, Place)>,
    /// The lightbulb request in flight.
    pub asking: Option<Place>,
    pub cache: Cache,
    pub hover: Option<HoverFixes>,
    /// The action applied once its `codeAction/resolve` answers.
    pub resolving: Option<(Source, Action)>,
    /// Where the lightbulb was drawn last (group, its cells), for clicks (set by drawing).
    pub bulb_hit: Option<(GroupId, ratatui::layout::Rect)>,
}

impl CodeActions {
    pub fn deadline(&self) -> Option<Instant> {
        self.rest.map(|(d, _)| d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(actions: &[Action]) -> Vec<&str> {
        actions.iter().map(|a| a.title.as_str()).collect()
    }

    /// Literals and bare commands; kinds, preferred, disabled; quick fixes first, the preferred
    /// first in each group, the server's order otherwise.
    #[test]
    fn parse_reads_and_sorts() {
        let answer = json!([
            { "title": "Extract function", "kind": "refactor.extract" },
            { "title": "Run thing", "command": "x.run", "arguments": [1, "a"] },
            { "title": "Add import", "kind": "quickfix", "edit": { "changes": {} } },
            { "title": "Organize imports", "kind": "source.organizeImports" },
            { "title": "Fix typo", "kind": "quickfix", "isPreferred": true,
              "command": { "title": "Fix", "command": "x.fix" } },
            { "title": "Inline", "kind": "refactor.inline", "disabled": { "reason": "Not here" } },
            { "title": "Quick again", "kind": "quickfix.other" },
            { "kind": "quickfix" },
        ]);
        let actions = parse(&answer);
        assert_eq!(
            titles(&actions),
            [
                "Fix typo",
                "Add import",
                "Quick again",
                "Extract function",
                "Inline",
                "Organize imports",
                "Run thing"
            ]
        );
        let fix = &actions[0];
        assert!(fix.preferred && fix.is_quick_fix() && fix.literal);
        assert_eq!(fix.execute_params(), Some(json!({ "command": "x.fix" })));
        assert!(fix.needs_resolve(true), "no edit: resolved first");
        assert!(!fix.needs_resolve(false), "not when the server can't");
        assert!(!actions[1].needs_resolve(true), "has its edit");
        assert_eq!(actions[4].disabled.as_deref(), Some("Not here"));
        let run = &actions[6];
        assert!(!run.literal && run.kind.is_none() && !run.needs_resolve(true));
        assert_eq!(
            run.execute_params(),
            Some(json!({ "command": "x.run", "arguments": [1, "a"] }))
        );
        assert!(parse(&Value::Null).is_empty());
        assert!(any_enabled(&actions));
        assert!(!any_enabled(&actions[4..5]));
    }

    /// The selection starts on the first enabled action and wraps round.
    #[test]
    fn popup_selection() {
        let actions = parse(&json!([
            { "title": "a", "disabled": { "reason": "no" } },
            { "title": "b" },
            { "title": "c" },
        ]));
        let source = Source {
            server: ServerId::test(1),
            encoding: PosEncoding::Utf8,
            resolve: false,
        };
        let place: (GroupId, BufferId, u64) = (0, 1, 0);
        let mut p = Popup::new(place, ByteIdx(0), ByteIdx(0), source, actions);
        assert_eq!(p.actions[p.selected].title, "b");
        p.move_by(1);
        assert_eq!(p.selected, 2);
        p.move_by(1);
        assert_eq!(p.selected, 0, "wraps");
        p.move_by(-1);
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn cache_is_per_revision() {
        let mut c = Cache::default();
        let (b, r): (BufferId, _) = (1, (ByteIdx(1), ByteIdx(1)));
        assert_eq!(c.get(b, 3, r), None);
        c.put(b, 3, r, true);
        assert_eq!(c.get(b, 3, r), Some(true));
        assert_eq!(c.get(b, 4, r), None, "another revision");
        c.put(b, 4, (ByteIdx(2), ByteIdx(2)), false);
        assert_eq!(c.get(b, 3, r), None, "the old revision's are gone");
        c.forget(b);
        assert_eq!(c.get(b, 4, (ByteIdx(2), ByteIdx(2))), None);
    }
}
