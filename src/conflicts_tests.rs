//! conflicts-b: the editor draws merge conflicts — the blocks in VS Code's merge colours, the
//! marker labels, the action row after `<<<<<<<` and its clicks — and the accept commands apply
//! one `EditKind::Other` step. The "Done when" parts covered here: a file with a two-way and a
//! diff3 conflict shows both conflicts with their action rows; a click on Accept Incoming and
//! Accept Current / Accept All Incoming from the commands leave what `git checkout --theirs` /
//! `--ours` gives for those hunks, each an undo step; a file with no markers draws nothing.
//! Next / Previous, the status bar count and Compare are conflicts-c; Source Control conflicts-d.

use std::fs;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, MergeCommand};
use crate::conflict::Choice;
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
    // Compare is drawn but does nothing until conflicts-c.
    let _term = draw(&mut app, 160);
    let compare = app.hits.groups[0]
        .merge_buttons
        .iter()
        .find(|(_, button, _)| *button == MergeButton::Compare)
        .map(|(rect, _, _)| *rect)
        .expect("the Compare button");
    app.mouse(MouseEventKind::Down(MouseButton::Left), compare.x + 1, compare.y);
    assert_eq!(text(&app), CONFLICTS, "Compare is a no-op for now");
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
