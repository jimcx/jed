//! Syntax plugins end to end. syntax_plugins-b, without native code: files detected as a plugin
//! language open as it but uncoloured; its `line_comment` and `pairs` drive Comment Line and
//! auto-close; `.workspace` associations and `[lsp.<id>]` (and the user's `settings.toml`
//! `[lsp.<id>]`) name plugin ids; detection order (filenames, extensions beating built-ins,
//! shebangs after built-ins). syntax_plugins-c: a prebuilt grammar loaded at start colours its
//! files (open before and after it arrives), a broken one is reported and leaves them uncoloured,
//! an app loads only its own `syntax_plugins`. Every test registers a plugin under its own id and
//! claims only extensions, names and shebangs of its own: the registry is process-global.

use std::path::Path;

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::Prompt;
use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::syntax::plugin::{self, PluginId};
use crate::syntax::{Associations, Language, detect};
use crate::workspace::Workspace;
use crate::workspace::trust::{Trust, TrustStore};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// Registers the manifest `text` (its id must be unique to the test).
fn register(text: &str) -> Language {
    let (m, errors) = plugin::parse(text, "language.toml");
    assert!(errors.is_empty(), "{errors:?}");
    Language::Plugin(plugin::register(m.unwrap()).unwrap())
}

/// A workspace `<tmp>/proj` with its trust store in `<tmp>/state` (never outside the temp dir).
fn root_of(dir: &tempfile::TempDir) -> std::path::PathBuf {
    crate::milestone12_tests::dirs(dir).0
}

fn app_on(root: &Path, first: Option<&str>) -> App {
    let initial = first.map(|f| InitialFile {
        buffer: Buffer::open(&root.join(f)).unwrap(),
        goto: None,
    });
    let store = TrustStore::load(root.parent().unwrap().join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.to_path_buf()), store, initial, true);
    app.test_plain_whitespace();
    app
}

fn status_bar(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let y = buf.area.height - 1;
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

const LUAISH: &str = r#"
id = "ptb_lua"
name = "Luaish"
line_comment = "--"
pairs = [{ open = "(", close = ")" }, { open = "\"", close = "\"", not_in = ["string"] }]
[files]
extensions = ["ptbl"]
filenames = ["Ptbfile"]
shebangs = ["ptblua"]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_a_plugin_file_opens_uncoloured_and_edits_by_its_config() {
    let lang = register(LUAISH);
    assert_eq!(lang.name(), "Luaish");
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join("a.ptbl"), "x = 1\n").unwrap();
    let mut app = app_on(&root, Some("a.ptbl"));
    app.test_start();
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.pump();
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.syntax.language(), lang);
    assert!(buf.syntax.tree().is_none());
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(status_bar(&term).contains("Luaish"), "{}", status_bar(&term));
    // No grammar: nothing is coloured, whatever the size.
    let (buf, _) = app.active_mut().unwrap();
    let rope = buf.rope.clone();
    assert!(buf.syntax.spans(&rope, 0..1).is_empty());

    // Comment Line inserts the manifest's token.
    app.press(K::Char('/'), cmd());
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "-- x = 1\n");
    // Auto-close per `pairs`: `(` and `"` close, `[` does not.
    app.press(K::End, M::NONE);
    app.type_text("(");
    app.type_text("\"");
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "-- x = 1(\"\")\n");
    app.press(K::End, M::NONE);
    app.type_text("[");
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "-- x = 1(\"\")[\n");
}

