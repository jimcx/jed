//! The stage_selected_lines task, subtask b (a text tab), end to end in a git repository: "Git:
//! Stage Selected Ranges" stages exactly the changed lines the selections touch (a mixed hunk's
//! picked lines and another hunk, several selections), saving a dirty buffer first (asking; the
//! prompt says "the selected lines"); "Git: Unstage Selected Ranges" takes a subset of the staged
//! lines back (through the buffer's text, no save); "Git: Revert Selected Ranges" puts the
//! index's lines back in the buffer as one undo step, left unsaved, refused when read-only, when
//! the buffer changed while the diff was worked out, and for an untracked file; lines nothing
//! changed in are "No changes in the selection" and write nothing. Subtask c is the same for a
//! diff tab's row selection (`diff_tab_*`).
//!
//! Subtask d is the task's "Done when", as `stage_selected_lines_done_when_*` scenarios: a text
//! tab with a dirty buffer, part of a mixed hunk plus another hunk staged (index equal to
//! `git apply --cached` of the hand-edited patch), a subset unstaged (`apply --cached -R`) and
//! reverted in one undo step (`..._text_tab_stage_unstage_revert`); the diff tab's three, split
//! with a left-side selection and inline with real keys, and a revert of a file not open equal
//! to `git apply -R` (`..._diff_tab_both_layouts_and_a_file_not_open`); a conflicted file
//! refused with nothing changed (`..._a_conflicted_file_is_refused`); the gutter, Source Control
//! and an open diff tab refreshing (`..._gutter_status_and_open_diff_refresh`); 10,000 cursors
//! staging 20,000 lines in debug mode (`..._ten_thousand_cursors`).

use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers as M};

use crate::app::{App, Prompt, What};
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::git::test_repo::git;
use crate::milestone10_tests::{dirs, open_file_app, repo_with, run_command, settle_markers};
use crate::selection::{Selection, Selections};

