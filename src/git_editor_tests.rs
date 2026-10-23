//! End-to-end tests for the Git editor task's languages (git_editor-a): files named
//! `COMMIT_EDITMSG` / `git-rebase-todo` open as "Git Commit Message" / "Git Rebase Message",
//! commit messages are coloured (comments, branch, change kinds, paths, trailers) and Cmd+/
//! comments with `# `; rulers (git_editor-b). Editor mode (git_editor-c, `mod editor_mode`):
//! `jed <message file>` / `jed --wait <file>` start as `main` decides (`crate::start_of`): single
//! file mode rooted at the work tree, no session, recent entry, trust dialog or servers, the
//! cursor at 1:1, the hint in the status bar (whole at 80 columns), Cmd+S then Cmd+W quits.

use std::fs;

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::milestone_tests::test_app;
use crate::syntax::Language;

const MESSAGE: &str = "Fix the thing\n\nBody.\n\nSigned-off-by: A <a@b.c>\n\n# Please enter the commit message for your changes. Lines starting\n# with '#' will be ignored, and an empty message aborts the commit.\n#\n# On branch main\n# Changes to be committed:\n#\tmodified:   src/a.rs\n";

fn open(dir: &tempfile::TempDir, rel: &str, text: &str) -> App {
    let root = crate::workspace::normalize(dir.path());
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    let mut app = test_app(&root, Buffer::open(&path).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    app
}

fn screen(term: &mut Terminal<TestBackend>, app: &mut App) -> Vec<String> {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// The foreground of the first cell of `needle` on screen.
fn fg_of(term: &Terminal<TestBackend>, rows: &[String], needle: &str) -> Option<ratatui::style::Color> {
    let (y, row) = rows.iter().enumerate().find(|(_, r)| r.contains(needle)).expect(needle);
    let col = row.find(needle).unwrap();
    let col = row[..col].chars().count();
    term.backend().buffer()[(col as u16, y as u16)].fg.into()
}

fn status(rows: &[String]) -> String {
    rows.last().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn git_editor_commit_message_is_coloured() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "COMMIT_EDITMSG",
        "MERGE_MSG",
        "SQUASH_MSG",
        "TAG_EDITMSG",
        "EDIT_DESCRIPTION",
        "NOTES_EDITMSG",
    ] {
        let app = open(&dir, &format!(".git/{name}"), MESSAGE);
        assert_eq!(
            app.active_buffer().unwrap().syntax.language(),
            Language::GitCommit,
            "{name}"
        );
    }
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", MESSAGE);
    let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.syntax.tree().is_some()))
            .await
    );
    let rows = screen(&mut term, &mut app);
    assert!(status(&rows).contains("Git Commit Message"), "{:?}", status(&rows));
    let key = |k: &str| app.theme.get(k).fg;
    assert_eq!(fg_of(&term, &rows, "# Please"), key("comment"));
    assert_eq!(fg_of(&term, &rows, "Signed-off-by"), key("label"));
    assert_eq!(fg_of(&term, &rows, "main"), key("markup.link"));
    assert_eq!(fg_of(&term, &rows, "modified:"), key("keyword"));
    assert_eq!(fg_of(&term, &rows, "src/a.rs"), key("string"));
    assert_ne!(key("comment"), key("string"));
}

#[tokio::test(flavor = "multi_thread")]
async fn git_editor_rebase_todo_comments_with_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(
        &dir,
        ".git/rebase-merge/git-rebase-todo",
        "pick abc123 One\npick def456 Two\n",
    );
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::GitRebase);
    let mut term = Terminal::new(TestBackend::new(100, 10)).unwrap();
    let rows = screen(&mut term, &mut app);
    assert!(status(&rows).contains("Git Rebase Message"), "{:?}", status(&rows));
    let cmd = if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    };
    app.press(K::Char('/'), cmd);
    let text = app.active_buffer().unwrap().rope.to_string();
    assert_eq!(text, "# pick abc123 One\npick def456 Two\n");
}

/// `[` and `(` auto-close in a commit message; `'` and `"` don't (apostrophes in prose), and Cmd+/
/// comments a line with `# `.
#[tokio::test(flavor = "multi_thread")]
async fn git_editor_commit_message_pairs_and_comment() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", "\n");
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.syntax.tree().is_some()))
            .await
    );
    // An apostrophe in a word, then quotes after a space (where other languages auto-close them;
    // nothing typed after them may type over a closer, or an auto-close would go unseen).
    app.type_text("it's 'x \"y ");
    app.type_text("[");
    app.type_text("(");
    let text = app.active_buffer().unwrap().rope.to_string();
    assert_eq!(text, "it's 'x \"y [()]\n");
    let cmd = if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    };
    app.press(K::Char('/'), cmd);
    let text = app.active_buffer().unwrap().rope.to_string();
    assert_eq!(text, "# it's 'x \"y [()]\n");
}

