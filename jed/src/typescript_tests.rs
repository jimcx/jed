//! End-to-end tests for the TypeScript and JavaScript task (`typescript`). typescript-a: `.js` /
//! `.jsx` files get their language (and status-bar name), Cmd+/ comments with `//`, backtick
//! closes and is typed over, a quote closes inside a template string's `${…}` but not in its
//! text, and the files open coloured. typescript-b: the same for `.ts`, `.d.ts` and `.tsx`.
//! typescript-c: the built-in `typescript-language-server` (fake: one server for the four languages with their
//! `languageId`s, the workspace's `node_modules` copy and `tsserver.path`, format options; real, skipped with a
//! note without a runnable server and a TypeScript with `tsserver.js`:
//! diagnostics in a lone file, hover / definition / completion / references / rename / a quick fix in a project).

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{open, open_app, project};
use crate::syntax::Language;

fn parsed(a: &crate::app::App) -> bool {
    a.active_buffer().unwrap().syntax.tree().is_some()
}

fn text(a: &crate::app::App) -> String {
    a.active_buffer().unwrap().rope.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn typescript_js_files_open_coloured_and_edit_by_the_javascript_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[
            ("a.js", "function f() {\n}\nlet s = \n"),
            ("b.jsx", "const e = <a b=\"c\">{d}</a>;\n"),
        ],
    );
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.js"));
    let lang = app.active_buffer().unwrap().syntax.language();
    assert_eq!((lang, lang.name()), (Language::JavaScript, "JavaScript"));
    assert!(app.settle(parsed).await);
    // Enter after `{` indents.
    app.press(K::End, M::NONE);
    app.press(K::Enter, M::NONE);
    app.type_text("g()");
    assert_eq!(text(&app), "function f() {\n    g()\n}\nlet s = \n");
    // Backtick closes and the closing one is typed over.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    app.type_text("`a");
    assert_eq!(text(&app), "function f() {\n    g()\n}\nlet s = `a`\n");
    app.type_text("`");
    assert_eq!(text(&app), "function f() {\n    g()\n}\nlet s = `a`\n");
    // Comment Line uses `//`.
    app.press(K::Char('/'), crate::milestone9_tests::cmd());
    assert_eq!(text(&app), "function f() {\n    g()\n}\n// let s = `a`\n");
    // A quote inside the comment doesn't close. `tree()` is `Some` only while it matches the text,
    // so wait until the scope before the end of the comment line (the text's last char is `\n`)
    // is read from a tree of the toggled text.
    let in_comment = |a: &crate::app::App| {
        let b = a.active_buffer().unwrap();
        let end = crate::buffer::pos::ByteIdx(b.rope.len() - 1);
        b.syntax.scope_before(end) == Some(crate::syntax::Scope::Comment)
    };
    assert!(app.settle(in_comment).await);
    app.press(K::End, M::NONE);
    app.type_text("'");
    assert_eq!(text(&app), "function f() {\n    g()\n}\n// let s = `a`'\n");

    open(&mut app, &root.join("b.jsx"));
    let lang = app.active_buffer().unwrap().syntax.language();
    assert_eq!((lang, lang.name()), (Language::JavaScriptReact, "JavaScript JSX"));
    assert!(app.settle(parsed).await);
    app.press(K::Char('/'), crate::milestone9_tests::cmd());
    assert_eq!(text(&app), "// const e = <a b=\"c\">{d}</a>;\n");
    // `<` isn't auto-closed in JSX.
    app.press(K::End, M::NONE);
    app.type_text("<");
    assert_eq!(text(&app), "// const e = <a b=\"c\">{d}</a>;<\n");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn typescript_quote_closes_in_template_substitution_not_in_text() {
    let dir = tempfile::tempdir().unwrap();
    let src = "let a = `x ${b + c} y`;\nlet d = `p - q`;\n";
    let root = project(dir.path(), &[("a.js", src)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.js"));
    assert!(app.settle(parsed).await);
    async fn quote_at(app: &mut crate::app::App, lefts: usize) {
        app.press(K::End, M::NONE);
        for _ in 0..lefts {
            app.press(K::Left, M::NONE);
        }
        app.type_text("\"");
        assert!(app.settle(parsed).await);
    }
    // After `+` inside `${…}`: code, so `"` closes.
    quote_at(&mut app, 7).await;
    assert_eq!(text(&app).lines().next(), Some("let a = `x ${b +\"\" c} y`;"));
    // After `-` in template text: a string, so `"` is typed alone.
    app.press(K::Down, M::NONE);
    quote_at(&mut app, 4).await;
    assert_eq!(text(&app).lines().nth(1), Some("let d = `p -\" q`;"));
    app.shutdown_servers().await;
}

/// typescript-b: `.ts`, `.d.ts` and `.tsx` get their language and name, Cmd+/ comments with `//`,
/// backtick closes, `<` closes in neither, and each opens parsed.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_ts_and_tsx_files_open_by_the_javascript_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[
            ("a.ts", "interface I {\n}\nlet s = \n"),
            ("b.d.ts", "declare const x: number;\n"),
            ("c.tsx", "const C = (p: Props) => <a b=\"c\">{p.x}</a>;\n"),
        ],
    );
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.ts"));
    let lang = app.active_buffer().unwrap().syntax.language();
    assert_eq!((lang, lang.name()), (Language::TypeScript, "TypeScript"));
    assert!(app.settle(parsed).await);
    app.press(K::End, M::NONE);
    app.press(K::Enter, M::NONE);
    app.type_text("a: T");
    assert_eq!(text(&app), "interface I {\n    a: T\n}\nlet s = \n");
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    app.type_text("`a");
    assert_eq!(text(&app), "interface I {\n    a: T\n}\nlet s = `a`\n");
    app.press(K::Char('/'), crate::milestone9_tests::cmd());
    assert_eq!(text(&app), "interface I {\n    a: T\n}\n// let s = `a`\n");

    open(&mut app, &root.join("b.d.ts"));
    let lang = app.active_buffer().unwrap().syntax.language();
    assert_eq!((lang, lang.name()), (Language::TypeScript, "TypeScript"));
    assert!(app.settle(parsed).await);

    open(&mut app, &root.join("c.tsx"));
    let lang = app.active_buffer().unwrap().syntax.language();
    assert_eq!((lang, lang.name()), (Language::TypeScriptReact, "TypeScript JSX"));
    assert!(app.settle(parsed).await);
    app.press(K::End, M::NONE);
    app.type_text("<");
    assert_eq!(text(&app), "const C = (p: Props) => <a b=\"c\">{p.x}</a>;<\n");
    app.shutdown_servers().await;
}

