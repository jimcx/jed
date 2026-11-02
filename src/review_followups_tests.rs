//! Code review follow-ups, subtask a: a mouse move redraws only when something it shows changed.
//! Crossterm's mouse capture reports every pointer move, so 100 moves over plain editor text ask
//! for no frame at all; and each hover effect a move still has (Source Control row buttons, a
//! Search result's `×`, the scroll-bar thumb colour, closing the hover popup by moving away, the
//! tab tooltip) asks for one, with the drawn frame showing the change.

use crossterm::event::MouseEventKind as E;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::app::App;
use crate::buffer::Buffer;
use crate::layout::{GroupId, ScrollTarget};
use crate::milestone_tests::test_app;

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// An app on `n` numbered lines in `a.txt`, drawn once.
fn long_file_app(dir: &tempfile::TempDir, n: usize) -> App {
    let root = crate::workspace::normalize(dir.path());
    let text: String = (0..n).map(|i| format!("line number {i}\n")).collect();
    std::fs::write(root.join("a.txt"), text).unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    draw(&mut app);
    app
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moves_over_plain_text_ask_for_no_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 50);
    let text = app.hits.group(app.groups.active).unwrap().text;
    app.mouse(E::Moved, text.x + 3, text.y + 2);
    draw(&mut app);
    app.test_take_redraw();
    for i in 0..100u16 {
        app.mouse(E::Moved, text.x + 2 + i % 6, text.y + 2 + i % 3);
    }
    assert!(!app.test_take_redraw(), "100 moves over text: no frame");
}

/// The thumb's cells' colours as drawn.
fn thumb_cells(app: &App, term: &Terminal<TestBackend>, g: GroupId) -> Vec<(Color, Color)> {
    let bar = app.hits.scrollbar(ScrollTarget::Group(g)).expect("a bar");
    let th = bar.thumb.expect("a thumb");
    let buf = term.backend().buffer();
    (th.start..th.start + th.len)
        .map(|d| {
            let c = &buf[(bar.track.x, bar.track.y + d)];
            (c.fg, c.bg)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_onto_the_scrollbar_thumb_redraws_with_the_hover_colour() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 200);
    let g = app.groups.active;
    let bar = app.hits.scrollbar(ScrollTarget::Group(g)).unwrap();
    let thumb = bar.thumb.unwrap();
    let text = app.hits.group(g).unwrap().text;
    app.mouse(E::Moved, text.x + 3, text.y + 1);
    let t = draw(&mut app);
    let before = thumb_cells(&app, &t, g);
    app.test_take_redraw();
    app.mouse(E::Moved, bar.track.x, bar.track.y + thumb.start);
    assert!(app.test_take_redraw(), "onto the thumb");
    let t = draw(&mut app);
    let over = thumb_cells(&app, &t, g);
    assert_ne!(before, over, "the thumb is drawn in its hover colour");
    app.mouse(E::Moved, text.x + 3, text.y + 1);
    assert!(app.test_take_redraw(), "off the thumb");
    let t = draw(&mut app);
    assert_eq!(thumb_cells(&app, &t, g), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_over_a_source_control_row_redraws_with_its_buttons() {
    use crate::milestone10_tests::{ctrl_shift_g, dirs, file, scm_row, settle_git};
    use crate::scm::Section;
    let (_d, root, state) = dirs();
    crate::milestone10_tests::scm_repo(&root);
    let ws = crate::workspace::Workspace::load(root.clone());
    let trust = crate::workspace::trust::TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app.test_plain_whitespace();
    assert!(settle_git(&mut app, |s| s.entries.len() == 5).await);
    ctrl_shift_g(&mut app);
    let key = file(Section::Changes, "a.txt");
    let (x, y) = scm_row(&mut app, &key);
    let has_button = |app: &App| app.hits.scm.as_ref().unwrap().buttons.iter().any(|(_, k, _)| *k == key);
    assert!(!has_button(&app), "no pointer: no button on an unselected row");
    app.test_take_redraw();
    app.mouse(E::Moved, x, y);
    assert!(app.test_take_redraw(), "onto the row");
    draw(&mut app);
    assert!(has_button(&app), "the + / − button is drawn");
    app.mouse(E::Moved, x, y);
    assert!(!app.test_take_redraw(), "same row again: nothing changed");
    app.mouse(E::Moved, 0, 0);
    assert!(app.test_take_redraw(), "off the row");
    draw(&mut app);
    assert!(!has_button(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_over_a_search_result_redraws_with_its_cross() {
    use crate::replace_tests::{draw as rdraw, key, open_app, search_for, term};
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut t = term();
    search_for(&mut app, "foo").await;
    rdraw(&mut t, &mut app);
    let list = app.hits.search.as_ref().unwrap().list;
    let cross = |app: &App| !app.hits.search.as_ref().unwrap().dismiss.is_empty();
    assert!(!cross(&app));
    app.test_take_redraw();
    app.mouse(E::Moved, list.x + 5, list.y);
    assert!(app.test_take_redraw(), "onto a result");
    rdraw(&mut t, &mut app);
    assert!(cross(&app), "its × is drawn");
    assert_eq!(app.search_view.hover, Some(key("a.txt", 0)));
    app.mouse(E::Moved, 0, 0);
    assert!(app.test_take_redraw(), "off it");
    rdraw(&mut t, &mut app);
    assert!(!cross(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_off_a_hover_popup_target_closes_it_with_a_frame() {
    use crate::fixes_tests::{cell, fixes_app, rest, terminal};
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = fixes_app(dir.path(), true).await;
    let mut term = terminal();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let at = cell(&app, 6, 5);
    assert!(rest(&mut app, &mut term, at));
    assert!(app.hover.popup.is_some());
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.hover.is_some(), "the popup is drawn");
    app.test_take_redraw();
    let away = cell(&app, 6, 3);
    app.mouse(E::Moved, away.0, away.1);
    assert!(app.hover.popup.is_none());
    assert!(app.test_take_redraw(), "closing the popup asks for a frame");
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.hover.is_none(), "and the frame has no popup");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_the_tab_tooltip_shows_after_its_rest_and_goes_with_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 5);
    let tab = app.hits.group(app.groups.active).unwrap().tab_bar;
    let (x, y) = (tab.x + 2, tab.y);
    let drawn = |app: &mut App| crate::milestone10_tests::find_text(&draw(app), "a.txt").len();
    let plain = drawn(&mut app);
    app.test_take_redraw();
    app.mouse(E::Moved, x, y);
    assert!(app.tab_tip.deadline().is_some(), "the timer runs");
    assert!(!app.test_take_redraw(), "starting the rest needs no frame");
    let deadline = app.tab_tip.deadline().unwrap();
    app.test_tab_tip_tick(deadline);
    assert!(app.test_take_redraw(), "the tooltip appears");
    assert!(app.tab_tip.shown.is_some());
    assert!(drawn(&mut app) > plain, "the frame shows the tooltip");
    app.mouse(E::Moved, x, y);
    assert!(!app.test_take_redraw(), "still on the tab");
    app.mouse(E::Moved, tab.x + 2, tab.y + 5);
    assert!(app.test_take_redraw(), "moving off drops it");
    assert_eq!(drawn(&mut app), plain);
}
