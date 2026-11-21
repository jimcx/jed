//! End-to-end tests of jmux_touch_menu (the touch layout's `☰` menu in place of the tab bars),
//! driving the server `App` headlessly through `Harness` of `jmux_touch_tests`; taps go through the
//! touch gesture path (a press, then a release within `SLOP`).
//!
//! Covered by `jmux_touch_menu_*`: with `--touch` no group draws a bar, each shell is the bar's
//! rows taller and told so, the status bar starts with `☰`, and a plain client keeps its bars with
//! no `☰` (`..._the_layout_...`); the menu drawn above the status bar at a phone's width and a wide
//! one, active tab highlighted, an empty row between items (`..._drawn_...`), the shell's text
//! blanked under it (`..._hides_...`); a hit on each row of
//! each item (`..._every_row_...`); picking a tab, New Tab, and an action added in one line
//! (`..._picking_a_tab_...`, `..._new_tab_...`, `..._an_added_action_...`); an outside tap and a
//! tap on `☰` close without acting, a tap acts at the release (`..._outside_...`, `..._acts_on_...`);
//! a swipe scrolls a long menu and picks nothing, a hold does nothing (`..._a_swipe_...`,
//! `..._a_hold_...`); a plain client's click picks (`..._a_plain_client_...`); `menu.toggle`, Up /
//! Down / Enter / Escape and swallowed keys (`..._keys_...`); only the focused group's tabs
//! (`..._the_focused_groups_tabs_...`). A takeover closes an open menu (`..._a_takeover_...`); the layout switch is in `jmux_touch_tests`.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use jtui::keymap::KeyCommand;

use crate::app::AppEvent;
use crate::command::{Command, Context, MenuCommand};
use crate::hit::Hit;
use crate::jmux_touch_tests::{BOTTOM, Harness, tap};
use crate::menu::Row;

fn press(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Down(MouseButton::Left), x, y);
}

fn drag(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
}

