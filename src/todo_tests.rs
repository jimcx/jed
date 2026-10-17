//! End-to-end tests for single PLAN.md TODO items, one `mod` per item.
//!
//! - `render_whitespace`: spaces drawn as `·` and tabs as `→` (in the tab's first cell) in
//!   `editorWhitespace.foreground` (over syntax colours), text columns unchanged, under the
//!   selection background and never dimming a drawn cursor; each of VS Code's modes (`none` / `boundary` / `selection` /
//!   `trailing` / `all`) from `settings.toml` and `.workspace`, a bad value reported; "View:
//!   Toggle Render Whitespace" from the palette, session-only and kept across a `.workspace` reload.

mod render_whitespace {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::style::{Color, Modifier};

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::config::settings;
    use crate::edit::RenderWhitespace;

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// An app on `<dir>/ws` (`.workspace` text `workspace`), the user's `settings.toml` holding
    /// `[settings]` + `user`, with `a.txt` (holding `text`) open. Returns it, the root and the
    /// `settings.toml` path.
    fn open_app(dir: &tempfile::TempDir, user: &str, workspace: &str, text: &str) -> (App, PathBuf, PathBuf) {
        open_file(dir, user, workspace, "a.txt", text)
    }

    /// `open_app` with the open file named `name`.
    fn open_file(
        dir: &tempfile::TempDir,
        user: &str,
        workspace: &str,
        name: &str,
        text: &str,
    ) -> (App, PathBuf, PathBuf) {
        let base = crate::workspace::normalize(dir.path());
        let config_dir = base.join("config");
        let user_file = write(&config_dir, settings::FILE_NAME, &format!("[settings]\n{user}"));
        let root = base.join("ws");
        write(&root, ".workspace", workspace);
        let path = write(&root, name, text);
        let loaded = settings::load(&user_file);
        let ws = crate::workspace::Workspace::load(root.clone()).with_user_settings(loaded.overrides);
        let trust = crate::workspace::trust::TrustStore::load(base.join("trust.toml"));
        let initial = crate::app::InitialFile {
            buffer: Buffer::open(&path).unwrap(),
            goto: None,
        };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.config_dir = config_dir;
        app.report_errors(loaded.errors);
        app.test_start();
        (app, root, user_file)
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// A drawn cell of the text area: its symbol, foreground, background, modifiers.
    struct Cell {
        sym: String,
        fg: Color,
        bg: Color,
        modifier: Modifier,
    }

    /// The first `n` cells of buffer line `line` (with no scrolling).
    fn cells(app: &App, term: &Terminal<TestBackend>, line: u16, n: u16) -> Vec<Cell> {
        let text = app.hits.groups[0].text;
        (0..n)
            .map(|x| {
                let c = &term.backend().buffer()[(text.x + x, text.y + line)];
                Cell {
                    sym: c.symbol().to_string(),
                    fg: c.fg,
                    bg: c.bg,
                    modifier: c.modifier,
                }
            })
            .collect()
    }

    /// Buffer line `line` as drawn, `n` cells.
    fn row(app: &App, term: &Terminal<TestBackend>, line: u16, n: u16) -> String {
        cells(app, term, line, n).iter().map(|c| c.sym.as_str()).collect()
    }

    /// Draws and returns buffer line 0's first 12 cells.
    fn drawn_line0(app: &mut App) -> String {
        let term = draw(app);
        row(app, &term, 0, 12)
    }

    fn dim(app: &App) -> Color {
        app.theme.get("editorWhitespace.foreground").fg.unwrap()
    }

    fn run_command(app: &mut App, title: &str) {
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text(title);
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, title);
        app.press(K::Enter, M::NONE);
    }

    const TEXT: &str = "\tab  c d \n\nend\n";

