//! Syntax plugins end to end. syntax_plugins-b, without native code: files detected as a plugin
//! language open as it but uncoloured; its `line_comment` and `pairs` drive Comment Line and
//! auto-close; `.workspace` associations and `[lsp.<id>]` (and the user's `settings.toml`
//! `[lsp.<id>]`) name plugin ids; detection order (filenames, extensions beating built-ins,
//! shebangs after built-ins). syntax_plugins-c: a prebuilt grammar loaded at start colours its
//! files (open before and after it arrives), a broken one is reported and leaves them uncoloured,
//! an app loads only its own `syntax_plugins`. Every test registers a plugin under its own id and
//! claims only extensions, names and shebangs of its own: the registry is process-global.
//!
//! syntax_plugins-g, the "Done when" end to end. With a C compiler: the CMake template copied in
//! as `cmake2` colours a `CMakeLists.txt` as the built-in does (`..._template_copied_as_cmake2_...`;
//! the template's own claims leave that file the built-in's, same test); a first start builds in
//! the background and frames stay in budget meanwhile (`..._frames_stay_in_budget_...`); a
//! `.workspace` can't add a plugin (`..._workspace_cannot_add_a_plugin`); a broken query is
//! reported in the status bar and the grammar's log (`..._broken_query_...`, the other broken
//! cases in `..._broken_grammars_...`). With a tree-sitter-lua checkout named by
//! `JED_TEST_LUA_GRAMMAR` (else those tests print a note and pass): `--new-language lua` + the
//! checkout's `src/` and `highlights.scm` + the edited manifest passes `--check-language`
//! (`..._lua_check_passes`); a `.lua` file opens as "Lua", is coloured, comments with `--`, closes
//! quotes, takes a `.workspace` association and `[lsp.lua]` (`..._lua_files_...`,
//! `..._lua_lsp`); first start builds, the next one doesn't, a changed `scanner.c` rebuilds
//! (`..._lua_build_cycle`); a compile error, a missing symbol, ABI 12 and a broken query each fail
//! `--check-language` and are reported by the app with the file left uncoloured
//! (`..._lua_broken_*`). Built-in-grammar versions of the broken cases are in
//! `syntax/authoring.rs`'s tests (`--check-language`) and above (the app).

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
    app.enable_file_watching();
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
        let log = app.logs.texts(&crate::logs::Tool::Grammar(id.into()));
        assert!(log.last().is_some_and(|l| l.contains(want)), "{id}: {log:?}");
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

// syntax_plugins-g

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::syntax::build::Toolchain;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

/// Replaces the manifest line starting with `prefix`.
fn set_line(text: &str, prefix: &str, new: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(if line.starts_with(prefix) { new } else { line });
        out.push('\n');
    }
    out
}

fn edit_manifest(dir: &Path, f: impl FnOnce(&str) -> String) {
    let file = dir.join("language.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, f(&text)).unwrap();
}

/// A tree-sitter-lua checkout from `JED_TEST_LUA_GRAMMAR`; `None` with a note without the
/// variable or a C compiler.
fn lua_checkout() -> Option<&'static Path> {
    static CHECKOUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    CHECKOUT
        .get_or_init(|| {
            let Some(dir) = std::env::var_os("JED_TEST_LUA_GRAMMAR") else {
                eprintln!("note: JED_TEST_LUA_GRAMMAR is not set; the Lua tests are skipped");
                return None;
            };
            plugin::test_lib::library(false)?;
            let dir = PathBuf::from(dir);
            assert!(
                dir.join("src/parser.c").is_file(),
                "{} is no tree-sitter-lua",
                dir.display()
            );
            Some(dir)
        })
        .as_deref()
}

