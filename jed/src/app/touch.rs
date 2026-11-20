//! The touch layout (`--touch` or `[settings] touch_layout`): `App::touch` and `tab_metrics` say
//! which layout is on, and a finger on a tab bar goes through `jtui::touch`'s rules (shared with
//! jmux). A left press on a bar hit records a pending `Drag::Press` and starts a `HOLD` timer.
//! Motion past `SLOP` is a swipe that scrolls the bar by whole tabs, following the finger; a
//! release within `SLOP` is a tap, acted on then (focus a tab, close it, scroll an arrow's tab,
//! the `+`, focus the group); the timer's `AppEvent::Hold` lifts a tab, after which it is an
//! ordinary `Drag::Tab` (`tab_drag.rs`).

use jtui::tabs::TabMetrics;
use jtui::touch::{Gesture, HOLD, Press};

use super::mouse::Drag;
use super::tab_drag::TabDrag;
use super::{App, AppEvent};
use crate::group::{Tab, is_terminal_group};
use crate::layout::{GroupId, Hit};

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

    /// A finger lands on a bar (`hit`): remember the press and start its hold timer.
    pub(super) fn touch_press(&mut self, hit: Hit, column: u16, row: u16) {
        let group = match hit {
            Hit::Tab(g, _) | Hit::TabClose(g, _) | Hit::TabArrow(g, _) | Hit::NewTab(g) | Hit::TabBar(g) => g,
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
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD).await;
            let _ = tx.send(AppEvent::Hold(serial));
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

    /// Delivers the hold timer of press `serial` without waiting for it.
    #[cfg(test)]
    pub(crate) fn test_hold(&mut self, serial: u64) {
        self.hold(serial);
    }

    /// The hold timer of press `serial` fired: lift the tab under the finger. A stale serial (the
    /// press ended, or a newer one began) or a press on anything but a tab does nothing.
    pub(super) fn hold(&mut self, serial: u64) {
        let Some(Drag::Press {
            hit: Hit::Tab(group, index),
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
        self.focus_group(group);
        self.activate_tab(index);
        self.drag = Some(Drag::Tab(TabDrag {
            from: group,
            index,
            tab: self.any_group(group).and_then(|g| g.tabs.get(index)).map(Tab::identity),
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
