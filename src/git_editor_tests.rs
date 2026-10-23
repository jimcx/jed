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
