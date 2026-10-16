//! Completion (`app.completion`, a `completion::Completion`; drawn by `ui::completion`): asking
//! (Ctrl+Space, a trigger character, typing a word: quick suggestions after
//! `completion::QUICK_DELAY` through `next_timer`), the answer becoming the list, filtering as
//! the word grows or shrinks, the list's keys and mouse, inserting the selected item at every
//! cursor, and `completionItem/resolve`.
//!
//! - The request is asked at the primary cursor and stays current while that cursor stays in the
//!   word it was asked in (typing on doesn't make it stale; the answer is filtered by what was
//!   typed since). A newer request cancels it.
//! - `check_completion` (after every event) closes the list when the editor loses the keys, the
//!   tab changes or the cursor leaves the word (a non-word char typed, a click, Backspace past its
//!   start); else refilters, and an incomplete list asks again when the word changed. A list
//!   with no matching item closes (an incomplete one waits for the next answer).
//! - Typing a commit character of the selected item inserts it first, then the char.
//! - Resolve: the selected item is resolved when the server supports it (its detail; additional
//!   edits). Accepting uses what the item has; if it was never resolved and has no additional
//!   edits, a resolve goes out after inserting it and its additional edits (auto-imports) are
//!   applied when it answers, as their own undo step, if the buffer is still as the insert left it.

use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use lsp_types::ServerCapabilities;
use serde_json::{Value, json};

use super::lsp::{Pending, Request};
use super::{App, Focus, Level};
use crate::buffer::BufferId;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx, LspPos};
use crate::buffer::undo::EditKind;
use crate::command::SuggestCommand;
use crate::completion::{self, Anchor, Ask, Insertion, Popup, Quick, Trigger};
use crate::layout::{GroupId, Hit};
use crate::syntax::Scope;

/// Wheel rows per notch over the list.
const WHEEL_ROWS: isize = 3;

fn supports_completion(caps: &ServerCapabilities) -> bool {
    caps.completion_provider.is_some()
}

impl App {
    /// The editor's cursor while it has the keys: its group, buffer and primary cursor.
    fn editor_cursor(&self) -> Option<(GroupId, BufferId, ByteIdx)> {
        if self.ui.focus != Focus::Editor || self.palette.is_some() || self.prompt.is_some() {
            return None;
        }
        let tab = self.group().active_tab()?;
        Some((self.groups.active, tab.buffer, tab.view.selections.primary().head))
    }

    /// The list is shown and belongs to the active editor (its keys apply: `Context::Suggest`).
    pub(super) fn completion_shown(&self) -> bool {
        let Some(p) = self.completion.popup.as_ref().filter(|p| !p.shown.is_empty()) else {
            return false;
        };
        self.editor_cursor()
            .is_some_and(|(g, b, _)| (g, b) == (p.anchor.group, p.anchor.buffer))
    }

    /// Ctrl+Space: asks at the cursor ("No suggestions." when no server can).
    pub(super) fn trigger_suggest(&mut self) {
        if !self.ask_completion(Trigger::Invoked) {
            self.notify(Level::Info, "No suggestions.".into());
        }
    }

    /// Asks for completions at the active view's primary cursor, replacing a pending request.
    /// False when no running server with completion has the document.
    fn ask_completion(&mut self, trigger: Trigger) -> bool {
        self.completion.quick = None;
        let Some((group, buffer, at)) = self.editor_cursor() else {
            return false;
        };
        let Some(buf) = self.buffers.get(buffer) else {
            return false;
        };
        let word = match trigger {
            Trigger::Character(_) => 0,
            _ => completion::word_before(&buf.rope, at),
        };
        self.completion.generation += 1;
        let generation = self.completion.generation;
        let extra = json!({ "context": trigger.context() });
        let request = Request::Completion { generation };
        if !self.send_request_with(
            group,
            buffer,
            at,
            "textDocument/completion",
            supports_completion,
            request,
            extra,
        ) {
            self.completion.ask = None;
            return false;
        }
        let (Some((server, encoding, _)), Some(buf)) = (self.lsp.server_of(buffer), self.buffers.get(buffer)) else {
            return false;
        };
        let text = (buf.rope.clone(), buf.sets_applied());
        let anchor = Anchor::new((group, buffer), (server, encoding), text, at, word);
        self.completion.ask = Some(Ask {
            generation,
            anchor,
            trigger,
        });
        true
    }

