//! End-to-end tests for the Git editor task's languages (git_editor-a): files named
//! `COMMIT_EDITMSG` / `git-rebase-todo` open as "Git Commit Message" / "Git Rebase Message",
//! commit messages are coloured (comments, branch, change kinds, paths, trailers) and Cmd+/
//! comments with `# `. Later subtasks add the editor mode, the scissors diff, rulers and IPC.

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