fn numbered(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

/// `numbered(20)` with lines 2 and 3 turned into `TWO` / `THREE` (one modified hunk), line 10
/// deleted and `added` after line 15.
fn changed() -> String {
    numbered(20)
        .replace("line 2\nline 3\n", "TWO\nTHREE\n")
        .replace("line 10\n", "")
        .replace("line 15\n", "line 15\nadded\n")
}

/// The app on `f.txt` of a repository whose work tree holds `changed()`.
async fn setup() -> (tempfile::TempDir, std::path::PathBuf, App) {
    let (d, root, state) = dirs();
    repo_with(&root, &[("f.txt", &numbered(20))], &[("f.txt", &changed())]);
    let mut app = open_file_app(&root, &state, "f.txt");
    // Typing would open a word list.
    app.settings.word_based_suggestions = false;
    assert!(settle_markers(&mut app).await);
    (d, root, app)
}

/// Selects whole lines `first..=last` with real keys (the selection ends at column 0 of the next).
fn select_lines(app: &mut App, first: usize, last: usize) {
    app.press_file_start();
    for _ in 0..first {
        app.press(KeyCode::Down, M::NONE);
    }
    for _ in first..=last {
        app.press(KeyCode::Down, M::SHIFT);
    }
}

/// One selection per `(first, last)` pair of 0-based lines, each over its lines' text.
fn select_each(app: &mut App, spans: &[(usize, usize)]) {
    let (buf, view) = app.active_mut().unwrap();
    let text = buf.rope.to_string();
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let list = spans
        .iter()
        .map(|&(a, b)| Selection::new(ByteIdx(starts[a]), ByteIdx(starts[b + 1].min(text.len()))))
        .collect();
    view.selections = Selections::new(list, 0);
}

async fn settle_done(app: &mut App, what: &str) {
    assert!(
        app.settle(|a| a.quick_diff.staging == 0 && a.message.as_ref().is_some_and(|m| m.text.contains(what)))
            .await,
        "no `{what}`: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
}

fn index_text(root: &Path) -> String {
    git(root, &["show", ":f.txt"])
}

fn buffer_text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

/// `changed()` with the picks of the tests below staged: `TWO` (its hunk's old lines go) and the
/// deletion of `line 10`; `THREE` and `added` stay unstaged.
fn staged_two_and_deletion() -> String {
    numbered(20)
        .replace("line 2\nline 3\n", "TWO\n")
        .replace("line 10\n", "")
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_stages_part_of_a_hunk_and_another_hunk() {
    let (_d, root, mut app) = setup().await;
    // `TWO` alone (real keys), and line 9 beside the deletion of line 10 (selection touching it).
    select_lines(&mut app, 2, 2);
    run_command(&mut app, "Git: Stage Selected Ranges");
    settle_done(&mut app, "Staged 3 changed lines").await;
    assert_eq!(index_text(&root), numbered(20).replace("line 2\nline 3\n", "TWO\n"));
    assert_eq!(
        git(&root, &["diff", "--numstat"]),
        "2\t1\tf.txt\n",
        "the rest is unstaged"
    );
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), changed());
    assert!(settle_markers(&mut app).await);

    select_lines(&mut app, 9, 9);
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "Staged 1 changed line").await;
    assert_eq!(index_text(&root), staged_two_and_deletion());
    // Nothing of the other lines: `THREE` and `added` are still the work tree's change.
    let unstaged = git(&root, &["diff", "--no-color", "-U0"]);
    assert!(unstaged.contains("+THREE") && unstaged.contains("+added"), "{unstaged}");
    assert!(!unstaged.contains("line 10"), "{unstaged}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_several_selections() {
    let (_d, root, mut app) = setup().await;
    // `TWO`, the deleted line's neighbour, and the added line: three selections, one stage.
    select_each(&mut app, &[(2, 2), (9, 9), (15, 15)]);
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "Staged 5 changed lines").await;
    assert_eq!(
        index_text(&root),
        staged_two_and_deletion().replace("line 15\n", "line 15\nadded\n")
    );
    assert_eq!(
        git(&root, &["diff", "--numstat"]),
        "1\t0\tf.txt\n",
        "only THREE is left"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_asks_to_save_a_dirty_buffer_first() {
    let (_d, root, mut app) = setup().await;
    select_lines(&mut app, 5, 5);
    app.press(KeyCode::Left, M::NONE);
    app.type_text("x");
    assert!(app.active_buffer().unwrap().is_dirty());
    assert!(settle_markers(&mut app).await);
    select_lines(&mut app, 5, 5);
    run_command(&mut app, "Git: Stage Selected Ranges");
    assert!(matches!(
        app.prompt,
        Some(Prompt::SaveBeforeStage { what: What::Ranges, .. })
    ));
    let term = {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
        t.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        t
    };
    let screen: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("before staging the selected lines?"), "{screen}");
    // Cancel: nothing staged, still dirty.
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.prompt.is_none() && app.active_buffer().unwrap().is_dirty());
    assert_eq!(git(&root, &["diff", "--cached"]), "");
    // Save and stage: the selection's line, from the file on disk.
    run_command(&mut app, "Git: Stage Selected Ranges");
    app.press(KeyCode::Char('s'), M::NONE);
    settle_done(&mut app, "Staged 2 changed lines").await;
    assert!(!app.active_buffer().unwrap().is_dirty());
    assert_eq!(
        index_text(&root),
        numbered(20).replace("line 5\n", "xline 5\n"),
        "only the selected line, not the other changes saved with it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_nothing_picked_writes_nothing() {
    let (_d, root, mut app) = setup().await;
    let before = git(&root, &["ls-files", "-s"]);
    select_lines(&mut app, 12, 13);
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "No changes in the selection").await;
    app.test_execute(Command::UnstageSelectedRanges);
    settle_done(&mut app, "No changes in the selection").await;
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "No changes in the selection").await;
    assert_eq!(git(&root, &["ls-files", "-s"]), before);
    assert_eq!(buffer_text(&app), changed());
    assert!(!app.active_buffer().unwrap().is_dirty());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_unstages_a_subset() {
    let (_d, root, mut app) = setup().await;
    git(&root, &["add", "f.txt"]);
    assert!(app.settle(|a| !a.git.running()).await);
    // Everything is staged; take back `TWO` (with its hunk's old lines) and the addition.
    select_each(&mut app, &[(2, 2), (15, 15)]);
    app.test_execute(Command::UnstageSelectedRanges);
    settle_done(&mut app, "Unstaged 4 changed lines").await;
    assert_eq!(
        index_text(&root),
        changed()
            .replace("TWO\nTHREE\n", "line 2\nline 3\nTHREE\n")
            .replace("added\n", "")
    );
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), changed());
    // The same selection again: the lines aren't staged any more.
    app.test_execute(Command::UnstageSelectedRanges);
    settle_done(&mut app, "No changes in the selection").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_unstage_maps_the_buffers_lines_to_the_index() {
    let (_d, root, mut app) = setup().await;
    git(&root, &["add", "f.txt"]);
    // Lines inserted above shift the buffer's lines against the index's; the buffer is dirty and
    // unstaging doesn't ask.
    app.press_file_start();
    app.type_text("a\nb\n");
    select_each(&mut app, &[(5, 5)]);
    assert_eq!(buffer_text(&app).lines().nth(5), Some("THREE"));
    app.test_execute(Command::UnstageSelectedRanges);
    assert!(app.prompt.is_none());
    settle_done(&mut app, "Unstaged 3 changed lines").await;
    assert!(app.active_buffer().unwrap().is_dirty(), "not saved");
    assert_eq!(
        index_text(&root),
        changed().replace("TWO\nTHREE\n", "line 2\nline 3\nTWO\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_reverts_in_one_undo_step() {
    let (_d, root, mut app) = setup().await;
    let before = buffer_text(&app);
    // `TWO` and the deleted line's neighbour: lines 2 and 3 come back (THREE stays), line 10 too.
    select_each(&mut app, &[(2, 2), (9, 9)]);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted 4 changed lines (not saved)").await;
    let reverted = changed()
        .replace("TWO\nTHREE\n", "line 2\nline 3\nTHREE\n")
        .replace("line 9\n", "line 9\nline 10\n");
    assert_eq!(buffer_text(&app), reverted);
    assert!(app.active_buffer().unwrap().is_dirty());
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        changed(),
        "disk untouched"
    );
    // One step: a single undo puts every line back, redo does it again.
    app.test_execute(Command::Undo);
    assert_eq!(buffer_text(&app), before);
    app.test_execute(Command::Redo);
    assert_eq!(buffer_text(&app), reverted);
    // The markers follow the edit: the reverted lines are no longer changes.
    assert!(settle_markers(&mut app).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_revert_is_refused_when_it_cannot_apply() {
    let (_d, _root, mut app) = setup().await;
    select_lines(&mut app, 2, 2);
    // The buffer changes while the diff is worked out: nothing is applied.
    app.test_execute(Command::RevertSelectedRanges);
    app.type_text("q");
    let typed = buffer_text(&app);
    settle_done(&mut app, "the file changed, try again").await;
    assert_eq!(buffer_text(&app), typed);

    // Read-only: refused at once.
    app.active_mut().unwrap().0.read_only = Some("test".into());
    app.test_execute(Command::RevertSelectedRanges);
    assert!(app.message.as_ref().unwrap().text.contains("Read-only: test"));
    assert_eq!(buffer_text(&app), typed);
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_revert_is_refused_for_an_untracked_file() {
    let (_d, root, state) = dirs();
    repo_with(&root, &[("f.txt", "a\n")], &[]);
    fs::write(root.join("new.txt"), "n\n").unwrap();
    let mut app = open_file_app(&root, &state, "new.txt");
    assert!(settle_markers(&mut app).await);
    select_lines(&mut app, 0, 0);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "isn't in the index").await;
    assert_eq!(buffer_text(&app), "n\n");
    assert!(!app.active_buffer().unwrap().is_dirty());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_commands_are_in_the_palette_and_unbound() {
    let all = Command::all();
    for (cmd, id, title) in [
        (
            Command::StageSelectedRanges,
            "git.stageSelectedRanges",
            "Git: Stage Selected Ranges",
        ),
        (
            Command::UnstageSelectedRanges,
            "git.unstageSelectedRanges",
            "Git: Unstage Selected Ranges",
        ),
        (
            Command::RevertSelectedRanges,
            "git.revertSelectedRanges",
            "Git: Revert Selected Ranges",
        ),
    ] {
        assert!(all.contains(&cmd));
        assert_eq!(cmd.name(), id);
        assert_eq!(cmd.title(), title);
        assert_eq!(Command::from_name(id), Some(cmd));
    }
}

/// A bare cursor (no selection) in a changed line is that line: it stages exactly it, and
/// reverts exactly it.
#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_a_bare_cursor_picks_its_line() {
    let (_d, root, mut app) = setup().await;
    app.press_file_start();
    for _ in 0..15 {
        app.press(KeyCode::Down, M::NONE);
    }
    assert_eq!(buffer_text(&app).lines().nth(15), Some("added"));
    assert!(!app.active_buffer().unwrap().is_dirty());
    // The cursor is on `added`, nothing selected.
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted 1 changed line (not saved)").await;
    assert_eq!(buffer_text(&app), changed().replace("added\n", ""));
    app.test_execute(Command::Undo);
    assert_eq!(buffer_text(&app), changed());
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "Staged 1 changed line").await;
    assert_eq!(index_text(&root), numbered(20).replace("line 15\n", "line 15\nadded\n"));
}

/// Reverting on a file with a BOM: the buffer's text has none, the index's blob does, so byte
/// offsets in the diff are shifted by 3 and a change on the first line must not duplicate or
/// lose the BOM.
#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_revert_on_a_bom_file() {
    let (_d, root, state) = dirs();
    let (old, new) = ("\u{feff}a\nb\nc\nd\n", "\u{feff}A\nb\nC\nd\n");
    repo_with(&root, &[("f.txt", old)], &[("f.txt", new)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    assert!(app.active_buffer().unwrap().bom);
    // The last change only.
    select_each(&mut app, &[(2, 2)]);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted 2 changed lines (not saved)").await;
    assert_eq!(buffer_text(&app), "A\nb\nc\nd\n");
    // The first line, whose edit starts at the BOM's end.
    select_each(&mut app, &[(0, 0)]);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted 2 changed lines (not saved)").await;
    assert_eq!(buffer_text(&app), "a\nb\nc\nd\n");
    assert!(app.active_buffer().unwrap().bom);
    // Saved, the file is the index's again, BOM once.
    app.test_execute(Command::Save);
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);
    assert_eq!(fs::read(root.join("f.txt")).unwrap(), old.as_bytes());
}

/// Reverting on a CRLF file: the replaced lines keep their `\r\n`.
#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_revert_on_a_crlf_file() {
    let (_d, root, state) = dirs();
    let (old, new) = ("a\r\nb\r\nc\r\nd\r\n", "a\r\nB\r\nc\r\nD\r\nE\r\n");
    repo_with(&root, &[("f.txt", old)], &[("f.txt", new)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    select_each(&mut app, &[(1, 1), (3, 4)]);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted").await;
    assert_eq!(buffer_text(&app), old);
    app.test_execute(Command::Undo);
    assert_eq!(buffer_text(&app), new);
}

// Subtask c: the same three commands on a diff tab's row selection.

use crate::diff_caret::Caret;
use crate::diff_view::Pair;

/// A repository with `f.txt` changed as `changed()` (`staged`: and added to the index) and
/// `g.txt`; the app on `f.txt` (`open`) or `g.txt` (so `f.txt` isn't open), with `f.txt`'s diff
/// for `pair` open and loaded.
async fn diff_setup(open: bool, pair: Pair, staged: bool) -> (tempfile::TempDir, std::path::PathBuf, App) {
    let (d, root, state) = dirs();
    repo_with(
        &root,
        &[("f.txt", &numbered(20)), ("g.txt", "g\n")],
        &[("f.txt", &changed())],
    );
    if staged {
        git(&root, &["add", "f.txt"]);
    }
    let mut app = open_file_app(&root, &state, if open { "f.txt" } else { "g.txt" });
    app.settings.word_based_suggestions = false;
    assert!(settle_markers(&mut app).await);
    app.open_diff(root.join("f.txt"), pair);
    assert!(crate::milestone10_tests::settle_diffs(&mut app).await);
    (d, root, app)
}

/// Selects lines `first..=last` of one side of the active diff tab, in the layout asked for.
fn pick_rows(app: &mut App, right: bool, first: usize, last: usize, inline: bool) {
    let d = app.group_mut().active_diff_mut().unwrap();
    d.set_layout(inline);
    let len = d.side_text(right, last).len();
    d.caret = Caret {
        right,
        anchor: (first, 0),
        head: (last, len),
        col: None,
    };
    d.cursor = d.aligned().row_of(right, last);
}

async fn settle_diff_done(app: &mut App, what: &str) {
    assert!(
        app.settle(|a| a.diffs.staging == 0
            && a.quick_diff.staging == 0
            && a.file_history.running() == 0
            && a.message.as_ref().is_some_and(|m| m.text.contains(what)))
            .await,
        "no `{what}`: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_stages_part_of_a_hunk_split() {
    // A file not open, then an open one: `TWO`'s row (its removed line too), not `THREE`'s.
    for open in [false, true] {
        let (_d, root, mut app) = diff_setup(open, Pair::UNSTAGED, false).await;
        pick_rows(&mut app, true, 2, 2, false);
        app.test_execute(Command::StageSelectedRanges);
        settle_diff_done(&mut app, "Staged 2 changed lines").await;
        assert_eq!(
            index_text(&root),
            numbered(20).replace("line 2\nline 3\n", "line 3\nTWO\n"), // the unpicked removed line stays (git add -p edit)
            "open {open}"
        );
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), changed());
        // The diff tab reads the index again: that hunk is down to `THREE`.
        assert!(app.settle(|a| a.diffs_settled() && !a.git.running()).await);
        let unstaged = git(&root, &["diff", "--no-color", "-U0"]);
        assert!(unstaged.contains("+THREE") && !unstaged.contains("+TWO"), "{unstaged}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_inline_left_side_takes_only_removed_lines() {
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    // The removed lines `line 2` and `line 3` on the left side; the added ones aren't covered.
    pick_rows(&mut app, false, 2, 3, true);
    app.test_execute(Command::StageSelectedRanges);
    settle_diff_done(&mut app, "Staged 2 changed lines").await;
    assert_eq!(index_text(&root), numbered(20).replace("line 2\nline 3\n", ""));
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_selection_with_real_keys() {
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    // The cursor starts on the first change (TWO's row); two Shift+Downs end at column 0 of the
    // third row, which isn't covered: TWO's and THREE's rows.
    app.group_mut().active_diff_mut().unwrap().set_layout(false);
    app.press(KeyCode::Down, M::SHIFT);
    app.press(KeyCode::Down, M::SHIFT);
    app.test_execute(Command::StageSelectedRanges);
    settle_diff_done(&mut app, "Staged 4 changed lines").await;
    assert_eq!(
        index_text(&root),
        numbered(20).replace("line 2\nline 3\n", "TWO\nTHREE\n")
    );
    // The other hunks are untouched.
    assert!(git(&root, &["diff", "--no-color", "-U0"]).contains("+added"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_unstages_a_subset() {
    let (_d, root, mut app) = diff_setup(false, Pair::STAGED, true).await;
    assert_eq!(index_text(&root), changed());
    // The added line (a right-only row) goes back out of the index.
    pick_rows(&mut app, true, 15, 15, false);
    app.test_execute(Command::UnstageSelectedRanges);
    settle_diff_done(&mut app, "Unstaged 1 changed line").await;
    assert_eq!(index_text(&root), changed().replace("added\n", ""));
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), changed());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_reverts_an_open_file_in_one_undo_step() {
    let (_d, root, mut app) = diff_setup(true, Pair::UNSTAGED, false).await;
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::RevertSelectedRanges);
    assert_eq!(message(&app), "Reverted 2 changed lines in f.txt (not saved)");
    // The file's tab (not the diff) holds the edit.
    let buf = app.buffers.iter().next().unwrap().1;
    assert_eq!(buf.rope.to_string(), changed().replace("TWO\n", "line 2\n"));
    assert!(buf.is_dirty());
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        changed(),
        "disk untouched"
    );
    app.test_execute(Command::PrevTab);
    app.test_execute(Command::Undo);
    assert_eq!(buffer_text(&app), changed(), "one undo step");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_reverts_a_file_that_is_not_open_and_records_history() {
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    let index_before = index_text(&root);
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::RevertSelectedRanges);
    settle_diff_done(&mut app, "Reverted 2 changed lines in f.txt").await;
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        changed().replace("TWO\n", "line 2\n")
    );
    assert_eq!(index_text(&root), index_before);
    let entries = crate::history::entries(&root, Path::new("f.txt")).unwrap();
    assert_eq!(entries.len(), 1);
    let kept = crate::history::read(&root, Path::new("f.txt"), &entries[0].id).unwrap();
    assert_eq!(String::from_utf8(kept).unwrap(), changed());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_asks_to_save_a_dirty_buffer_first() {
    let (_d, root, mut app) = diff_setup(true, Pair::UNSTAGED, false).await;
    // Edit line 5 in the file's tab: the diff tab compares the buffer's text.
    app.test_execute(Command::PrevTab);
    select_lines(&mut app, 5, 5);
    app.press(KeyCode::Left, M::NONE);
    app.type_text("x");
    app.test_execute(Command::NextTab);
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().hunks().len() == 4)
            .await
    );
    pick_rows(&mut app, true, 5, 5, false);
    app.test_execute(Command::StageSelectedRanges);
    assert!(matches!(
        app.prompt,
        Some(Prompt::SaveBeforeStage { what: What::Diff, .. })
    ));
    assert_eq!(git(&root, &["diff", "--cached"]), "");
    app.press(KeyCode::Char('s'), M::NONE);
    settle_diff_done(&mut app, "Staged 2 changed lines").await;
    assert_eq!(index_text(&root), numbered(20).replace("line 5\n", "xline 5\n"));
    assert!(!app.buffers.iter().next().unwrap().1.is_dirty());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_refusals_change_nothing() {
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::UnstageSelectedRanges);
    assert_eq!(message(&app), "These changes aren't staged");
    // Rows with no change: nothing picked.
    pick_rows(&mut app, true, 5, 6, false);
    for cmd in [Command::StageSelectedRanges, Command::RevertSelectedRanges] {
        app.test_execute(cmd);
        assert_eq!(message(&app), "No changes in the selection");
    }
    assert_eq!(index_text(&root), numbered(20));
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), changed());

    // A staged diff: staging again and reverting are refused.
    let (_d, root, mut app) = diff_setup(false, Pair::STAGED, true).await;
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::StageSelectedRanges);
    assert_eq!(message(&app), "These changes are already staged");
    app.test_execute(Command::RevertSelectedRanges);
    assert!(message(&app).starts_with("Only working tree changes can be reverted"));
    assert_eq!(index_text(&root), changed());
    assert_eq!(app.diffs.staging + app.quick_diff.staging, 0);

    // Not git's: the saved-file comparison of an open file.
    let (_d, root, state) = dirs();
    repo_with(&root, &[("f.txt", &numbered(20))], &[("f.txt", &changed())]);
    let mut app = open_file_app(&root, &state, "f.txt");
    app.open_diff(root.join("f.txt"), Pair::SAVED);
    assert!(crate::milestone10_tests::settle_diffs(&mut app).await);
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::StageSelectedRanges);
    assert_eq!(message(&app), "This diff isn't git's: nothing to stage");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_inline_selection_with_real_keys() {
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    // Inline, the first change's rows are the removed `line 2` / `line 3`, then `TWO` / `THREE`:
    // one Shift+Down ends at column 0 of the second row, so only `line 2` is picked.
    app.group_mut().active_diff_mut().unwrap().set_layout(true);
    app.press(KeyCode::Down, M::SHIFT);
    app.test_execute(Command::StageSelectedRanges);
    settle_diff_done(&mut app, "Staged 1 changed line").await;
    assert_eq!(index_text(&root), numbered(20).replace("line 2\n", ""));
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_revert_of_a_read_only_buffer_changes_nothing() {
    let (_d, _root, mut app) = diff_setup(true, Pair::UNSTAGED, false).await;
    app.buffers.iter_mut().next().unwrap().1.read_only = Some("test".into());
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::RevertSelectedRanges);
    assert!(message(&app).starts_with("Read-only"), "{}", message(&app));
    assert_eq!(app.buffers.iter().next().unwrap().1.rope.to_string(), changed());
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_refuses_when_the_file_changed_on_disk() {
    // The diff is shown, then the file (not open) changes underneath: nothing is written.
    for cmd in [Command::StageSelectedRanges, Command::RevertSelectedRanges] {
        let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
        let other = changed().replace("line 7\n", "seven\n");
        fs::write(root.join("f.txt"), &other).unwrap();
        pick_rows(&mut app, true, 2, 2, false);
        app.test_execute(cmd);
        settle_diff_done(&mut app, "the file changed").await;
        assert_eq!(index_text(&root), numbered(20), "{cmd:?}");
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), other, "{cmd:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_revert_refuses_a_buffer_edited_since_the_diff() {
    let (_d, _root, mut app) = diff_setup(true, Pair::UNSTAGED, false).await;
    // Edit the file's tab and come straight back, before the diff reads the buffer again.
    app.test_execute(Command::PrevTab);
    app.press_file_start();
    app.type_text("x");
    app.test_execute(Command::NextTab);
    assert!(app.group().active_diff().is_some());
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::RevertSelectedRanges);
    assert_eq!(
        message(&app),
        "Can't revert the change in f.txt: the file changed, try again"
    );
    assert_eq!(
        app.buffers.iter().next().unwrap().1.rope.to_string(),
        format!("x{}", changed())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_diff_tab_cancelled_save_stages_nothing() {
    let (_d, root, mut app) = diff_setup(true, Pair::UNSTAGED, false).await;
    app.test_execute(Command::PrevTab);
    select_lines(&mut app, 5, 5);
    app.press(KeyCode::Left, M::NONE);
    app.type_text("x");
    app.test_execute(Command::NextTab);
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().hunks().len() == 4)
            .await
    );
    pick_rows(&mut app, true, 5, 5, false);
    app.test_execute(Command::StageSelectedRanges);
    assert!(app.prompt.is_some());
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.prompt.is_none());
    assert!(app.settle(|a| a.quick_diff.staging == 0).await);
    assert_eq!(git(&root, &["diff", "--cached"]), "");
    assert!(app.buffers.iter().next().unwrap().1.is_dirty());
}

