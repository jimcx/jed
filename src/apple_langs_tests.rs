//! End-to-end tests for the Apple languages task (`apple_langs`). So far apple_langs-a,
//! Objective-C / Objective-C++ (apple_langs-b, Swift, is at the end of the file, including `"`
//! auto-closing inside `\(…)` but not in string text): `.m` / `.mm` files get their language and
//! open with the built-in
//! `clangd` (a fake server: one launch for `.m` and `.mm`, `languageId` `objective-c` in
//! `didOpen`, `objective-cpp` for `.mm`); typing `@"` in a `.m` file closes the string and
//! Cmd+/ comments a line with `//`; a real clangd (skipped with a note when it isn't on PATH) reports an error, shows
//! hover, goes to a definition and completes a member in a `.m` file. apple_langs-c: `.swift`
//! files open with the built-in `sourcekit-lsp` (fake server, `languageId` `swift`); a real one
//! (skipped when not runnable) reports an error, hovers, goes to a definition across a SwiftPM
//! package's files and completes a member.

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

/// apple_langs-b: a `.swift` file is Swift; `"` closes outside strings and is
/// typed over, `'` doesn't pair, Enter after `{` indents, Cmd+/ uses `//`.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_swift_edits_by_the_swift_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.swift", "func f() {\n}\nlet s = \n")]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.swift"));
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::Swift);
    let parsed = |a: &crate::app::App| a.active_buffer().unwrap().syntax.tree().is_some();
    assert!(app.settle(parsed).await);
    let text = |a: &crate::app::App| a.active_buffer().unwrap().rope.to_string();
    // Inside the braces: Enter indents.
    app.press(K::End, M::NONE);
    app.press(K::Enter, M::NONE);
    app.type_text("g()");
    assert_eq!(text(&app), "func f() {\n    g()\n}\nlet s = \n");
    // `"` closes and the closing quote is typed over.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    app.type_text("\"a");
    assert_eq!(text(&app), "func f() {\n    g()\n}\nlet s = \"a\"\n");
    app.type_text("\"");
    assert_eq!(text(&app), "func f() {\n    g()\n}\nlet s = \"a\"\n");
    // `'` has no pair in Swift.
    app.type_text("'");
    assert_eq!(text(&app), "func f() {\n    g()\n}\nlet s = \"a\"'\n");
    // Comment Line uses `//`.
    app.press(K::Char('/'), crate::milestone9_tests::cmd());
    assert_eq!(text(&app), "func f() {\n    g()\n}\n// let s = \"a\"'\n");
    app.shutdown_servers().await;
}

/// apple_langs-b: auto-close reads Swift's string scopes from the tree: inside `\(…)` (also
/// right after the `\(`) a `"` closes as in code; in a string's text it doesn't.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_swift_quote_closes_in_interpolation_not_in_string_text() {
    let dir = tempfile::tempdir().unwrap();
    let src = "let q = \"a \\(b + c) d\"\nlet p = \"a \\( b) d\"\nlet r = \"a - b\"\n";
    let root = project(dir.path(), &[("a.swift", src)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.swift"));
    let parsed = |a: &crate::app::App| a.active_buffer().unwrap().syntax.tree().is_some();
    assert!(app.settle(parsed).await);
    let text = |a: &crate::app::App| a.active_buffer().unwrap().rope.to_string();
    // Types `"` at `lefts` from the end of the current line, then waits for the re-parse.
    async fn quote_at(app: &mut crate::app::App, lefts: usize) {
        app.press(K::End, M::NONE);
        for _ in 0..lefts {
            app.press(K::Left, M::NONE);
        }
        app.type_text("\"");
        assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    }
    // After `+` in `\(b + c)`: code, so `"` closes.
    quote_at(&mut app, 6).await;
    assert_eq!(text(&app).lines().next(), Some("let q = \"a \\(b +\"\" c) d\""));
    // Right after `\(`: code too.
    app.press(K::Down, M::NONE);
    quote_at(&mut app, 6).await;
    assert_eq!(text(&app).lines().nth(1), Some("let p = \"a \\(\"\" b) d\""));
    // After `-` in `"a - b"`: string text, so `"` is typed alone.
    app.press(K::Down, M::NONE);
    quote_at(&mut app, 3).await;
    assert_eq!(text(&app).lines().nth(2), Some("let r = \"a -\" b\""));
    app.shutdown_servers().await;
}

/// apple_langs-c: a `.swift` file starts the built-in `sourcekit-lsp` (a fake server), with
/// `languageId` `swift`.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_swift_files_start_the_sourcekit_lsp_server() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.swift", "let x = 1\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.swift"));
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::Swift);
    assert!(app.settle(|a| encoding(a, Language::Swift).is_some()).await);
    assert_eq!(fake.launches(), ["sourcekit-lsp"]);
    assert_eq!(fake.commands(), ["sourcekit-lsp"], "found on PATH: run as named");
    assert!(app.settle(|_| fake.first("textDocument/didOpen").is_some()).await);
    let open_msg = fake.first("textDocument/didOpen").unwrap();
    assert_eq!(open_msg["params"]["textDocument"]["languageId"], "swift");
    assert_eq!(open_msg["params"]["textDocument"]["uri"], uri(&root.join("a.swift")));
    app.shutdown_servers().await;
}

