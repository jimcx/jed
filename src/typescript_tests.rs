//! End-to-end tests for the TypeScript and JavaScript task (`typescript`). typescript-a: `.js` /
//! `.jsx` files get their language (and status-bar name), Cmd+/ comments with `//`, backtick
//! closes and is typed over, a quote closes inside a template string's `${…}` but not in its
//! text, and the files open coloured.

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