/// What the guide says: `new_language`, the checkout's `src/` and `highlights.scm` copied in, the
/// manifest edited (`extensions`, `line_comment`, `brackets`, `pairs`). `ext` is the extension.
fn lua_plugin(parent: &Path, id: &str, ext: &str, checkout: &Path) -> PathBuf {
    crate::syntax::authoring::new_language(parent, id).unwrap();
    let dir = parent.join(id);
    std::fs::copy(checkout.join("src/parser.c"), dir.join("src/parser.c")).unwrap();
    std::fs::copy(checkout.join("src/scanner.c"), dir.join("src/scanner.c")).unwrap();
    copy_dir(&checkout.join("src/tree_sitter"), &dir.join("src/tree_sitter"));
    std::fs::copy(
        checkout.join("queries/highlights.scm"),
        dir.join("queries/highlights.scm"),
    )
    .unwrap();
    edit_manifest(&dir, |t| {
        let t = set_line(t, "# line_comment = ", "line_comment = \"--\"");
        let t = set_line(&t, "extensions = ", &format!("extensions = [\"{ext}\"]"));
        let t = set_line(&t, "symbol = ", "symbol = \"tree_sitter_lua\"");
        let t = set_line(&t, "brackets = ", "brackets = [\"()\", \"[]\", \"{}\"]");
        t.replace(
            "    { open = \"\\\"\", close = \"\\\"\", not_in = [\"string\"] },",
            "    { open = \"\\\"\", close = \"\\\"\", not_in = [\"string\"] },\n    { open = \"'\", close = \"'\", not_in = [\"string\", \"comment\"] },",
        )
    });
    dir
}

const LUA_SAMPLE: &str = "local function f(x)\n  return x .. \"s\" -- note\nend\n";

struct LuaFixture {
    pid: PluginId,
    report: String,
    ok: bool,
}

/// The done-when flow once per process (`check_language` registers the plugin): id `lua`, the
/// extension `lua`.
fn lua() -> Option<&'static LuaFixture> {
    static LUA: OnceLock<Option<LuaFixture>> = OnceLock::new();
    LUA.get_or_init(|| {
        let checkout = lua_checkout()?;
        let languages = tempfile::tempdir().unwrap().keep();
        lua_plugin(&languages, "lua", "lua", checkout);
        let sample = languages.join("sample.lua");
        std::fs::write(&sample, LUA_SAMPLE).unwrap();
        let (report, ok) =
            crate::syntax::authoring::check_language(&languages, "lua", Some(&sample), &Toolchain::from_env());
        let pid = PluginId::find("lua").expect("check_language registers the plugin");
        Some(LuaFixture { pid, report, ok })
    })
    .as_ref()
}

#[test]
fn syntax_plugins_lua_check_passes() {
    let Some(l) = lua() else { return };
    assert!(l.ok, "{}", l.report);
    assert!(!l.report.contains('✗'), "{}", l.report);
    assert!(l.report.trim_end().ends_with("OK"), "{}", l.report);
    let config = &l.pid.get().config;
    assert_eq!(config.line_comment, Some("--"));
    assert_eq!(l.pid.get().name, "Lua");
    // The sample is coloured: some escape sequence appears around its text.
    assert!(l.report.contains("\x1b["), "{}", l.report);
}

