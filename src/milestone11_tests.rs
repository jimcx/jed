//! Milestone 11 end to end (file history). Done when: a history write failure never fails a
//! save (`milestone_11_done_when_a_history_write_failure_never_fails_a_save`: two broken
//! histories, the file saved and the buffer clean each time, a warning, recording again once
//! fixed); restore and then undo gives back the exact buffer
//! (`milestone_11_done_when_restore_then_undo_gives_back_the_exact_buffer`: text with unsaved
//! edits, CRLF and BOM, every selection and the primary, dirty state; a clean buffer comes back
//! clean; typing just before the restore stays its own undo step).
//!
//! 11a, recording: a save through the app snapshots the
//! exact bytes written (BOM and CRLF kept) under `.jed/history/`, once per content (a save of the
//! same text adds nothing), `.jed/.gitignore` keeping it out of git; `.workspace` `[history]`
//! limits (`max_entries` drops the oldest, `max_file_size` skips big files, `enabled = false`
//! records nothing, read live after a `.workspace` reload); a history write failure never fails
//! the save (the file is written, the buffer clean, a warning shown); files outside the
//! workspace get no history; an explorer rename moves the file's history (a failed one leaves
//! it); watcher events under `.jed/` never reach the app's file system handling (no listings,
//! git refreshes or disk checks from history writes).
//!
//! 11b, Show / Compare / Restore: "File History: Show" from the palette lists the active file's
//! entries newest first ("just now · 8 B"), Enter opens a diff tab of the entry against the
//! buffer (drawn, titled with the entry's age); "File History: Restore" on that tab, or its own
//! picker, replaces the buffer's text by the entry's (decoded as loading does: BOM dropped, CRLF
//! kept) as one undo step — cursors in unchanged lines stay, nothing is written, undo gives the
//! exact buffer back; refused for a read-only buffer and a binary entry; a buffer edited while
//! the entry is read gets the entry over its edits (still one step), a closed one nothing, and
//! an entry trimmed after the picker listed it an error; a file without history or outside the
//! workspace gets a message.
//!
//! 11c, entry points: "Explorer: Show File History" (the palette, on the explorer's selection)
//! and a right-click on an explorer file's row list that file's entries though it isn't open
//! (a folder: a message); its Compare shows the entry against the file on disk, then the buffer
//! once Restore has opened the file and put the entry in it (nothing written, undo gives the
//! loaded text); Restore from the picker opens a file that isn't open too (one that opens
//! read-only: refused, nothing changed); a right-click on a
//! tab activates it and lists its file's entries (a diff tab's file too; no history or outside
//! the workspace: a message).

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Level};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::diff_view::Pair;
use crate::git::test_repo::{commit_all, init};
use crate::history;
use crate::palette::Mode;
use crate::selection::{Selection, Selections};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// A temp dir with `proj/` (the workspace) and `state/` (the trust store, outside it).
fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&state).unwrap();
    (dir, root, state)
}

fn open_app(root: &Path, state: &Path) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app
}

/// Opens `path` (read from disk) in a new tab.
fn open(app: &mut App, path: &Path) {
    app.test_open(Buffer::open(path).unwrap());
}

/// Saves the active buffer and waits for the save and every history task to end.
async fn save(app: &mut App) {
    app.press(K::Char('s'), cmd());
    let id = app.group().active_tab().unwrap().buffer;
    assert!(
        app.settle(|a| {
            let b = a.buffers.get(id).unwrap();
            !b.save_in_flight() && a.file_history.running() == 0
        })
        .await,
        "save done"
    );
}

