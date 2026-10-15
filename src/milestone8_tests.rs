//! Milestone 8 end to end.
//!
//! 8a, the `syntax` module: a `.cpp` file opened through
//! the app is parsed off the UI thread and the status bar shows its language mode ("C++",
//! "Python", "Plain Text"); `.workspace` `[files] associations` pick the language (also after a
//! live reload of `.workspace`) and a shebang does for a file without an extension; typing,
//! multi-cursor edits (one `InputEdit` per change; tens of thousands of cursors drop the tree for a
//! fresh background parse),
//! paste, undo and redo keep the tree equal to a fresh parse of the text, also when they happen
//! while the first parse is still running; a file over `[settings] syntax_max_bytes` has no tree
//! but still shows its language.
//!
//! 8b, highlighting: a newly opened file is drawn in plain text, then in Dark+'s token colours
//! once its tree arrives; a selection and find matches show their backgrounds under the token's
//! colour; colours line up past tabs, wide characters and CRLF; typing keeps the colours, also
//! while a re-parse runs in the background (no frame falls back to plain text), and the new tree's
//! colours replace the old; 256-colour terminals get the fallback colours; past the size limit
//! text stays plain; a 1 MB single-line file is coloured only at its start, within a frame.
//!
//! Perf (asserted in release only: `cargo test --release milestone -- --nocapture`): on a
//! generated 1 MB C++ file the first parse runs in the background, and a keystroke plus its
//! incremental re-parse plus a frame stays well inside a 16 ms frame, also at 63k cursors. With
//! highlighting ("done when": typing in a 1 MB C++ file never takes a frame over 16 ms; a newly
//! opened file shows plain text first, then colours, with no freeze): opening draws plain text at
//! once and every frame while the parse runs stays under 16 ms; keys typed at several places and
//! PageDown through the file stay under 16 ms each, the worst one included.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Style;

use crate::app::App;
use crate::buffer::Buffer;
use crate::milestone_tests::test_app;
use crate::syntax::Language;
use crate::theme::Theme;