    /// Whether the completion request `generation` is still wanted: it's the latest, and the
    /// editor's cursor is still in the word it was asked in.
    pub(super) fn completion_ask_current(&self, generation: u64) -> bool {
        let Some(ask) = self.completion.ask.as_ref().filter(|a| a.generation == generation) else {
            return false;
        };
        let a = &ask.anchor;
        self.editor_cursor().is_some_and(|(g, b, head)| {
            (g, b) == (a.group, a.buffer) && self.buffers.get(b).is_some_and(|buf| a.in_word(&buf.rope, head))
        })
    }

    /// A completion request failed (an error answer).
    pub(super) fn completion_failed(&mut self, generation: u64) {
        if let Some(ask) = self.completion.ask.take_if(|a| a.generation == generation)
            && ask.trigger == Trigger::Invoked
        {
            self.notify(Level::Info, "No suggestions.".into());
        }
    }

    /// The answer to the pending completion request (current: `answer` checked): the list,
    /// filtered by what was typed since; nothing matching closes it (the command says so).
    pub(super) fn completion_answer(&mut self, generation: u64, value: &Value) {
        let Some(ask) = self.completion.ask.take_if(|a| a.generation == generation) else {
            return;
        };
        let Some((_, buffer, head)) = self.editor_cursor() else {
            return;
        };
        let (items, incomplete) = completion::parse(value);
        let mut items = completion::place(items, &ask.anchor);
        let caps = self
            .lsp
            .server_of(ask.anchor.buffer)
            .and_then(|(_, _, caps)| caps.completion_provider.clone());
        let all_commit = caps
            .as_ref()
            .and_then(|c| c.all_commit_characters.clone())
            .unwrap_or_default();
        if !all_commit.is_empty() {
            for item in items.iter_mut().filter(|i| i.commit.is_empty()) {
                item.commit = all_commit.clone();
            }
        }
        let a = &ask.anchor;
        let asked = a.rope.slice(a.at_snap.0 - a.word..a.at_snap.0).to_string();
        let mut popup = Popup::new(ask.anchor, generation, items, incomplete, asked);
        popup.resolve = caps.is_some_and(|c| c.resolve_provider == Some(true));
        let Some(buf) = self.buffers.get(buffer) else { return };
        popup.refilter(&buf.rope, head, &mut self.completion.filter, true);
        if popup.shown.is_empty() {
            if ask.trigger == Trigger::Invoked {
                self.notify(Level::Info, "No suggestions.".into());
            }
            self.completion.popup = popup.incomplete.then_some(popup);
            return;
        }
        self.completion.popup = Some(popup);
        self.needs_redraw = true;
        self.resolve_selected();
    }

    /// After every event: closes the list (and drops a pending request and the quick timer)
    /// when the editor lost the keys, the tab changed or the cursor left the word; else filters
    /// by the word, asking again for an incomplete list whose word changed.
    pub(super) fn check_completion(&mut self) {
        let cursor = self.editor_cursor();
        if let Some(q) = self.completion.quick
            && cursor.is_none_or(|(g, b, _)| (g, b) != (q.group, q.buffer))
        {
            self.completion.quick = None;
        }
        // A request no longer wanted (`check_requests` cancels it next).
        if let Some(ask) = &self.completion.ask
            && !self.completion_ask_current(ask.generation)
        {
            self.completion.ask = None;
        }
        let Some(p) = &self.completion.popup else { return };
        let head = cursor
            .filter(|(g, b, _)| (*g, *b) == (p.anchor.group, p.anchor.buffer))
            .and_then(|(_, b, head)| {
                let buf = self.buffers.get(b)?;
                p.anchor.in_word(&buf.rope, head).then_some(head)
            });
        let Some(head) = head else {
            self.close_completion();
            return;
        };
        let buffer = p.anchor.buffer;
        let Some(buf) = self.buffers.get(buffer) else { return };
        let p = self.completion.popup.as_mut().expect("checked");
        let before = p.query.clone();
        p.refilter(&buf.rope, head, &mut self.completion.filter, false);
        if p.query == before {
            return;
        }
        self.needs_redraw = true;
        let ask_again = p.incomplete && p.query.as_deref() != Some(p.asked.as_str());
        if p.shown.is_empty() && !p.incomplete {
            self.close_completion();
            return;
        }
        if ask_again {
            self.ask_completion(Trigger::Incomplete);
        }
        self.resolve_selected();
    }

    /// Closes the list, drops the pending request (and resolves for the list) and the timer.
    pub(super) fn close_completion(&mut self) {
        if self.completion.popup.take().is_some() {
            self.needs_redraw = true;
        }
        self.completion.ask = None;
        self.completion.quick = None;
        self.cancel_requests(|p| {
            matches!(
                p.request,
                Request::Completion { .. } | Request::Resolve { accepted: None, .. }
            )
        });
    }