/// The foreground of the cell at screen column `col` (counted in chars) of the row holding `row_text`.
fn fg_at(term: &Terminal<TestBackend>, rows: &[String], row_text: &str, col: usize) -> ratatui::style::Color {
    let y = rows.iter().position(|r| r.contains(row_text)).expect(row_text);
    let x0 = rows[y][..rows[y].find(row_text).unwrap()].chars().count();
    term.backend().buffer()[((x0 + col) as u16, y as u16)].fg
}

/// A 60-column subject colours past column 50 as `overflow`, text on line 2 is `invalid`, and
/// `git commit -v`'s diff below the scissors line is coloured by line kind (not above it).
#[tokio::test(flavor = "multi_thread")]
async fn git_editor_subject_overflow_line_two_and_scissors_diff() {
    let dir = tempfile::tempdir().unwrap();
    let subject = "fix: aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeee"; // 55 chars
    let subject = format!("{subject}ffff"); // 59
    let text = format!(
        "{subject}\ntext on line two\n- not a diff\n\n# ------------------------ >8 ------------------------\n# Do not modify or remove the line above.\n# Everything below it will be ignored.\ndiff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n context\n-old line\n+new line\n"
    );
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", &text);
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.syntax.tree().is_some()))
            .await
    );
    let rows = screen(&mut term, &mut app);
    let key = |k: &str| app.theme.get(k).fg.unwrap();
    assert_eq!(fg_at(&term, &rows, "fix: aaaa", 49), key("markup.heading"));
    assert_eq!(fg_at(&term, &rows, "fix: aaaa", 50), key("overflow"));
    assert_eq!(fg_at(&term, &rows, "fix: aaaa", 58), key("overflow"));
    assert_eq!(fg_at(&term, &rows, "text on line two", 3), key("invalid"));
    assert_eq!(fg_at(&term, &rows, "- not a diff", 0), key("editor"));
    assert_eq!(fg_of(&term, &rows, "@@ -1,2").unwrap(), key("diff.hunk"));
    assert_eq!(fg_of(&term, &rows, "diff --git").unwrap(), key("diff.header"));
    assert_eq!(fg_of(&term, &rows, "-old line").unwrap(), key("diff.deleted"));
    assert_eq!(fg_of(&term, &rows, "+new line").unwrap(), key("diff.inserted"));
    assert_eq!(fg_at(&term, &rows, " context", 1), key("editor"));
}

/// `COMMIT_EDITMSG` draws a ruler at column 72 on short lines and rows past the text, never over
/// text, keeping each cell's background (the current line, a selection); other files draw none
/// until `rulers` is set; a set `rulers` (even `[]`) replaces git-commit's 72; scrolling sideways
/// moves it.
#[tokio::test(flavor = "multi_thread")]
async fn git_editor_rulers() {
    let ruler_cols = |rows: &[String], x0: usize| -> Vec<Vec<usize>> {
        rows.iter()
            .map(|r| {
                r.chars()
                    .enumerate()
                    .filter(|(i, c)| *c == '\u{2502}' && *i >= x0)
                    .map(|(i, _)| i - x0)
                    .collect()
            })
            .collect()
    };
    let long = "x".repeat(80);
    let text = format!("short\n\n{long}\n");
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", &text);
    let mut term = Terminal::new(TestBackend::new(170, 10)).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.syntax.tree().is_some()))
            .await
    );
    let rows = screen(&mut term, &mut app);
    let r0 = rows.iter().position(|r| r.contains("short")).unwrap();
    let x0 = rows[r0][..rows[r0].find("short").unwrap()].chars().count();
    let cols = ruler_cols(&rows, x0);
    assert_eq!(cols[r0], vec![72], "{rows:?}");
    assert_eq!(cols[r0 + 1], vec![72]);
    assert!(cols[r0 + 2].is_empty(), "text keeps its glyphs: {:?}", cols[r0 + 2]);
    assert_eq!(cols[r0 + 5], vec![72], "rows past the text too");
    // Its colour, over the backgrounds: the cursor's line keeps its highlight, a plain row the
    // editor's.
    let cell =
        |term: &Terminal<TestBackend>, row: usize| term.backend().buffer()[((x0 + 72) as u16, row as u16)].clone();
    let fg = app.theme.get("editorRuler.foreground").fg.unwrap();
    let line_bg = app.theme.get("editor.lineHighlight").bg.unwrap();
    assert_eq!((cell(&term, r0).fg, cell(&term, r0).bg), (fg, line_bg));
    assert_eq!(cell(&term, r0 + 5).fg, fg);
    assert_eq!(cell(&term, r0 + 5).bg, app.theme.get("editor").bg.unwrap());

    // Horizontal scroll moves it.
    app.active_mut().unwrap().1.left_col = 10;
    let rows = screen(&mut term, &mut app);
    assert_eq!(ruler_cols(&rows, x0)[r0], vec![62]);

    // A plain file: none; the setting adds them.
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, "a.txt", &text);
    let rows = screen(&mut term, &mut app);
    assert!(ruler_cols(&rows, x0).iter().all(|c| c.is_empty()));
    app.settings.rulers = Some(vec![10]);
    let rows = screen(&mut term, &mut app);
    let cols = ruler_cols(&rows, x0);
    assert_eq!(cols[r0], vec![10]);
    assert!(cols[r0 + 2].is_empty());

    // A selected space shows the ruler on the selection's background.
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", &format!("short\n\n{}\n", " ".repeat(80)));
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::End, M::SHIFT);
    let rows = screen(&mut term, &mut app);
    let sel_bg = app.theme.get("editor.selection").bg.unwrap();
    assert_eq!(ruler_cols(&rows, x0)[r0 + 2], vec![72], "{rows:?}");
    assert_eq!((cell(&term, r0 + 2).fg, cell(&term, r0 + 2).bg), (fg, sel_bg));

    // A set `rulers` replaces git-commit's 72; `[]` turns it off.
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(&dir, ".git/COMMIT_EDITMSG", &text);
    app.settings.rulers = Some(vec![10]);
    assert_eq!(ruler_cols(&screen(&mut term, &mut app), x0)[r0], vec![10]);
    app.settings.rulers = Some(Vec::new());
    assert!(
        ruler_cols(&screen(&mut term, &mut app), x0)
            .iter()
            .all(|c| c.is_empty())
    );
}

