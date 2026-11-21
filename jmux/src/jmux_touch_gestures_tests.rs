//! End-to-end tests of jmux_touch_gestures (the touch layout's swipe, tap and hold on a tab bar),
//! driving the server `App` headlessly through `Harness` of `jmux_touch_tests`. The hold timer's
//! `AppEvent::Hold(serial)` is delivered directly, except in the one test that lets the real timer
//! fire.
//!
//! Covered by `jmux_touch_gestures_*`: a swipe starting on a tab, on `×`, on an arrow or on the
//! empty bar scrolls an overflowing bar in the finger's direction and moves, focuses and closes
//! nothing (`..._a_swipe_...`, `..._swiping_the_empty_bar_...`); a tap acts at the release, not
//! the press, and a release inside `SLOP` of the press still taps (`..._a_tap_acts_on_release_...`,
//! `..._jitter_...`); a hold lifts the tab, drawn with `tab.lifted`, and it then drags to a gap in
//! its own bar (`..._a_hold_lifts_...`, `..._a_lifted_tab_...`); another bar and a terminal half are
//! in `jmux_touch_tests`; a hold on `×` does nothing and the release closes
//! (`..._a_hold_on_the_close_target_...`); a stale hold timer does nothing (`..._a_stale_hold_...`);
//! the real timer lifts a tab (`..._the_real_timer_...`); a plain client's press drags at once
//! (`..._a_plain_client_...`). Also: a swipe stops at the bar's drawn end, one leaving the bar keeps
//! scrolling, arrow / `+` / `×` act on release (asking for a running command), the wheel scrolls,
//! a swipe moves a long tab column by column (`..._column_by_column`).

use crossterm::event::{MouseButton, MouseEventKind};

use crate::app::{AppEvent, Drag};
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

/// A finger swiping from `from` by `dx` columns on the same row, 2 cells at a time, then up.
fn swipe(h: &mut Harness, from: (u16, u16), dx: i32) {
    press(h, from.0, from.1);
    let step = if dx < 0 { -2 } else { 2 };
    let mut moved: i32 = 0;
    while moved.abs() < dx.abs() {
        moved += step;
        drag(h, (i32::from(from.0) + moved).max(0) as u16, from.1);
    }
    release(h, (i32::from(from.0) + moved).max(0) as u16, from.1);
}

fn hold(h: &mut Harness) {
    let serial = h.app.pending_press_serial().expect("a pending press");
    h.app.handle(AppEvent::Hold(serial));
}

fn lifted(h: &Harness) -> bool {
    matches!(h.app.drag, Some(Drag::Tab(_)))
}

fn runs(h: &Harness) -> Vec<jterm::run::RunId> {
    h.app.groups.active_group().tabs.iter().map(|t| t.run.id).collect()
}

