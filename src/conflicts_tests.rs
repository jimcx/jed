//! conflicts-b: the editor draws merge conflicts — the blocks in VS Code's merge colours, the
//! marker labels, the action row after `<<<<<<<` and its clicks — and the accept commands apply
//! one `EditKind::Other` step. conflicts-c: Next / Previous Conflict (Alt+F8 /
//! Shift+Alt+F8, wrapping), the status bar's "N conflicts" and Compare Current Conflict (a
//! read-only diff tab of the two blocks). conflicts-d: Source Control's "Merge Changes" section
//! (opening a conflicted file opens it, not a diff), Stage File as resolve with the "still has
//! markers" question, and the Done-when walks against a real `git merge`.
//!
//! The "Done when" parts covered here: a file with a two-way and a diff3 conflict shows both
//! conflicts with their action rows; a click on Accept Incoming and Accept Current / Accept All
//! Incoming from the commands leave what `git checkout --theirs` / `--ours` gives for those
//! hunks, each an undo step; Next / Previous move and wrap; the status bar counts them; Compare
//! opens Current ↔ Incoming; a real merge's file sits under "Merge Changes" until staged, which
//! asks while markers remain; a file with no markers draws nothing, and a 1 MB one stays within
//! the frame budget.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::app::{App, InitialFile, Prompt};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, MergeCommand};
use crate::conflict::Choice;
use crate::diff_view::Pair;
use crate::git::Change;
use crate::git::test_repo::{commit_all, git, init};
use crate::layout::MergeButton;
use crate::milestone10_tests::{
    ctrl_shift_g, dirs, file, open_file_app, scm_button, scm_rows, selected_key, settle_git, settle_stage,
};
use crate::scm::Section;
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

#[tokio::test(flavor = "multi_thread")]
async fn inline_blame_does_not_overwrite_the_conflict_row() {
    let (_d, root, state) = dirs();
    merge_repo(&root, false);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(
        settle_git(&mut app, |s| {
            matches!(s.entry(Path::new("a.txt")), Some(e) if e.unstaged == Some(Change::Conflicted))
        })
        .await
    );
    // Inline blame on: the marker line is uncommitted, so its label would be "Uncommitted
    // changes" — which must not overwrite the conflict row.
    app.test_execute(Command::ToggleBlame);
    assert!(
        app.settle(|a| {
            let b = a.active_buffer().unwrap();
            !b.blame.running() && !b.blame.hunks().is_empty()
        })
        .await
    );

    // The cursor on the `<<<<<<<` line (Next Conflict's target): the conflict label and action
    // row are drawn, not the blame.
    let start = at(&app, "<<<<<<<");
    set_cursor(&mut app, start);
    let term = draw(&mut app, 160);
    let (text, y) = row_of(&app, 1);
    let row = row_text(&term, text, y);
    assert!(row.contains("(Current Change)"), "the conflict label: {row:?}");
    assert!(row.contains("Accept Current"), "the action row: {row:?}");
    assert!(!row.contains("Uncommitted changes"), "no blame over the row: {row:?}");
}

// conflicts-d: a real `git merge`, Source Control's "Merge Changes", Stage File as resolve.

/// The base and the two branches' versions of a.txt: two non-overlapping regions changed on
/// both sides, so `git merge other` leaves two conflicts.
const BASE_FILE: &str = "alpha\none\nbravo\ncharlie\ntwo\ndelta\necho\nthree\nfoxtrot\n";
const OURS_FILE: &str = "alpha\nOURS ONE\nbravo\ncharlie\ntwo\ndelta\necho\nOURS THREE\nfoxtrot\n";
const THEIRS_FILE: &str = "alpha\nTHEIRS ONE\nbravo\ncharlie\ntwo\ndelta\necho\nTHEIRS THREE\nfoxtrot\n";

/// A repository left in the middle of `git merge other` with two conflicts in a.txt (diff3
/// markers with `diff3`). `b.txt` is unchanged, for opening the app on a file that isn't the
/// conflicted one.
pub(crate) fn merge_repo(root: &Path, diff3: bool) {
    init(root);
    fs::write(root.join("a.txt"), BASE_FILE).unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    commit_all(root, "base");
    git(root, &["checkout", "-q", "-b", "other"]);
    fs::write(root.join("a.txt"), THEIRS_FILE).unwrap();
    commit_all(root, "theirs");
    git(root, &["checkout", "-q", "main"]);
    fs::write(root.join("a.txt"), OURS_FILE).unwrap();
    commit_all(root, "ours");
    if diff3 {
        git(root, &["config", "merge.conflictStyle", "diff3"]);
    }
    // `git merge` exits 1 on a conflict, so `test_repo::git` (which asserts success) can't run it:
    // same environment, the conflict's exit code allowed.
    let out = std::process::Command::new("git")
        .args(["merge", "other"])
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git runs");
    assert_eq!(
        out.status.code(),
        Some(1),
        "two conflicts: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn click(app: &mut App, (x, y): (u16, u16)) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The line (0-based) of the `n`th line starting with `marker`.
fn marker_line(text: &str, marker: &str, n: usize) -> usize {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with(marker))
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("no {marker:?} line {n} in {text:?}"))
}