/// A workspace with `a.lua`, `x.rockspec` (a `.workspace` association) and the Lua plugin loaded.
fn lua_app(dir: &tempfile::TempDir, pid: PluginId) -> App {
    let root = root_of(dir);
    std::fs::write(root.join("a.lua"), "local function f() end\nx = \n").unwrap();
    std::fs::write(root.join("x.rockspec"), "package = \"x\"\n").unwrap();
    std::fs::write(
        root.join(".workspace"),
        "[files.associations]\n\"*.rockspec\" = \"lua\"\n",
    )
    .unwrap();
    let mut app = app_on(&root, Some("a.lua"));
    app.syntax_plugins = vec![pid];
    app.test_start();
    app
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_files_open_coloured_and_edit_by_the_manifest() {
    let Some(l) = lua() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut app = lua_app(&dir, l.pid);
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await,
        "no parse"
    );
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(status_bar(&term).contains("Lua"), "{}", status_bar(&term));
    let keys = {
        let (buf, _) = app.active_mut().unwrap();
        let rope = buf.rope.clone();
        let spans = buf.syntax.spans(&rope, 0..1).to_vec();
        spans
            .iter()
            .map(|s| crate::syntax::KEYS[s.highlight.index()])
            .collect::<Vec<_>>()
    };
    assert!(keys.iter().any(|k| k.starts_with("keyword")), "{keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("function")), "{keys:?}");

    // Comment Line.
    app.press(K::Char('/'), cmd());
    assert_eq!(
        app.active_buffer().unwrap().rope.to_string(),
        "-- local function f() end\nx = \n"
    );
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    // Auto-close per `pairs`.
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    app.type_text("\"");
    assert_eq!(
        app.active_buffer().unwrap().rope.to_string(),
        "-- local function f() end\nx = \"\"\n"
    );

    // The association.
    let x = root_of(&dir).join("x.rockspec");
    app.test_open(Buffer::open(&x).unwrap());
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::Plugin(l.pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_lsp() {
    let Some(l) = lua() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    std::fs::write(root.join("a.lua"), "x = 1\n").unwrap();
    let user = crate::config::settings::parse("[lsp.lua]\ncommand = \"lua-language-server\"\n");
    assert!(user.errors.is_empty(), "{:?}", user.errors);
    let (servers, errors) = crate::lsp::config::user_servers(&user.lsp);
    assert!(errors.is_empty(), "{errors:?}");
    let initial = InitialFile {
        buffer: Buffer::open(&root.join("a.lua")).unwrap(),
        goto: None,
    };
    let store = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), store, Some(initial), true);
    app.syntax_plugins = vec![l.pid];
    app.lsp.builtins.user = servers;
    let fake = FakeLauncher::new(FakeOptions::default());
    app.lsp.set_launcher(fake.clone());
    app.test_start();
    app.press(K::Char('r'), M::NONE);
    let u = crate::milestone9_tests::uri(&root.join("a.lua"));
    assert!(
        app.settle(|_| fake.document(&u).is_some()).await,
        "{:?}",
        fake.launches()
    );
    assert_eq!(fake.launches(), ["lua-language-server"]);
    assert_eq!(fake.document(&u).unwrap().language, "lua");
    app.shutdown_servers().await;
}

/// First start builds (the command line is in the log), the next start finds it current, a changed
/// `scanner.c` rebuilds on the start after.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_build_cycle() {
    use crate::tools_view::GrammarState as G;
    let Some(checkout) = lua_checkout() else { return };
    let languages = tempfile::tempdir().unwrap();
    lua_plugin(languages.path(), "lua_cycle", "luacycle", checkout);
    let pid = {
        let (manifests, errors) = plugin::load_dir(languages.path());
        assert!(errors.is_empty(), "{errors:?}");
        plugin::register(manifests.into_iter().find(|m| m.id == "lua_cycle").unwrap()).unwrap()
    };
    let tool = crate::logs::Tool::Grammar("lua_cycle".into());
    let start = |dir: &tempfile::TempDir| {
        let root = root_of(dir);
        std::fs::write(root.join("a.luacycle"), "local function f() end\n").unwrap();
        let mut app = app_on(&root, Some("a.luacycle"));
        app.syntax_plugins = vec![pid];
        app.test_start();
        app
    };
    let commands = |app: &App| app.logs.texts(&tool).iter().filter(|l| l.starts_with("$ ")).count();

    let d1 = tempfile::tempdir().unwrap();
    let mut app = start(&d1);
    assert!(!coloured_lines(&mut app, 1), "not before the build");
    assert!(app.settle(|a| grammar_state(a, "lua_cycle") == G::Ready).await);
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    assert!(coloured_lines(&mut app, 1));
    assert_eq!(commands(&app), 1, "{:?}", app.logs.texts(&tool));
    assert!(app.logs.texts(&tool)[0].contains("scanner.c"));

    let d2 = tempfile::tempdir().unwrap();
    let mut app = start(&d2);
    assert!(app.settle(|a| grammar_state(a, "lua_cycle") == G::Ready).await);
    assert_eq!(app.logs.texts(&tool), ["up to date", "grammar loaded"]);
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    assert!(coloured_lines(&mut app, 1));

    let scanner = languages.path().join("lua_cycle/src/scanner.c");
    let mut text = std::fs::read_to_string(&scanner).unwrap();
    text.push_str("\n// changed\n");
    std::fs::write(&scanner, text).unwrap();
    let d3 = tempfile::tempdir().unwrap();
    let mut app = start(&d3);
    assert!(app.settle(|a| grammar_state(a, "lua_cycle") == G::Ready).await);
    let log = app.logs.texts(&tool);
    assert_eq!(commands(&app), 1, "{log:?}");
    assert!(log[0].starts_with("$ ") && log[0].contains("scanner.c"), "{log:?}");
    assert_eq!(log.last().map(String::as_str), Some("grammar loaded"), "{log:?}");
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    assert!(coloured_lines(&mut app, 1));
}

