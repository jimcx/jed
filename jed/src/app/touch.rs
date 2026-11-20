//! The touch layout (`--touch` or `[settings] touch_layout`): `App::touch` and `tab_metrics` say
//! which layout is on, and a finger on a tab bar goes through `jtui::touch`'s rules (shared with
//! jmux). A left press on a bar hit records a pending `Drag::Press` with a `HOLD` deadline (`Timer::TouchHold`).
//! Motion past `SLOP` is a swipe that scrolls the bar by whole tabs, following the finger; a
//! release within `SLOP` is a tap, acted on then (focus a tab, close it, scroll an arrow's tab,
//! the `+`, focus the group); the timer lifts a tab, after which it is an
//! ordinary `Drag::Tab` (`tab_drag.rs`).

use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use jtui::tabs::TabMetrics;
use jtui::touch::{Gesture, HOLD, Press};

use super::mouse::Drag;
use super::tab_drag::TabDrag;
use super::{App, Focus};
use crate::group::{Tab, is_terminal_group};
use crate::layout::{GroupId, Hit};

/// A touch hold in progress: where it began and whether the finger has moved since.
#[derive(Debug, Clone, Copy)]
pub(super) struct TouchHold {
    hit: Hit,
    at: (u16, u16),
    moved: bool,
}

impl App {
    /// Whether the touch layout is on: `--touch` (which wins) or `touch_layout` in `settings.toml`.
    /// The one place everything touch reads it; a file opened through the socket doesn't change it.
    pub fn touch(&self) -> bool {
        self.touch_flag || self.settings.touch_layout
    }

    /// How every tab bar is laid out: the one source of a bar's rows and padding.
    pub fn tab_metrics(&self) -> TabMetrics {
        if self.touch() {
            TabMetrics::TOUCH
        } else {
            TabMetrics::NORMAL
        }
    }

    /// Brings the editor groups in line with the layout: in the touch layout side-by-side groups
    /// merge into one (`Groups::merge_side_by_side`) and new splits stack; with it off splits are
    /// as always (merged groups stay merged). Run at start, after the session restores and when the
    /// settings reload.
    pub(crate) fn sync_touch_layout(&mut self) {
        let touch = self.touch();
        self.groups.stacked = touch;
        if touch && self.groups.merge_side_by_side() {
            self.reveal_active();
        }
    }

    /// Hides the side panel as `ToggleSidebar` does; focus leaves it for the editor. Its width and
    /// view are kept.
    pub(super) fn hide_side(&mut self) {
        self.ui.side_visible = false;
        if self.ui.focus == Focus::SidePanel {
            self.ui.focus = Focus::Editor;
        }
    }

    /// The side panel opened or showed a tab in an editor group: in the touch layout (where it floats
    /// over the editor) it hides and focus goes to the editor.
    pub(super) fn side_opened(&mut self) {
        if self.touch() && self.ui.side_visible {
            self.hide_side();
            self.ui.focus = Focus::Editor;
        }
    }

    /// A problem or location was opened from the bottom panel: in the touch layout the panel hides
    /// (keeping its tab and height) and the editor has the keys.
    pub(super) fn panel_opened(&mut self) {
        if self.touch() && self.ui.panel_visible {
            self.hide_panel();
            self.ui.focus = Focus::Editor;
        }
    }

    /// A left press in the editor, bottom panel or terminal area outside the floating side panel
    /// hides it and goes no further.
    pub(super) fn touch_dismiss_side(&mut self, m: &MouseEvent) -> bool {
        if !self.touch() || !self.ui.side_visible || m.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let r = &self.hits.regions;
        let p = ratatui::layout::Position { x: m.column, y: m.row };
        let outside = r.side.is_some_and(|s| !s.contains(p))
            && (r.editor.contains(p)
                || r.panel.is_some_and(|a| a.contains(p))
                || r.terminal.is_some_and(|a| a.contains(p)));
        if outside {
            self.hide_side();
        }
        outside
    }

    /// Whether a press on `hit` is on a tab bar.
    pub(super) fn is_bar_hit(hit: Hit) -> bool {
        matches!(
            hit,
            Hit::Tab(..) | Hit::TabClose(..) | Hit::TabArrow(..) | Hit::NewTab(_) | Hit::TabBar(_)
        )
    }

    /// The tab a finger has picked up in `group`'s bar (drawn as `tab.lifted`).
    pub(crate) fn tab_lifted_in(&self, group: GroupId) -> Option<usize> {
        match &self.drag {
            Some(Drag::Tab(TabDrag { from, index, .. })) if self.touch() && *from == group => Some(*index),
            _ => None,
        }
    }

    /// Whether a press on `hit` is on a view the touch layout reads gestures in: a group's text,
    /// diff, hex or shell, or an Explorer row (the scroll bars, the gutter and the banner are other
    /// hits and act as always).
    pub(super) fn is_view_hit(hit: Hit) -> bool {
        matches!(hit, Hit::Editor(_) | Hit::ExplorerRow(_))
    }

