//! The Keyboard Shortcuts view: "Preferences: Keyboard Shortcuts" opens a full-screen list of
//! every command with its key, context and source, filtered as the query is typed; keys-a opens
//! and closes it end to end (through `Command::KeyboardShortcuts`, also from the palette); Esc or
//! a mouse press outside closes it; it swallows keys and paste; its command is named, titled and
//! unbound. The keymap's source tracking has its unit tests in `config/keymap.rs`; keys-b's
//! search, selection and scrolling are unit-tested in `keys.rs`.

use crossterm::event::{Event, KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::keymap::{Keymap, Platform};
use crate::layout::ScrollTarget;

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
}
