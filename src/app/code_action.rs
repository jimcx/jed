//! Code actions (`app.code_actions`, a `code_action::CodeActions`; the list drawn by
//! `ui::code_action`, the lightbulb by `ui::mod`): Cmd+. (`editor.action.quickFix`) asks
//! `textDocument/codeAction` for the primary selection (empty: the cursor) with the diagnostics
//! there, the answer becomes a list at the cursor; applying an action runs its `WorkspaceEdit`
//! (one undo step per file, as rename's) and then its command (`workspace/executeCommand`). A
//! literal without an edit is resolved first (`codeAction/resolve`) when the server can.
//!
//! - `context.diagnostics` are that server's own, as it published them (`Diagnostic::raw`:
//!   `code`, `data`, …) with the range where they are now; other servers' aren't sent.
//! - The list closes on Esc, a click elsewhere, an edit, the cursor moving, the tab or focus
//!   changing (`check_code_actions`); keys it doesn't bind go to the editor.
//! - A server's `workspace/applyEdit` (what a command usually sends back) is applied the same way
//!   and answered `{ applied }` (`server_request`).
//! - The lightbulb: after the cursor rests `code_action::BULB_DELAY` on a place (buffer revision,
//!   primary selection) not asked about yet, an automatic request goes out; the gutter shows
//!   `💡` on the cursor line while the answer for the current place has an enabled action. The
//!   answers are cached by revision (moving back to a place asks nothing); a publish for the
//!   buffer forgets them. A click on it opens the list.
//! - The hover: when its popup shows diagnostics, their quick fixes are asked for (`only:
//!   quickfix`); if there are any, a "Quick Fix…" row ends the diagnostics section, and a click
//!   on it opens the list with those fixes at the first diagnostic.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use lsp_types::{CodeActionProviderCapability, ServerCapabilities};
use serde_json::{Value, json};

use super::lsp::{Pending, Request};
use super::{App, Focus, Level};
use crate::buffer::BufferId;
use crate::buffer::pos::{self, ByteIdx};
use crate::code_action::{self, Action, HoverFixes, Place, Popup, Purpose, Source};
use crate::command::CodeActionCommand;
use crate::diagnostics::{self, Mark};
use crate::layout::{GroupId, Hit};
use crate::lsp::ServerId;
use crate::lsp::rpc::{Id, METHOD_NOT_FOUND, RpcError};
use crate::rename;

/// Wheel rows per notch over the list.
const WHEEL_ROWS: isize = 3;

pub(super) fn supports_code_actions(caps: &ServerCapabilities) -> bool {
    match &caps.code_action_provider {
        Some(CodeActionProviderCapability::Simple(on)) => *on,
        Some(CodeActionProviderCapability::Options(_)) => true,
        None => false,
    }
}

fn resolves(caps: &ServerCapabilities) -> bool {
    matches!(&caps.code_action_provider, Some(CodeActionProviderCapability::Options(o)) if o.resolve_provider == Some(true))
}

/// VS Code's message when the server has nothing there.
const NONE_AVAILABLE: &str = "No code actions available";

/// The union of the marks' drawn ranges (what the hover's fixes are asked for).
pub(super) fn marks_range(rope: &ropey::Rope, marks: &[Mark]) -> Option<(ByteIdx, ByteIdx)> {
    let mut peek = pos::Peek::new(rope);
    marks.iter().fold(None, |acc, m| {
        let (s, e) = diagnostics::extent(rope, &mut peek, m);
        Some(match acc {
            None => (s, e),
            Some((a, b)) => (a.min(s), b.max(e)),
        })
    })
}

impl App {
    /// The editor's active text tab while it has the keys: its group, buffer and primary
    /// selection's range (start, end) and head.
    fn action_target(&self) -> Option<(GroupId, BufferId, (ByteIdx, ByteIdx), ByteIdx)> {
        if self.ui.focus != Focus::Editor || self.palette.is_some() || self.prompt.is_some() {
            return None;
        }
        let tab = self.group().active_tab()?;
        let p = tab.view.selections.primary();
        Some((self.groups.active, tab.buffer, (p.start(), p.end()), p.head))
    }

    /// Whether `server` (running) resolves code actions.
    fn server_resolves(&self, server: ServerId) -> bool {
        self.lsp.servers().iter().any(|s| {
            s.id == server
                && matches!(&s.state, crate::lsp::State::Running { capabilities, .. } if resolves(capabilities))
        })
    }