#[test]
fn syntax_plugins_detection_order() {
    // Only claims that no other test's detection can see: the registry is process-global, so a
    // claim that wins (`.h`, say) would change other tests' files. The claims that must lose
    // (`Pipfile`, `lock`, `sh`) are built-in names, and lose whenever the order is right.
    register(
        r#"
id = "ptb_order"
[files]
extensions = ["rbxmx", "ptbo.tpl", "ptbo", "lock"]
filenames = ["Ptbmakefile", "ptbname.c", "Pipfile"]
shebangs = ["ptbo", "python-ptbo", "sh"]
"#,
    );
    let order = Language::from_id("ptb_order").unwrap();
    let none = Associations::default();
    let d = |path: &str, first: &str| detect(Path::new(path), Path::new("/ws"), first, &none);
    assert_eq!(d("/ws/a.ptbo", ""), order);
    assert_eq!(d("/ws/A.PTBO", ""), order);
    assert_eq!(d("/ws/dir/a.b.ptbo.tpl", ""), order);
    assert_eq!(d("/ws/Ptbmakefile", ""), order);
    assert_eq!(
        d("/ws/x.RBXMX", ""),
        order,
        "a plugin's extension beats the built-in one (XML)"
    );
    assert_eq!(
        d("/ws/ptbname.c", ""),
        order,
        "a plugin's file name beats a built-in extension"
    );
    assert_eq!(d("/ws/other.c", ""), Language::C);
    assert_eq!(
        d("/ws/.ptbo", ""),
        Language::PlainText,
        "a dotfile's name isn't an extension"
    );
    // Built-in exact names beat plugin file names and extensions.
    assert_eq!(d("/ws/Pipfile", ""), Language::Toml);
    assert_eq!(d("/ws/uv.lock", ""), Language::Toml);
    assert_eq!(d("/ws/x.lock", ""), order);
    // Shebangs: built-in programs first, then plugins' (versions allowed); no extension needed.
    assert_eq!(d("/ws/run", "#!/usr/bin/env ptbo"), order);
    assert_eq!(d("/ws/run", "#!/usr/bin/ptbo2 -x"), order);
    assert_eq!(d("/ws/run", "#!/usr/bin/env python-ptbo"), order);
    assert_eq!(d("/ws/run", "#!/usr/bin/python3"), Language::Python);
    assert_eq!(
        d("/ws/run", "#!/bin/sh"),
        Language::ShellScript,
        "the built-in `sh` first"
    );
    assert_eq!(d("/ws/run", "#!/bin/ptbox"), Language::PlainText, "not a version");
    // The file name decides before the shebang.
    assert_eq!(d("/ws/a.c", "#!/usr/bin/env ptbo"), Language::C);
    assert_eq!(d("/ws/Ptbmakefile", "#!/bin/bash"), order);
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_workspace_association_names_a_plugin_id() {
    let lang = register("id = \"ptb_assoc\"\nname = \"Assoc\"\nline_comment = \";\"\n");
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join("a.ptbassoc"), "x\n").unwrap();
    std::fs::write(root.join("b.txt"), "y\n").unwrap();
    std::fs::write(
        root.join(".workspace"),
        "[files.associations]\n\"*.ptbassoc\" = \"ptb_assoc\"\n\"b.txt\" = \"ptb_assoc\"\n\"c.txt\" = \"ptb_nope\"\n",
    )
    .unwrap();
    let mut app = app_on(&root, Some("a.ptbassoc"));
    app.test_start();
    app.pump();
    assert_eq!(app.active_buffer().unwrap().syntax.language(), lang);
    // The unknown id is reported, as for a built-in typo.
    let ws = Workspace::load(root.clone());
    let (_, errors) = Associations::new(&ws.config.files.associations);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("ptb_nope"));
    // Comment Line through the association.
    app.press(K::Char('/'), cmd());
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "; x\n");
}