/// apple_langs-c: when `sourcekit-lsp` isn't on PATH and `xcrun --find` locates it (the
/// launcher's `locate`), that path is launched under the same server name and the tool log says
/// so; other servers' commands are never located.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_sourcekit_lsp_off_path_runs_what_xcrun_found() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.swift", "let x = 1\n"), ("b.c", "int x;\n")]);
    let found = "/Xcode/usr/bin/sourcekit-lsp";
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        located: Some(found.into()),
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.swift"));
    assert!(app.settle(|a| encoding(a, Language::Swift).is_some()).await);
    open(&mut app, &root.join("b.c"));
    assert!(app.settle(|a| encoding(a, Language::C).is_some()).await);
    assert_eq!(fake.launches(), ["sourcekit-lsp", "clangd"]);
    assert_eq!(fake.commands(), [found, "clangd"]);
    let tool = crate::logs::Tool::Server("sourcekit-lsp".into());
    let note = format!("Not on PATH: using {found} (from xcrun)");
    assert!(app.logs.texts(&tool).contains(&note), "{:?}", app.logs.texts(&tool));
    app.shutdown_servers().await;
}

/// apple_langs-c: a missing `sourcekit-lsp` (not on PATH, not found by xcrun) gives the usual
/// not-found error naming `[lsp.swift]`, once.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_missing_sourcekit_lsp_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.swift", "let x = 1\n"), ("b.swift", "let y = 2\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        missing: vec!["sourcekit-lsp".into()],
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.swift"));
    assert!(app.settle(|a| a.message.is_some()).await);
    let message = app.message.take().unwrap();
    assert_eq!(message.level, crate::app::Level::Error);
    assert_eq!(
        message.text,
        "Language server sourcekit-lsp not found: install it, or set [lsp.swift] command in .workspace"
    );
    open(&mut app, &root.join("b.swift"));
    idle(&mut app).await;
    assert!(app.message.is_none(), "reported once");
    assert_eq!(fake.launches(), ["sourcekit-lsp"]);
    app.shutdown_servers().await;
}

/// Whether `sourcekit-lsp` can be run here: on PATH, or (macOS) found by `xcrun`.
fn sourcekit_runs() -> bool {
    which("sourcekit-lsp").is_some()
        || (cfg!(target_os = "macos")
            && std::process::Command::new("xcrun")
                .args(["--find", "sourcekit-lsp"])
                .output()
                .is_ok_and(|o| o.status.success()))
}

const PACKAGE_SWIFT: &str = "// swift-tools-version:5.5\nimport PackageDescription\nlet package = Package(\n  name: \"App\",\n  targets: [.executableTarget(name: \"App\", path: \"Sources/App\")]\n)\n";

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_sourcekit_lsp_diagnostics() {
    if !sourcekit_runs() {
        eprintln!("apple_langs_real_sourcekit_lsp_diagnostics: skipped, sourcekit-lsp isn't runnable");
        return;
    }
    // A single file, no package: sourcekit-lsp still reports this file's errors.
    let fix = |app: &mut crate::app::App| {
        app.press(K::Down, M::NONE);
        app.press(K::End, M::NONE);
        app.press(K::Left, M::NONE);
        app.press(K::Char('d'), crate::milestone9_tests::cmd());
        app.type_text("0");
    };
    real_diagnostics(
        ("a.swift", "func f() -> Int {\n  return undefined_thing;\n}\n"),
        "undefined_thing",
        |_| {},
        fix,
        "sourcekit-lsp",
    )
    .await;
}

/// A SwiftPM package of one target and two files: hover and go to definition across files.
#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_sourcekit_lsp_hover_and_definition() {
    if !sourcekit_runs() {
        eprintln!("apple_langs_real_sourcekit_lsp_hover_and_definition: skipped, sourcekit-lsp isn't runnable");
        return;
    }
    real_hover_and_definition(
        &[
            ("Sources/App/main.swift", "print(answer(1))\n"),
            (
                "Sources/App/util.swift",
                "// The answer.\nfunc answer(_ x: Int) -> Int {\n  return 41 + x\n}\n",
            ),
            ("Package.swift", PACKAGE_SWIFT),
        ],
        "answer",
        &["answer", "Int"],
        "Sources/App/util.swift",
        1,
        |_| {},
        "sourcekit-lsp",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apple_langs_real_sourcekit_lsp_completion() {
    if !sourcekit_runs() {
        eprintln!("apple_langs_real_sourcekit_lsp_completion: skipped, sourcekit-lsp isn't runnable");
        return;
    }
    real_completion(
        (
            "a.swift",
            "struct Point {\n  var xcoord = 0\n  var ycoord = 0\n}\nfunc f() {\n  let pt = Point()\n  pt\n}\n",
        ),
        "pt",
        "ycoord",
        |_| {},
        "sourcekit-lsp",
    )
    .await;
}
