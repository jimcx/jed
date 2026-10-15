//! Milestone 8 end to end (8a, the `syntax` module, no colours yet): a `.cpp` file opened through
//! the app is parsed off the UI thread and the status bar shows its language mode ("C++",
//! "Python", "Plain Text"); `.workspace` `[files] associations` pick the language (also after a
//! live reload of `.workspace`) and a shebang does for a file without an extension; typing,
//! multi-cursor edits (one `InputEdit` per change; tens of thousands of cursors drop the tree for a
//! fresh background parse),
//! paste, undo and redo keep the tree equal to a fresh parse of the text, also when they happen
//! while the first parse is still running; a file over `[settings] syntax_max_bytes` has no tree
//! but still shows its language.
//!
//! Perf (asserted in release only: `cargo test --release milestone -- --nocapture`): on a
//! generated 1 MB C++ file the first parse runs in the background, and a keystroke plus its
//! incremental re-parse plus a frame stays well inside a 16 ms frame, also at 63k cursors.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::milestone_tests::test_app;
use crate::syntax::Language;

/// A keystroke in the 1 MB C++ file, its incremental re-parse and a frame, on average (release;
/// 3.5-5 ms on a slow x86 (i7-2600K), the re-parse 2-4 ms of it).
const KEYSTROKE_BUDGET: Duration = Duration::from_millis(8);
/// The slowest keystroke (~12 ms there, an unbalanced bracket): a re-parse stops after
/// `syntax::INLINE_TIME` (12 ms) and continues in the background.
const KEYSTROKE_WORST: Duration = Duration::from_millis(16);
/// The same in debug builds: only catches a full re-parse sneaking onto the UI thread.
const KEYSTROKE_DEBUG_BOUND: Duration = Duration::from_millis(400);
/// The first parse of the 1 MB file, off the UI thread (release).
const FIRST_PARSE_BUDGET: Duration = Duration::from_millis(1500);

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn write(root: &Path, rel: &str, text: impl AsRef<[u8]>) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

/// An app on a workspace with `.workspace` text `config`, with `rel` (holding `text`) open.
fn open_app(dir: &tempfile::TempDir, config: &str, rel: &str, text: &str) -> (App, PathBuf) {
    let root = crate::workspace::normalize(dir.path());
    write(&root, ".workspace", config);
    let path = write(&root, rel, text);
    let mut app = test_app(&root, Buffer::open(&path).unwrap());
    app.test_start();
    (app, root)
}

fn parsed(app: &App) -> bool {
    app.active_buffer().is_some_and(|b| b.syntax.tree().is_some())
}

fn status_line(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..120).map(|x| buf[(x, 9)].symbol()).collect()
}

/// The active buffer's tree equals a fresh parse of its text (structure and positions).
fn assert_tree_current(app: &App) {
    let buf = app.active_buffer().unwrap();
    let tree = buf.syntax.tree().expect("a current tree");
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&buf.syntax.language().grammar().unwrap()).unwrap();
    let fresh = parser.parse(buf.rope.to_string(), None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
    let (mut a, mut b) = (tree.walk(), fresh.walk());
    loop {
        let (x, y) = (a.node(), b.node());
        assert_eq!(
            (x.kind(), x.byte_range(), x.start_position(), x.end_position()),
            (y.kind(), y.byte_range(), y.start_position(), y.end_position())
        );
        // Depth-first, in step.
        let down = a.goto_first_child();
        assert_eq!(down, b.goto_first_child());
        if down {
            continue;
        }
        loop {
            let next = a.goto_next_sibling();
            assert_eq!(next, b.goto_next_sibling());
            if next {
                break;
            }
            let up = a.goto_parent();
            assert_eq!(up, b.goto_parent());
            if !up {
                return;
            }
        }
    }
}

/// Waits for the tree (a re-parse on the UI thread can run out of time on a loaded machine and
/// finish in the background), then checks it.
async fn current(app: &mut App) {
    assert!(app.settle(parsed).await);
    assert_tree_current(app);
}

