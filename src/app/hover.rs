//! Hover (`app.hover`, a `hover::HoverState`; drawn by `ui::hover`): the command (the cursor's
//! position), the mouse resting on text (`hover::DELAY`, through `next_timer`), the answer
//! becoming the popup, and what closes it.
//!
//! - A key press closes the popup (and drops a pending hover); Esc in the editor only closes it.
//!   Clicks, the wheel and drags outside the popup close it too; over it, the wheel scrolls a
//!   long one and other mouse events stop there.
//! - The mouse moving off a mouse-opened popup's range (and not onto the popup) closes it; a
//!   command-opened one stays (the pointer isn't on it to begin with).
//! - `check_hover` (after every event) closes it when its buffer was edited, its group's tab
//!   changed or its view scrolled.

use std::time::Instant;

use crossterm::event::{MouseEvent, MouseEventKind};
use lsp_types::{HoverProviderCapability, ServerCapabilities};
use serde_json::Value;

use super::App;
use super::lsp::{Pending, Request};
use crate::buffer::BufferId;
use crate::buffer::pos::{self, ByteIdx};
use crate::hover::{self, Popup, Rest};
use crate::layout::{GroupId, Hit};

/// Wheel rows per notch inside the popup.
const POPUP_WHEEL: isize = 3;

fn supports_hover(caps: &ServerCapabilities) -> bool {
    caps.hover_provider
        .as_ref()
        .is_some_and(|h| !matches!(h, HoverProviderCapability::Simple(false)))
}

impl App {
    /// The command: the hover for the active view's cursor.
    pub(super) fn show_hover(&mut self) {
        let group = self.groups.active;
        let Some(tab) = self.group().active_tab() else { return };
        let (buffer, at) = (tab.buffer, tab.view.selections.primary().head);
        self.close_hover();
        if !self.request_hover(group, buffer, at, None) {
            self.notify(super::Level::Info, "No hover information".into());
        }
    }

    fn request_hover(
        &mut self,
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        word: Option<(ByteIdx, ByteIdx)>,
    ) -> bool {
        self.send_request(
            group,
            buffer,
            at,
            "textDocument/hover",
            supports_hover,
            Request::Hover { word },
        )
    }

    /// A hover answer (current: `answer` checked): the popup, anchored at the range the server
    /// named, else at the run the mouse rests on / the word at the cursor.
    pub(super) fn hover_answer(&mut self, p: &Pending, word: Option<(ByteIdx, ByteIdx)>, value: &Value) {
        let Some(h) = hover::parse(value) else {
            if word.is_none() {
                self.notify(super::Level::Info, "No hover information".into());
            }
            return;
        };
        let Some(buf) = self.buffers.get(p.buffer) else { return };
        let Some(view) = self
            .groups
            .get(p.group)
            .and_then(|g| g.active_tab())
            .filter(|t| t.buffer == p.buffer)
            .map(|t| &t.view)
        else {
            return;
        };
        let rope = &buf.rope;
        let range = h
            .range
            .map(|(s, e)| {
                (
                    pos::from_lsp_pos(rope, s, p.encoding),
                    pos::from_lsp_pos(rope, e, p.encoding),
                )
            })
            .or(word)
            .or_else(|| pos::word_at(rope, p.at))
            .unwrap_or((p.at, p.at));
        self.hover.popup = Some(Popup {
            group: p.group,
            buffer: p.buffer,
            revision: buf.revision(),
            scroll: (view.top_line(), view.left_col),
            range,
            lines: h.lines,
            mouse: word.is_some(),
            top: 0,
            rows: 0,
            shown: 0,
        });
    }

    /// Closes the popup, stops the resting timer and drops a pending hover.
    pub(super) fn close_hover(&mut self) {
        if self.hover.popup.take().is_some() {
            self.needs_redraw = true;
        }
        self.hover.rest = None;
        self.cancel_requests(|p| matches!(p.request, Request::Hover { .. }));
    }

