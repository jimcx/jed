//! The command-driven tests task, subtask command_driven_tests-e: jmux's key routing is checked
//! here, once; every other jmux test runs a feature through its command (`app.run(Command::..)`,
//! `test_harness.rs`). A raw Ctrl, Cmd or Alt press elsewhere fails the guard in
//! `jed/src/command_driven_tests.rs` (`command_driven_tests_guard_*`, which walks `jed/src` and
//! `jmux/src`); `press_bound(Command::..)` is for tests about key handling.
//!
//! What is checked here:
//! - Keymap level, both platforms in one run (`keymap::defaults`): every binding resolves to its
//!   command in its context (a global one in some context), and every command a test reaches
//!   through `press_bound(Command::..)` is bound on both platforms. (That no (key, context) is
//!   bound twice and every command has a key is `keymap.rs`'s own test.)
//! - `command_driven_tests_default_keys_match_the_snapshot` pins every default binding to
//!   `command_driven_keys.txt` (`JMUX_BLESS_KEYS=1` rewrites it; the only test that writes to the tree).
//! - App level, this platform's keymap (`command_driven_tests_route_*`): one app per context, the
//!   record switch (`App::test_dispatch_log`) on, every binding that applies there pressed (its
//!   own, its parents' and the global ones, minus what `passes_through` gives away), and the log
//!   must be exactly the bound commands in order. Nothing runs, so one app covers a context.
//!   The Side context is re-focused before each press, since a terminal command takes the keys back.
//! - The dispatch's special cases, each pressing real keys: a key release does nothing; a held key
//!   repeats only the tab cycling and the side bar's / fields' moves; a terminal command takes
//!   the side bar's keys back and the bar's own commands do not; the close question swallows every
//!   key it doesn't bind; the side bar and the search bar take unbound keys themselves; the
//!   terminal gets unbound keys and plain Ctrl+letters (`passes_through`), which no command takes.
//! - The guard's own tests live with the guard, in jed's file.
//!
//! jmux has no prefix key: every key is either a binding, typed into the focused part, or sent to
//! the shell, so there is no prefix state to check. All four contexts can be set up headlessly
//! (`Context::{Terminal, Confirm, Search, Side}`), so nothing is listed as not set up. Deferred:
//! this suite against the other platform's keymap at the app level, and user `keybindings.toml`
//! overrides (`jmux_config_b_*` covers one).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use jtui::keymap::{KeyCombo, KeyContext, Platform};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::{App, AppEvent, Config, Confirm};
use crate::command::{Command, Context, SearchCommand, SideCommand};
use crate::keymap::{JmuxKeymap, defaults};
use crate::protocol::{ClientMsg, PROTOCOL_VERSION};

const PLATFORMS: [Platform; 2] = [Platform::MacOs, Platform::Linux];
const CONTEXTS: [Context; 4] = [Context::Terminal, Context::Confirm, Context::Search, Context::Side];

// ---------------------------------------------------------------------------------------------
// Keymap level
// ---------------------------------------------------------------------------------------------

#[test]
fn command_driven_tests_every_binding_resolves_to_its_command() {
    for platform in PLATFORMS {
        let km = JmuxKeymap::from_list(defaults(platform));
        for (combo, context, command, _) in km.bindings_with_source() {
            let ev = KeyEvent::new(combo.code, combo.mods);
            match context {
                Some(ctx) => assert_eq!(
                    km.lookup(&ev, ctx),
                    Some(command),
                    "{platform:?}: {} in {ctx:?} should be {command:?}",
                    combo.label()
                ),
                None => assert!(
                    CONTEXTS.iter().any(|c| km.lookup(&ev, *c) == Some(command)),
                    "{platform:?}: the global {} ({command:?}) is shadowed in every context",
                    combo.label()
                ),
            }
        }
    }
}

/// Drops `Path::` prefixes from `Search(SearchCommand::Open)`-style text.
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

/// The text of a call's arguments after its `(` up to the matching `)`.
fn call_args(after_open: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in after_open.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return &after_open[..i];
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    after_open
}

/// The command each `press_bound(Command::..)` in jmux's sources names (as its Debug output,
/// `Side(Focus)`), with the file it is in.
fn press_calls() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")) {
        let src = fs::read_to_string(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let needle = ".press_bound(Command::";
        let mut rest = src.as_str();
        while let Some(at) = rest.find(needle) {
            let from = at + needle.len();
            let arg: String = call_args(&rest[from..])
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            let arg = strip_paths(&arg);
            if arg.starts_with(|c: char| c.is_ascii_uppercase()) {
                out.push((arg, name.clone()));
            }
            rest = &rest[from..];
        }
    }
    out
}