    /// Cmd+.: the code actions for the primary selection (the cursor), in a list there.
    pub(super) fn quick_fix(&mut self) {
        let Some((group, buffer, range, head)) = self.action_target() else {
            return;
        };
        self.close_completion();
        self.close_hover();
        self.code_actions.popup = None;
        self.lsp.sync_documents(&mut self.buffers);
        let supported = self
            .lsp
            .server_of(buffer)
            .is_some_and(|(_, _, caps)| supports_code_actions(caps));
        let Some(buf) = self.buffers.get(buffer).filter(|_| supported) else {
            self.notify(Level::Info, "No language server offers code actions here".into());
            return;
        };
        let rope = &buf.rope;
        let marks: Vec<Mark> = if range.0 == range.1 {
            buf.diagnostics.touching(rope, head).into_iter().cloned().collect()
        } else {
            buf.diagnostics
                .overlapping(range.0, range.1)
                .filter(|m| m.start < range.1 && (m.end > range.0 || m.start == m.end))
                .cloned()
                .collect()
        };
        if !self.request_code_actions(Purpose::List, (group, buffer, head), range, &marks) {
            self.notify(Level::Info, "No language server offers code actions here".into());
        }
    }

    /// Sends `textDocument/codeAction` for `range` of `buffer` with those of `marks` from the
    /// buffer's server (as it published them, at their place now). False when no running server
    /// with code actions has the document.
    fn request_code_actions(
        &mut self,
        purpose: Purpose,
        (group, buffer, head): (GroupId, BufferId, ByteIdx),
        range: (ByteIdx, ByteIdx),
        marks: &[Mark],
    ) -> bool {
        self.lsp.sync_documents(&mut self.buffers);
        let Some((server, encoding, _)) = self.lsp.server_of(buffer) else {
            return false;
        };
        let Some(buf) = self.buffers.get(buffer) else {
            return false;
        };
        let rope = &buf.rope;
        let lsp_range = |(s, e): (ByteIdx, ByteIdx)| {
            let (s, e) = (pos::lsp_pos(rope, s, encoding), pos::lsp_pos(rope, e, encoding));
            json!({
                "start": { "line": s.line, "character": s.character },
                "end": { "line": e.line, "character": e.character },
            })
        };
        let diagnostics: Vec<Value> = marks
            .iter()
            .filter(|m| m.server == server)
            .map(|m| {
                let mut raw = match &m.diag.raw {
                    Value::Object(_) => m.diag.raw.clone(),
                    _ => json!({ "message": m.diag.message }),
                };
                raw["range"] = lsp_range((m.start, m.end));
                raw
            })
            .collect();
        let mut context = json!({
            "diagnostics": diagnostics,
            // Invoked (1) by Cmd+. and the hover; automatic (2) for the lightbulb.
            "triggerKind": if purpose == Purpose::Bulb { 2 } else { 1 },
        });
        if purpose == Purpose::Hover {
            context["only"] = json!(["quickfix"]);
        }
        let extra = json!({ "range": lsp_range(range), "context": context });
        self.send_doc_request(
            (group, buffer, head),
            "textDocument/codeAction",
            supports_code_actions,
            Request::CodeActions { purpose, range },
            extra,
        )
    }

    /// A code action answer (current: `answer` checked), to whoever asked.
    pub(super) fn code_actions_answer(
        &mut self,
        p: &Pending,
        purpose: Purpose,
        range: (ByteIdx, ByteIdx),
        value: &Value,
    ) {
        let actions = code_action::parse(value);
        let source = Source {
            server: p.server,
            encoding: p.encoding,
            resolve: self.server_resolves(p.server),
        };
        match purpose {
            Purpose::List => {
                if actions.is_empty() {
                    self.notify(Level::Info, NONE_AVAILABLE.into());
                    return;
                }
                self.close_completion();
                self.close_hover();
                let place = (p.group, p.buffer, p.revision);
                self.code_actions.popup = Some(Popup::new(place, p.at, p.at, source, actions));
            }
            Purpose::Bulb => self.bulb_answer(p, range, code_action::any_enabled(&actions)),
            Purpose::Hover => {
                let fixes: Vec<Action> = actions.into_iter().filter(Action::is_quick_fix).collect();
                if !code_action::any_enabled(&fixes) {
                    return;
                }
                self.code_actions.hover = Some(HoverFixes {
                    group: p.group,
                    buffer: p.buffer,
                    revision: p.revision,
                    range,
                    source,
                    actions: fixes,
                });
                self.add_quick_fix_row();
            }
        }
        self.needs_redraw = true;
    }

    /// A code action request failed (not a routine error): the list says so; the lightbulb's
    /// place counts as having none.
    pub(super) fn code_actions_failed(&mut self, p: &Pending, purpose: Purpose, range: (ByteIdx, ByteIdx), why: &str) {
        match purpose {
            Purpose::List => self.notify(Level::Error, format!("Code actions failed: {why}")),
            Purpose::Bulb => self.bulb_answer(p, range, false),
            Purpose::Hover => {}
        }
    }