    /// A mouse event, before anything else sees it: true when the popup takes it (it's over the
    /// popup). Moves start the resting timer over text; clicks, the wheel and drags elsewhere
    /// close the popup.
    pub(super) fn hover_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        let over = hit == Hit::Hover;
        match m.kind {
            MouseEventKind::Moved => {
                if over {
                    self.hover.rest = None;
                    return true;
                }
                let target = self.hover_target(m.column, m.row);
                self.hover.mouse = target.map(|(g, b, _, w)| (g, b, w));
                if let Some(p) = &self.hover.popup {
                    let on = target.is_some_and(|(g, b, at, _)| {
                        g == p.group && b == p.buffer && p.range.0 <= at && (at < p.range.1 || at == p.range.0)
                    });
                    if on {
                        self.hover.rest = None;
                        return false;
                    }
                    if p.mouse {
                        self.hover.popup = None;
                        self.needs_redraw = true;
                    }
                }
                self.hover.rest = match target {
                    Some((group, buffer, at, word)) => match self.hover.rest.take() {
                        // Still on the same run: the timer keeps running.
                        Some(r) if r.group == group && r.buffer == buffer && r.word == word => Some(r),
                        _ => Some(Rest {
                            deadline: Instant::now() + hover::DELAY,
                            group,
                            buffer,
                            at,
                            word,
                        }),
                    },
                    None => None,
                };
                false
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -POPUP_WHEEL
                } else {
                    POPUP_WHEEL
                };
                if let Some(p) = &mut self.hover.popup {
                    p.scroll_by(delta);
                }
                true
            }
            _ if over => true,
            _ => {
                self.close_hover();
                false
            }
        }
    }

    /// The text under a screen cell, in any group: its group, buffer, position, and the run of
    /// same-class characters there. `None` past a line's end, on blank space or off the text.
    fn hover_target(&self, x: u16, y: u16) -> Option<(GroupId, BufferId, ByteIdx, (ByteIdx, ByteIdx))> {
        let Hit::Editor(g) = self.hits.hit(x, y) else {
            return None;
        };
        let text = self.hits.group(g)?.text;
        if !text.contains(ratatui::layout::Position { x, y }) {
            return None;
        }
        let tab = self.groups.get(g)?.active_tab()?;
        let rope = &self.buffers.get(tab.buffer)?.rope;
        let line = tab.view.top_line() + (y - text.y) as usize;
        if line >= pos::line_count(rope) {
            return None;
        }
        let tab_width = self.settings.tab_width;
        let l = pos::Line::new(rope, line);
        let col = pos::DisplayCol(tab.view.left_col + (x - text.x) as usize);
        if col >= l.col(l.end(), tab_width) {
            return None;
        }
        let at = l.byte_at_col(col, tab_width);
        if pos::Peek::new(rope).after(at).is_none_or(char::is_whitespace) {
            return None;
        }
        Some((g, tab.buffer, at, pos::word_range(rope, at)))
    }

    /// The resting timer's deadline passed (`now`): the hover for where the mouse rests.
    pub(super) fn hover_tick(&mut self, now: Instant) {
        let Some(r) = self.hover.rest.clone() else { return };
        if r.deadline > now {
            return;
        }
        self.hover.rest = None;
        let shows = self
            .groups
            .get(r.group)
            .and_then(|g| g.active_tab())
            .is_some_and(|t| t.buffer == r.buffer);
        if shows {
            self.request_hover(r.group, r.buffer, r.at, Some(r.word));
        }
    }

    /// After every event: the popup closes when its buffer was edited, its group's tab changed or
    /// its view scrolled.
    pub(super) fn check_hover(&mut self) {
        let Some(p) = &self.hover.popup else { return };
        let fits = self
            .groups
            .get(p.group)
            .and_then(|g| g.active_tab())
            .is_some_and(|t| t.buffer == p.buffer && (t.view.top_line(), t.view.left_col) == p.scroll)
            && self.buffers.get(p.buffer).is_some_and(|b| b.revision() == p.revision);
        if !fits {
            self.hover.popup = None;
            self.needs_redraw = true;
        }
    }
}