/// About `size` bytes of varied C++: classes, templates, functions, comments, strings.
pub fn generated_cpp(size: usize) -> String {
    let mut out = String::from("#include <vector>\n#include <string>\n\nnamespace gen {\n\n");
    let mut i = 0;
    while out.len() < size {
        out.push_str(&format!(
            "/// Holds item {i}.\n\
             template <typename T>\n\
             class Item{i} : public Base {{\n\
             public:\n\
             \x20   explicit Item{i}(T value) : value_(value), name_(\"item {i} 界\") {{}}\n\
             \x20   T get() const {{ return value_; }}\n\
             \x20   int count(const std::vector<int>& xs) const {{\n\
             \x20       int n = 0;\n\
             \x20       for (auto x : xs) {{\n\
             \x20           if (x > {i} && x % 3 == 0) {{ n += x * 2; }} // odd ones\n\
             \x20       }}\n\
             \x20       return n;\n\
             \x20   }}\n\
             private:\n\
             \x20   T value_;\n\
             \x20   std::string name_;\n\
             }};\n\n"
        ));
        i += 1;
    }
    out.push_str("} // namespace gen\n");
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_cpp_file_is_parsed_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "", "src/main.cpp", "int main() { return 0; }\n");
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::Cpp);
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);
    let status = status_line(&mut app);
    assert!(status.trim_end().ends_with("UTF-8   LF   C++"), "{status:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_language_detection() {
    let dir = tempfile::tempdir().unwrap();
    let config = "[files]\nassociations = { \"*.bzl\" = \"python\", \"*.h\" = \"c\", \"vendor/**\" = \"plaintext\" }\n";
    let (mut app, root) = open_app(&dir, config, "SConstruct", "env = Environment()\n");
    let lang = |app: &App, path: &Path| {
        let id = app.buffers.find_path(path).unwrap();
        app.buffers.get(id).unwrap().syntax.language()
    };
    assert_eq!(lang(&app, &root.join("SConstruct")), Language::Python);
    assert!(app.settle(parsed).await);
    assert!(status_line(&mut app).trim_end().ends_with("LF   Python"));

    let files = [
        ("defs.bzl", "x = 1\n", Language::Python),
        ("api.h", "int f(void);\n", Language::C),
        ("other.hpp", "int f();\n", Language::Cpp),
        ("vendor/lib.c", "int f(void);\n", Language::PlainText),
        ("tool", "#!/usr/bin/env -S python3 -u\nprint(1)\n", Language::Python),
        ("notes.txt", "hello\n", Language::PlainText),
    ];
    for (rel, text, _) in files {
        let path = write(&root, rel, text);
        app.test_open(Buffer::open(&path).unwrap());
    }
    for (rel, _, want) in files {
        assert_eq!(lang(&app, &root.join(rel)), want, "{rel}");
    }
    assert!(status_line(&mut app).trim_end().ends_with("LF   Plain Text"));

    // A live `.workspace` change re-detects open buffers.
    write(
        &root,
        ".workspace",
        "[files]\nassociations = { \"*.txt\" = \"python\" }\n",
    );
    let notes = root.join("notes.txt");
    assert!(app.settle(|a| lang(a, &notes) == Language::Python).await);
    assert_eq!(lang(&app, &root.join("api.h")), Language::Cpp);
    assert!(app.settle(parsed).await, "parsed as Python");
    assert_tree_current(&app);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_edits_keep_the_tree_current() {
    let dir = tempfile::tempdir().unwrap();
    let text = "int add(int a, int b) {\n    return a + b;\n}\n\nint twice(int x) { return add(x, x); }\n";
    let (mut app, _) = open_app(&dir, "", "a.c", text);
    assert!(app.settle(parsed).await);

    // Typing, Enter, deleting.
    app.type_text("/* é界 */ ");
    app.press(K::Enter, M::NONE);
    app.press(K::Backspace, M::NONE);
    current(&mut app).await;

    // Every `int` becomes `long` at once (several changes in one set), then undo / redo.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("5");
    app.press(K::Enter, M::NONE);
    app.press(K::Right, M::SHIFT);
    app.press(K::Right, M::SHIFT);
    app.press(K::Right, M::SHIFT);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    assert!(app.group().active_tab().unwrap().view.selections.len() > 3);
    app.type_text("long");
    current(&mut app).await;
    assert!(
        app.active_buffer()
            .unwrap()
            .rope
            .to_string()
            .contains("long twice(long x)")
    );
    app.press(K::Char('z'), cmd());
    current(&mut app).await;
    app.press(K::Char('z'), cmd() | M::SHIFT);
    current(&mut app).await;

    // A paste.
    app.press(K::Esc, M::NONE);
    app.handle_test_event(crossterm::event::Event::Paste("\nstruct S { int s; };\r\n".into()));
    current(&mut app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_edits_during_the_first_parse() {
    let dir = tempfile::tempdir().unwrap();
    let text = generated_cpp(if cfg!(debug_assertions) { 100_000 } else { 400_000 });
    let (mut app, _) = open_app(&dir, "", "big.cc", &text);
    // Typed before the first parse can have finished (it started in `test_start`).
    app.type_text("int x; ");
    app.press(K::Enter, M::NONE);
    app.press(K::Char('z'), cmd());
    app.type_text("// done");
    // No `AppEvent` has been handled yet, so the first parse's result can't have been taken.
    assert!(!parsed(&app), "typed while the first parse runs");
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);
    assert!(
        app.active_buffer()
            .unwrap()
            .rope
            .to_string()
            .starts_with("// done#include")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_no_tree_past_the_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(
        &dir,
        "[settings]\nsyntax_max_bytes = 1000\n",
        "big.py",
        &"x = 1\n".repeat(200),
    );
    app.pump();
    let buf = app.active_buffer().unwrap();
    assert!(buf.syntax.too_large());
    assert!(buf.syntax.tree().is_none());
    assert!(status_line(&mut app).trim_end().ends_with("LF   Python"));

    // A small file in the same workspace is parsed; deleting below the limit parses the big one.
    app.test_open(Buffer::open(&write(&root, "small.py", "x = 1\n")).unwrap());
    assert!(
        app.settle(|a| a.active_buffer().unwrap().name() == "small.py" && parsed(a))
            .await
    );
    app.press(K::Char('w'), cmd());
    assert_eq!(app.active_buffer().unwrap().name(), "big.py");
    app.press(K::Char('a'), cmd());
    app.press(K::Backspace, M::NONE);
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);
}

/// The 1 MB C++ file: first parse in the background, then typing with incremental re-parses.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_typing_in_a_one_mb_cpp_file() {
    let dir = tempfile::tempdir().unwrap();
    // Debug builds use a quarter of it: still catches a full re-parse per key.
    let text = generated_cpp(if cfg!(debug_assertions) { 250_000 } else { 1_000_000 });
    let (mut app, _) = open_app(&dir, "", "big.cpp", &text);
    let t = Instant::now();
    assert!(app.settle(parsed).await);
    let first = t.elapsed();
    println!("first parse of {} bytes: {first:?}", text.len());
    assert!(
        !app.active_buffer()
            .unwrap()
            .syntax
            .tree()
            .unwrap()
            .root_node()
            .has_error()
    );

    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    // Somewhere in the middle, inside a function body.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text(&(text.lines().count() / 2).to_string());
    app.press(K::Enter, M::NONE);
    app.press(K::End, M::NONE);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut inline = 0;
    let keys = "  int y = count(xs); // ok";
    for c in keys.chars() {
        let t = Instant::now();
        app.press(K::Char(c), M::NONE);
        app.pump(); // the incremental re-parse, as the event loop's turn
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let e = t.elapsed();
        // Re-parsed on the UI thread, unless it hit `INLINE_TIME`: then wait for the
        // background parse, so the next key starts from a current tree.
        if parsed(&app) {
            inline += 1;
        } else {
            assert!(app.settle(parsed).await);
        }
        worst = worst.max(e);
        total += e;
        assert!(e < KEYSTROKE_DEBUG_BOUND, "key {c:?} took {e:?}");
    }
    let n = keys.chars().count() as u32;
    let avg = total / n;
    println!("typing: {n} keys, avg {avg:?}, worst {worst:?}, {inline} re-parsed on the UI thread");
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);

    // `/*` at the top comments out the whole file: too slow for the UI thread, so the re-parse
    // stops and finishes in the background.
    app.press(K::Home, cmd());
    let t = Instant::now();
    app.type_text("/*");
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let comment = t.elapsed();
    println!("typing /* at the top: {comment:?}");
    assert!(!parsed(&app), "re-parsing in the background");
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);
    app.press(K::Char('z'), cmd());
    assert!(app.settle(parsed).await);

    // Every `t` (~63k cursors): too wide a change set to edit the tree with, so it is dropped (and
    // freed off-thread) and a fresh parse runs in the background.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("7");
    app.press(K::Enter, M::NONE);
    app.press(K::Right, M::SHIFT);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    let count = app.group().active_tab().unwrap().view.selections.len();
    assert!(count > text.len() / 20, "{count}");
    let t = Instant::now();
    app.press(K::Char('X'), M::SHIFT);
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let many = t.elapsed();
    println!("typing at {count} cursors (re-parse in the background): {many:?}");
    let t = Instant::now();
    assert!(app.settle(parsed).await);
    println!("background re-parse after it: {:?}", t.elapsed());
    assert_tree_current(&app);

    if !cfg!(debug_assertions) {
        assert!(first < FIRST_PARSE_BUDGET, "first parse took {first:?}");
        assert!(avg < KEYSTROKE_BUDGET, "typing took {avg:?} a key");
        assert!(
            inline + 3 >= n as usize,
            "only {inline} keys re-parsed on the UI thread"
        );
        assert!(worst < KEYSTROKE_WORST, "a keystroke took {worst:?}");
        assert!(comment < KEYSTROKE_WORST * 2, "typing /* took {comment:?}");
        // M6's budget for a key at 60k cursors.
        assert!(many < KEYSTROKE_WORST, "typing at {count} cursors took {many:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_rename_redetects_the_language() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, "", "tool.txt", "def f():\n    return 1\n");
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::PlainText);
    app.press(K::Char('e'), cmd() | M::SHIFT);
    let from = root.join("tool.txt");
    assert!(app.settle(|a| a.explorer.find(&from).is_some()).await);
    app.press(K::Home, M::NONE);
    for _ in 0..3 {
        if app.explorer.selected.as_deref() != Some(&from) {
            app.press(K::Down, M::NONE);
        }
    }
    assert_eq!(app.explorer.selected.as_deref(), Some(from.as_path()));
    app.press(K::F(2), M::NONE);
    for _ in 0.."txt".len() {
        app.press(K::Backspace, M::NONE);
    }
    app.type_text("py");
    app.press(K::Enter, M::NONE);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.language() == Language::Python && parsed(a))
            .await
    );
    assert_tree_current(&app);
}