/// `text` with conflict `k` (`<<<<<<<` … `>>>>>>>`) replaced by one of its sides' content (the
/// line after `side`'s marker): what `git checkout --ours` / `--theirs` leaves for that hunk.
fn accepted_side(text: &str, k: usize, side: &str) -> String {
    let content = text
        .lines()
        .nth(marker_line(text, side, k) + 1)
        .expect("the side's text");
    let mut lines: Vec<&str> = text.lines().collect();
    lines.splice(
        marker_line(text, "<<<<<<<", k)..=marker_line(text, ">>>>>>>", k),
        [content],
    );
    lines.join("\n") + "\n"
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicts_done_when_a_real_merge_source_control_accept_and_stage_resolve() {
    let (_d, root, state) = dirs();
    merge_repo(&root, false);
    let merged = fs::read_to_string(root.join("a.txt")).unwrap();
    assert_eq!(merged.matches("<<<<<<<").count(), 2);
    assert_eq!(merged.matches(">>>>>>> other").count(), 2);
    // git's merge stages keep the branches' whole files: the accepted hunks are exactly theirs'
    // first and ours' second region.
    assert_eq!(git(&root, &["show", ":2:a.txt"]), OURS_FILE);
    assert_eq!(git(&root, &["show", ":3:a.txt"]), THEIRS_FILE);

    // The app opens on b.txt; the conflict is reached through Source Control.
    let mut app = open_file_app(&root, &state, "b.txt");
    assert!(
        settle_git(&mut app, |s| {
            matches!(s.entry(Path::new("a.txt")), Some(e) if e.unstaged == Some(Change::Conflicted))
        })
        .await
    );
    ctrl_shift_g(&mut app);
    assert_eq!(
        scm_rows(&mut app),
        ["▾ Merge Changes 1", "a.txt !", "▾ Changes 0"],
        "the conflicted file is under Merge Changes, not Changes"
    );
    // The `!` in `gitDecoration.conflictingResourceForeground`.
    let term = draw(&mut app, 160);
    let hit = app.hits.scm.clone().unwrap();
    let letter = term.backend().buffer()[(hit.list.x + hit.list.width - 2, hit.list.y + 1)].clone();
    assert_eq!(letter.symbol(), "!");
    assert_eq!(
        Option::<Color>::from(letter.fg),
        app.theme.get("gitDecoration.conflictingResourceForeground").fg
    );
    // Enter opens the file itself, not a diff.
    assert_eq!(selected_key(&app), Some(file(Section::MergeChanges, "a.txt")));
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path == root.join("a.txt")))
            .await
    );
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    assert!(app.group().active_diff().is_none(), "not a diff tab");

    // Both conflicts are coloured, each with its action row.
    let term = draw(&mut app, 160);
    let current_header = app.theme.get("merge.currentHeaderBackground").bg;
    let incoming_header = app.theme.get("merge.incomingHeaderBackground").bg;
    for k in 0..2 {
        let line = marker_line(&merged, "<<<<<<<", k);
        let (text, y) = row_of(&app, line);
        assert_eq!(bg(&term, (text.x, y)), current_header, "conflict {k}'s header");
        let row = row_text(&term, text, y);
        assert!(
            row.contains("Accept Current") && row.contains("Accept Incoming"),
            "{row:?}"
        );
        let (text, y) = row_of(&app, marker_line(&merged, "=======", k));
        assert_eq!(
            bg(&term, (text.x, y)),
            incoming_header,
            "conflict {k}'s incoming header"
        );
    }
    let starts: Vec<ByteIdx> = {
        let mut seen = Vec::new();
        for (_, _, start) in &app.hits.groups[0].merge_buttons {
            if !seen.contains(start) {
                seen.push(*start);
            }
        }
        seen
    };
    assert_eq!(starts.len(), 2, "both action rows drawn");
    for start in &starts {
        for button in [
            MergeButton::Accept(Choice::Current),
            MergeButton::Accept(Choice::Incoming),
            MergeButton::Accept(Choice::Both),
            MergeButton::Compare,
        ] {
            assert!(
                app.hits.groups[0]
                    .merge_buttons
                    .iter()
                    .any(|(_, b, s)| b == &button && s == start),
                "{button:?} on the conflict at {start:?}"
            );
        }
    }

    // Click Accept Incoming on the first conflict: theirs' first hunk, one undo step.
    let rect = app.hits.groups[0]
        .merge_buttons
        .iter()
        .find(|(_, b, s)| *b == MergeButton::Accept(Choice::Incoming) && *s == starts[0])
        .map(|(rect, ..)| *rect)
        .unwrap();
    click(&mut app, (rect.x + 1, rect.y));
    let after_incoming = accepted_side(&merged, 0, "=======");
    assert_eq!(text(&app), after_incoming);
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), merged, "one undo step");

    // Accept it again, then Accept Current on the second (at its shifted start): the first hunk
    // stays theirs', the second becomes ours', as `git checkout --theirs` / `--ours` would leave
    // them.
    click(&mut app, (rect.x + 1, rect.y));
    assert_eq!(text(&app), after_incoming);
    let second_start = at(&app, "<<<<<<<");
    set_cursor(&mut app, second_start);
    app.test_execute(Command::Merge(MergeCommand::AcceptCurrent));
    let after_both = accepted_side(&after_incoming, 0, "<<<<<<<");
    assert_eq!(text(&app), after_both);
    assert_eq!(text(&app), OURS_FILE.replacen("OURS ONE", "THEIRS ONE", 1));
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), after_incoming, "the second accept is one undo step");
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), merged, "and the first one too");

    // Next Conflict wraps past the last.
    set_cursor(&mut app, 0);
    for expected in [starts[0], starts[1], starts[0]] {
        app.press(KeyCode::F(8), KeyModifiers::ALT);
        assert_eq!(head(&app), expected);
    }

    // Compare opens current ↔ incoming of the second conflict.
    set_cursor(&mut app, starts[1].0);
    app.test_execute(Command::Merge(MergeCommand::Compare));
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().active_diff().expect("a diff tab");
    assert_eq!(d.pair, Pair::conflict());
    assert_eq!(text_of(&d.doc.as_ref().unwrap().left), "OURS THREE");
    assert_eq!(text_of(&d.doc.as_ref().unwrap().right), "THEIRS THREE");

    // Stage File on the file still holding markers asks first: Cancel leaves it conflicted.
    ctrl_shift_g(&mut app);
    assert_eq!(
        scm_rows(&mut app),
        ["▾ Merge Changes 1", "a.txt !", "▾ Changes 0"],
        "still under Merge Changes"
    );
    let button = scm_button(&mut app, &file(Section::MergeChanges, "a.txt"));
    click(&mut app, button);
    assert!(matches!(app.prompt, Some(Prompt::ConfirmStageConflict { .. })));
    let term = draw(&mut app, 160);
    let screen: Vec<String> = {
        let out = term.backend().buffer();
        (0..out.area.height)
            .map(|y| (0..out.area.width).map(|x| out[(x, y)].symbol()).collect())
            .collect()
    };
    assert!(
        screen.iter().any(|r| r.contains("still has conflict markers")),
        "{screen:#?}"
    );
    assert!(screen.iter().any(|r| r.contains("tage anyway")), "{screen:#?}");
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.prompt.is_none());
    assert!(
        settle_git(&mut app, |s| {
            matches!(s.entry(Path::new("a.txt")), Some(e) if e.unstaged == Some(Change::Conflicted))
        })
        .await
    );

    // Stage anyway (`git add`) resolves it, and it moves to Staged Changes.
    click(&mut app, button);
    assert!(matches!(app.prompt, Some(Prompt::ConfirmStageConflict { .. })));
    app.press(KeyCode::Char('s'), KeyModifiers::NONE);
    assert!(settle_stage(&mut app).await);
    assert_eq!(
        scm_rows(&mut app),
        ["▾ Staged Changes 1", "a.txt M", "▾ Changes 0"],
        "staged: Merge Changes is gone"
    );
    assert_eq!(git(&root, &["show", ":a.txt"]), merged, "the disk's text was added");
    assert_eq!(git(&root, &["diff", "--name-only"]), "", "nothing unstaged");
    assert_eq!(git(&root, &["diff", "--cached", "--name-only"]), "a.txt\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicts_done_when_a_diff3_merge_shows_the_base_block() {
    let (_d, root, state) = dirs();
    merge_repo(&root, true);
    let merged = fs::read_to_string(root.join("a.txt")).unwrap();
    assert_eq!(merged.matches("|||||||").count(), 2, "{merged:?}");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(
        settle_git(&mut app, |s| {
            matches!(s.entry(Path::new("a.txt")), Some(e) if e.unstaged == Some(Change::Conflicted))
        })
        .await
    );
    // The scan finds the base block of both conflicts.
    {
        let (buf, _) = app.active_mut().unwrap();
        let revision = buf.revision();
        let conflicts = buf.conflicts.scan(&buf.rope, revision);
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.iter().all(|c| c.base.is_some()), "both have a base block");
        assert!(conflicts.iter().all(|c| {
            c.base
                .as_ref()
                .is_some_and(|b| !buf.rope.slice(b.start.0..b.end.0).to_string().is_empty())
        }));
    }
    // Drawn: the `|||||||` line's label and the base block's colour, on both conflicts.
    let term = draw(&mut app, 160);
    let common_header = app.theme.get("merge.commonHeaderBackground").bg;
    let common_content = app.theme.get("merge.commonContentBackground").bg;
    for k in 0..2 {
        let line = marker_line(&merged, "|||||||", k);
        let (text, y) = row_of(&app, line);
        assert_eq!(bg(&term, (text.x, y)), common_header, "conflict {k}'s base header");
        let row = row_text(&term, text, y);
        assert!(row.contains("(Common Ancestor)"), "{row:?}");
        let (text, y) = row_of(&app, line + 1);
        assert_eq!(bg(&term, (text.x, y)), common_content, "conflict {k}'s base content");
    }
}