fn started(a: &crate::app::App, lang: Language) -> bool {
    crate::milestone9_tests::encoding(a, lang).is_some()
}

/// typescript-c: `.js`, `.jsx`, `.ts` and `.tsx` files in one project start ONE built-in
/// `typescript-language-server --stdio` (a fake server), each opened with its own `languageId`;
/// the root is the workspace root even for a file in a subfolder with its own `tsconfig.json`.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_four_languages_share_one_typescript_language_server() {
    use crate::milestone9_tests::uri;
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("a.js", "let a = 1;\n"),
        ("b.jsx", "let b = <a/>;\n"),
        ("sub/tsconfig.json", "{}\n"),
        ("sub/c.ts", "let c: number = 1;\n"),
        ("sub/d.tsx", "let d = <a/>;\n"),
    ];
    let root = project(dir.path(), &files);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    let langs = [
        ("a.js", Language::JavaScript, "javascript"),
        ("b.jsx", Language::JavaScriptReact, "javascriptreact"),
        ("sub/c.ts", Language::TypeScript, "typescript"),
        ("sub/d.tsx", Language::TypeScriptReact, "typescriptreact"),
    ];
    for (file, lang, id) in langs {
        open(&mut app, &root.join(file));
        assert!(app.settle(|a| started(a, lang)).await, "{file}");
        let u = uri(&root.join(file));
        assert!(app.settle(|_| fake.document(&u).is_some()).await, "{file}");
        assert_eq!(fake.document(&u).unwrap().language, id);
    }
    assert_eq!(fake.launches(), ["typescript-language-server"]);
    assert_eq!(fake.commands(), ["typescript-language-server"]);
    let init = fake.first("initialize").unwrap();
    assert_eq!(init["params"]["rootUri"], uri(&root));
    assert!(init["params"]["initializationOptions"].is_null(), "{init}");
    app.shutdown_servers().await;
}

