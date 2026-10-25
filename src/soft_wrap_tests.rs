//! soft_wrap-a: the `word_wrap` setting (`off` / `on`, default `off`) and "View: Toggle Word
//! Wrap" (`view.toggleWordWrap`, Alt+Z; Cmd+Alt+Z on macOS), which flips the effective mode for
//! the session without writing it to a settings file.
//!
//! Drawing, scrolling and movement on wrapped rows come in the task's later subtasks. The
//! "Done when" parts covered here: the setting is read from `settings.toml` and `.workspace`,
//! defaults to `off`, and the toggle flips `App::word_wrap()` and keeps doing so across a
//! `.workspace` reload.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::settings;
use crate::edit::WordWrap;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A workspace with `a.txt` open, a user `settings.toml` holding `user` and a `.workspace`
/// holding `workspace` (both as `[settings]` bodies, empty for none).
fn open(dir: &tempfile::TempDir, user: &str, workspace: &str) -> (App, PathBuf, PathBuf) {
    let base = crate::workspace::normalize(dir.path());
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    if !user.is_empty() {
        fs::write(config_dir.join(settings::FILE_NAME), format!("[settings]\n{user}")).unwrap();
    }
    let root = base.join("ws");
    fs::create_dir_all(&root).unwrap();
    if !workspace.is_empty() {
        fs::write(root.join(".workspace"), format!("[settings]\n{workspace}")).unwrap();
    }
    fs::write(root.join("a.txt"), "one two three\nfour\n").unwrap();
    let loaded = settings::load(&config_dir.join(settings::FILE_NAME));
    let ws = Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let trust = TrustStore::load(base.join("trust.toml"));
    let initial = InitialFile {
        buffer: Buffer::open(&root.join("a.txt")).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config_dir.clone();
    app.report_errors(loaded.errors);
    app.test_start();
    (app, root, config_dir)
}

/// VS Code's toggle: Alt+Z, Option+Command+Z on macOS.
fn toggle_mods() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER | KeyModifiers::ALT
    } else {
        KeyModifiers::ALT
    }
}

#[test]
fn soft_wrap_setting_defaults_off() {
    assert_eq!(settings::parse("").overrides.word_wrap, None);
    let none = settings::SettingsOverrides::default();
    assert_eq!(settings::layer(&none, &none).word_wrap, WordWrap::Off);
}

#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_toggle_flips_the_effective_mode_for_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root, config_dir) = open(&dir, "word_wrap = \"off\"\n", "");
    assert_eq!(app.settings.word_wrap, WordWrap::Off);
    assert_eq!(app.word_wrap(), WordWrap::Off);

    app.press(KeyCode::Char('z'), toggle_mods());
    assert_eq!(app.last_command, Some(Command::ToggleWordWrap));
    assert_eq!(app.settings.word_wrap, WordWrap::Off, "the setting is untouched");
    assert_eq!(app.word_wrap(), WordWrap::On);

    // A `.workspace` reload changes the setting but keeps the session toggle: an `on` setting
    // with the toggle flipped is off.
    fs::write(root.join(".workspace"), "[settings]\nword_wrap = \"on\"\n").unwrap();
    assert!(app.settle(|a| a.settings.word_wrap == WordWrap::On).await);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    app.press(KeyCode::Char('z'), toggle_mods());
    assert_eq!(app.word_wrap(), WordWrap::On);

    // Never written to either file.
    assert_eq!(
        fs::read_to_string(config_dir.join(settings::FILE_NAME)).unwrap(),
        "[settings]\nword_wrap = \"off\"\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".workspace")).unwrap(),
        "[settings]\nword_wrap = \"on\"\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_setting_on_is_toggled_off_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open(&dir, "word_wrap = \"on\"\n", "word_wrap = \"on\"\n");
    assert!(app.settle(|a| a.settings.word_wrap == WordWrap::On).await);
    assert_eq!(app.word_wrap(), WordWrap::On);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::On);
}