/// A plugin's server, started on its file through the app: the user's `settings.toml` entry
/// (wired as `main` does: `settings::parse` → `user_servers` → `Builtins::user`) while the
/// workspace is undecided-then-restricted, the `.workspace` entry once trusted, the user's again
/// after a `.workspace` reload drops its entry.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lsp_for_a_plugin_id() {
    let lang = register("id = \"ptb_lsp\"\n[files]\nextensions = [\"ptblsp\"]\n");
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    std::fs::write(root.join("a.ptblsp"), "x\n").unwrap();
    std::fs::write(
        root.join(".workspace"),
        "[lsp.ptb_lsp]\ncommand = \"ws-server\"\nargs = [\"--ws\"]\n",
    )
    .unwrap();
    let user = crate::config::settings::parse("[lsp.ptb_lsp]\ncommand = \"user-server\"\nargs = [\"-u\"]\n");
    assert!(user.errors.is_empty(), "{:?}", user.errors);
    let (servers, errors) = crate::lsp::config::user_servers(&user.lsp);
    assert!(errors.is_empty(), "{errors:?}");

    let initial = InitialFile {
        buffer: Buffer::open(&root.join("a.ptblsp")).unwrap(),
        goto: None,
    };
    let store = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), store, Some(initial), true);
    app.lsp.builtins.user = servers;
    let fake = FakeLauncher::new(FakeOptions::default());
    app.lsp.set_launcher(fake.clone());
    app.test_start();
    let config = |app: &App| {
        app.lsp
            .server_for(lang)
            .map(|s| (s.config.name.clone(), s.config.args.clone(), s.config.from_workspace))
    };
    let user_server = Some(("user-server".to_string(), vec!["-u".to_string()], false));
    let ws_server = Some(("ws-server".to_string(), vec!["--ws".to_string()], true));

    // Undecided: the trust prompt, and no server (not even the user's) until it's answered.
    assert_eq!(app.trust, Trust::Undecided);
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
    app.pump();
    assert_eq!(config(&app), None);
    assert!(fake.launches().is_empty(), "{:?}", fake.launches());

    // Restricted: the user's server, and the file is opened on it as the plugin's language.
    app.press(K::Char('r'), M::NONE);
    assert_eq!(app.trust, Trust::Restricted);
    let u = crate::milestone9_tests::uri(&root.join("a.ptblsp"));
    assert!(
        app.settle(|a| config(a) == user_server && fake.document(&u).is_some())
            .await,
        "{:?} {:?}",
        config(&app),
        fake.launches()
    );
    assert_eq!(fake.document(&u).unwrap().language, "ptb_lsp");

    // Trusted: the workspace's entry wins.
    app.test_execute(Command::ManageWorkspaceTrust);
    app.press(K::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);
    assert!(
        app.settle(|a| config(a) == ws_server && fake.launches().len() == 2)
            .await,
        "{:?} {:?}",
        config(&app),
        fake.launches()
    );
    assert_eq!(fake.launches(), ["user-server", "ws-server"]);

    // A reload without the entry: the user's server again.
    std::fs::write(root.join(".workspace"), "").unwrap();
    assert!(
        app.settle(|a| config(a) == user_server && fake.launches().len() == 3)
            .await,
        "{:?} {:?}",
        config(&app),
        fake.launches()
    );
    assert_eq!(fake.launches(), ["user-server", "ws-server", "user-server"]);
    app.shutdown_servers().await;
}

#[test]
fn syntax_plugins_user_lsp_ids_are_checked_against_the_plugins() {
    register("id = \"ptb_userlsp\"\n");
    let user = crate::config::settings::parse(
        "[lsp.ptb_userlsp]\ncommand = \"a\"\n[lsp.python]\ncommand = \"pyright\"\n[lsp.ptb_missing]\ncommand = \"b\"\n",
    );
    assert!(user.errors.is_empty(), "{:?}", user.errors);
    let (servers, errors) = crate::lsp::config::user_servers(&user.lsp);
    assert_eq!(
        servers.keys().map(String::as_str).collect::<Vec<_>>(),
        ["ptb_userlsp", "python"]
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("ptb_missing"));
    assert!(PluginId::find("ptb_userlsp").is_some());
}

/// Opens file `a.<ext>` holding `text` in an app started with only the plugin `id` (from a built
/// library in its own directory) in `app.syntax_plugins`; the temp dirs (plugin, workspace).
fn app_with_built_plugin(
    id: &str,
    ext: &str,
    abi12: bool,
    symbol: &str,
    text: &str,
) -> Option<(tempfile::TempDir, tempfile::TempDir, App, PluginId)> {
    use plugin::test_lib;
    let lib = test_lib::library(abi12)?;
    let plugins = tempfile::tempdir().unwrap();
    let query = include_str!("syntax/queries/cmake.scm");
    test_lib::plugin_dir(plugins.path(), id, ext, symbol, lib, query);
    let pid = test_lib::register_dir(plugins.path(), id);
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join(format!("a.{ext}")), text).unwrap();
    let mut app = app_on(&root, Some(&format!("a.{ext}")));
    app.syntax_plugins = vec![pid];
    app.test_start();
    Some((plugins, dir, app, pid))
}