fn coloured_lines(app: &mut App, lines: usize) -> bool {
    let (buf, _) = app.active_mut().unwrap();
    let rope = buf.rope.clone();
    buf.syntax.tree().is_some() && !buf.syntax.spans(&rope, 0..lines).is_empty()
}

/// One broken Lua plugin `id`: `--check-language` fails with `check_want`; an app reports
/// `Syntax plugin <id>: …` containing `app_want` (status bar and the grammar's log), leaves the
/// file uncoloured and keeps taking input.
async fn lua_broken(id: &str, break_it: impl FnOnce(&Path), check_want: &[&str], app_want: &[&str]) {
    let Some(checkout) = lua_checkout() else { return };
    let languages = tempfile::tempdir().unwrap();
    let dir = lua_plugin(languages.path(), id, id, checkout);
    break_it(&dir);
    let (out, ok) = crate::syntax::authoring::check_language(languages.path(), id, None, &Toolchain::from_env());
    assert!(!ok, "{out}");
    assert!(
        out.trim_end().ends_with("\nFAILED") && !out.lines().any(|l| l == "OK"),
        "the report ends FAILED: {out}"
    );
    for w in check_want {
        assert!(out.contains(w), "{w}: {out}");
    }
    let pid = PluginId::find(id).unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = root_of(&work);
    std::fs::write(root.join(format!("a.{id}")), "local x = 1\n").unwrap();
    let mut app = app_on(&root, Some(&format!("a.{id}")));
    app.syntax_plugins = vec![pid];
    app.test_start();
    assert!(app.settle(|a| a.message.is_some()).await, "{id}: no message");
    let m = app.message.as_ref().unwrap();
    assert_eq!(m.level, crate::app::Level::Error);
    assert!(m.text.starts_with(&format!("Syntax plugin {id}: ")), "{}", m.text);
    let log = app.logs.texts(&crate::logs::Tool::Grammar(id.into())).join("\n");
    for w in app_want {
        assert!(m.text.contains(w), "{w}: {}", m.text);
        assert!(log.contains(w), "{w}: {log}");
    }
    assert!(pid.get().id == id && Language::Plugin(pid).grammar().is_none());
    app.type_text("hi");
    assert!(app.settle(|_| true).await);
    assert!(!coloured_lines(&mut app, 1));
    assert!(app.active_buffer().unwrap().rope.to_string().starts_with("hilocal"));
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_broken_compile_error() {
    lua_broken(
        "lua_cc",
        |d| {
            let p = d.join("src/scanner.c");
            let t = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, format!("{t}\nint broken( {{ nonsense;\n")).unwrap();
        },
        &["build failed", "scanner.c"],
        &["error"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_broken_missing_symbol() {
    lua_broken(
        "lua_sym",
        |d| edit_manifest(d, |t| set_line(t, "symbol = ", "symbol = \"tree_sitter_nothing\"")),
        &["no symbol `tree_sitter_nothing`"],
        &["tree_sitter_nothing"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_broken_abi_12() {
    lua_broken(
        "lua_abi",
        |d| {
            let p = d.join("src/parser.c");
            let t = std::fs::read_to_string(&p).unwrap();
            assert!(t.contains("#define LANGUAGE_VERSION 15"));
            std::fs::write(
                &p,
                t.replace("#define LANGUAGE_VERSION 15", "#define LANGUAGE_VERSION 12"),
            )
            .unwrap();
        },
        &["ABI version 12 is not supported", "regenerate"],
        &["ABI version 12"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_lua_broken_query() {
    lua_broken(
        "lua_query",
        |d| {
            let p = d.join("queries/highlights.scm");
            let t = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, format!("{t}\n(no_such_node) @keyword\n")).unwrap();
        },
        &["highlights.scm: highlight query, line"],
        &["highlights.scm", "line", "column"],
    )
    .await;
}

/// A query that doesn't compile, through the CMake grammar (no Lua checkout needed): the message
/// has the file, line and column, the log too.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_broken_query_is_reported_in_the_app() {
    let Some(lib) = plugin::test_lib::library(false) else {
        return;
    };
    let plugins = tempfile::tempdir().unwrap();
    plugin::test_lib::plugin_dir(
        plugins.path(),
        "ptg_query",
        "ptgquery",
        "tree_sitter_cmake",
        lib,
        "(no_such_node) @keyword\n",
    );
    let pid = plugin::test_lib::register_dir(plugins.path(), "ptg_query");
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join("a.ptgquery"), "project(x)\n").unwrap();
    let mut app = app_on(&root, Some("a.ptgquery"));
    app.syntax_plugins = vec![pid];
    app.test_start();
    assert!(app.settle(|a| a.message.is_some()).await);
    let m = app.message.as_ref().unwrap();
    assert!(
        m.text.starts_with("Syntax plugin ptg_query: ")
            && m.text.contains("highlights.scm")
            && m.text.contains("line 1 column"),
        "{}",
        m.text
    );
    let log = app.logs.texts(&crate::logs::Tool::Grammar("ptg_query".into()));
    assert!(log.last().unwrap().contains("highlights.scm"), "{log:?}");
    assert!(Language::Plugin(pid).grammar().is_none());
    app.type_text("hi");
    assert!(app.settle(|_| true).await);
    assert!(!coloured_lines(&mut app, 1));
}

/// "Copying `examples/languages/cmake/` in under the id `cmake2` colours a `CMakeLists.txt`
/// exactly as the built-in CMake does." The copy's file claims are dropped (the registry is
/// process-global, and a plugin claiming `.cmake` would take every other test's files) and the
/// file is reached through a `.workspace` association. With its claims the template would not
/// even be chosen for `CMakeLists.txt`: built-in exact names beat plugin file names.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_template_copied_as_cmake2_colours_like_built_in() {
    // The claims, and the rule that leaves the file the built-in's.
    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/languages/cmake");
    let (m, errors) = plugin::parse(&std::fs::read_to_string(template.join("language.toml")).unwrap(), "t");
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(m.unwrap().filenames, ["CMakeLists.txt"]);
    assert_eq!(
        detect(
            Path::new("/ws/CMakeLists.txt"),
            Path::new("/ws"),
            "",
            &Associations::default()
        ),
        Language::CMake
    );

    if plugin::test_lib::library(false).is_none() {
        return;
    }
    let plugins = tempfile::tempdir().unwrap();
    let copy = plugins.path().join("cmake2");
    copy_dir(&template, &copy);
    edit_manifest(&copy, |t| {
        let t = set_line(t, "id = ", "id = \"cmake2\"");
        let t = set_line(&t, "name = ", "name = \"CMake Two\"");
        let t = set_line(&t, "extensions = ", "extensions = []");
        set_line(&t, "filenames = ", "filenames = []")
    });
    let pid = plugin::test_lib::register_dir(plugins.path(), "cmake2");
    assert_eq!(pid.get().name, "CMake Two");

    let text = "cmake_minimum_required(VERSION 3.20)\nproject(demo)\nif(WIN32)\n  set(NAME \"x\") # note\nendif()\n";
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    std::fs::write(root.join("CMakeLists.txt"), text).unwrap();
    std::fs::write(
        root.join(".workspace"),
        "[files.associations]\n\"CMakeLists.txt\" = \"cmake2\"\n",
    )
    .unwrap();
    let mut app = app_on(&root, Some("CMakeLists.txt"));
    app.syntax_plugins = vec![pid];
    app.test_start();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await,
        "no parse: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(status_bar(&term).contains("CMake Two"), "{}", status_bar(&term));
    let (buf, _) = app.active_mut().unwrap();
    assert_eq!(buf.syntax.language(), Language::Plugin(pid));
    let rope = buf.rope.clone();
    let spans = buf.syntax.spans(&rope, 0..5).to_vec();
    let built_in = crate::syntax::highlight_text(Language::CMake, text);
    assert!(built_in.len() > 5);
    assert_eq!(spans, built_in);
}

/// A grammar build runs off the UI thread, so frames drawn while it builds stay in budget. The
/// build is forced while the frame loop is already up, so it is reliably observed (a build run on
/// the UI thread would blow the budget).
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_frames_stay_in_budget_while_a_grammar_builds() {
    use crate::tools_view::GrammarState as G;
    use std::time::{Duration, Instant};
    let plugins = tempfile::tempdir().unwrap();
    let Some(pid) = source_plugin(plugins.path(), "ptg_stall", "ptgstall") else {
        return;
    };
    // The CMake grammar is tiny: a warm compiler builds it in a few ms, before three frames (2 ms
    // apart) are drawn, so the "still building" count below would be a race. Pad `parser.c` with a
    // large external array so the build reliably spans many frames.
    let parser = plugins.path().join("ptg_stall/src/parser.c");
    let mut text = std::fs::read_to_string(&parser).unwrap();
    text.push_str("\nconst unsigned char jed_frame_padding[] = {");
    let digits = "0,1,2,3,4,5,6,7,8,9,";
    for _ in 0..(2 * 1024 * 1024 / digits.len()) {
        text.push_str(digits);
    }
    text.push_str("0};\n");
    std::fs::write(&parser, text).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_for_source_plugin(&dir, pid, "ptgstall");
    assert!(
        app.settle(|a| grammar_state(a, "ptg_stall") == G::Ready).await,
        "the first build landed"
    );
    let budget = if cfg!(debug_assertions) {
        Duration::from_millis(200)
    } else {
        Duration::from_millis(16)
    };
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    // Force a rebuild and, in the same synchronous block, draw frames until it lands.
    rebuild_through_palette(&mut app);
    let (mut building, mut frames, mut worst) = (0, 0, Duration::ZERO);
    while grammar_state(&app, "ptg_stall") != G::Ready {
        let t = Instant::now();
        app.pump();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        worst = worst.max(t.elapsed());
        frames += 1;
        building += 1;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(building >= 3, "only {building} frames ran during a rebuild");
    assert!(worst < budget, "worst frame {worst:?} of {frames}");
}

/// A `.workspace` can neither add, build nor point at a plugin: it is rejected as a whole
/// (`[languages]` is unknown), a plugin directory inside the workspace is never read, and the
/// only loader call in `main` takes the user's config directory.
#[tokio::test(flavor = "multi_thread")]
async fn syntax_plugins_workspace_cannot_add_a_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let root = root_of(&dir);
    for base in ["languages", ".jed/languages"] {
        let p = root.join(base).join("ptg_ws");
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::write(
            p.join("language.toml"),
            "id = \"ptg_ws\"\n[files]\nextensions = [\"ptgws\"]\n",
        )
        .unwrap();
        std::fs::write(p.join("src/parser.c"), "this is not C;\n").unwrap();
    }
    std::fs::write(root.join("a.ptgws"), "x\n").unwrap();
    std::fs::write(
        root.join(".workspace"),
        "[languages]\nptg_ws = \"languages/ptg_ws\"\n[files.associations]\n\"*.ptgws\" = \"ptg_ws\"\n",
    )
    .unwrap();
    let mut app = app_on(&root, Some("a.ptgws"));
    app.test_start();
    assert!(app.workspace.config_error.is_some());
    assert!(PluginId::find("ptg_ws").is_none());
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::PlainText);
    let started = std::time::Instant::now();
    assert!(
        app.settle(|_| started.elapsed() > std::time::Duration::from_millis(200))
            .await
    );
    assert!(app.grammars.is_empty());
    assert!(PluginId::find("ptg_ws").is_none());
    fn any_build_toml(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name() == "build.toml" || (e.file_type().unwrap().is_dir() && any_build_toml(&e.path())))
    }
    assert!(!any_build_toml(&root));
    // The one place plugins are loaded from (whitespace dropped, so formatting can't matter).
    let main: String = include_str!("main.rs").split_whitespace().collect();
    assert_eq!(main.matches("load_and_register(").count(), 1);
    assert!(main.contains("plugin::load_and_register(&paths::config_dir().join(\"languages\"))"));
}
