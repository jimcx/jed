//! Markdown preview tabs in the app (`markdown_tab::MarkdownTab`, drawn by `ui::markdown`):
//! "Markdown: Open Preview to the Side" (`open_markdown_preview`), the movement / selection /
//! select-all / copy commands (`markdown_command`; every other editor command does nothing in a
//! preview), Cmd+C (`markdown_copy`) and the mouse (`markdown_mouse`: wheel, click, Shift+click,
//! drag).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::mouse::Drag;
use super::{App, AppEvent, Level};
use crate::buffer::BufferId;
use crate::buffer::pos;
use crate::clipboard::Copied;
use crate::command::Command;
use crate::group::Tab;
use crate::layout::{Hit, SplitDir};
use crate::markdown::{self, Doc};
use crate::markdown_tab::{MarkdownTab, SYNC_LIMIT};
use crate::syntax::Language;

/// How long after the source's last edit its previews are rendered again.
pub const DELAY: Duration = Duration::from_millis(150);

/// The previews' schedule.
#[derive(Default)]
pub struct MarkdownState {
    /// Edited sources' previews are due (`DELAY` after the last edit seen).
    pub deadline: Option<Instant>,
    /// Renders started (synchronous or background), for tests.
    pub renders: usize,
}

impl MarkdownState {
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Rows the wheel scrolls; columns for a sideways wheel.
const WHEEL_ROWS: isize = 3;
const WHEEL_COLS: isize = 4;

impl App {
    /// Keeps the previews in step with their sources, once per loop turn and after a frame:
    /// a changed source restarts the debounce; when it is due the stale previews render again
    /// (small documents here, big ones in the background → `AppEvent::View(ViewEvent::MarkdownRendered)`, only
    /// the newest result is taken); a width the document was not laid out at renders at once;
    /// a source scrolled moves its previews (proportionally to the rows) unless the user
    /// scrolled the preview since; a source no tab shows marks its previews closed; a renamed
    /// source renames them.
    pub(super) fn sync_markdown(&mut self) {
        if self.groups.markdown_tabs_mut().next().is_none() {
            self.markdown.deadline = None;
            return;
        }
        let now = Instant::now();
        // The first view of each source buffer: its top line (the active tab of a group wins).
        let mut tops: HashMap<BufferId, usize> = HashMap::new();
        for id in self.groups.ids() {
            let Some(g) = self.groups.get(id) else { continue };
            for (i, t) in g.tabs.iter().enumerate() {
                if let Some(t) = t.text()
                    && (i == g.active || !tops.contains_key(&t.buffer))
                {
                    tops.insert(t.buffer, t.view.top_line());
                }
            }
        }
        let due = self.markdown.deadline.is_some_and(|d| d <= now);
        if due {
            self.markdown.deadline = None;
        }
        let tx = self.tx.clone();
        let runtime = tokio::runtime::Handle::try_current().is_ok();
        let (buffers, state, redraw) = (&self.buffers, &mut self.markdown, &mut self.needs_redraw);
        for m in self.groups.markdown_tabs_mut() {
            if m.closed {
                continue;
            }
            let (Some(buf), Some(&top)) = (buffers.get(m.source), tops.get(&m.source)) else {
                m.closed = true;
                *redraw = true;
                continue;
            };
            if buf.has_file() && m.path.as_deref() != Some(buf.path.as_path()) {
                m.path = Some(buf.path.clone());
                m.name = buf.name();
                *redraw = true;
            }
            let revision = buf.revision();
            if revision != m.seen {
                m.seen = revision;
                state.deadline = Some(now + DELAY);
            }
            // A restored preview keeps its own scroll until the source scrolls (`src_top` unseen).
            if m.src_top == usize::MAX {
                m.src_top = top;
                m.following = false;
            } else if top != m.src_top {
                m.src_top = top;
                m.following = true;
            }
            if m.following {
                let before = m.top;
                m.follow(top, pos::line_count(&buf.rope));
                *redraw |= m.top != before;
            }
            // What to render: the new text once the debounce has passed, else (a new width
            // only) the text as it is.
            // A first render still running waits for the debounce too (no 1 MB copy a key).
            let first = m.revision == u64::MAX;
            if first && m.requested.is_some() && !due {
                continue;
            }
            let target = if (due || first) && m.revision != revision {
                revision
            } else {
                m.revision
            };
            let width = m.view_width;
            if (target == m.revision && m.width == width) || m.requested == Some((target, width)) {
                continue;
            }
            let text: Arc<str> = if target == m.revision {
                m.text.clone()
            } else {
                buf.rope.to_string().into()
            };
            state.renders += 1;
            if text.len() <= SYNC_LIMIT {
                let doc = markdown::render(&text, width);
                m.apply(text, doc, width, target);
                *redraw = true;
            } else if runtime {
                m.generation += 1;
                m.requested = Some((target, width));
                let (tab, generation) = (m.id, m.generation);
                let tx = tx.clone();
                tokio::task::spawn_blocking(move || {
                    let doc = markdown::render(&text, width);
                    let _ = tx.send(AppEvent::View(super::hex::ViewEvent::MarkdownRendered {
                        tab,
                        generation,
                        width,
                        revision: target,
                        text,
                        doc,
                    }));
                });
            }
        }
    }