/// The grammar is loaded in the background after start: a file open before it arrives is
/// coloured once it does, and so is one opened after (and a text no buffer holds, as a diff
/// tab's sides).
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_loaded_grammar_colours_its_files() {
    let text = "project(demo)\nset(NAME \"x\") # note\n";
    let Some((_plugins, dir, mut app, pid)) =
        app_with_built_plugin("ptc_good", "ptcgood", false, "tree_sitter_cmake", text)
    else {
        return;
    };
    let lang = Language::Plugin(pid);
    assert_eq!(app.active_buffer().unwrap().syntax.language(), lang);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await,
        "no parse"
    );
    let spans = |app: &mut App| {
        let (buf, _) = app.active_mut().unwrap();
        let rope = buf.rope.clone();
        buf.syntax.spans(&rope, 0..2).to_vec()
    };
    let first = spans(&mut app);
    assert!(first.len() >= 3, "{first:?}");
    assert!(app.message.is_none(), "{:?}", app.message.as_ref().map(|m| &m.text));

    // Opened after the load.
    let b = root_of(&dir).join("b.ptcgood");
    std::fs::write(&b, text).unwrap();
    app.test_open(Buffer::open(&b).unwrap());
    assert_eq!(app.active_buffer().unwrap().syntax.language(), lang);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await,
        "no parse of the file opened later"
    );
    assert_eq!(spans(&mut app), first);
    let sides = crate::syntax::highlight_text(lang, text);
    assert_eq!(sides, first);
}

/// A plugin registered but not in `app.syntax_plugins` (another app's) is never loaded by it.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_an_app_loads_only_its_own_plugins() {
    let Some(lib) = plugin::test_lib::library(false) else {
        return;
    };
    let plugins = tempfile::tempdir().unwrap();
    let query = include_str!("syntax/queries/cmake.scm");
    plugin::test_lib::plugin_dir(plugins.path(), "ptc_other", "ptcother", "tree_sitter_cmake", lib, query);
    let pid = plugin::test_lib::register_dir(plugins.path(), "ptc_other");
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join("a.ptcother"), "project(x)\n").unwrap();
    let mut app = app_on(&root, Some("a.ptcother"));
    app.test_start();
    // Long enough for a load (a few ms) to have finished and been handled.
    let started = std::time::Instant::now();
    assert!(
        app.settle(|_| started.elapsed() > std::time::Duration::from_millis(300))
            .await
    );
    assert!(Language::Plugin(pid).grammar().is_none());
    assert!(app.active_buffer().unwrap().syntax.tree().is_none());
}

/// A library of ABI 12, a missing symbol: reported, the file stays uncoloured, jed keeps working.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_broken_grammars_are_reported_and_leave_the_file_uncoloured() {
    for (id, abi12, symbol, want) in [
        ("ptc_abi12", true, "tree_sitter_cmake", "ABI version 12"),
        ("ptc_nosym", false, "tree_sitter_absent", "tree_sitter_absent"),
    ] {
        let Some((_plugins, _dir, mut app, pid)) = app_with_built_plugin(id, id, abi12, symbol, "project(x)\n") else {
            return;
        };
        assert!(app.settle(|a| a.message.is_some()).await, "{id}: no message");
        let m = app.message.as_ref().unwrap();
        assert!(
            m.text.starts_with(&format!("Syntax plugin {id}: ")) && m.text.contains(want),
            "{}",
            m.text
        );
        assert_eq!(m.level, crate::app::Level::Error);
        assert!(Language::Plugin(pid).grammar().is_none());
        app.type_text("hi");
        assert!(app.settle(|_| true).await);
        let (buf, _) = app.active_mut().unwrap();
        assert!(buf.syntax.tree().is_none());
        let rope = buf.rope.clone();
        assert!(buf.syntax.spans(&rope, 0..1).is_empty());
        assert!(rope.to_string().starts_with("hiproject"));
    }
}