/// The file's history, newest first: each entry's bytes.
fn snapshots(root: &Path, rel: &str) -> Vec<Vec<u8>> {
    let rel = Path::new(rel);
    history::entries(root, rel)
        .unwrap()
        .iter()
        .map(|e| history::read(root, rel, &e.id).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_a_save_records_its_bytes_once_per_content() {
    let (_d, root, state) = dirs();
    init(&root);
    fs::write(root.join("src/a.txt"), b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
    commit_all(&root, "one");
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));

    app.type_text("x");
    save(&mut app).await;
    assert_eq!(app.file_history.done, 1);
    let first = b"\xEF\xBB\xBFxone\r\ntwo\r\n".to_vec();
    assert_eq!(fs::read(root.join("src/a.txt")).unwrap(), first);
    assert_eq!(
        snapshots(&root, "src/a.txt"),
        vec![first.clone()],
        "the exact bytes saved"
    );
    assert_eq!(fs::read_to_string(root.join(".jed/.gitignore")).unwrap(), "*\n");
    let entry = &history::entries(&root, Path::new("src/a.txt")).unwrap()[0];
    assert_eq!(entry.size, first.len() as u64);
    let folder = history::history_dir(&root).join(history::folder_name("src/a.txt"));
    assert!(folder.join("entries.toml").is_file());
    assert!(folder.join(&entry.id).is_file());

    // The same text again: nothing new.
    save(&mut app).await;
    assert_eq!(app.file_history.done, 2);
    assert_eq!(snapshots(&root, "src/a.txt").len(), 1);

    // An edit: newest first.
    app.press(K::Enter, M::NONE);
    save(&mut app).await;
    let all = snapshots(&root, "src/a.txt");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], fs::read(root.join("src/a.txt")).unwrap());
    assert_eq!(all[1], first);

    // Git sees the file's change, never `.jed/`.
    let status = crate::git::test_repo::git(&root, &["status", "--porcelain", "--untracked-files=all"]);
    assert_eq!(status.trim(), "M src/a.txt", "{status}");
    assert!(
        app.message.as_ref().is_some_and(|m| m.level == Level::Info),
        "no warning"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_history_limits_from_the_workspace_file() {
    let (_d, root, state) = dirs();
    fs::write(
        root.join(".workspace"),
        "[history]\nmax_entries = 2\nmax_file_size = 10\n",
    )
    .unwrap();
    fs::write(root.join("src/a.txt"), "").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));

    for c in ["1", "2", "3"] {
        app.type_text(c);
        save(&mut app).await;
    }
    assert_eq!(
        snapshots(&root, "src/a.txt"),
        vec![b"123".to_vec(), b"12".to_vec()],
        "the oldest dropped"
    );
    let folder = history::history_dir(&root).join(history::folder_name("src/a.txt"));
    assert_eq!(
        fs::read_dir(&folder).unwrap().count(),
        3,
        "two snapshots and entries.toml"
    );

    // Over max_file_size: saved, not snapshotted.
    app.type_text("45678901");
    save(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), "12345678901");
    assert_eq!(snapshots(&root, "src/a.txt").len(), 2);
    assert_eq!(snapshots(&root, "src/a.txt")[0], b"123");

    // Turned off in `.workspace` (reloaded live): no history task at all.
    fs::write(root.join(".workspace"), "[history]\nenabled = false\n").unwrap();
    assert!(app.settle(|a| !a.workspace.config.history.enabled).await, "reloaded");
    app.press(K::Backspace, M::NONE);
    let done = app.file_history.done;
    save(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), "1234567890");
    assert_eq!(app.file_history.done, done, "nothing recorded");
    assert_eq!(snapshots(&root, "src/a.txt").len(), 2);
}