    /// A finger lands on a bar or a view (`hit`, pressed by `m`): remember the press and start its
    /// hold timer.
    pub(super) fn touch_press(&mut self, hit: Hit, m: MouseEvent) {
        self.drag = Some(Drag::Press {
            hit,
            gesture: Press::new((m.column, m.row)),
            carry: 0,
            deadline: Instant::now() + HOLD,
            event: m,
        });
    }

    /// When the pending press becomes a hold (`Timer::TouchHold`); none once it ends.
    pub(super) fn touch_deadline(&self) -> Option<Instant> {
        match &self.drag {
            Some(Drag::Press { deadline, .. }) => Some(*deadline),
            _ => None,
        }
    }

    /// `Timer::TouchHold` fired: the pending press, if its deadline has passed, becomes a hold.
    pub(super) fn touch_tick(&mut self, now: Instant) {
        if self.touch_deadline().is_some_and(|d| d <= now) {
            self.hold();
        }
    }

    /// A mouse event while a touch hold is going (or a fresh press ending it): a drag past the
    /// press's cell makes it a drag, not a menu; a release that never moved opens the menu the right
    /// button would (a lifted tab dropped where it was). True when it took the release.
    pub(super) fn touch_hold_event(&mut self, m: &MouseEvent) -> bool {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.touch_hold = None,
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(h) = &mut self.touch_hold
                    && (m.column, m.row) != h.at
                {
                    h.moved = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(h) = self.touch_hold.take() else {
                    return false;
                };
                // A release in another cell is a drag even when the terminal sent no motion; a
                // prompt that opened meanwhile has the release.
                if h.moved || (m.column, m.row) != h.at || self.prompt.is_some() {
                    return false;
                }
                // The release goes the ordinary way: it drops a lifted tab where it was (no move).
                self.mouse_event_with(*m, true);
                self.right_click(h.hit, h.at);
                self.needs_redraw = true;
                return true;
            }
            _ => {}
        }
        false
    }

    /// Whether a touch press is pending (not yet a tap, swipe or hold).
    #[cfg(test)]
    pub(crate) fn press_pending(&self) -> bool {
        matches!(&self.drag, Some(Drag::Press { .. }))
    }

    /// Fires the pending press's hold timer without waiting for it.
    #[cfg(test)]
    pub(crate) fn test_hold(&mut self) {
        self.hold();
    }

    /// The pending press's hold timer fired. On a tab, lift it. On a view or an Explorer row, the
    /// press is replayed as the plain click, so a drag from here selects as a mouse drag does; the
    /// release without moving opens the menu (`touch_hold_event`). No pending press, or one that
    /// has swiped, does nothing.
    fn hold(&mut self) {
        let Some(Drag::Press {
            hit, gesture, event, ..
        }) = &mut self.drag
        else {
            return;
        };
        if gesture.held() != Gesture::Lifted {
            return;
        }
        let (hit, event) = (*hit, *event);
        let m = event;
        if Self::is_view_hit(hit) {
            self.drag = None;
            self.mouse_event_with(m, true);
            self.touch_hold = Some(TouchHold {
                hit,
                at: (m.column, m.row),
                moved: false,
            });
            self.needs_redraw = true;
            return;
        }
        let Hit::Tab(group, index) = hit else {
            return;
        };
        self.focus_group(group);
        self.activate_tab(index);
        self.drag = Some(Drag::Tab(TabDrag {
            from: group,
            index,
            tab: self.any_group(group).and_then(|g| g.tabs.get(index)).map(Tab::identity),
            target: None,
        }));
        self.touch_hold = Some(TouchHold {
            hit,
            at: (m.column, m.row),
            moved: false,
        });
        self.needs_redraw = true;
    }

    /// The finger moved while a press is pending: past `SLOP` it scrolls the bar or the view.
    pub(super) fn press_moved(&mut self, column: u16, row: u16) {
        let Some(Drag::Press {
            hit, gesture, carry, ..
        }) = &mut self.drag
        else {
            return;
        };
        let hit = *hit;
        let Gesture::Swipe { dx, dy } = gesture.moved((column, row)) else {
            return;
        };
        if Self::is_view_hit(hit) {
            self.swipe_view(hit, (column, row), dx as isize, dy as isize);
            return;
        }
        // The tabs follow the finger: moving left brings later tabs in.
        *carry -= dx;
        let (group, mut carry) = (Self::bar_group(hit), *carry);
        let metrics = self.tab_metrics();
        // The width the tabs and arrows share, as drawn; a bar without arrows has nothing to scroll.
        let area = self
            .hits
            .group(group)
            .and_then(|h| h.tab_arrows.as_ref().map(|a| a.forward.end.saturating_sub(h.tab_bar.x)));
        if let Some(area) = area
            && let Some(g) = self.any_group(group)
        {
            let widths = crate::ui::tabs::widths(g, &self.buffers, &metrics);
            let max_scroll = crate::ui::tabs::max_scroll(&widths, area, metrics);
            let mut scroll = g.scroll;
            let width = |i: usize| widths.get(i).map_or(1, |w| *w as i32);
            loop {
                if carry >= width(scroll) && scroll < max_scroll {
                    carry -= width(scroll);
                    scroll += 1;
                } else if scroll > 0 && carry <= -width(scroll - 1) {
                    scroll -= 1;
                    carry += width(scroll);
                } else {
                    break;
                }
            }
            // At either end the rest is no use: it would have to be undone before the bar moved back.
            if (scroll == 0 && carry < 0) || (scroll >= max_scroll && carry > 0) {
                carry = 0;
            }
            if let Some(g) = self.any_group_mut(group) {
                g.scroll = scroll;
            }
        } else {
            carry = 0;
        }
        if let Some(Drag::Press { carry: c, .. }) = &mut self.drag {
            *c = carry;
        }
    }

    /// A swipe over a view scrolls what it shows by the rows and columns moved, the content
    /// following the finger, as the wheel does: a text, diff or hex tab's lines, a shell's
    /// scrollback (or, for a program that reports the mouse, one wheel event per row), the
    /// Explorer's rows.
    fn swipe_view(&mut self, hit: Hit, at: (u16, u16), dx: isize, dy: isize) {
        let (rows, cols) = (-dy, -dx);
        let g = match hit {
            Hit::ExplorerRow(_) => {
                self.explorer.scroll_by(rows);
                return;
            }
            Hit::Editor(g) => g,
            _ => return,
        };
        if self.any_group(g).is_some_and(|g| g.active_shell().is_some()) {
            // Taken out of the press, as the program only gets events outside a drag.
            let drag = self.drag.take();
            let kind = if rows < 0 {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            };
            let wheel = MouseEvent {
                kind,
                column: at.0,
                row: at.1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            };
            let mut reported = false;
            for _ in 0..rows.abs() {
                reported = self.terminal_mouse(hit, wheel);
            }
            self.drag = drag;
            if !reported && let Some(shell) = self.any_group(g).and_then(|g| g.active_shell()) {
                shell.run.scroll(rows);
            }
            return;
        }
        let Some(group) = self.groups.get_mut(g) else {
            return;
        };
        if let Some(d) = group.active_diff_mut() {
            d.scroll_by(rows);
            d.scroll_columns(cols);
        } else if let Some(h) = group.active_hex_mut() {
            h.scroll_by(rows);
        } else if let Some(tab) = group.active_tab_mut()
            && let Some(buf) = self.buffers.get(tab.buffer)
        {
            tab.view.scroll_lines(&buf.rope, rows);
            tab.view.scroll_columns(cols);
        }
    }

    /// The group whose tab bar `hit` is on.
    fn bar_group(hit: Hit) -> GroupId {
        match hit {
            Hit::Tab(g, _) | Hit::TabClose(g, _) | Hit::TabArrow(g, _) | Hit::NewTab(g) | Hit::TabBar(g) => g,
            _ => unreachable!("not a bar hit"),
        }
    }

    /// The finger lifted while a press is pending: a tap acts, anything else (a swipe) does nothing.
    pub(super) fn press_released(&mut self, column: u16, row: u16) {
        let Some(Drag::Press {
            hit,
            mut gesture,
            event,
            ..
        }) = self.drag.take()
        else {
            return;
        };
        if gesture.released((column, row)) != Gesture::Tap {
            return;
        }
        if Self::is_view_hit(hit) {
            // A tap is the plain click (cursor, shell click, diff button, double / triple tap).
            self.mouse_event_with(event, true);
            let up = MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                ..event
            };
            self.mouse_event_with(up, true);
            return;
        }
        self.tap(Self::bar_group(hit), hit);
    }

    fn tap(&mut self, group: GroupId, hit: Hit) {
        match hit {
            Hit::Tab(_, index) => {
                self.focus_group(group);
                self.activate_tab(index);
            }
            Hit::TabClose(_, index) => self.close_tab(group, index),
            Hit::TabArrow(_, forward) => {
                self.focus_group(group);
                if let Some(g) = self.any_group_mut(group) {
                    g.scroll_tabs(if forward { 1 } else { -1 });
                }
            }
            Hit::NewTab(_) => self.new_tab_in(group),
            _ => self.focus_group(group),
        }
    }

    /// The `+` at a bar's end: a new tab in `group` (an untitled file, or a shell in the terminal area).
    pub(super) fn new_tab_in(&mut self, group: GroupId) {
        self.focus_group(group);
        if is_terminal_group(group) {
            self.new_shell();
        } else {
            self.new_untitled_file();
        }
    }
}
