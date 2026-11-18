//! hist_versions-a: "File History: Open" opens a history entry's text in a read-only version tab
//! (`Buffer::version`), titled `a.rs (History: just now)` and highlighted by the file's language,
//! from the palette picker and from the Explorer section's Space; copy, select all and splits
//! work, nothing keyed by a path takes it (the file's own buffer stays the file's), a gone entry
//! reports and opens nothing, and sessions bring the tab back from its snapshot (dropped when
//! the entry is gone).

use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::app::App;
use crate::buffer::BufferId;
use crate::command::{Command, FileHistoryCommand};
use crate::history::{self, Snapshot};
use crate::milestone10_tests::{dirs, open_file_app};
use crate::palette::Mode;
use crate::syntax::Language;
use crate::version::VersionSource;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// An app on `a.rs` with two history entries ("xfn a() {}\n" older, "xyyfn a() {}\n" newest).
async fn app() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf, App) {
    let (d, root, state) = dirs();
    fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.rs");
    app.type_text("x");
    save(&mut app).await;
    std::thread::sleep(std::time::Duration::from_millis(2));
    app.type_text("yy");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 2).await);
    (d, root, state, app)
}

async fn save(app: &mut App) {
    app.test_execute(Command::Save);
    let id = app.group().active_tab().unwrap().buffer;
    assert!(
        app.settle(|a| !a.buffers.get(id).unwrap().save_in_flight() && a.file_history.running() == 0)
            .await,
        "save done"
    );
}

fn versions(app: &App) -> Vec<BufferId> {
    app.buffers
        .iter()
        .filter(|(_, b)| b.version.is_some())
        .map(|(id, _)| id)
        .collect()
}

async fn version_open(app: &mut App, n: usize) -> BufferId {
    assert!(app.settle(|a| versions(a).len() == n).await, "a version buffer opens");
    *versions(app).last().unwrap()
}

fn entry(app: &App, row: usize) -> Snapshot {
    app.file_history_view.entries[row].snapshot()
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_open_from_the_palette_makes_a_read_only_tab() {
    let (_d, root, _state, mut app) = app().await;
    let older = entry(&app, 1);
    app.test_execute(Command::OpenFileHistory);
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await
    );
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    let id = version_open(&mut app, 1).await;
    assert_eq!(app.group().active_tab().unwrap().buffer, id, "the new tab is active");
    assert_eq!(app.group().tabs.len(), 2);

    let buf = app.buffers.get(id).unwrap();
    let bytes = history::read_snapshot(&root, Path::new("a.rs"), older).unwrap();
    assert_eq!(buf.rope.to_string().as_bytes(), bytes.as_slice());
    assert_eq!(buf.rope.to_string(), "xfn a() {}\n");
    assert_eq!(buf.name(), "a.rs (History: just now)");
    assert!(!buf.has_file() && buf.read_only.is_some() && !buf.is_dirty());
    assert_eq!(
        buf.syntax.language(),
        Language::Rust,
        "highlighted by the file's language"
    );

    // The file's own buffer is still the file's.
    let file = app.buffers.find_path(&root.join("a.rs")).unwrap();
    assert_ne!(file, id);
    assert_eq!(app.buffers.get(file).unwrap().rope.to_string(), "xyyfn a() {}\n");

    // A version tab counts as its file for the File History section.
    assert!(
        app.settle(|a| a.file_history_view.path.as_deref() == Some(root.join("a.rs").as_path()))
            .await,
        "the section follows the version's file"
    );
    assert!(app.buffers.find_path(&root.join("a.rs")).is_some_and(|f| f != id));

    // Typing is refused; copy, select all and splits work.
    app.type_text("q");
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "xfn a() {}\n");
    app.test_execute(Command::SelectAll);
    app.test_execute(Command::Copy);
    assert_eq!(app.clipboard.read().unwrap(), "xfn a() {}\n");
    app.test_execute(Command::SplitRight);
    assert_eq!(app.groups.ids().len(), 2);
    assert_eq!(app.active_buffer().unwrap().name(), "a.rs (History: just now)");

    // Saving the file leaves the version alone.
    app.group_mut().active = 0;
    app.type_text("z");
    save(&mut app).await;
    assert_eq!(versions(&app), [id]);
    assert_eq!(app.buffers.get(id).unwrap().rope.to_string(), "xfn a() {}\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_space_in_the_section_opens_the_entry_and_reopening_shows_it() {
    let (_d, _root, _state, mut app) = app().await;
    app.test_execute(Command::ShowFileHistory);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    let id = version_open(&mut app, 1).await;
    assert_eq!(app.buffers.get(id).unwrap().rope.to_string(), "xyyfn a() {}\n");

    // The same entry again: the same tab, activated.
    app.group_mut().active = 0;
    assert!(
        app.settle(|a| a.file_history_view.entries.len() == 2 && !a.file_history_view.loading)
            .await
    );
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    assert!(app.settle(|a| a.file_history.restoring == 0).await);
    assert_eq!(versions(&app), [id]);
    assert_eq!(app.group().active_tab().unwrap().buffer, id);
    assert_eq!(app.group().tabs.len(), 2);

    // The other entry: another tab.
    app.group_mut().active = 0;
    assert!(
        app.settle(|a| a.file_history_view.entries.len() == 2 && !a.file_history_view.loading)
            .await
    );
    app.file_history_view.select_row(1);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    version_open(&mut app, 2).await;
    assert_eq!(app.group().tabs.len(), 3);
}