/// typescript-c: a workspace with its own `node_modules/.bin/typescript-language-server` and
/// `node_modules/typescript/lib` runs that server and sends `tsserver.path`.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_prefers_the_workspaces_own_server_and_typescript() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[
            ("a.ts", "let a = 1;\n"),
            ("node_modules/.bin/typescript-language-server", "#!/bin/sh\n"),
            ("node_modules/typescript/lib/tsserver.js", "\n"),
        ],
    );
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.ts"));
    assert!(app.settle(|a| started(a, Language::TypeScript)).await);
    assert_eq!(fake.launches(), ["typescript-language-server"]);
    let bin = root.join("node_modules/.bin/typescript-language-server");
    assert_eq!(fake.commands(), [bin.to_string_lossy().into_owned()]);
    let init = fake.first("initialize").unwrap();
    let lib = root.join("node_modules/typescript/lib");
    assert_eq!(
        init["params"]["initializationOptions"],
        serde_json::json!({ "tsserver": { "path": lib } })
    );
    let tool = crate::logs::Tool::Server("typescript-language-server".into());
    let log = app.logs.texts(&tool);
    assert!(log.iter().any(|l| l.starts_with("Using the workspace's ")), "{log:?}");
    app.shutdown_servers().await;
}

/// typescript-c: Format Document sends jed's tab size and `insertSpaces` for a `.ts` file (the
/// server's own defaults ignore them).
#[tokio::test(flavor = "multi_thread")]
async fn typescript_format_sends_the_tab_size() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("a.ts", "if (a) {\n  b();\n}\n")];
    let (mut app, fake, _) = crate::milestone9_tests::lsp_project(dir.path(), &files, true).await;
    if cfg!(target_os = "macos") {
        app.press(K::Char('f'), M::SHIFT | M::ALT);
    } else {
        app.press(K::Char('i'), M::CONTROL | M::SHIFT);
    }
    assert!(
        app.settle(|_| !fake.requests("textDocument/formatting").is_empty())
            .await
    );
    let params = fake.requests("textDocument/formatting").pop().unwrap();
    assert_eq!(params["options"]["insertSpaces"], true);
    assert!(params["options"]["tabSize"].as_u64().is_some_and(|n| n > 0), "{params}");
    app.shutdown_servers().await;
}

// Real `typescript-language-server` (needs `typescript` beside it): skipped with a note when it isn't runnable.

const TSCONFIG: &str = "{ \"compilerOptions\": { \"strict\": true, \"target\": \"es2020\", \"module\": \"esnext\", \"moduleResolution\": \"bundler\" } }\n";
const SERVER_NAME: &str = "typescript-language-server";

/// The TypeScript for the server to use, or a skip note: a `node_modules/typescript/lib` with a `tsserver.js`
/// beside the installed server (TypeScript 7 has no `tsserver.js`: typescript-language-server 6 can't use it).
/// Tests pass it as `tsserver.path`, as jed does for a workspace's own copy.
fn real_setup(test: &str) -> Option<impl Fn(&mut crate::app::App) + Clone> {
    use crate::milestone9_tests::which;
    let found = (|| {
        let server = std::fs::canonicalize(which(SERVER_NAME)?).ok()?;
        server
            .ancestors()
            .map(|d| d.join("node_modules/typescript/lib"))
            .find(|l| l.join("tsserver.js").is_file())
    })();
    let Some(lib) = found else {
        eprintln!("{test}: skipped, {SERVER_NAME} or a TypeScript with tsserver.js isn't installed");
        return None;
    };
    Some(move |app: &mut crate::app::App| {
        app.lsp.builtins.typescript_language_server.init_options =
            Some(serde_json::json!({ "tsserver": { "path": lib } }));
    })
}

/// A lone `.ts` file (no `tsconfig.json`: an inferred project) gets a type error, which goes
/// when fixed.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_real_server_diagnostics_in_a_lone_file() {
    let Some(configure) = real_setup("typescript_real_server_diagnostics_in_a_lone_file") else {
        return;
    };
    let fix = |app: &mut crate::app::App| {
        app.press(K::Home, M::NONE);
        app.press(K::End, M::SHIFT);
        app.type_text("const x: number = 0;");
    };
    crate::milestone9_tests::real_diagnostics(("a.ts", "const x: number = \"a\";\n"), "x", configure, fix, SERVER_NAME)
        .await;
}

/// A TypeScript project: hover and go to definition across files.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_real_server_hover_and_definition() {
    let Some(configure) = real_setup("typescript_real_server_hover_and_definition") else {
        return;
    };
    crate::milestone9_tests::real_hover_and_definition(
        &[
            ("a.ts", "import { answer } from \"./b\";\nconsole.log(answer(1));\n"),
            (
                "b.ts",
                "// The answer.\nexport function answer(x: number): number {\n  return 41 + x;\n}\n",
            ),
            ("tsconfig.json", TSCONFIG),
        ],
        "answer",
        &["answer"],
        "b.ts",
        1,
        configure,
        SERVER_NAME,
    )
    .await;
}

