//! The command-driven tests task, subtask command_driven_tests-a: key routing is checked here,
//! once; every other test runs a feature through its command.
//!
//! **The rule: run the command, don't press its key.** A test starts a feature with
//! `app.test_execute(Command::..)`. It presses a Ctrl, Cmd or Alt key only when the test is about
//! key handling (a focused field, a shell tab, an overlay), and then through
//! `app.press_bound(Command::..)`, never a raw platform key. The `command_driven_tests_guard_*`
//! test fails on a `.press(` with such modifiers in any other test file that isn't on its allowlist.
//!
//! What is checked here:
//! - Keymap level, both platforms in one run (`Keymap::default_for(Platform::MacOs / Linux)`):
//!   no two default entries for one (key, context) with different commands, every context's
//!   binding resolves to its command, every global binding is reachable from some context, and
//!   every command a test reaches through `press_bound(Command::..)` is bound on both platforms.
//! - `command_driven_tests_default_keys_match_the_snapshot` pins every default binding to
//!   `command_driven_keys.txt` (`JED_BLESS_KEYS=1` rewrites it; the only test that writes to the tree).
//! - App level, this platform's keymap (`command_driven_tests_route_*`): one app per context, the
//!   record switch (`App::test_dispatch_log`) on, every binding that applies there pressed (its
//!   own, its parents' and the global ones, minus what `passes_through` or a focused field
//!   gives away), and the log must be exactly the bound commands in order. Nothing runs, so one
//!   app covers a context's bindings. To add a context: write a `route_<name>` test that builds
//!   the app in it and calls `check_route(&mut app, Context::X)`, and take it off `NOT_SET_UP`.
//! - The dispatch's special cases, each pressing real keys and running: an open prompt, context
//!   menu, cheat sheet, keyboard shortcuts view and inline rename take a bound key before the
//!   keymap, and the gutter peek takes Esc; a focused field keeps Ctrl/Cmd+A, C, V, X; a shell tab gets plain Ctrl+letters
//!   except Ctrl+Q; Ctrl+C over a terminal selection never copies.
//! - The guard (`command_driven_tests_guard_*`).
//!
//! Contexts that can't be set up headlessly (`NOT_SET_UP`, with reasons), and what's deferred:
//! - `Suggest`, `CodeActions`: the lists come from a language server's answer to a request;
//!   `completion_tests` / `code_action` tests script a fake server for that.
//! - `ContextMenu`: it takes every key before the keymap, so no binding reaches the dispatch
//!   (the special-case test `command_driven_tests_overlays_take_bound_keys_first` covers it).
//! - `Commit`: a commit tab needs a git repository and its history walk.
//! - `Task`: the panel's task tab needs a running task process.
//! - `Composer`: the comment box needs a review diff over a forge fixture.
//! - Deferred: this suite against the other platform's keymap at the app level, and user
//!   `keybindings.toml` overrides.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use jtui::keymap::KeyContext;

use crate::app::{App, Focus};
use crate::buffer::Buffer;
use crate::clipboard::Copied;
use crate::command::Command;
use crate::config::keymap::{self, Context, KeyCombo, Keymap, Platform};
use crate::context_menu::MenuItem;
use crate::layout::{PanelTab, SidePanel};
use crate::shell_tests::{dirs, draw, screen_text, shell_app};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

const PLATFORMS: [Platform; 2] = [Platform::MacOs, Platform::Linux];

/// Contexts the app-level suite can't set up headlessly, with the reason (see the `//!` doc).
const NOT_SET_UP: &[(Context, &str)] = &[
    (Context::Suggest, "needs a language server's completion list"),
    (Context::CodeActions, "needs a language server's code action list"),
    (Context::ContextMenu, "takes every key before the keymap"),
    (Context::Commit, "needs a git repository and its history walk"),
    (Context::Task, "needs a running task process"),
    (Context::Composer, "needs a review diff over a forge fixture"),
];

// ---------------------------------------------------------------------------------------------
// Keymap level
// ---------------------------------------------------------------------------------------------

#[test]
fn command_driven_tests_defaults_have_no_shadowed_entries() {
    for platform in PLATFORMS {
        let mut seen: HashMap<(KeyCombo, Option<Context>), Command> = HashMap::new();
        for b in keymap::default_list(platform) {
            if let Some(first) = seen.insert((b.combo, b.context), b.command) {
                assert_eq!(
                    first,
                    b.command,
                    "{platform:?}: {} in {:?} is bound to {first:?} and later to {:?}, which would silently replace it",
                    b.combo.label(),
                    b.context,
                    b.command
                );
            }
        }
    }
}

#[test]
fn command_driven_tests_every_binding_resolves_to_its_command() {
    for platform in PLATFORMS {
        let km = Keymap::default_for(platform);
        for (combo, context, command) in km.bindings() {
            let ev = KeyEvent::new(combo.code, combo.mods);
            match context {
                Some(ctx) => assert_eq!(
                    km.lookup(&ev, ctx),
                    Some(command),
                    "{platform:?}: {} in {ctx:?} should be {command:?}",
                    combo.label()
                ),
                None => assert!(
                    Context::ALL.iter().any(|(c, _)| km.lookup(&ev, *c) == Some(command)),
                    "{platform:?}: the global {} ({command:?}) is shadowed in every context",
                    combo.label()
                ),
            }
        }
    }
}

