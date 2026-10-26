//! Git change marks in the scroll bar end to end (git_marks-b, the task's "Done when"): a text
//! tab's bar lights a cell per changed line, projected over the whole file and under the thumb,
//! in the gutter's colours (unstaged) or their darker staged siblings (from the index → HEAD
//! diff); a line staged and edited again shows the unstaged colour (unstaged wins); the marks
//! map through an edit and stay at their cell as the view scrolls; a diff tab marks its hunks'
//! rows by `Hunk::kind`; a hex or shell tab and single file mode have none.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers as M;
use crossterm::event::MouseEventKind;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::app::{App, InitialFile, Mode};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::diff_view::Pair;
use crate::git::test_repo::{commit_all, git, init};
use crate::hex::HexDoc;
use crate::milestone10_tests::{bg, dirs, draw_sized, marker_colour, open_file_app, settle_git, settle_markers};
use crate::scrollbar::{Scroll, Scrollable};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

fn numbered(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

/// A repository at `root` with `f.txt` committed as `original` and the working tree holding
/// `changed`.
fn repo(root: &Path, original: &str, changed: &str) {
    init(root);
    fs::write(root.join("f.txt"), original).unwrap();
    commit_all(root, "one");
    fs::write(root.join("f.txt"), changed).unwrap();
}

/// The active group's bar as last drawn.
fn bar(app: &App) -> crate::layout::ScrollBarHit {
    app.hits
        .group(app.groups.active)
        .expect("a group")
        .scrollbar
        .expect("the tab draws a bar")
}

/// The cell of the text tab's bar that `line` projects to.
fn text_cell(app: &App, line: usize) -> u16 {
    let buf = app.active_buffer().expect("a text tab");
    let view = &app.group().active_tab().expect("a tab").view;
    let lines = crate::buffer::pos::line_count(&buf.rope);
    Scroll::text(lines, view.height, view.top_line()).cell(bar(app).track.height, line)
}

/// The cell of the diff tab's bar that `row` projects to.
fn diff_cell(app: &App, row: usize) -> u16 {
    let d = app.group().active_diff().expect("a diff tab");
    d.scroll().cell(bar(app).track.height, row)
}

/// Every scroll-bar mark's colour (the gutter's keys: they hold it in the foreground, the bar
/// fills a blank cell with it).
fn gutter_colours(app: &App) -> Vec<Color> {
    [
        "editorGutter.addedBackground",
        "editorGutter.modifiedBackground",
        "editorGutter.deletedBackground",
        "editorGutter.stagedAddedBackground",
        "editorGutter.stagedModifiedBackground",
        "editorGutter.stagedDeletedBackground",
    ]
    .into_iter()
    .filter_map(|key| marker_colour(app, key))
    .collect()
}

/// Each cell of the group's bar, by its colour.
fn bar_cells(app: &App, term: &Terminal<TestBackend>) -> Vec<Option<Color>> {
    let track = bar(app).track;
    (0..track.height).map(|y| bg(term, (track.x, track.y + y))).collect()
}

/// Scrolls the active editor down with the wheel (past where a minimal reveal of the last line
/// stops), so the thumb is at the track's end and marks at the file's top are clear of it.
fn wheel_down(app: &mut App, times: usize) {
    let text = app.hits.group(app.groups.active).expect("a group").text;
    for _ in 0..times {
        app.mouse(MouseEventKind::ScrollDown, text.x + 1, text.y + 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_text_tab_shows_staged_and_unstaged_changes() {
    let (_d, root, state) = dirs();
    let original = numbered(40);
    init(&root);
    fs::write(root.join("f.txt"), &original).unwrap();
    commit_all(&root, "one");
    // Staged: line 10 changed. Then the working tree changes lines 2 and 20 too.
    fs::write(root.join("f.txt"), original.replace("line 10\n", "line ten\n")).unwrap();
    git(&root, &["add", "f.txt"]);
    let working = original
        .replace("line 10\n", "line ten\n")
        .replace("line 2\n", "line two\n")
        .replace("line 20\n", "line twenty\n");
    fs::write(root.join("f.txt"), &working).unwrap();

    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let shown: Vec<_> = app
        .active_buffer()
        .unwrap()
        .quick_diff
        .marks()
        .iter()
        .map(|m| (m.staged, m.kind))
        .collect();
    assert_eq!(
        shown,
        vec![
            (false, crate::git::HunkKind::Modified),
            (true, crate::git::HunkKind::Modified),
            (false, crate::git::HunkKind::Modified),
        ],
        "lines 2 and 20 unstaged, line 10 staged"
    );
    let _ = draw_sized(&mut app, 100, 24);
    let cells = [text_cell(&app, 2), text_cell(&app, 10), text_cell(&app, 20)];
    assert!(cells[0] != cells[1] && cells[1] != cells[2], "{cells:?}");
    // Drawn at the top the thumb covers the first cells: scroll to the end, where the marks (at
    // the file's top) are clear of it, and check they are the same cells.
    wheel_down(&mut app, 20);
    let term = draw_sized(&mut app, 100, 24);
    assert_eq!([2, 10, 20].map(|line| text_cell(&app, line)), cells, "marks stay put");
    let hit = bar(&app);
    for (cell, line) in cells.into_iter().zip([2, 10, 20]) {
        assert!(
            !hit.thumb.is_some_and(|t| t.contains(cell)),
            "the mark on line {line} is clear of the thumb"
        );
    }
    let unstaged = marker_colour(&app, "editorGutter.modifiedBackground");
    let staged = marker_colour(&app, "editorGutter.stagedModifiedBackground");
    assert_ne!(unstaged, staged, "the staged variant is a different colour");
    let track = hit.track;
    for (cell, line) in cells.into_iter().zip([2, 10, 20]) {
        let want = if line == 10 { staged } else { unstaged };
        assert_eq!(bg(&term, (track.x, track.y + cell)), want, "line {line} at cell {cell}");
    }
    // An unchanged line's cell is not a mark's.
    let plain = text_cell(&app, 6);
    assert_ne!(bg(&term, (track.x, track.y + plain)), unstaged);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_unstaged_wins_where_a_staged_line_is_edited_again() {
    let (_d, root, state) = dirs();
    let original = numbered(30);
    init(&root);
    fs::write(root.join("f.txt"), &original).unwrap();
    commit_all(&root, "one");
    // Lines 5 and 20 staged; line 5 then edited again in the working tree, line 25 changed too.
    fs::write(
        root.join("f.txt"),
        original
            .replace("line 5\n", "line five\n")
            .replace("line 20\n", "line twenty\n"),
    )
    .unwrap();
    git(&root, &["add", "f.txt"]);
    let working = original
        .replace("line 5\n", "line FIVE\n")
        .replace("line 20\n", "line twenty\n")
        .replace("line 25\n", "line twenty-five\n");
    fs::write(root.join("f.txt"), &working).unwrap();

    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let shown: Vec<_> = app
        .active_buffer()
        .unwrap()
        .quick_diff
        .marks()
        .iter()
        .map(|m| (m.staged, m.kind))
        .collect();
    assert_eq!(
        shown,
        vec![
            (false, crate::git::HunkKind::Modified),
            (true, crate::git::HunkKind::Modified),
            (false, crate::git::HunkKind::Modified),
        ],
        "line 5 staged then edited is unstaged; line 20 is staged only"
    );
    let _ = draw_sized(&mut app, 100, 24);
    wheel_down(&mut app, 20);
    let term = draw_sized(&mut app, 100, 24);
    let (unstaged, staged) = (
        marker_colour(&app, "editorGutter.modifiedBackground"),
        marker_colour(&app, "editorGutter.stagedModifiedBackground"),
    );
    let track = bar(&app).track;
    let cell = |line| text_cell(&app, line);
    assert_eq!(bg(&term, (track.x, track.y + cell(5))), unstaged, "unstaged wins");
    assert_eq!(bg(&term, (track.x, track.y + cell(20))), staged, "staged only");
    assert_eq!(bg(&term, (track.x, track.y + cell(25))), unstaged);
    let staged_cells: Vec<u16> = (0..track.height)
        .filter(|&y| bg(&term, (track.x, track.y + y)) == staged)
        .collect();
    assert_eq!(staged_cells, vec![cell(20)], "the staged colour is line 20's only");
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_diff_tab_marks_its_hunks() {
    let (_d, root, state) = dirs();
    let original = numbered(12);
    let changed = original
        .replace("line 2\n", "line two\n")
        .replace("line 5\n", "line 5\nadded\n")
        .replace("line 8\n", "");
    repo(&root, &original, &changed);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(
        settle_git(&mut app, |s| s.entry(Path::new("f.txt")).is_some()).await,
        "the status knows f.txt"
    );
    app.open_diff(root.join("f.txt"), Pair::UNSTAGED);
    assert!(
        app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await,
        "the diff is loaded"
    );
    let term = draw_sized(&mut app, 150, 30);
    assert!(!app.group().active_diff().unwrap().inline, "side by side");
    let d = app.group().active_diff().unwrap();
    assert_eq!(d.hunks().len(), 3);
    let track = bar(&app).track;
    assert!(bar(&app).thumb.is_none(), "a short diff shows no thumb");
    let editor = app.theme.get("editor").bg;
    let (added, deleted, modified) = (
        marker_colour(&app, "editorGutter.addedBackground"),
        marker_colour(&app, "editorGutter.deletedBackground"),
        marker_colour(&app, "editorGutter.modifiedBackground"),
    );
    let starts = d.aligned().starts.clone();
    let inline = d.inline;
    let hunks: Vec<_> = d
        .hunks()
        .iter()
        .map(|h| {
            let rows = if inline {
                h.old.len() + h.new.len()
            } else {
                h.old.len().max(h.new.len())
            };
            (h.kind(), rows)
        })
        .collect();
    assert_eq!(
        hunks.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        vec![
            crate::git::HunkKind::Modified,
            crate::git::HunkKind::Added,
            crate::git::HunkKind::Deleted
        ]
    );
    for (k, &(kind, rows)) in hunks.iter().enumerate() {
        let start = starts[k];
        let want = match kind {
            crate::git::HunkKind::Added => added,
            crate::git::HunkKind::Deleted => deleted,
            crate::git::HunkKind::Modified => modified,
        };
        for row in [start, start + rows - 1] {
            let cell = diff_cell(&app, row);
            assert_eq!(bg(&term, (track.x, track.y + cell)), want, "hunk {k} row {row}");
        }
    }
    // No mark spills past its hunk: every row's cell is its hunk's colour, or the bar's base on
    // an unchanged row (each row projects to its own cell here).
    for row in 0..d.rows().len() {
        let cell = diff_cell(&app, row);
        let want = (0..hunks.len())
            .find(|&k| (starts[k]..starts[k] + hunks[k].1).contains(&row))
            .and_then(|k| match hunks[k].0 {
                crate::git::HunkKind::Added => added,
                crate::git::HunkKind::Deleted => deleted,
                crate::git::HunkKind::Modified => modified,
            })
            .or(editor);
        assert_eq!(bg(&term, (track.x, track.y + cell)), want, "row {row}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_text_tab_deleted_and_added_lines_light_their_cells() {
    let (_d, root, state) = dirs();
    let original = numbered(40);
    init(&root);
    fs::write(root.join("f.txt"), &original).unwrap();
    commit_all(&root, "one");
    // Line 20 deleted and a line added after line 30, in the working tree.
    let changed = original
        .replace("line 20\n", "")
        .replace("line 30\n", "line 30\nadded line\n");
    fs::write(root.join("f.txt"), &changed).unwrap();

    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let buf = app.active_buffer().unwrap();
    let shown: Vec<_> = buf
        .quick_diff
        .marks()
        .iter()
        .map(|m| (m.kind, buf.quick_diff.lines_of(&buf.rope, m).unwrap()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (crate::git::HunkKind::Deleted, 19..20),
            (crate::git::HunkKind::Added, 30..31),
        ],
        "a deletion lights the line above it (19), an addition its own line (30)"
    );
    let _ = draw_sized(&mut app, 100, 24);
    wheel_down(&mut app, 20);
    let term = draw_sized(&mut app, 100, 24);
    let deleted = marker_colour(&app, "editorGutter.deletedBackground");
    let added = marker_colour(&app, "editorGutter.addedBackground");
    let track = bar(&app).track;
    assert_eq!(
        bg(&term, (track.x, track.y + text_cell(&app, 19))),
        deleted,
        "the line above"
    );
    assert_eq!(bg(&term, (track.x, track.y + text_cell(&app, 30))), added);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_survive_an_edit_and_a_scroll() {
    let (_d, root, state) = dirs();
    let original = numbered(200);
    repo(&root, &original, &original.replace("line 100\n", "line one hundred\n"));
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let term = draw_sized(&mut app, 100, 24);
    let before = text_cell(&app, 100);
    let modified = marker_colour(&app, "editorGutter.modifiedBackground");
    let track = bar(&app).track;
    assert_eq!(bg(&term, (track.x, track.y + before)), modified);

    // A line typed at the top: the mark moves with the text at once, mapped through the change
    // set (no recompute needed).
    app.press(KeyCode::Home, M::CONTROL);
    app.press(KeyCode::Enter, M::NONE);
    let term = draw_sized(&mut app, 100, 24);
    let after = text_cell(&app, 101);
    assert_eq!(
        bg(&term, (track.x, track.y + after)),
        modified,
        "the mark followed the edit"
    );
    // Recomputed from the new text, it is still there.
    assert!(settle_markers(&mut app).await);
    let buf = app.active_buffer().unwrap();
    assert!(
        buf.quick_diff
            .marks()
            .iter()
            .any(|m| buf.quick_diff.lines_of(&buf.rope, m).is_some_and(|l| l.contains(&101))),
        "the recomputed marks cover line 101"
    );
    // Scrolled to the bottom, the mark is still at the same cell (projected over the file).
    wheel_down(&mut app, 100);
    let term = draw_sized(&mut app, 100, 24);
    assert!(app.group().active_tab().unwrap().view.top_line() > 0, "scrolled");
    assert_eq!(text_cell(&app, 101), after, "a mark stays put as you scroll");
    let hit = bar(&app);
    assert!(!hit.thumb.is_some_and(|t| t.contains(after)), "clear of the thumb");
    assert_eq!(bg(&term, (hit.track.x, hit.track.y + after)), modified);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_staged_mark_clears_after_an_external_commit() {
    let (_d, root, state) = dirs();
    let original = numbered(30);
    init(&root);
    fs::write(root.join("f.txt"), &original).unwrap();
    commit_all(&root, "one");
    // Stage a change only (the working tree equals the index): line 10's mark is the staged kind.
    fs::write(root.join("f.txt"), original.replace("line 10\n", "line ten\n")).unwrap();
    git(&root, &["add", "f.txt"]);

    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let shown: Vec<_> = app
        .active_buffer()
        .unwrap()
        .quick_diff
        .marks()
        .iter()
        .map(|m| (m.staged, m.kind))
        .collect();
    assert_eq!(shown, vec![(true, crate::git::HunkKind::Modified)], "line 10 staged");

    // Commit outside jed: the staged change moves into HEAD, so its mark must go. The quick
    // diff's cached staged half is keyed by the index blob, which the commit doesn't change, so
    // this only clears because `head_changed` marks it stale and it is recomputed against the new
    // HEAD (index == HEAD now: nothing to mark).
    git(&root, &["commit", "-q", "-m", "two"]);
    assert!(
        settle_git(&mut app, |s| s.entries.is_empty() && s.head_id.is_some()).await,
        "the commit is seen"
    );
    assert!(settle_markers(&mut app).await);
    assert!(
        app.active_buffer().unwrap().quick_diff.marks().is_empty(),
        "the staged mark moved into HEAD"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_marks_hex_shell_and_single_file_tabs_have_none() {
    // A hex tab of a changed file: no marks (no buffer's quick diff draws on it).
    let (_d, root, state) = dirs();
    init(&root);
    fs::write(root.join("bin.dat"), b"a\0b\n").unwrap();
    commit_all(&root, "one");
    fs::write(root.join("bin.dat"), b"a\0B\n").unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.open_initial_hex(root.join("bin.dat"), HexDoc::new(b"a\0B\n".to_vec()));
    app.test_start();
    let term = draw_sized(&mut app, 100, 24);
    assert!(app.active_hex().is_some(), "a hex tab");
    let colours = gutter_colours(&app);
    let cells = bar_cells(&app, &term);
    assert!(
        cells.iter().all(|c| !c.is_some_and(|c| colours.contains(&c))),
        "no marks on a hex tab: {cells:?}"
    );

    // A shell tab draws a bar (the terminal's scroll bar), but no marks on it.
    let (_d, root, state) = dirs();
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.test_start();
    app.tasks.shell = PathBuf::from("/bin/sh");
    app.test_execute(Command::NewShell);
    assert!(app.group().active_shell().is_some(), "a shell tab");
    let term = draw_sized(&mut app, 100, 24);
    let colours = gutter_colours(&app);
    let cells = bar_cells(&app, &term);
    assert!(
        cells.iter().all(|c| !c.is_some_and(|c| colours.contains(&c))),
        "no marks on a shell tab: {cells:?}"
    );

    // Single file mode: no git status, so no marks (the bar is still drawn).
    let (_d, root, state) = dirs();
    let original = numbered(20);
    repo(&root, &original, &original.replace("line 3\n", "line three\n"));
    let ws = Workspace::load(root.clone());
    let buffer = Buffer::open(&root.join("f.txt")).unwrap();
    let initial = Some(InitialFile { buffer, goto: None });
    let identity = App::start_identity(&ws, Mode::SingleFile, initial.as_ref());
    let mut app = App::new_in(
        Mode::SingleFile,
        ws,
        TrustStore::load(state.join("trust.toml")),
        initial,
        identity,
        true,
    );
    app.test_start();
    app.test_plain_whitespace();
    let term = draw_sized(&mut app, 100, 24);
    assert!(
        app.active_buffer().unwrap().quick_diff.marks().is_empty(),
        "no quick diff in single file mode"
    );
    let cells = bar_cells(&app, &term);
    assert!(
        cells.iter().all(|c| !c.is_some_and(|c| colours.contains(&c))),
        "no marks in single file mode: {cells:?}"
    );
}
