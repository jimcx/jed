//! hist_versions-a: "File History: Open" opens a history entry's text in a read-only version tab
//! (`Buffer::version`), titled `a.rs (History: just now)` and highlighted by the file's language,
//! from the palette picker and from the Explorer section's Space; copy, select all and splits
//! work, nothing keyed by a path takes it (the file's own buffer stays the file's), a gone entry
//! reports and opens nothing, and sessions bring the tab back from its snapshot (dropped when
//! the entry is gone).
//!
//! hist_versions-b (`mod commit_file`): a commit's file opens as the same kind of tab, titled
//! `a.rs (abc1234)`, from a commit tab's stat row, a commit diff tab (command and header button);
//! a gone commit or path reports, and sessions keep it by commit id and path.

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

// ---- hist_versions-b: a commit's file ----

mod commit_file {
    use super::*;
    use crate::app::Focus;
    use crate::command::{CommitCommand, GitHistoryCommand, Motion};
    use crate::diff_view::Text;
    use crate::git::test_repo::{commit_all, git, init};
    use crate::layout::SidePanel;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Two commits: the first adds `a.rs` and `b.txt`, the second changes `a.rs` and deletes
    /// `b.txt`. The disk then has a third text of `a.rs`. Returns the second's full id.
    fn repo(root: &Path) -> String {
        init(root);
        fs::write(root.join("a.rs"), "fn one() {}\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        commit_all(root, "first");
        fs::write(root.join("a.rs"), "fn two() {}\n").unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        commit_all(root, "second");
        let id = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        fs::write(root.join("a.rs"), "fn three() {}\n").unwrap();
        id
    }

    fn draw(app: &mut App) {
        app.ui.side_width = 30;
        let mut term = Terminal::new(TestBackend::new(240, 30)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    }

    /// The app on `a.rs` with the newest commit open in a commit tab (cursor on a.rs's row).
    async fn with_commit_tab(root: &Path, state: &Path) -> App {
        let mut app = open_file_app(root, state, "a.rs");
        assert!(
            app.settle(|a| !a.git.running() && !a.git_history.loading && a.git_history.commits.len() == 2)
                .await
        );
        app.ui.focus = Focus::SidePanel;
        app.ui.side_panel = SidePanel::Explorer;
        app.git_history.set_focused(true);
        app.test_execute(Command::GitHistory(GitHistoryCommand::Down));
        app.test_execute(Command::GitHistory(GitHistoryCommand::Open));
        assert!(
            app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
                .await
        );
        app
    }

    fn doc_start(app: &mut App) {
        app.test_execute(Command::Move {
            motion: Motion::DocStart,
            select: false,
        });
    }

    fn move_to_line_of(app: &mut App, needle: &str) {
        doc_start(app);
        let text = app.active_buffer().unwrap().rope.to_string();
        let line = text.lines().position(|l| l.contains(needle)).expect(needle);
        for _ in 0..line {
            app.test_execute(Command::Move {
                motion: Motion::Down,
                select: false,
            });
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_commit_tabs_stat_row_opens_the_file_at_the_commit() {
        let (_d, root, state) = dirs();
        let id = repo(&root);
        let mut app = with_commit_tab(&root, &state).await;
        // Alt+Enter, the key bound for it in a commit tab.
        app.press_bound(Command::Commit(CommitCommand::OpenFile));
        let buf = version_open(&mut app, 1).await;
        let buf = app.buffers.get(buf).unwrap();
        assert_eq!(
            buf.rope.to_string(),
            "fn two() {}\n",
            "the commit's blob, not the disk's"
        );
        assert_eq!(buf.name(), format!("a.rs ({})", &id[..7]));
        assert!(buf.read_only.is_some());
        assert!(!buf.has_file());
        assert_eq!(buf.syntax.language(), Language::Rust);
        assert_eq!(app.group().tabs.len(), 3, "a.rs, the commit, the version");

        // Again from the commit tab: the same tab is shown, none opens.
        app.group_mut().active = 1;
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        assert_eq!(app.group().active, 2);
        assert_eq!(versions(&app).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_commit_row_without_a_file_says_so() {
        let (_d, root, state) = dirs();
        repo(&root);
        let mut app = with_commit_tab(&root, &state).await;
        doc_start(&mut app);
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        assert_eq!(app.message.as_ref().unwrap().text, "No file on this line");
        assert!(versions(&app).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_file_deleted_in_the_commit_reports_and_opens_nothing() {
        let (_d, root, state) = dirs();
        let id = repo(&root);
        let mut app = with_commit_tab(&root, &state).await;
        draw(&mut app);
        move_to_line_of(&mut app, "b.txt");
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        let want = format!("b.txt isn't in commit {}", &id[..7]);
        assert!(
            app.settle(|a| a.message.as_ref().is_some_and(|m| m.text.contains(&want)))
                .await,
            "{:?}",
            app.message.as_ref().map(|m| &m.text)
        );
        assert!(versions(&app).is_empty());
    }

    /// Opens a.rs's diff from the commit tab's stat row; the diff tab is in the group to the right.
    async fn with_commit_diff(root: &Path, state: &Path) -> App {
        let mut app = with_commit_tab(root, state).await;
        draw(&mut app);
        app.test_execute(Command::Commit(CommitCommand::Open));
        assert!(app.settle(|a| a.diffs_settled()).await);
        draw(&mut app);
        app
    }

    fn diff_group(app: &App) -> crate::layout::GroupId {
        app.groups
            .ids()
            .into_iter()
            .find(|&g| app.groups.get(g).unwrap().active_diff().is_some())
            .expect("a diff tab")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_commit_diff_tab_opens_the_file_by_command() {
        let (_d, root, state) = dirs();
        let id = repo(&root);
        let mut app = with_commit_diff(&root, &state).await;
        let g = diff_group(&app);
        let d = app.groups.get(g).unwrap().active_diff().unwrap();
        assert!(matches!(d.pair.right, Text::Commit(_)));
        app.groups.active = g;
        app.test_execute(Command::OpenFileAtCommit);
        let b = version_open(&mut app, 1).await;
        let buf = app.buffers.get(b).unwrap();
        assert_eq!(buf.rope.to_string(), "fn two() {}\n");
        assert_eq!(buf.name(), format!("a.rs ({})", &id[..7]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_the_commit_diff_header_button_opens_the_file() {
        let (_d, root, state) = dirs();
        repo(&root);
        let mut app = with_commit_diff(&root, &state).await;
        let g = diff_group(&app);
        let r = app
            .hits
            .group(g)
            .and_then(|h| h.diff.as_ref())
            .and_then(|d| d.open_version)
            .expect("the Open Version button is drawn");
        app.mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            r.x + 1,
            r.y,
        );
        let b = version_open(&mut app, 1).await;
        assert_eq!(app.buffers.get(b).unwrap().rope.to_string(), "fn two() {}\n");
    }

    #[test]
    fn hist_versions_commit_read_gives_the_blob_at_that_commit() {
        let (_d, root, _state) = dirs();
        init(&root);
        fs::write(root.join("a.rs"), "one\n").unwrap();
        commit_all(&root, "root");
        let first = git2::Oid::from_str(git(&root, &["rev-parse", "HEAD"]).trim()).unwrap();
        fs::write(root.join("a.rs"), "two\n").unwrap();
        commit_all(&root, "second");
        let second = git2::Oid::from_str(git(&root, &["rev-parse", "HEAD"]).trim()).unwrap();
        fs::write(root.join("a.rs"), "three\n").unwrap();
        let rel = Path::new("a.rs");
        let read = |id| crate::version::read_commit_file(&root, id, rel);
        // The root commit and a later one each give their own bytes, not the disk's.
        assert_eq!(read(first).unwrap(), b"one\n");
        assert_eq!(read(second).unwrap(), b"two\n");
        let missing = crate::version::read_commit_file(&root, first, Path::new("nope.rs"));
        assert_eq!(
            missing.unwrap_err(),
            format!("nope.rs isn't in commit {}", &first.to_string()[..7])
        );
        let gone = git2::Oid::from_str("1234567890123456789012345678901234567890").unwrap();
        assert_eq!(read(gone).unwrap_err(), "commit 1234567 is gone");
        assert_eq!(
            crate::version::VersionSource::Commit {
                id: first,
                rel: rel.into()
            }
            .label(0),
            first.to_string()[..7]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_commit_tab_opens_a_renamed_file_by_its_new_path() {
        let (_d, root, state) = dirs();
        init(&root);
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(root.join("old.rs"), "fn renamed() {\n    1;\n    2;\n    3;\n}\n").unwrap();
        commit_all(&root, "first");
        git(&root, &["mv", "old.rs", "new.rs"]);
        fs::write(root.join("a.rs"), "fn a2() {}\n").unwrap();
        commit_all(&root, "rename");
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let mut app = with_commit_tab(&root, &state).await;
        draw(&mut app);
        move_to_line_of(&mut app, "new.rs");
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        let b = version_open(&mut app, 1).await;
        let buf = app.buffers.get(b).unwrap();
        assert_eq!(buf.rope.to_string(), "fn renamed() {\n    1;\n    2;\n    3;\n}\n");
        assert_eq!(buf.name(), format!("new.rs ({})", &id[..7]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_the_open_version_button_is_only_on_commit_diffs() {
        let (_d, root, state) = dirs();
        repo(&root);
        let mut app = open_file_app(&root, &state, "a.rs");
        assert!(app.settle(|a| !a.git.running() && a.git.status.is_some()).await);
        app.test_execute(Command::OpenChange);
        assert!(app.settle(|a| a.diffs_settled()).await);
        draw(&mut app);
        let g = diff_group(&app);
        assert!(!matches!(
            app.groups.get(g).unwrap().active_diff().unwrap().pair.right,
            Text::Commit(_)
        ));
        let hit = app.hits.group(g).and_then(|h| h.diff.as_ref()).expect("a diff header");
        assert!(hit.open_version.is_none());
        app.groups.active = g;
        app.test_execute(Command::OpenFileAtCommit);
        assert_eq!(app.message.as_ref().unwrap().text, "Not in a commit or a commit's diff");
        assert!(versions(&app).is_empty());
    }

    /// A workspace below the repository's top: the session stores the tab's path relative to the
    /// workspace (`a.rs`) but the commit's path relative to the repository (`sub/a.rs`), and
    /// restoring reads the commit's.
    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_session_commit_file_uses_the_stored_repo_path() {
        let (_d, top, state) = dirs();
        init(&top);
        let ws = top.join("sub");
        fs::create_dir(&ws).unwrap();
        fs::write(ws.join("a.rs"), "fn one() {}\n").unwrap();
        commit_all(&top, "first");
        fs::write(ws.join("a.rs"), "fn two() {}\n").unwrap();
        commit_all(&top, "second");
        let id = git(&top, &["rev-parse", "HEAD"]).trim().to_string();
        fs::write(ws.join("a.rs"), "fn three() {}\n").unwrap();
        let mut app = with_commit_tab(&ws, &state).await;
        app.state_dir = Some(state.clone());
        assert!(app.settle(|a| !a.session.loading()).await);
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        version_open(&mut app, 1).await;
        app.test_execute(Command::Quit);
        app.shutdown().await;

        let mut app = reopened(&ws, &state).await;
        assert!(app.settle(|a| !versions(a).is_empty()).await, "the tab is restored");
        let buf = app.buffers.get(versions(&app)[0]).unwrap();
        assert_eq!(buf.rope.to_string(), "fn two() {}\n");
        assert_eq!(buf.name(), format!("a.rs ({})", &id[..7]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_the_command_elsewhere_says_where_it_works() {
        let (_d, root, state) = dirs();
        repo(&root);
        let mut app = open_file_app(&root, &state, "a.rs");
        assert!(app.settle(|a| !a.git.running()).await);
        app.test_execute(Command::OpenFileAtCommit);
        assert_eq!(app.message.as_ref().unwrap().text, "Not in a commit or a commit's diff");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_sessions_keep_a_commit_file_and_drop_it_when_the_commit_is_gone() {
        let (_d, root, state) = dirs();
        let id = repo(&root);
        let mut app = with_commit_tab(&root, &state).await;
        app.state_dir = Some(state.clone());
        assert!(app.settle(|a| !a.session.loading()).await);
        app.test_execute(Command::Commit(CommitCommand::OpenFile));
        version_open(&mut app, 1).await;
        app.test_execute(Command::Quit);
        app.shutdown().await;

        let mut app = reopened(&root, &state).await;
        assert!(app.settle(|a| !versions(a).is_empty()).await);
        let b = versions(&app)[0];
        let buf = app.buffers.get(b).unwrap();
        assert_eq!(buf.rope.to_string(), "fn two() {}\n");
        assert_eq!(buf.name(), format!("a.rs ({})", &id[..7]));
        app.test_execute(Command::Quit);
        app.shutdown().await;

        git(&root, &["reset", "--hard", "HEAD~1"]);
        git(&root, &["reflog", "expire", "--expire=now", "--all"]);
        git(&root, &["gc", "--prune=now", "--quiet"]);
        let app = reopened(&root, &state).await;
        assert!(versions(&app).is_empty(), "dropped with the commit gone");
    }
}

/// hist_versions-c: "File History: Restore Selection" puts the version's lines back into the
/// file's buffer as one undo step, from a version tab, a history compare diff tab and a commit
/// diff tab; a file not open opens first, a read-only buffer refuses, nothing is saved.
mod restore {
    use super::*;
    use crate::command::Motion;
    use crate::diff_caret::Caret;
    use crate::diff_view::Pair;

    const V: &str = "a\nb\nc\nd\ne\nf\n";
    const NOW: &str = "a\nB\nc\nd\nE\nf\n";

    /// `t.txt` = V recorded as the older entry, then the buffer made `NOW` (saved: the newer
    /// entry), the older entry's version tab open and active. Returns the file's buffer and
    /// the older entry.
    async fn setup() -> (tempfile::TempDir, std::path::PathBuf, App, BufferId, Snapshot) {
        let (d, root, state) = dirs();
        fs::write(root.join("t.txt"), V).unwrap();
        let mut app = open_file_app(&root, &state, "t.txt");
        app.type_text("x");
        app.press(K::Backspace, M::NONE);
        save(&mut app).await;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let file = app.group().active_tab().unwrap().buffer;
        app.test_execute(Command::SelectAll);
        app.type_text(NOW);
        save(&mut app).await;
        assert!(app.settle(|a| a.file_history_view.entries.len() == 2).await);
        let older = entry(&app, 1);
        app.open_version(root.join("t.txt"), VersionSource::Snapshot(older));
        let v = version_open(&mut app, 1).await;
        assert_eq!(text(&app, v), V);
        (d, root, app, file, older)
    }

    fn text(app: &App, id: BufferId) -> String {
        app.buffers.get(id).unwrap().rope.to_string()
    }

    fn named(app: &App) -> Option<BufferId> {
        app.buffers
            .iter()
            .find(|(_, b)| b.has_file() && b.name() == "t.txt")
            .map(|(id, _)| id)
    }

    fn msg(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    async fn said(app: &mut App, what: &str) {
        assert!(app.settle(|a| msg(a).contains(what)).await, "no `{what}`: {}", msg(app));
    }

    /// Selects lines `first..=last` of the active diff tab's `right` side.
    fn pick_rows(app: &mut App, right: bool, first: usize, last: usize) {
        let d = app.group_mut().active_diff_mut().unwrap();
        let len = d.side_text(right, last).len();
        d.caret = Caret {
            right,
            anchor: (first, 0),
            head: (last, len),
            col: None,
        };
        d.cursor = d.aligned().row_of(right, last);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_puts_the_selected_lines_back_as_one_undo_step() {
        let (_d, root, mut app, file, _) = setup().await;
        app.press(K::Down, M::NONE); // the version's line 1: "b"
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "Restored 1 line from t.txt (History:").await;
        assert!(msg(&app).ends_with("(not saved)"));
        assert_eq!(text(&app, file), "a\nb\nc\nd\nE\nf\n", "only line 1 changed");
        assert!(app.buffers.get(file).unwrap().is_dirty());
        assert_eq!(fs::read_to_string(root.join("t.txt")).unwrap(), NOW, "nothing saved");
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::Undo);
        assert_eq!(text(&app, file), NOW, "one undo removes it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_takes_every_selection_and_leaves_the_rest() {
        let (_d, _root, mut app, file, _) = setup().await;
        // Select lines 1..=4 of the version: both changed lines.
        app.press(K::Down, M::NONE);
        for _ in 0..4 {
            app.press(K::Down, M::SHIFT);
        }
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "Restored 2 lines").await;
        assert_eq!(text(&app, file), V);
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::Undo);
        assert_eq!(text(&app, file), NOW, "both lines are one undo step");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_a_selection_ending_at_column_zero_leaves_that_line() {
        let (_d, _root, mut app, file, _) = setup().await;
        // From line 3's start to line 4's start: line 3 ("d", unchanged) only, not line 4 (E).
        for _ in 0..3 {
            app.press(K::Down, M::NONE);
        }
        app.press(K::Down, M::SHIFT);
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "No changes in the selection").await;
        assert_eq!(text(&app, file), NOW);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_maps_through_a_file_edited_since() {
        let (_d, _root, mut app, file, _) = setup().await;
        // Two lines inserted at the top of the file's buffer, after the version.
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::Move {
            motion: Motion::DocStart,
            select: false,
        });
        app.type_text("n1\nn2\n");
        app.test_execute(Command::NextTab);
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE); // the version's line 4: "e"
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "Restored 1 line").await;
        assert_eq!(text(&app, file), "n1\nn2\na\nB\nc\nd\ne\nf\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_refuses_when_the_buffer_changed_meanwhile() {
        let (_d, _root, mut app, file, _) = setup().await;
        app.press(K::Down, M::NONE);
        app.test_execute(Command::RestoreFileHistorySelection);
        // The diff runs off-thread and its result waits in the channel until the app pumps
        // events (`settle`): this edit lands first, through the real key path.
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::Move {
            motion: Motion::DocStart,
            select: false,
        });
        app.type_text("z");
        said(&mut app, "the file changed, try again").await;
        assert_eq!(text(&app, file), format!("z{NOW}"), "only the user's edit");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_of_unchanged_lines_says_so() {
        let (_d, _root, mut app, file, _) = setup().await;
        app.test_execute(Command::RestoreFileHistorySelection); // line 0: "a", unchanged
        said(&mut app, "No changes in the selection").await;
        assert_eq!(text(&app, file), NOW);
        assert!(!app.buffers.get(file).unwrap().is_dirty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_refuses_a_read_only_buffer() {
        let (_d, _root, mut app, file, _) = setup().await;
        app.buffers.get_mut(file).unwrap().read_only = Some("locked".into());
        app.press(K::Down, M::NONE);
        app.test_execute(Command::RestoreFileHistorySelection);
        // Refused at once, before anything is read off-thread.
        assert!(msg(&app).contains("Read-only: locked"), "{}", msg(&app));
        assert_eq!(app.file_history.restoring, 0);
        said(&mut app, "Read-only: locked").await;
        assert_eq!(text(&app, file), NOW);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_opens_a_file_that_isnt_open_then_restores() {
        let (_d, _root, mut app, _file, _) = setup().await;
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::CloseTab);
        assert!(named(&app).is_none(), "the file is closed");
        assert_eq!(app.group().tabs.len(), 1, "only the version tab is left");
        app.press(K::Down, M::NONE);
        app.test_execute(Command::RestoreFileHistorySelection);
        assert!(app.settle(|a| named(a).is_some()).await, "the file opens");
        said(&mut app, "Restored 1 line").await;
        let id = named(&app).unwrap();
        assert_eq!(text(&app, id), "a\nb\nc\nd\nE\nf\n");
        assert!(app.buffers.get(id).unwrap().is_dirty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_outside_a_version_or_compare_tab_says_where() {
        let (_d, _root, mut app, file, _) = setup().await;
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::RestoreFileHistorySelection);
        assert!(msg(&app).contains("Select lines in a version tab"), "{}", msg(&app));
        assert_eq!(text(&app, file), NOW);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_from_a_history_compare_tab() {
        let (_d, root, mut app, file, older) = setup().await;
        // Two lines added on top and saved, so the file's line numbers differ from the entry's.
        app.test_execute(Command::PrevTab);
        app.test_execute(Command::Move {
            motion: Motion::DocStart,
            select: false,
        });
        app.type_text("n1\nn2\n");
        save(&mut app).await;
        app.open_diff(root.join("t.txt"), Pair::history(older));
        assert!(app.settle(|a| a.diffs_settled()).await);
        pick_rows(&mut app, false, 4, 4); // the entry's line 4: "e"
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "Restored 1 line").await;
        assert_eq!(text(&app, file), "n1\nn2\na\nB\nc\nd\ne\nf\n", "only e is back");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hist_versions_restore_from_a_commit_diff_tab() {
        use crate::git::test_repo::{commit_all, init};
        let (_d, root, state) = dirs();
        init(&root);
        fs::write(root.join("t.txt"), "a\nb\nc\n").unwrap();
        commit_all(&root, "first");
        fs::write(root.join("t.txt"), "a\nB\nc\n").unwrap();
        commit_all(&root, "second");
        fs::write(root.join("t.txt"), "a\nX\nc\nd\n").unwrap();
        let mut app = open_file_app(&root, &state, "t.txt");
        let id = named(&app).unwrap();
        let (c, p) = {
            let out = |a: &str| crate::git::test_repo::git(&root, &["rev-parse", a]).trim().to_string();
            (out("HEAD"), out("HEAD~1"))
        };
        let (c, p) = (git2::Oid::from_str(&c).unwrap(), git2::Oid::from_str(&p).unwrap());
        assert!(app.settle(|a| a.git.status.is_some()).await);
        app.open_diff(root.join("t.txt"), Pair::commit(c, Some(p)));
        assert!(crate::milestone10_tests::settle_diffs(&mut app).await);
        pick_rows(&mut app, true, 1, 1); // the commit's line 1: "B"
        app.test_execute(Command::RestoreFileHistorySelection);
        said(&mut app, "Restored 1 line").await;
        assert_eq!(text(&app, id), "a\nB\nc\nd\n");
        assert!(app.buffers.get(id).unwrap().is_dirty());
        assert_eq!(fs::read_to_string(root.join("t.txt")).unwrap(), "a\nX\nc\nd\n");
    }
}