/// The command each `press_bound(Command::..)` / `press_field_key(Command::..)` in the test
/// sources names (spelled as its Debug output: `Find(Next)`), with the call and the file it is
/// in. The argument is the whole balanced expression, so `Command::Shell(ShellCommand::Top)` is
/// `Shell(Top)`.
fn press_calls() -> Vec<(&'static str, String, String)> {
    let mut out = Vec::new();
    let mut files = sources();
    files.push((
        "command_driven_tests.rs".into(),
        include_str!("command_driven_tests.rs").into(),
    ));
    for (path, src) in files {
        for call in ["press_bound", "press_field_key"] {
            // This file's own `press_field_key` test names a command the editor doesn't bind.
            if call == "press_field_key" && path == "command_driven_tests.rs" {
                continue;
            }
            let needle = format!(".{call}(Command::");
            let mut rest = src.as_str();
            while let Some(at) = rest.find(&needle) {
                let from = at + needle.len();
                let arg = call_args(&rest[from..]).0;
                let name: String = arg.chars().filter(|c| !c.is_whitespace()).collect();
                // Every `Path::` prefix goes (`Find(FindCommand::Next)` is `Find(Next)`).
                let name = strip_paths(&name);
                if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                    out.push((call, name, path.clone()));
                }
                rest = &rest[from..];
            }
        }
    }
    out
}

/// Drops `Path::` prefixes from `Find(FindCommand::Next)`-style text.
fn strip_paths(text: &str) -> String {
    let mut s = text.to_string();
    while let Some(i) = s.find("::") {
        let start = s[..i]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |p| p + 1);
        s.replace_range(start..i + 2, "");
    }
    s
}

