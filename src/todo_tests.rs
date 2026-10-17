//! End-to-end tests for single PLAN.md TODO items, one `mod` per item.
//!
//! - `render_whitespace`: spaces drawn as `·` and tabs as `→` (in the tab's first cell) in
//!   `editorWhitespace.foreground` (over syntax colours), text columns unchanged, under the
//!   selection background and never dimming a drawn cursor; each of VS Code's modes (`none` / `boundary` / `selection` /
//!   `trailing` / `all`) from `settings.toml` and `.workspace`, a bad value reported; "View:
//!   Toggle Render Whitespace" from the palette, session-only and kept across a `.workspace` reload.
//! - `scroll_bars`: a vertical scroll bar in each editor group's last column (text and diff
//!   tabs): the thumb sized to the screen's share and placed at the top line, none for a text
//!   that fits; pressing the thumb and dragging scrolls (also with the pointer off the bar, until
//!   release), a press on the track centres the thumb there, neither moving a cursor nor focusing
//!   the group; the wheel over it scrolls; hover / drag colours (no hover left after a release
//!   elsewhere); text clicks unchanged; below the changed-on-disk banner, none for a group
//!   without tabs, the find widget left of it, tiny editors.

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

mod scroll_bars {
    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::buffer::pos;
    use crate::command::Command;
    use crate::layout::GroupId;
    use crate::scrollbar::Scroll;
    use crate::selection::Selections;

    const DOWN: Mouse = Mouse::Down(MouseButton::Left);
    const DRAG: Mouse = Mouse::Drag(MouseButton::Left);
    const UP: Mouse = Mouse::Up(MouseButton::Left);

    fn numbered(lines: usize) -> String {
        (1..=lines).map(|i| format!("line {i}\n")).collect()
    }