    /// A char typed in the editor (already inserted): a trigger character asks at once; a word
    /// char with no list or request starts the quick-suggestion timer.
    pub(super) fn completion_typed(&mut self, c: char) {
        let Some((group, buffer, _)) = self.editor_cursor() else {
            return;
        };
        let Some(triggers) = self
            .lsp
            .server_of(buffer)
            .and_then(|(_, _, caps)| caps.completion_provider.as_ref())
            .map(|p| p.trigger_characters.clone().unwrap_or_default())
        else {
            return;
        };
        if triggers.iter().any(|t| t.chars().eq([c])) {
            self.close_completion();
            self.ask_completion(Trigger::Character(c));
            return;
        }
        let pending = self.completion.ask.is_some();
        if pos::is_word_char(c) && self.completion.popup.is_none() && !pending && self.settings.quick_suggestions {
            let revision = self.buffers.get(buffer).map_or(0, |b| b.revision());
            self.completion.quick = Some(Quick {
                deadline: Instant::now() + completion::QUICK_DELAY,
                group,
                buffer,
                revision,
            });
        }
    }

    /// The quick-suggestion timer passed `now`: asks, if nothing changed since the word char was
    /// typed and the cursor is in code (not a string or comment, when the tree says) after a word.
    pub(super) fn completion_tick(&mut self, now: Instant) {
        let Some(q) = self.completion.quick else { return };
        if q.deadline > now {
            return;
        }
        self.completion.quick = None;
        let Some((group, buffer, head)) = self.editor_cursor() else {
            return;
        };
        let Some(buf) = self.buffers.get(buffer) else { return };
        if (group, buffer) != (q.group, q.buffer)
            || buf.revision() != q.revision
            || self.completion.popup.is_some()
            || self.completion.ask.is_some()
            || buf.syntax.scope_before(head).is_some_and(|s| s != Scope::Code)
            || completion::word_before(&buf.rope, head) == 0
        {
            return;
        }
        self.ask_completion(Trigger::Quick);
    }

    /// The list's keys.
    pub(super) fn suggest_command(&mut self, c: SuggestCommand) {
        let Some(p) = &mut self.completion.popup else { return };
        let page = p.page() as isize;
        match c {
            SuggestCommand::Up => p.move_by(-1, true),
            SuggestCommand::Down => p.move_by(1, true),
            SuggestCommand::PageUp => p.move_by(-page, false),
            SuggestCommand::PageDown => p.move_by(page, false),
            SuggestCommand::Accept => {
                self.accept_completion(None);
                return;
            }
            SuggestCommand::Close => {
                self.close_completion();
                return;
            }
        }
        self.resolve_selected();
    }

    /// A typed char that's a commit character of the selected item inserts the item first.
    pub(super) fn completion_commit(&mut self, c: char) {
        if !self.completion_shown() {
            return;
        }
        let commits = self
            .completion
            .popup
            .as_ref()
            .and_then(Popup::selected_item)
            .is_some_and(|item| item.commit.iter().any(|s| s.chars().eq([c])));
        if commits {
            self.accept_completion(None);
        }
    }

    /// Inserts the selected item (or row `row` of the list) at every cursor and closes the list.
    pub(super) fn accept_completion(&mut self, row: Option<usize>) {
        let Some(popup) = self.completion.popup.take() else {
            return;
        };
        self.close_completion();
        let Some(index) = popup.shown.get(row.unwrap_or(popup.selected)).copied() else {
            return;
        };
        if !self.active_writable() {
            return;
        }
        let item = &popup.items[index];
        let a = &popup.anchor;
        let group = self.groups.active;
        if group != a.group {
            return;
        }
        let Some((buf, view)) = self.active_mut() else { return };
        let head = view.selections.primary().head;
        let from = (a.start.0 as isize - item.lead).clamp(0, head.0 as isize) as usize;
        let end = (a.at.0 + item.after).max(head.0);
        let encoding = a.encoding;
        let additional = item
            .additional
            .iter()
            .filter_map(|(s, e, t)| {
                let s = a.now(pos::from_lsp_pos(&a.rope, *s, encoding))?;
                let e = a.now(pos::from_lsp_pos(&a.rope, *e, encoding))?;
                Some((s, e.max(s), t.clone()))
            })
            .collect();
        completion::insert(
            buf,
            view,
            &Insertion {
                text: &item.text,
                select: item.select,
                before: head.0 - from,
                after: end - head.0,
                additional,
            },
        );
        let revision = buf.revision();
        if popup.resolve && !item.resolved && item.additional.is_empty() {
            // Its additional edits (if any) come with the resolve: the server sees the insert first.
            self.lsp.sync_documents(&mut self.buffers);
            let head = self
                .group()
                .active_tab()
                .map_or(head, |t| t.view.selections.primary().head);
            self.send_server_request(
                a.server,
                encoding,
                (a.group, a.buffer, head),
                "completionItem/resolve",
                item.raw.clone(),
                Request::Resolve {
                    generation: popup.generation,
                    index,
                    accepted: Some(revision),
                },
            );
        }
    }

