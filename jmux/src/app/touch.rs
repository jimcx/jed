//! The touch layout's gestures on a tab bar (`--touch`; the rules are `jtui::touch`'s, shared with
//! jed). A left press on the bar records a pending `Drag::Press` and starts a `HOLD` timer. Motion
//! past `SLOP` is a swipe that scrolls the bar by whole tabs, following the finger; a release within
//! `SLOP` is a tap, acted on then (focus a tab, close, scroll an arrow's tab, `+`, focus the group);
//! the timer's `AppEvent::Hold` lifts a tab, after which it is an ordinary `Drag::Tab`.

use jtui::split::GroupId;
use jtui::touch::{Gesture, HOLD, Press};
use unicode_width::UnicodeWidthStr;

use super::{App, AppEvent, Drag};
use crate::hit::Hit;
use crate::tab_drag::TabDrag;

impl App {
    /// A finger lands on the bar (`hit`): remember the press and start its hold timer.
    pub(super) fn touch_press(&mut self, hit: Hit, column: u16, row: u16) {
        let group = match hit {
            Hit::Tab { group, .. }
            | Hit::Close { group, .. }
            | Hit::Arrow { group, .. }
            | Hit::NewTab { group }
            | Hit::Bar { group } => group,
            _ => return,
        };
        self.hold_serial += 1;
        let serial = self.hold_serial;
        self.drag = Some(Drag::Press {
            group,
            hit,
            gesture: Press::new((column, row)),
            carry: 0,
            serial,
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

    /// The hold timer of press `serial` fired: lift the tab under the finger. A stale serial (the
    /// press ended, or a newer one began) or a press on anything but a tab does nothing.
    pub(super) fn hold(&mut self, serial: u64) {
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
            group, gesture, carry, ..
        }) = &mut self.drag
        else {
            return;
        };
        let Gesture::Swipe { dx } = gesture.moved((column, row)) else {
            return;
        };
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
            ..
        }) = self.drag.take()
        else {
            return;
        };
        if gesture.released((column, row)) != Gesture::Tap {
            return;
        }
        self.tap(group, hit);
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