// Subtask d: the done-when, as scenarios against `git apply` of hand-edited patches.

use crate::git::test_repo::git_with_input;

/// A copy of the repository next to it (the index and work tree as they are now).
fn copy_repo(root: &Path, name: &str) -> std::path::PathBuf {
    let to = root.with_file_name(name);
    assert!(
        std::process::Command::new("cp")
            .arg("-a")
            .arg(root)
            .arg(&to)
            .status()
            .unwrap()
            .success()
    );
    to
}

/// `git apply --unidiff-zero [--cached] [-R]` of `hunks` (zero-context, for `f.txt`) in `dir`.
fn hand_apply(dir: &Path, cached: bool, reverse: bool, hunks: &str) {
    let mut args = vec!["apply", "--unidiff-zero"];
    args.extend(cached.then_some("--cached"));
    args.extend(reverse.then_some("-R"));
    git_with_input(dir, &args, Some(&format!("--- a/f.txt\n+++ b/f.txt\n{hunks}")));
}

/// `TWO`'s hunk with its old lines, and the deletion of `line 10`: what picking `TWO` and the
/// deletion's neighbour stages, as `git add -p`'s edit would leave the index.
const STAGE_TWO_AND_DELETION: &str = "@@ -3,2 +3,1 @@\n-line 2\n-line 3\n+TWO\n@@ -11,1 +9,0 @@\n-line 10\n";
/// The whole first hunk.
const TWO_AND_THREE: &str = "@@ -3,2 +3,2 @@\n-line 2\n-line 3\n+TWO\n+THREE\n";
/// Just the first of those.
const TWO_HUNK: &str = "@@ -3,2 +3,1 @@\n-line 2\n-line 3\n+TWO\n";

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_done_when_text_tab_stage_unstage_revert() {
    let (_d, root, mut app) = setup().await;
    // A dirty buffer: line 18 edited, outside every pick.
    select_lines(&mut app, 18, 18);
    app.press(KeyCode::Left, M::NONE);
    app.type_text("x");
    assert!(settle_markers(&mut app).await);
    let work = changed().replace("line 18\n", "xline 18\n");
    assert_eq!(buffer_text(&app), work);

    // Stage: part of the mixed hunk (`TWO`) and another hunk (the deletion), saved first.
    let by_hand = copy_repo(&root, "by_hand");
    hand_apply(&by_hand, true, false, STAGE_TWO_AND_DELETION);
    select_each(&mut app, &[(2, 2), (9, 9)]);
    run_command(&mut app, "Git: Stage Selected Ranges");
    assert!(matches!(
        app.prompt,
        Some(Prompt::SaveBeforeStage { what: What::Ranges, .. })
    ));
    app.press(KeyCode::Char('s'), M::NONE);
    settle_done(&mut app, "Staged 4 changed lines").await;
    assert_eq!(index_text(&root), git(&by_hand, &["show", ":f.txt"]));
    assert_eq!(git(&root, &["ls-files", "-s"]), git(&by_hand, &["ls-files", "-s"]));
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), work, "saved first");
    assert!(!app.active_buffer().unwrap().is_dirty());
    // The hunks nothing was picked in are untouched: `THREE`, the addition and line 18 remain.
    let unstaged = git(&root, &["diff", "--no-color", "-U0"]);
    for line in ["+THREE", "+added", "+xline 18"] {
        assert!(unstaged.contains(line), "{unstaged}");
    }
    assert!(settle_markers(&mut app).await);

    // Unstage a subset: `TWO`'s hunk goes back, the deletion stays staged.
    let by_hand = copy_repo(&root, "by_hand_2");
    hand_apply(&by_hand, true, true, TWO_HUNK);
    select_each(&mut app, &[(2, 2)]);
    app.test_execute(Command::UnstageSelectedRanges);
    settle_done(&mut app, "Unstaged 3 changed lines").await;
    assert_eq!(index_text(&root), git(&by_hand, &["show", ":f.txt"]));
    assert_eq!(index_text(&root), numbered(20).replace("line 10\n", ""));
    assert!(settle_markers(&mut app).await);

    // Revert `TWO` (and its hunk's old lines): the buffer, one undo step, disk untouched.
    let before = buffer_text(&app);
    select_each(&mut app, &[(2, 2)]);
    app.test_execute(Command::RevertSelectedRanges);
    settle_done(&mut app, "Reverted 3 changed lines (not saved)").await;
    assert_eq!(buffer_text(&app), before.replace("TWO\n", "line 2\nline 3\n"));
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), work);
    app.test_execute(Command::Undo);
    assert_eq!(buffer_text(&app), before, "one undo step");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_done_when_diff_tab_both_layouts_and_a_file_not_open() {
    // Stage from the left side of a split diff: its rows 2 and 3 each bring their right line.
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    let by_hand = copy_repo(&root, "by_hand");
    hand_apply(&by_hand, true, false, TWO_AND_THREE);
    pick_rows(&mut app, false, 2, 3, false);
    app.test_execute(Command::StageSelectedRanges);
    settle_diff_done(&mut app, "Staged 4 changed lines").await;
    assert_eq!(index_text(&root), git(&by_hand, &["show", ":f.txt"]));
    assert_eq!(
        index_text(&root),
        numbered(20).replace("line 2\nline 3\n", "TWO\nTHREE\n")
    );

    // Revert a file that isn't open, from the right side of a split diff: disk rewritten like
    // `git apply -R` of the patch, the index alone.
    let (_d, root, mut app) = diff_setup(false, Pair::UNSTAGED, false).await;
    let by_hand = copy_repo(&root, "by_hand");
    hand_apply(&by_hand, false, true, "@@ -3,1 +3,1 @@\n-line 2\n+TWO\n");
    let index = index_text(&root);
    pick_rows(&mut app, true, 2, 2, false);
    app.test_execute(Command::RevertSelectedRanges);
    settle_diff_done(&mut app, "Reverted 2 changed lines in f.txt").await;
    assert_eq!(
        fs::read(root.join("f.txt")).unwrap(),
        fs::read(by_hand.join("f.txt")).unwrap()
    );
    assert_eq!(index_text(&root), index);

    // Unstage from a staged diff, inline, with real keys: the caret starts on the left side, and
    // two Shift+Downs go from `line 2` over `line 3` to the next left line (`line 10`, column 0),
    // so the first hunk's rows (the added ones between) are covered, the deletion's not.
    let (_d, root, mut app) = diff_setup(false, Pair::STAGED, true).await;
    let by_hand = copy_repo(&root, "by_hand");
    hand_apply(&by_hand, true, true, TWO_AND_THREE);
    app.group_mut().active_diff_mut().unwrap().set_layout(true);
    app.press(KeyCode::Down, M::SHIFT);
    app.press(KeyCode::Down, M::SHIFT);
    app.test_execute(Command::UnstageSelectedRanges);
    settle_diff_done(&mut app, "Unstaged 4 changed lines").await;
    assert_eq!(index_text(&root), git(&by_hand, &["show", ":f.txt"]));
    assert_eq!(index_text(&root), changed().replace("TWO\nTHREE\n", "line 2\nline 3\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_done_when_a_conflicted_file_is_refused() {
    let (_d, root, state) = dirs();
    repo_with(&root, &[("f.txt", "a\nb\nc\n")], &[]);
    // Stages 1 / 2 / 3 in the index and markers in the work tree.
    let blob = |text: &str| {
        fs::write(root.join("blob"), text).unwrap();
        git(&root, &["hash-object", "-w", "blob"]).trim().to_string()
    };
    let (base, ours, theirs) = (blob("a\nb\nc\n"), blob("a\nB\nc\n"), blob("a\nX\nc\n"));
    fs::remove_file(root.join("blob")).unwrap();
    let info = format!("100644 {base} 1\tf.txt\n100644 {ours} 2\tf.txt\n100644 {theirs} 3\tf.txt\n");
    git(&root, &["rm", "--cached", "-q", "f.txt"]);
    git_with_input(&root, &["update-index", "--index-info"], Some(&info));
    fs::write(
        root.join("f.txt"),
        "a\n<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\nc\n",
    )
    .unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let index = git(&root, &["ls-files", "-s"]);
    let disk = fs::read_to_string(root.join("f.txt")).unwrap();
    for cmd in [
        Command::StageSelectedRanges,
        Command::UnstageSelectedRanges,
        Command::RevertSelectedRanges,
    ] {
        select_each(&mut app, &[(0, 6)]);
        app.test_execute(cmd);
        assert!(app.settle(|a| a.quick_diff.staging == 0).await);
        let m = message(&app);
        assert!(m.contains("conflict"), "{cmd:?}: {m}");
        assert_eq!(git(&root, &["ls-files", "-s"]), index);
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), disk);
        assert_eq!(buffer_text(&app), disk);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_done_when_ten_thousand_cursors() {
    // 20,000 lines, every other one changed: 10,000 hunks, a cursor on each.
    let old: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    let new: String = (0..20_000)
        .map(|i| {
            if i % 2 == 0 {
                format!("new {i}\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    let (_d, root, state) = dirs();
    repo_with(&root, &[("f.txt", &old)], &[("f.txt", &new)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    app.settings.word_based_suggestions = false;
    assert!(settle_markers(&mut app).await);
    let spans: Vec<(usize, usize)> = (0..10_000).map(|k| (2 * k, 2 * k)).collect();
    select_each(&mut app, &spans);
    let t = std::time::Instant::now();
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "Staged 20000 changed lines").await;
    let took = t.elapsed();
    assert!(took.as_secs() < 30, "10k cursors took {took:?}");
    assert_eq!(index_text(&root), new);
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_selected_lines_done_when_gutter_status_and_open_diff_refresh() {
    let (_d, root, mut app) = setup().await;
    assert!(
        app.active_buffer()
            .unwrap()
            .quick_diff
            .marks()
            .iter()
            .all(|m| !m.staged)
    );
    app.open_diff(root.join("f.txt"), Pair::STAGED);
    assert!(crate::milestone10_tests::settle_diffs(&mut app).await);
    app.test_execute(Command::PrevTab);
    select_each(&mut app, &[(2, 2)]);
    app.test_execute(Command::StageSelectedRanges);
    settle_done(&mut app, "Staged 3 changed lines").await;
    // Source Control: the file is now both staged and unstaged.
    assert!(
        app.settle(|a| !a.git.running()
            && a.git
                .status
                .as_ref()
                .and_then(|s| s.entry(Path::new("f.txt")))
                .is_some_and(|e| e.staged.is_some() && e.unstaged.is_some()))
            .await
    );
    // The gutter: the staged change is marked staged, the rest still unstaged.
    assert!(
        app.settle(|a| {
            let marks = a.active_buffer().unwrap().quick_diff.marks();
            marks.iter().any(|m| m.staged) && marks.iter().any(|m| !m.staged)
        })
        .await
    );
    // The open staged diff tab reads the index again and now has the staged hunk.
    app.test_execute(Command::NextTab);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group_mut().active_diff_mut().unwrap();
    assert_eq!(d.side_text(true, 2), "TWO");
}
