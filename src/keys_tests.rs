//! The Keyboard Shortcuts view: "Preferences: Keyboard Shortcuts" opens a full-screen list of
//! every command with its key, context and source, filtered as the query is typed; keys-a opens
//! and closes it end to end (through `Command::KeyboardShortcuts`, also from the palette); Esc or
//! a mouse press outside closes it; it swallows keys and paste; its command is named, titled and
//! unbound. keys-b's search, selection and scrolling are unit-tested in `keys.rs`; keys-c's
//! record-keys mode (Alt+K, Esc cancels) and Enter's `keybindings.toml` stub are here, with the
//! stub's `create_if_missing` / `append_stub` in `config/keymap.rs`.

use crossterm::event::{Event, KeyCode as K, KeyEvent, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::keymap::{KeyCombo, Keymap, Platform};
use crate::layout::ScrollTarget;

mod keys {
    use super::*;

    /// Cmd on macOS, Ctrl on Linux.
    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn open_app(dir: &tempfile::TempDir) -> App {
        let root = crate::workspace::normalize(dir.path());
        let path = root.join("a.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&path).unwrap());
        // Where "Enter" writes keybindings.toml: never the real user's config dir.
        app.config_dir = root.join("config");
        app.test_start();
        app
    }

    /// Draws one frame (as `draw_sized` in `milestone10_tests`, which is private to it).
    fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// Whether any drawn row holds `needle`.
    fn shown(term: &Terminal<TestBackend>, needle: &str) -> bool {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .any(|line| line.contains(needle))
    }

    fn unbound_count(app: &App) -> usize {
        let bound: Vec<_> = app.keymap.bindings_with_source().map(|(_, _, cmd, _)| cmd).collect();
        Command::all().into_iter().filter(|c| !bound.contains(c)).count()
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
        app.press(K::Tab, M::NONE);
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

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_b_lists_every_command_with_key_context_and_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let view = app.keys.as_ref().unwrap();
        let bindings = app.keymap.bindings_with_source().count();
        assert_eq!(view.rows.len(), bindings + unbound_count(&app));
        assert_eq!(view.all.len(), view.rows.len(), "an empty query keeps every row");
        assert_eq!((view.selected, view.top), (0, 0));
        let mut sorted = view.rows.clone();
        sorted.sort_by_cached_key(|r| (r.command.title(), r.key.clone()));
        assert_eq!(view.rows, sorted);

        // A bound row, and a command with no key at all (an empty key and source).
        let save = view.rows.iter().find(|r| r.command == Command::Save).unwrap();
        let save_key = app.keymap.binding_for(Command::Save).unwrap().label();
        assert_eq!(save.key, save_key);
        assert_eq!((save.context.as_str(), save.source.as_str()), ("", "Default"));
        let shortcuts = view
            .rows
            .iter()
            .find(|r| r.command == Command::KeyboardShortcuts)
            .unwrap();
        assert_eq!((shortcuts.key.as_str(), shortcuts.source.as_str()), ("", ""));

        // A context row drawn with its key, context and source.
        app.type_text("select previous row");
        let row = app.keys.as_ref().unwrap().rows[0].clone();
        assert_eq!(row.command, Command::Locations(crate::command::LocationsCommand::Up));
        assert_eq!(row.context, "Locations");
        assert_eq!(row.source, "Default");
        let term = draw(&mut app, 100, 24);
        assert!(shown(&term, "Search keybindings:"));
        // A title longer than its column is cut at the edge.
        let title: String = row.command.title().chars().take(12).collect();
        assert!(shown(&term, &title));
        assert!(shown(&term, &row.key));
        assert!(shown(&term, "Locations"));
        assert!(shown(&term, "Default"));
        // Narrow: the context and source columns are dropped before the title.
        let narrow = draw(&mut app, 60, 20);
        assert!(shown(&narrow, "Search keybindings:"));
        assert!(shown(&narrow, &title));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_b_fuzzy_filters_by_key_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let all = app.keys.as_ref().unwrap().rows.len();
        // As key text is labelled on this platform.
        let query = if cfg!(target_os = "macos") {
            "cmd+shift"
        } else {
            "ctrl+shift"
        };
        app.type_text(query);
        let view = app.keys.as_ref().unwrap();
        assert_eq!(view.query, query);
        assert!(!view.rows.is_empty());
        assert!(view.rows.len() < all, "the search narrows the list");
        assert!(view.rows.iter().any(|r| r.command == Command::SaveAs));
        assert!(
            !view.rows.iter().any(|r| r.command == Command::Quit),
            "Quit's key, title and name have no {query} in them"
        );
        let save_as = view
            .rows
            .iter()
            .find(|r| r.command == Command::SaveAs)
            .unwrap()
            .key
            .clone();
        let term = draw(&mut app, 100, 24);
        assert!(shown(&term, query));
        assert!(shown(&term, &save_as));

        // Backspace empties the search, and every row comes back.
        for _ in 0..query.len() {
            app.press(K::Backspace, M::NONE);
        }
        let view = app.keys.as_ref().unwrap();
        assert_eq!(view.query, "");
        assert_eq!(view.rows.len(), all);

        // A query nothing matches empties the list and shows the empty message.
        app.type_text("zzznotacommand");
        assert!(app.keys.as_ref().unwrap().rows.is_empty());
        let empty = draw(&mut app, 100, 24);
        assert!(shown(&empty, "No matching keybindings"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_b_selection_moves_and_scrolls() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let term = draw(&mut app, 100, 24);
        let hit = app.hits.keys.expect("the view is drawn");
        assert!(hit.scrollbar.is_some_and(|s| s.thumb.is_some()), "more rows than fit");
        assert!(app.hits.scrollbar(ScrollTarget::Keys).is_some());
        drop(term);
        let (len, height) = {
            let view = app.keys.as_ref().unwrap();
            (view.rows.len(), view.height)
        };
        assert!(len > height);

        app.press(K::Down, M::NONE);
        assert_eq!(app.keys.as_ref().unwrap().selected, 1);
        app.press(K::Up, M::NONE);
        assert_eq!(app.keys.as_ref().unwrap().selected, 0);
        app.press(K::End, M::NONE);
        let view = app.keys.as_ref().unwrap();
        assert_eq!((view.selected, view.top), (len - 1, len - height));
        app.press(K::Home, M::NONE);
        assert_eq!(
            (app.keys.as_ref().unwrap().selected, app.keys.as_ref().unwrap().top),
            (0, 0)
        );
        app.press(K::PageDown, M::NONE);
        assert_eq!(app.keys.as_ref().unwrap().selected, height);
        assert!(app.keys.as_ref().unwrap().top > 0, "a page down scrolls the list");
        app.press(K::PageUp, M::NONE);
        assert_eq!(app.keys.as_ref().unwrap().selected, 0);

        // Esc closes it.
        app.press(K::Esc, M::NONE);
        assert!(app.keys.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_b_the_wheel_scrolls_and_a_press_outside_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let _term = draw(&mut app, 100, 24);
        let hit = app.hits.keys.expect("the view is drawn");
        let (x, y) = (hit.rect.x + 10, hit.rect.y + 10);
        app.mouse(Mouse::ScrollDown, x, y);
        assert!(app.keys.as_ref().unwrap().top > 0, "the wheel scrolls it");
        let top = app.keys.as_ref().unwrap().top;
        app.mouse(Mouse::ScrollUp, x, y);
        assert!(app.keys.as_ref().unwrap().top < top);
        app.mouse(Mouse::Down(MouseButton::Left), x, y);
        assert!(app.keys.is_some(), "a press on the list does nothing");
        app.mouse(Mouse::Down(MouseButton::Left), 1, 23);
        assert!(app.keys.is_none(), "a press outside it closes it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_b_the_scroll_bar_clicks_and_drags() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let _term = draw(&mut app, 100, 24);
        let bar = app.hits.keys.as_ref().unwrap().scrollbar.expect("a scroll bar");
        assert!(bar.thumb.is_some());
        // A click near the bottom of the track jumps the view down (and grabs the thumb).
        let (tx, ty) = (bar.track.x, bar.track.y);
        let bottom = bar.track.y + bar.track.height - 1;
        app.mouse(Mouse::Down(MouseButton::Left), tx, bottom);
        assert!(app.keys.as_ref().unwrap().top > 0, "the track click scrolls it");
        // Dragging the thumb back to the top scrolls it back.
        app.mouse(Mouse::Drag(MouseButton::Left), tx, ty);
        app.mouse(Mouse::Up(MouseButton::Left), tx, ty);
        assert_eq!(app.keys.as_ref().unwrap().top, 0, "the drag to the top scrolls it back");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_done_when_opens_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        // A user override, loaded the way main does.
        std::fs::create_dir_all(&app.config_dir).unwrap();
        let user = app.config_dir.join(crate::config::keymap::FILE_NAME);
        std::fs::write(&user, "[[binding]]\nkey = \"ctrl+alt+s\"\ncommand = \"file.save\"\n").unwrap();
        let (keymap, errors) = Keymap::load(&user, Platform::current());
        assert!(errors.is_empty(), "{errors:?}");
        app.set_keymap(keymap, errors);

        // The palette command opens the view (Cmd+Shift+P, then its title).
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text("Keyboard Shortcuts");
        app.press(K::Enter, M::NONE);
        assert!(app.palette.is_none(), "the palette closes for it");
        assert!(app.keys.is_some(), "the palette command opens the view");

        // Every row has its command's title; a binding adds its key, context and source.
        let view = app.keys.as_ref().unwrap();
        assert_eq!(view.rows.len(), view.all.len(), "an empty query keeps every row");
        for row in &view.rows {
            assert!(!row.command.title().is_empty());
            if row.key.is_empty() {
                assert_eq!(
                    (row.spec.as_str(), row.context.as_str(), row.source.as_str()),
                    ("", "", "")
                );
            } else {
                assert!(!row.spec.is_empty(), "{:?}", row.command);
                assert!(!row.source.is_empty(), "{:?}", row.command);
            }
        }
        // The override replaced Save's default: its key, config spelling and source are the user's.
        let save = view.rows.iter().find(|r| r.command == Command::Save).unwrap();
        assert_eq!((save.key.as_str(), save.spec.as_str()), ("Ctrl+Alt+S", "ctrl+alt+s"));
        assert_eq!(save.source, "User");
        // A context binding and a default source.
        let next = view
            .rows
            .iter()
            .find(|r| r.command == Command::Find(crate::command::FindCommand::Next))
            .unwrap();
        assert_eq!(next.context, "Find");
        assert_eq!(next.source, "Default");

        let term = draw(&mut app, 120, 24);
        assert!(shown(&term, "Search keybindings:"));
        assert!(shown(&term, "Default"));
        // Select the user's Save row and a context row to see their columns drawn.
        let index = app
            .keys
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.command == Command::Save)
            .unwrap();
        app.keys.as_mut().unwrap().select(index);
        let term = draw(&mut app, 120, 24);
        assert!(shown(&term, "Ctrl+Alt+S"));
        assert!(shown(&term, "User"));
        let index = app
            .keys
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.command == Command::Find(crate::command::FindCommand::Next))
            .unwrap();
        app.keys.as_mut().unwrap().select(index);
        let term = draw(&mut app, 120, 24);
        assert!(shown(&term, "Find"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_done_when_fuzzy_and_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);
        let all = app.keys.as_ref().unwrap().all.len();
        // Typing key text filters (as it is labelled on this platform).
        let typed = if cfg!(target_os = "macos") {
            "cmd+shift"
        } else {
            "ctrl+shift"
        };
        app.type_text(typed);
        let view = app.keys.as_ref().unwrap();
        assert_eq!(view.query, typed);
        assert!(view.rows.len() < all, "the search narrows the list");
        assert!(view.rows.iter().any(|r| r.command == Command::SaveAs));

        // Alt+K records: until the combo comes, the search box says so.
        app.press(K::Char('k'), M::ALT);
        assert!(app.keys.as_ref().unwrap().record);
        let term = draw(&mut app, 100, 24);
        assert!(shown(&term, "Press key combination"));
        // The selection keys still work and are not recorded.
        let selected = app.keys.as_ref().unwrap().selected;
        app.press(K::Down, M::NONE);
        let view = app.keys.as_ref().unwrap();
        assert!(view.record, "Down is not the recorded combo");
        assert_eq!(view.query, typed);
        assert_eq!(view.selected, selected + 1);

        // The next combo becomes the query (its label) and filters to its bindings.
        let (code, mods) = if cfg!(target_os = "macos") {
            (K::Char('s'), M::SUPER | M::SHIFT)
        } else {
            (K::Char('s'), M::CONTROL | M::SHIFT)
        };
        let label = KeyCombo::from_event(&KeyEvent::new(code, mods)).label();
        app.press(code, mods);
        let view = app.keys.as_ref().unwrap();
        assert!(!view.record, "one combo ends recording");
        assert_eq!(view.query, label);
        assert_eq!(view.selected, 0, "a new search selects the top row");
        assert!(view.rows.len() < all, "the recorded key narrows the list");
        assert!(view.rows.iter().any(|r| r.command == Command::SaveAs));
        assert!(!app.keys.as_ref().unwrap().record);
        let term = draw(&mut app, 100, 24);
        assert!(shown(&term, &label));

        // Esc cancels recording (the view stays); a second Esc closes it.
        app.press(K::Char('k'), M::ALT);
        assert!(app.keys.as_ref().unwrap().record);
        app.press(K::Esc, M::NONE);
        assert!(app.keys.is_some(), "Esc cancels recording, not the view");
        assert!(!app.keys.as_ref().unwrap().record);
        app.press(K::Esc, M::NONE);
        assert!(app.keys.is_none(), "a second Esc closes it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_done_when_record_captures_a_plain_key_and_enter() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::KeyboardShortcuts);

        // A plain char with no modifier is the combo (its upper-case label), not a navigation key
        // or swallowed text.
        app.press(K::Char('k'), M::ALT);
        assert!(app.keys.as_ref().unwrap().record);
        app.press(K::Char('x'), M::NONE);
        let view = app.keys.as_ref().unwrap();
        assert!(!view.record, "the plain key ends recording");
        assert_eq!(view.query, "X", "a plain char's label is its upper case");

        // Enter while recording is captured as a combo, not a stub write.
        app.press(K::Char('k'), M::ALT);
        assert!(app.keys.as_ref().unwrap().record);
        app.press(K::Enter, M::NONE);
        let view = app.keys.as_ref().unwrap();
        assert!(!view.record, "Enter ends recording too");
        assert_eq!(view.query, "Enter");
        assert!(app.keys.is_some(), "Enter was recorded, not a stub write");
        assert!(
            !app.config_dir.join(crate::config::keymap::FILE_NAME).exists(),
            "no keybindings.toml written"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn keys_done_when_enter_writes_a_stub() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        let path = app.config_dir.join(crate::config::keymap::FILE_NAME);
        assert!(!path.exists(), "no keybindings.toml yet");

        // A bound command: the file is created from the template, then gets its binding.
        app.test_execute(Command::KeyboardShortcuts);
        app.type_text("save as");
        let index = app
            .keys
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.command == Command::SaveAs)
            .expect("the search keeps Save As");
        app.keys.as_mut().unwrap().select(index);
        let row = app.keys.as_ref().unwrap().selected_row().unwrap().clone();
        assert_eq!(row.command, Command::SaveAs);
        assert!(!row.spec.is_empty());
        app.press(K::Enter, M::NONE);
        assert!(app.keys.is_none(), "the view closes for the file");
        assert!(
            app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await,
            "keybindings.toml opens in a tab"
        );
        assert!(path.exists(), "the file was created");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(crate::config::keymap::KEYBINDINGS_TEMPLATE));
        assert!(
            text.contains(&format!(
                "[[binding]]\nkey = \"{}\"\ncommand = \"{}\"\n",
                row.spec,
                row.command.name()
            )),
            "{text}"
        );

        // An unbound command gets an empty key and a note to set one (close the tab first, so
        // the reopened file shows the append).
        app.test_execute(Command::CloseTab);
        app.test_execute(Command::KeyboardShortcuts);
        app.type_text("keyboard shortcuts");
        let index = app
            .keys
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.command == Command::KeyboardShortcuts)
            .unwrap();
        app.keys.as_mut().unwrap().select(index);
        let row = app.keys.as_ref().unwrap().selected_row().unwrap().clone();
        assert_eq!((row.key.as_str(), row.spec.as_str()), ("", ""), "unbound");
        app.press(K::Enter, M::NONE);
        assert!(
            app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await,
            "keybindings.toml opens again"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[[binding]]\nkey = \"\"  # set the key\ncommand = \"preferences.keyboardShortcuts\"\n"),
            "{text}"
        );
        assert_eq!(text.matches("\n[[binding]]").count(), 2, "the first stub is kept");
    }
}
