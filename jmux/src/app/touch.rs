//! The touch layout's gestures in a terminal and on the tab menu (`--touch`; the rules are
//! `jtui::touch`'s, shared with jed). A left press in a terminal, on the `☰` button or on the open
//! menu records a pending `Drag::Press` and starts a `HOLD` timer. In a terminal a swipe
//! scrolls the scrollback by the rows moved (or sends a program that reports the mouse one wheel
//! event per row), a tap replays the click (double / triple tap select a word / line), and a hold
//! replays the press, so a drag from there selects as a mouse drag does (or is reported to the
//! program). On the menu a swipe scrolls it and a tap acts (`app/menu.rs`); a hold does nothing, and
//! neither does the release after it.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jtui::split::GroupId;
use jtui::touch::{Gesture, HOLD, Press};

use super::{App, AppEvent, Drag};
use crate::hit::Hit;

impl App {
    /// A finger lands in a terminal, on `☰` or on the menu (`hit`, pressed by `m`): remember the
    /// press and start its hold timer.
    pub(super) fn touch_press(&mut self, hit: Hit, m: MouseEvent) {
        if !matches!(
            hit,
            Hit::Terminal { .. } | Hit::Menu | Hit::MenuItem(_) | Hit::MenuDead | Hit::MenuOutside
        ) {
            return;
        }
        self.hold_serial += 1;
        let serial = self.hold_serial;
        self.drag = Some(Drag::Press {
            hit,
            gesture: Press::new((m.column, m.row)),
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

    /// The hold timer of press `serial` fired: in a terminal replay the press so a drag from here
    /// selects; on the menu or `☰` a long press is neither a tap nor a swipe, so its release does
    /// nothing. A stale serial (the press ended, or a newer one began) does nothing.
    pub(super) fn hold(&mut self, serial: u64) {
        let Some(Drag::Press {
            hit,
            gesture,
            serial: s,
            event,
        }) = &mut self.drag
        else {
            return;
        };
        if *s != serial || gesture.held() != Gesture::Lifted {
            return;
        }
        let Hit::Terminal { group } = *hit else {
            return;
        };
        let event = *event;
        self.drag = None;
        self.terminal_pointer(group, event);
        self.needs_redraw = true;
    }

    /// The finger moved while a press is pending: past `SLOP` it scrolls a terminal or the menu.
    pub(super) fn press_moved(&mut self, column: u16, row: u16) {
        let Some(Drag::Press {
            hit, gesture, event, ..
        }) = &mut self.drag
        else {
            return;
        };
        let Gesture::Swipe { dy, .. } = gesture.moved((column, row)) else {
            return;
        };
        match *hit {
            Hit::Terminal { group } => {
                let at = (event.column, event.row);
                self.swipe_terminal(group, at, dy);
            }
            // The items follow the finger: moving up brings later rows in.
            Hit::MenuItem(_) | Hit::MenuDead => {
                if let Some(menu) = &mut self.menu {
                    menu.scroll_by(-dy);
                }
            }
            _ => {}
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
        self.menu_tap(hit);
    }

    /// A swipe over `group`'s terminal (pressed at `at`) moved `dy` rows: the text follows the
    /// finger, scrolling the scrollback, or a program that reports the mouse gets one wheel event
    /// per row (at the press, inside its text), and a full-screen one without it a cursor key per row.
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
            // A full-screen program without mouse reporting (less): a cursor key per row.
            if let Some(app_cursor) = shell.run.alternate_scroll() {
                let kind = if rows < 0 {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                };
                if let Some(bytes) = jterm::mouse::alternate_scroll(kind, app_cursor, rows.unsigned_abs() as usize) {
                    shell.run.write(&bytes);
                }
            } else {
                shell.run.scroll(rows as isize);
            }
        }
    }
}