/// Completion after `.` lists a member and inserts it.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_real_server_completion() {
    let Some(configure) = real_setup("typescript_real_server_completion") else {
        return;
    };
    crate::milestone9_tests::real_completion(
        ("a.ts", "const pt = { xcoord: 1, ycoord: 2 };\npt\n"),
        "pt",
        "xcoord",
        configure,
        SERVER_NAME,
    )
    .await;
}

/// References and rename across two files of a project, from the definition (renaming at an import's use
/// renames only that file's binding: `import { answer as solution }`).
#[tokio::test(flavor = "multi_thread")]
async fn typescript_real_server_references_and_rename() {
    let Some(configure) = real_setup("typescript_real_server_references_and_rename") else {
        return;
    };
    crate::milestone9_tests::real_references_and_rename(
        &[
            (
                "b.ts",
                "export function answer(x: number): number {\n  return 41 + x;\n}\n",
            ),
            ("a.ts", "import { answer } from \"./b\";\nconsole.log(answer(1));\n"),
            ("tsconfig.json", TSCONFIG),
        ],
        "answer",
        "solution",
        &[
            ("a.ts", "import { solution } from \"./b\";\nconsole.log(solution(1));\n"),
            (
                "b.ts",
                "export function solution(x: number): number {\n  return 41 + x;\n}\n",
            ),
        ],
        configure,
        SERVER_NAME,
    )
    .await;
}

/// A quick fix: Cmd+. on a misspelled name lists "Change spelling to 'total'" (tsserver answers it as a
/// `command` carrying the edit, which jed runs with `workspace/executeCommand`; the server then sends
/// `workspace/applyEdit`) and Enter applies it.
#[tokio::test(flavor = "multi_thread")]
async fn typescript_real_server_quick_fix() {
    use crate::milestone9_tests::{REAL_SERVERS, children_settled, marks, put_cursor, settle_for};
    let Some(configure) = real_setup("typescript_real_server_quick_fix") else {
        return;
    };
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let text = "const total = 1;\nconsole.log(totl);\n";
    let root = project(dir.path(), &[("a.ts", text), ("tsconfig.json", TSCONFIG)]);
    let path = root.join("a.ts");
    let mut app = open_app(&root, std::sync::Arc::new(crate::lsp::ProcessLauncher));
    configure(&mut app);
    open(&mut app, &path);
    let has = |a: &crate::app::App| marks(a, &path).iter().any(|m| m.0 == "totl");
    assert!(settle_for(&mut app, 60, has).await, "{:?}", marks(&app, &path));
    let at = text.rfind("totl").unwrap() + 1;
    let mut term = crate::milestone9_tests::terminal();
    let t = std::time::Instant::now();
    let mut listed = false;
    while !listed && t.elapsed() < std::time::Duration::from_secs(60) {
        put_cursor(&mut app, at);
        app.press(K::Char('.'), crate::milestone9_tests::cmd());
        listed = settle_for(&mut app, 5, |a| a.code_actions.popup.is_some()).await;
        if !listed {
            app.press(K::Esc, M::NONE);
        }
    }
    assert!(listed, "message {:?}", app.message.as_ref().map(|m| &m.text));
    crate::milestone9_tests::draw(&mut term, &mut app);
    let r = app.hits.code_actions.expect("list drawn").rect;
    let buf = term.backend().buffer();
    let rows: Vec<String> = (r.y + 1..r.y + r.height - 1)
        .map(|y| {
            (r.x + 1..r.x + r.width - 1)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    let row = rows
        .iter()
        .position(|l| l.contains("Change spelling to 'total'"))
        .unwrap_or_else(|| panic!("{rows:?}"));
    for _ in 0..row {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Enter, M::NONE);
    let fixed = |a: &crate::app::App| a.active_buffer().unwrap().rope.to_string().contains("log(total)");
    assert!(
        settle_for(&mut app, 30, fixed).await,
        "{:?}; message {:?}",
        app.active_buffer().unwrap().rope.to_string(),
        app.message.as_ref().map(|m| &m.text)
    );
    app.press(K::Char('q'), crate::milestone9_tests::cmd());
    app.shutdown_servers().await;
    if let Some(n) = children_settled(SERVER_NAME, 0) {
        assert_eq!(n, 0, "no {SERVER_NAME} left");
    }
}