fn release(h: &mut Harness, x: u16, y: u16) {
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The status bar's `☰` cell.
fn button(h: &Harness) -> (u16, u16) {
    (1, h.rows - 1)
}

/// Taps `☰` and waits for the menu's frame.
async fn open(h: &mut Harness) {
    let (x, y) = button(h);
    tap(h, x, y);
    h.settle("the open menu", |h| h.app.menu.is_some() && h.app.hit.menu.is_some())
        .await;
}

fn rows_of(h: &Harness) -> (ratatui::layout::Rect, Vec<Row>) {
    let m = h.app.hit.menu.clone().expect("a drawn menu");
    (m.area, m.rows)
}

/// The y of item `i`'s text row on screen.
fn text_y(h: &Harness, i: usize) -> u16 {
    let (area, rows) = rows_of(h);
    area.y + rows.iter().position(|r| *r == Row::Text(i)).expect("the item is drawn") as u16
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_the_layout_has_no_bars_and_a_button() {
    let plain = Harness::new(14, 60, "", false).await;
    for text in ["", BOTTOM] {
        let mut h = Harness::new(14, 60, text, true).await;
        h.settle("the shell fits", |h| h.shells_fit()).await;
        let g = h.app.hit.groups[0].clone();
        assert_eq!((g.bar.height, g.term.height, g.term.y), (0, 12, 0), "{text:?}");
        assert!(g.tabs.is_empty() && g.arrows.is_none());
        let id = g.id;
        assert_eq!(h.shell_size(id), (12, 59), "told the group's full height");
        assert_eq!(
            h.shell_size(id).0,
            plain.shell_size(plain.app.hit.groups[0].id).0,
            "the bar's row goes to the second status row"
        );
        assert!(!h.row(0).contains("fakesh ○ ×"), "no tab bar drawn: {}", h.row(0));
        assert!(
            h.row(12).starts_with(" ☰  work") && h.row(12).trim_end().ends_with("1 terminal"),
            "the button, the session and the counts: {:?}",
            h.row(12)
        );
        assert!(
            h.row(13).starts_with("     fakesh") && h.row(13).trim_end().ends_with("detach"),
            "the title and the hint below: {:?}",
            h.row(13)
        );
        for (x, y) in [(0, 12), (3, 12), (0, 13), (3, 13)] {
            assert_eq!(h.app.hit.at(x, y), Some(Hit::Menu), "the button spans both rows");
        }
        assert_eq!(h.app.hit.at(4, 12), None);
        assert_eq!(h.app.hit.at(4, 13), None);
        // A new shell starts at the full height too.
        h.app.run(Command::NewTerminal);
        assert_eq!(h.shell_size(id), (12, 59));
    }
    assert_eq!(plain.app.hit.groups[0].bar.height, 1);
    assert!(plain.app.hit.menu_button.is_none());
    assert!(!plain.row(13).contains('☰') && plain.row(0).contains("fakesh"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_is_drawn_above_the_status_bar_with_padded_items() {
    for (cols, width) in [(36u16, 36u16), (100, 40)] {
        let mut h = Harness::new(30, cols, "", true).await;
        h.tabs(3).await;
        open(&mut h).await;
        let (area, rows) = rows_of(&h);
        // pad, tab, pad, tab, pad, tab, pad, separator, pad, New Tab, pad
        assert_eq!((area.x, area.width, area.height), (0, width, 11), "{cols}");
        assert_eq!(area.y + area.height, 28, "the bottom edge is the status bar's top");
        assert_eq!(rows.len(), 11);
        for i in 0..3 {
            let y = text_y(&h, i);
            assert!(h.row(y).trim_start().starts_with("fakesh"), "{}", h.row(y));
            assert!(h.row(y - 1).trim().is_empty() && h.row(y + 1).trim().is_empty());
        }
        let new = text_y(&h, 3);
        assert!(h.row(new).trim_start().starts_with("New Tab"), "{}", h.row(new));
        assert!(
            h.row(new - 2).starts_with('─'),
            "a separator before: {:?}",
            h.row(new - 2)
        );
        // The active tab is drawn highlighted, the others and New Tab are not.
        let active = h.active();
        let bg = |h: &Harness, i: usize| h.bg_at(width - 1, text_y(h, i));
        for i in 0..4 {
            assert_eq!(bg(&h, i) == bg(&h, active), i == active, "item {i}");
        }
        if cols > width {
            assert_ne!(
                h.bg_at(width + 2, text_y(&h, 0)),
                bg(&h, 1),
                "beyond the menu is the terminal"
            );
        }
        // The menu button is highlighted while it is open.
        assert_ne!(h.bg_at(1, 29), h.bg_at(10, 29));
    }
}

/// The menu blanks the shell text under it: no character drawn behind it shows through.
#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_hides_the_text_behind_it() {
    let mut h = Harness::new(16, 40, "", true).await;
    h.settle("a prompt", |h| h.text().contains("$")).await;
    let lines: String = (1..=20).map(|i| format!("{}{i}\\n", "X".repeat(36))).collect();
    h.app.type_text(&format!("printf '{lines}'\n"));
    h.settle("the output", |h| h.text().contains("XXX20")).await;
    open(&mut h).await;
    let (area, rows) = rows_of(&h);
    for (r, row) in rows.iter().enumerate() {
        let y = area.y + r as u16;
        let line: String = h.row(y).chars().take(usize::from(area.width)).collect();
        assert!(!line.contains('X'), "row {y} ({row:?}) shows the shell: {line:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_every_row_of_an_item_picks_it() {
    let mut h = Harness::new(30, 36, "", true).await;
    h.tabs(3).await;
    open(&mut h).await;
    let (area, rows) = rows_of(&h);
    for (r, row) in rows.iter().enumerate() {
        let y = area.y + r as u16;
        let want = match row {
            Row::Pad(i) | Row::Text(i) => Hit::MenuItem(*i),
            Row::Separator => Hit::MenuDead,
        };
        for x in [0, 10, 35] {
            assert_eq!(h.app.hit.at(x, y), Some(want), "({x}, {y})");
        }
    }
    // Every tab item has its text row and a padding row to tap, and the first tab's rows pick it.
    let texts: Vec<u16> = (0..3).map(|i| text_y(&h, i)).collect();
    #[allow(clippy::needless_range_loop)]
    for i in 0..3 {
        let owned: Vec<u16> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Pad(j) | Row::Text(j) if *j == i))
            .map(|(r, _)| area.y + r as u16)
            .collect();
        assert!(owned.len() >= 2 && owned.contains(&texts[i]), "{owned:?}");
        for y in owned {
            if h.app.menu.is_none() {
                h.app.run(Command::Menu(MenuCommand::Toggle));
                h.app.needs_redraw = true;
            }
            h.settle("the menu", |h| h.app.menu.is_some() && h.app.hit.menu.is_some())
                .await;
            let (area, _) = rows_of(&h);
            assert_eq!(area.y + area.height, 28);
            tap(&mut h, 4, y);
            assert!(h.app.menu.is_none(), "row {y} of item {i} picks it and closes the menu");
            assert_eq!(h.active(), i, "row {y}");
            h.settle("the menu gone", |h| h.app.hit.menu.is_none()).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_picking_a_tab_focuses_it_and_closes_the_menu() {
    let mut h = Harness::new(20, 50, "", true).await;
    h.tabs(3).await;
    assert_eq!(h.active(), 2);
    open(&mut h).await;
    let y = text_y(&h, 0);
    tap(&mut h, 6, y);
    assert_eq!((h.active(), h.tab_count(), h.app.menu.is_none()), (0, 3, true));
    h.settle("the menu gone", |h| !h.text().contains("New Tab")).await;
    assert!(h.app.hit.menu.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_new_tab_opens_a_terminal_and_closes_the_menu() {
    let mut h = Harness::new(20, 50, "", true).await;
    open(&mut h).await;
    let y = text_y(&h, 1);
    assert!(h.row(y).contains("New Tab"));
    tap(&mut h, 6, y);
    h.settle("a second tab", |h| {
        h.tab_count() == 2 && h.shells_fit() && h.app.menu.is_none()
    })
    .await;
    assert_eq!(h.active(), 1);
    assert!(!h.text().contains("New Tab"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_an_added_action_is_one_line_in_the_list() {
    let mut h = Harness::new(20, 50, "", true).await;
    // The shipped list is New Tab alone.
    assert_eq!(crate::menu::actions().len(), 1);
    h.app.menu_actions.push(("Split Right", Command::SplitRight));
    open(&mut h).await;
    let y = text_y(&h, 2);
    assert!(h.row(y).contains("Split Right"), "{}", h.row(y));
    assert!(h.row(y + 1).trim().is_empty());
    tap(&mut h, 6, y);
    h.settle("a split", |h| h.app.hit.groups.len() == 2 && h.app.menu.is_none())
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_tap_outside_or_on_the_button_closes_without_acting() {
    let mut h = Harness::new(20, 50, "", true).await;
    h.tabs(2).await;
    let before = (h.active(), h.tab_count(), h.app.groups.active);
    for (x, y) in [(30, 2), (20, 19), (45, 5)] {
        open(&mut h).await;
        tap(&mut h, x, y);
        assert!(h.app.menu.is_none(), "({x}, {y}) closes it");
        assert_eq!((h.active(), h.tab_count(), h.app.groups.active), before);
        assert!(h.app.drag.is_none());
        h.settle("the menu gone", |h| h.app.hit.menu.is_none()).await;
    }
    // `☰` again closes it, and the tap before opened it.
    open(&mut h).await;
    tap(&mut h, 0, 19);
    assert!(h.app.menu.is_none());
    assert_eq!(h.tab_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_tap_acts_on_release_and_tolerates_jitter() {
    let mut h = Harness::new(20, 50, "", true).await;
    h.tabs(2).await;
    open(&mut h).await;
    let y = text_y(&h, 0);
    press(&mut h, 6, y);
    assert!(h.app.menu.is_some() && h.active() == 1, "the press alone picks nothing");
    release(&mut h, 7, y);
    assert!(h.app.menu.is_none());
    assert_eq!(h.active(), 0, "a release within SLOP of the press taps");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_swipe_scrolls_a_long_menu_and_picks_nothing() {
    // 9 rows above the two-row status bar; 3 tabs and New Tab are 11 rows.
    let mut h = Harness::new(11, 40, "", true).await;
    h.tabs(3).await;
    open(&mut h).await;
    let (area, _) = rows_of(&h);
    assert_eq!((area.y, area.height), (0, 9));
    assert_eq!(h.app.menu.as_ref().unwrap().scroll, 0);
    let active = h.active();
    let count = h.tab_count();
    // The items follow the finger: up brings later rows in.
    press(&mut h, 6, 6);
    for y in [5, 4, 3] {
        drag(&mut h, 6, y);
    }
    release(&mut h, 6, 3);
    let m = h.app.menu.clone().expect("still open");
    assert_eq!(m.scroll, 3.min(m.max_scroll));
    assert_eq!(m.max_scroll, 2);
    assert_eq!((h.active(), h.tab_count()), (active, count), "nothing picked");
    h.frame().await;
    let new = text_y(&h, 3);
    assert!(h.row(new).contains("New Tab"), "the last rows came in: {}", h.text());
    // And back down.
    press(&mut h, 6, 3);
    for y in [4, 5, 6, 7] {
        drag(&mut h, 6, y);
    }
    release(&mut h, 6, 7);
    assert_eq!(h.app.menu.as_ref().unwrap().scroll, 0);
    assert_eq!((h.active(), h.tab_count()), (active, count));
    // The wheel scrolls it too.
    h.app.send_mouse(MouseEventKind::ScrollDown, 6, 4);
    assert_eq!(h.app.menu.as_ref().unwrap().scroll, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_hold_does_nothing() {
    let mut h = Harness::new(20, 50, "", true).await;
    h.tabs(2).await;
    open(&mut h).await;
    let y = text_y(&h, 0);
    press(&mut h, 6, y);
    let serial = h.app.pending_press_serial().expect("a pending press");
    h.app.handle(AppEvent::Hold(serial));
    assert!(h.app.menu.is_some());
    release(&mut h, 6, y);
    assert!(h.app.menu.is_some(), "the release after a hold picks nothing");
    assert_eq!((h.active(), h.tab_count()), (1, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_plain_clients_click_picks() {
    let mut h = Harness::new(20, 50, "", false).await;
    h.tabs(2).await;
    assert!(h.app.hit.menu_button.is_none());
    h.app.run(Command::Menu(MenuCommand::Toggle));
    h.settle("the menu", |h| h.app.hit.menu.is_some()).await;
    let y = text_y(&h, 0);
    h.click(MouseButton::Left, 5, y);
    release(&mut h, 5, y);
    assert_eq!((h.app.menu.is_none(), h.active()), (true, 0));
    h.settle("the bars", |h| {
        h.app.hit.menu.is_none() && h.app.hit.groups[0].bar.height == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_keys_move_pick_and_close() {
    let mut h = Harness::new(20, 50, "", true).await;
    h.tabs(3).await;
    assert_eq!(h.app.context(), Context::Terminal);
    h.app.run(Command::Menu(MenuCommand::Toggle));
    assert_eq!(h.app.context(), Context::Menu);
    h.frame().await;
    // From the active tab (the third): Up goes to the second, Down twice to New Tab, then it stops.
    h.app.press_bound(Command::Menu(MenuCommand::Up));
    assert_eq!(h.app.menu.as_ref().unwrap().highlight, Some(1));
    h.app.press_bound(Command::Menu(MenuCommand::Down));
    h.app.press_bound(Command::Menu(MenuCommand::Down));
    h.app.press_bound(Command::Menu(MenuCommand::Down));
    assert_eq!(h.app.menu.as_ref().unwrap().highlight, Some(3));
    h.frame().await;
    let y = text_y(&h, 3);
    assert_ne!(h.bg_at(5, y), h.bg_at(5, text_y(&h, 0)), "the highlight is drawn");
    // Unbound keys are swallowed: nothing reaches the shell, the menu stays.
    h.app.type_text("echo hi");
    h.app.press(KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(h.app.menu.is_some() && h.tab_count() == 3);
    h.frame().await;
    assert!(!h.text().contains("echo hi"), "{}", h.text());
    // Enter on New Tab opens a terminal and closes the menu.
    h.app.press_bound(Command::Menu(MenuCommand::Pick));
    assert!(h.app.menu.is_none());
    h.settle("a fourth tab", |h| h.tab_count() == 4 && h.shells_fit()).await;
    // Enter on a tab focuses it; Escape closes without picking; the toggle closes.
    h.app.run(Command::Menu(MenuCommand::Toggle));
    h.app.press_bound(Command::Menu(MenuCommand::Up));
    h.app.press_bound(Command::Menu(MenuCommand::Up));
    h.app.press_bound(Command::Menu(MenuCommand::Pick));
    assert_eq!((h.app.menu.is_none(), h.active()), (true, 1));
    h.app.run(Command::Menu(MenuCommand::Toggle));
    h.app.press_bound(Command::Menu(MenuCommand::Close));
    assert!(h.app.menu.is_none() && h.app.context() == Context::Terminal);
    h.app.run(Command::Menu(MenuCommand::Toggle));
    h.app.run(Command::Menu(MenuCommand::Toggle));
    assert!(h.app.menu.is_none());
    assert_eq!(h.active(), 1);
    assert_eq!(Command::Menu(MenuCommand::Toggle).title(), "Toggle Tab Menu");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_lists_only_the_focused_groups_tabs() {
    let mut h = Harness::new(24, 80, "", true).await;
    h.tabs(3).await;
    h.app.run(Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    assert!(h.app.hit.groups.iter().all(|g| g.bar.height == 0));
    // The new group (focused) has one tab; the other has three.
    open(&mut h).await;
    let items = h.app.menu_items();
    assert_eq!(items.len(), 2, "one tab and New Tab: {items:?}");
    tap(&mut h, 79, 3);
    // Tap the first group's terminal to focus it: its three tabs are listed.
    tap(&mut h, 2, 3);
    h.frame().await;
    open(&mut h).await;
    assert_eq!(h.app.menu_items().len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_menu_a_takeover_closes_an_open_menu() {
    let mut h = Harness::new(20, 50, "", true).await;
    open(&mut h).await;
    h.take_over(2, false).await;
    assert!(h.app.menu.is_none(), "the new client starts without the old one menu");
    assert!(h.app.hit.menu_button.is_none(), "a plain client has no menu button");
}
