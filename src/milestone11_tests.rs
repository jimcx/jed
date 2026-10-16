//! Milestone 11 end to end (file history). 11a, recording: a save through the app snapshots the
//! exact bytes written (BOM and CRLF kept) under `.red/history/`, once per content (a save of the
//! same text adds nothing), `.red/.gitignore` keeping it out of git; `.workspace` `[history]`
//! limits (`max_entries` drops the oldest, `max_file_size` skips big files, `enabled = false`
//! records nothing, read live after a `.workspace` reload); a history write failure never fails
//! the save (the file is written, the buffer clean, a warning shown); files outside the
//! workspace get no history; an explorer rename moves the file's history (a failed one leaves
//! it); watcher events under `.red/` never reach the app's file system handling (no listings,
//! git refreshes or disk checks from history writes).

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::app::{App, Level};
use crate::buffer::Buffer;
use crate::git::test_repo::{commit_all, init};
use crate::history;
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
    assert_eq!(fs::read_to_string(root.join(".red/.gitignore")).unwrap(), "*\n");
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

    // Git sees the file's change, never `.red/`.
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

#[tokio::test(flavor = "multi_thread")]
async fn milestone_11_a_history_failure_never_fails_the_save() {
    let (_d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    // A file where the history folder should be.
    fs::create_dir(root.join(".red")).unwrap();
    fs::write(root.join(".red/history"), "not a folder").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));

    app.type_text("b");
    save(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("src/a.txt")).unwrap(), "ba\n", "saved");
    assert!(!app.active_buffer().unwrap().is_dirty());
    assert_eq!(app.file_history.done, 1);
    let message = app.message.as_ref().expect("a message");
    assert_eq!(message.level, Level::Warning);
    assert!(
        message.text.starts_with("File history of src/a.txt not saved"),
        "{}",
        message.text
    );

    // Fixed: the next save records again.
    fs::remove_file(root.join(".red/history")).unwrap();
    app.type_text("c");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/a.txt"), vec![b"bca\n".to_vec()]);
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
    assert!(!root.join(".red").exists());
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
async fn milestone_11_watcher_events_under_red_are_ignored() {
    let (_d, root, state) = dirs();
    fs::write(root.join("src/a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, &state);
    open(&mut app, &root.join("src/a.txt"));
    app.type_text("x");
    save(&mut app).await;
    assert_eq!(snapshots(&root, "src/a.txt").len(), 1);

    // Writes under `.red/`, then one elsewhere: by the time that one is handled, the earlier
    // ones have arrived too (one watcher, one channel).
    fs::write(history::history_dir(&root).join("junk"), "j").unwrap();
    fs::write(root.join(".red/other"), "o").unwrap();
    let sentinel = root.join("sentinel.txt");
    fs::write(&sentinel, "s").unwrap();
    assert!(
        app.settle(|a| a.fs_flushed.contains(&sentinel)).await,
        "the watcher works"
    );
    let red = root.join(".red");
    let under_red: Vec<_> = app.fs_flushed.iter().filter(|p| p.starts_with(&red)).collect();
    assert!(under_red.is_empty(), "{under_red:?}");
}
