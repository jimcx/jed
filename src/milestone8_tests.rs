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
//! 8c, comment line and auto-close pairs: Cmd+/ comments and uncomments a C++ and a Python
//! selection (blank lines skipped, a selection ending at a line start not taking that line) in one
//! undo step, does nothing in plain text and is refused in a read-only buffer; typing `(` closes the
//! pair with the cursor between, `)` types over it, Backspace deletes an empty auto-inserted pair,
//! leaving the pair ends type-over, `"` wraps a selection (one undo step), paste never auto-closes,
//! a quote inside a string (with a tree) isn't closed but Python's `f'` is; `[settings]
//! auto_close_pairs = false` in `.workspace` turns it all off (not comment line). At the 1 MB file's
//! 63k cursors `(`, type-over and pair delete stay within a key's 16 ms, and Cmd+/ on 84k lines
//! within 80 ms (release).
//!
//! 8d, the user settings file: `settings.toml` in the config dir (passed explicitly, never via
//! `XDG_CONFIG_HOME`) turning `auto_close_pairs` off types `(` plainly; `.workspace` turning it
//! back on wins, at startup and on a live reload, which keeps the user's value for keys
//! `.workspace` doesn't set (and falls back to it when a key is removed); a syntax error shows a
//! notification and keeps the defaults; unknown keys and bad values are reported and skipped
//! while the other keys apply; "Preferences: Open User Settings" creates the directory and file
//! from the template, or opens an existing file as it is.
//!
//! 8e, auto-indent and bracket matching: typing a C++ function with Enter and `}` (auto-close
//! off, then on: Enter between `{}` / `()` puts the closer on its own line) gives exactly indented
//! text, and the Enter groups with the typing in one undo step; Python indents after `if …:` (a
//! trailing comment skipped), dedents after `return`, and `else:` / an `else:` typed deeper go back
//! to their `if`; the brackets next to the cursor and their partners are drawn with
//! `editor.bracketMatch` (not one in a string), at every cursor, and not when the cursor leaves
//! them or `match_brackets` is off; `auto_indent` / `match_brackets` off in `settings.toml` (a plain
//! line break, no outdent, no highlight) and back on from a `.workspace` reload. At ~65k cursors
//! (1 MB) Enter (with the tree) and the outdenting `}` stay within M6's 80 ms line-command budget;
//! moving and typing in the 1 MB generated C++ file with bracket highlights keeps frames < 16 ms.
//!
//! 8f, more languages: Rust, Markdown, YAML, JSON, JSONC, XML, TOML, shell, CMake, Makefile and
//! Dockerfile files opened in the app are detected by extension, name (`Cargo.lock`,
//! `CMakeLists.txt`, `Makefile`, `Dockerfile`, `Containerfile.*`, `tsconfig.json`) or shebang
//! (`env bash`), show VS Code's language name in the status bar, get a tree equal to a fresh parse
//! and are drawn in their Dark+ colours (Markdown headings, XML tags and attributes, YAML keys, …);
//! Cmd+/ uses each language's token (`//`, `#`; nothing in JSON, Markdown, XML) and undoes back;
//! JSON auto-closes brackets and quotes but not inside a string, Rust leaves `'` alone, Markdown
//! closes `<`; Enter indents after a YAML key and outdents before a Rust `}`; Makefiles indent with
//! tabs (status bar "Tab Size"); Markdown's brackets are matched from its text. A 1 MB file in
//! Rust, JSON, YAML, Markdown, XML, TOML and shell opens with plain frames while it parses in the
//! background and types within a frame (release).
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
    app.test_plain_whitespace();
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

    // `/*` at the top comments out the whole file: the UI thread gives up on the re-parse after
    // `INLINE_TIME` and it finishes in the background. A fast machine lexes the one big comment
    // within that time, so the tree may already be current: either way the key stays quick.
    app.press_file_start();
    let t = Instant::now();
    app.type_text("/*");
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let comment = t.elapsed();
    let at_once = parsed(&app);
    println!("typing /* at the top: {comment:?} (re-parsed on the UI thread: {at_once})");
    assert!(comment < KEYSTROKE_DEBUG_BOUND, "/* took {comment:?}");
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
    app.press_file_start();
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
            // FLAKE: 2026-10-24, one key of this 1 MB C++ typing loop took >400 ms once under
            // load (a background re-parse still running). State dumped here to diagnose next time.
            if e >= FRAME_DEBUG_BOUND {
                eprintln!(
                    "FLAKE milestone_8_highlighted_one_mb_cpp_file_frames: key {c:?} at place {i} \
                     took {e:?}, fell_back={fell_back}, parsed={}, n={n}, worst_so_far={worst_key:?}",
                    parsed(&app),
                );
            }
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
    app.press_file_start();
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

// ---- 8c: comment line and auto-close pairs ----

/// Cmd+/ on every line of a 1 MB file, or a key at the 1 MB file's 63k cursors, plus a frame
/// (release; M6's line commands have 80 ms, a typed key 16 ms).
const COMMENT_BUDGET: Duration = Duration::from_millis(80);
const PAIR_KEY_BUDGET: Duration = Duration::from_millis(16);
/// Debug builds only catch going quadratic.
const PAIR_DEBUG_BOUND: Duration = Duration::from_millis(2000);

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