#[test]
fn hist_versions_space_is_bound_in_the_section() {
    let keymap = crate::config::keymap::Keymap::default_for(crate::config::keymap::Platform::Linux);
    let key = crossterm::event::KeyEvent::new(K::Char(' '), M::NONE);
    assert_eq!(
        keymap.lookup(&key, crate::config::keymap::Context::FileHistory),
        Some(Command::FileHistory(FileHistoryCommand::OpenVersion))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_a_gone_entry_reports_and_opens_nothing() {
    let (_d, root, _state, mut app) = app().await;
    let bogus = Snapshot { time: 1, hash: 2 };
    app.open_version(root.join("a.rs"), VersionSource::Snapshot(bogus));
    assert!(app.settle(|a| a.file_history.restoring == 0).await);
    assert!(versions(&app).is_empty());
    assert_eq!(app.group().tabs.len(), 1);
    let message = app.message.as_ref().expect("a message").text.clone();
    assert!(message.contains("Can't open that version of a.rs"), "{message}");
}

/// A new app on `root` with the saved session read.
async fn reopened(root: &Path, state: &Path) -> App {
    let mut app = App::new(
        Workspace::load(root.to_path_buf()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.state_dir = Some(state.to_path_buf());
    app.test_start();
    assert!(app.settle(|a| !a.session.loading()).await, "the session is read");
    app
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_sessions_keep_the_version_tab_and_drop_it_when_gone() {
    let (_d, root, state, mut app) = app().await;
    app.state_dir = Some(state.clone());
    assert!(app.settle(|a| !a.session.loading()).await);
    app.test_execute(Command::ShowFileHistory);
    app.press(K::Down, M::NONE);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    version_open(&mut app, 1).await;
    app.test_execute(Command::Quit);
    app.shutdown().await;

    let app = reopened(&root, &state).await;
    let ids = versions(&app);
    assert_eq!(ids.len(), 1, "the version tab came back");
    let buf = app.buffers.get(ids[0]).unwrap();
    assert_eq!(buf.rope.to_string(), "xfn a() {}\n");
    assert!(buf.name().starts_with("a.rs (History: "));
    assert_eq!(buf.syntax.language(), Language::Rust);
    assert_eq!(app.group().tabs.len(), 2);

    // Quit again, drop the history, reopen: no version tab.
    let (root, state) = (app.workspace.root.clone(), app.state_dir.clone().unwrap());
    let mut app = app;
    app.test_execute(Command::Quit);
    app.shutdown().await;
    fs::remove_dir_all(root.join(".jed").join("history")).unwrap();
    let app = reopened(&root, &state).await;
    assert!(versions(&app).is_empty(), "dropped with the entry gone");
    assert_eq!(app.group().tabs.len(), 1, "only a.rs");
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_text_is_the_snapshots_bytes_with_a_crlf_file_and_a_bom() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.rs"), b"\xef\xbb\xbffn a() {}\r\nlet x = 1;\r\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.rs");
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    app.test_execute(Command::ShowFileHistory);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    let id = version_open(&mut app, 1).await;
    let buf = app.buffers.get(id).unwrap();
    // The BOM is the file's encoding, not text; the CRLFs stay.
    assert_eq!(buf.rope.to_string(), "xfn a() {}\r\nlet x = 1;\r\n");
    assert!(buf.read_only.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_open_from_a_version_tab_opens_another_entry_of_the_file() {
    let (_d, _root, _state, mut app) = app().await;
    app.test_execute(Command::ShowFileHistory);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    version_open(&mut app, 1).await;
    app.test_execute(Command::OpenFileHistory);
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.mode() == Mode::History))
            .await,
        "the picker lists the version's file's entries"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hist_versions_a_version_has_no_path_so_nothing_keyed_by_the_file_finds_it() {
    let (_d, root, _state, mut app) = app().await;
    app.test_execute(Command::ShowFileHistory);
    app.test_execute(Command::FileHistory(FileHistoryCommand::OpenVersion));
    let id = version_open(&mut app, 1).await;
    assert!(
        app.buffers.get(id).unwrap().path.as_os_str().is_empty(),
        "the version keeps the file's path only in its VersionDoc"
    );
    // Close the real file's tab: no buffer is the file's any more.
    app.group_mut().active = 0;
    app.test_execute(Command::CloseTab);
    assert!(app.settle(|a| a.buffers.find_path(&root.join("a.rs")).is_none()).await);
    // The file changing on disk doesn't touch, reload or dirty the version.
    fs::write(root.join("a.rs"), "changed on disk\n").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    app.pump();
    let buf = app.buffers.get(id).unwrap();
    assert_eq!(buf.rope.to_string(), "xyyfn a() {}\n");
    assert!(!buf.is_dirty());
}