/// Done when: a history write failure never fails a save. Two broken histories (a file where
/// `.jed/history/` should be, then a folder where the file's `entries.toml` should be): each
/// save writes the file and leaves the buffer clean (no changed-on-disk conflict), with a
/// warning; once fixed, saves record again.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_done_when_a_history_write_failure_never_fails_a_save() {
    let (_d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    // A file where the history folder should be.
    fs::create_dir(root.join(".jed")).unwrap();
    fs::write(root.join(".jed/history"), "not a folder").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));

    let saved_with_a_warning = |app: &App, text: &str| {
        assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), text, "saved");
        let buf = app.active_buffer().unwrap();
        assert!(!buf.is_dirty() && buf.disk_conflict.is_none() && !buf.deleted);
        let message = app.message.as_ref().expect("a message");
        assert_eq!(message.level, Level::Warning);
        assert!(
            message.text.starts_with("File history of src/a.txt not saved"),
            "{}",
            message.text
        );
    };
    app.type_text("b");
    save(&mut app).await;
    assert_eq!(app.file_history.done, 1);
    saved_with_a_warning(&app, "ba\n");

    // Fixed: the next save records again.
    fs::remove_file(root.join(".jed/history")).unwrap();
    app.type_text("c");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/a.txt"), vec![b"bca\n".to_vec()]);

    // A folder where the file's entries list should be.
    let entries = history::history_dir(&root)
        .join(history::folder_name("src/a.txt"))
        .join("entries.toml");
    fs::remove_file(&entries).unwrap();
    fs::create_dir(&entries).unwrap();
    app.type_text("d");
    save(&mut app).await;
    saved_with_a_warning(&app, "bcda\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_files_outside_the_workspace_have_no_history() {
    let (d, root, state) = dirs();
    let outside = workspace::normalize(d.path()).join("outside.txt");
    fs::write(&outside, "o\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &outside);
    app.type_text("x");
    save(&mut app).await;
    assert_eq!(fs::read_to_string(&outside).unwrap(), "xo\n");
    assert_eq!(app.file_history.done, 0);
    assert!(!root.join(".jed").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_an_explorer_rename_moves_the_history() {
    let (_d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));
    app.type_text("1");
    save(&mut app).await;
    app.type_text("2");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/a.txt").len(), 2);

    // F2 in the explorer on the file.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    let a = root.join("src/a.txt");
    let src = root.join("src");
    assert!(app.settle(|app| app.explorer.find(&src).is_some()).await, "root listed");
    app.explorer.selected = Some(src);
    app.press(K::Right, M::NONE);
    assert!(app.settle(|app| app.explorer.find(&a).is_some()).await, "src listed");
    let rename_to = |app: &mut App, name: &str| {
        app.explorer.selected = Some(a.clone());
        app.press(K::F(2), M::NONE);
        for _ in 0.."a.txt".len() {
            app.press(K::Backspace, M::NONE);
        }
        app.type_text(name);
        app.press(K::Enter, M::NONE);
    };

    // A rename that fails (the name is taken) leaves the history where it is.
    fs::write(root.join("src/c.txt"), "c\n").unwrap();
    rename_to(&mut app, "c.txt");
    assert!(
        app.settle(|a| a.file_history.done == 3 && a.file_history.running() == 0)
            .await
    );
    assert!(app.message.as_ref().is_some_and(|m| m.level == Level::Error));
    assert!(root.join("src/a.txt").is_file());
    assert_eq!(snapshots(&root, "src/a.txt").len(), 2);
    assert!(snapshots(&root, "src/c.txt").is_empty());

    rename_to(&mut app, "b.txt");
    assert!(
        app.settle(|a| a.file_history.done == 4 && a.file_history.running() == 0)
            .await
    );
    assert!(root.join("src/b.txt").is_file());
    assert!(app.message.as_ref().is_none_or(|m| m.level != Level::Warning));
    assert!(snapshots(&root, "src/a.txt").is_empty());
    assert_eq!(snapshots(&root, "src/b.txt"), vec![b"12a\n".to_vec(), b"1a\n".to_vec()]);

    // Saving under the new name adds to the moved history.
    app.press(K::Char('1'), cmd());
    app.type_text("3");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/b.txt").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_watcher_events_under_jed_are_ignored() {
    let (_d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));
    app.type_text("x");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/a.txt").len(), 1);

    // Writes under `.jed/`, then one elsewhere: by the time that one is handled, the earlier
    // ones have arrived too (one watcher, one channel).
    fs::write(history::history_dir(&root).join("junk"), "j").unwrap();
    fs::write(root.join(".jed/other"), "o").unwrap();
    let sentinel = root.join("sentinel.txt");
    fs::write(&sentinel, "s").unwrap();
    assert!(
        app.settle(|a| a.fs_flushed.contains(&sentinel)).await,
        "the watcher works"
    );
    let jed = root.join(".jed");
    let under_jed: Vec<_> = app.fs_flushed.iter().filter(|p| p.starts_with(&jed)).collect();
    assert!(under_jed.is_empty(), "{under_jed:?}");
}

// 11b: Show, Compare, Restore.

/// Runs a palette command by its title, through the keys.
fn run_command(app: &mut App, title: &str) {
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(K::Enter, M::NONE);
}

/// Runs "File History: Show" / "Restore" and waits for its picker; returns its row labels.
async fn open_history(app: &mut App, title: &str) -> Vec<String> {
    run_command(app, title);
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await,
        "the history picker"
    );
    app.palette
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.label.clone())
        .collect()
}