/// The active view's selections as (anchor, head).
fn sels(app: &App) -> Vec<(usize, usize)> {
    let view = &app.group().active_tab().unwrap().view;
    view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect()
}

fn set_cursors(app: &mut App, at: &[usize]) {
    use crate::selection::{Selection, Selections};
    let list = at
        .iter()
        .map(|&p| Selection::cursor(crate::buffer::pos::ByteIdx(p)))
        .collect();
    app.active_mut().unwrap().1.selections = Selections::new(list, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_comment_line_toggles_and_undoes_in_one_step() {
    let dir = tempfile::tempdir().unwrap();
    let cpp = "int f() {\n    int x = 1;\n\n    return x;\n}\n";
    let (mut app, _) = open_app(&dir, "", "a.cpp", cpp);
    // Shift+Down three times from line 2: lines 2-4 (the selection ends at line 5's start).
    set_cursors(&mut app, &[10]);
    app.press(K::Down, M::SHIFT);
    app.press(K::Down, M::SHIFT);
    app.press(K::Down, M::SHIFT);
    app.press(K::Char('/'), cmd());
    assert_eq!(
        text(&app),
        "int f() {\n    // int x = 1;\n\n    // return x;\n}\n",
        "blank line skipped, the line the selection ends at the start of left alone"
    );
    assert_eq!(sels(&app), vec![(10, 46)], "the same lines stay selected");
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), cpp);
    app.press(K::Char('/'), cmd());
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), cpp, "one undo step");
    app.press(K::Char('z'), cmd() | M::SHIFT);
    app.press(K::Char('a'), cmd());
    app.press(K::Char('/'), cmd());
    assert_eq!(
        text(&app),
        "// int f() {\n//     // int x = 1;\n\n//     // return x;\n// }\n",
        "a commented line among others gets commented again"
    );
    // The tree follows (the file is all comments now).
    assert!(app.settle(parsed).await);
    assert_tree_current(&app);

    let py = "def f(x):\n    # twice\n    return x * 2\n";
    let (mut app, _) = open_app(&dir, "", "a.py", py);
    app.press(K::Char('a'), cmd());
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), "# def f(x):\n#     # twice\n#     return x * 2\n");
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), py);
    // A cursor on one line.
    set_cursors(&mut app, &[26]);
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), "def f(x):\n    # twice\n    # return x * 2\n");

    // Plain text has no comment token; read-only buffers refuse.
    let (mut app, _) = open_app(&dir, "", "a.txt", "notes\n");
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), "notes\n");
    let (mut app, _) = open_app(&dir, "", "b.cpp", "int x;\n");
    app.active_mut().unwrap().0.read_only = Some("test".into());
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), "int x;\n");
    assert!(app.message.as_ref().unwrap().text.contains("Read-only"));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_auto_close_type_over_wrap_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "", "a.cpp", "\n");
    app.type_text("foo(");
    assert_eq!(text(&app), "foo()\n");
    assert_eq!(sels(&app), vec![(4, 4)], "between the pair");
    app.type_text("x)");
    assert_eq!(text(&app), "foo(x)\n", "typed over the closer");
    assert_eq!(sels(&app), vec![(6, 6)]);
    // Backspace in an auto-inserted pair deletes both.
    app.type_text(";[");
    assert_eq!(text(&app), "foo(x);[]\n");
    app.press(K::Backspace, M::NONE);
    assert_eq!(text(&app), "foo(x);\n");
    // Leaving the pair (and coming back) ends type-over; `)` is typed.
    app.type_text("(");
    app.press(K::Home, M::NONE);
    app.press(K::End, M::NONE);
    app.press(K::Left, M::NONE);
    app.type_text(")");
    assert_eq!(text(&app), "foo(x);())\n");
    // Wrap a selection in quotes, still selected.
    app.press(K::Char('a'), cmd());
    app.press(K::Backspace, M::NONE);
    app.type_text("say hi");
    let word = if cfg!(target_os = "macos") { M::ALT } else { M::CONTROL };
    app.press(K::Left, M::SHIFT | word);
    app.type_text("\"");
    assert_eq!(text(&app), "say \"hi\"");
    assert_eq!(sels(&app), vec![(5, 7)]);
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "say hi", "the wrap is one undo step");
    // Paste never auto-closes.
    app.press(K::End, M::NONE);
    app.handle_test_event(crossterm::event::Event::Paste(" (".into()));
    assert_eq!(text(&app), "say hi (");

    // A quote in a string (with the tree) isn't closed.
    let (mut app, _) = open_app(&dir, "", "b.py", "s = \"ab\"\n");
    assert!(app.settle(parsed).await);
    set_cursors(&mut app, &[6]);
    app.type_text("'");
    assert_eq!(text(&app), "s = \"a'b\"\n");
    app.press(K::End, M::NONE);
    app.type_text(" + f'");
    assert_eq!(text(&app), "s = \"a'b\" + f''\n", "Python string prefix");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_auto_close_pairs_setting_off() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "[settings]\nauto_close_pairs = false\n", "a.cpp", "x\n");
    assert!(!app.settings.auto_close_pairs);
    app.press(K::End, M::NONE);
    app.type_text("(\"");
    assert_eq!(text(&app), "x(\"\n");
    app.press(K::Left, M::SHIFT);
    app.type_text("[");
    assert_eq!(text(&app), "x([\n", "no wrap: the selection is replaced");
    app.press(K::Char('/'), cmd());
    assert_eq!(text(&app), "// x([\n", "comment line isn't affected");
}