// syntax_plugins-e: building a grammar from `src/` in the background, its Tools row and log,
// failures, and "Languages: Rebuild Syntax Plugins".

/// A plugin directory `<parent>/<id>` with the template's `src/` and query and no library
/// (`None`, with a note, without a C compiler).
fn source_plugin(parent: &Path, id: &str, ext: &str) -> Option<PluginId> {
    plugin::test_lib::library(false)?;
    let dir = parent.join(id);
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/languages/cmake/src");
    std::fs::create_dir_all(dir.join("src/tree_sitter")).unwrap();
    std::fs::create_dir_all(dir.join("queries")).unwrap();
    for f in [
        "parser.c",
        "scanner.c",
        "tree_sitter/alloc.h",
        "tree_sitter/array.h",
        "tree_sitter/parser.h",
    ] {
        std::fs::copy(from.join(f), dir.join("src").join(f)).unwrap();
    }
    std::fs::write(
        dir.join("language.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nsymbol = \"tree_sitter_cmake\"\n[files]\nextensions = [\"{ext}\"]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("queries/highlights.scm"),
        include_str!("syntax/queries/cmake.scm"),
    )
    .unwrap();
    Some(plugin::test_lib::register_dir(parent, id))
}

/// A started app with file `a.<ext>` open and only `pid` in `syntax_plugins`.
fn app_for_source_plugin(dir: &tempfile::TempDir, pid: PluginId, ext: &str) -> App {
    let root = root_of(dir);
    std::fs::write(root.join(format!("a.{ext}")), "project(demo)\nset(NAME \"x\") # note\n").unwrap();
    let mut app = app_on(&root, Some(&format!("a.{ext}")));
    app.syntax_plugins = vec![pid];
    app.test_start();
    app
}

fn grammar_row(app: &App, id: &str) -> crate::tools_view::Row {
    app.tools_rows()
        .into_iter()
        .find(|r| matches!(r, crate::tools_view::Row::Grammar { id: i, .. } if i == id))
        .expect("the grammar's row")
}

fn grammar_state(app: &App, id: &str) -> crate::tools_view::GrammarState {
    match grammar_row(app, id) {
        crate::tools_view::Row::Grammar { state, .. } => state,
        _ => unreachable!(),
    }
}

fn coloured(app: &mut App) -> bool {
    let (buf, _) = app.active_mut().unwrap();
    let rope = buf.rope.clone();
    buf.syntax.tree().is_some() && buf.syntax.spans(&rope, 0..2).len() >= 3
}

fn rebuild_through_palette(app: &mut App) {
    let title = "Languages: Rebuild Syntax Plugins";
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(K::Enter, M::NONE);
}

/// First start: the row is queued at once, the build runs off the UI thread and its command line
/// and compiler output land in the grammar's log; the file is coloured when it finishes. The next
/// start finds the stamp current: nothing is compiled.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_build_at_start_then_up_to_date() {
    use crate::tools_view::GrammarState as G;
    let plugins = tempfile::tempdir().unwrap();
    let Some(pid) = source_plugin(plugins.path(), "pte_first", "ptefirst") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for_source_plugin(&dir, pid, "ptefirst");
    assert_ne!(grammar_state(&app, "pte_first"), G::Ready, "not built yet");
    assert!(!coloured(&mut app));
    // While the start's job runs, the rebuild command refuses to start a second one.
    rebuild_through_palette(&mut app);
    let m = app.message.take().expect("a message");
    assert!(m.text.contains("already building"), "{}", m.text);
    assert!(app.settle(|a| grammar_state(a, "pte_first") == G::Ready).await);
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    assert!(coloured(&mut app));
    let tool = crate::logs::Tool::Grammar("pte_first".into());
    let log = app.logs.texts(&tool);
    assert!(
        log[0].starts_with("$ ") && log[0].contains("-O2 -I src src/parser.c"),
        "{log:?}"
    );
    assert_eq!(log.last().unwrap(), "grammar loaded");
    assert_eq!(
        log.iter().filter(|l| l.starts_with("$ ")).count(),
        1,
        "one build: {log:?}"
    );
    assert!(plugins.path().join("pte_first/build.toml").is_file());
    assert!(app.message.is_none());

    // The Tools row opens the log.
    assert_eq!(grammar_row(&app, "pte_first").tool(), Some(tool.clone()));

    // A second app on the same directory: stamp current, nothing compiled.
    let dir2 = tempfile::tempdir().unwrap();
    let mut app2 = app_for_source_plugin(&dir2, pid, "ptefirst");
    assert!(app2.settle(|a| grammar_state(a, "pte_first") == G::Ready).await);
    assert_eq!(app2.logs.texts(&tool), ["up to date", "grammar loaded"]);
    assert!(
        app2.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some())
            .await
    );
    assert!(coloured(&mut app2));
}