    /// An app with `a.txt` holding `text` open, drawn once (80×24: 22 text rows).
    fn open_app(dir: &tempfile::TempDir, text: &str) -> (App, Terminal<TestBackend>) {
        let root = crate::workspace::normalize(dir.path());
        let path = root.join("a.txt");
        std::fs::write(&path, text).unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&path).unwrap());
        app.test_start();
        let term = draw(&mut app);
        (app, term)
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    fn group(app: &App, g: GroupId) -> &crate::layout::GroupHit {
        app.hits.group(g).unwrap()
    }

    fn track(app: &App, g: GroupId) -> Rect {
        group(app, g).scrollbar.expect("a scroll bar").track
    }

    fn slider(app: &App, key: &str) -> Color {
        app.theme.get(key).bg.unwrap()
    }

    /// The track's rows (from its top) drawn in the slider colour `key`, as (start, len); also
    /// checks they are the rows the hit map says.
    fn drawn_thumb(app: &App, term: &Terminal<TestBackend>, g: GroupId, key: &str) -> Option<(u16, u16)> {
        let t = track(app, g);
        let colour = slider(app, key);
        let rows: Vec<u16> = (0..t.height)
            .filter(|&y| term.backend().buffer()[(t.x, t.y + y)].bg == colour)
            .collect();
        let drawn = rows.first().map(|&s| (s, rows.len() as u16));
        if let Some((s, n)) = drawn {
            assert_eq!(rows, (s..s + n).collect::<Vec<_>>(), "one run");
        }
        let hit = group(app, g).scrollbar.unwrap().thumb.map(|t| (t.start, t.len));
        if drawn.is_some() || key == "scrollbarSlider.background" {
            assert_eq!(drawn, hit, "drawn where the hit map says");
        }
        drawn
    }

    fn thumb(app: &App, term: &Terminal<TestBackend>) -> Option<(u16, u16)> {
        drawn_thumb(app, term, app.groups.active, "scrollbarSlider.background")
    }

    fn top(app: &App) -> usize {
        app.group().active_tab().unwrap().view.top_line()
    }

    fn selections(app: &App) -> Selections {
        app.group().active_tab().unwrap().view.selections.clone()
    }

    /// The thumb the state says, for comparing with what's drawn.
    fn expected(app: &App) -> (u16, u16) {
        let tab = app.group().active_tab().unwrap();
        let lines = pos::line_count(&app.active_buffer().unwrap().rope);
        let t = Scroll::text(lines, tab.view.height, tab.view.top_line())
            .thumb(track(app, app.groups.active).height)
            .unwrap();
        (t.start, t.len)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_long_text_shows_a_thumb_sized_to_the_screen_at_the_top_line() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, term) = open_app(&dir, &numbered(200));
        let editor = group(&app, app.groups.active).editor;
        let t = track(&app, app.groups.active);
        assert_eq!(
            (t.x, t.y, t.width),
            (editor.x + editor.width, editor.y, 1),
            "right of the editor"
        );
        assert_eq!(t.height, 22);
        let text = group(&app, app.groups.active).text;
        assert_eq!(text.x + text.width, t.x, "the text ends before the bar");
        assert_eq!(app.group().active_tab().unwrap().view.width, text.width as usize);
        // 201 lines (the last empty) on 22 rows, scrollable until the last is at the top:
        // 22 / (200 + 22) of the track.
        assert_eq!(thumb(&app, &term), Some((0, 2)));

        // Scrolled down: the thumb follows the top line.
        for delta in [3, 30, 120] {
            for _ in 0..delta / 3 {
                app.mouse(Mouse::ScrollDown, text.x + 2, text.y + 2);
            }
            let term = draw(&mut app);
            assert_eq!(thumb(&app, &term), Some(expected(&app)));
        }
        let term = draw(&mut app);
        assert!(thumb(&app, &term).unwrap().0 > 0);
        // At the end: the thumb at the end of the track.
        app.press(K::End, cmd());
        let term = draw(&mut app);
        let (start, len) = thumb(&app, &term).unwrap();
        assert!(start + len < 22, "the last line isn't at the top yet");
        for _ in 0..100 {
            app.mouse(Mouse::ScrollDown, text.x + 2, text.y + 2);
        }
        assert_eq!(top(&app), 200, "the last line at the top");
        let term = draw(&mut app);
        assert_eq!(thumb(&app, &term), Some((20, 2)));
    }

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_text_that_fits_shows_no_thumb_and_the_bar_ignores_clicks() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, term) = open_app(&dir, &numbered(5));
        assert_eq!(thumb(&app, &term), None);
        let t = track(&app, app.groups.active);
        let before = selections(&app);
        app.mouse(DOWN, t.x, t.y + 10);
        app.mouse(DRAG, t.x, t.y + 15);
        app.mouse(UP, t.x, t.y + 15);
        assert_eq!((top(&app), selections(&app)), (0, before));
        // Scrolled with the wheel (the last line can go to the top): a thumb to drag back with.
        app.mouse(Mouse::ScrollDown, t.x - 5, t.y + 1);
        assert_eq!(top(&app), 3);
        let term = draw(&mut app);
        let (start, len) = thumb(&app, &term).unwrap();
        assert!(start > 0, "{start} {len}");
        app.mouse(DOWN, t.x, t.y + start);
        app.mouse(DRAG, t.x, 0);
        assert_eq!(top(&app), 0);
        // While held at the top the drag goes on (the thumb stays drawn, active).
        let term = draw(&mut app);
        assert_eq!(
            drawn_thumb(&app, &term, app.groups.active, "scrollbarSlider.activeBackground").map(|t| t.0),
            Some(0)
        );
        // Back to where it was grabbed: the first row the thumb is drawn there for (5 rows of
        // scroll on a 4-cell run, so rows 2 and 3 share a cell).
        app.mouse(DRAG, t.x, t.y + start);
        assert_eq!((start, top(&app)), (2, 2));
        app.mouse(DRAG, t.x, 0);
        app.mouse(UP, t.x, 0);
        let term = draw(&mut app);
        assert_eq!(thumb(&app, &term), None, "released at the top: hidden again");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dragging_the_thumb_scrolls_and_moves_no_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, term) = open_app(&dir, &numbered(1000));
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::SHIFT);
        let before = selections(&app);
        let t = track(&app, app.groups.active);
        let (start, len) = thumb(&app, &term).unwrap();
        assert_eq!((start, len), (0, 1));
        // Grab it (the pointer over it first, as a real press) and drag 10 rows down: the
        // thumb follows the pointer exactly.
        app.mouse(Mouse::Moved, t.x, t.y);
        app.mouse(DOWN, t.x, t.y);
        assert_eq!(top(&app), 0, "a press on the thumb doesn't scroll");
        app.mouse(DRAG, t.x, t.y + 10);
        let at_10 = top(&app);
        assert!(at_10 > 0);
        let term = draw(&mut app);
        assert_eq!(
            drawn_thumb(&app, &term, app.groups.active, "scrollbarSlider.activeBackground"),
            Some((10, 1))
        );
        // The pointer off the bar, even off the editor: the drag goes on by its row.
        app.mouse(DRAG, 2, t.y + 15);
        let at_15 = top(&app);
        assert!(at_15 > at_10);
        let term = draw(&mut app);
        assert_eq!(
            drawn_thumb(&app, &term, app.groups.active, "scrollbarSlider.activeBackground"),
            Some((15, 1))
        );
        // Past the bottom: the last line at the top.
        app.mouse(DRAG, 2, 23);
        assert_eq!(top(&app), 1000);
        app.mouse(UP, 2, 23);
        // Released: later drags do nothing.
        app.mouse(DRAG, t.x, t.y);
        assert_eq!(top(&app), 1000);
        assert_eq!(selections(&app), before, "selections untouched");
        let term = draw(&mut app);
        assert_eq!(
            thumb(&app, &term),
            Some((21, 1)),
            "released away from the pointer: the normal colour, not the hover from before the press"
        );
        assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grabbing_a_long_thumb_keeps_the_grabbed_cell_under_the_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, term) = open_app(&dir, &numbered(40));
        let t = track(&app, app.groups.active);
        let (start, len) = thumb(&app, &term).unwrap();
        assert_eq!((start, len), (0, 8), "22 rows of 62");
        // Grabbed by its 5th cell, moved 6 rows down.
        app.mouse(DOWN, t.x, t.y + 4);
        app.mouse(DRAG, t.x, t.y + 10);
        let term = draw(&mut app);
        assert_eq!(
            drawn_thumb(&app, &term, app.groups.active, "scrollbarSlider.activeBackground"),
            Some((6, 8))
        );
        // And back above its start: the top.
        app.mouse(DRAG, t.x, t.y + 1);
        assert_eq!(top(&app), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_press_on_the_track_centres_the_thumb_there_and_grabs_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        let before = selections(&app);
        let t = track(&app, app.groups.active);
        app.mouse(DOWN, t.x, t.y + 15);
        let term = draw(&mut app);
        let (start, len) = drawn_thumb(&app, &term, app.groups.active, "scrollbarSlider.activeBackground").unwrap();
        assert_eq!(len, 4, "22 rows of 121");
        assert_eq!(start, 15 - len / 2, "centred on the pointer");
        let at = top(&app);
        // Held: dragging moves it from there.
        app.mouse(DRAG, t.x, t.y + 16);
        assert!(top(&app) > at);
        app.mouse(UP, t.x, t.y + 16);
        // Above the thumb: back up.
        app.mouse(DOWN, t.x, t.y);
        app.mouse(UP, t.x, t.y);
        assert_eq!(top(&app), 0);
        assert_eq!(selections(&app), before);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_wheel_over_the_bar_scrolls_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        let t = track(&app, app.groups.active);
        app.mouse(Mouse::ScrollDown, t.x, t.y + 5);
        assert_eq!(top(&app), 3);
        app.mouse(Mouse::ScrollUp, t.x, t.y + 5);
        assert_eq!(top(&app), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_pointer_over_the_thumb_draws_it_in_the_hover_colour() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        let t = track(&app, app.groups.active);
        let hover = "scrollbarSlider.hoverBackground";
        app.mouse(Mouse::Moved, t.x, t.y + 1);
        let term = draw(&mut app);
        assert_eq!(drawn_thumb(&app, &term, app.groups.active, hover), Some((0, 4)));
        app.mouse(Mouse::Moved, t.x, t.y + 10);
        let term = draw(&mut app);
        assert_eq!(
            drawn_thumb(&app, &term, app.groups.active, hover),
            None,
            "on the track: not the thumb"
        );
        assert_eq!(thumb(&app, &term), Some((0, 4)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_click_on_the_text_still_places_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        let text = group(&app, app.groups.active).text;
        app.mouse(DOWN, text.x + 3, text.y + 4);
        app.mouse(UP, text.x + 3, text.y + 4);
        let buf = app.active_buffer().unwrap();
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        assert_eq!(pos::line_of(&buf.rope, head), 4);
        assert_eq!(head.0, pos::line_start(&buf.rope, 4).0 + 3);
        // The text area's last column, left of the bar: still text.
        app.mouse(DOWN, text.x + text.width - 1, text.y + 2);
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        assert_eq!(pos::line_of(&app.active_buffer().unwrap().rope, head), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_bar_of_an_unfocused_group_scrolls_it_without_focusing_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(300));
        let left = app.groups.active;
        app.test_execute(Command::SplitRight);
        let right = app.groups.active;
        assert_ne!(left, right);
        let term = draw(&mut app);
        let t = track(&app, left);
        assert!(t.x < track(&app, right).x);
        assert!(drawn_thumb(&app, &term, left, "scrollbarSlider.background").is_some());
        app.mouse(DOWN, t.x, t.y + 20);
        app.mouse(UP, t.x, t.y + 20);
        assert_eq!(app.groups.active, right, "focus stays");
        let view_top = |g| app.groups.get(g).unwrap().active_tab().unwrap().view.top_line();
        assert!(view_top(left) > 200);
        assert_eq!(view_top(right), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_diff_tab_has_a_bar_over_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(200));
        // An unsaved change, compared with the file on disk.
        app.type_text("x");
        let path = app.active_buffer().unwrap().path.clone();
        app.open_diff(path, crate::diff_view::Pair::SAVED);
        assert!(
            app.settle(|a| a.group().active_diff().is_some_and(|d| d.doc.is_some()))
                .await
        );
        let term = draw(&mut app);
        let g = app.groups.active;
        let rows = group(&app, g).diff.as_ref().unwrap().rows;
        let t = track(&app, g);
        assert_eq!(
            (t.x, t.y, t.height),
            (rows.x + rows.width, rows.y, rows.height),
            "beside the rows"
        );
        let d = app.group().active_diff().unwrap();
        let n = d.rows().len();
        assert!(n > 100, "{n} rows");
        let drawn = drawn_thumb(&app, &term, g, "scrollbarSlider.background").unwrap();
        let expected = Scroll::rows(n, d.height, d.top).thumb(t.height).unwrap();
        assert_eq!(drawn, (expected.start, expected.len));
        assert!(drawn.0 <= 1, "at the first change, near the top");
        // A cursor move asks the next frame to reveal the cursor; dragged to the bottom before
        // that frame: the last page, the pointer deciding (not snapped back to the cursor).
        app.press(K::Down, M::NONE);
        app.mouse(Mouse::Moved, t.x, t.y + drawn.0);
        app.mouse(DOWN, t.x, t.y + drawn.0);
        app.mouse(DRAG, t.x, 23);
        app.mouse(UP, t.x, 23);
        let term = draw(&mut app);
        let d = app.group().active_diff().unwrap();
        assert_eq!(d.top, n - d.height);
        assert!(d.cursor < 10, "the cursor stays: {}", d.cursor);
        let (start, len) = drawn_thumb(&app, &term, g, "scrollbarSlider.background").unwrap();
        assert_eq!(start + len, t.height, "at the end of the track");
        // The header row's cell isn't part of the track.
        let header = term.backend().buffer()[(t.x, t.y - 1)].bg;
        assert_eq!(Some(header), app.theme.get("diffEditor.header").bg);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_banner_row_is_above_the_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        app.type_text("x");
        let path = app.active_buffer().unwrap().path.clone();
        std::fs::write(&path, numbered(120)).unwrap();
        assert!(app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await);
        let term = draw(&mut app);
        let g = group(&app, app.groups.active);
        let banner = g.banner.as_ref().expect("the banner").row;
        let t = track(&app, app.groups.active);
        assert_eq!((t.y, t.height), (banner.y + 1, 21), "below the banner");
        assert_eq!((g.editor.y, g.editor.height), (t.y, t.height));
        assert_eq!(app.group().active_tab().unwrap().view.height, 21);
        assert_eq!(thumb(&app, &term), Some(expected(&app)));
        // The banner's row at the right edge is the banner's, not the bar's.
        assert!(matches!(app.hits.hit(t.x, banner.y), crate::layout::Hit::Banner(..)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_group_without_tabs_has_no_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        let full = {
            let g = group(&app, app.groups.active);
            g.editor.width + 1
        };
        app.test_execute(Command::CloseTab);
        assert!(app.group().tabs.is_empty());
        draw(&mut app);
        let g = group(&app, app.groups.active);
        assert!(g.scrollbar.is_none());
        assert_eq!(g.editor.width, full, "the splash gets the whole width");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_find_widget_sits_left_of_the_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        app.test_execute(Command::OpenFind);
        let term = draw(&mut app);
        let t = track(&app, app.groups.active);
        let find = app.hits.find.as_ref().expect("the find widget").area;
        assert_eq!(find.x + find.width, t.x);
        assert_eq!(thumb(&app, &term), Some((0, 4)), "the thumb not covered");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tiny_editors_draw_and_take_clicks_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &numbered(100));
        app.type_text("x");
        let path = app.active_buffer().unwrap().path.clone();
        app.open_diff(path, crate::diff_view::Pair::SAVED);
        assert!(
            app.settle(|a| a.group().active_diff().is_some_and(|d| d.doc.is_some()))
                .await
        );
        let diff = app.group().active;
        for tab in [diff, 0] {
            app.group_mut().active = tab;
            let mut thumbs = 0;
            for h in 4..10 {
                let mut term = Terminal::new(TestBackend::new(40, h)).unwrap();
                term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
                let Some(bar) = app.hits.group(app.groups.active).and_then(|g| g.scrollbar) else {
                    continue;
                };
                let t = bar.track;
                if let Some(th) = bar.thumb {
                    thumbs += 1;
                    assert!(th.len >= 1 && th.start + th.len <= t.height, "{h}: {th:?} on {t:?}");
                }
                for y in t.y..t.y + t.height {
                    app.mouse(DOWN, t.x, y);
                    app.mouse(DRAG, t.x, 0);
                    app.mouse(DRAG, t.x, h - 1);
                    app.mouse(UP, t.x, h - 1);
                    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
                }
            }
            assert!(thumbs >= 4, "tab {tab}: {thumbs} thumbs drawn");
        }
    }
}