    /// Resolves the selected item if the server can and it isn't yet (a newer resolve replaces a
    /// pending one).
    fn resolve_selected(&mut self) {
        let Some(p) = &self.completion.popup else { return };
        let Some(index) = p.selected_index().filter(|&i| p.resolve && !p.items[i].resolved) else {
            return;
        };
        let generation = p.generation;
        let pending = self.requests.values().any(|r| {
            matches!(r.request, Request::Resolve { generation: g, index: i, accepted: None } if g == generation && i == index)
        });
        if pending {
            return;
        }
        let a = &p.anchor;
        let (server, encoding, place) = (a.server, a.encoding, (a.group, a.buffer, a.at));
        let params = p.items[index].raw.clone();
        self.send_server_request(
            server,
            encoding,
            place,
            "completionItem/resolve",
            params,
            Request::Resolve {
                generation,
                index,
                accepted: None,
            },
        );
    }

    /// A resolve answer (current: `answer` checked): merged into the shown item, or for an
    /// inserted item its additional edits applied (one undo step) to the unchanged buffer.
    pub(super) fn resolve_answer(
        &mut self,
        p: &Pending,
        generation: u64,
        index: usize,
        accepted: Option<u64>,
        value: &Value,
    ) {
        let Some(_) = accepted else {
            if let Some(popup) = self.completion.popup.as_mut().filter(|c| c.generation == generation)
                && let Some(item) = popup.items.get_mut(index)
            {
                item.resolve(value);
            }
            return;
        };
        let edits: Vec<(LspPos, LspPos, String)> = {
            let mut item = completion::parse(&json!([value])).0;
            match item.pop() {
                Some(mut i) => {
                    i.additional.clear();
                    i.resolve(value);
                    i.additional
                }
                None => return,
            }
        };
        if edits.is_empty() {
            return;
        }
        let Some(group) = self.groups.get_mut(p.group) else {
            return;
        };
        let active = group.active;
        let Some(tab) = group.tabs.get_mut(active).filter(|t| t.buffer == p.buffer) else {
            return;
        };
        let Some(buf) = self.buffers.get_mut(p.buffer) else {
            return;
        };
        let rope = &buf.rope;
        let mut edits: Vec<(ByteIdx, ByteIdx, String)> = edits
            .into_iter()
            .map(|(s, e, t)| {
                let s = pos::from_lsp_pos(rope, s, p.encoding);
                (s, pos::from_lsp_pos(rope, e, p.encoding).max(s), t)
            })
            .collect();
        edits.sort_by_key(|(s, e, _)| (*s, *e));
        let mut changes = ChangeSet::with_capacity(edits.len(), 0);
        let mut prev = ByteIdx(0);
        for (s, e, t) in &edits {
            if *s >= prev {
                changes.push(*s, *e, t);
                prev = *e;
            }
        }
        if changes.is_empty() {
            return;
        }
        let before = tab.view.selections.clone();
        let mut after = before.clone();
        {
            let m = changes.mapper();
            after.transform(|s| crate::selection::Selection {
                anchor: m.map(s.anchor, Assoc::After),
                head: m.map(s.head, Assoc::After),
                goal: None,
            });
        }
        buf.edit(changes, before, after.clone(), EditKind::Other);
        tab.view.selections = after;
        self.needs_redraw = true;
    }

    /// A mouse event, before the hover and the rest see it: true when the list takes it (it's
    /// over the list: a click inserts that row's item, the wheel scrolls). A click or the wheel
    /// elsewhere closes the list.
    pub(super) fn completion_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        if self.completion.popup.is_none() {
            return false;
        }
        let over = hit == Hit::Completion;
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -WHEEL_ROWS
                } else {
                    WHEEL_ROWS
                };
                if let Some(p) = &mut self.completion.popup {
                    p.scroll_by(delta);
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) if over => {
                if let Some(h) = self.hits.completion
                    && m.row >= h.rows.y
                    && m.row < h.rows.y + h.rows.height
                {
                    self.accept_completion(Some(h.top + (m.row - h.rows.y) as usize));
                }
                true
            }
            _ if over => true,
            MouseEventKind::Down(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.close_completion();
                false
            }
            _ => false,
        }
    }
}
