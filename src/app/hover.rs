//! Hover (`app.hover`, a `hover::HoverState`; drawn by `ui::hover`): the command (the cursor's
//! position), the mouse resting on text (`hover::DELAY`, through `next_timer`), the answer
//! becoming the popup, and what closes it.
//!
//! - The popup shows the diagnostics under the pointer (or at the cursor) as soon as the rest
//!   ends (or the command runs), without waiting for the server; the hover request still goes
//!   out and its answer rebuilds the popup with the server's content after them. No server, no
//!   `hoverProvider`, a null or failed answer leave the diagnostics alone; "No hover
//!   information" only when neither exists.
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
use crate::diagnostics::{self, Diagnostic};
use crate::hover::{self, Popup, Rest};
use crate::layout::{GroupId, Hit};

/// A server's hover, read: its lines and the range it's about (if it said).
type Answer = (Vec<hover::Line>, Option<(ByteIdx, ByteIdx)>);

/// Wheel rows per notch inside the popup.
const POPUP_WHEEL: isize = 3;

fn supports_hover(caps: &ServerCapabilities) -> bool {
    caps.hover_provider
        .as_ref()
        .is_some_and(|h| !matches!(h, HoverProviderCapability::Simple(false)))
}

impl App {
    /// The command: the hover for the active view's cursor. The diagnostics there show at once;
    /// the server's answer is appended when it comes.
    pub(super) fn show_hover(&mut self) {
        let group = self.groups.active;
        let Some(tab) = self.group().active_tab() else { return };
        let (buffer, at) = (tab.buffer, tab.view.selections.primary().head);
        self.close_hover();
        let shown = self.hover_popup(group, buffer, at, None, None);
        if shown {
            self.request_hover_fixes_at(group, buffer, at, false);
        }
        if !self.request_hover(group, buffer, at, None) && !shown {
            self.notify(super::Level::Info, "No hover information".into());
        }
    }

    /// The diagnostics the popup shows at `at` (drawn on that cell for the mouse, else touching
    /// the cursor).
    fn hover_marks(&self, buffer: BufferId, at: ByteIdx, mouse: bool) -> Vec<diagnostics::Mark> {
        let Some(buf) = self.buffers.get(buffer) else {
            return Vec::new();
        };
        let marks = if mouse {
            buf.diagnostics.drawn_at(&buf.rope, at)
        } else {
            buf.diagnostics.touching(&buf.rope, at)
        };
        marks.into_iter().cloned().collect()
    }