    /// A background render finished: taken when it is the newest asked for by that tab.
    pub(super) fn markdown_rendered(
        &mut self,
        tab: u64,
        generation: u64,
        width: usize,
        revision: u64,
        text: Arc<str>,
        doc: Doc,
    ) {
        let Some(m) = self.groups.markdown_tabs_mut().find(|m| m.id == tab) else {
            return;
        };
        // A newer request, or a render done here since (a document shrunk under `SYNC_LIMIT`),
        // makes this one stale.
        if m.generation != generation || m.requested != Some((revision, width)) {
            tracing::debug!(tab, generation, "markdown: stale render dropped");
            return;
        }
        m.apply(text, doc, width, revision);
        self.needs_redraw = true;
    }

    /// The active tab's Markdown preview, if it is one.
    pub fn active_markdown(&self) -> Option<&MarkdownTab> {
        self.group().active_markdown()
    }

    /// "Markdown: Open Preview to the Side": a preview of the active Markdown text tab in the
    /// editor group after the active one (a new split to the right when there is none), active
    /// there; the source's group keeps the focus. A preview of that source in any group is
    /// focused instead. Another kind of tab (or language): a notice and nothing else.
    pub(super) fn open_markdown_preview(&mut self) {
        let Some(buf) = self.active_buffer() else {
            self.notify(Level::Info, "Markdown preview needs a Markdown file".to_string());
            return;
        };
        if buf.syntax.language() != Language::Markdown {
            self.notify(Level::Info, "Markdown preview needs a Markdown file".to_string());
            return;
        }
        let source = self.group().active_tab().map(|t| t.buffer).expect("a text tab");
        if let Some((g, i)) = self.groups.find_markdown(source) {
            if let Some(group) = self.groups.get_mut(g) {
                group.active = i;
            }
            self.focus_group(g);
            return;
        }
        let tab = MarkdownTab::new(
            source,
            buf.has_file().then(|| buf.path.clone()),
            buf.name(),
            buf.rope.to_string().into(),
            buf.revision(),
        );
        let from = self.groups.active;
        let ids = self.groups.ids();
        let next = ids
            .iter()
            .position(|&g| g == from)
            .and_then(|i| ids.get(i + 1).copied());
        let to = match next {
            Some(g) => g,
            None => {
                let g = self.groups.split_empty(SplitDir::Right);
                self.groups.active = from;
                g
            }
        };
        if let Some(g) = self.groups.get_mut(to) {
            g.open_tab(Tab::Markdown(Box::new(tab)));
        }
        self.ui.focus = super::Focus::Editor;
        self.needs_redraw = true;
    }

    /// A command while the active tab is a preview: movement and selection (Shift), select all,
    /// Esc (drops the selection). Every other editor command does nothing there (Cmd+C is
    /// `copy`'s). False when the active tab isn't one.
    pub(super) fn markdown_command(&mut self, cmd: Command) -> bool {
        let Some(m) = self.group_mut().active_markdown_mut() else {
            return false;
        };
        match cmd {
            Command::Move { motion, select } => m.motion(motion, select),
            Command::SelectAll => m.select_all(),
            Command::CancelSelection => m.anchor = None,
            _ => tracing::debug!(command = %cmd.name(), "not in a Markdown preview"),
        }
        self.needs_redraw = true;
        true
    }

    /// Cmd+C on a preview: the selected rendered text (nothing without a selection). False when
    /// the active tab isn't a preview.
    pub(super) fn markdown_copy(&mut self) -> bool {
        let Some(m) = self.active_markdown() else { return false };
        if let Some(text) = m.copy_text() {
            self.clipboard.write(Copied {
                text,
                whole_line: false,
                pieces: Vec::new(),
            });
        }
        true
    }

    /// The mouse over a group's preview: the wheel scrolls it (without focusing; sideways with
    /// Shift or a sideways wheel), a click puts the caret under the pointer (Shift: extends the
    /// selection) and starts a drag selecting from there. False for other events and groups not
    /// showing a preview.
    pub(super) fn markdown_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        if let (MouseEventKind::Drag(MouseButton::Left), Some(Drag::Markdown(g))) = (m.kind, &self.drag) {
            let g = *g;
            let Some(rows) = self.hits.group(g).map(|h| h.text) else {
                return true;
            };
            let row = m.row.saturating_sub(rows.y) as usize;
            let col = m.column.saturating_sub(rows.x) as usize;
            if let Some(t) = self.groups.get_mut(g).and_then(|g| g.active_markdown_mut()) {
                t.click(row, col, true);
            }
            return true;
        }
        let Hit::Editor(g) = hit else { return false };
        let rows = self.hits.group(g).map(|h| h.text).unwrap_or_default();
        let Some(t) = self.groups.get_mut(g).and_then(|g| g.active_markdown_mut()) else {
            return false;
        };
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        match m.kind {
            MouseEventKind::ScrollUp if shift => t.scroll_cols(-WHEEL_COLS),
            MouseEventKind::ScrollDown if shift => t.scroll_cols(WHEEL_COLS),
            MouseEventKind::ScrollUp => t.scroll_by(-WHEEL_ROWS),
            MouseEventKind::ScrollDown => t.scroll_by(WHEEL_ROWS),
            MouseEventKind::ScrollLeft => t.scroll_cols(-WHEEL_COLS),
            MouseEventKind::ScrollRight => t.scroll_cols(WHEEL_COLS),
            MouseEventKind::Down(MouseButton::Left) => {
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                if rows.contains(p) {
                    t.click((m.row - rows.y) as usize, (m.column - rows.x) as usize, shift);
                    self.drag = Some(Drag::Markdown(g));
                }
                self.focus_group(g);
            }
            _ => return false,
        }
        true
    }
}