#[test]
fn command_driven_tests_press_calls_are_bound_on_both_platforms() {
    let all: HashMap<String, Command> = Command::all().into_iter().map(|c| (format!("{c:?}"), c)).collect();
    let keymaps: Vec<(Platform, Keymap)> = PLATFORMS.iter().map(|p| (*p, Keymap::default_for(*p))).collect();
    let calls = press_calls();
    assert!(
        calls.iter().any(|(call, ..)| *call == "press_field_key")
            && calls.iter().any(|(_, name, _)| name.contains('(')),
        "the scanner finds press_field_key calls and nested command paths"
    );
    for (call, name, file) in calls {
        // `Command::Foo` with the variant's arguments left out (`Move { .. }`) isn't a name.
        let Some(cmd) = all.get(&name) else {
            panic!("{file}: {call}(Command::{name}) names no command in Command::all()");
        };
        for (platform, km) in &keymaps {
            // A field takes the editor context's key; `press_bound` takes the current context's,
            // which the guard can't know, so any context (or a global) will do for it.
            let bound = if call == "press_field_key" {
                km.bindings().any(|(combo, _, c)| {
                    c == *cmd && km.lookup(&KeyEvent::new(combo.code, combo.mods), Context::Editor) == Some(*cmd)
                })
            } else {
                km.bindings().any(|(_, _, c)| c == *cmd)
            };
            assert!(
                bound,
                "{file}: {call}({cmd:?}) but {platform:?} binds no {}key to it",
                if call == "press_field_key" {
                    "editor-context "
                } else {
                    ""
                }
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// App level: the routing suite
// ---------------------------------------------------------------------------------------------

/// An app on a workspace with `a.txt`, `b.txt`, `bin.dat` (a NUL byte: opens as hex) and
/// `notes.md`, with `a.txt` open.
fn base() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path()).join("proj");
    let state = workspace::normalize(dir.path()).join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(root.join("b.txt"), "one\nthree\n").unwrap();
    fs::write(root.join("bin.dat"), b"\x00\x01\x02\x03binary").unwrap();
    fs::write(root.join("notes.md"), "# Notes\n\ntext\n").unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app.test_open(Buffer::open(&root.join("a.txt")).unwrap());
    (dir, app)
}

fn root_of(app: &App) -> std::path::PathBuf {
    app.workspace.root.clone()
}

/// The commands bound to `ctx`'s bindings, its parents' and the global ones, resolved with no
/// use of `Keymap::lookup`: (key, command) sorted by the key's label.
fn applicable(app: &App, ctx: Context) -> Vec<(KeyCombo, Command)> {
    let mut by_key: BTreeMap<String, (KeyCombo, HashMap<Option<Context>, Command>)> = BTreeMap::new();
    for (combo, c, cmd) in app.keymap.bindings() {
        by_key
            .entry(combo.label())
            .or_insert_with(|| (combo, HashMap::new()))
            .1
            .insert(c, cmd);
    }
    let mut chain = vec![ctx];
    while let Some(parent) = chain.last().unwrap().parent() {
        chain.push(parent);
    }
    let mut out = Vec::new();
    for (combo, bound) in by_key.into_values() {
        let own = chain.iter().find_map(|c| bound.get(&Some(*c)).copied());
        let found = own.or_else(|| {
            if ctx.passes_through(&combo) {
                None
            } else {
                bound.get(&None).copied()
            }
        });
        if let Some(cmd) = found {
            out.push((combo, cmd));
        }
    }
    out
}

/// Builds the log for `ctx`: every applicable binding pressed with nothing running must record
/// exactly its command, in order. (`app` is already in `ctx`.)
fn check_route(app: &mut App, ctx: Context) {
    assert_eq!(app.test_context(), ctx, "the app is set up in {ctx:?}");
    let expected: Vec<(KeyCombo, Command)> = applicable(app, ctx)
        .into_iter()
        .filter(|(_, cmd)| !app.test_field_takes(*cmd))
        .collect();
    assert!(
        expected.len() >= 40,
        "{ctx:?}: only {} bindings pressed",
        expected.len()
    );
    assert!(!expected.is_empty(), "{ctx:?} has bindings to press");
    app.test_dispatch_log = Some(Vec::new());
    for (combo, _) in &expected {
        app.handle_test_event(Event::Key(KeyEvent::new(combo.code, combo.mods)));
        assert_eq!(app.test_context(), ctx, "pressing {} left {ctx:?}", combo.label());
    }
    let log = app.test_dispatch_log.take().unwrap();
    let want: Vec<Command> = expected.iter().map(|(_, c)| *c).collect();
    if log != want {
        let n = log.iter().zip(&want).take_while(|(a, b)| a == b).count();
        let key = expected.get(n).map(|(k, _)| k.label()).unwrap_or_default();
        panic!(
            "{ctx:?}: the log differs from the bindings at #{n} (key {key}): log {:?}, bound {:?} (log has {}, bound {})",
            log.get(n),
            want.get(n),
            log.len(),
            want.len()
        );
    }
}

#[test]
fn command_driven_tests_every_context_is_covered_or_listed() {
    let listed: HashSet<Context> = NOT_SET_UP.iter().map(|(c, _)| *c).collect();
    let covered: HashSet<Context> = COVERED.iter().copied().collect();
    for (ctx, name) in Context::ALL {
        assert!(
            listed.contains(&ctx) ^ covered.contains(&ctx),
            "{name}: either a route_ test (listed in COVERED) or a NOT_SET_UP entry, not both or neither"
        );
    }
}

/// The contexts with a `command_driven_tests_route_*` test below.
const COVERED: &[Context] = &[
    Context::Editor,
    Context::Explorer,
    Context::Palette,
    Context::Find,
    Context::Replace,
    Context::Search,
    Context::SearchResults,
    Context::Problems,
    Context::Locations,
    Context::Diff,
    Context::Scm,
    Context::Tools,
    Context::Review,
    Context::GitHistory,
    Context::FileHistory,
    Context::Hex,
    Context::Markdown,
    Context::Shell,
    Context::TermSearch,
];

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_editor() {
    let (_d, mut app) = base();
    check_route(&mut app, Context::Editor);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_explorer() {
    let (_d, mut app) = base();
    app.test_execute(Command::FocusExplorer);
    check_route(&mut app, Context::Explorer);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_palette() {
    let (_d, mut app) = base();
    app.test_execute(Command::ShowCommands);
    check_route(&mut app, Context::Palette);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_find_and_replace() {
    let (_d, mut app) = base();
    app.test_execute(Command::OpenFind);
    check_route(&mut app, Context::Find);
    app.test_execute(Command::OpenReplace);
    check_route(&mut app, Context::Replace);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_search_and_results() {
    let (_d, mut app) = base();
    app.test_execute(Command::FindInFiles);
    check_route(&mut app, Context::Search);
    app.search_view.list_focused = true;
    check_route(&mut app, Context::SearchResults);
}

fn focus_panel(app: &mut App, tab: PanelTab) {
    app.ui.focus = Focus::Panel;
    app.ui.panel_tab = tab;
}

fn focus_side(app: &mut App, panel: SidePanel) {
    app.ui.focus = Focus::SidePanel;
    app.ui.side_panel = panel;
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_bottom_panel_lists() {
    let (_d, mut app) = base();
    focus_panel(&mut app, PanelTab::Problems);
    check_route(&mut app, Context::Problems);
    focus_panel(&mut app, PanelTab::Locations);
    check_route(&mut app, Context::Locations);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_side_panels() {
    let (_d, mut app) = base();
    focus_side(&mut app, SidePanel::SourceControl);
    check_route(&mut app, Context::Scm);
    focus_side(&mut app, SidePanel::Tools);
    check_route(&mut app, Context::Tools);
    focus_side(&mut app, SidePanel::Review);
    check_route(&mut app, Context::Review);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_history_sections() {
    let (_d, mut app) = base();
    focus_side(&mut app, SidePanel::Explorer);
    app.git_history.set_focused(true);
    check_route(&mut app, Context::GitHistory);
    app.git_history.set_focused(false);
    app.file_history_view.set_focused(true);
    check_route(&mut app, Context::FileHistory);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_diff() {
    let (_d, mut app) = base();
    let root = root_of(&app);
    app.open_compare(
        crate::diff_view::Source::File(root.join("a.txt")),
        crate::diff_view::Source::File(root.join("b.txt")),
    );
    assert!(app.settle(|a| a.diffs_settled()).await, "the diff loaded");
    check_route(&mut app, Context::Diff);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_hex() {
    let (_d, mut app) = base();
    let root = root_of(&app);
    app.test_open_file(root.join("bin.dat"));
    assert!(app.settle(|a| a.active_hex().is_some()).await, "bin.dat opened as hex");
    check_route(&mut app, Context::Hex);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_markdown() {
    let (_d, mut app) = base();
    let root = root_of(&app);
    app.test_open_file(root.join("notes.md"));
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path.ends_with("notes.md")))
            .await,
        "notes.md opened"
    );
    app.test_execute(Command::OpenMarkdownPreview);
    if app.test_context() != Context::Markdown {
        app.test_execute(Command::FocusNextGroup);
    }
    check_route(&mut app, Context::Markdown);
}

/// An app with a shell tab in the editor (`/bin/sh`), its prompt up.
async fn shell_tab() -> (tempfile::TempDir, App) {
    let (dir, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    assert!(
        app.settle(|a| !screen_text(a).trim().is_empty()).await,
        "the shell printed its prompt"
    );
    (dir, app)
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_shell_and_its_search_bar() {
    let (_d, mut app) = shell_tab().await;
    check_route(&mut app, Context::Shell);
    app.test_execute(Command::TermSearch(crate::command::TermSearchCommand::Open));
    check_route(&mut app, Context::TermSearch);
}

// ---------------------------------------------------------------------------------------------
// The dispatch's special cases (they run, with real keys)
// ---------------------------------------------------------------------------------------------

/// Presses the key `cmd` has in the editor context, whatever context is up now (an overlay's
/// own context may not bind it: what is tested is that the overlay takes the key first).
fn press_editor_key_of(app: &mut App, cmd: Command) {
    let combo = app
        .keymap
        .binding_for(cmd)
        .unwrap_or_else(|| panic!("{cmd:?} has a binding"));
    app.handle_test_event(Event::Key(KeyEvent::new(combo.code, combo.mods)));
}

/// With an overlay up, a key bound to `cmd` is the overlay's: `cmd` doesn't run.
fn assert_overlay_takes(app: &mut App, still_up: impl Fn(&App) -> bool, what: &str) {
    assert!(still_up(app), "{what} is up");
    let before = app.last_command;
    // Save has a key everywhere; with a prompt or menu up it must not save.
    press_editor_key_of(app, Command::Save);
    assert_eq!(app.last_command, before, "{what}: the bound key ran no command");
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_overlays_take_bound_keys_first() {
    // An open prompt: closing a dirty tab asks.
    let (_d, mut app) = base();
    app.type_text("x");
    app.test_execute(Command::CloseTab);
    assert_overlay_takes(&mut app, |a| a.prompt.is_some(), "the unsaved-changes prompt");
    assert!(app.active_buffer().unwrap().is_dirty(), "nothing was saved");

    // A context menu.
    let (_d, mut app) = base();
    app.open_context_menu((5, 5), vec![MenuItem::new("Close", Command::CloseTab)]);
    assert_eq!(app.test_context(), Context::ContextMenu);
    assert_overlay_takes(&mut app, |a| a.context_menu.is_some(), "the context menu");

    // The cheat sheet and the keyboard shortcuts view.
    let (_d, mut app) = base();
    app.test_execute(Command::CheatSheet);
    assert_overlay_takes(&mut app, |a| a.cheat_sheet.is_some(), "the cheat sheet");
    let (_d, mut app) = base();
    app.test_execute(Command::KeyboardShortcuts);
    assert_overlay_takes(&mut app, |a| a.keys.is_some(), "the keyboard shortcuts view");

    // An inline rename in the Explorer.
    let (_d, mut app) = base();
    app.test_execute(Command::FocusExplorer);
    assert!(
        app.settle(|a| !a.explorer.rows().is_empty()).await,
        "the listing arrived"
    );
    app.press(KeyCode::Down, KeyModifiers::NONE);
    app.test_execute(Command::Explorer(crate::command::ExplorerCommand::Rename));
    assert_overlay_takes(&mut app, |a| a.inline_rename.is_some(), "the inline rename");
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_a_focused_field_keeps_its_edit_keys() {
    let (_d, mut app) = base();
    app.test_execute(Command::OpenFind);
    let selections = app.group().active_tab().unwrap().view.selections.clone();
    app.type_text("abc");
    assert_eq!(app.find.query.text, "abc");

    // Select all is an editor-context binding: the find context doesn't bind it, so the field
    // takes the editor's key (press_bound would panic here: no key resolves to it in Find; the
    // same goes for copy, cut and paste).
    press_editor_key_of(&mut app, Command::SelectAll);
    assert_eq!(
        app.find.query.selection(),
        Some(0..3),
        "select all selected the field's text"
    );
    assert_eq!(
        app.group().active_tab().unwrap().view.selections,
        selections,
        "the editor's selection is untouched"
    );

    app.clipboard.write(Copied {
        text: "keep".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    press_editor_key_of(&mut app, Command::Copy);
    assert_eq!(app.clipboard.read().unwrap(), "abc", "copy took the field's selection");
    assert_eq!(app.find.query.text, "abc");

    press_editor_key_of(&mut app, Command::Cut);
    assert_eq!(app.find.query.text, "", "cut removed it from the field");
    assert_eq!(app.clipboard.read().unwrap(), "abc");

    press_editor_key_of(&mut app, Command::Paste);
    assert_eq!(app.find.query.text, "abc", "paste went into the field");
    assert_eq!(
        app.active_buffer().unwrap().rope.to_string(),
        "one\ntwo\n",
        "the editor's text is untouched"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "no key resolves to SelectAll in context Find")]
async fn command_driven_tests_press_bound_names_the_command_and_context_when_unbound() {
    let (_d, mut app) = base();
    app.test_execute(Command::OpenFind);
    // Select all is the editor context's binding; the find context has none.
    app.press_bound(Command::SelectAll);
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "press_field_key: the editor context binds no key to Shell(Top)")]
async fn command_driven_tests_press_field_key_names_the_command_when_the_editor_binds_none() {
    let (_d, mut app) = base();
    // Shell top is the shell context's binding; the editor context has none.
    app.press_field_key(Command::Shell(crate::command::ShellCommand::Top));
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_a_shell_tab_gets_plain_ctrl_letters_except_ctrl_q() {
    let (_d, mut app) = shell_tab().await;
    app.test_dispatch_log = Some(Vec::new());
    // Ctrl+D would end the shell, Ctrl+K is Clear (the shell context's own binding on Linux).
    for c in ['r', 'w', 'a', 'e', 'u', 's', 'f', 'o', 'p'] {
        app.handle_test_event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
    }
    assert_eq!(
        app.test_dispatch_log.as_deref(),
        Some(&[][..]),
        "plain Ctrl+letters went to the shell, not the keymap"
    );
    app.handle_test_event(Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)));
    // Ctrl+Q is Quit where the keymap binds it (Cmd+Q is macOS's, where Ctrl+Q goes to the shell).
    let want: &[Command] = if cfg!(target_os = "macos") {
        &[]
    } else {
        &[Command::Quit]
    };
    assert_eq!(app.test_dispatch_log.as_deref(), Some(want), "Ctrl+Q");
    app.test_dispatch_log = None;
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_terminal_copy_key_and_ctrl_c() {
    let (_d, mut app) = shell_tab().await;
    assert!(app.group().active_shell().unwrap().run.write(b"printf 'alpha\\n'\n"));
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "alpha"))
            .await,
        "the command printed its line"
    );
    let text = screen_text(&app);
    let row = text.lines().position(|l| l.trim_end() == "alpha").unwrap() as u16;
    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    use crossterm::event::{MouseButton, MouseEventKind};
    let select = |app: &mut App| {
        app.mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y + row);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), area.x + 5, area.y + row);
        app.mouse(MouseEventKind::Up(MouseButton::Left), area.x + 5, area.y + row);
        assert!(
            app.group().active_shell().unwrap().run.selection().is_some(),
            "the drag selected text"
        );
        app.clipboard.write(Copied {
            text: "keep".into(),
            whole_line: false,
            pieces: Vec::new(),
        });
    };
    // The platform's copy key (Cmd+C on macOS, Ctrl+Shift+C on Linux) copies the selection.
    select(&mut app);
    app.press_bound(Command::Copy);
    assert_eq!(
        app.clipboard.read().unwrap(),
        "alpha",
        "the copy key copied the selection"
    );
    // Ctrl+C alone is the program's SIGINT, selection or not.
    select(&mut app);
    app.handle_test_event(Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    assert_eq!(app.clipboard.read().unwrap(), "keep", "Ctrl+C never copies");
}

// ---------------------------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------------------------

const GUARD_HELP: &str = "Tests run features through their command, not key presses: start a feature with \
`app.test_execute(Command::..)`. Press a Ctrl, Cmd or Alt key only when the test is about key handling \
(a focused field, a shell tab, an overlay), and then through `app.press_bound(Command::..)`, never a raw \
platform key. To keep a press, add the file (or the line) with its reason to the guard's allowlist in \
jed/src/command_driven_tests.rs.";

/// Files with presses not yet migrated to `test_execute` (path under `jed/src`, reason). Each
/// migration subtask removes its files; the guard fails on an entry with nothing left to allow.
const ALLOWED_FILES: &[(&str, &str)] = &[
    ("app/disk.rs", "not migrated yet: subtask d"),
    ("app/mod.rs", "not migrated yet: subtask d"),
    ("app/palette.rs", "not migrated yet: subtask d"),
    ("apple_langs_tests.rs", "not migrated yet: subtask d"),
    ("clear_scrollback_tests.rs", "not migrated yet: subtask d"),
    ("compare_files_tests.rs", "not migrated yet: subtask c"),
    ("conflicts_tests.rs", "not migrated yet: subtask c"),
    ("focus_groups_tests.rs", "not migrated yet: subtask d"),
    ("fuzz_tests.rs", "not migrated yet: subtask d"),
    ("git_editor_tests.rs", "not migrated yet: subtask c"),
    ("git_history_tests.rs", "not migrated yet: subtask c"),
    ("git_marks_tests.rs", "not migrated yet: subtask c"),
    ("hex_more_tests.rs", "not migrated yet: subtask c"),
    ("hex_tests.rs", "not migrated yet: subtask c"),
    ("history_view_tests.rs", "not migrated yet: subtask c"),
    ("kitty_recent_tests.rs", "not migrated yet: subtask d"),
    ("markdown_preview_tests.rs", "not migrated yet: subtask c"),
    ("milestone10_tests.rs", "not migrated yet: subtask c"),
    ("milestone11_tests.rs", "not migrated yet: subtask c"),
    ("milestone12_tests.rs", "not migrated yet: subtask c"),
    ("milestone13_tests.rs", "not migrated yet: subtask c"),
    ("milestone9_tests.rs", "not migrated yet: subtask c"),
    ("mr_review_tests.rs", "not migrated yet: subtask d"),
    ("mr_review_tests/done_when.rs", "not migrated yet: subtask d"),
    ("mr_review_tests/github.rs", "not migrated yet: subtask d"),
    ("mr_review_tests/gitlab.rs", "not migrated yet: subtask d"),
    ("mr_review_tests/live.rs", "not migrated yet: subtask d"),
    ("mr_review_tests/suggest.rs", "not migrated yet: subtask d"),
    ("review_conversation_tests.rs", "not migrated yet: subtask d"),
    ("review_fixes_tests.rs", "not migrated yet: subtask d"),
    ("review_forge_fixes_tests.rs", "not migrated yet: subtask d"),
    ("shell_tests.rs", "not migrated yet: subtask d"),
    ("spell_check_tests.rs", "not migrated yet: subtask c"),
    ("syntax_plugins_tests.rs", "not migrated yet: subtask d"),
    ("term_area_tests.rs", "not migrated yet: subtask d"),
    ("term_search_tests.rs", "not migrated yet: subtask d"),
    ("term_select_tests.rs", "not migrated yet: subtask d"),
    ("tools_tests.rs", "not migrated yet: subtask d"),
    ("typescript_tests.rs", "not migrated yet: subtask c"),
];

/// Single presses that stay: (path under `jed/src`, text of the line, reason). The guard fails
/// on an entry whose line is gone.
const ALLOWED_LINES: &[(&str, &str, &str)] = &[
    (
        "todo_tests.rs",
        "app.press(K::Char('y'), M::CONTROL);",
        "the test's own keybindings.toml binds ctrl+y: the key is the subject, it is no default binding",
    ),
    (
        "todo_tests.rs",
        "app.press(K::Char('k'), M::CONTROL);",
        "the test's own keybindings.toml binds ctrl+k in the find context",
    ),
    (
        "todo_tests.rs",
        "app.press(K::Char('h'), M::CONTROL | M::ALT);",
        "the live-reload test's keybindings.toml binds ctrl+alt+h",
    ),
    (
        "todo_tests.rs",
        "app.press(paste.code, paste.mods);",
        "the key the keymap binds to Paste (none on macOS, where Cmd+V is the terminal's): the field takes it",
    ),
    (
        "todo_tests.rs",
        "app.press(K::Char('a'), M::CONTROL); // select all in the editor",
        "no binding in the context menu: the test is that a key the menu does not bind is swallowed",
    ),
    (
        "milestone5_tests.rs",
        "app.press(save_key.code, save_key.mods);",
        "the test's keybindings.toml rebinds file.save: the default Save key is pressed to show it no longer saves",
    ),
    (
        "milestone5_tests.rs",
        "app.press(K::Char('s'), M::CONTROL | M::ALT);",
        "the key the test's keybindings.toml binds to file.save",
    ),
    (
        "milestone5_tests.rs",
        "app.press(sidebar_key.code, sidebar_key.mods);",
        "the test's keybindings.toml binds the side bar key to palette.next in the palette context only",
    ),
    (
        "milestone6_tests.rs",
        "app.press(K::Char('n'), M::CONTROL);",
        "the test's keybindings.toml binds ctrl+n to find.next in the find context; the editor's own Ctrl+N is checked after",
    ),
    (
        "milestone7_tests.rs",
        "app.press(K::Char('t'), M::CONTROL);",
        "the test's keybindings.toml binds ctrl+t to search.toggleCase in the search context",
    ),
    (
        "milestone7_tests.rs",
        "app.press(default_toggle.code, default_toggle.mods);",
        "the default toggle key, which the test's keybindings.toml has taken the command from",
    ),
    (
        "keys_tests.rs",
        "app.press(K::Char('k'), M::ALT);",
        "Alt+K is the Keyboard Shortcuts view's record key: it is no command binding",
    ),
    (
        "keys_tests.rs",
        "app.press(code, mods);",
        "the key recorded by the Keyboard Shortcuts view's record mode: the key is the subject",
    ),
];

/// Every `.rs` under `jed/src` with its path relative to it (`/`-separated), except this file.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                if rel != "command_driven_tests.rs" {
                    out.push((rel, fs::read_to_string(&path).unwrap()));
                }
            }
        }
    }
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    out
}

/// The text of a call's arguments after its `(` up to the matching `)`, and where that `)` is.
fn call_args(after_open: &str) -> (&str, usize) {
    let mut depth = 0usize;
    for (i, c) in after_open.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return (&after_open[..i], i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    (after_open, after_open.len())
}

/// The second argument of a call's text, split at the first top-level comma.
fn second_arg(args: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (i, c) in args.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some(args[i + 1..].trim().trim_end_matches(',').trim()),
            _ => {}
        }
    }
    None
}

/// Whether a modifier expression is plainly none or Shift (`M::NONE`, `KeyModifiers::SHIFT`,
/// `NONE | SHIFT`, `KeyModifiers::empty()`). Anything else (CONTROL, SUPER, ALT, a helper such as
/// `cmd()`, a variable) can be a platform key.
fn plain_modifiers(expr: &str) -> bool {
    expr.split('|').all(|part| {
        let part = part.trim();
        let name = part.rsplit("::").next().unwrap_or(part);
        matches!(name, "NONE" | "SHIFT" | "empty()")
    })
}

/// The `.press(code, mods)` calls in `src` whose modifiers aren't plainly none / Shift: (1-based
/// line, the call's first line trimmed). Calls inside `//` comment lines are skipped.
fn flagged_presses(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for (n, line) in src.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//")
            && let Some(at) = line.find(".press(")
        {
            let from = offset + at + ".press(".len();
            let (args, _) = call_args(&src[from..]);
            if let Some(mods) = second_arg(args)
                && !plain_modifiers(mods)
            {
                out.push((n + 1, line.trim().to_string()));
            }
        }
        offset += line.len();
    }
    out
}

#[test]
fn command_driven_tests_guard_finds_platform_key_presses() {
    let flagged = |src: &str| flagged_presses(src).len();
    // The task's example: flagged until it is `test_execute`.
    assert_eq!(flagged("app.press(KeyCode::Char('s'), KeyModifiers::CONTROL);"), 1);
    assert_eq!(flagged("app.test_execute(Command::Save);"), 0);
    // Multi-line, combined and helper forms are caught.
    assert_eq!(
        flagged("app.press(\n    KeyCode::Char('s'),\n    KeyModifiers::CONTROL,\n);"),
        1
    );
    assert_eq!(flagged("a.press(K::Left, M::CONTROL | M::SHIFT);"), 1);
    assert_eq!(flagged("a.press(KeyCode::Char('p'), cmd());"), 1);
    assert_eq!(flagged("a.press(KeyCode::Char('p'), primary);"), 1);
    assert_eq!(flagged("a.press(KeyCode::Char('n'), KeyModifiers::ALT);"), 1);
    assert_eq!(
        flagged("a.press(KeyCode::Char('p'), KeyModifiers::SUPER | KeyModifiers::SHIFT);"),
        1
    );
    // Plain keys, Shift and commented calls aren't.
    assert_eq!(flagged("a.press(KeyCode::Enter, KeyModifiers::NONE);"), 0);
    assert_eq!(flagged("a.press(K::Tab, M::SHIFT);"), 0);
    assert_eq!(flagged("a.press(K::Tab, M::NONE | M::SHIFT);"), 0);
    assert_eq!(flagged("a.press(K::Tab, KeyModifiers::empty());"), 0);
    assert_eq!(flagged("// a.press(K::Char('s'), M::CONTROL)"), 0);
    assert_eq!(
        flagged("a.press(KeyCode::Char(','), cmd());\na.press(K::Esc, M::NONE);"),
        1
    );
    assert_eq!(
        flagged_presses("x\n  a.press(K::Left, M::ALT);").first().map(|f| f.0),
        Some(2)
    );
}

#[test]
fn command_driven_tests_guard_no_new_platform_key_presses() {
    let mut problems = Vec::new();
    let mut files_with_presses: HashSet<String> = HashSet::new();
    let mut lines_used: HashSet<(&str, &str)> = HashSet::new();
    for (path, src) in sources() {
        let mut flagged = flagged_presses(&src);
        flagged.retain(|(_, text)| {
            ALLOWED_LINES
                .iter()
                .find(|(p, t, _)| *p == path && text.contains(t))
                .map(|(p, t, _)| lines_used.insert((p, t)))
                .is_none()
        });
        if flagged.is_empty() {
            continue;
        }
        files_with_presses.insert(path.clone());
        if ALLOWED_FILES.iter().any(|(p, _)| *p == path) {
            continue;
        }
        for (line, text) in flagged {
            problems.push(format!("{path}:{line}: {text}"));
        }
    }
    assert!(
        problems.is_empty(),
        "{GUARD_HELP}\n\nPresses with Ctrl, Cmd or Alt (or a helper):\n{}",
        problems.join("\n")
    );
    for (path, reason) in ALLOWED_FILES {
        assert!(
            files_with_presses.contains(*path),
            "{path} is on the allowlist ({reason}) but has no such press left: remove the entry"
        );
    }
    for (path, text, reason) in ALLOWED_LINES {
        assert!(
            lines_used.contains(&(*path, *text)),
            "{path}: no press line contains {text:?} ({reason}): remove the entry"
        );
    }
}

/// The gutter's peek takes Enter (stage) and Esc before the keymap: Esc closes the peek and
/// leaves the selection that `CancelSelection` would collapse; the next Esc reaches the keymap.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_the_gutter_peek_takes_esc_before_the_keymap() {
    let (_d, root, state) = dirs();
    let original: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let changed = original.replace("line 20\n", "twenty\n");
    crate::milestone10_tests::repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = crate::milestone10_tests::open_file_app(&root, &state, "f.txt");
    assert!(crate::milestone10_tests::settle_markers(&mut app).await);
    // Scroll line 20 into view, then click its marker.
    for _ in 0..20 {
        app.test_execute(Command::Move {
            motion: crate::command::Motion::Down,
            select: false,
        });
    }
    let _ = draw(&mut app, 100, 30);
    let text_rect = app.hits.group(app.groups.active).unwrap().text;
    let top = app.group().active_tab().unwrap().view.top_line();
    let y = text_rect.y + (20 - top) as u16;
    let click = |app: &mut App| {
        app.mouse(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            text_rect.x - 1,
            y,
        );
        app.mouse(
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
            text_rect.x - 1,
            y,
        );
    };
    click(&mut app);
    assert!(app.quick_diff.peek.is_some(), "the peek is open");
    app.test_execute(Command::SelectAll);
    let selected = |app: &App| {
        let buf = app.active_buffer().unwrap();
        let sel = &app.group().active_tab().unwrap().view.selections;
        sel.iter().any(|s| !s.is_empty()) && buf.rope.len() > 0
    };
    assert!(selected(&app), "something is selected");
    // Esc closes the peek and nothing else: CancelSelection did not run.
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.quick_diff.peek.is_none());
    assert!(selected(&app), "the first Esc went to the peek, not CancelSelection");
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!selected(&app), "with no peek, Esc is CancelSelection");
}