/// The Done-when's frame budget for the per-frame conflict scan: a 1 MB file with no markers is
/// scanned once (cached by revision), so drawing it and a loop turn stay within the frame.
/// Debug builds only report the timings (they are far slower).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_conflicts_the_scan_does_not_cost_per_frame() {
    let mut text = String::with_capacity(1_100_000);
    let mut i = 0;
    while text.len() < 1_000_000 {
        text.push_str(&format!("line {i} of a big file with nothing to merge here\n"));
        i += 1;
    }
    let (_dir, mut app) = open(&text);
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
    // The first frame scans the file (and caches it by revision).
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    {
        let (buf, _) = app.active_mut().unwrap();
        let revision = buf.revision();
        assert!(buf.conflicts.scan(&buf.rope, revision).is_empty());
    }
    let frames = 20;
    let start = Instant::now();
    for _ in 0..frames {
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let frame = start.elapsed() / frames;
    let start = Instant::now();
    for _ in 0..frames {
        app.pump();
    }
    let turn = start.elapsed() / frames;
    println!("conflicts scan: frame {frame:?}, turn {turn:?} (1 MB, no markers)");
    if !cfg!(debug_assertions) {
        let budget = Duration::from_millis(16);
        assert!(frame < budget, "a frame took {frame:?}, over {budget:?}");
        assert!(turn < budget, "a turn took {turn:?}, over {budget:?}");
    }
}