    #[tokio::test(flavor = "multi_thread")]
    async fn all_by_default_draws_dots_and_arrows_without_moving_text() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "", "", TEXT);
        assert_eq!(app.settings.render_whitespace, RenderWhitespace::All, "the default");
        // The cursor away from the whitespace (cursor cells aren't dimmed).
        app.press(K::End, cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "→   ab··c·d·");
        let cs = cells(&app, &term, 0, 12);
        for x in [0, 6, 7, 9, 11] {
            assert_eq!(cs[x].fg, dim(&app), "cell {x} in the whitespace colour");
        }
        assert_ne!(cs[4].fg, dim(&app), "text keeps its colour");

        // Off: the same columns, blanks instead of glyphs.
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"none\"\n", "", TEXT);
        app.press(K::End, cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "    ab  c d ");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn boundary_skips_single_spaces_between_words() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"boundary\"\n", "", TEXT);
        app.press(K::End, cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "→   ab··c d·");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn trailing_draws_only_whitespace_at_line_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"trailing\"\n", "", "\ta b  \n  \nx\n");
        app.press(K::End, cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 9), "    a b··");
        assert_eq!(row(&app, &term, 1, 3), "·· ", "a blank line is all trailing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn selection_draws_only_selected_whitespace_on_the_selection_background() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"selection\"\n", "", "a b c\nd e\n");
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 5), "a b c", "nothing selected");
        // Select "a b" (Shift+Right × 3).
        for _ in 0..3 {
            app.press(K::Right, M::SHIFT);
        }
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 5), "a·b c");
        assert_eq!(row(&app, &term, 1, 3), "d e");
        let cs = cells(&app, &term, 0, 2);
        let selection = app.theme.get("editor.selection").bg.unwrap();
        assert_eq!(
            (cs[1].fg, cs[1].bg),
            (dim(&app), selection),
            "glyph colour over the selection"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn select_all_keeps_the_glyphs_under_the_selection_background() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "", "", TEXT);
        app.press(K::Char('a'), cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "→   ab··c·d·");
        let selection = app.theme.get("editor.selection").bg.unwrap();
        let cs = cells(&app, &term, 0, 12);
        for x in [0, 6, 11] {
            assert_eq!((cs[x].fg, cs[x].bg), (dim(&app), selection), "cell {x}");
        }
        for (x, c) in cs.iter().enumerate().take(4).skip(1) {
            assert_eq!(c.bg, selection, "the tab's cell {x} is selected too");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_whitespace_colour_wins_over_syntax_colours() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_file(&dir, "", "", "a.py", "x = \"a b\"\n");
        assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
        app.press(K::End, cmd());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 9), "x·=·\"a·b\"");
        let cs = cells(&app, &term, 0, 9);
        let string = app.theme.get("string").fg.unwrap();
        assert_eq!(cs[5].fg, string, "the string is coloured");
        assert_eq!(cs[6].fg, dim(&app), "a space in the string in the whitespace colour");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cursors_on_a_space_stay_visible() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "", "", "a b\na b\n");
        app.press(K::Right, M::NONE);
        app.test_execute(Command::AddCursorBelow);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 3), "a·b");
        assert_eq!(row(&app, &term, 1, 3), "a·b");
        let text = app.hits.groups[0].text;
        // The primary: the terminal's cursor on the `·`, not dimmed.
        let mut term = term;
        let at = term.get_cursor_position().unwrap();
        let (line, other) = if at == Position::new(text.x + 1, text.y) {
            (0, 1)
        } else {
            (1, 0)
        };
        assert_eq!(
            at,
            Position::new(text.x + 1, text.y + line),
            "the terminal cursor on a `·`"
        );
        let primary = &cells(&app, &term, line, 2)[1];
        assert_ne!(primary.fg, dim(&app), "the primary's cell isn't dimmed");
        // The secondary: a reversed cell, not in the dim colour (it would be its background).
        let secondary = &cells(&app, &term, other, 2)[1];
        assert!(secondary.modifier.contains(Modifier::REVERSED));
        assert_ne!(secondary.fg, dim(&app), "the secondary cursor isn't dimmed");
        assert_eq!(secondary.sym, "·");

        // Unfocused, no terminal cursor marks the primary: its `·` is dimmed like the others.
        app.test_execute(Command::FocusExplorer);
        let term = draw(&mut app);
        assert_eq!(cells(&app, &term, line, 2)[1].fg, dim(&app));
        let secondary = &cells(&app, &term, other, 2)[1];
        assert!(secondary.modifier.contains(Modifier::REVERSED));
        assert_ne!(secondary.fg, dim(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn toggle_flips_between_none_and_the_setting_for_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let user = "render_whitespace = \"boundary\"\n";
        let (mut app, root, user_file) = open_app(&dir, user, "", TEXT);
        app.press(K::End, cmd());
        assert_eq!(drawn_line0(&mut app), "→   ab··c d·");
        run_command(&mut app, "View: Toggle Render Whitespace");
        assert_eq!(app.last_command, Some(Command::ToggleRenderWhitespace));
        assert_eq!(app.render_whitespace(), RenderWhitespace::None);
        assert_eq!(drawn_line0(&mut app), "    ab  c d ");
        assert_eq!(
            fs::read_to_string(&user_file).unwrap(),
            format!("[settings]\n{user}"),
            "never written to settings.toml"
        );
        assert_eq!(fs::read_to_string(root.join(".workspace")).unwrap(), "");

        // A `.workspace` reload changes the setting but keeps the toggle.
        fs::write(
            root.join(".workspace"),
            "[settings]\nrender_whitespace = \"trailing\"\n",
        )
        .unwrap();
        assert!(
            app.settle(|a| a.settings.render_whitespace == RenderWhitespace::Trailing)
                .await
        );
        assert_eq!(drawn_line0(&mut app), "    ab  c d ", "still toggled off");
        // Toggled back: the setting as it is now.
        app.test_execute(Command::ToggleRenderWhitespace);
        assert_eq!(drawn_line0(&mut app), "    ab  c d·");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn toggle_turns_all_on_when_the_setting_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"none\"\n", "", TEXT);
        app.press(K::End, cmd());
        assert_eq!(drawn_line0(&mut app), "    ab  c d ");
        app.test_execute(Command::ToggleRenderWhitespace);
        assert_eq!(drawn_line0(&mut app), "→   ab··c·d·");
        app.test_execute(Command::ToggleRenderWhitespace);
        assert_eq!(drawn_line0(&mut app), "    ab  c d ");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_setting_wins_over_the_users() {
        let dir = tempfile::tempdir().unwrap();
        let ws = "[settings]\nrender_whitespace = \"trailing\"\n";
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"none\"\n", ws, TEXT);
        app.press(K::End, cmd());
        assert_eq!(app.settings.render_whitespace, RenderWhitespace::Trailing);
        assert_eq!(drawn_line0(&mut app), "    ab  c d·");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_bad_value_is_reported() {
        // In settings.toml: reported and skipped, the other keys apply.
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root, _) = open_app(&dir, "render_whitespace = \"sometimes\"\ntab_width = 2\n", "", TEXT);
        let msg = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
        assert!(msg.contains("render_whitespace"), "{msg}");
        assert_eq!(
            app.settings.render_whitespace,
            RenderWhitespace::All,
            "the default stays"
        );
        assert_eq!(app.settings.tab_width, 2);

        // In `.workspace`: the file is rejected (as any bad `.workspace`), the last good one kept.
        fs::write(
            root.join(".workspace"),
            "[settings]\nrender_whitespace = \"sometimes\"\n",
        )
        .unwrap();
        assert!(app.settle(|a| a.workspace.config_error.is_some()).await);
        let error = app.workspace.config_error.clone().unwrap();
        assert!(
            error.contains("render_whitespace") || error.contains("sometimes"),
            "{error}"
        );
        assert_eq!(app.settings.render_whitespace, RenderWhitespace::All);
    }
}
