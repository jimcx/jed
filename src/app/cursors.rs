//! Cmd+D / Cmd+Shift+L / Cmd+Alt+D sessions (`multicursor`) and Cmd+U (`view::CursorHistory`).

use super::{App, Focus};
use crate::buffer::BufferId;
use crate::command::Command;
use crate::layout::GroupId;
use crate::multicursor::{self, Session, Step};

/// The running Cmd+D session and the view it belongs to: (group, tab index, buffer).
pub(super) struct MatchSession {
    view: (GroupId, usize, Option<BufferId>),
    session: Session,
}

/// Scroll position of the active view before an input event, for the Cmd+U entry it may push.
#[derive(Clone, Copy)]
pub(super) struct PreEvent {
    view: (GroupId, usize, Option<BufferId>),
    top_line: usize,
    left_col: usize,
}

impl App {
    /// Cmd+D, Cmd+Shift+L, Cmd+Alt+D on the active view.
    pub(super) fn match_command(&mut self, cmd: Command) {
        let at = self.active_view_id();
        let mut session = self.match_session.take().filter(|s| s.view == at).map(|s| s.session);
        let find = self.find_options;
        let Some((buf, view)) = self.active_mut() else {
            return;
        };
        buf.seal_undo();
        match cmd {
            Command::AddNextMatch => multicursor::next_match(&buf.rope, view, &mut session, find, Step::Add),
            Command::SkipToNextMatch => multicursor::next_match(&buf.rope, view, &mut session, find, Step::Skip),
            Command::SelectAllMatches => multicursor::select_all(&buf.rope, view, &mut session, find),
            _ => unreachable!("not a match command: {cmd:?}"),
        }
        self.match_session = session.map(|session| MatchSession { view: at, session });
        self.session_changed = true;
    }

    /// The find options changed: a running Cmd+D session that searches with them ends
    /// (word mode doesn't use them).
    pub(super) fn find_options_changed(&mut self) {
        if self.match_session.as_ref().is_some_and(|s| !s.session.word_mode) {
            self.match_session = None;
        }
    }

    /// Cmd+U: back to the selections (and scroll) before the last selection change.
    pub(super) fn cursor_undo(&mut self) {
        let Some((buf, view)) = self.active_mut() else {
            return;
        };
        buf.seal_undo();
        if let Some(state) = view.cursor_history.pop() {
            view.selections = state.selections;
            view.set_top_line(&buf.rope, state.top_line);
            view.left_col = state.left_col;
            view.reveal_cursor = false;
            view.reveal_range = None;
            self.match_session = None;
        }
    }

    fn active_view_id(&self) -> (GroupId, usize, Option<BufferId>) {
        let g = self.group();
        (self.groups.active, g.active, g.active_tab().map(|t| t.buffer))
    }

    pub(super) fn note_pre_event(&mut self) {
        let view = self.active_view_id();
        self.pre_event = self.group().active_tab().map(|t| PreEvent {
            view,
            top_line: t.view.top_line(),
            left_col: t.view.left_col,
        });
        self.session_changed = false;
    }

    /// After an input event (from `sync_views`): pushes the active view's previous selections
    /// for Cmd+U if they changed without a text change (`edited`: its buffer changed), and ends
    /// the Cmd+D session when anything but a match command changed the selections or the
    /// editor lost focus (VS Code: `onDidChangeCursorSelection`, `onDidBlurEditorText`).
    pub(super) fn track_selections(&mut self, edited: bool) {
        let at = self.active_view_id();
        let focused = self.ui.focus == Focus::Editor && self.palette.is_none() && self.prompt.is_none();
        let pre = self.pre_event.filter(|p| p.view == at);
        let session_changed = self.session_changed;
        let mut end_session = !focused;
        let g = self.groups.active_group_mut();
        if let Some(view) = g.tabs.get_mut(g.active).map(|t| &mut t.view) {
            let changed = if edited {
                view.cursor_history.reset(&view.selections);
                true
            } else {
                let (top, left) = pre.map_or((view.top_line(), view.left_col), |p| (p.top_line, p.left_col));
                view.cursor_history.record(&view.selections, top, left)
            };
            end_session |= changed && !session_changed;
        }
        if end_session || self.match_session.as_ref().is_some_and(|s| s.view != at) {
            self.match_session = None;
        }
    }
}