    /// Whether the list is shown over the active editor (its keys apply: `Context::CodeActions`).
    pub(super) fn code_actions_shown(&self) -> bool {
        let Some(p) = &self.code_actions.popup else {
            return false;
        };
        self.action_target()
            .is_some_and(|(g, b, ..)| (g, b) == (p.group, p.buffer))
    }

    /// The list's keys.
    pub(super) fn code_action_command(&mut self, c: CodeActionCommand) {
        let Some(p) = &mut self.code_actions.popup else { return };
        match c {
            CodeActionCommand::Up => p.move_by(-1),
            CodeActionCommand::Down => p.move_by(1),
            CodeActionCommand::Accept => self.accept_code_action(None),
            CodeActionCommand::Close => self.close_code_actions(),
        }
    }

    pub(super) fn close_code_actions(&mut self) {
        if self.code_actions.popup.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// Applies the selected action (or row `row`'s): a disabled one only says why not; one the
    /// server must resolve first is resolved, then applied.
    fn accept_code_action(&mut self, row: Option<usize>) {
        let Some(popup) = &mut self.code_actions.popup else {
            return;
        };
        let index = row.unwrap_or(popup.selected);
        let Some(action) = popup.actions.get(index).cloned() else {
            return;
        };
        if let Some(reason) = &action.disabled {
            popup.selected = index;
            self.notify(Level::Info, reason.clone());
            return;
        }
        let popup = self.code_actions.popup.take().expect("checked");
        self.needs_redraw = true;
        let place = (popup.group, popup.buffer, popup.cursor);
        if action.needs_resolve(popup.source.resolve) {
            let s = popup.source;
            self.code_actions.resolving = Some((s, action.clone()));
            if !self.send_server_request(
                s.server,
                s.encoding,
                place,
                "codeAction/resolve",
                action.raw.clone(),
                Request::ResolveAction,
            ) {
                self.notify(Level::Error, "Code action failed: the language server stopped".into());
            }
            return;
        }
        self.run_code_action(popup.source, place, &action);
    }

    /// A `codeAction/resolve` answer (current): the action with what it filled in, applied.
    pub(super) fn resolve_action_answer(&mut self, p: &Pending, value: &Value) {
        let Some((source, mut action)) = self.code_actions.resolving.take() else {
            return;
        };
        if let Some(edit) = value.get("edit").filter(|e| !e.is_null()) {
            action.edit = Some(edit.clone());
        }
        if let Some(c) = value
            .get("command")
            .filter(|c| c.get("command").and_then(Value::as_str).is_some())
        {
            action.command = Some(c.clone());
        }
        if action.edit.is_none() && action.command.is_none() {
            self.notify(Level::Info, format!("'{}' has nothing to apply", action.title));
            return;
        }
        self.run_code_action(source, (p.group, p.buffer, p.at), &action);
    }

    /// Runs `action`: its edit, then (if the edit applied) its command.
    fn run_code_action(&mut self, source: Source, place: (GroupId, BufferId, ByteIdx), action: &Action) {
        if let Some(edit) = &action.edit {
            let applied = rename::parse_workspace_edit(edit)
                .and_then(|files| self.apply_workspace_edit(files, source.encoding, "Code action"));
            if let Err(e) = applied {
                self.notify(Level::Error, format!("Code action failed: {e}"));
                return;
            }
        }
        if let Some(params) = action.execute_params() {
            let request = Request::ExecuteCommand {
                title: action.title.clone(),
            };
            if !self.send_server_request(
                source.server,
                source.encoding,
                place,
                "workspace/executeCommand",
                params,
                request,
            ) {
                self.notify(Level::Error, "Code action failed: the language server stopped".into());
            }
        }
    }

    /// A request the server sent: `workspace/applyEdit` is applied and answered `{ applied }`
    /// (with the reason when it isn't); anything else is MethodNotFound.
    pub(super) fn server_request(&mut self, server: ServerId, id: Id, method: &str, params: &Value) {
        if method != "workspace/applyEdit" {
            let error = RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("jed doesn't handle {method}"),
            };
            self.lsp.respond(server, id, Err(error));
            return;
        }
        let Some(encoding) = self.lsp.encoding_of(server) else {
            return;
        };
        let edit = params.get("edit").unwrap_or(&Value::Null);
        let applied =
            rename::parse_workspace_edit(edit).and_then(|files| self.apply_workspace_edit(files, encoding, "Edit"));
        let result = match applied {
            Ok(()) => json!({ "applied": true }),
            Err(e) => {
                let label = params.get("label").and_then(Value::as_str).unwrap_or("Edit");
                self.notify(Level::Error, format!("{label} not applied: {e}"));
                json!({ "applied": false, "failureReason": e })
            }
        };
        self.lsp.respond(server, id, Ok(result));
        self.needs_redraw = true;
    }

