//! The compare_files task, subtask compare_files-a, end to end (through `App::open_compare`,
//! which the commands of subtask b call). Done when: a diff tab compares two files (disk), a
//! file with an open buffer's unsaved text (the disk's text isn't shown), an untitled buffer
//! with a file or with another untitled buffer; the tab is titled `a ↔ b` with a dirty mark on
//! an unsaved side; typing in either buffer, or either file changing on disk, updates the diff; a
//! binary side says so; a closed `Open` buffer's side (untitled too) freezes to its last text (a
//! `File` side reads the disk again, unmarked) and the tab stays; comparing a
//! text with itself says so and opens nothing; the tab draws side by side and inline without
//! Stage / Revert / Open File; sessions don't keep it.
//!
//! Subtask compare_files-b, through the palette (keys and a click): "Compare Active File With…"
//! lists the open texts (unsaved marked, untitled by title, never the active one) then the
//! files not open (none in single file mode), a query filtering both, and compares the pick
//! (left) with the active buffer; "Compare Open Editors…" picks two open texts in turn; Esc
//! opens nothing; too few texts, or no text active, says so.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Level};
use crate::buffer::{Buffer, BufferId};
use crate::diff_view::Source;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// An app on a workspace with `a.txt` ("one\ntwo\n") and `b.txt` ("one\nthree\n"), nothing open.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, App) {
    setup_watching(false)
}

/// `setup`, with the file watcher on when `watch`.
fn setup_watching(watch: bool) -> (tempfile::TempDir, PathBuf, PathBuf, App) {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(root.join("b.txt"), "one\nthree\n").unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    if watch {
        app.enable_file_watching();
    }
    app.test_start();
    (dir, root, state, app)
}

fn open(app: &mut App, path: &Path) -> BufferId {
    app.test_open(Buffer::open(path).unwrap());
    app.group().active_tab().unwrap().buffer
}

/// Cmd+N: a new untitled buffer with `text` typed into it.
fn untitled(app: &mut App, text: &str) -> BufferId {
    app.press(K::Char('n'), cmd());
    app.type_text(text);
    app.group().active_tab().unwrap().buffer
}

async fn compare(app: &mut App, left: Source, right: Source) {
    app.open_compare(left, right);
    assert!(app.settle(|a| a.diffs_settled()).await, "loaded");
}

fn sides(app: &App) -> (Vec<String>, Vec<String>) {
    let doc = app.group().active_diff().unwrap().doc.clone().expect("loaded");
    let lines = |l: &crate::diff_view::Lines| (0..l.len()).map(|i| l.line(i).to_string()).collect();
    (lines(&doc.left), lines(&doc.right))
}

fn title(app: &App) -> String {
    app.group().active_diff().unwrap().title()
}