/// Cmd+Shift+L on a word, then `(` (auto-closed at every cursor), `)` (typed over) and a
/// Backspace in the pair, at the 1 MB file's 63k cursors; and Cmd+/ on all of a 1 MB file's
/// 90k lines and back.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_pairs_and_comment_line_on_one_mb() {
    let release = !cfg!(debug_assertions);
    let size = if release { 1_000_000 } else { 250_000 };
    let mut original = String::new();
    let mut i = 0;
    while original.len() < size {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "", "big.cpp", &original);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    set_cursors(&mut app, &[5]);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    app.press(K::Right, M::NONE);
    let count = sels(&app).len();
    assert_eq!(count, 2 * i);
    let mut timed = |app: &mut App, what: &str, key: K| {
        let t = Instant::now();
        app.press(key, M::NONE);
        app.pump();
        draw(&mut term, app);
        let took = t.elapsed();
        println!("{what} at {count} cursors: {took:?}");
        assert!(took < PAIR_DEBUG_BOUND, "{what} took {took:?}");
        if release {
            assert!(took < PAIR_KEY_BUDGET, "{what} too slow: {took:?}");
        }
    };
    timed(&mut app, "typing ( (auto-closed)", K::Char('('));
    assert!(text(&app).starts_with("let item() = value_0; // item()\n"));
    timed(&mut app, "typing x", K::Char('x'));
    timed(&mut app, "typing ) (typed over)", K::Char(')'));
    assert!(text(&app).starts_with("let item(x) = value_0; // item(x)\n"));
    timed(&mut app, "typing [", K::Char('['));
    timed(&mut app, "Backspace in []", K::Backspace);
    assert!(text(&app).starts_with("let item(x) = value_0; // item(x)\n"));
    assert_eq!(sels(&app).len(), count);

    let mut lines = String::new();
    let mut n = 0;
    while lines.len() < size {
        lines.push_str(&format!("  f({n});\n"));
        n += 1;
    }
    let (mut app, _) = open_app(&dir, "", "lines.cpp", &lines);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    app.press(K::Char('a'), cmd());
    let mut toggle = |app: &mut App, what: &str| {
        let t = Instant::now();
        app.press(K::Char('/'), cmd());
        draw(&mut term, app);
        let took = t.elapsed();
        println!("Cmd+/ {what} on {n} lines: {took:?}");
        assert!(took < PAIR_DEBUG_BOUND, "{what} took {took:?}");
        if release {
            assert!(took < COMMENT_BUDGET, "{what} too slow: {took:?}");
        }
    };
    toggle(&mut app, "commenting");
    assert!(text(&app).starts_with("  // f(0);\n  // f(1);\n"));
    toggle(&mut app, "uncommenting");
    assert_eq!(text(&app), lines);
    // Every line its own cursor: each toggles alone.
    app.press(K::Char('I'), M::ALT | M::SHIFT);
    assert_eq!(sels(&app).len(), n);
    toggle(&mut app, "at a cursor per line");
    assert!(text(&app).starts_with("  // f(0);\n"));
}