    /// After every event: the list closes when its editor lost the keys, its tab changed, the
    /// buffer was edited or the cursor moved; the hover's fixes go with their popup; the
    /// lightbulb's rest starts on a place not asked about yet.
    pub(super) fn check_code_actions(&mut self) {
        if let Some(p) = &self.code_actions.popup {
            let fits = self.action_target().is_some_and(|(g, b, _, head)| {
                (g, b) == (p.group, p.buffer)
                    && head == p.cursor
                    && self.buffers.get(b).is_some_and(|buf| buf.revision() == p.revision)
            });
            if !fits {
                self.close_code_actions();
            }
        }
        if let Some(h) = &self.code_actions.hover {
            let fits = self
                .hover
                .popup
                .as_ref()
                .is_some_and(|p| (p.group, p.buffer, p.revision) == (h.group, h.buffer, h.revision));
            if !fits {
                self.code_actions.hover = None;
            }
        }
        // The lightbulb's request was cancelled or dropped (the place moved on): nothing is
        // asking any more, so coming back to that place asks again.
        if self.code_actions.asking.is_some()
            && !self.requests.values().any(|p| {
                matches!(
                    p.request,
                    Request::CodeActions {
                        purpose: Purpose::Bulb,
                        ..
                    }
                )
            })
        {
            self.code_actions.asking = None;
        }
        let place = self.bulb_place();
        let ca = &mut self.code_actions;
        ca.rest = match place {
            None => None,
            Some(place)
                if ca.cache.get(place.buffer, place.revision, place.range).is_some() || ca.asking == Some(place) =>
            {
                None
            }
            Some(place) => match ca.rest {
                Some((deadline, p)) if p == place => Some((deadline, p)),
                _ => Some((std::time::Instant::now() + code_action::BULB_DELAY, place)),
            },
        };
    }

    /// Where the lightbulb applies: the active editor's buffer revision and primary selection,
    /// while it has the keys and its server offers code actions.
    fn bulb_place(&self) -> Option<Place> {
        let (group, buffer, range, _) = self.action_target()?;
        let buf = self
            .buffers
            .get(buffer)
            .filter(|b| b.has_file() && b.read_only.is_none())?;
        let (_, _, caps) = self.lsp.server_of(buffer)?;
        supports_code_actions(caps).then_some(Place {
            group,
            buffer,
            revision: buf.revision(),
            range,
        })
    }

    /// The line the lightbulb is drawn on (in the active group), if its place has actions.
    pub fn bulb_line(&self) -> Option<(GroupId, usize)> {
        let place = self.bulb_place()?;
        if self.code_actions.cache.get(place.buffer, place.revision, place.range) != Some(true) {
            return None;
        }
        let tab = self.group().active_tab()?;
        let rope = &self.buffers.get(place.buffer)?.rope;
        Some((place.group, pos::line_of(rope, tab.view.selections.primary().head)))
    }

    /// The lightbulb's rest passed `now`: its request, if the place is still the same.
    pub(super) fn bulb_tick(&mut self, now: std::time::Instant) {
        let Some((deadline, place)) = self.code_actions.rest else {
            return;
        };
        if deadline > now {
            return;
        }
        self.code_actions.rest = None;
        if self.bulb_place() != Some(place) {
            return;
        }
        let Some(buf) = self.buffers.get(place.buffer) else {
            return;
        };
        let head = self
            .group()
            .active_tab()
            .map_or(place.range.0, |t| t.view.selections.primary().head);
        let marks: Vec<Mark> = buf.diagnostics.touching(&buf.rope, head).into_iter().cloned().collect();
        if self.request_code_actions(Purpose::Bulb, (place.group, place.buffer, head), place.range, &marks) {
            self.code_actions.asking = Some(place);
        } else {
            self.code_actions
                .cache
                .put(place.buffer, place.revision, place.range, false);
        }
    }

    fn bulb_answer(&mut self, p: &Pending, range: (ByteIdx, ByteIdx), has: bool) {
        let ca = &mut self.code_actions;
        ca.asking
            .take_if(|a| (a.buffer, a.revision, a.range) == (p.buffer, p.revision, range));
        ca.cache.put(p.buffer, p.revision, range, has);
        self.needs_redraw = true;
    }