/// A keystroke in the 1 MB C++ file, its incremental re-parse and a frame, on average (release;
/// 3.5-5 ms on a slow x86 (i7-2600K), the re-parse 2-4 ms of it).
const KEYSTROKE_BUDGET: Duration = Duration::from_millis(8);
/// The slowest keystroke (~10 ms there, an unbalanced bracket): a re-parse stops after
/// `syntax::INLINE_TIME` (8 ms) and continues in the background.
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
        // Most keys: the first few at this spot can take 6-10 ms there and finish in the background.
        assert!(
            inline * 4 >= n as usize * 3,
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

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// Where `needle` is drawn (its first cell), searching rows top to bottom.
fn find_cell(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    let chars: Vec<String> = needle.chars().map(String::from).collect();
    for y in 0..buf.area.height {
        let syms: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        if let Some(x) =
            (0..=syms.len().saturating_sub(chars.len())).find(|&x| chars.iter().zip(&syms[x..]).all(|(c, s)| c == s))
        {
            return Some((x as u16, y));
        }
    }
    None
}

/// The foreground of the cell `offset` columns into the first place `needle` is drawn.
fn fg_of(term: &Terminal<TestBackend>, needle: &str, offset: u16) -> Style {
    let (x, y) = find_cell(term, needle).unwrap_or_else(|| panic!("{needle:?} not on screen"));
    let cell = &term.backend().buffer()[(x + offset, y)];
    Style::new().fg(cell.fg).bg(cell.bg)
}

/// The theme key's foreground (only the foreground is compared).
fn fg(app: &App, key: &str) -> Option<ratatui::style::Color> {
    app.theme.get(key).fg
}

const CPP: &str = "// greeting\nint main() {\n    const char* s = \"hello\";\n    return 42;\n}\n";

/// Asserts the C++ sample's tokens are drawn in their colours.
fn assert_coloured(term: &Terminal<TestBackend>, app: &App) {
    assert_eq!(fg_of(term, "// greeting", 3).fg, fg(app, "comment"));
    assert_eq!(fg_of(term, "int main", 0).fg, fg(app, "type.builtin"));
    assert_eq!(fg_of(term, "int main", 4).fg, fg(app, "function"));
    assert_eq!(fg_of(term, "const char", 0).fg, fg(app, "keyword"));
    assert_eq!(fg_of(term, "\"hello\"", 2).fg, fg(app, "string"));
    assert_eq!(fg_of(term, "return 42", 0).fg, fg(app, "keyword.control"));
    assert_eq!(fg_of(term, "return 42", 7).fg, fg(app, "number"));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_highlighting() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "", "main.cpp", CPP);
    let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
    let plain = fg(&app, "editor");

    // The first frame comes before the parse's result is taken: plain text.
    draw(&mut term, &mut app);
    assert!(!parsed(&app));
    for (needle, offset) in [
        ("// greeting", 3),
        ("int main", 0),
        ("\"hello\"", 2),
        ("return 42", 0),
        ("return 42", 7),
    ] {
        assert_eq!(fg_of(&term, needle, offset).fg, plain, "{needle}");
    }
    // Then colours.
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_coloured(&term, &app);
    assert_ne!(fg(&app, "keyword"), plain);
    assert_ne!(fg(&app, "keyword"), fg(&app, "keyword.control"));

    // A selection over `return`: the selection's background, the keyword's colour.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("4");
    app.press(K::Enter, M::NONE);
    app.press(K::Home, M::NONE);
    for _ in 0.."return".len() {
        app.press(K::Right, M::SHIFT);
    }
    draw(&mut term, &mut app);
    let cell = fg_of(&term, "return 42", 2);
    assert_eq!(cell.fg, fg(&app, "keyword.control"));
    assert_eq!(Some(cell.bg.unwrap()), app.theme.get("editor.selection").bg);
    app.press(K::End, M::NONE);

    // Find matches: their backgrounds, the tokens' colours (`const`'s `o` is the current match, in
    // a keyword; `hello`'s is another, in a string).
    app.press(K::Char('f'), cmd());
    app.type_text("o");
    draw(&mut term, &mut app);
    let cell = fg_of(&term, "\"hello\"", 5);
    assert_eq!(cell.fg, fg(&app, "string"));
    assert_eq!(cell.bg, app.theme.get("editor.findMatchHighlight").bg);
    let cell = fg_of(&term, "const char", 1);
    assert_eq!(cell.fg, fg(&app, "keyword"));
    assert_eq!(cell.bg, app.theme.get("editor.findMatch").bg);
    app.press(K::Esc, M::NONE);
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("4");
    app.press(K::Enter, M::NONE);
    app.press(K::End, M::NONE);

    // Typing keeps the colours on every frame, and new text gets its own once re-parsed.
    for c in " // done".chars() {
        app.press(K::Char(c), M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        assert_coloured(&term, &app);
    }
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "// done", 3).fg, fg(&app, "comment"));

    // An unclosed `/*` at the top (tree-sitter's error recovery doesn't make it a comment), then
    // closed further down: the text between is a comment.
    app.press(K::Home, cmd());
    app.type_text("/*");
    app.pump();
    draw(&mut term, &mut app);
    assert_ne!(fg_of(&term, "return 42", 0).fg, plain);
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    app.type_text("*/");
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "int main", 0).fg, fg(&app, "comment"));
    assert_eq!(fg_of(&term, "return 42", 0).fg, fg(&app, "keyword.control"));
    app.press(K::Char('z'), cmd());
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "return 42", 0).fg, fg(&app, "keyword.control"));

    // A 256-colour terminal: the same keys, the fallback colours.
    app.theme = Theme::dark_plus(false);
    draw(&mut term, &mut app);
    assert_coloured(&term, &app);
    assert_ne!(fg(&app, "keyword"), Theme::dark_plus(true).get("keyword").fg);
}

/// An edit whose re-parse goes to the background (63k-cursor-wide here) keeps drawing the last
/// colours, moved along with the text, until the new tree arrives.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_colours_survive_a_background_reparse() {
    let dir = tempfile::tempdir().unwrap();
    let text = generated_cpp(100_000);
    let (mut app, _) = open_app(&dir, "", "big.cpp", &text);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "class Item0", 0).fg, fg(&app, "keyword"));
    assert_eq!(fg_of(&term, "/// Holds item 0", 4).fg, fg(&app, "comment"));

    // Every `t` at once: the tree is dropped and re-parsed in the background.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("7");
    app.press(K::Enter, M::NONE);
    app.press(K::Right, M::SHIFT);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    assert!(app.group().active_tab().unwrap().view.selections.len() > 1000);
    app.press(K::Char('X'), M::SHIFT);
    app.pump();
    assert!(!parsed(&app), "re-parsing in the background");
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "class IXem0", 0).fg, fg(&app, "keyword"));
    assert_eq!(fg_of(&term, "public Base", 0).fg, fg(&app, "keyword"));
    assert_eq!(fg_of(&term, "/// Holds iXem 0", 4).fg, fg(&app, "comment"));
    assert_eq!(
        fg_of(&term, "reXurn", 0).fg,
        fg(&app, "keyword.control"),
        "the old colours"
    );
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "/// Holds iXem 0", 4).fg, fg(&app, "comment"));
    // The new tree's colours: `reXurn value_;` declares a variable of type `reXurn`.
    assert_eq!(fg_of(&term, "reXurn", 0).fg, fg(&app, "type"));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_plain_text_past_the_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("{CPP}{}", "int x = 1;\n".repeat(100));
    let (mut app, _) = open_app(&dir, "[settings]\nsyntax_max_bytes = 1000\n", "big.cpp", &text);
    let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
    app.pump();
    assert!(app.active_buffer().unwrap().syntax.too_large());
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "return 42", 0).fg, fg(&app, "editor"));
}