/// An app as `main` builds it with a user config dir (`<dir>/config`, holding `settings.toml`
/// text `user` unless `None`) beside the workspace (`<dir>/ws`, `.workspace` text `config`), with
/// `rel` (holding `text`) open. Returns the app, the workspace root and the config dir.
fn user_app(
    dir: &tempfile::TempDir,
    user: Option<&str>,
    config: &str,
    rel: &str,
    text: &str,
) -> (App, PathBuf, PathBuf) {
    use crate::config::settings;
    let base = crate::workspace::normalize(dir.path());
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    if let Some(user) = user {
        write(&config_dir, settings::FILE_NAME, user);
    }
    let root = base.join("ws");
    write(&root, ".workspace", config);
    let path = write(&root, rel, text);
    let loaded = settings::load(&config_dir.join(settings::FILE_NAME));
    let ws = crate::workspace::Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let trust = crate::workspace::trust::TrustStore::load(base.join("trust.toml"));
    let initial = crate::app::InitialFile {
        buffer: Buffer::open(&path).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config_dir.clone();
    app.report_errors(loaded.errors);
    app.test_start();
    app.test_plain_whitespace();
    (app, root, config_dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_user_settings_file() {
    // The user turns auto-close off: `(` types plainly.
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\nauto_close_pairs = false\ntab_width = 2\n";
    let (mut app, root, _) = user_app(&dir, Some(user), "", "a.cpp", "x\n");
    assert!(!app.settings.auto_close_pairs);
    assert_eq!(app.settings.tab_width, 2);
    assert!(app.message.is_none(), "{:?}", app.message.as_ref().map(|m| &m.text));
    app.press(K::End, M::NONE);
    app.type_text("(");
    assert_eq!(text(&app), "x(\n");

    // A live `.workspace` reload turns it back on and keeps the user's value for the other keys.
    fs::write(root.join(".workspace"), "[settings]\nauto_close_pairs = true\n").unwrap();
    assert!(app.settle(|a| a.settings.auto_close_pairs).await, ".workspace reloaded");
    assert_eq!(app.settings.tab_width, 2, "the user's tab_width survives the reload");
    app.type_text("(");
    assert_eq!(text(&app), "x(()\n");
    // Removing the key from `.workspace` falls back to the user's value, not the default.
    fs::write(root.join(".workspace"), "[settings]\ninsert_spaces = false\n").unwrap();
    assert!(app.settle(|a| !a.settings.insert_spaces).await);
    assert!(!app.settings.auto_close_pairs);
    assert_eq!(app.settings.tab_width, 2);

    // `.workspace` wins from the start too.
    let dir = tempfile::tempdir().unwrap();
    let ws = "[settings]\nauto_close_pairs = true\n";
    let (mut app, _, _) = user_app(&dir, Some(user), ws, "a.cpp", "x\n");
    assert!(app.settings.auto_close_pairs);
    assert_eq!(app.settings.tab_width, 2);
    app.press(K::End, M::NONE);
    app.type_text("(");
    assert_eq!(text(&app), "x()\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_bad_user_settings_are_reported() {
    // A syntax error: a notification, and the defaults.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = user_app(&dir, Some("[settings\nauto_close_pairs = false\n"), "", "a.cpp", "x\n");
    let msg = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(msg.starts_with("settings.toml: line 1:"), "{msg}");
    assert!(app.settings.auto_close_pairs, "defaults kept");
    app.press(K::End, M::NONE);
    app.type_text("(");
    assert_eq!(text(&app), "x()\n");

    // A misspelt key and a bad value are reported; the good key still applies.
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\nauto_close_pair = false\ntab_width = \"2\"\ninsert_spaces = false\n";
    let (app, _, _) = user_app(&dir, Some(user), "", "a.cpp", "x\n");
    let msg = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(
        msg.contains("auto_close_pair") && msg.ends_with("(and 1 more)"),
        "{msg}"
    );
    assert!(!app.settings.insert_spaces);
    assert_eq!(app.settings.tab_width, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_open_user_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, config_dir) = user_app(&dir, None, "", "a.cpp", "x\n");
    fs::remove_dir(&config_dir).unwrap();
    let path = config_dir.join(crate::config::settings::FILE_NAME);
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text("Open User Settings");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await);
    assert_eq!(
        text(&app),
        crate::config::settings::TEMPLATE,
        "created from the template"
    );

    // An existing file is opened as it is.
    fs::write(&path, "[settings]\ntab_width = 3\n").unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let (mut app, _, _) = user_app(&dir2, None, "", "a.cpp", "x\n");
    app.config_dir = config_dir;
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text("Open User Settings");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await);
    assert_eq!(text(&app), "[settings]\ntab_width = 3\n");
}

// ---- 8e: auto-indent and bracket matching ----

/// Enter, or a typed closing bracket, at the 1 MB file's ~80k cursors plus a frame (release;
/// M6's line commands have 80 ms).
const INDENT_BUDGET: Duration = Duration::from_millis(80);

/// The background of the cell `offset` columns into the first place `needle` is drawn.
fn bg_of(term: &Terminal<TestBackend>, needle: &str, offset: u16) -> Option<ratatui::style::Color> {
    let (x, y) = find_cell(term, needle).unwrap_or_else(|| panic!("{needle:?} not on screen"));
    Some(term.backend().buffer()[(x + offset, y)].bg)
}

/// How many cells of the editors' text have the bracket highlight's background. Only the text:
/// without truecolor a scroll bar's thumb is the same grey (the explorer's, once it lists more
/// files than fit).
fn bracket_cells(term: &Terminal<TestBackend>, app: &App) -> usize {
    let want = app.theme.get("editor.bracketMatch").bg.unwrap();
    let buf = term.backend().buffer();
    app.hits
        .groups
        .iter()
        .flat_map(|g| g.text.positions())
        .filter(|&p| buf[p].bg == want)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_auto_indent_cpp_and_python() {
    let dir = tempfile::tempdir().unwrap();
    // Typed with auto-close off, every bracket by hand: Enter indents after `{`, `}` outdents.
    let (mut app, _) = open_app(&dir, "[settings]\nauto_close_pairs = false\n", "a.cpp", "");
    for line in [
        "int f(int x) {",
        "if (x) {",
        "return g(1,",
        "2);",
        "}",
        "return 0;",
        "}",
    ] {
        app.type_text(line);
        app.press(K::Enter, M::NONE);
        app.pump();
    }
    assert_eq!(
        text(&app),
        "int f(int x) {\n    if (x) {\n        return g(1,\n            2);\n    }\n    return 0;\n}\n"
    );
    // Enter and its indentation group with the typing around it, as typing does.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "");

    // With auto-close: `{` closes, Enter between the pair puts `}` on its own line.
    let (mut app, _) = open_app(&dir, "", "b.cpp", "");
    app.type_text("void g() {");
    app.press(K::Enter, M::NONE);
    app.type_text("h(");
    app.press(K::Enter, M::NONE);
    app.type_text("1");
    assert_eq!(text(&app), "void g() {\n    h(\n        1\n    )\n}");
    assert_eq!(sels(&app), vec![(27, 27)]);

    // Python: a level after `if …:`, one less after `return`, `else:` back to its `if`.
    let (mut app, _) = open_app(&dir, "", "a.py", "");
    assert!(app.settle(parsed).await);
    for line in ["def f(x):", "if x:  # yes", "return 1", "y = 2", "else"] {
        app.type_text(line);
        if line != "else" {
            app.press(K::Enter, M::NONE);
        }
        app.pump();
    }
    app.type_text(":");
    assert_eq!(
        text(&app),
        "def f(x):\n    if x:  # yes\n        return 1\n    y = 2\n    else:"
    );
    let (mut app, _) = open_app(&dir, "", "b.py", "def f(x):\n    if x:\n        y = 1\n");
    app.press_file_end();
    app.type_text("        else:");
    assert_eq!(text(&app), "def f(x):\n    if x:\n        y = 1\n    else:");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_bracket_matching() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int f() {\n    return g(\"}\");\n}\n";
    let (mut app, _) = open_app(&dir, "", "a.cpp", src);
    assert!(app.settle(parsed).await);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    let hl = app.theme.get("editor.bracketMatch").bg;
    // Before the `{`: it and the `}` on line 3 (not the one in the string).
    set_cursors(&mut app, &[8]);
    draw(&mut term, &mut app);
    assert_eq!(bg_of(&term, "int f() {", 8), hl);
    let (x, y) = find_cell(&term, "int f() {").unwrap();
    let close = &term.backend().buffer()[(x, y + 2)];
    assert_eq!((close.symbol(), Some(close.bg)), ("}", hl));
    assert_eq!(bracket_cells(&term, &app), 2);
    // After the `(` of `g(`: that pair.
    set_cursors(&mut app, &[src.find("g(").unwrap() + 2]);
    draw(&mut term, &mut app);
    assert_eq!(bg_of(&term, "g(\"", 1), hl);
    assert_eq!(bg_of(&term, "\");", 1), hl);
    assert_eq!(bracket_cells(&term, &app), 2);
    // Away from brackets: none.
    app.press(K::Home, M::NONE);
    draw(&mut term, &mut app);
    assert_eq!(bracket_cells(&term, &app), 0);
    // Every cursor gets its pair.
    set_cursors(&mut app, &[8, src.find("g(").unwrap() + 2]);
    draw(&mut term, &mut app);
    assert_eq!(bracket_cells(&term, &app), 4);

    // Off in `.workspace`.
    let (mut app, _) = open_app(&dir, "[settings]\nmatch_brackets = false\n", "b.cpp", src);
    set_cursors(&mut app, &[8]);
    draw(&mut term, &mut app);
    assert_eq!(bracket_cells(&term, &app), 0);
}

/// Both settings off in the user's `settings.toml`, back on from `.workspace` (live reload).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_auto_indent_and_match_brackets_settings() {
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\nauto_indent = false\nmatch_brackets = false\n";
    let (mut app, root, _) = user_app(&dir, Some(user), "", "a.cpp", "  f() {}\n");
    assert!(!app.settings.auto_indent && !app.settings.match_brackets);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    set_cursors(&mut app, &[7]);
    draw(&mut term, &mut app);
    assert_eq!(bracket_cells(&term, &app), 0);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "  f() {\n}\n", "a plain line break");
    app.type_text("  }");
    assert_eq!(text(&app), "  f() {\n  }}\n", "no outdent");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "  f() {}\n");

    fs::write(
        root.join(".workspace"),
        "[settings]\nauto_indent = true\nmatch_brackets = true\n",
    )
    .unwrap();
    assert!(
        app.settle(|a| a.settings.auto_indent && a.settings.match_brackets)
            .await
    );
    set_cursors(&mut app, &[7]);
    draw(&mut term, &mut app);
    assert_eq!(bracket_cells(&term, &app), 2);
    app.press(K::Enter, M::NONE);
    // A level of the file's own indentation (two spaces, guessed on load).
    assert_eq!(text(&app), "  f() {\n    \n  }\n");
}

/// Enter between `{}` at every cursor of a 1 MB C++ file (~80k cursors, with a current tree),
/// then `}` at all of them (outdent, the tree dropped by the wide edit), within M6's 80 ms line
/// command budget; and moving / typing in the 1 MB generated C++ file with bracket matching
/// stays within a 16 ms frame (release).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_auto_indent_and_bracket_matching_on_one_mb() {
    let release = !cfg!(debug_assertions);
    let size = if release { 1_000_000 } else { 250_000 };
    let mut original = String::new();
    let mut i = 0;
    while original.len() < size {
        original.push_str(&format!("void function_{i}() {{}} // {{}}\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, "", "big.cpp", &original);
    assert!(app.settle(parsed).await);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    // Select the first `{}`, then every `{}`; cursors between each pair.
    set_cursors(&mut app, &[original.find('{').unwrap()]);
    app.press(K::Right, M::SHIFT);
    app.press(K::Right, M::SHIFT);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    app.press(K::Left, M::NONE);
    app.press(K::Right, M::NONE);
    let count = sels(&app).len();
    assert_eq!(count, 2 * i);
    let bound = if release { INDENT_BUDGET } else { LINE_DEBUG_BOUND };
    let mut timed = |app: &mut App, what: &str, key: K| {
        let t = Instant::now();
        app.press(key, M::NONE);
        app.pump();
        draw(&mut term, app);
        let took = t.elapsed();
        println!("{what} at {count} cursors: {took:?}");
        assert!(took < bound, "{what} too slow: {took:?}");
    };
    timed(&mut app, "Enter (with the tree)", K::Enter);
    assert!(
        text(&app).starts_with("void function_0() {\n    \n} // {\n}\nvoid function_1() {\n    \n} // {\n}\n"),
        "between a pair in code, a plain break in the comment"
    );
    assert_eq!(sels(&app).len(), count);
    timed(&mut app, "typing } (outdent, no tree)", K::Char('}'));
    assert!(
        text(&app).starts_with("void function_0() {\n}\n} // {\n}}\n"),
        "{:?}",
        &text(&app)[..60]
    );

    // Bracket matching while moving and typing in the generated 1 MB C++ file.
    let text = generated_cpp(size);
    let (mut app, _) = open_app(&dir, "", "gen.cpp", &text);
    assert!(app.settle(parsed).await);
    // A `for (…) {` line in the middle: brackets on most lines around it.
    let line = text
        .lines()
        .enumerate()
        .skip(text.lines().count() / 2)
        .find(|(_, l)| l.contains("for (auto x"))
        .unwrap()
        .0;
    app.press(K::Char('g'), M::CONTROL);
    app.type_text(&(line + 1).to_string());
    app.press(K::Enter, M::NONE);
    app.press(K::End, M::NONE);
    draw(&mut term, &mut app);
    let (mut worst, mut highlighted) = (Duration::ZERO, 0);
    let keys = [K::Left; 40]
        .into_iter()
        .chain([K::Down; 20])
        .chain("x[]".chars().map(K::Char));
    for key in keys {
        let t = Instant::now();
        app.press(key, M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        worst = worst.max(t.elapsed());
        if bracket_cells(&term, &app) > 0 {
            highlighted += 1;
        }
        if !parsed(&app) {
            assert!(app.settle(parsed).await);
        }
    }
    println!("bracket matching: worst frame {worst:?}, {highlighted} frames with a highlight");
    assert!(highlighted > 5);
    if release {
        assert!(worst < FRAME, "a frame took {worst:?}");
    } else {
        assert!(worst < FRAME_DEBUG_BOUND, "a frame took {worst:?}");
    }
}

/// Debug builds only catch going quadratic.
const LINE_DEBUG_BOUND: Duration = Duration::from_secs(5);

/// 8f: a file per new language, opened through the app: detected by name, extension or shebang,
/// named in the status bar, parsed in the background, then drawn in its colours.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_more_languages_are_detected_and_coloured() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, "", "notes.txt", "notes\n");
    let mut term = Terminal::new(TestBackend::new(120, 16)).unwrap();
    // (file, text, language, status bar name, (needle, offset, key) drawn in that colour)
    type Sample = (
        &'static str,
        &'static str,
        Language,
        &'static str,
        &'static [(&'static str, u16, &'static str)],
    );
    let files: &[Sample] = &[
        (
            "src/main.rs",
            "fn main() {\n    let x: u8 = 42; // c\n}\n",
            Language::Rust,
            "Rust",
            &[
                ("fn main", 0, "keyword"),
                ("fn main", 3, "function"),
                ("= 42", 2, "number"),
                ("// c", 0, "comment"),
            ],
        ),
        (
            "README.md",
            "# Title\n\n- item\n> said\n",
            Language::Markdown,
            "Markdown",
            &[
                ("# Title", 2, "markup.heading"),
                ("- item", 0, "markup.list"),
                ("> said", 0, "markup.quote"),
            ],
        ),
        (
            ".github/ci.yml",
            "on:\n  push: true # c\n",
            Language::Yaml,
            "YAML",
            &[
                ("on:", 0, "tag"),
                ("push: true", 6, "constant.builtin"),
                ("# c", 0, "comment"),
            ],
        ),
        (
            "package.json",
            "{\"name\": \"jed\", \"n\": 1}\n",
            Language::Json,
            "JSON",
            &[
                ("\"name\"", 1, "property"),
                ("\"jed\"", 1, "string"),
                (": 1}", 2, "number"),
            ],
        ),
        (
            "tsconfig.json",
            "// c\n{}\n",
            Language::Jsonc,
            "JSON with Comments",
            &[("// c", 0, "comment")],
        ),
        (
            "pom.xml",
            "<project a=\"1\"><!-- c --></project>\n",
            Language::Xml,
            "XML",
            &[
                ("<project", 0, "punctuation.tag"),
                ("<project", 1, "tag"),
                ("a=\"1\"", 0, "tag.attribute"),
                ("a=\"1\"", 3, "string"),
                ("<!-- c", 5, "comment"),
            ],
        ),
        (
            "Cargo.toml",
            "[package]\nname = \"jed\"\n",
            Language::Toml,
            "TOML",
            &[
                ("[package]", 1, "type"),
                ("name =", 0, "property"),
                ("\"jed\"", 1, "string"),
            ],
        ),
        (
            "Cargo.lock",
            "version = 4\n",
            Language::Toml,
            "TOML",
            &[("version = 4", 10, "number")],
        ),
        (
            "build.sh",
            "if true; then echo \"$HOME\"; fi\n",
            Language::ShellScript,
            "Shell Script",
            &[
                ("if true", 0, "keyword.control"),
                ("echo \"$", 0, "function"),
                ("\"$HOME", 0, "string"),
            ],
        ),
        (
            "tool",
            "#!/usr/bin/env bash\nexport X=1\n",
            Language::ShellScript,
            "Shell Script",
            &[("#!/usr", 0, "comment"), ("export X", 0, "keyword")],
        ),
        (
            "CMakeLists.txt",
            "project(jed VERSION 1.0)\n",
            Language::CMake,
            "CMake",
            &[("project(jed", 0, "function"), ("VERSION", 0, "constant")],
        ),
        (
            "Makefile",
            "CC = cc\nall: x.o\n\t$(CC) x.o\n",
            Language::Makefile,
            "Makefile",
            &[
                ("CC = cc", 0, "variable"),
                ("all: x.o", 0, "function"),
                ("$(CC) x", 2, "variable"),
            ],
        ),
        (
            "Dockerfile",
            "FROM ubuntu\nENV A=\"b\"\n",
            Language::Dockerfile,
            "Dockerfile",
            &[
                ("FROM ubuntu", 0, "keyword"),
                ("ENV A", 4, "property"),
                ("\"b\"", 1, "string"),
            ],
        ),
        (
            "web.dockerfile",
            "FROM x\n",
            Language::Dockerfile,
            "Dockerfile",
            &[("FROM x", 0, "keyword")],
        ),
        (
            "Containerfile.dev",
            "FROM y\n",
            Language::Dockerfile,
            "Dockerfile",
            &[("FROM y", 0, "keyword")],
        ),
    ];
    for &(rel, src, language, name, colours) in files {
        let path = write(&root, rel, src);
        app.test_open(Buffer::open(&path).unwrap());
        assert_eq!(app.active_buffer().unwrap().syntax.language(), language, "{rel}");
        let status = status_line(&mut app);
        assert!(
            status.trim_end().ends_with(&format!("LF   {name}")),
            "{rel}: {status:?}"
        );
        assert!(app.settle(parsed).await, "{rel} parsed");
        assert_tree_current(&app);
        draw(&mut term, &mut app);
        for &(needle, offset, key) in colours {
            assert_eq!(
                fg_of(&term, needle, offset).fg,
                fg(&app, key),
                "{rel}: {needle:?}+{offset} as {key}"
            );
        }
    }
    // Headings are bold as well; XML tag brackets are dimmer than the text.
    assert_ne!(fg(&app, "punctuation.tag"), fg(&app, "editor"));
    assert!(
        app.theme
            .get("markup.heading")
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    // Makefiles indent with tabs (VS Code's `[makefile]` default), shown in the status bar.
    assert!(status_line(&mut app).contains("Spaces: 4"), "Containerfile.dev");
    let (mut app, _) = open_app(&dir, "", "Makefile", "all:\n");
    assert!(status_line(&mut app).contains("Tab Size: 4"));
}

/// 8f: comment line, auto-close and auto-indent follow each new language's configuration.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_more_languages_editing() {
    let dir = tempfile::tempdir().unwrap();
    // Cmd+/: `//` in Rust and JSONC, `#` in TOML, YAML and shell; nothing in JSON or Markdown.
    for (rel, src, commented) in [
        ("a.rs", "fn f() {}\n", "// fn f() {}\n"),
        ("b.jsonc", "{}\n", "// {}\n"),
        ("c.toml", "a = 1\n", "# a = 1\n"),
        ("d.yaml", "  a: 1\n", "  # a: 1\n"),
        ("e.sh", "echo hi\n", "# echo hi\n"),
        ("CMakeLists.txt", "project(x)\n", "# project(x)\n"),
        ("Dockerfile", "FROM x\n", "# FROM x\n"),
        ("f.json", "{}\n", "{}\n"),
        ("g.md", "text\n", "text\n"),
        ("h.xml", "<a/>\n", "<a/>\n"),
    ] {
        let (mut app, _) = open_app(&dir, "", rel, src);
        app.press(K::Char('/'), cmd());
        assert_eq!(text(&app), commented, "{rel}");
        app.press(K::Char('/'), cmd());
        assert_eq!(text(&app), src, "{rel} back");
    }

    // JSON: brackets and quotes auto-close, not inside a string (with the tree).
    let (mut app, _) = open_app(&dir, "", "a.json", "\n");
    assert!(app.settle(parsed).await);
    set_cursors(&mut app, &[0]);
    app.type_text("{\"a");
    assert_eq!(text(&app), "{\"a\"}\n");
    app.pump();
    assert!(app.settle(parsed).await);
    app.type_text("[");
    assert_eq!(text(&app), "{\"a[\"}\n", "no pair in a string");
    app.press(K::Backspace, M::NONE);
    app.type_text("\": [");
    assert_eq!(text(&app), "{\"a\": []}\n", "typed over the quote, `[` closed");
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "{\"a\": [\n    \n]}\n");

    // Rust: `'` (lifetimes) isn't closed, `"` is; Enter before a closing bracket outdents (C's
    // `indentationRules`).
    let (mut app, _) = open_app(&dir, "", "b.rs", "");
    assert!(app.settle(parsed).await);
    app.type_text("fn f<'a>(s: &'a str) {");
    assert_eq!(text(&app), "fn f<'a>(s: &'a str) {}");
    app.press(K::Enter, M::NONE);
    app.type_text("g(\"x");
    assert_eq!(text(&app), "fn f<'a>(s: &'a str) {\n    g(\"x\")\n}");
    let (mut app, _) = open_app(&dir, "", "c.rs", "fn f() {\n    x;}\n");
    set_cursors(&mut app, &[15]);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "fn f() {\n    x;\n}\n");

    // YAML: Enter after a key indents.
    let (mut app, _) = open_app(&dir, "", "c.yml", "");
    app.type_text("on:");
    app.press(K::Enter, M::NONE);
    app.type_text("push:");
    app.press(K::Enter, M::NONE);
    app.type_text("- x");
    assert_eq!(text(&app), "on:\n    push:\n        - x");

    // Makefile: Tab and auto-indent insert tabs whatever `insert_spaces` says.
    let (mut app, _) = open_app(&dir, "", "Makefile", "");
    app.type_text("all:");
    app.press(K::Enter, M::NONE);
    app.press(K::Tab, M::NONE);
    app.type_text("cc x.c");
    app.press(K::Enter, M::NONE);
    app.type_text("cc y.c");
    assert_eq!(text(&app), "all:\n\tcc x.c\n\tcc y.c");

    // Markdown: brackets matched from the text (the tree's leaves are whole paragraphs).
    let src = "Some (text [x]) here\n";
    let (mut app, _) = open_app(&dir, "", "d.md", src);
    assert!(app.settle(parsed).await);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    set_cursors(&mut app, &[5]);
    draw(&mut term, &mut app);
    let hl = app.theme.get("editor.bracketMatch").bg;
    assert_eq!(bg_of(&term, "(text", 0), hl);
    assert_eq!(bg_of(&term, "]) here", 1), hl);
    assert_eq!(bracket_cells(&term, &app), 2);
    set_cursors(&mut app, &[20]);
    app.type_text(" <");
    assert_eq!(text(&app), "Some (text [x]) here <>\n", "`<` auto-closes in Markdown");
}

/// 8f, M8's "done when" for the new grammars: a 1 MB file in each language opens with a plain
/// frame, colours arrive from the background parse, and a key plus its re-parse plus a frame stays
/// within a frame (release; debug catches going quadratic).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_8_more_languages_one_mb_frames() {
    let dir = tempfile::tempdir().unwrap();
    let release = !cfg!(debug_assertions);
    let size = if release { 1_000_000 } else { 200_000 };
    let bound = if release { FRAME } else { FRAME_DEBUG_BOUND };
    // (file, one repeated unit with `{i}` numbered, the line typed at and what's typed)
    let samples: &[(&str, &str, &str)] = &[
        (
            "big.rs",
            "fn f{i}(x: &str) -> u32 {\n    let v = vec![1, 2]; // c\n    v.len() as u32\n}\n",
            " let y = 1;",
        ),
        (
            "big.json",
            "  \"key{i}\": {\"a\": [1, 2.5, true, null], \"b\": \"text\"},\n",
            " \"z\": 1,",
        ),
        (
            "big.yaml",
            "item{i}:\n  name: \"x\" # c\n  list:\n    - 1\n    - two\n",
            " k: v",
        ),
        (
            "big.md",
            "## Heading {i}\n\nSome *text* with `code` and [a link](x).\n\n- item\n> quote\n\n",
            " more text",
        ),
        (
            "big.xml",
            "<item id=\"{i}\" a='b'><name>x</name><!-- c --></item>\n",
            " <z/>",
        ),
        ("big.toml", "[t{i}]\nname = \"x\" # c\nlist = [1, 2]\n", " y = 2"),
        (
            "big.sh",
            "f{i}() {\n  if [ -n \"$1\" ]; then echo \"$(ls)\"; fi\n}\n",
            " echo hi",
        ),
    ];
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
    for &(rel, unit, keys) in samples {
        // JSON: a blank first line, so the cursor isn't on the root `{` (matching it walks all its
        // 1 MB of siblings, ~6 ms: 8e's tree matching, the same for any language's huge block).
        let mut text = if rel.ends_with(".json") {
            String::from("\n{\n")
        } else {
            String::new()
        };
        let mut i = 0;
        while text.len() < size {
            text.push_str(&unit.replace("{i}", &i.to_string()));
            i += 1;
        }
        if rel.ends_with(".json") {
            text.push_str("  \"end\": 0\n}\n");
        }
        let (mut app, _) = open_app(&dir, "", rel, &text);
        let t = Instant::now();
        draw(&mut term, &mut app);
        let mut worst = t.elapsed();
        assert!(!parsed(&app), "{rel}: parsed in the background");
        let t = Instant::now();
        while !parsed(&app) {
            assert!(t.elapsed() < Duration::from_secs(20), "{rel}: no tree");
            let f = Instant::now();
            app.pump();
            draw(&mut term, &mut app);
            worst = worst.max(f.elapsed());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let parse = t.elapsed();
        let f = Instant::now();
        draw(&mut term, &mut app);
        worst = worst.max(f.elapsed());
        let lines = text.lines().count();
        app.press(K::Char('g'), M::CONTROL);
        app.type_text(&(lines / 2).to_string());
        app.press(K::Enter, M::NONE);
        app.press(K::End, M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        let mut worst_key = Duration::ZERO;
        for c in keys.chars() {
            let t = Instant::now();
            app.press(K::Char(c), M::NONE);
            app.pump();
            draw(&mut term, &mut app);
            worst_key = worst_key.max(t.elapsed());
            if !parsed(&app) {
                assert!(app.settle(parsed).await, "{rel}");
            }
        }
        println!("{rel}: parse {parse:?}, worst open frame {worst:?}, worst key {worst_key:?}");
        assert!(worst < bound, "{rel}: a frame while opening took {worst:?}");
        assert!(worst_key < bound, "{rel}: a key took {worst_key:?}");
    }
}