    /// The buffer at `path` got new diagnostics: the lightbulb asks again.
    pub(super) fn diagnostics_changed(&mut self, buffer: BufferId) {
        self.code_actions.cache.forget(buffer);
    }

    /// The hover popup shows `marks` of `buffer` (in `group`): asks for their quick fixes unless
    /// they're known already.
    pub(super) fn request_hover_fixes(&mut self, group: GroupId, buffer: BufferId, marks: &[Mark]) {
        let Some(buf) = self.buffers.get(buffer) else { return };
        let Some(range) = marks_range(&buf.rope, marks) else {
            return;
        };
        let revision = buf.revision();
        let known = self
            .code_actions
            .hover
            .as_ref()
            .is_some_and(|h| (h.group, h.buffer, h.revision, h.range) == (group, buffer, revision, range));
        if known {
            return;
        }
        self.code_actions.hover = None;
        let head = self
            .groups
            .get(group)
            .and_then(|g| g.active_tab())
            .map_or(range.0, |t| t.view.selections.primary().head);
        self.request_code_actions(Purpose::Hover, (group, buffer, head), range, marks);
    }

    /// The hover's fixes, if they're for the popup about to show `marks` of `buffer` at
    /// `revision` in `group`.
    pub(super) fn hover_fixes_for(&self, group: GroupId, buffer: BufferId, revision: u64, marks: &[Mark]) -> bool {
        let Some(h) = &self.code_actions.hover else {
            return false;
        };
        let Some(buf) = self.buffers.get(buffer) else {
            return false;
        };
        (h.group, h.buffer, h.revision) == (group, buffer, revision) && marks_range(&buf.rope, marks) == Some(h.range)
    }

    /// The fixes came after the popup opened: the row goes in after its diagnostics.
    fn add_quick_fix_row(&mut self) {
        let Some(h) = &self.code_actions.hover else { return };
        let Some(p) = &mut self.hover.popup else { return };
        // Only while the popup shows the diagnostics they're for (not after a publish took them).
        if (p.group, p.buffer, p.revision) != (h.group, h.buffer, h.revision)
            || p.diagnostics != Some(h.range)
            || p.lines.iter().any(|l| l.kind == crate::hover::Kind::Link)
        {
            return;
        }
        p.lines
            .insert(p.fixes_at, crate::hover::Line::link(code_action::QUICK_FIX_ROW));
        self.needs_redraw = true;
    }

    /// A click on the hover's "Quick Fix…" row: the list of its fixes at the first diagnostic.
    pub(super) fn open_hover_fixes(&mut self) {
        let Some(h) = self.code_actions.hover.take() else {
            return;
        };
        self.close_hover();
        if self.buffers.get(h.buffer).is_none_or(|b| b.revision() != h.revision) {
            return;
        }
        self.focus_group(h.group);
        self.ui.focus = Focus::Editor;
        let Some(tab) = self.group().active_tab().filter(|t| t.buffer == h.buffer) else {
            return;
        };
        let cursor = tab.view.selections.primary().head;
        let place = (h.group, h.buffer, h.revision);
        self.code_actions.popup = Some(Popup::new(place, cursor, h.range.0, h.source, h.actions));
        self.needs_redraw = true;
    }

    /// A mouse event, before the completion list and the hover see it: true when the list takes
    /// it (it's over the list: a click applies that row's action, the wheel scrolls). A click or
    /// the wheel elsewhere closes the list.
    pub(super) fn code_actions_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        if self.code_actions.popup.is_none() {
            return false;
        }
        let over = hit == Hit::CodeActions;
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -WHEEL_ROWS
                } else {
                    WHEEL_ROWS
                };
                if let Some(p) = &mut self.code_actions.popup {
                    p.scroll_by(delta);
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) if over => {
                if let Some(h) = self.hits.code_actions
                    && m.row >= h.rows.y
                    && m.row < h.rows.y + h.rows.height
                {
                    self.accept_code_action(Some(h.top + (m.row - h.rows.y) as usize));
                }
                true
            }
            _ if over => true,
            MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.close_code_actions();
                false
            }
            _ => false,
        }
    }

    /// A click on the lightbulb (the last frame's): the list, as Cmd+. there.
    pub(super) fn bulb_click(&mut self, m: &MouseEvent) -> bool {
        let Some((group, rect)) = self.code_actions.bulb_hit else {
            return false;
        };
        if !rect.contains(ratatui::layout::Position { x: m.column, y: m.row }) {
            return false;
        }
        self.focus_group(group);
        self.ui.focus = Focus::Editor;
        self.quick_fix();
        true
    }
}