/// Waits until no restore is reading its entry.
async fn settle_restore(app: &mut App) {
    assert!(app.settle(|a| a.file_history.restoring == 0).await, "restore done");
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// `src/a.txt` saved as "one\ntwo\n" (8 B) then "one\nTWO\nthree\n" (14 B): two entries.
async fn two_entries(app: &mut App, root: &Path) {
    fs::write(root.join("src/a.txt"), "").unwrap();
    open(app, &root.join("src/a.txt"));
    app.type_text("one\ntwo\n");
    save(app).await;
    std::thread::sleep(std::time::Duration::from_millis(2));
    app.press(K::Up, M::NONE);
    app.press(K::Delete, M::NONE);
    app.press(K::Delete, M::NONE);
    app.press(K::Delete, M::NONE);
    app.type_text("TWO\nthree");
    save(app).await;
    assert_eq!(text(app), "one\nTWO\nthree\n");
    assert_eq!(snapshots(root, "src/a.txt").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_show_lists_the_entries_and_compares_one_in_a_diff_tab() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;
    app.type_text("!"); // unsaved: the diff shows the buffer, not the file

    let rows = open_history(&mut app, "File History: Show").await;
    assert_eq!(rows, ["just now · 14 B", "just now · 8 B"], "newest first");
    let p = app.palette.as_ref().unwrap();
    assert!(p.rows[0].detail.ends_with(" UTC"), "{}", p.rows[0].detail);
    assert!(p.hint.as_ref().unwrap().contains("src/a.txt"));
    // Typing filters.
    app.type_text("8 B");
    assert_eq!(app.palette.as_ref().unwrap().rows.len(), 1);
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());

    let older = history::entries(&root, Path::new("src/a.txt")).unwrap()[1].snapshot();
    let d = app.group().active_diff().expect("a diff tab");
    assert_eq!(d.pair, Pair::history(older));
    assert_eq!(d.title(), "a.txt (History: just now)");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let doc = app.group().active_diff().unwrap().doc.clone().unwrap();
    assert_eq!((doc.left.line(1), doc.right.line(1)), ("two", "TWO"));
    assert_eq!(doc.right.line(2), "three!");
    assert_eq!(doc.hunks.len(), 1);

    let mut term = Terminal::new(TestBackend::new(150, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let screen = lines(&term).join("\n");
    assert!(screen.contains("a.txt (History: just now)"), "the tab:\n{screen}");
    assert!(screen.contains(" History: just now"), "the header");
    assert!(screen.contains(" Yours"), "the header");
    assert!(screen.contains("three!"), "the buffer's side");

    // The same entry again: the same tab.
    open_history(&mut app, "File History: Show").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert_eq!(app.group().tabs.len(), 2);
    // The newest: another tab.
    open_history(&mut app, "File History: Show").await;
    app.press(K::Enter, M::NONE);
    assert_eq!(app.group().tabs.len(), 3);
    // Esc closes the picker without doing anything.
    open_history(&mut app, "File History: Show").await;
    app.press(K::Esc, M::NONE);
    assert!(app.palette.is_none() && app.file_history.picker.is_none());
    assert_eq!(app.group().tabs.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_is_one_undo_step_and_writes_nothing() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;
    app.type_text("!");
    // The cursor on the first line, which the restore doesn't change.
    let tab = app.group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::single(Selection::cursor(ByteIdx(2)));
    let before = text(&app);

    let rows = open_history(&mut app, "File History: Restore").await;
    assert_eq!(rows.len(), 2);
    assert!(
        app.palette
            .as_ref()
            .unwrap()
            .hint
            .as_ref()
            .unwrap()
            .contains("restores")
    );
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "one\ntwo\n", "the older entry");
    let message = app.message.as_ref().unwrap();
    assert_eq!(message.text, "Restored a.txt as it was just now (not saved)");
    assert!(app.active_buffer().unwrap().is_dirty());
    assert_eq!(
        fs::read_to_string(root.join("src/a.txt")).unwrap(),
        "one\nTWO\nthree\n",
        "nothing written"
    );
    let head = app.group().active_tab().unwrap().view.selections.primary().head;
    assert_eq!(head, ByteIdx(2), "a cursor in an unchanged line stays");

    // One undo step gives the exact buffer back; redo restores again.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), before);
    app.press(K::Char('z'), cmd() | M::SHIFT);
    assert_eq!(text(&app), "one\ntwo\n");

    // Restoring what the buffer already holds: a message, no edit.
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert!(app.message.as_ref().unwrap().text.contains("already reads as it did"));
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), before, "still one step");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_from_the_compare_tab() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;
    open_history(&mut app, "File History: Show").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(app.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len(), 1);

    run_command(&mut app, "File History: Restore");
    assert!(app.palette.is_none(), "no picker: the tab's entry");
    settle_restore(&mut app).await;
    let buffer = app.buffers.find_path(&root.join("src/a.txt")).unwrap();
    assert_eq!(app.buffers.get(buffer).unwrap().rope.to_string(), "one\ntwo\n");
    assert!(app.group().active_diff().is_some(), "the diff tab stays");
    // The diff follows the buffer: nothing left to show.
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.is_empty())
            .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_decodes_the_entry_as_loading_does() {
    let (_d, root, state) = dirs();
    let bytes = b"\xEF\xBB\xBFone\r\ntwo\r\n";
    fs::write(root.join("src/a.txt"), bytes).unwrap();
    history::record(&root, Path::new("src/a.txt"), bytes, &Default::default()).unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));
    let loaded = text(&app);
    app.type_text("xyz");

    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert_eq!(text(&app), loaded, "the text as if loaded: BOM dropped, CRLF kept");
    assert!(app.active_buffer().unwrap().bom);
    // Saving now writes the entry's bytes.
    save(&mut app).await;
    assert_eq!(fs::read(root.join("src/a.txt")).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_refused_for_read_only_buffers_and_binary_entries() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;
    let buffer = app.group().active_tab().unwrap().buffer;

    // Read-only: refused before the picker.
    app.buffers.get_mut(buffer).unwrap().read_only = Some("not valid UTF-8".into());
    run_command(&mut app, "File History: Restore");
    let message = app.message.as_ref().unwrap();
    assert_eq!(
        (message.level, message.text.as_str()),
        (Level::Error, "Read-only: not valid UTF-8")
    );
    assert!(app.palette.is_none());
    // Read-only from the compare tab too.
    open_history(&mut app, "File History: Show").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.group().active_diff().is_some());
    run_command(&mut app, "File History: Restore");
    assert_eq!(app.message.as_ref().unwrap().text, "Read-only: not valid UTF-8");
    assert_eq!(app.file_history.restoring, 0);
    assert_eq!(app.buffers.get(buffer).unwrap().rope.to_string(), "one\nTWO\nthree\n");

    // Made read-only while the entry is read: refused when it arrives.
    app.test_execute(crate::command::Command::CloseTab);
    app.buffers.get_mut(buffer).unwrap().read_only = None;
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    app.buffers.get_mut(buffer).unwrap().read_only = Some("locked".into());
    settle_restore(&mut app).await;
    assert_eq!(app.message.as_ref().unwrap().text, "Read-only: locked");
    assert_eq!(text(&app), "one\nTWO\nthree\n");

    // A binary entry: an error, the buffer untouched.
    app.buffers.get_mut(buffer).unwrap().read_only = None;
    history::record(&root, Path::new("src/a.txt"), b"bin\0ary", &Default::default()).unwrap();
    let rows = open_history(&mut app, "File History: Restore").await;
    assert_eq!(rows[0], "just now · 7 B");
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    let message = app.message.as_ref().unwrap();
    assert_eq!(
        (message.level, message.text.as_str()),
        (Level::Error, "Can't restore a.txt: the entry is a binary file")
    );
    assert_eq!(text(&app), "one\nTWO\nthree\n");
    assert!(!app.active_buffer().unwrap().is_dirty());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_no_history_or_outside_the_workspace_says_so() {
    let (d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));
    run_command(&mut app, "File History: Show");
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text.contains("no file history yet")))
            .await
    );
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "src/a.txt has no file history yet: a snapshot is recorded each time you save"
    );
    assert!(app.palette.is_none());

    let outside = workspace::normalize(d.path()).join("outside.txt");
    fs::write(&outside, "o\n").unwrap();
    open(&mut app, &outside);
    run_command(&mut app, "File History: Show");
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "outside.txt has no file history: it's outside the workspace"
    );
    run_command(&mut app, "File History: Restore");
    assert!(app.message.as_ref().unwrap().text.contains("outside the workspace"));
    assert!(app.palette.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_over_edits_made_while_it_reads() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;

    open_history(&mut app, "File History: Restore").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    // Typed before the entry's diff arrives: it was taken against the older text.
    app.type_text("zz");
    let edited = text(&app);
    assert_eq!(edited, "one\nTWO\nthreezz\n");
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "one\ntwo\n", "the entry, over the edit too");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), edited, "one undo step back to the edited text");

    // A cursor in an unchanged line after a changed one keeps its place in that line (not its
    // byte offset, as a whole-text replacement would).
    let tab = app.group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::single(Selection::cursor(ByteIdx(0)));
    app.type_text("XX");
    assert_eq!(text(&app), "XXone\nTWO\nthreezz\n");
    let tab = app.group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::single(Selection::cursor(ByteIdx(6))); // "XXone\n|TWO"
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE); // the newest: "one\nTWO\nthree\n"
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "one\nTWO\nthree\n");
    let head = app.group().active_tab().unwrap().view.selections.primary().head;
    assert_eq!(head, ByteIdx(4), "still at the start of TWO");
    // Two changes (lines 1 and 3), still one undo step.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "XXone\nTWO\nthreezz\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_of_a_closed_buffer_or_a_trimmed_entry() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state);
    two_entries(&mut app, &root).await;
    let path = root.join("src/a.txt");

    // The tab closed while the entry is read: nothing happens (and nothing panics).
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    app.test_execute(crate::command::Command::CloseTab);
    assert!(app.buffers.find_path(&path).is_none());
    settle_restore(&mut app).await;
    assert!(app.buffers.find_path(&path).is_none());
    assert!(
        !app.message.as_ref().is_some_and(|m| m.text.starts_with("Restored")),
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\nthree\n");

    // The picked entry trimmed from the history after the picker listed it: an error.
    open(&mut app, &path);
    open_history(&mut app, "File History: Restore").await;
    let limits = crate::workspace::config::HistoryConfig {
        max_entries: 2,
        ..Default::default()
    };
    history::record(&root, Path::new("src/a.txt"), b"third\n", &limits).unwrap();
    assert_eq!(snapshots(&root, "src/a.txt").len(), 2);
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    let message = app.message.as_ref().unwrap();
    assert_eq!(message.level, Level::Error);
    assert!(message.text.starts_with("Can't restore a.txt:"), "{}", message.text);
    assert!(message.text.contains("gone"), "{}", message.text);
    assert_eq!(text(&app), "one\nTWO\nthree\n");
    assert!(!app.active_buffer().unwrap().is_dirty());
}

