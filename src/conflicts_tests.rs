//! conflicts-b: the editor draws merge conflicts — the blocks in VS Code's merge colours, the
//! marker labels, the action row after `<<<<<<<` and its clicks — and the accept commands apply
//! one `EditKind::Other` step. conflicts-c: Next / Previous Conflict (Alt+F8 /
//! Shift+Alt+F8, wrapping), the status bar's "N conflicts" and Compare Current Conflict (a
//! read-only diff tab of the two blocks).
//!
//! The "Done when" parts covered here: a file with a two-way and a diff3 conflict shows both
//! conflicts with their action rows; a click on Accept Incoming and Accept Current / Accept All
//! Incoming from the commands leave what `git checkout --theirs` / `--ours` gives for those
//! hunks, each an undo step; Next / Previous move and wrap; the status bar counts them; Compare
//! opens Current ↔ Incoming; a file with no markers draws nothing and has nowhere to go. Source
//! Control is conflicts-d.

use std::fs;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, MergeCommand};
use crate::conflict::Choice;
use crate::diff_view::Pair;
use crate::layout::MergeButton;
use crate::milestone10_tests::dirs;
use crate::selection::{Selection, Selections};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A two-way conflict, then a diff3 one: git's markers as `git merge-file` writes them.
const CONFLICTS: &str = "a\n\
<<<<<<< HEAD\n\
ours1\n\
=======\n\
theirs1\n\
>>>>>>> main\n\
mid\n\
<<<<<<< HEAD\n\
ours2\n\
||||||| base\n\
base2\n\
=======\n\
theirs2\n\
>>>>>>> main\n\
end\n";

fn open(text: &str) -> (tempfile::TempDir, App) {
    let (dir, root, state) = dirs();
    fs::write(root.join("a.txt"), text).unwrap();
    let buffer = Buffer::open(&root.join("a.txt")).unwrap();
    let ws = Workspace::load(root);
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.test_start();
    app.test_plain_whitespace();
    (dir, app)
}