/// Every default binding as `<platform> <context or global> <key> <command>`, sorted: the
/// checked-in `command_driven_keys.txt` pins the default keys, since the routing tests derive
/// their keys from the keymap and so follow a rebinding.
fn key_snapshot() -> String {
    let mut lines = Vec::new();
    for (name, platform) in [("macos", Platform::MacOs), ("linux", Platform::Linux)] {
        for b in keymap::default_list(platform) {
            let ctx = b.context.map_or("global".to_string(), |c| format!("{c:?}"));
            lines.push(format!("{name} {ctx} {} {:?}", b.combo.label(), b.command));
        }
    }
    lines.sort();
    lines.dedup();
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The only test that writes to the source tree, and only with `JED_BLESS_KEYS=1`.
#[test]
fn command_driven_tests_default_keys_match_the_snapshot() {
    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/command_driven_keys.txt");
    let now = key_snapshot();
    if std::env::var_os("JED_BLESS_KEYS").is_some() {
        fs::write(PATH, &now).unwrap();
        return;
    }
    let saved = include_str!("command_driven_keys.txt");
    if saved == now {
        return;
    }
    let old: HashSet<&str> = saved.lines().collect();
    let new: HashSet<&str> = now.lines().collect();
    let mut removed: Vec<&str> = old.difference(&new).copied().collect();
    let mut added: Vec<&str> = new.difference(&old).copied().collect();
    removed.sort();
    added.sort();
    panic!(
        "the default key bindings changed.\nremoved:\n  {}\nadded:\n  {}\nIf that is intended, update jed/src/command_driven_keys.txt with `JED_BLESS_KEYS=1 scripts/check test default_keys` (and the macOS table in docs/terminals/README.md for a Cmd binding).",
        removed.join("\n  "),
        added.join("\n  ")
    );
}

/// `has_field_meaning` matters when the keymap itself resolves a key to copy / cut / paste in a
/// field's context (a user binding here): the field takes the command, the editor doesn't run
/// it. (The default keys are editor-context ones, which a field's own handling reads.)
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_a_field_bound_cut_and_paste_act_on_the_field() {
    let (_d, mut app) = base();
    let bind = |ctrl_alt: char, command| keymap::Binding {
        combo: KeyCombo {
            code: KeyCode::Char(ctrl_alt),
            mods: KeyModifiers::CONTROL | KeyModifiers::ALT,
        },
        context: Some(Context::Find),
        command,
        source: jtui::keymap::Source::User,
    };
    app.keymap = Keymap::with_user(
        Platform::current(),
        &[bind('x', Command::Cut), bind('v', Command::Paste)],
    );
    app.test_execute(Command::OpenFind);
    app.type_text("abc");
    press_editor_key_of(&mut app, Command::SelectAll);
    let press = |app: &mut App, c: char| {
        app.handle_test_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
    };
    press(&mut app, 'x');
    assert_eq!(app.find.query.text, "", "cut removed the field's text");
    assert_eq!(app.clipboard.read().unwrap(), "abc");
    press(&mut app, 'v');
    assert_eq!(app.find.query.text, "abc", "paste went into the field");
    assert_eq!(
        app.active_buffer().unwrap().rope.to_string(),
        "one\ntwo\n",
        "the editor's text is untouched"
    );
}
