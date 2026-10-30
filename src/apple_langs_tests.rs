//! End-to-end tests for the Apple languages task (`apple_langs`). So far apple_langs-a,
//! Objective-C / Objective-C++: `.m` / `.mm` files get their language and open with the built-in
//! `clangd` (a fake server: one launch for `.m` and `.mm`, `languageId` `objective-c` in
//! `didOpen`, `objective-cpp` for `.mm`); typing `@"` in a `.m` file closes the string and
//! Cmd+/ comments a line with `//`; a real clangd (skipped with a note when it isn't on PATH) reports an error, shows
//! hover, goes to a definition and completes a member in a `.m` file.

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{encoding, idle, open, open_app, project, real_completion, real_diagnostics};
use crate::milestone9_tests::{real_hover_and_definition, uri, which};
use crate::syntax::Language;

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_objc_files_start_the_clangd_server() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[("a.m", "int main(void) { return 0; }\n"), ("b.mm", "int x;\n")],
    );
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.m"));
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::ObjC);
    assert!(app.settle(|a| encoding(a, Language::ObjC).is_some()).await);
    assert_eq!(fake.launches(), ["clangd"]);
    assert!(app.settle(|_| fake.first("textDocument/didOpen").is_some()).await);
    let open_msg = fake.first("textDocument/didOpen").unwrap();
    assert_eq!(open_msg["params"]["textDocument"]["languageId"], "objective-c");

    open(&mut app, &root.join("b.mm"));
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::ObjCpp);
    let mm = uri(&root.join("b.mm"));
    assert!(app.settle(|_| fake.document(&mm).is_some()).await);
    assert_eq!(fake.document(&mm).unwrap().language, "objective-cpp");
    assert_eq!(fake.document(&uri(&root.join("a.m"))).unwrap().language, "objective-c");
    idle(&mut app).await;
    assert_eq!(fake.launches(), ["clangd"], ".mm shares the server");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_clangd_objc_diagnostics() {
    if which("clangd").is_none() {
        eprintln!("apple_langs_real_clangd_objc_diagnostics: skipped, clangd isn't on PATH");
        return;
    }
    let fix = |app: &mut crate::app::App| {
        app.press(K::Down, M::NONE);
        app.press(K::End, M::NONE);
        app.press(K::Left, M::NONE);
        app.press(K::Char('d'), crate::milestone9_tests::cmd());
        app.type_text("0");
    };
    real_diagnostics(
        ("a.m", "int main(void) {\n  return undefined_thing;\n}\n"),
        "undefined_thing",
        |_| {},
        fix,
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_clangd_objc_hover_and_definition() {
    if which("clangd").is_none() {
        eprintln!("apple_langs_real_clangd_objc_hover_and_definition: skipped, clangd isn't on PATH");
        return;
    }
    real_hover_and_definition(
        &[
            ("a.m", "#import \"b.h\"\nint main(void) {\n  return answer(1);\n}\n"),
            (
                "b.h",
                "// The answer.\nstatic inline int answer(int x) {\n  return 41 + x;\n}\n",
            ),
        ],
        "answer",
        &["answer", "int x"],
        "b.h",
        1,
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_clangd_objc_completion() {
    if which("clangd").is_none() {
        eprintln!("apple_langs_real_clangd_objc_completion: skipped, clangd isn't on PATH");
        return;
    }
    real_completion(
        (
            "a.m",
            "struct Point {\n  int xcoord;\n  int ycoord;\n};\nint main(void) {\n  struct Point pt;\n  pt\n}\n",
        ),
        "pt",
        "ycoord",
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_objc_edits_by_the_c_like_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.m", "id s = ;\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.m"));
    let parsed = |a: &crate::app::App| a.active_buffer().unwrap().syntax.tree().is_some();
    assert!(app.settle(parsed).await);
    let text = |a: &crate::app::App| a.active_buffer().unwrap().rope.to_string();
    // `@"` closes as `"` does; the closing quote is typed over.
    app.press(K::End, M::NONE);
    app.press(K::Left, M::NONE);
    app.type_text("@\"hi");
    assert_eq!(text(&app), "id s = @\"hi\";\n");
    app.type_text("\"");
    assert_eq!(text(&app), "id s = @\"hi\";\n");
    // Inside the string `'` doesn't close (`notIn` strings, as in C).
    app.press(K::Left, M::NONE);
    assert!(app.settle(parsed).await);
    app.type_text("'");
    assert_eq!(text(&app), "id s = @\"hi'\";\n");
    // Comment Line uses `//`.
    app.press(K::Char('/'), crate::milestone9_tests::cmd());
    assert_eq!(text(&app), "// id s = @\"hi'\";\n");
    app.shutdown_servers().await;
}