fn draw(app: &mut App, width: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn bg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Option<Color> {
    term.backend().buffer()[(x, y)].bg.into()
}

/// The row the buffer's line `line` is drawn on, and the drawn text area.
fn row_of(app: &App, line: usize) -> (Rect, u16) {
    let g = &app.hits.groups[0];
    let top = app.group().active_tab().unwrap().view.top_line();
    (g.text, g.text.y + (line - top) as u16)
}

/// The drawn text of a screen row (the text area only).
fn row_text(term: &Terminal<TestBackend>, text: Rect, y: u16) -> String {
    let out = term.backend().buffer();
    (text.x..text.x + text.width).map(|x| out[(x, y)].symbol()).collect()
}

fn at(app: &App, needle: &str) -> usize {
    text(app).find(needle).unwrap()
}

fn set_cursor(app: &mut App, byte: usize) {
    let (_, view) = app.active_mut().unwrap();
    view.selections = Selections::single(Selection::cursor(ByteIdx(byte)));
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicts_draw_their_blocks_labels_and_action_row() {
    let (_dir, mut app) = open(CONFLICTS);
    let key_bg = |key: &str| app.theme.get(key).bg;
    let current_header = key_bg("merge.currentHeaderBackground");
    let current_content = key_bg("merge.currentContentBackground");
    let incoming_header = key_bg("merge.incomingHeaderBackground");
    let incoming_content = key_bg("merge.incomingContentBackground");
    let common_header = key_bg("merge.commonHeaderBackground");
    let common_content = key_bg("merge.commonContentBackground");
    let editor_bg = key_bg("editor");
    let label_fg = app.theme.get("git.blame.editorDecorationForeground").fg.unwrap();
    let action_bg = app.theme.get("quickDiff.action").bg;
    let term = draw(&mut app, 160);

    // The first conflict: `<<<<<<<` header, current content, `=======` incoming header,
    // incoming content, `>>>>>>>` incoming header.
    for (line, expected) in [
        (1, current_header),
        (2, current_content),
        (3, incoming_header),
        (4, incoming_content),
        (5, incoming_header),
        // The diff3 one: `|||||||` common header and the base block's content.
        (9, common_header),
        (10, common_content),
        (11, incoming_header),
        (12, incoming_content),
    ] {
        let (text, y) = row_of(&app, line);
        assert_eq!(bg(&term, (text.x, y)), expected, "line {line}'s background");
    }
    // Line `mid` is between the conflicts: the editor's own background.
    let (text, y) = row_of(&app, 6);
    assert_eq!(bg(&term, (text.x, y)), editor_bg, "between the conflicts");

    // The labels after each marker line's text (dim, as blame is).
    for (line, label) in [
        (1, "(Current Change)"),
        (5, "(Incoming Change)"),
        (9, "(Common Ancestor)"),
        (13, "(Incoming Change)"),
    ] {
        let (text, y) = row_of(&app, line);
        let row = row_text(&term, text, y);
        let x = row
            .find(label)
            .unwrap_or_else(|| panic!("{label:?} not drawn: {row:?}"));
        assert_eq!(
            term.backend().buffer()[(text.x + x as u16, y)].fg,
            label_fg,
            "{label}'s colour"
        );
    }
    // The `=======` line has no label.
    let (text, y) = row_of(&app, 3);
    assert!(!row_text(&term, text, y).contains("(Incoming Change)"));

    // The action row after each `<<<<<<<` line: all four buttons at this width, in order, in
    // the quick diff action colour (two conflicts: two rows).
    let g = app.hits.groups[0].clone();
    let first = ByteIdx(at(&app, "<<<<<<< HEAD"));
    let starts: std::collections::BTreeSet<_> = g.merge_buttons.iter().map(|(_, _, start)| *start).collect();
    assert_eq!(starts.len(), 2, "one row per conflict");
    let buttons: Vec<_> = g
        .merge_buttons
        .iter()
        .filter(|(_, _, start)| *start == first)
        .map(|(rect, button, _)| (*rect, *button))
        .collect();
    assert_eq!(
        buttons.iter().map(|(_, b)| *b).collect::<Vec<_>>(),
        vec![
            MergeButton::Accept(Choice::Current),
            MergeButton::Accept(Choice::Incoming),
            MergeButton::Accept(Choice::Both),
            MergeButton::Compare,
        ],
        "the first conflict's action row"
    );
    assert_eq!(bg(&term, (buttons[0].0.x + 1, buttons[0].0.y)), action_bg);
    // Narrower: buttons are dropped from the right (Compare first); Accept Current still fits.
    let term = draw(&mut app, 90);
    let g = app.hits.groups[0].clone();
    assert!(
        !g.merge_buttons.is_empty() && g.merge_buttons.len() < 8,
        "some but not every button fits 90 columns: {:?}",
        g.merge_buttons
    );
    assert!(
        g.merge_buttons.iter().all(|(_, b, _)| *b != MergeButton::Compare),
        "Compare is dropped first"
    );
    let (text, y) = row_of(&app, 1);
    assert!(row_text(&term, text, y).contains("Accept Current"));
}

#[tokio::test(flavor = "multi_thread")]
async fn clicking_accept_incoming_leaves_theirs_in_one_undo_step() {
    let (_dir, mut app) = open(CONFLICTS);
    let _term = draw(&mut app, 160);
    let start = ByteIdx(at(&app, "<<<<<<< HEAD"));
    let rect = app.hits.groups[0]
        .merge_buttons
        .iter()
        .find(|(_, button, conflict)| *button == MergeButton::Accept(Choice::Incoming) && *conflict == start)
        .map(|(rect, _, _)| *rect)
        .expect("the first conflict's Accept Incoming button");
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
    // `git checkout --theirs` for the first hunk: the incoming block, markers gone.
    assert_eq!(
        text(&app),
        "a\ntheirs1\nmid\n<<<<<<< HEAD\nours2\n||||||| base\nbase2\n=======\ntheirs2\n>>>>>>> main\nend\n"
    );
    assert!(app.active_buffer().unwrap().has_edits());
    // One `EditKind::Other` step: one undo brings the markers back.
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), CONFLICTS);
    // The action row's Compare opens the conflict's blocks in a diff tab (conflicts-c).
    let _term = draw(&mut app, 160);
    let compare = app.hits.groups[0]
        .merge_buttons
        .iter()
        .find(|(_, button, _)| *button == MergeButton::Compare)
        .map(|(rect, _, _)| *rect)
        .expect("the Compare button");
    app.mouse(MouseEventKind::Down(MouseButton::Left), compare.x + 1, compare.y);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().active_diff().expect("a diff tab");
    assert_eq!(d.pair, Pair::conflict());
    assert_eq!(text_of(&d.doc.as_ref().unwrap().left), "ours1");
    assert_eq!(text_of(&d.doc.as_ref().unwrap().right), "theirs1");
}