fn lines(app: &mut App, width: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

fn file(root: &Path, name: &str) -> Source {
    Source::File(root.join(name))
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_two_files_on_disk() {
    let (_d, root, _s, mut app) = setup();
    compare(&mut app, file(&root, "a.txt"), file(&root, "b.txt")).await;
    assert_eq!(title(&app), "a.txt ↔ b.txt");
    let (l, r) = sides(&app);
    assert_eq!(
        (l, r),
        (vec!["one".into(), "two".into()], vec!["one".into(), "three".into()])
    );
    assert_eq!(app.group().active_diff().unwrap().hunks().len(), 1);
    assert!(app.buffers.find_path(&root.join("a.txt")).is_none(), "nothing opened");
    // Again: the same tab, shown again; the other way round is another tab.
    let tabs = app.group().tabs.len();
    open(&mut app, &root.join("a.txt"));
    app.open_compare(file(&root, "a.txt"), file(&root, "b.txt"));
    assert_eq!(app.group().tabs.len(), tabs + 1, "only a.txt's text tab added");
    assert_eq!(title(&app), "a.txt ↔ b.txt");
    app.open_compare(file(&root, "b.txt"), file(&root, "a.txt"));
    assert_eq!(app.group().tabs.len(), tabs + 2);
    assert_eq!(title(&app), "b.txt ↔ a.txt");
}

/// A file not open that changes on disk is read again, on either side.
#[tokio::test(flavor = "multi_thread")]
async fn compare_files_follows_disk_changes_of_either_file() {
    let (_d, root, _s, mut app) = setup_watching(true);
    compare(&mut app, file(&root, "a.txt"), file(&root, "b.txt")).await;
    fs::write(root.join("a.txt"), "one\nleft\n").unwrap();
    let left = |a: &App| {
        a.group()
            .active_diff()
            .unwrap()
            .doc
            .as_ref()
            .is_some_and(|d| d.left.line(1) == "left")
    };
    assert!(app.settle(|a| left(a) && a.diffs_settled()).await, "left file re-read");
    fs::write(root.join("b.txt"), "one\nright\n").unwrap();
    let right = |a: &App| {
        a.group()
            .active_diff()
            .unwrap()
            .doc
            .as_ref()
            .is_some_and(|d| d.right.line(1) == "right")
    };
    assert!(
        app.settle(|a| right(a) && a.diffs_settled()).await,
        "right file re-read"
    );
}

/// A binary side: the tab says so instead of showing rows.
#[tokio::test(flavor = "multi_thread")]
async fn compare_files_binary_says_binary() {
    let (_d, root, _s, mut app) = setup();
    fs::write(root.join("c.bin"), b"a\0b").unwrap();
    compare(&mut app, file(&root, "a.txt"), file(&root, "c.bin")).await;
    assert!(app.group().active_diff().unwrap().doc.as_ref().unwrap().binary);
    let rows = lines(&mut app, 100).join("\n");
    assert!(rows.contains("Binary file"), "{rows}");
}

/// Moving a compare tab into a group replaces only a tab comparing the same two texts.
#[test]
fn compare_files_tabs_are_the_same_only_with_the_same_sides() {
    use crate::diff_view::DiffTab;
    use crate::group::Tab;
    let (a, b, c) = (PathBuf::from("/w/a"), PathBuf::from("/w/b"), PathBuf::from("/w/c"));
    let tab = |l: &PathBuf| {
        Tab::Diff(Box::new(DiffTab::compare(
            1,
            b.clone(),
            Source::File(l.clone()),
            Source::File(b.clone()),
        )))
    };
    assert!(tab(&a).same_as(&tab(&a)));
    assert!(!tab(&a).same_as(&tab(&c)), "same right file, other left");
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_dirty_buffer_shows_its_unsaved_text() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    app.press(K::Char('x'), M::NONE);
    assert!(app.active_buffer().unwrap().is_dirty());
    let on_disk = fs::read_to_string(root.join("a.txt")).unwrap();
    compare(&mut app, file(&root, "a.txt"), file(&root, "b.txt")).await;
    let (l, _) = sides(&app);
    assert_eq!(l[0], "xone", "the unsaved text, not the disk's {on_disk:?}");
    assert_eq!(title(&app), "a.txt ● ↔ b.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_untitled_buffers() {
    let (_d, root, _s, mut app) = setup();
    let u1 = untitled(&mut app, "one\nmine");
    compare(&mut app, Source::Open(u1), file(&root, "b.txt")).await;
    assert_eq!(title(&app), "Untitled-1 ● ↔ b.txt");
    let (l, r) = sides(&app);
    assert_eq!(
        (l.as_slice(), r.len()),
        (&["one".to_string(), "mine".to_string()][..], 2)
    );

    let u2 = untitled(&mut app, "one\nyours");
    compare(&mut app, Source::Open(u1), Source::Open(u2)).await;
    assert_eq!(title(&app), "Untitled-1 ● ↔ Untitled-2 ●");
    let (l, r) = sides(&app);
    assert_eq!((l[1].as_str(), r[1].as_str()), ("mine", "yours"));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_follows_edits_in_either_buffer() {
    let (_d, root, _s, mut app) = setup();
    let a = open(&mut app, &root.join("a.txt"));
    let b = open(&mut app, &root.join("b.txt"));
    compare(&mut app, Source::Open(a), Source::Open(b)).await;
    let tab_of = |app: &App, id| app.group().find(id).unwrap();

    // Typing in the right buffer (a Later reload, after the delay).
    let i = tab_of(&app, b);
    app.test_activate_tab(i);
    app.type_text("R");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().tabs.iter().find_map(|t| t.diff()).unwrap();
    assert_eq!(d.doc.as_ref().unwrap().right.line(0), "Rone");
    assert_eq!(d.title(), "a.txt ↔ b.txt ●");

    // And in the left one.
    let i = tab_of(&app, a);
    app.test_activate_tab(i);
    app.type_text("L");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().tabs.iter().find_map(|t| t.diff()).unwrap();
    assert_eq!(d.doc.as_ref().unwrap().left.line(0), "Lone");
    assert_eq!(d.title(), "a.txt ● ↔ b.txt ●");

    // Saving the left one clears its mark and keeps the text.
    app.press(K::Char('s'), cmd());
    assert!(
        app.settle(|a| a.diffs_settled() && !a.buffers.get(a_id(a)).unwrap().save_in_flight())
            .await
    );
    let d = app.group().tabs.iter().find_map(|t| t.diff()).unwrap();
    assert_eq!(d.title(), "a.txt ↔ b.txt ●");
    assert_eq!(d.doc.as_ref().unwrap().left.line(0), "Lone");

    fn a_id(app: &App) -> BufferId {
        app.group().active_tab().unwrap().buffer
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_closed_buffer_freezes_its_side() {
    let (_d, root, _s, mut app) = setup();
    let a = open(&mut app, &root.join("a.txt"));
    let b = open(&mut app, &root.join("b.txt"));
    untitled(&mut app, "");
    compare(&mut app, Source::Open(a), Source::File(root.join("b.txt"))).await;
    let tabs = app.group().tabs.len();
    // Close a.txt's (clean) tab, then b.txt's.
    let i = app.group().find(a).unwrap();
    app.test_activate_tab(i);
    app.press(K::Char('w'), cmd());
    assert!(app.buffers.get(a).is_none());
    assert_eq!(app.group().tabs.len(), tabs - 1, "only the buffer's tab closed");
    let i = app.group().tabs.iter().position(|t| t.diff().is_some()).unwrap();
    app.test_activate_tab(i);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(sides(&app).0, vec!["one", "two"], "frozen to the last text");
    assert_eq!(title(&app), "a.txt ↔ b.txt");

    // The File side whose buffer closes (unsaved edits dropped) reads the disk again, changed
    // there meanwhile, and loses its dirty mark; the frozen side doesn't read a.txt's new text.
    let i = app.group().find(b).unwrap();
    app.test_activate_tab(i);
    app.type_text("x");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().tabs.iter().find_map(|t| t.diff()).unwrap();
    assert_eq!(d.title(), "a.txt ↔ b.txt ●");
    fs::write(root.join("a.txt"), "one\nchanged\n").unwrap();
    fs::write(root.join("b.txt"), "one\nfour\n").unwrap();
    app.press(K::Char('w'), cmd());
    app.press(K::Char('d'), M::NONE);
    assert!(app.buffers.get(b).is_none());
    let i = app.group().tabs.iter().position(|t| t.diff().is_some()).unwrap();
    app.test_activate_tab(i);
    assert!(app.settle(|a| a.diffs_settled() && sides(a).1 == ["one", "four"]).await);
    assert_eq!(
        sides(&app),
        (vec!["one".into(), "two".into()], vec!["one".into(), "four".into()])
    );
    assert_eq!(title(&app), "a.txt ↔ b.txt");
}

/// An untitled buffer closed without saving: its side keeps the text and the tab stays.
#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_closed_untitled_buffer_keeps_its_text() {
    let (_d, root, _s, mut app) = setup();
    let u = untitled(&mut app, "one\nmine");
    compare(&mut app, Source::Open(u), file(&root, "b.txt")).await;
    let i = app.group().find(u).unwrap();
    app.test_activate_tab(i);
    app.press(K::Char('w'), cmd());
    app.press(K::Char('d'), M::NONE);
    assert!(app.buffers.get(u).is_none());
    let i = app
        .group()
        .tabs
        .iter()
        .position(|t| t.diff().is_some())
        .expect("the tab stays");
    app.test_activate_tab(i);
    // b.txt changing on disk loads the tab again: the left side is still the closed text.
    fs::write(root.join("b.txt"), "one\nfive\n").unwrap();
    assert!(app.settle(|a| a.diffs_settled() && sides(a).1 == ["one", "five"]).await);
    assert_eq!(sides(&app).0, vec!["one", "mine"]);
    assert!(title(&app).starts_with("Untitled-1"), "{}", title(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_text_with_itself_opens_nothing() {
    let (_d, root, _s, mut app) = setup();
    let a = open(&mut app, &root.join("a.txt"));
    let u = untitled(&mut app, "x");
    let tabs = app.group().tabs.len();
    for (l, r) in [
        (file(&root, "a.txt"), file(&root, "a.txt")),
        (Source::Open(a), Source::Open(a)),
        (Source::Open(a), file(&root, "a.txt")),
        (file(&root, "a.txt"), Source::Open(a)),
        (Source::Open(u), Source::Open(u)),
    ] {
        app.open_compare(l, r);
        let m = app.message.as_ref().unwrap();
        assert_eq!(
            (m.level, m.text.as_str()),
            (Level::Info, "Cannot compare a file with itself")
        );
        app.message = None;
        assert_eq!(app.group().tabs.len(), tabs);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_draws_without_stage_revert_or_open_file() {
    let (_d, root, _s, mut app) = setup();
    compare(&mut app, file(&root, "a.txt"), file(&root, "b.txt")).await;
    for width in [140, 60] {
        let rows = lines(&mut app, width).join("\n");
        assert!(rows.contains("a.txt"), "{rows}");
        assert!(rows.contains("three") && rows.contains("two"), "{rows}");
        for button in ["Stage", "Revert", "Open File"] {
            assert!(!rows.contains(button), "{button} at {width}: {rows}");
        }
        // Side by side shows both sides' titles over their columns.
        if width == 140 {
            assert!(rows.contains("1 change"), "{rows}");
            let header = rows.lines().find(|l| l.contains("b.txt") && !l.contains('×')).unwrap();
            assert!(header.contains("a.txt") && header.contains("b.txt"), "{header}");
        }
    }
    // Stage and revert refuse.
    app.test_execute(crate::command::Command::StageHunk);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "This diff isn't git's: nothing to stage"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_tabs_are_not_kept_in_sessions() {
    let (_d, root, state, mut app) = setup();
    app.state_dir = Some(state.clone());
    assert!(app.settle(|a| !a.session.loading()).await);
    open(&mut app, &root.join("a.txt"));
    compare(&mut app, file(&root, "a.txt"), file(&root, "b.txt")).await;
    app.test_execute(crate::command::Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;

    let session = crate::session::load(&state, &root).unwrap().expect("written");
    let tabs = &session.groups[0].tabs;
    assert_eq!(tabs.len(), 1, "{tabs:?}");
    assert!(matches!(&tabs[0], crate::session::TabState::Text { path, .. } if path.ends_with("a.txt")));
}

// ---- subtask b: the picker and the two commands ----

/// Runs a palette command by its title (Cmd+Shift+P, the title, Enter).
fn run_command(app: &mut App, title: &str) {
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(K::Enter, M::NONE);
}

fn labels(app: &App) -> Vec<String> {
    app.palette
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.label.clone())
        .collect()
}

/// Types `query` in the open compare picker, waits for a row `label`, and selects it.
async fn pick(app: &mut App, query: &str, label: &str) {
    app.type_text(query);
    let want = label.to_string();
    let found = app
        .settle(|a| {
            a.palette
                .as_ref()
                .is_some_and(|p| p.rows.iter().any(|r| r.label == want))
        })
        .await;
    assert!(found, "lists {label}: {:?}", labels(app));
    let p = app.palette.as_mut().unwrap();
    let i = p.rows.iter().position(|r| r.label == label).unwrap();
    p.select(i);
}

/// Waits for the open compare picker's file walk to finish (its hint stops saying it looks).
async fn walked(app: &mut App) -> bool {
    app.settle(|a| {
        let hint = a.palette.as_ref().and_then(|p| p.hint.as_deref());
        hint.is_some_and(|h| !h.starts_with("Looking for files"))
    })
    .await
}

fn type_at_start(app: &mut App, text: &str) {
    app.press(K::Home, cmd());
    app.type_text(text);
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_active_file_with_an_unsaved_open_buffer() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    type_at_start(&mut app, "X");
    open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    assert_eq!(app.palette.as_ref().unwrap().mode(), crate::palette::Mode::Compare);
    let rows = &app.palette.as_ref().unwrap().rows;
    assert_eq!(rows[0].label, "a.txt", "open buffers first");
    assert!(rows[0].detail.contains("unsaved"), "{}", rows[0].detail);
    assert!(
        !labels(&app).contains(&"b.txt".to_string()),
        "the active buffer isn't offered"
    );
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert!(app.settle(|a| a.diffs_settled()).await);
    let (l, r) = sides(&app);
    assert_eq!(l, strs(&["Xone", "two"]), "the unsaved text, on the left");
    assert_eq!(r, strs(&["one", "three"]));
    let compare = app.group().active_diff().unwrap().compare.clone().unwrap();
    assert!(compare.sides[0].dirty && !compare.sides[1].dirty);
    // From the diff tab there is no active text to compare.
    run_command(&mut app, "File: Compare Active File With…");
    assert!(app.palette.is_none());
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Open a file or an untitled buffer to compare"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_active_file_with_an_untitled_buffer() {
    let (_d, root, _s, mut app) = setup();
    let id = untitled(&mut app, "hello");
    open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    assert_eq!(labels(&app)[0], "Untitled-1");
    pick(&mut app, "Untitled", "Untitled-1").await;
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let compare = app.group().active_diff().unwrap().compare.clone().unwrap();
    assert_eq!(compare.sides[0].source, Source::Open(id));
    let (l, r) = sides(&app);
    assert_eq!(l, strs(&["hello"]));
    assert_eq!(r, strs(&["one", "three"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_active_file_with_a_file_that_is_not_open() {
    let (_d, root, _s, mut app) = setup();
    fs::write(root.join("c.txt"), "cee\n").unwrap();
    let a = open(&mut app, &root.join("a.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    pick(&mut app, "c.txt", "c.txt").await;
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let compare = app.group().active_diff().unwrap().compare.clone().unwrap();
    assert_eq!(compare.sides[0].source, file(&root, "c.txt"));
    assert_eq!(compare.sides[1].source, Source::Open(a));
    let (l, r) = sides(&app);
    assert_eq!((l, r), (strs(&["cee"]), strs(&["one", "two"])));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_open_files_are_listed_once_and_never_the_active_one() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    assert!(walked(&mut app).await);
    assert_eq!(
        labels(&app),
        ["a.txt"],
        "a.txt as an open text only, not again as a file"
    );
    app.press(K::Esc, M::NONE);
    assert!(app.palette.is_none());
    assert!(app.group().active_diff().is_none(), "Esc opens nothing");
    assert_eq!(app.group().tabs.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_lone_buffer_has_nothing_open_to_pick() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    run_command(&mut app, "File: Compare Open Editors…");
    assert!(app.palette.is_none());
    assert_eq!(app.message.as_ref().unwrap().text, "Open two files to compare them");
    assert_eq!(app.group().tabs.len(), 1);
    // Compare Active File With still lists files, never the buffer itself.
    run_command(&mut app, "File: Compare Active File With…");
    assert!(app.palette.is_some());
    assert!(walked(&mut app).await);
    assert_eq!(labels(&app), ["b.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_open_editors_picks_two_in_turn() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    untitled(&mut app, "mine");
    open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Open Editors…");
    assert_eq!(
        labels(&app),
        ["a.txt", "Untitled-1", "b.txt"],
        "open texts only, no files"
    );
    pick(&mut app, "b.t", "b.txt").await;
    app.press(K::Enter, M::NONE);
    // The picker reopens for the second pick, without the first.
    let p = app.palette.as_ref().expect("second pick");
    assert!(p.field.text.is_empty());
    assert!(p.hint.as_deref().unwrap().contains("b.txt"), "{:?}", p.hint);
    assert_eq!(labels(&app), ["a.txt", "Untitled-1"]);
    // The second pick by a click on its row.
    lines(&mut app, 100);
    let hit = app.hits.palette.expect("the palette drawn");
    app.mouse(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        hit.rows.x + 2,
        hit.rows.y + 1,
    );
    assert!(app.palette.is_none());
    assert!(app.settle(|a| a.diffs_settled()).await);
    let (l, r) = sides(&app);
    assert_eq!((l, r), (strs(&["one", "three"]), strs(&["mine"])));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_open_editors_esc_on_the_second_pick_opens_nothing() {
    let (_d, root, _s, mut app) = setup();
    open(&mut app, &root.join("a.txt"));
    open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Open Editors…");
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_some());
    app.press(K::Esc, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(app.group().tabs.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_a_query_matches_open_texts_then_files() {
    let (_d, root, _s, mut app) = setup();
    fs::write(root.join("c.txt"), "cee\n").unwrap();
    fs::write(root.join("notes.md"), "md\n").unwrap();
    let a = open(&mut app, &root.join("a.txt"));
    let b = open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    app.type_text("txt");
    let found = app.settle(|a| labels(a) == ["a.txt", "c.txt"]).await;
    assert!(
        found,
        "the open text, then the file; not b.txt nor notes.md: {:?}",
        labels(&app)
    );
    let rows = &app.palette.as_ref().unwrap().rows;
    assert_eq!(rows[0].action, crate::palette::Action::CompareBuffer(a));
    assert!(matches!(rows[1].action, crate::palette::Action::Open(_)));
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert!(app.settle(|a| a.diffs_settled()).await);
    let compare = app.group().active_diff().unwrap().compare.clone().unwrap();
    assert_eq!(compare.sides[0].source, file(&root, "c.txt"));
    assert_eq!(compare.sides[1].source, Source::Open(b));
    assert!(
        !app.mru.files.iter().any(|f| f.ends_with("c.txt")),
        "a compared file isn't a recently opened one: {:?}",
        app.mru.files
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_files_single_file_mode_lists_open_texts_only() {
    let (_d, root, _s, mut app) = setup();
    fs::write(root.join("c.txt"), "cee\n").unwrap();
    app.mode = crate::app::Mode::SingleFile;
    let a = open(&mut app, &root.join("a.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    assert!(app.palette.is_none(), "nothing else open");
    assert_eq!(app.message.as_ref().unwrap().text, "Open two files to compare them");
    let b = open(&mut app, &root.join("b.txt"));
    run_command(&mut app, "File: Compare Active File With…");
    let p = app.palette.as_ref().expect("the picker");
    assert_eq!(
        p.hint.as_deref(),
        Some("Select a file or editor to compare with b.txt"),
        "no workspace walk"
    );
    assert_eq!(labels(&app), ["a.txt"], "no files listed");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let compare = app.group().active_diff().unwrap().compare.clone().unwrap();
    assert_eq!(compare.sides[0].source, Source::Open(a));
    assert_eq!(compare.sides[1].source, Source::Open(b));
}
