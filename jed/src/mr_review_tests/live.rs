//! Threads and drafts placed on the live text of a review diff: lines typed above them (unsaved)
//! move them and their blocks, an edit of their own lines keeps them there, and only a commit
//! changing those lines outdates a thread; a new draft's lines are counted in the merge request's
//! head, and lines that exist only locally can't be commented on. A gutter bar marks the lines
//! each thread and draft is on.

use super::suggest::{boot, lines, message, on_line3, select, text, world};
use super::*;
use crate::diff_caret::Caret;
use crate::forge::Side;
use crate::review_anchor::Placement;

/// The one thread's placement.
fn place(app: &App) -> Placement {
    app.review.review().unwrap().threads[0].place.clone()
}

/// The lines (0-based) the review diff tab's blocks follow, by role.
fn block_lines(app: &App) -> Vec<(crate::review_blocks::Role, usize)> {
    let tab = app
        .group()
        .tabs
        .iter()
        .find_map(|t| t.diff().filter(|d| d.pair.is_review()));
    tab.unwrap().threads.list().iter().map(|b| (b.role, b.line)).collect()
}

/// `mod.txt`'s text tab opened and active (the review diff is tab 0), its cursor at the start.
async fn open_text(app: &mut App, root: &Path) {
    app.test_open_file(root.join("mod.txt"));
    assert!(
        app.settle(|a| a.buffers.find_path(&root.join("mod.txt")).is_some())
            .await
    );
}

/// Types `n` new lines at the start of the file (its text tab active, cursor at the start).
async fn lines_above(app: &mut App, n: usize) {
    for i in 0..n {
        app.type_text(&format!("top {i}"));
        app.press(KeyCode::Enter, KeyModifiers::NONE);
    }
    assert!(app.settle(|a| a.diffs_settled()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_lines_typed_above_a_thread_move_it_and_its_block() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3("Why this"));
    let mut app = boot(&root, &server).await;
    assert_eq!(
        place(&app),
        Placement::Placed {
            side: Side::Right,
            lines: 2..3
        }
    );
    open_text(&mut app, &root).await;
    lines_above(&mut app, 2).await;
    // Unsaved: the file on disk is as it was.
    assert_eq!(
        fs::read_to_string(root.join("mod.txt")).unwrap(),
        "m1\nm2 changed\nm3\n"
    );
    assert_eq!(
        place(&app),
        Placement::Placed {
            side: Side::Right,
            lines: 4..5
        }
    );
    app.test_activate_tab(0);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(block_lines(&app), [(crate::review_blocks::Role::Thread, 4)]);
    // Removed again: back at its line.
    let id = app.buffers.find_path(&root.join("mod.txt")).unwrap();
    while app.buffers.get_mut(id).unwrap().undo().is_some() {}
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3\n");
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert!(
        app.settle(|a| place(a)
            == Placement::Placed {
                side: Side::Right,
                lines: 2..3
            })
        .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_an_edit_of_a_threads_line_keeps_it_there_and_a_commit_outdates_it() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3("Why this"));
    let mut app = boot(&root, &server).await;
    open_text(&mut app, &root).await;
    app.press(KeyCode::Down, KeyModifiers::NONE);
    app.press(KeyCode::Down, KeyModifiers::NONE);
    app.press(KeyCode::End, KeyModifiers::NONE);
    app.type_text(" edited");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    app.type_text("m4");
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 edited\nm4\n");
    assert!(app.settle(|a| a.diffs_settled()).await);
    // Edited locally: still on its line, as long.
    assert_eq!(
        place(&app),
        Placement::Placed {
            side: Side::Right,
            lines: 2..3
        }
    );
    // Committed, the merge request's code changed under it: outdated.
    app.test_execute(Command::Save);
    assert!(app.settle(|a| !a.buffers.iter().any(|(_, b)| b.is_dirty())).await);
    commit_all(&root, "edit");
    app.test_refresh_git();
    assert!(app.settle(|a| place(a) == Placement::Outdated).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_draft_follows_lines_typed_above_and_new_ones_count_in_the_head() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, |_, _| "[]".into());
    let mut app = boot(&root, &server).await;
    // A draft on lines 2-3 ("m2 changed", "m3").
    select(&mut app, true, 1, 2);
    app.test_execute(Command::ReviewComment);
    app.type_text("Check this");
    app.press_bound(Command::ReviewSaveComment);
    assert_eq!(app.test_composer_text(), None);
    open_text(&mut app, &root).await;
    lines_above(&mut app, 1).await;
    app.test_activate_tab(0);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let rows = app.review.draft_rows();
    assert_eq!((rows[0].draft.start, rows[0].draft.end, rows[0].held), (2, 3, None));
    assert_eq!(block_lines(&app), [(crate::review_blocks::Role::Draft, 3)]);
    // A new one on "m3" (line 4 shown) counts it as the head's line 3.
    select(&mut app, true, 3, 3);
    app.test_execute(Command::ReviewComment);
    app.type_text("And this");
    app.press_bound(Command::ReviewSaveComment);
    assert_eq!(app.test_composer_text(), None);
    let rows = app.review.draft_rows();
    let new = rows.iter().find(|r| r.draft.body == "And this").unwrap();
    assert_eq!((new.draft.start, new.draft.end, new.held), (3, 3, None));
    // The typed line isn't in the merge request.
    select(&mut app, true, 0, 0);
    app.test_execute(Command::ReviewComment);
    assert_eq!(app.test_composer_text(), None);
    assert!(message(&app).contains("changed locally"), "{}", message(&app));
}

/// The active review diff's floating Comment button, as last drawn.
fn comment_button(app: &App) -> Option<ratatui::layout::Rect> {
    app.hits
        .group(app.groups.active)
        .unwrap()
        .diff
        .as_ref()
        .unwrap()
        .comment
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_the_floating_comment_button_comments_on_the_selected_lines() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, |_, _| "[]".into());
    let mut app = boot(&root, &server).await;
    // No selection: no button.
    assert_eq!(comment_button(&app), None);
    // Lines 2-3 selected: the button on line 3's row, at the right end.
    select(&mut app, true, 1, 2);
    draw_wide(&mut app, 120);
    let b = comment_button(&app).expect("the button is drawn");
    let screen = lines(&mut app, 120, 24);
    let row = &screen[b.y as usize];
    assert!(row.contains("m3") && row.contains("+ Comment"), "{row}");
    // A click opens the box on those lines; while it is open the button is gone.
    app.mouse(MouseEventKind::Down(MouseButton::Left), b.x + 1, b.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), b.x + 1, b.y);
    assert_eq!(app.test_composer_text(), Some(String::new()));
    draw_wide(&mut app, 120);
    assert_eq!(comment_button(&app), None);
    app.type_text("From the button");
    app.press_bound(Command::ReviewSaveComment);
    let rows = app.review.draft_rows();
    let d = &rows.iter().find(|r| r.draft.body == "From the button").unwrap().draft;
    assert_eq!((d.start, d.end, d.right), (2, 3, true));
    // A selection ending at a line's start doesn't take that line.
    app.group_mut().active_diff_mut().unwrap().caret = Caret {
        right: true,
        anchor: (0, 0),
        head: (2, 0),
        col: None,
    };
    draw_wide(&mut app, 120);
    let b = comment_button(&app).expect("the button is drawn");
    let row = lines(&mut app, 120, 24)[b.y as usize].clone();
    assert!(row.contains("m2 changed"), "{row}");
}