/// The longest frame allowed (release): the "done when" bound.
const FRAME: Duration = Duration::from_millis(16);
/// Debug builds: only catches a full parse or a whole-file query on the UI thread.
const FRAME_DEBUG_BOUND: Duration = Duration::from_millis(400);

/// "Done when": opening a 1 MB C++ file draws plain text at once and keeps drawing while the
/// parse runs, then colours; typing at several places and paging through it never takes a frame
/// over 16 ms (release).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_highlighted_one_mb_cpp_file_frames() {
    let dir = tempfile::tempdir().unwrap();
    let release = !cfg!(debug_assertions);
    let text = generated_cpp(if release { 1_000_000 } else { 250_000 });
    let bound = if release { FRAME } else { FRAME_DEBUG_BOUND };
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();

    // Opening: the first frame before the parse, plain.
    let (mut app, _) = open_app(&dir, "", "big.cpp", &text);
    let t = Instant::now();
    draw(&mut term, &mut app);
    let first = t.elapsed();
    assert!(!parsed(&app));
    assert_eq!(fg_of(&term, "class Item0", 0).fg, fg(&app, "editor"));
    // Frames while it parses (a turn of the loop each: background results, then a draw).
    let (mut frames, mut worst_open) = (0, first);
    let t = Instant::now();
    while !parsed(&app) {
        assert!(t.elapsed() < Duration::from_secs(10), "no tree");
        let f = Instant::now();
        app.pump();
        draw(&mut term, &mut app);
        worst_open = worst_open.max(f.elapsed());
        frames += 1;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The frame that first shows colours (queries the visible lines).
    let f = Instant::now();
    draw(&mut term, &mut app);
    let coloured = f.elapsed();
    worst_open = worst_open.max(coloured);
    assert_eq!(fg_of(&term, "class Item0", 0).fg, fg(&app, "keyword"));
    println!(
        "open: first frame {first:?}, {frames} frames while parsing ({:?}), first coloured frame {coloured:?}, worst {worst_open:?}",
        t.elapsed()
    );
    assert!(frames > 1, "frames were drawn while parsing");

    // Typing at several places: a key, its re-parse (or its start in the background) and a frame.
    let lines = text.lines().count();
    let (mut worst_key, mut total, mut n, mut fell_back) = (Duration::ZERO, Duration::ZERO, 0u32, 0);
    let keyword_control = fg(&app, "keyword.control");
    for (i, at) in [lines / 2, 10, lines - 20, lines / 3, (lines * 3) / 4]
        .into_iter()
        .enumerate()
    {
        app.press(K::Char('g'), M::CONTROL);
        app.type_text(&at.to_string());
        app.press(K::Enter, M::NONE);
        app.press(K::End, M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        let keys = ["  int y = 0;", " // note", " x += 1;", "  /* c */", " f(\"s\");"][i];
        for c in keys.chars() {
            let t = Instant::now();
            app.press(K::Char(c), M::NONE);
            app.pump();
            draw(&mut term, &mut app);
            let e = t.elapsed();
            assert!(e < FRAME_DEBUG_BOUND, "key {c:?} took {e:?}");
            worst_key = worst_key.max(e);
            total += e;
            n += 1;
            // Every frame is coloured, also when the re-parse went to the background.
            assert_eq!(fg_of(&term, "return", 0).fg, keyword_control, "key {c:?}");
            if !parsed(&app) {
                fell_back += 1;
                assert!(app.settle(parsed).await);
            }
        }
    }
    println!(
        "typing: {n} keys at 5 places, avg {:?}, worst {worst_key:?}, {fell_back} re-parsed in the background",
        total / n
    );

    // Paging down from the top (each page a new highlight query past the margin).
    app.press(K::Home, cmd());
    app.pump();
    draw(&mut term, &mut app);
    let mut worst_page = Duration::ZERO;
    for _ in 0..60 {
        let t = Instant::now();
        app.press(K::PageDown, M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        worst_page = worst_page.max(t.elapsed());
        assert_eq!(fg_of(&term, "return", 0).fg, keyword_control);
    }
    let queries = app.active_buffer().unwrap().syntax.highlight_queries_run();
    println!("paging: 60 pages, worst {worst_page:?}; {queries} highlight queries in all");

    assert!(worst_page < bound, "a page took {worst_page:?}");
    if release {
        assert!(worst_open < FRAME, "a frame while opening took {worst_open:?}");
        assert!(worst_key < FRAME, "a key took {worst_key:?}");
    }
}

/// Colours line up with the drawn cells past tabs, wide and multi-byte characters and CRLF line
/// breaks; a span ending inside a grapheme (a combining mark after an identifier) draws fine.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_colours_line_up_with_wide_text() {
    let dir = tempfile::tempdir().unwrap();
    let text = "int f() {\r\n\treturn \"界é\" + 777; /* 界 c */\r\n\tint x\u{301} = 1;\r\n}\r\n";
    let (mut app, _) = open_app(&dir, "", "wide.c", text);
    let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
    assert!(app.settle(parsed).await);
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "return", 0).fg, fg(&app, "keyword.control"));
    assert_eq!(fg_of(&term, " return", 0).fg, fg(&app, "editor"), "the tab's last cell");
    assert_eq!(fg_of(&term, "\"", 0).fg, fg(&app, "string"));
    assert_eq!(fg_of(&term, "界", 0).fg, fg(&app, "string"));
    assert_eq!(fg_of(&term, "é\"", 1).fg, fg(&app, "string"));
    assert_eq!(fg_of(&term, "777", 0).fg, fg(&app, "number"));
    assert_eq!(fg_of(&term, "777", 2).fg, fg(&app, "number"));
    assert_eq!(fg_of(&term, "/*", 0).fg, fg(&app, "comment"));
    assert_eq!(fg_of(&term, "c */", 3).fg, fg(&app, "comment"));
    // `int x\u{301}`: the identifier's span ends before the combining mark (inside the cell).
    let (x, y) = find_cell(&term, "= 1;").unwrap();
    let cell = |dx: u16| &term.backend().buffer()[(x - dx, y)];
    assert_eq!((cell(6).symbol(), Some(cell(6).fg)), ("i", fg(&app, "type.builtin")));
    assert_eq!(cell(2).symbol(), "x\u{301}");
    assert_eq!(fg_of(&term, "= 1;", 2).fg, fg(&app, "number"));
}