/// A compile error: one message, the row failed with an error badge and the compiler's output in
/// its log, the file uncoloured and editable. Fixing the source and running the rebuild command
/// loads the grammar and colours the file; a second rebuild of the loaded grammar says the new
/// build is used from the next start.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_build_failure_is_reported_and_rebuild_recovers() {
    use crate::tools_view::GrammarState as G;
    let plugins = tempfile::tempdir().unwrap();
    let Some(pid) = source_plugin(plugins.path(), "pte_broken", "ptebroken") else {
        return;
    };
    let parser = plugins.path().join("pte_broken/src/parser.c");
    let good = std::fs::read_to_string(&parser).unwrap();
    std::fs::write(&parser, format!("{good}\nthis is not C;\n")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for_source_plugin(&dir, pid, "ptebroken");
    assert!(app.settle(|a| grammar_state(a, "pte_broken") == G::Failed).await);
    assert!(app.settle(|a| a.message.is_some()).await);
    let m = app.message.as_ref().unwrap();
    assert_eq!(m.level, crate::app::Level::Error);
    assert!(
        m.text.starts_with("Syntax plugin pte_broken: ") && m.text.contains("error"),
        "{}",
        m.text
    );
    assert!(grammar_row(&app, "pte_broken").errors() > 0);
    let tool = crate::logs::Tool::Grammar("pte_broken".into());
    let log = app.logs.texts(&tool);
    assert!(log[0].starts_with("$ ") && log.len() > 2, "{log:?}");
    assert!(!plugins.path().join("pte_broken").join(plugin::LIBRARY).exists());
    app.type_text("hi");
    assert!(app.settle(|_| true).await);
    assert!(!coloured(&mut app));
    assert!(app.active_buffer().unwrap().rope.to_string().starts_with("hiproject"));

    // Fixed; the command rebuilds and loads it.
    std::fs::write(&parser, good).unwrap();
    rebuild_through_palette(&mut app);
    assert!(app.settle(|a| grammar_state(a, "pte_broken") == G::Ready).await);
    assert_eq!(grammar_row(&app, "pte_broken").errors(), 0, "the log starts afresh");
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    assert!(Language::Plugin(pid).grammar().is_some());

    // Loaded already: rebuilt on disk, used from the next start.
    app.message = None;
    rebuild_through_palette(&mut app);
    assert!(app.settle(|a| a.message.is_some()).await);
    let text = &app.message.as_ref().unwrap().text;
    assert!(text.contains("rebuilt") && text.contains("next start"), "{text}");
    assert_eq!(grammar_state(&app, "pte_broken"), G::Ready);

    // A failed rebuild of a grammar in use: the file stays coloured and the row ready, but the
    // failure is still reported and badged.
    let good = std::fs::read_to_string(&parser).unwrap();
    std::fs::write(&parser, format!("{good}\nthis is not C;\n")).unwrap();
    app.message = None;
    rebuild_through_palette(&mut app);
    assert!(app.settle(|a| a.message.is_some()).await);
    let m = app.message.as_ref().unwrap();
    assert!(
        m.level == crate::app::Level::Error && m.text.contains("error"),
        "{}",
        m.text
    );
    assert_eq!(grammar_state(&app, "pte_broken"), G::Ready);
    assert!(grammar_row(&app, "pte_broken").errors() > 0);
    assert!(coloured(&mut app));
}