/// An overflowing single bar, scrolled to `scroll`, settled.
async fn overflowing(scroll: usize) -> Harness {
    let mut h = Harness::new(14, 40, "", true).await;
    h.tabs(5).await;
    h.settle("arrows", |h| h.app.hit.groups[0].arrows.is_some()).await;
    let id = h.app.hit.groups[0].id;
    h.app.groups.get_mut(id).unwrap().scroll = scroll;
    h.app.needs_redraw = true;
    h.frame().await;
    assert_eq!(h.app.groups.active_group().scroll, scroll);
    h
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_swipe_on_a_tab_scrolls_with_the_finger_and_changes_nothing() {
    let mut h = overflowing(2).await;
    let active = h.active();
    let runs_before = runs(&h);
    let t = h.app.hit.groups[0].tabs[1];
    // Right: earlier tabs come in.
    swipe(&mut h, (t.x0 + 1, 1), 25);
    let right = h.app.groups.active_group().scroll;
    assert!(right < 2, "scrolled back: {right}");
    h.frame().await;
    // Left: later tabs come in.
    let t = h.app.hit.groups[0].tabs[1];
    swipe(&mut h, (t.x0 + 1, 1), -25);
    assert!(h.app.groups.active_group().scroll > right, "scrolled forward");
    assert_eq!((h.tab_count(), h.active(), runs(&h)), (5, active, runs_before));
    assert!(h.app.drag.is_none() && h.app.confirm.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_swipe_from_the_close_target_or_an_arrow_scrolls_and_closes_nothing() {
    let mut h = overflowing(2).await;
    let t = h.app.hit.groups[0].tabs[0];
    swipe(&mut h, (t.close_x0 + 1, 1), 25);
    assert!(h.app.groups.active_group().scroll < 2, "from the ×");
    assert_eq!(h.tab_count(), 5);
    h.frame().await;

    let id = h.app.hit.groups[0].id;
    h.app.groups.get_mut(id).unwrap().scroll = 1;
    h.app.needs_redraw = true;
    h.frame().await;
    let forward = h.app.hit.groups[0].arrows.clone().unwrap().forward;
    // A tap on the forward arrow would scroll to 2; this swipe goes the other way.
    swipe(&mut h, (forward.start + 1, 1), 25);
    assert_eq!(h.app.groups.active_group().scroll, 0, "from the forward arrow");
    assert_eq!(h.tab_count(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_swiping_the_empty_bar_does_not_focus_the_group() {
    let mut h = Harness::new(20, 61, "", true).await;
    h.app.run(crate::command::Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.app.run(crate::command::Command::FocusSide(crate::group::Side::Left));
    let g = h.app.hit.group(right).unwrap().clone();
    let x = g.tabs[0].x1 + 3;
    assert!(x < g.new_tab.clone().unwrap().start, "empty bar beside the tab");
    swipe(&mut h, (x, 1), 10);
    assert_eq!(h.app.groups.active, left, "a swipe focuses nothing");
    // A tap on it does focus its group.
    tap(&mut h, x, 1);
    assert_eq!(h.app.groups.active, right);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_tap_acts_on_release_not_press() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    assert_eq!(h.active(), 2);
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.x0 + 1, 1);
    assert_eq!(h.active(), 2, "not focused by the press");
    release(&mut h, t.x0 + 1, 1);
    assert_eq!(h.active(), 0);
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.close_x0 + 1, 1);
    assert_eq!(h.tab_count(), 3, "not closed by the press");
    release(&mut h, t.close_x0 + 1, 1);
    assert_eq!(h.tab_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_jitter_within_slop_still_taps_or_holds() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.x0 + 3, 1);
    drag(&mut h, t.x0 + 4, 2);
    release(&mut h, t.x0 + 4, 2);
    assert_eq!((h.active(), h.app.groups.active_group().scroll), (0, 0), "a tap");
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.x0 + 3, 1);
    drag(&mut h, t.x0 + 2, 0);
    hold(&mut h);
    assert!(lifted(&h), "still a hold");
    release(&mut h, t.x0 + 2, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_hold_lifts_the_tab_and_drawn_with_tab_lifted() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t = h.app.hit.groups[0].tabs[1];
    let normal = h.bg_at(t.x0 + 1, 0);
    press(&mut h, t.x0 + 1, 1);
    assert!(!lifted(&h) && h.active() == 2, "nothing before the hold");
    hold(&mut h);
    assert!(lifted(&h));
    assert_eq!(h.active(), 1, "the lifted tab is active");
    h.frame().await;
    let lifted_bg = vt100::Color::Rgb(0x0E, 0x63, 0x9C);
    assert_ne!(normal, lifted_bg);
    for y in 0..3 {
        assert_eq!(h.bg_at(t.x0 + 1, y), lifted_bg, "lifted on row {y}");
    }
    // Put down where it was: nothing moves.
    let before = runs(&h);
    release(&mut h, t.x0 + 1, 1);
    h.frame().await;
    assert_eq!((runs(&h), h.active(), h.tab_count()), (before, 1, 3));
    assert!(h.app.drag.is_none());
    assert_ne!(h.bg_at(t.x0 + 1, 0), lifted_bg, "set down");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_lifted_tab_drags_to_a_gap_in_its_own_bar() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let before = runs(&h);
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.x0 + 1, 1);
    hold(&mut h);
    let last = h.app.hit.groups[0].tabs[2];
    drag(&mut h, last.x1 + 1, 1);
    h.frame().await;
    release(&mut h, last.x1 + 1, 1);
    h.frame().await;
    assert_eq!(runs(&h), vec![before[1], before[2], before[0]], "moved to the end");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_hold_on_the_close_target_does_nothing_and_the_release_closes() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.close_x0 + 1, 1);
    hold(&mut h);
    assert!(!lifted(&h) && h.app.pending_press_serial().is_some());
    assert_eq!(h.tab_count(), 3);
    release(&mut h, t.close_x0 + 1, 1);
    assert_eq!(h.tab_count(), 2, "the release was a tap");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_stale_hold_does_nothing() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t0 = h.app.hit.groups[0].tabs[0];
    // The press became a tap.
    press(&mut h, t0.x0 + 1, 1);
    let first = h.app.pending_press_serial().unwrap();
    release(&mut h, t0.x0 + 1, 1);
    h.app.handle(AppEvent::Hold(first));
    assert!(h.app.drag.is_none(), "no press to lift");
    // A newer press: the old serial doesn't lift it, its own does.
    let t1 = h.app.hit.groups[0].tabs[1];
    press(&mut h, t1.x0 + 1, 1);
    let second = h.app.pending_press_serial().unwrap();
    assert_ne!(first, second);
    h.app.handle(AppEvent::Hold(first));
    assert!(!lifted(&h));
    h.app.handle(AppEvent::Hold(second));
    assert!(lifted(&h));
    release(&mut h, t1.x0 + 1, 1);
    // A press that became a swipe is not lifted by its timer.
    let g = h.app.hit.groups[0].tabs[0];
    press(&mut h, g.x0 + 1, 1);
    let third = h.app.pending_press_serial().unwrap();
    drag(&mut h, g.x0 + 8, 1);
    h.app.handle(AppEvent::Hold(third));
    assert!(!lifted(&h));
    release(&mut h, g.x0 + 8, 1);
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_the_real_timer_lifts_a_tab() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.x0 + 1, 1);
    h.settle("the tab lifted", |h| lifted(h) && h.active() == 1).await;
    release(&mut h, t.x0 + 1, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_plain_client_press_drags_at_once() {
    let mut h = Harness::new(16, 70, "", false).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.x0 + 1, 0);
    assert!(lifted(&h), "a drag at once");
    assert_eq!(h.active(), 0, "and the tab focused on the press");
    release(&mut h, t.x0 + 1, 0);
    assert!(h.app.pending_press_serial().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_swipe_never_scrolls_past_what_the_bar_draws() {
    // Many narrow tabs, so the last few fit together and the bar's end is before the last tab.
    let mut h = Harness::new(14, 70, "", true).await;
    h.tabs(12).await;
    h.settle("arrows", |h| h.app.hit.groups[0].arrows.is_some()).await;
    let id = h.app.hit.groups[0].id;
    h.app.groups.get_mut(id).unwrap().scroll = 0;
    h.app.needs_redraw = true;
    h.frame().await;
    assert_eq!(h.app.groups.active_group().scroll, 0);
    let t = h.app.hit.groups[0].tabs[0];
    // Swipe to the bar's end, then one long motion with no frame in between: nothing clamps it but
    // the swipe itself.
    for _ in 0..12 {
        swipe(&mut h, (t.x0 + 40, 1), -30);
    }
    let end = h.app.groups.active_group().scroll;
    press(&mut h, 65, 1);
    drag(&mut h, 5, 1);
    release(&mut h, 5, 1);
    let swiped = h.app.groups.active_group().scroll;
    h.frame().await;
    let drawn = h.app.groups.active_group().scroll;
    assert_eq!(swiped, drawn, "the swipe stopped where the bar does");
    assert!(
        end == drawn && drawn > 0 && drawn < 11,
        "the bar ends before the last tab: {drawn}"
    );
    // So a short swipe back moves the bar at once.
    swipe(&mut h, (t.x0 + 3, 1), 30);
    assert!(
        h.app.groups.active_group().scroll < drawn,
        "a reverse swipe is not sluggish"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_swipe_leaving_the_bar_keeps_scrolling_until_release() {
    let mut h = overflowing(3).await;
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.x0 + 1, 1);
    drag(&mut h, t.x0 + 8, 1);
    let first = h.app.groups.active_group().scroll;
    // Down into the terminal: still the same swipe.
    for k in 2..8 {
        drag(&mut h, t.x0 + 4 * k, 9);
    }
    assert!(
        h.app.groups.active_group().scroll < first,
        "kept scrolling below the bar"
    );
    release(&mut h, t.x0 + 30, 9);
    assert!(h.app.drag.is_none() && h.tab_count() == 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_an_arrow_and_the_plus_act_on_release_not_press() {
    let mut h = overflowing(1).await;
    let arrows = h.app.hit.groups[0].arrows.clone().unwrap();
    press(&mut h, arrows.forward.start + 1, 1);
    assert_eq!(h.app.groups.active_group().scroll, 1, "not scrolled by the press");
    release(&mut h, arrows.forward.start + 1, 1);
    assert_eq!(h.app.groups.active_group().scroll, 2, "scrolled by the tap");
    press(&mut h, arrows.back.start + 1, 1);
    // A hold on an arrow does nothing; its release is still a tap.
    hold(&mut h);
    assert!(!lifted(&h));
    release(&mut h, arrows.back.start + 1, 1);
    assert_eq!(h.app.groups.active_group().scroll, 1);

    let plus = h.app.hit.groups[0].new_tab.clone().unwrap();
    press(&mut h, plus.start + 1, 1);
    assert_eq!(h.tab_count(), 5, "no tab from the press");
    hold(&mut h);
    assert!(!lifted(&h));
    release(&mut h, plus.start + 1, 1);
    assert_eq!(h.tab_count(), 6, "a tab from the tap");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_tapping_the_close_target_of_a_running_command_asks() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(2).await;
    h.app.type_text("printf '\\033]133;B\\a'; sleep 30\n");
    h.settle("a running command", |h| {
        h.app.groups.active_group().active_tab().is_some_and(|t| !t.idle)
    })
    .await;
    h.frame().await;
    let t = h.app.hit.groups[0].tabs[1];
    press(&mut h, t.close_x0 + 1, 1);
    assert!(h.app.confirm.is_none(), "not asked by the press");
    release(&mut h, t.close_x0 + 1, 1);
    assert!(h.app.confirm.is_some(), "asked by the tap");
    assert_eq!(h.tab_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_the_wheel_scrolls_and_a_lifted_tab_set_down_stays() {
    let mut h = overflowing(2).await;
    // The wheel on the bar still scrolls in the touch layout.
    h.app.send_mouse(MouseEventKind::ScrollUp, 10, 1);
    assert_eq!(h.app.groups.active_group().scroll, 1, "the wheel scrolls the bar");
    let before = runs(&h);
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.x0 + 1, 1);
    hold(&mut h);
    drag(&mut h, t.x0 + 2, 1);
    release(&mut h, t.x0 + 1, 1);
    assert_eq!((runs(&h), h.tab_count()), (before, 5));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_plain_clients_press_on_the_arrow_and_close_target_acts_at_once() {
    let mut h = Harness::new(14, 40, "", false).await;
    h.tabs(5).await;
    h.settle("arrows", |h| h.app.hit.groups[0].arrows.is_some()).await;
    let arrows = h.app.hit.groups[0].arrows.clone().unwrap();
    let before = h.app.groups.active_group().scroll;
    press(&mut h, arrows.back.start, 0);
    assert_eq!(
        h.app.groups.active_group().scroll,
        before - 1,
        "the arrow scrolls on press"
    );
    release(&mut h, arrows.back.start, 0);
    let t = h.app.hit.groups[0].tabs[0];
    press(&mut h, t.close_x0, 0);
    assert_eq!(h.tab_count(), 4, "the × closes on press");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_gestures_a_swipe_moves_a_long_tab_column_by_column() {
    // A first tab wider than the bar: the swipe moves through it, not a whole tab at a time.
    let mut h = Harness::new(14, 40, "", true).await;
    h.tabs(2).await;
    let id = h.app.hit.groups[0].id;
    let g = h.app.groups.get_mut(id).unwrap();
    g.tabs[0].title = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ ○".into();
    g.active = 0;
    h.app.needs_redraw = true;
    h.frame().await;
    h.settle("the long title", |h| h.row(1).contains("abcdefgh")).await;
    let left = h.app.hit.groups[0].arrows.clone().unwrap().back.end;
    let t = h.app.hit.groups[0].tabs[0];
    assert_eq!((t.x0, h.app.groups.active_group().scroll), (left, 0));

    swipe(&mut h, (t.x0 + 10, 1), -6);
    let g = h.app.groups.active_group();
    assert_eq!((g.scroll, g.shift), (0, 6), "six columns into the first tab");
    h.app.needs_redraw = true;
    h.frame().await;
    h.settle("the title moved", |h| {
        h.row(1).contains("efghij") && !h.row(1).contains("abcd")
    })
    .await;
    assert_eq!(h.app.hit.groups[0].tabs[0].x0, left, "the cut tab starts at the arrow");

    // Back the same way, then forward and the back arrow, which shows the cut tab whole.
    swipe(&mut h, (t.x0 + 10, 1), 6);
    assert_eq!(h.app.groups.active_group().shift, 0);
    swipe(&mut h, (t.x0 + 10, 1), -4);
    assert_eq!(h.app.groups.active_group().shift, 4);
    h.app.needs_redraw = true;
    h.frame().await;
    let back = h.app.hit.groups[0].arrows.clone().unwrap().back;
    tap(&mut h, back.start + 1, 1);
    let g = h.app.groups.active_group();
    assert_eq!((g.scroll, g.shift), (0, 0), "the back arrow shows the cut tab");

    // A long swipe stops with the last tab's right edge at the bar's.
    swipe(&mut h, (30, 1), -60);
    h.app.needs_redraw = true;
    h.frame().await;
    let g = h.app.groups.active_group();
    assert!(g.scroll == 0 && g.shift > 6, "still in the long tab: {}", g.shift);
    let forward = h.app.hit.groups[0].arrows.clone().unwrap().forward;
    assert_eq!(h.app.hit.groups[0].tabs[1].x1, forward.start);
}