/// A side's lines as text.
fn text_of(lines: &crate::diff_view::Lines) -> String {
    (0..lines.len()).map(|i| lines.line(i)).collect::<Vec<_>>().join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn next_and_previous_conflict_move_and_wrap() {
    let (_dir, mut app) = open(CONFLICTS);
    let starts: Vec<ByteIdx> = {
        let buf = app.active_buffer().unwrap();
        let revision = buf.revision();
        let mut scan = crate::conflict::Conflicts::default();
        scan.scan(&buf.rope, revision).iter().map(|c| c.start).collect()
    };
    assert_eq!(starts.len(), 2);
    set_cursor(&mut app, 0);

    // Alt+F8: the first, then the second, then wraps to the first.
    app.press(KeyCode::F(8), KeyModifiers::ALT);
    assert_eq!(head(&app), starts[0]);
    // The target is revealed (the cursor is brought on screen next draw) and undo sealed.
    assert_eq!(
        app.group().active_tab().unwrap().view.reveal_range,
        Some((starts[0], starts[0]))
    );
    app.press(KeyCode::F(8), KeyModifiers::ALT);
    assert_eq!(head(&app), starts[1]);
    app.press(KeyCode::F(8), KeyModifiers::ALT);
    assert_eq!(head(&app), starts[0], "wraps");
    assert_eq!(app.group().active_tab().unwrap().view.selections.len(), 1);
    // Shift+Alt+F8: back to the second at once (wrapping), then the first.
    app.press(KeyCode::F(8), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(head(&app), starts[1], "wraps back");
    app.press(KeyCode::F(8), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(head(&app), starts[0]);

    // A file without markers: nothing moves.
    let (_dir, mut app) = open("plain\ntext\n");
    set_cursor(&mut app, 3);
    app.press(KeyCode::F(8), KeyModifiers::ALT);
    app.press(KeyCode::F(8), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(head(&app), ByteIdx(3));
    // The command, too (the palette's entry).
    app.test_execute(Command::Merge(MergeCommand::Next));
    assert_eq!(head(&app), ByteIdx(3));
}

/// The active view's primary cursor.
fn head(app: &App) -> ByteIdx {
    app.group().active_tab().unwrap().view.selections.primary().head
}

/// The status bar's last row as drawn.
fn status_text(term: &Terminal<TestBackend>) -> String {
    let out = term.backend().buffer();
    let area = out.area;
    (area.x..area.x + area.width)
        .map(|x| out[(x, area.y + area.height - 1)].symbol())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_bar_counts_the_conflicts() {
    let (_dir, mut app) = open(CONFLICTS);
    let term = draw(&mut app, 160);
    let status = status_text(&term);
    assert!(status.contains("2 conflicts"), "{status:?}");
    assert!(status.contains("a.txt"), "beside the file: {status:?}");

    // A file with one conflict.
    let (_dir, mut app) = open("<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> main\n");
    let term = draw(&mut app, 160);
    assert!(status_text(&term).contains("1 conflict "), "{:?}", status_text(&term));

    // None without markers.
    let (_dir, mut app) = open("plain\ntext\n");
    let term = draw(&mut app, 160);
    assert!(!status_text(&term).contains("conflict"), "{:?}", status_text(&term));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_shows_the_current_and_incoming_blocks() {
    let (_dir, mut app) = open(CONFLICTS);
    let _term = draw(&mut app, 160);
    let ours2 = at(&app, "ours2");
    set_cursor(&mut app, ours2);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.settle(|a| a.diffs_settled()).await);

    let d = app.group().active_diff().expect("a diff tab");
    assert_eq!(d.pair, Pair::conflict());
    assert_eq!(d.pair.staging(), None);
    assert!(!d.pair.needs_repo());
    assert_eq!(d.title(), "a.txt (Current ↔ Incoming)");
    assert_eq!(d.pair.titles(), ("Current".into(), "Incoming".into()));
    let doc = d.doc.as_ref().expect("loaded");
    assert_eq!(text_of(&doc.left), "ours2");
    assert_eq!(text_of(&doc.right), "theirs2");

    // Drawn: the header over each side, no Stage / Unstage or Revert buttons.
    let term = draw(&mut app, 160);
    let hit = app.hits.groups[0].clone();
    let header = row_text(&term, hit.editor, hit.editor.y);
    assert!(header.contains("Current"), "{header:?}");
    assert!(header.contains("Incoming"), "{header:?}");
    let diff = hit.diff.expect("the diff view's hit");
    assert!(diff.buttons.is_empty(), "nothing to stage: {:?}", diff.buttons);
    assert!(diff.reverts.is_empty(), "nothing to revert");

    // A cursor outside every conflict: nothing opens (the text tab stays).
    let (_dir, mut app) = open(CONFLICTS);
    set_cursor(&mut app, 0);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.group().active_diff().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_opens_a_new_tab_per_conflict() {
    let (_dir, mut app) = open(CONFLICTS);
    let _term = draw(&mut app, 160);
    let ours1 = at(&app, "ours1");
    set_cursor(&mut app, ours1);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.settle(|a| a.diffs_settled()).await);

    // Back to the text tab, compare the second conflict: a second diff tab, not the first reused.
    app.group_mut().active = 0;
    let ours2 = at(&app, "ours2");
    set_cursor(&mut app, ours2);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.settle(|a| a.diffs_settled()).await);

    let mut blocks = app
        .group()
        .tabs
        .iter()
        .filter_map(|t| {
            let d = t.diff()?;
            let doc = d.doc.as_ref().expect("loaded");
            Some((text_of(&doc.left), text_of(&doc.right)))
        })
        .collect::<Vec<_>>();
    blocks.sort();
    assert_eq!(
        blocks,
        vec![
            ("ours1".to_string(), "theirs1".to_string()),
            ("ours2".to_string(), "theirs2".to_string())
        ],
        "one diff tab per conflict, each with its own blocks"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_tab_shows_no_conflict_count() {
    // A compare opens a diff tab over a conflicted buffer: the count is the active text
    // buffer's, so a diff tab shows none (the file name and "N conflicts" both go).
    let (_dir, mut app) = open(CONFLICTS);
    let _term = draw(&mut app, 160);
    assert!(status_text(&_term).contains("2 conflicts"));
    let ours1 = at(&app, "ours1");
    set_cursor(&mut app, ours1);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.settle(|a| a.diffs_settled()).await);
    let term = draw(&mut app, 160);
    assert!(!status_text(&term).contains("conflict"), "{:?}", status_text(&term));
}

#[tokio::test(flavor = "multi_thread")]
async fn accept_commands_work_at_the_cursor_and_for_every_conflict() {
    let (_dir, mut app) = open(CONFLICTS);

    // A cursor in the second (diff3) conflict: Accept Current keeps ours2.
    let ours2 = at(&app, "ours2");
    set_cursor(&mut app, ours2);
    app.test_execute(Command::Merge(MergeCommand::AcceptCurrent));
    assert_eq!(
        text(&app),
        "a\n<<<<<<< HEAD\nours1\n=======\ntheirs1\n>>>>>>> main\nmid\nours2\nend\n"
    );
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), CONFLICTS, "one undo step");

    // Accept None keeps the base block of a diff3 conflict.
    set_cursor(&mut app, ours2);
    app.test_execute(Command::Merge(MergeCommand::AcceptNone));
    assert_eq!(
        text(&app),
        "a\n<<<<<<< HEAD\nours1\n=======\ntheirs1\n>>>>>>> main\nmid\nbase2\nend\n"
    );
    app.test_execute(Command::Undo);

    // Accept All Incoming clears both conflicts at once, as one step.
    set_cursor(&mut app, 0);
    app.test_execute(Command::Merge(MergeCommand::AcceptAllIncoming));
    assert_eq!(text(&app), "a\ntheirs1\nmid\ntheirs2\nend\n");
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), CONFLICTS);

    // A cursor outside every conflict: nothing happens.
    set_cursor(&mut app, 0);
    app.test_execute(Command::Merge(MergeCommand::AcceptCurrent));
    assert_eq!(text(&app), CONFLICTS, "no conflict under the cursor");

    // A selection touching both conflicts accepts just those two (both sides).
    let (first, last) = (at(&app, "ours1"), at(&app, "theirs2"));
    let (_, view) = app.active_mut().unwrap();
    view.selections = Selections::single(Selection::new(ByteIdx(first), ByteIdx(last)));
    app.test_execute(Command::Merge(MergeCommand::AcceptBoth));
    assert_eq!(text(&app), "a\nours1\ntheirs1\nmid\nours2\ntheirs2\nend\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_without_markers_draws_no_action_row() {
    let (_dir, mut app) = open("plain\ntext\n");
    let _term = draw(&mut app, 160);
    assert!(app.hits.groups[0].merge_buttons.is_empty());
    set_cursor(&mut app, 0);
    app.test_execute(Command::Merge(MergeCommand::AcceptAllIncoming));
    assert_eq!(text(&app), "plain\ntext\n");
}
