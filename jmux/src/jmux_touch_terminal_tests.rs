//! End-to-end tests of jmux_touch_terminal (the touch layout's gestures in a terminal), driving the
//! server `App` headlessly through `Harness` of `jmux_touch_tests`. The hold timer's
//! `AppEvent::Hold(serial)` is delivered directly.
//!
//! Covered by `jmux_touch_terminal_*`: a swipe scrolls the scrollback with the finger and selects
//! nothing (`..._a_swipe_scrolls_...`); a hold then drag selects, and a tap clears it
//! (`..._a_hold_then_drag_selects_...`); a double tap selects the word (`..._a_double_tap_...`); a
//! tap focuses another group (`..._a_tap_focuses_...`); a program that reports the mouse gets one
//! wheel event per row from a swipe and a click from a tap (`..._a_program_that_reports_...`); a
//! plain client's press in a terminal still selects at once (`..._a_plain_client_...`).

use crossterm::event::{MouseButton, MouseEventKind};

use crate::app::AppEvent;
use crate::command::Command;
use crate::jmux_touch_tests::{Harness, tap};

fn press(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Down(MouseButton::Left), x, y);
}

fn drag(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
}

fn release(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

fn hold(h: &mut Harness) {
    let serial = h.app.pending_press_serial().expect("a pending press");
    h.app.handle(AppEvent::Hold(serial));
}

fn run(h: &Harness) -> &jterm::run::Run {
    &h.app.groups.active_group().active_tab().unwrap().run
}

fn scrollback(h: &Harness) -> usize {
    run(h).screen().screen().scrollback()
}

fn selected(h: &Harness) -> Option<String> {
    run(h).copy_selection()
}

/// A touch session whose shell has printed `line` (a command typed into it) and then `marker`.
async fn with_output(cols: u16, line: &str, marker: &str) -> Harness {
    let mut h = Harness::new(16, cols, "", true).await;
    h.settle("a prompt", |h| h.text().contains("$")).await;
    h.app.type_text(line);
    h.settle("the output", |h| h.text().contains(marker)).await;
    h.frame().await;
    h
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_swipe_scrolls_the_scrollback_and_selects_nothing() {
    let mut h = with_output(60, "seq 0 199\n", "199").await;
    let text = h.app.hit.groups[0].text;
    let (x, y) = (text.x + 2, text.y + 3);
    assert_eq!(scrollback(&h), 0);
    // The finger moving down brings earlier output in.
    press(&mut h, x, y);
    drag(&mut h, x, y + 4);
    release(&mut h, x, y + 4);
    assert_eq!(scrollback(&h), 4);
    press(&mut h, x, y);
    drag(&mut h, x, y - 2);
    release(&mut h, x, y - 2);
    assert_eq!(scrollback(&h), 2);
    assert_eq!(run(&h).selection(), None);
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_hold_then_drag_selects_and_a_tap_clears_it() {
    let mut h = with_output(60, "printf 'ALPHA_%s\\n' BETA\n", "ALPHA_BETA").await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("ALPHA_BETA")).unwrap();
    press(&mut h, 0, row);
    assert_eq!(run(&h).selection(), None, "nothing selected by the press alone");
    hold(&mut h);
    drag(&mut h, 3, row);
    drag(&mut h, 6, row);
    release(&mut h, 6, row);
    assert_eq!(selected(&h).as_deref(), Some("ALPHA_"));
    tap(&mut h, 20, row);
    assert_eq!(run(&h).selection(), None, "a tap drops the selection");
    assert_eq!(scrollback(&h), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_double_tap_selects_the_word() {
    let mut h = with_output(60, "printf 'ALPHA %s\\n' BETA\n", "ALPHA BETA").await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("ALPHA BETA")).unwrap();
    tap(&mut h, 2, row);
    tap(&mut h, 2, row);
    assert_eq!(selected(&h).as_deref(), Some("ALPHA"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_tap_focuses_another_group() {
    let mut h = Harness::new(16, 80, "", true).await;
    h.app.run(Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2).await;
    let first = h.app.hit.groups[0].clone();
    assert_ne!(h.app.groups.active, first.id);
    let (x, y) = (first.text.x + 2, first.text.y + 2);
    press(&mut h, x, y);
    assert_ne!(h.app.groups.active, first.id, "not on the press");
    release(&mut h, x, y);
    assert_eq!(h.app.groups.active, first.id, "the tap focused it");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_program_that_reports_the_mouse_gets_wheel_events_and_taps() {
    let mut h = with_output(150, "printf '\\033[?1000h\\033[?1006h'; keyecho\n", "flags=").await;
    h.settle("mouse reporting on", |h| {
        run(h).screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    })
    .await;
    let text = h.app.hit.groups[0].text;
    let (x, y) = (text.x + 2, text.y + 5);
    press(&mut h, x, y);
    drag(&mut h, x, y + 2);
    release(&mut h, x, y + 2);
    // The finger moving down is the wheel up (button 64), once per row; nothing was clicked.
    h.settle("two wheel events", |h| {
        h.text().matches("1b 5b 3c 36 34 3b").count() == 2
    })
    .await;
    assert!(!h.text().contains("1b 5b 3c 30 3b"), "no click:\n{}", h.text());
    assert_eq!(scrollback(&h), 0);
    tap(&mut h, x, y);
    // `CSI < 0 ; 3 ; 6 M` then `... m`: the press and release at the tapped cell.
    h.settle("the tap's press and release", |h| {
        let t = h.text();
        t.contains("1b 5b 3c 30 3b 33 3b 36 4d") && t.contains("1b 5b 3c 30 3b 33 3b 36 6d")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_terminal_a_plain_clients_press_selects_at_once() {
    let mut h = Harness::new(16, 60, "", false).await;
    h.settle("a prompt", |h| h.text().contains("$")).await;
    h.app.type_text("printf 'ALPHA_%s\\n' BETA\n");
    h.settle("the output", |h| h.text().contains("ALPHA_BETA")).await;
    h.frame().await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("ALPHA_BETA")).unwrap();
    press(&mut h, 0, row);
    assert!(h.app.pending_press_serial().is_none());
    drag(&mut h, 6, row);
    release(&mut h, 6, row);
    assert_eq!(selected(&h).as_deref(), Some("ALPHA_"));
}
