//! The touch layout's gestures on a tab bar and in a terminal (`--touch`; the rules are
//! `jtui::touch`'s, shared with jed). A left press on the bar or a terminal records a pending
//! `Drag::Press` and starts a `HOLD` timer. On the bar, motion past `SLOP` is a swipe that scrolls
//! it by whole tabs, following the finger; a release within `SLOP` is a tap, acted on then (focus a
//! tab, close, scroll an arrow's tab, `+`, focus the group); the timer's `AppEvent::Hold` lifts a
//! tab, after which it is an ordinary `Drag::Tab`. In a terminal a swipe scrolls the scrollback by
//! the rows moved (or sends a program that reports the mouse one wheel event per row), a tap
//! replays the click (double / triple tap select a word / line), and a hold replays the press, so a
//! drag from there selects as a mouse drag does (or is reported to the program).

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jtui::split::GroupId;
use jtui::touch::{Gesture, HOLD, Press};
use unicode_width::UnicodeWidthStr;

use super::{App, AppEvent, Drag};
use crate::hit::Hit;
use crate::tab_drag::TabDrag;

impl App {
    /// A finger lands on the bar or a terminal (`hit`, pressed by `m`): remember the press and
    /// start its hold timer.
    pub(super) fn touch_press(&mut self, hit: Hit, m: MouseEvent) {
        let group = match hit {
            Hit::Tab { group, .. }
            | Hit::Close { group, .. }
            | Hit::Arrow { group, .. }
            | Hit::NewTab { group }
            | Hit::Bar { group }
            | Hit::Terminal { group } => group,
            _ => return,
        };
        self.hold_serial += 1;
        let serial = self.hold_serial;
        self.drag = Some(Drag::Press {
            group,
            hit,
            gesture: Press::new((m.column, m.row)),
            carry: 0,
            serial,
            event: m,
        });
        let events = self.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD).await;
            let _ = events.send(AppEvent::Hold(serial));
        });
    }

    /// The serial of the pending touch press, if there is one (what its timer will send).
    #[cfg(test)]
    pub(crate) fn pending_press_serial(&self) -> Option<u64> {
        match &self.drag {
            Some(Drag::Press { serial, .. }) => Some(*serial),
            _ => None,
        }
    }

    /// The hold timer of press `serial` fired: lift the tab under the finger, or in a terminal
    /// replay the press so a drag from here selects. A stale serial (the press ended, or a newer one
    /// began) or a press on anything else does nothing.
    pub(super) fn hold(&mut self, serial: u64) {
        if let Some(Drag::Press {
            hit: Hit::Terminal { group },
            gesture,
            serial: s,
            event,
            ..
        }) = &mut self.drag
        {
            if *s != serial || gesture.held() != Gesture::Lifted {
                return;
            }
            let (group, event) = (*group, *event);
            self.drag = None;
            self.terminal_pointer(group, event);
            self.needs_redraw = true;
            return;
        }
        let Some(Drag::Press {
            hit: Hit::Tab { group, index },
            gesture,
            serial: s,
            ..
        }) = &mut self.drag
        else {
            return;
        };
        if *s != serial || gesture.held() != Gesture::Lifted {
            return;
        }
        let (group, index) = (*group, *index);
        let Some(run) = self.groups.get(group).and_then(|g| g.tabs.get(index)).map(|t| t.run.id) else {
            self.drag = None;
            return;
        };
        self.groups.active = group;
        if let Some(g) = self.groups.get_mut(group) {
            g.active = index;
        }
        self.drag = Some(Drag::Tab(TabDrag {
            from: group,
            index,
            run,
            target: None,
        }));
        self.needs_redraw = true;
    }

    /// The finger moved while a press is pending: past `SLOP` it scrolls the bar.
    pub(super) fn press_moved(&mut self, column: u16, row: u16) {
        let Some(Drag::Press {
            group,
            hit,
            gesture,
            carry,
            event,
            ..
        }) = &mut self.drag
        else {
            return;
        };
        let Gesture::Swipe { dx, dy } = gesture.moved((column, row)) else {
            return;
        };
        if let Hit::Terminal { group } = *hit {
            let at = (event.column, event.row);
            self.swipe_terminal(group, at, dy);
            return;
        }
        // The tabs follow the finger: moving left brings later tabs in.
        *carry -= dx;
        let (group, mut carry) = (*group, *carry);
        let metrics = self.tab_metrics();
        let bar = self.hit.group(group).map_or(0, |h| h.bar.width);
        if let Some(g) = self.groups.get_mut(group) {
            let width = |g: &crate::group::Group, i: usize| {
                g.tabs
                    .get(i)
                    .map_or(1, |t| i32::from(metrics.tab_width(t.title.width() as u16)))
            };
            // Never past what the renderer draws, or a reverse swipe would undo undrawn scroll.
            let widths: Vec<u32> = (0..g.tabs.len()).map(|i| width(g, i) as u32).collect();
            let area = if bar > metrics.new_tab {
                bar - metrics.new_tab
            } else {
                bar
            };
            let max_scroll = crate::tabbar::max_scroll(&widths, area, metrics);
            loop {
                let w = width(g, g.scroll);
                if carry >= w && g.scroll < max_scroll {
                    g.scroll_tabs(1);
                    carry -= w;
                } else if g.scroll > 0 && carry <= -width(g, g.scroll - 1) {
                    carry += width(g, g.scroll - 1);
                    g.scroll_tabs(-1);
                } else {
                    break;
                }
            }
            // At either end the rest is no use: it would have to be undone before the bar moved back.
            if (g.scroll == 0 && carry < 0) || (g.scroll >= max_scroll && carry > 0) {
                carry = 0;
            }
        }
        if let Some(Drag::Press { carry: c, .. }) = &mut self.drag {
            *c = carry;
        }
    }

    /// The finger lifted while a press is pending: a tap acts, anything else (a swipe) does nothing.
    pub(super) fn press_released(&mut self, column: u16, row: u16) {
        let Some(Drag::Press {
            group,
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
        if let Hit::Terminal { group } = hit {
            // The plain click: a program's click, an empty selection that the release drops, a
            // double / triple tap's word / line.
            self.terminal_pointer(group, event);
            let up = MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                ..event
            };
            self.pointer_event(up);
            return;
        }
        self.tap(group, hit);
    }

    /// A swipe over `group`'s terminal (pressed at `at`) moved `dy` rows: the text follows the
    /// finger, scrolling the scrollback, or a program that reports the mouse gets one wheel event
    /// per row (at the press, inside its text).
    fn swipe_terminal(&mut self, group: GroupId, at: (u16, u16), dy: i32) {
        let rows = -dy;
        if rows == 0 {
            return;
        }
        if self.reports_mouse(group) {
            let wheel = MouseEvent {
                kind: if rows < 0 {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                column: at.0,
                row: at.1,
                modifiers: KeyModifiers::NONE,
            };
            for _ in 0..rows.unsigned_abs() {
                self.terminal_mouse(group, wheel);
            }
        } else if let Some(shell) = self.groups.get(group).and_then(|g| g.active_tab()) {
            shell.run.scroll(rows as isize);
        }
    }

    fn tap(&mut self, group: GroupId, hit: Hit) {
        match hit {
            Hit::Tab { index, .. } => {
                self.groups.active = group;
                if let Some(g) = self.groups.get_mut(group) {
                    g.active = index;
                }
            }
            Hit::Close { index, .. } => self.request_close(group, index),
            Hit::Arrow { forward, .. } => {
                if let Some(g) = self.groups.get_mut(group) {
                    g.scroll_tabs(if forward { 1 } else { -1 });
                }
            }
            Hit::NewTab { .. } => {
                self.groups.active = group;
                self.new_terminal(group);
            }
            _ => self.groups.active = group,
        }
    }
}