#[test]
fn command_driven_tests_press_calls_are_bound_on_both_platforms() {
    let all: HashSet<String> = Command::all().into_iter().map(|c| format!("{c:?}")).collect();
    let calls = press_calls();
    assert!(
        calls.iter().any(|(name, _)| name.contains('(')),
        "the scanner finds press_bound calls and nested command paths"
    );
    for (name, file) in calls {
        assert!(
            all.contains(&name),
            "{file}: press_bound(Command::{name}) names no command"
        );
        for platform in PLATFORMS {
            let bound = defaults(platform).iter().any(|b| format!("{:?}", b.command) == name);
            assert!(bound, "{file}: press_bound({name}) but {platform:?} binds no key to it");
        }
    }
}

/// Every default binding as `<platform> <context or global> <key> <command>`, sorted: the
/// checked-in `command_driven_keys.txt` pins the default keys, since the routing tests derive
/// their keys from the keymap and so follow a rebinding.
fn key_snapshot() -> String {
    let mut lines = Vec::new();
    for (name, platform) in [("macos", Platform::MacOs), ("linux", Platform::Linux)] {
        for b in defaults(platform) {
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

/// The only test that writes to the source tree, and only with `JMUX_BLESS_KEYS=1`.
#[test]
fn command_driven_tests_default_keys_match_the_snapshot() {
    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/command_driven_keys.txt");
    let now = key_snapshot();
    if std::env::var_os("JMUX_BLESS_KEYS").is_some() {
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
        "jmux's default key bindings changed.\nremoved:\n  {}\nadded:\n  {}\nIf that is intended, update jmux/src/command_driven_keys.txt with `JMUX_BLESS_KEYS=1 scripts/check test default_keys` (and the macOS table in docs/terminals/README.md for a Cmd binding).",
        removed.join("\n  "),
        added.join("\n  ")
    );
}

// ---------------------------------------------------------------------------------------------
// App level: the routing suite
// ---------------------------------------------------------------------------------------------

struct Rig {
    app: App,
    // The event queue and the writer's queue; kept so frames sent to the client do not fail.
    _events: UnboundedReceiver<AppEvent>,
    _conn_rx: tokio::sync::mpsc::Receiver<crate::protocol::ServerMsg>,
}

/// A started app on the fake shell with a client attached at 24x80 and its first shell up.
async fn rig() -> Rig {
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let config = Config {
        name: "work".into(),
        socket: PathBuf::new(),
        program: jterm::testing::fake_shell(),
        cwd: std::env::temp_dir(),
        open_url: |_| Ok(()),
        dirs: None,
        settings: Default::default(),
        settings_path: None,
        settings_errors: Vec::new(),
        keybindings_path: None,
        keymap: None,
    };
    let mut app = App::new(config, tx);
    app.start();
    let (ctx, conn_rx) = tokio::sync::mpsc::channel(256);
    let writer = tokio::spawn(std::future::pending::<()>());
    app.handle(AppEvent::Connected {
        conn: 1,
        tx: ctx,
        writer,
    });
    let msg = ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        rows: 24,
        cols: 80,
        truecolor: true,
        term: "xterm".into(),
        touch: false,
    };
    app.handle(AppEvent::Client { conn: 1, msg });
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.groups.active_group().active_tab().is_none() {
        assert!(Instant::now() < deadline, "the first shell did not start");
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(10), events.recv()).await {
            app.handle(ev);
        }
    }
    Rig {
        app,
        _events: events,
        _conn_rx: conn_rx,
    }
}

/// Puts the app in `ctx`.
fn enter(app: &mut App, ctx: Context) {
    match ctx {
        Context::Terminal => {}
        Context::Side => app.run(Command::Side(SideCommand::Focus)),
        Context::Search => app.run(Command::Search(SearchCommand::Open)),
        Context::Confirm => {
            let tab = app.groups.active_group().active_tab().unwrap();
            app.confirm = Some(Confirm {
                run: tab.run.id,
                name: "sh".into(),
            });
        }
    }
    assert_eq!(app.context(), ctx, "the app is set up in {ctx:?}");
}

/// The commands bound to `ctx`'s bindings, its parents' and the global ones, resolved with no
/// use of `Keymap::lookup`: (key, command) sorted by the key's label.
fn applicable(app: &App, ctx: Context) -> Vec<(KeyCombo, Command)> {
    let mut by_key: BTreeMap<String, (KeyCombo, HashMap<Option<Context>, Command>)> = BTreeMap::new();
    for (combo, c, cmd, _) in app.keymap.bindings_with_source() {
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

/// Every applicable binding pressed must record exactly its command, in order. At least `min`
/// bindings apply. (`app` is already in `ctx`.)
fn check_route(app: &mut App, ctx: Context, min: usize) {
    assert_eq!(app.context(), ctx, "the app is set up in {ctx:?}");
    let expected = applicable(app, ctx);
    assert!(
        expected.len() >= min,
        "{ctx:?}: only {} bindings pressed",
        expected.len()
    );
    app.test_dispatch_log = Some(Vec::new());
    for (combo, _) in &expected {
        // A terminal command takes the side bar's keys back before it is recorded.
        if ctx == Context::Side {
            app.side.focused = true;
        }
        assert_eq!(app.context(), ctx, "before pressing {}", combo.label());
        app.press(combo.code, combo.mods);
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
fn command_driven_tests_every_context_is_covered() {
    // Adding a `Context` variant makes this match fail to compile until it has a route test.
    for ctx in CONTEXTS {
        match ctx {
            Context::Terminal | Context::Confirm | Context::Search | Context::Side => {}
        }
    }
    let names: HashSet<&str> = <Context as KeyContext>::names().into_iter().collect();
    assert_eq!(names.len(), CONTEXTS.len(), "every context is in CONTEXTS: {names:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_terminal() {
    let mut r = rig().await;
    check_route(&mut r.app, Context::Terminal, 40);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_side() {
    let mut r = rig().await;
    enter(&mut r.app, Context::Side);
    check_route(&mut r.app, Context::Side, 40);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_search() {
    let mut r = rig().await;
    enter(&mut r.app, Context::Search);
    check_route(&mut r.app, Context::Search, 40);
}

#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_route_confirm() {
    let mut r = rig().await;
    enter(&mut r.app, Context::Confirm);
    check_route(&mut r.app, Context::Confirm, 4);
}

// ---------------------------------------------------------------------------------------------
// Special cases: keys that go to a part instead of the keymap, and the dispatch's own rules
// ---------------------------------------------------------------------------------------------

fn plain(app: &mut App, code: KeyCode) {
    app.press(code, KeyModifiers::NONE);
}

fn with_kind(app: &mut App, code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) {
    let mut ev = KeyEvent::new(code, mods);
    ev.kind = kind;
    app.send_input(Event::Key(ev));
}

/// The key bound to `command` in the app's keymap (this platform's).
fn combo_of(app: &App, command: Command) -> KeyCombo {
    app.keymap.binding_for(command).expect("a default key")
}

/// A key release does nothing, and a held key repeats only the tab cycling (and the side bar's
/// and fields' moves): a repeat of the new-terminal key is ignored.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_releases_do_nothing_and_only_some_keys_repeat() {
    let mut r = rig().await;
    let app = &mut r.app;
    app.test_dispatch_log = Some(Vec::new());
    let new = combo_of(app, Command::NewTerminal);
    let next = combo_of(app, Command::NextTab);
    with_kind(app, new.code, new.mods, KeyEventKind::Release);
    with_kind(app, new.code, new.mods, KeyEventKind::Repeat);
    with_kind(app, next.code, next.mods, KeyEventKind::Release);
    assert_eq!(
        app.test_dispatch_log.as_deref(),
        Some(&[][..]),
        "releases and a held new-tab key do nothing"
    );
    with_kind(app, next.code, next.mods, KeyEventKind::Repeat);
    with_kind(app, new.code, new.mods, KeyEventKind::Press);
    assert_eq!(
        app.test_dispatch_log.take().unwrap(),
        vec![Command::NextTab, Command::NewTerminal],
        "a held next-tab key repeats; a press runs its command"
    );
}

/// A held key (`KeyEventKind::Repeat`) of every binding records its command exactly when the
/// command is one of the repeating ones, in each context.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_every_repeating_command_repeats_and_no_other() {
    let repeats = |c: Command| {
        matches!(
            c,
            Command::NextTab
                | Command::PrevTab
                | Command::Field(_)
                | Command::Side(SideCommand::Up | SideCommand::Down | SideCommand::PageUp | SideCommand::PageDown)
        )
    };
    let mut r = rig().await;
    let app = &mut r.app;
    let mut seen: HashSet<Command> = HashSet::new();
    for ctx in [Context::Terminal, Context::Side, Context::Search] {
        // Which context the app is in is set up fresh for every key.
        for (combo, cmd) in applicable(app, ctx) {
            app.side.focused = false;
            if app.context() == Context::Search {
                app.run(Command::Search(SearchCommand::Close));
            }
            enter(app, ctx);
            app.test_dispatch_log = Some(Vec::new());
            with_kind(app, combo.code, combo.mods, KeyEventKind::Repeat);
            let log = app.test_dispatch_log.take().unwrap();
            let want: Vec<Command> = if repeats(cmd) { vec![cmd] } else { vec![] };
            assert_eq!(log, want, "{ctx:?}: held {} ({cmd:?})", combo.label());
            if repeats(cmd) {
                seen.insert(cmd);
            }
        }
    }
    for must in [
        Command::NextTab,
        Command::PrevTab,
        Command::Side(SideCommand::Up),
        Command::Side(SideCommand::Down),
    ] {
        assert!(seen.contains(&must), "{must:?} was checked as repeating");
    }
    assert!(
        seen.iter().any(|c| matches!(c, Command::Field(_))),
        "a field command was checked"
    );
}

/// A terminal command, by its key or by `run`, takes the side bar's keys back; a side command
/// keeps them.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_terminal_commands_release_the_side_bar_by_key_and_by_run() {
    let mut r = rig().await;
    let app = &mut r.app;
    enter(app, Context::Side);
    app.press_bound(Command::Side(SideCommand::Down));
    assert!(app.side.focused, "a side command keeps the focus (key)");
    app.run(Command::Side(SideCommand::Down));
    assert!(app.side.focused, "a side command keeps the focus (run)");
    app.press_bound(Command::NewTerminal);
    assert!(!app.side.focused, "NewTerminal by its key released the bar");
    assert_eq!(app.context(), Context::Terminal);
    enter(app, Context::Side);
    app.run(Command::NewTerminal);
    assert!(!app.side.focused, "NewTerminal by run released the bar");
    assert_eq!(app.context(), Context::Terminal);
}

/// A terminal command takes the keys back from the side bar; its own commands keep them.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_a_terminal_command_takes_the_keys_back_from_the_side_bar() {
    let mut r = rig().await;
    let app = &mut r.app;
    enter(app, Context::Side);
    // A side bar command keeps the focus.
    app.press_bound(Command::Side(SideCommand::Down));
    assert_eq!(app.context(), Context::Side);
    // A terminal command (the search key) gives the keys back.
    app.press_bound(Command::Search(SearchCommand::Open));
    assert!(!app.side.focused, "the terminal's command took the keys back");
    assert_eq!(app.context(), Context::Search);
}

/// The close question swallows every key it doesn't bind, Ctrl chords included, and the shell
/// sees none of them.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_the_close_question_swallows_unbound_keys() {
    let mut r = rig().await;
    let app = &mut r.app;
    enter(app, Context::Confirm);
    app.test_dispatch_log = Some(Vec::new());
    app.needs_redraw = false;
    plain(app, KeyCode::Char('x'));
    plain(app, KeyCode::Tab);
    plain(app, KeyCode::F(5));
    app.press(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.test_dispatch_log.take().unwrap(), vec![], "no command ran");
    assert!(app.confirm.is_some(), "the question is still open");
    assert!(!app.needs_redraw, "nothing was typed into the shell");
    app.press_bound(Command::ConfirmNo);
    assert!(app.confirm.is_none());
}

/// The side bar's filter and the search bar's query take the keys no binding takes, never the shell.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_the_bars_take_unbound_keys_themselves() {
    let mut r = rig().await;
    let app = &mut r.app;
    enter(app, Context::Side);
    app.type_text("zq");
    assert_eq!(app.side.filter.text, "zq");
    app.press_bound(Command::Side(SideCommand::Back));
    assert!(!app.side.focused);
    enter(app, Context::Search);
    app.type_text("qz");
    assert_eq!(app.search.as_ref().unwrap().field.text, "qz");
    app.press_bound(Command::Search(SearchCommand::Close));
    assert_eq!(app.context(), Context::Terminal);
}

/// In the terminal, unbound keys and plain Ctrl+letters go to the shell (nothing is recorded,
/// the screen is marked for a redraw); a key a command takes goes to no shell.
#[tokio::test(flavor = "multi_thread")]
async fn command_driven_tests_the_terminal_gets_unbound_keys_and_plain_ctrl_letters() {
    let mut r = rig().await;
    let app = &mut r.app;
    app.test_dispatch_log = Some(Vec::new());
    for (code, mods) in [
        (KeyCode::Char('a'), KeyModifiers::NONE),
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Char('d'), KeyModifiers::CONTROL),
        (KeyCode::Char('b'), KeyModifiers::CONTROL),
    ] {
        app.needs_redraw = false;
        app.press(code, mods);
        assert!(app.needs_redraw, "{code:?} {mods:?} went to the shell");
    }
    assert_eq!(app.test_dispatch_log.take().unwrap(), vec![], "no command took them");
    // A bound key goes to its command instead (recorded, nothing written).
    app.needs_redraw = false;
    app.test_dispatch_log = Some(Vec::new());
    app.press_bound(Command::NewTerminal);
    assert_eq!(app.test_dispatch_log.take().unwrap(), vec![Command::NewTerminal]);
    assert!(!app.needs_redraw, "the key did not reach the shell");
}

/// Removing a binding `press_bound` relies on fails with the command and context named.
#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "press_bound: no key resolves to NewTerminal in context Confirm")]
async fn command_driven_tests_press_bound_names_the_command_and_context_when_unbound() {
    let mut r = rig().await;
    enter(&mut r.app, Context::Confirm);
    r.app.press_bound(Command::NewTerminal);
}