/// Done when: restore and then undo gives back the exact buffer — its text with unsaved edits
/// (CRLF and BOM kept), every selection, and its dirty state; redo restores again. A clean
/// buffer comes back clean.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_done_when_restore_then_undo_gives_back_the_exact_buffer() {
    let (_d, root, state) = dirs();
    let saved = b"\xEF\xBB\xBFfn a() {}\r\nfn b() {}\r\nfn c() {}\r\n";
    fs::write(root.join("src/a.txt"), saved).unwrap();
    history::record(&root, Path::new("src/a.txt"), b"old\r\ntext\r\n", &Default::default()).unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));

    // Clean: restore, then undo, is clean again.
    let clean = text(&app);
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "old\r\ntext\r\n");
    assert!(app.active_buffer().unwrap().is_dirty());
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), clean);
    assert!(!app.active_buffer().unwrap().is_dirty(), "clean again");

    // Unsaved edits, several cursors and a selection.
    app.type_text("// ");
    app.press(K::Esc, M::NONE); // a non-edit action: seals the typing's undo step
    let tab = app.group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::new(
        vec![
            Selection::cursor(ByteIdx(3)),
            Selection::new(ByteIdx(14), ByteIdx(18)),
            Selection::cursor(ByteIdx(27)),
        ],
        1,
    );
    let before_text = text(&app);
    let before_selections = app.group().active_tab().unwrap().view.selections.clone();
    assert!(before_text.starts_with("// fn a()"));
    assert!(app.active_buffer().unwrap().is_dirty());

    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "old\r\ntext\r\n");
    assert_eq!(fs::read(root.join("src/a.txt")).unwrap(), saved, "nothing written");

    app.press(K::Char('z'), cmd());
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.rope.to_string(), before_text, "the exact text");
    assert!(buf.bom && buf.is_dirty());
    let view = &app.group().active_tab().unwrap().view;
    assert_eq!(
        view.selections.as_slice(),
        before_selections.as_slice(),
        "every selection"
    );
    assert_eq!(view.selections.primary_index(), before_selections.primary_index());
    // Redo restores again; undo once more is the same buffer.
    app.press(K::Char('z'), cmd() | M::SHIFT);
    assert_eq!(text(&app), "old\r\ntext\r\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), before_text);
    // The undo before it is the typing's: back to the file as saved.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), clean);
    assert!(!app.active_buffer().unwrap().is_dirty());

    // Typing right before the restore (nothing sealing it in between): the restore is still
    // its own step, so one undo gives back the typing.
    app.type_text("x");
    let typed = text(&app);
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE);
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "old\r\ntext\r\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), typed, "the typing kept");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), clean);
}

