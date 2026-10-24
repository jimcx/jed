//! The Keyboard Shortcuts view, keys-a: "Preferences: Keyboard Shortcuts" opens an (empty for now)
//! view and closes it again end to end — through `Command::KeyboardShortcuts`, also from the
//! palette; Esc, the command again or a mouse press closes it; it swallows keys and paste; its
//! command is named, titled and unbound. The keymap's source tracking has its unit tests in
//! `config/keymap.rs`.

use crossterm::event::{Event, KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::keymap::{Keymap, Platform};

mod keys {
    use super::*;

    fn open_app(dir: &tempfile::TempDir) -> App {
        let root = crate::workspace::normalize(dir.path());
        let path = root.join("a.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&path).unwrap());
        app.test_start();
        app
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_a_opens_and_closes_the_view() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        assert!(app.keys.is_none());
        app.test_execute(Command::KeyboardShortcuts);
        assert!(app.keys.is_some(), "the command opens it");
        app.press(K::Esc, M::NONE);
        assert!(app.keys.is_none(), "Esc closes it");
        // Its command toggles it.
        app.test_execute(Command::KeyboardShortcuts);
        assert!(app.keys.is_some());
        app.test_execute(Command::KeyboardShortcuts);
        assert!(app.keys.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_a_opens_from_the_palette() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::ShowCommands);
        app.test_execute(Command::KeyboardShortcuts);
        assert!(app.palette.is_none(), "the palette closes for it");
        assert!(app.keys.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_a_swallows_keys_and_paste() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        app.type_text("x");
        app.press(K::Enter, M::NONE);
        app.press(K::F(1), M::SHIFT);
        app.press(
            K::Char('p'),
            if cfg!(target_os = "macos") {
                M::SUPER
            } else {
                M::CONTROL
            },
        );
        app.handle_test_event(Event::Paste("pasted".into()));
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "abc\n");
        assert!(app.palette.is_none());
        assert!(app.cheat_sheet.is_none(), "its own keys don't reach the cheat sheet");
        assert!(app.keys.is_some(), "nothing but Esc closed it");
        app.press(K::Esc, M::NONE);
        assert!(app.keys.is_none());
        // Typing works again.
        app.type_text("x");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "xabc\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_a_mouse_press_closes_it_and_the_wheel_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        // Nothing is drawn yet, so every press is outside it.
        app.mouse(Mouse::ScrollDown, 40, 10);
        app.mouse(Mouse::Up(MouseButton::Left), 40, 10);
        assert!(app.keys.is_some(), "moving the mouse and the wheel leave it open");
        app.mouse(Mouse::Down(MouseButton::Left), 40, 10);
        assert!(app.keys.is_none());
    }

    #[test]
    fn keys_a_the_command_is_unbound_and_named() {
        assert_eq!(Command::KeyboardShortcuts.name(), "preferences.keyboardShortcuts");
        assert_eq!(Command::KeyboardShortcuts.title(), "Preferences: Keyboard Shortcuts");
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            assert_eq!(km.binding_for(Command::KeyboardShortcuts), None, "{platform:?}");
            assert!(
                km.bindings_with_source()
                    .all(|(_, _, cmd, _)| cmd != Command::KeyboardShortcuts),
                "{platform:?}: bound"
            );
        }
    }
}
