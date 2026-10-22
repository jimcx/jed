//! Syntax plugins (syntax_plugins-b) end to end, without native code: files detected as a plugin
//! language open as it but uncoloured; its `line_comment` and `pairs` drive Comment Line and
//! auto-close; `.workspace` associations and `[lsp.<id>]` (and the user's `settings.toml`
//! `[lsp.<id>]`) name plugin ids; detection order (filenames, extensions beating built-ins,
//! shebangs after built-ins). Every test registers a plugin under its own id and claims only
//! extensions, names and shebangs of its own: the registry is process-global.

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