// 11c: the explorer and the tab bar.

/// `src/a.txt` on disk as "disk\n" with two history entries, "one\n" then "two\n", not open.
fn history_not_open(root: &Path) -> PathBuf {
    let rel = Path::new("src/a.txt");
    history::record(root, rel, b"one\n", &Default::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    history::record(root, rel, b"two\n", &Default::default()).unwrap();
    let path = root.join(rel);
    fs::write(&path, "disk\n").unwrap();
    path
}

/// Shows the explorer with `src/` expanded.
async fn explorer_with_src(app: &mut App, root: &Path, file: &Path) {
    app.press(K::Char('e'), cmd() | M::SHIFT);
    let src = root.join("src");
    assert!(app.settle(|app| app.explorer.find(&src).is_some()).await, "root listed");
    app.explorer.selected = Some(src);
    app.press(K::Right, M::NONE);
    assert!(app.settle(|app| app.explorer.find(file).is_some()).await, "src listed");
}

fn draw(app: &mut App) {
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// Right-clicks the explorer's row of `path` (drawn first).
fn right_click_row(app: &mut App, path: &Path) {
    draw(app);
    let index = app.explorer.rows().iter().position(|r| r.entry.path == path).unwrap();
    let rows = app.hits.explorer_rows;
    let y = rows.y + (index - app.explorer.scroll) as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Right), rows.x + 2, y);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_explorer_shows_the_history_of_a_file_not_open() {
    let (_d, root, state) = dirs();
    let path = history_not_open(&root);
    let mut app = open_app(&root, &state);
    explorer_with_src(&mut app, &root, &path).await;

    // A right-click on its row: the picker (the row selected, the explorer keeps the keys).
    right_click_row(&mut app, &path);
    assert_eq!(app.explorer.selected.as_deref(), Some(path.as_path()));
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await
    );
    assert!(
        app.palette
            .as_ref()
            .unwrap()
            .hint
            .as_ref()
            .unwrap()
            .contains("src/a.txt")
    );
    app.press(K::Esc, M::NONE);

    // From the palette (the explorer's selection), then Compare: the entry against the disk.
    let rows = open_history(&mut app, "Explorer: Show File History").await;
    assert_eq!(rows, ["just now · 4 B", "just now · 4 B"]);
    app.press(K::Enter, M::NONE);
    assert!(app.buffers.find_path(&path).is_none(), "not opened");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let doc = app.group().active_diff().unwrap().doc.clone().unwrap();
    assert_eq!((doc.left.line(0), doc.right.line(0)), ("two", "disk"));

    // Restore on that tab: the file opens, then gets the entry (not written).
    run_command(&mut app, "File History: Restore");
    assert!(
        app.settle(|a| a
            .buffers
            .find_path(&path)
            .is_some_and(|b| a.buffers.get(b).unwrap().rope == "two\n"))
            .await,
        "opened and restored"
    );
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "two\n", "the file's tab is active");
    assert!(app.active_buffer().unwrap().is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "disk\n", "nothing written");
    assert!(app.message.as_ref().unwrap().text.starts_with("Restored a.txt"));
    // The compare tab now reads the buffer: nothing left to show.
    let diff = app.group().tabs.iter().position(|t| t.diff().is_some()).unwrap();
    app.test_execute(crate::command::Command::PrevTab);
    assert_eq!(app.group().active, diff);
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.is_empty())
            .await
    );
    // Undo gives back the file as it was loaded.
    app.test_execute(crate::command::Command::NextTab);
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "disk\n");
    assert!(!app.active_buffer().unwrap().is_dirty());

    // A folder's right-click is "Find in Folder…" (no picker); the command on it: a message.
    right_click_row(&mut app, &root.join("src"));
    assert!(app.palette.is_none());
    assert_eq!(app.search_view.include.text, "./src");
    app.explorer.selected = Some(root.join("src"));
    run_command(&mut app, "Explorer: Show File History");
    assert!(app.palette.is_none());
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "src is a folder: file history is kept per file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_restore_from_the_picker_opens_a_file_not_open() {
    let (_d, root, state) = dirs();
    let path = history_not_open(&root);
    fs::write(root.join("src/b.txt"), "b\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/b.txt"));
    // A diff tab of a.txt that isn't a history one (the banner's pair): Restore lists entries.
    app.open_diff(path.clone(), Pair::SAVED);
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(
        app.settle(|a| a
            .buffers
            .find_path(&path)
            .is_some_and(|b| a.buffers.get(b).unwrap().rope == "one\n"))
            .await,
        "opened and restored"
    );
    settle_restore(&mut app).await;
    assert_eq!(text(&app), "one\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), "disk\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "disk\n");

    // A file that opens read-only (not UTF-8): opened, the restore refused.
    let latin = root.join("src/c.txt");
    history::record(&root, Path::new("src/c.txt"), b"entry\n", &Default::default()).unwrap();
    fs::write(&latin, b"caf\xE9\n").unwrap();
    app.open_diff(latin.clone(), Pair::SAVED);
    open_history(&mut app, "File History: Restore").await;
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.buffers.find_path(&latin).is_some()).await, "opened");
    settle_restore(&mut app).await;
    let message = app.message.as_ref().unwrap();
    assert_eq!(message.level, Level::Error);
    assert!(message.text.starts_with("Read-only:"), "{}", message.text);
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.path, latin);
    assert!(!buf.is_dirty() && !buf.rope.to_string().contains("entry"));
    assert_eq!(fs::read(&latin).unwrap(), b"caf\xE9\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_right_click_on_a_tab_shows_its_files_history() {
    let (d, root, state) = dirs();
    let path = history_not_open(&root);
    fs::write(root.join("src/b.txt"), "b\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &path);
    open(&mut app, &root.join("src/b.txt"));
    assert_eq!(app.group().active, 1);

    let right_click_tab = |app: &mut App, i: usize| {
        draw(app);
        let t = app.hits.groups[0].tabs.iter().find(|t| t.index == i).copied().unwrap();
        app.mouse(
            MouseEventKind::Down(MouseButton::Right),
            t.x0 + 1,
            app.hits.groups[0].tab_bar.y,
        );
    };
    // The other tab: it's activated, and its file's entries listed.
    right_click_tab(&mut app, 0);
    assert_eq!(app.group().active, 0);
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await
    );
    assert!(
        app.palette
            .as_ref()
            .unwrap()
            .hint
            .as_ref()
            .unwrap()
            .contains("src/a.txt")
    );
    app.press(K::Enter, M::NONE);
    let d0 = app.group().active_diff().expect("the compare tab");
    assert_eq!(d0.path, path);
    let compare = app.group().active;

    // On the compare tab: its file's history again.
    right_click_tab(&mut app, compare);
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await
    );
    app.press(K::Esc, M::NONE);

    // A file without history says so.
    let b = app.buffers.find_path(&root.join("src/b.txt")).unwrap();
    let b = app.group().find(b).unwrap();
    right_click_tab(&mut app, b);
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text.contains("no file history yet")))
            .await
    );
    assert_eq!(app.group().active, b);

    // Outside the workspace too.
    let outside = workspace::normalize(d.path()).join("outside.txt");
    fs::write(&outside, "o\n").unwrap();
    open(&mut app, &outside);
    let last = app.group().tabs.len() - 1;
    right_click_tab(&mut app, last);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "outside.txt has no file history: it's outside the workspace"
    );
    assert!(app.palette.is_none());
}