/// The review diff's rows drawn 120 wide: each row's text and the colour of its gutter bar
/// (`None`: no bar).
fn marked_rows(app: &mut App) -> Vec<(String, Option<ratatui::style::Color>)> {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..24)
        .map(|y| {
            let row: String = (0..120).map(|x| buf[(x, y)].symbol().to_string()).collect();
            let bar = (0..120).find(|&x| buf[(x, y)].symbol() == "▎").map(|x| buf[(x, y)].fg);
            (row, bar)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_gutter_bars_mark_the_lines_of_threads_and_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3("Why this"));
    let mut app = boot(&root, &server).await;
    let thread = app.theme.get("review.lineMarker").fg;
    let draft = app.theme.get("review.draft.lineMarker").fg;
    // The bar of the diff row (right of the side panel) showing `text`.
    let bar = |rows: &[(String, Option<ratatui::style::Color>)], text: &str| {
        rows.iter()
            .find(|(r, _)| r.rsplit('│').next().is_some_and(|code| code.contains(text)))
            .unwrap_or_else(|| panic!("no row with {text}"))
            .1
    };
    // The thread is on line 3 alone.
    let rows = marked_rows(&mut app);
    assert_eq!(bar(&rows, "m3"), thread);
    assert_eq!(bar(&rows, "m1"), None);
    assert_eq!(bar(&rows, "m2 changed"), None);
    // A draft on lines 1-2: both marked in the draft's colour; the thread's line keeps its own.
    select(&mut app, true, 0, 1);
    app.test_execute(Command::ReviewComment);
    app.type_text("Both lines");
    app.press_bound(Command::ReviewSaveComment);
    assert!(app.review.draft_rows().iter().any(|r| r.draft.body == "Both lines"));
    let rows = marked_rows(&mut app);
    assert_eq!(bar(&rows, "m1"), draft);
    assert_eq!(bar(&rows, "m2 changed"), draft);
    assert_eq!(bar(&rows, "m3"), thread);
}