    /// Asks for the quick fixes of the diagnostics the popup shows (its "Quick Fix…" row).
    fn request_hover_fixes_at(&mut self, group: GroupId, buffer: BufferId, at: ByteIdx, mouse: bool) {
        let marks = self.hover_marks(buffer, at, mouse);
        if !marks.is_empty() {
            self.request_hover_fixes(group, buffer, &marks);
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

    /// A hover answer (current: `answer` checked): the popup again, with the server's content
    /// after the diagnostics.
    pub(super) fn hover_answer(&mut self, p: &Pending, word: Option<(ByteIdx, ByteIdx)>, value: &Value) {
        let answer = hover::parse(value).and_then(|h| {
            let rope = &self.buffers.get(p.buffer)?.rope;
            let range = h.range.map(|(s, e)| {
                (
                    pos::from_lsp_pos(rope, s, p.encoding),
                    pos::from_lsp_pos(rope, e, p.encoding),
                )
            });
            Some((h.lines, range))
        });
        if !self.hover_popup(p.group, p.buffer, p.at, word, answer) && word.is_none() {
            self.notify(super::Level::Info, "No hover information".into());
        }
    }

    /// Shows the popup for `at` in `buffer` (shown by `group`'s active tab): the diagnostics
    /// there (drawn on that cell for the mouse resting on `word`, else touching the cursor),
    /// then a rule and `answer` (the server's lines and range), if any. It's anchored at the
    /// server's range, else at the run the mouse rests on / the word at the cursor, and is
    /// about that and the diagnostics' squiggles. False (and nothing changed) when there's
    /// nothing to show.
    fn hover_popup(
        &mut self,
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        word: Option<(ByteIdx, ByteIdx)>,
        answer: Option<Answer>,
    ) -> bool {
        let Some(buf) = self.buffers.get(buffer) else {
            return false;
        };
        let Some(view) = self
            .groups
            .get(group)
            .and_then(|g| g.active_tab())
            .filter(|t| t.buffer == buffer)
            .map(|t| &t.view)
        else {
            return false;
        };
        let rope = &buf.rope;
        let marks = if word.is_some() {
            buf.diagnostics.drawn_at(rope, at)
        } else {
            buf.diagnostics.touching(rope, at)
        };
        if marks.is_empty() && answer.is_none() {
            return false;
        }
        let diags: Vec<&Diagnostic> = marks.iter().map(|m| &*m.diag).collect();
        let mut lines = hover::diagnostic_lines(&diags);
        let owned: Vec<diagnostics::Mark> = marks.iter().map(|m| (*m).clone()).collect();
        let diag_range = super::code_action::marks_range(rope, &owned);
        if !owned.is_empty() && self.hover_fixes_for(group, buffer, buf.revision(), &owned) {
            lines.push(hover::Line::link(crate::code_action::QUICK_FIX_ROW));
        }
        let fixes_at = lines.len();
        let mut server_range = None;
        if let Some((answer, range)) = answer {
            if !lines.is_empty() {
                lines.push(hover::Line::rule());
            }
            lines.extend(answer);
            server_range = range;
        }
        let about = server_range
            .or(word)
            .or_else(|| pos::word_at(rope, at))
            .unwrap_or((at, at));
        let mut peek = pos::Peek::new(rope);
        let range = marks.iter().fold(about, |(s, e), m| {
            let (ms, me) = diagnostics::extent(rope, &mut peek, m);
            (s.min(ms), e.max(me))
        });
        let mouse = word.is_some();
        let revision = buf.revision();
        // The answer after the diagnostics keeps where the wheel scrolled them.
        let top = self
            .hover
            .popup
            .as_ref()
            .filter(|p| p.group == group && p.buffer == buffer && p.revision == revision && p.mouse == mouse)
            .map_or(0, |p| p.top);
        self.hover.popup = Some(Popup {
            group,
            buffer,
            revision,
            scroll: (view.top_line(), view.left_col),
            anchor: about.0,
            range,
            lines,
            fixes_at,
            diagnostics: diag_range,
            link_row: None,
            mouse,
            top,
            rows: 0,
            shown: 0,
        });
        self.needs_redraw = true;
        true
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
            MouseEventKind::Down(crossterm::event::MouseButton::Left)
                if over && self.hover.popup.as_ref().is_some_and(|p| p.link_row == Some(m.row)) =>
            {
                self.open_hover_fixes();
                true
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
    /// same-class characters there. `None` past a line's end, on blank space or off the text,
    /// unless a squiggle is drawn on that cell (a diagnostic on whitespace, at a line end, or the
    /// cell past an empty line).
    fn hover_target(&self, x: u16, y: u16) -> Option<(GroupId, BufferId, ByteIdx, (ByteIdx, ByteIdx))> {
        let Hit::Editor(g) = self.hits.hit(x, y) else {
            return None;
        };
        let text = self.hits.group(g)?.text;
        if !text.contains(ratatui::layout::Position { x, y }) {
            return None;
        }
        let tab = self.groups.get(g)?.active_tab()?;
        let buf = self.buffers.get(tab.buffer)?;
        let rope = &buf.rope;
        let line = tab.view.top_line() + (y - text.y) as usize;
        if line >= pos::line_count(rope) {
            return None;
        }
        let tab_width = self.settings.for_buffer(buf).tab_width;
        let l = pos::Line::new(rope, line);
        let col = pos::DisplayCol(tab.view.left_col + (x - text.x) as usize);
        let end_col = l.col(l.end(), tab_width);
        let squiggled = |at| !buf.diagnostics.drawn_at(rope, at).is_empty();
        let at = if col < end_col {
            let at = l.byte_at_col(col, tab_width);
            if pos::Peek::new(rope).after(at).is_none_or(char::is_whitespace) && !squiggled(at) {
                return None;
            }
            at
        } else if col == end_col && l.start() == l.end() && squiggled(l.end()) {
            l.end()
        } else {
            return None;
        };
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
            // The diagnostics there at once; the server's answer follows.
            if self.hover_popup(r.group, r.buffer, r.at, Some(r.word), None) {
                self.request_hover_fixes_at(r.group, r.buffer, r.at, true);
            }
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