/// Editor mode (git_editor-c): `main`'s start on a real repository's message file.
mod editor_mode {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, InitialFile, Mode};
    use crate::buffer::Buffer;
    use crate::lsp::fake::{FakeLauncher, FakeOptions};
    use crate::workspace::Workspace;
    use crate::workspace::trust::TrustStore;

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A `git2` repo with a `.workspace` (a server for commit messages, a task, tab width 3; trust
    /// undecided) and a state dir holding the workspace's own session, written by a workspace
    /// mode run with `a.cpp` open (its recent entry removed); returns (work tree, state dir,
    /// session file).
    async fn repo(dir: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let base = crate::workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        git2::Repository::init(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(
            root.join(".workspace"),
            "[settings]\ntab_width = 3\n\n[lsp.git-commit]\ncommand = \"fake-ls\"\n\n\
             [tasks.build]\ncommand = \"echo build\"\n",
        )
        .unwrap();
        fs::write(root.join("a.cpp"), "int a;\n").unwrap();
        let (mut app, _) = open(&[&root.join("a.cpp")], &state);
        assert_eq!(app.mode, Mode::Workspace);
        assert!(app.settle(|a| !a.session.loading()).await);
        app.test_execute(crate::command::Command::Quit);
        app.shutdown().await;
        let session = crate::session::file(&state, &root);
        assert!(session.is_file(), "the workspace's own session");
        let _ = fs::remove_file(crate::recent::file(&state));
        (root, state, session)
    }

    /// What `main` does for `args`: `start_of` decides the mode, root and hint, then the app is
    /// built from them as `main` builds it.
    fn open(args: &[&Path], state: &Path) -> (App, std::sync::Arc<FakeLauncher>) {
        let args = crate::parse_args_from(args.iter().map(|a| a.to_str().unwrap().to_string())).unwrap();
        let start = crate::start_of(&args).unwrap();
        let ws = Workspace::load(start.root);
        let (path, goto) = start.file.expect("a file");
        let buffer = Buffer::open_with(&path, ws.settings().indentation()).unwrap();
        let initial = Some(InitialFile { buffer, goto });
        let mut app = App::new_in(
            start.mode,
            ws,
            TrustStore::load(state.join("trust.toml")),
            initial,
            true,
        );
        app.editor_hint = start.hint;
        app.state_dir = Some(state.to_path_buf());
        let fake = FakeLauncher::new(FakeOptions::default());
        app.lsp.set_launcher(fake.clone());
        app.test_start();
        app.test_plain_whitespace();
        (app, fake)
    }

    fn status(app: &mut App, width: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, 11)].symbol()).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn git_editor_commit_message_opens_lean_and_saves() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state, session) = repo(&dir).await;
        let saved = fs::read_to_string(&session).unwrap();
        assert!(saved.contains("a.cpp"), "{saved}");
        let msg = root.join(".git/COMMIT_EDITMSG");
        fs::write(&msg, "\n# Please enter the commit message\n#\n# On branch main\n").unwrap();
        let (mut app, fake) = open(&[&msg], &state);

        assert_eq!(app.mode, Mode::SingleFile, "no flag needed");
        assert_eq!(app.workspace.root, root, "the work tree is the root");
        assert_eq!(app.settings.tab_width, 3, "the .workspace's settings apply");
        assert!(app.prompt.is_none(), "no trust dialog");
        assert!(
            app.groups.len() == 1 && app.group().tabs.len() == 1,
            "no session restored"
        );
        assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
        app.pump();
        assert!(fake.launches().is_empty(), "no language server");
        assert!(!app.ui.side_visible && !app.ui.panel_visible);
        let bar = status(&mut app, 140);
        assert!(
            bar.contains(" Save and close to commit · an empty message aborts "),
            "{bar:?}"
        );
        assert!(!bar.contains("Single file"), "{bar:?}");
        assert!(
            bar.contains("Git Commit Message") && bar.contains("Ln 1, Col 1"),
            "{bar:?}"
        );
        // At 80 columns the hint stays whole: the position and what else fits beside it.
        let bar = status(&mut app, 80);
        assert!(
            bar.starts_with(" Save and close to commit · an empty message aborts    "),
            "{bar:?}"
        );
        assert!(bar.trim_end().ends_with("Ln 1, Col 1   Spaces: 3"), "{bar:?}");

        app.type_text("Fix it");
        app.press(K::Char('s'), cmd());
        assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await, "saved");
        app.press(K::Char('w'), cmd());
        assert!(app.quitting(), "closing the last tab quits");
        app.shutdown().await;
        assert!(fs::read_to_string(&msg).unwrap().starts_with("Fix it\n# Please"));
        assert_eq!(fs::read_to_string(&session).unwrap(), saved, "the session untouched");
        assert!(!crate::recent::file(&state).exists(), "no recent entry");
        let sessions = fs::read_dir(session.parent().unwrap()).unwrap().count();
        assert_eq!(sessions, 1, "no other session written");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn git_editor_rebase_todo_has_its_own_hint_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state, _) = repo(&dir).await;
        let todo = root.join(".git/rebase-merge/git-rebase-todo");
        fs::create_dir_all(todo.parent().unwrap()).unwrap();
        fs::write(&todo, "pick abc one\n# Rebase\n").unwrap();
        let (mut app, _fake) = open(&[&todo], &state);
        assert_eq!(app.mode, Mode::SingleFile);
        assert_eq!(app.workspace.root, root);
        let bar = status(&mut app, 80);
        assert!(
            bar.starts_with(" Save and close to rebase · an empty list aborts "),
            "{bar:?}"
        );
        app.press(K::Char('/'), cmd());
        assert!(
            app.active_buffer()
                .unwrap()
                .rope
                .to_string()
                .starts_with("# pick abc one\n")
        );
    }

    /// `start_of`'s root comes from git, not from the nearest `.workspace`: without one the
    /// fallback would be `.git/` itself; a linked worktree gets its own work tree.
    #[test]
    fn git_editor_root_from_git_without_a_workspace_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = crate::workspace::normalize(dir.path());
        let repo = git2::Repository::init(base.join("repo")).unwrap();
        let sig = git2::Signature::now("a", "a@b.c").unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        repo.worktree("w", &base.join("wt"), None).unwrap();
        for (msg, root) in [
            (base.join("repo/.git/COMMIT_EDITMSG"), base.join("repo")),
            (base.join("repo/.git/worktrees/w/COMMIT_EDITMSG"), base.join("wt")),
        ] {
            fs::write(&msg, "\n").unwrap();
            let args = crate::parse_args_from([msg.to_str().unwrap().to_string()]).unwrap();
            let start = crate::start_of(&args).unwrap();
            assert_eq!(start.mode, Mode::SingleFile);
            assert_eq!(start.root, root, "{}", msg.display());
            assert_eq!(start.file.unwrap().0, msg);
        }
    }

    /// `--wait` on any file: single file mode, its usual root and status bar; a file named like
    /// git's without `HEAD` beside it isn't editor mode.
    #[tokio::test(flavor = "multi_thread")]
    async fn git_editor_wait_on_another_file_keeps_single_file_hint() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state, _) = repo(&dir).await;
        let wait = Path::new("--wait");
        let (mut app, _fake) = open(&[wait, &root.join("a.cpp")], &state);
        assert_eq!(app.mode, Mode::SingleFile);
        assert_eq!(app.workspace.root, root, "the .workspace's folder");
        assert!(status(&mut app, 80).starts_with(" Single file "));

        let lookalike = root.join("notes/COMMIT_EDITMSG");
        fs::create_dir_all(lookalike.parent().unwrap()).unwrap();
        fs::write(&lookalike, "x\n").unwrap();
        let (app, _fake) = open(&[&lookalike], &state);
        assert_eq!(app.mode, Mode::Workspace, "no HEAD beside it: an ordinary file");
        assert!(app.editor_hint.is_none());
    }
}