/// The Done-when's per-keystroke budget: typing into a 1 MB file with no markers. Each edit
/// drops the cached scan (`Conflicts::map`), so the following frame rescans the whole rope; that
/// frame must stay within the typing budget (release; debug builds only report).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_conflicts_typing_does_not_rescan_at_a_cost_per_frame() {
    let mut text = String::with_capacity(1_100_000);
    let mut i = 0;
    while text.len() < 1_000_000 {
        text.push_str(&format!("line {i} of a big file with nothing to merge here\n"));
        i += 1;
    }
    let (_dir, mut app) = open(&text);
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
    // The first frame scans the file (and caches it by revision).
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    {
        let (buf, _) = app.active_mut().unwrap();
        let revision = buf.revision();
        assert!(buf.conflicts.scan(&buf.rope, revision).is_empty());
    }
    // Type several keys: each edit drops the cached scan, and the following frame rescans the
    // whole rope (the cost this measures).
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let keys = "type some words here";
    for c in keys.chars() {
        let t = Instant::now();
        app.press(KeyCode::Char(c), KeyModifiers::NONE);
        app.pump();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let e = t.elapsed();
        worst = worst.max(e);
        total += e;
    }
    let n = keys.chars().count() as u32;
    let avg = total / n;
    println!("conflicts typing: {n} keys, avg {avg:?}, worst {worst:?} (1 MB, no markers)");
    if !cfg!(debug_assertions) {
        let budget = Duration::from_millis(16);
        assert!(avg < budget, "typing took {avg:?} a key");
        assert!(worst < budget, "a keystroke took {worst:?}");
    }
}