/// A 1 MB file on one line (minified code, a data table): only its start is coloured, so the
/// frame that queries it stays a frame (release: ~7 ms; the whole line took ~180 ms).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_one_mb_line() {
    let dir = tempfile::tempdir().unwrap();
    let release = !cfg!(debug_assertions);
    let text = generated_cpp(if release { 1_000_000 } else { 250_000 })
        .replace("// odd ones", "")
        .replace("/// Holds item", "/* Holds item */")
        .replace('\n', " ");
    let (mut app, _) = open_app(&dir, "", "long.cpp", &text);
    let mut term = Terminal::new(TestBackend::new(160, 20)).unwrap();
    assert!(app.settle(parsed).await);
    let t = Instant::now();
    draw(&mut term, &mut app);
    let first = t.elapsed();
    assert_eq!(fg_of(&term, "namespace gen", 0).fg, fg(&app, "keyword"));
    assert_eq!(fg_of(&term, "/* Holds", 0).fg, fg(&app, "comment"));
    // Typing at the start: the stale colours, then the new tree's (one query each).
    let mut worst = Duration::ZERO;
    for c in "int ".chars() {
        let t = Instant::now();
        app.press(K::Char(c), M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        worst = worst.max(t.elapsed());
        assert!(app.settle(parsed).await);
        let t = Instant::now();
        draw(&mut term, &mut app);
        worst = worst.max(t.elapsed());
        assert_eq!(fg_of(&term, "namespace gen", 0).fg, fg(&app, "keyword"));
    }
    println!("one 1 MB line: first coloured frame {first:?}, worst frame typing {worst:?}");
    assert!(first.max(worst) < if release { FRAME } else { FRAME_DEBUG_BOUND });
}
