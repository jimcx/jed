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
//! - `side_scroll_bars`: the same bar in the last column of the explorer tree, the Search view's
//!   results, the Source Control list and the bottom panel's Problems / Locations lists: the
//!   thumb from the list's rows, height and top; hover, press-and-drag (off the bar and past the
//!   end: where the wheel's end is), a press on the track, the wheel over it; the list's
//!   selection and the focus unchanged; counts, git letters and buttons left of it; lists that
//!   fit show no thumb and their bar ignores clicks; no bar for an empty list or another tab;
//!   a drag whose list goes away (another view, the panel hidden, another tab) scrolls nothing.
//! - `inline_blame`: "Git: Toggle Inline Blame" (off by default; palette or command) draws the
//!   primary cursor line's blame `GAP` cells after its text in
//!   `git.blame.editorDecorationForeground` (`Ann, 6 yr ago • First commit`), following the
//!   cursor without recomputing; hidden by a selection over lines; "Uncommitted changes" for an
//!   unsaved edit (after the delay), back after undo; a new commit outside jed shows; cut with
//!   "…" at the text area's edge (not on the scroll bar), none on a line filling the width,
//!   the text never moved; nothing for untracked files, outside a repository, or toggled off;
//!   staged lines still uncommitted; nothing computed while off or for background tabs; turned
//!   off and on again it is blamed afresh (an amended HEAD commit shows).
//! - `folder_filter`: the Search view's files to include / exclude (shown by `…` / Cmd+Shift+J)
//!   narrow the search as the reference filtered by hand (folder names, paths, `*.md`, lists),
//!   on top of `.workspace`'s excludes; exclude beats include; `!` within a field; dirty buffers
//!   (also ones whose file is gone) filtered the same way; an invalid glob's error and nothing
//!   run; hidden fields still apply (`…` lit); Tab / Shift+Tab, clicks, pastes, the debounce and
//!   the list below them; "Find in Folder…" from an explorer folder's right-click and the
//!   palette (a file's right-click still shows its history).
//! - `rust_language_server`: `rust-analyzer` is the built-in server for Rust (no
//!   `initializationOptions`; `.workspace` `[lsp.rust]` overrides it when trusted); work-done
//!   progress shows in the status bar ("rust-analyzer: Indexing 40% 3/25 (core)") until it ends;
//!   ContentModified answers are dropped quietly; a server dying during `initialize` says why
//!   (its last stderr line); and against the real `rust-analyzer` (skipped
//!   with a note when it doesn't run) on a tiny crate: hover, definition, references, completion,
//!   `cargo check` errors on save, rename and format document.

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

mod side_scroll_bars {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    use crate::app::{App, Focus};
    use crate::buffer::Buffer;
    use crate::buffer::pos::{LspPos, PosEncoding};
    use crate::grouped;
    use crate::layout::{PanelTab, ScrollBarHit, ScrollTarget, SidePanel};
    use crate::locations::{Location, Locations};
    use crate::lsp::ServerId;
    use crate::problems;
    use crate::scrollbar::Scroll;
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    const DOWN: Mouse = Mouse::Down(MouseButton::Left);
    const DRAG: Mouse = Mouse::Drag(MouseButton::Left);
    const UP: Mouse = Mouse::Up(MouseButton::Left);
    const NORMAL: &str = "scrollbarSlider.background";
    const HOVER: &str = "scrollbarSlider.hoverBackground";
    const ACTIVE: &str = "scrollbarSlider.activeBackground";

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A temp dir with `proj/` (the workspace) and `state/` (the trust store, outside the repo).
    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        (dir, root, state)
    }

    /// `n` files `fNN.txt` in `root`, each holding `text`.
    fn files(root: &Path, n: usize, text: &str) {
        for i in 0..n {
            fs::write(root.join(format!("f{i:02}.txt")), text).unwrap();
        }
    }

    /// The app on `root` with `f00.txt` open (the editor focused), the explorer listed.
    async fn open_app(root: &Path, state: &Path) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let trust = TrustStore::load(state.join("trust.toml"));
        let buffer = Buffer::open(&root.join("f00.txt")).unwrap();
        let initial = crate::app::InitialFile { buffer, goto: None };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.test_start();
        app.test_plain_whitespace();
        assert!(app.settle(|a| a.explorer.row_count() > 0).await);
        app
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    fn bar(app: &App, t: ScrollTarget) -> ScrollBarHit {
        app.hits.scrollbar(t).expect("a scroll bar")
    }

    /// The track's rows drawn in the slider colour `key` as (start, len), checked to be one run
    /// and (in any colour but a missing one) where the hit map says.
    fn drawn_thumb(app: &App, term: &Terminal<TestBackend>, t: ScrollTarget, key: &str) -> Option<(u16, u16)> {
        let track = bar(app, t).track;
        let colour = app.theme.get(key).bg.unwrap();
        let rows: Vec<u16> = (0..track.height)
            .filter(|&y| term.backend().buffer()[(track.x, track.y + y)].bg == colour)
            .collect();
        let drawn = rows.first().map(|&s| (s, rows.len() as u16));
        if let Some((s, n)) = drawn {
            assert_eq!(rows, (s..s + n).collect::<Vec<_>>(), "one run");
        }
        if drawn.is_some() || key == NORMAL {
            assert_eq!(
                drawn,
                bar(app, t).thumb.map(|t| (t.start, t.len)),
                "drawn where the hit map says"
            );
        }
        drawn
    }

    /// What a list's thumb should be: `rows` rows on a track as tall as the list, from `top`.
    fn expected(rows: usize, track: u16, top: usize) -> Option<(u16, u16)> {
        Scroll::rows(rows, track as usize, top)
            .thumb(track)
            .map(|t| (t.start, t.len))
    }

    /// A list under test: its bar, its row count, its top row and what must not change
    /// (its selection, by key).
    struct List {
        target: ScrollTarget,
        rows: fn(&App) -> usize,
        top: fn(&App) -> usize,
        selection: fn(&App) -> String,
    }

    /// The shared checks: the drawn thumb follows the list's scroll; hover, press-and-drag
    /// (off the bar too, past the end: the wheel's end), a press on the track, the wheel over
    /// the bar; the list's selection and the focus never change.
    fn drag_the_bar(app: &mut App, l: &List) {
        let (t, rows) = (l.target, (l.rows)(app));
        let (focus, selection) = (app.ui.focus, (l.selection)(app));
        let term = draw(app);
        let track = bar(app, t).track;
        let h = track.height;
        assert!(rows > h as usize, "{rows} rows in {h}: the list must not fit");
        assert_eq!((l.top)(app), 0);
        assert_eq!(drawn_thumb(app, &term, t, NORMAL), expected(rows, h, 0));
        let (_, len) = drawn_thumb(app, &term, t, NORMAL).unwrap();
        assert!(len < h);

        // The pointer over the thumb: the hover colour.
        app.mouse(Mouse::Moved, track.x, track.y);
        let term = draw(app);
        assert_eq!(drawn_thumb(app, &term, t, HOVER), Some((0, len)));

        // Pressed and dragged 3 rows: the thumb follows the pointer.
        app.mouse(DOWN, track.x, track.y);
        assert_eq!((l.top)(app), 0, "a press on the thumb doesn't scroll");
        app.mouse(DRAG, track.x, track.y + 3);
        let mid = (l.top)(app);
        assert!(mid > 0);
        let term = draw(app);
        assert_eq!(drawn_thumb(app, &term, t, ACTIVE), Some((3, len)));
        assert_eq!(expected(rows, h, mid), Some((3, len)));

        // Off the bar and past the bottom: the list's end, as the wheel's.
        app.mouse(DRAG, 0, 23);
        let end = rows - h as usize;
        assert_eq!((l.top)(app), end, "the last row at the bottom");
        app.mouse(UP, 0, 23);
        app.mouse(DRAG, track.x, track.y);
        assert_eq!((l.top)(app), end, "released: drags do nothing");
        let term = draw(app);
        assert_eq!(
            drawn_thumb(app, &term, t, NORMAL),
            Some((h - len, len)),
            "at the track's end"
        );

        // A press on the track above it: the thumb centres there (and is held until release).
        app.mouse(DOWN, track.x, track.y + 1);
        app.mouse(UP, track.x, track.y + 1);
        let at = (l.top)(app);
        assert!(at < mid, "{at} {mid}");
        let term = draw(app);
        assert_eq!(drawn_thumb(app, &term, t, NORMAL), expected(rows, h, at));
        assert_eq!(at, 0, "1 - len / 2 is above the track: the top");

        // The wheel over the bar scrolls the list as the wheel over it, to the same end.
        app.mouse(Mouse::ScrollDown, track.x, track.y + 2);
        assert_eq!((l.top)(app), 3.min(end));
        for _ in 0..rows {
            app.mouse(Mouse::ScrollDown, track.x, track.y + 2);
        }
        assert_eq!((l.top)(app), end);
        app.mouse(Mouse::ScrollUp, track.x, track.y + 2);
        assert_eq!((l.top)(app), end - 3);

        assert_eq!(app.ui.focus, focus, "the focus stays");
        assert_eq!((l.selection)(app), selection, "the list's selection stays");
    }

    fn explorer_list() -> List {
        List {
            target: ScrollTarget::Side(SidePanel::Explorer),
            rows: |a| a.explorer.row_count(),
            top: |a| a.explorer.scroll,
            selection: |a| format!("{:?}", a.explorer.selected),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_explorer_tree_has_a_bar_that_scrolls_it_without_selecting() {
        let (_dir, root, state) = dirs();
        files(&root, 60, "x\n");
        let mut app = open_app(&root, &state).await;
        assert!(app.settle(|a| a.explorer.row_count() == 60).await);
        app.explorer.selected = Some(root.join("f02.txt"));
        let _ = draw(&mut app);
        let side = app.hits.regions.side.unwrap();
        let track = bar(&app, explorer_list().target).track;
        // The rows' last column (left of the side bar's border), from the first row down.
        assert_eq!(track.x, side.x + side.width - 2);
        assert_eq!(
            (track.y, track.height),
            (app.hits.explorer_rows.y, app.hits.explorer_rows.height)
        );
        assert_eq!(
            app.hits.explorer_rows.x + app.hits.explorer_rows.width,
            track.x,
            "rows left of it"
        );
        drag_the_bar(&mut app, &explorer_list());
        assert_eq!(app.ui.focus, Focus::Editor);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_explorer_that_fits_shows_no_thumb_and_its_bar_ignores_clicks() {
        let (_dir, root, state) = dirs();
        files(&root, 5, "x\n");
        let mut app = open_app(&root, &state).await;
        assert!(app.settle(|a| a.explorer.row_count() == 5).await);
        let term = draw(&mut app);
        let t = explorer_list().target;
        assert_eq!(drawn_thumb(&app, &term, t, NORMAL), None);
        let track = bar(&app, t).track;
        // A press on the bar beside a row: the selection (the open file) stays, no focus, no
        // scroll.
        assert_eq!(app.explorer.selected, Some(root.join("f00.txt")));
        let row_y = app.hits.explorer_rows.y + 1;
        app.mouse(DOWN, track.x, row_y);
        app.mouse(DRAG, track.x, row_y + 3);
        app.mouse(UP, track.x, row_y + 3);
        assert_eq!(app.explorer.selected, Some(root.join("f00.txt")));
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(app.explorer.scroll, 0);
        // A click on the row itself, left of the bar, still selects it.
        app.mouse(DOWN, track.x - 1, row_y);
        app.mouse(UP, track.x - 1, row_y);
        assert_eq!(app.explorer.selected, Some(root.join("f01.txt")));
        assert_eq!(app.ui.focus, Focus::SidePanel);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_search_views_results_have_a_bar_below_the_query() {
        let (_dir, root, state) = dirs();
        files(&root, 10, &"a needle here\n".repeat(5));
        let mut app = open_app(&root, &state).await;
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("needle");
        app.search_tick(std::time::Instant::now() + crate::search_view::DEBOUNCE);
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.files.len() == 10))
                .await
        );
        // The results take the keys, their first row selected.
        app.press(K::Down, M::NONE);
        assert!(app.search_view.list_focused);
        let list = List {
            target: ScrollTarget::Side(SidePanel::Search),
            rows: |a| grouped::row_count(&a.search.as_ref().unwrap().files),
            top: |a| a.search_view.scroll(a.search.as_ref().unwrap()),
            selection: |a| format!("{:?} {}", a.search_view.list.selected(), a.search_view.list_focused),
        };
        let term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        let track = bar(&app, list.target).track;
        assert_eq!(
            (track.y, track.height),
            (hit.list.y, hit.list.height),
            "beside the results only"
        );
        assert_eq!(hit.list.x + hit.list.width, track.x);
        // The bar in the column left of the side bar's border; the query row keeps the full
        // width: its toggles end where they did without a bar (one cell from the border).
        let side = app.hits.regions.side.unwrap();
        assert_eq!(track.x, side.x + side.width - 2);
        assert_eq!(hit.details.x + hit.details.width, side.x + side.width - 2);
        // The file rows' counts end left of the bar (one cell between).
        let buf = term.backend().buffer();
        let row = hit.rows.iter().position(|k| k.1 == 0).unwrap() as u16 + hit.list.y;
        assert_eq!(
            buf[(track.x - 3, row)].symbol(),
            "5",
            "the count right-aligned left of the bar"
        );
        drag_the_bar(&mut app, &list);
        assert_eq!(app.ui.focus, Focus::SidePanel);
    }

    /// A repository with `n` untracked files.
    async fn scm_app(n: usize) -> (tempfile::TempDir, App) {
        let (dir, root, state) = dirs();
        crate::git::test_repo::init(&root);
        files(&root, n, "x\n");
        let mut app = open_app(&root, &state).await;
        assert!(
            app.settle(|a| !a.git.running() && a.git.status.as_ref().is_some_and(|s| s.entries.len() == n))
                .await
        );
        app.press(K::Char('g'), M::CONTROL | M::SHIFT);
        (dir, app)
    }

    fn scm_list() -> List {
        List {
            target: ScrollTarget::Side(SidePanel::SourceControl),
            rows: |a| a.scm.rows(a.git.status.as_ref().unwrap()).len(),
            top: |a| a.scm.scroll(&a.scm.rows(a.git.status.as_ref().unwrap())),
            selection: |a| {
                let rows = a.scm.rows(a.git.status.as_ref().unwrap());
                format!("{:?}", a.scm.selected(&rows).map(|r| r.key()))
            },
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_source_control_list_has_a_bar_left_of_which_letters_and_buttons_go() {
        let (_dir, mut app) = scm_app(40).await;
        assert_eq!(app.ui.focus, Focus::SidePanel);
        let list = scm_list();
        assert!((list.selection)(&app).contains("f00.txt"), "the first file selected");
        // (Selected before the view's first frame, so scrolled below its header: back up.)
        let _ = draw(&mut app);
        let track = bar(&app, list.target).track;
        app.mouse(Mouse::ScrollUp, track.x - 5, track.y);
        let term = draw(&mut app);
        let hit = app.hits.scm.clone().unwrap();
        assert_eq!((track.y, track.height), (hit.list.y, hit.list.height));
        assert_eq!(hit.list.x + hit.list.width, track.x);
        // The selected file's letter and its stage button left of the bar.
        let buf = term.backend().buffer();
        let y = hit.list.y + 1;
        assert_eq!(buf[(track.x - 2, y)].symbol(), "U");
        let (button, ..) = hit.buttons.iter().find(|(r, ..)| r.y == y).unwrap();
        assert_eq!(button.x, track.x - 4);
        assert_eq!(buf[(track.x, y)].symbol(), " ", "nothing drawn on the bar");
        drag_the_bar(&mut app, &list);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_source_control_list_that_fits_shows_no_thumb() {
        let (_dir, mut app) = scm_app(3).await;
        let term = draw(&mut app);
        let t = scm_list().target;
        assert_eq!(drawn_thumb(&app, &term, t, NORMAL), None);
        let before = (scm_list().selection)(&app);
        let track = bar(&app, t).track;
        // The keys back in the editor; a press on the bar beside a file row: no selection
        // change, no diff opened, the focus stays.
        app.press(K::Esc, M::NONE);
        assert_eq!(app.ui.focus, Focus::Editor);
        app.mouse(DOWN, track.x, track.y + 2);
        app.mouse(UP, track.x, track.y + 2);
        assert_eq!((scm_list().selection)(&app), before);
        assert!(app.group().active_diff().is_none());
        assert_eq!(app.ui.focus, Focus::Editor);
    }

    /// `n` files with 5 problems each in the language server store (files not open: their
    /// LSP positions).
    fn plant_problems(app: &mut App, root: &Path, n: usize) {
        for i in 0..n {
            let path = root.join(format!("p{i:02}.cpp"));
            let diagnostics: Vec<_> = (0..5)
                .map(|l| {
                    json!({
                        "range": { "start": { "line": l, "character": 0 }, "end": { "line": l, "character": 1 } },
                        "severity": 1,
                        "message": format!("problem {l}"),
                    })
                })
                .collect();
            let params = json!({ "uri": crate::lsp::uri::from_path(&path).as_str(), "diagnostics": diagnostics });
            let published = crate::lsp::diagnostics::parse(params).unwrap();
            app.lsp
                .diagnostics
                .publish(ServerId::test(1), PosEncoding::Utf8, published);
        }
    }

    fn problems_list() -> List {
        List {
            target: ScrollTarget::Panel(PanelTab::Problems),
            rows: |a| grouped::row_count(&problems::files(&a.lsp.diagnostics, &a.buffers)),
            top: |a| a.problems.list.scroll(&problems::files(&a.lsp.diagnostics, &a.buffers)),
            selection: |a| format!("{:?}", a.problems.list.selected()),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_problems_list_has_a_bar_at_the_panels_right_edge() {
        let (_dir, root, state) = dirs();
        files(&root, 1, "x\n");
        let mut app = open_app(&root, &state).await;
        plant_problems(&mut app, &root, 6);
        app.press(K::Char('m'), cmd() | M::SHIFT);
        assert_eq!(app.ui.focus, Focus::Panel);
        let list = problems_list();
        assert!(app.problems.list.selected().is_some(), "the first row selected");
        let term = draw(&mut app);
        let panel = app.hits.regions.panel.unwrap();
        let hit = app.hits.panel_list.clone().unwrap();
        let track = bar(&app, list.target).track;
        assert_eq!(track.x, panel.x + panel.width - 1, "the panel's last column");
        assert_eq!(
            (track.y, track.height),
            (hit.list.y, hit.list.height),
            "below the tab strip"
        );
        assert_eq!(hit.list.x + hit.list.width, track.x);
        // A file row's count left of the bar.
        let buf = term.backend().buffer();
        assert_eq!(buf[(track.x - 3, hit.list.y)].symbol(), "5");
        drag_the_bar(&mut app, &list);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_empty_problems_list_has_no_bar() {
        let (_dir, root, state) = dirs();
        files(&root, 1, "x\n");
        let mut app = open_app(&root, &state).await;
        app.press(K::Char('m'), cmd() | M::SHIFT);
        let _ = draw(&mut app);
        assert!(app.hits.scrollbar(ScrollTarget::Panel(PanelTab::Problems)).is_none());
        // A list that fits: a bar without a thumb.
        plant_problems(&mut app, &root, 1);
        let term = draw(&mut app);
        assert_eq!(drawn_thumb(&app, &term, problems_list().target, NORMAL), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_locations_list_has_a_bar_too() {
        let (_dir, root, state) = dirs();
        files(&root, 1, "x\n");
        let mut app = open_app(&root, &state).await;
        let places = (0..4)
            .flat_map(|f| {
                let path = root.join(format!("l{f}.rs"));
                (0..6).map(move |line| {
                    let at = LspPos { line, character: 0 };
                    Location {
                        path: path.clone(),
                        start: at,
                        end: at,
                    }
                })
            })
            .collect();
        app.locations = Some(Locations::new("References to 'x'".into(), PosEncoding::Utf8, places, 1));
        app.ui.panel_visible = true;
        app.ui.panel_tab = PanelTab::Locations;
        // A row on screen selected (once drawn, so it doesn't scroll).
        let _ = draw(&mut app);
        let l = app.locations.as_mut().unwrap();
        l.list.select(&l.files, 2);
        let term = draw(&mut app);
        let panel = app.hits.regions.panel.unwrap();
        let hit = app.hits.panel_list.clone().unwrap();
        let track = bar(&app, ScrollTarget::Panel(PanelTab::Locations)).track;
        assert_eq!(track.x, panel.x + panel.width - 1, "the panel's last column");
        assert_eq!((track.y, track.height), (hit.list.y, hit.list.height));
        assert_eq!(hit.list.x + hit.list.width, track.x);
        // A file row's count left of the bar.
        assert_eq!(term.backend().buffer()[(track.x - 3, hit.list.y)].symbol(), "6");
        let list = List {
            target: ScrollTarget::Panel(PanelTab::Locations),
            rows: |a| grouped::row_count(&a.locations.as_ref().unwrap().files),
            top: |a| {
                let l = a.locations.as_ref().unwrap();
                l.list.scroll(&l.files)
            },
            selection: |a| format!("{:?}", a.locations.as_ref().unwrap().list.selected()),
        };
        drag_the_bar(&mut app, &list);
        assert_eq!(app.ui.focus, Focus::Editor);
        // Another tab shown: its bar is gone, and the Locations target scrolls nothing.
        app.ui.panel_tab = PanelTab::Problems;
        let _ = draw(&mut app);
        assert!(app.hits.scrollbar(list.target).is_none());
    }
    /// A drag whose list goes away (another side bar view, the panel hidden, another panel
    /// tab) scrolls nothing, even before a frame redraws the bars.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_drag_whose_list_goes_away_scrolls_nothing() {
        let (_dir, root, state) = dirs();
        files(&root, 60, "x\n");
        let mut app = open_app(&root, &state).await;
        assert!(app.settle(|a| a.explorer.row_count() == 60).await);
        let _ = draw(&mut app);
        let track = bar(&app, explorer_list().target).track;
        app.mouse(DOWN, track.x, track.y);
        // The Search view instead (nothing searched yet).
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.mouse(DRAG, track.x, track.y + 5);
        assert_eq!(app.explorer.scroll, 0, "the hidden explorer stays");
        app.mouse(UP, track.x, track.y + 5);

        plant_problems(&mut app, &root, 6);
        app.press(K::Char('m'), cmd() | M::SHIFT);
        let _ = draw(&mut app);
        let list = problems_list();
        let track = bar(&app, list.target).track;
        app.mouse(DOWN, track.x, track.y);
        app.press(K::Char('j'), cmd());
        assert!(!app.ui.panel_visible);
        app.mouse(DRAG, track.x, track.y + 3);
        assert_eq!((list.top)(&app), 0, "the hidden panel's list stays");
        app.mouse(UP, track.x, track.y + 3);

        // Another tab mid-drag (the Locations list shown instead).
        app.press(K::Char('j'), cmd());
        let _ = draw(&mut app);
        app.mouse(DOWN, track.x, track.y);
        let at = LspPos { line: 0, character: 0 };
        let place = Location {
            path: root.join("f00.txt"),
            start: at,
            end: at,
        };
        app.locations = Some(Locations::new("x".into(), PosEncoding::Utf8, vec![place], 1));
        app.ui.panel_tab = PanelTab::Locations;
        app.mouse(DRAG, track.x, track.y + 3);
        assert_eq!((list.top)(&app), 0, "the Problems list, no longer shown, stays");
    }
}

mod inline_blame {
    use std::fs;
    use std::path::Path;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use crate::app::App;
    use crate::blame::Want;
    use crate::command::Command;
    use crate::git::test_repo::{git, init};
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};

    /// 2020-01-01T00:00:00Z and 2021-06-01T12:00:00Z.
    const ANN: (&str, u64) = ("2020-01-01T00:00:00Z", 1_577_836_800);
    const BOB: (&str, u64) = ("2021-06-01T12:00:00Z", 1_622_548_800);

    /// The width of the text area `draw` gives (the explorer and the scroll bar left out).
    const WIDTH: usize = 61;

    /// Commits everything as `who` (`<who>@example.com`), authored at `date`.
    fn commit_as(root: &Path, who: &str, date: &str, message: &str) {
        git(root, &["add", "-A"]);
        let author = format!("{who} <{}@example.com>", who.to_lowercase());
        git(
            root,
            &["commit", "-q", "--author", &author, "--date", date, "-m", message],
        );
    }

    /// Line 0 by Ann; lines 1–6 by Bob: a short one, one long enough to cut the blame, one
    /// filling the width, one past it, one leaving `MIN_WIDTH` cells after the gap, one less.
    fn text() -> String {
        let gap = crate::blame::GAP;
        let min = crate::blame::MIN_WIDTH;
        format!(
            "alpha\nbeta\n{}\n{}\n{}\n{}\n{}\n",
            "c".repeat(WIDTH - 20),
            "d".repeat(WIDTH),
            "e".repeat(WIDTH + 5),
            "f".repeat(WIDTH - gap - min),
            "g".repeat(WIDTH - gap - min + 1)
        )
    }

    /// A repository with `a.txt` committed by Ann, then changed by Bob, and the app on it with the
    /// status read.
    async fn blamed_project() -> (tempfile::TempDir, std::path::PathBuf, App) {
        let (dir, root, state) = dirs();
        init(&root);
        fs::write(root.join("a.txt"), "alpha\nold\n").unwrap();
        commit_as(&root, "Ann", ANN.0, "First commit\n\nWith a body");
        fs::write(root.join("a.txt"), text()).unwrap();
        commit_as(&root, "Bob", BOB.0, "Second change");
        let mut app = open_file_app(&root, &state, "a.txt");
        assert!(settle_git(&mut app, |_| true).await);
        (dir, root, app)
    }

    async fn settle_blame(app: &mut App) -> bool {
        app.settle(|a| {
            let b = a.active_buffer().unwrap();
            !b.blame.running() && b.blame.want(b.revision()) == Want::Nothing
        })
        .await
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        assert_eq!(app.hits.groups[0].text.width as usize, WIDTH);
        term
    }

    /// Buffer line `line` as drawn across the text area (no scrolling), trailing blanks dropped.
    fn row(app: &App, term: &Terminal<TestBackend>, line: u16) -> String {
        let text = app.hits.groups[0].text;
        let buf = term.backend().buffer();
        let s: String = (0..text.width)
            .map(|x| buf[(text.x + x, text.y + line)].symbol())
            .collect();
        s.trim_end().to_string()
    }

    /// The foreground of the cell at column `col` of buffer line `line`.
    fn fg(app: &App, term: &Terminal<TestBackend>, line: u16, col: u16) -> Color {
        let text = app.hits.groups[0].text;
        term.backend().buffer()[(text.x + col, text.y + line)].fg
    }

    fn blame_colour(app: &App) -> Color {
        app.theme.get("git.blame.editorDecorationForeground").fg.unwrap()
    }

    fn label(who: &str, secs: u64, summary: &str) -> String {
        let when = crate::history::ago(secs * 1000, crate::history::now_ms());
        format!("{who}, {when} • {summary}")
    }

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn go_to_line(app: &mut App, line: usize) {
        app.press(K::Home, cmd());
        for _ in 0..line {
            app.press(K::Down, M::NONE);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_cursor_lines_blame_follows_the_cursor_and_edits() {
        let (_d, _root, mut app) = blamed_project().await;
        // Off by default: nothing drawn, nothing computed.
        app.pump();
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0), "alpha");
        let b = &app.active_buffer().unwrap().blame;
        assert!(!b.running() && b.hunks().is_empty());

        app.test_execute(Command::ToggleBlame);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        let ann = label("Ann", ANN.1, "First commit");
        assert_eq!(row(&app, &term, 0), format!("alpha   {ann}"));
        assert_eq!(fg(&app, &term, 0, 8), blame_colour(&app));
        assert_eq!(row(&app, &term, 1), "beta", "only the cursor line");

        // Another commit's line: drawn at once, nothing recomputed.
        go_to_line(&mut app, 1);
        app.pump();
        assert!(!app.active_buffer().unwrap().blame.running());
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0), "alpha");
        assert_eq!(
            row(&app, &term, 1),
            format!("beta   {}", label("Bob", BOB.1, "Second change"))
        );

        // A selection over two lines hides it; within one line it stays.
        app.press(K::Up, M::SHIFT);
        let term = draw(&mut app);
        assert_eq!(
            (row(&app, &term, 0), row(&app, &term, 1)),
            ("alpha".into(), "beta".into())
        );
        app.press(K::Down, M::NONE);
        app.press(K::End, M::SHIFT);
        let term = draw(&mut app);
        assert!(row(&app, &term, 1).starts_with("beta   Bob, "));

        // An unsaved edit: uncommitted once recomputed (after the delay); other lines keep theirs.
        // Until then the hunks follow the edit: Bob's still, no computation before the delay.
        app.press(K::End, M::NONE);
        app.type_text("!");
        app.pump();
        assert!(!app.active_buffer().unwrap().blame.running());
        assert!(app.blame.deadline().is_some());
        let term = draw(&mut app);
        assert!(row(&app, &term, 1).starts_with("beta!   Bob, "));
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 1), "beta!   Uncommitted changes");
        go_to_line(&mut app, 0);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0), format!("alpha   {ann}"));

        // Undone: Bob's again.
        app.press(K::Char('z'), cmd());
        assert!(settle_blame(&mut app).await);
        go_to_line(&mut app, 1);
        let term = draw(&mut app);
        assert!(row(&app, &term, 1).starts_with("beta   Bob, "));

        // Turned off: nothing.
        app.test_execute(Command::ToggleBlame);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 1), "beta");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn long_lines_cut_the_blame_at_the_edge_and_full_ones_draw_none() {
        let (_d, _root, mut app) = blamed_project().await;
        app.test_execute(Command::ToggleBlame);
        assert!(settle_blame(&mut app).await);
        let bob = label("Bob", BOB.1, "Second change");

        // 17 cells after the line's end: cut with "…" in the text area's last cell, never on
        // the scroll bar.
        go_to_line(&mut app, 2);
        let term = draw(&mut app);
        let line = "c".repeat(WIDTH - 20);
        let room = 20 - crate::blame::GAP;
        let cut: String = bob.chars().take(room - 1).collect();
        assert_eq!(row(&app, &term, 2), format!("{line}   {cut}…"));
        assert_eq!(fg(&app, &term, 2, WIDTH as u16 - 1), blame_colour(&app));

        // Filling the width, or past it: no blame, the text unchanged.
        for (l, c) in [(3, 'd'), (4, 'e')] {
            go_to_line(&mut app, l);
            let term = draw(&mut app);
            assert_eq!(row(&app, &term, l as u16), c.to_string().repeat(WIDTH));
        }

        // `MIN_WIDTH` cells left after the gap: cut to them; one fewer: none.
        let min = crate::blame::MIN_WIDTH;
        go_to_line(&mut app, 5);
        let term = draw(&mut app);
        let line = "f".repeat(WIDTH - crate::blame::GAP - min);
        let cut: String = bob.chars().take(min - 1).collect();
        assert_eq!(row(&app, &term, 5), format!("{line}   {cut}…"));
        go_to_line(&mut app, 6);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 6), "g".repeat(WIDTH - crate::blame::GAP - min + 1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_commit_updates_it() {
        let (_d, root, mut app) = blamed_project().await;
        // From the palette.
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text("Git: Toggle Inline Blame");
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, "Git: Toggle Inline Blame");
        app.press(K::Enter, M::NONE);
        assert!(app.blame.on);
        assert!(settle_blame(&mut app).await);
        go_to_line(&mut app, 1);
        app.press(K::End, M::NONE);
        app.type_text("!");
        app.press(K::Char('s'), cmd());
        assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 1), "beta!   Uncommitted changes");

        // Staged, not committed: still uncommitted.
        git(&root, &["add", "a.txt"]);
        assert!(settle_git(&mut app, |s| s.entries.iter().any(|e| e.staged.is_some())).await);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 1), "beta!   Uncommitted changes");

        commit_as(&root, "Cy", "2022-01-01T00:00:00Z", "Third");
        assert!(settle_git(&mut app, |s| s.entries.is_empty()).await);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(
            row(&app, &term, 1),
            format!("beta!   {}", label("Cy", 1_640_995_200, "Third"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn untracked_files_and_files_outside_a_repository_have_none() {
        // Untracked.
        let (_d, root, state) = dirs();
        init(&root);
        fs::write(root.join("a.txt"), "alpha\n").unwrap();
        commit_as(&root, "Ann", ANN.0, "First");
        fs::write(root.join("new.txt"), "fresh\n").unwrap();
        let mut app = open_file_app(&root, &state, "new.txt");
        assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
        app.test_execute(Command::ToggleBlame);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0), "fresh");
        assert!(app.active_buffer().unwrap().blame.hunks().is_empty());

        // No repository.
        let (_d, root, state) = dirs();
        fs::write(root.join("a.txt"), "alpha\n").unwrap();
        let mut app = open_file_app(&root, &state, "a.txt");
        app.test_execute(Command::ToggleBlame);
        assert!(app.settle(|a| !a.git.running()).await);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0), "alpha");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_shown_buffers_are_blamed_and_turning_it_off_forgets() {
        let (_d, root, mut app) = blamed_project().await;
        fs::write(root.join("b.txt"), "bee\n").unwrap();
        commit_as(&root, "Cy", "2022-01-01T00:00:00Z", "Third");
        assert!(settle_git(&mut app, |s| s.entries.is_empty()).await);
        let a = app.active_buffer().unwrap().path.clone();
        app.test_open(crate::buffer::Buffer::open(&root.join("b.txt")).unwrap());
        app.test_execute(Command::ToggleBlame);
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        assert_eq!(
            row(&app, &term, 0),
            format!("bee   {}", label("Cy", 1_640_995_200, "Third"))
        );
        // a.txt, in a background tab: not blamed until shown.
        let buf = app.buffers.iter().map(|(_, b)| b).find(|b| b.path == a).unwrap();
        assert!(buf.blame.hunks().is_empty() && buf.blame.want(buf.revision()) == Want::Now);
        app.test_execute(Command::NextTab);
        assert_eq!(app.active_buffer().unwrap().path, a);
        assert!(settle_blame(&mut app).await);
        go_to_line(&mut app, 1);
        let term = draw(&mut app);
        assert_eq!(
            row(&app, &term, 1),
            format!("beta   {}", label("Bob", BOB.1, "Second change"))
        );

        // Back on b.txt: turned off, HEAD's commit rewritten (b.txt's text unchanged), turned on:
        // blamed afresh at once, not the blame from before.
        app.test_execute(Command::NextTab);
        assert_ne!(app.active_buffer().unwrap().path, a);
        app.test_execute(Command::ToggleBlame);
        git(
            &root,
            &[
                "commit",
                "-q",
                "--amend",
                "--author",
                "Eve <eve@example.com>",
                "-m",
                "Amended",
            ],
        );
        assert!(settle_git(&mut app, |_| true).await);
        app.test_execute(Command::ToggleBlame);
        assert!(app.active_buffer().unwrap().blame.running(), "computed at once");
        assert!(settle_blame(&mut app).await);
        let term = draw(&mut app);
        let row0 = row(&app, &term, 0);
        assert!(
            row0.starts_with("bee   Eve, ") && row0.ends_with(" • Amended"),
            "{row0}"
        );
    }
}

mod folder_filter {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use crossterm::event::{Event, KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, Focus};
    use crate::buffer::Buffer;
    use crate::layout::SidePanel;
    use crate::search::Options;
    use crate::search_view::{DEBOUNCE, Input};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};
    use crate::workspace_search::reference;

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
    }

    /// A temp dir with `proj/` (the workspace: needles in several folders, `docs/` excluded by
    /// `.workspace`) and `state/` (the trust store).
    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(&state).unwrap();
        for (rel, text) in [
            ("README.md", "a needle in the readme\n"),
            ("src/main.rs", "fn main() { needle(); }\n"),
            ("src/app/mod.rs", "// needle\nneedle needle\n"),
            ("src/app/notes.md", "needle notes\n"),
            ("src/gen/out.rs", "needle gen\n"),
            ("lib/src/x.rs", "a needle\n"),
            ("tests/t.rs", "needle test\n"),
            ("docs/d.md", "needle docs\n"),
            (".workspace", "[files]\nexclude = [\"docs/\"]\n"),
        ] {
            write(&root, rel, text);
        }
        (dir, root, state)
    }

    /// The app on `root` with `README.md` open (the editor focused), the explorer listed.
    async fn open_app(root: &Path, state: &Path) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let trust = TrustStore::load(state.join("trust.toml"));
        let buffer = Buffer::open(&root.join("README.md")).unwrap();
        let initial = crate::app::InitialFile { buffer, goto: None };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.test_start();
        app.test_plain_whitespace();
        assert!(app.settle(|a| a.explorer.row_count() > 0).await);
        app
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// The drawn text of row `y` between columns `x` and `x + width`.
    fn text_at(term: &Terminal<TestBackend>, x: u16, y: u16, width: u16) -> String {
        let buf = term.backend().buffer();
        (x..x + width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The reference search (every file, then this test's own idea of the filter: `keep` on the
    /// relative path; `docs/` is `.workspace`'s exclude) as `path:line:start-end`.
    fn expected(root: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
        reference::reference(root, "needle", Options::default())
            .into_iter()
            .filter(|h| {
                let path = h.split(':').next().unwrap();
                !path.starts_with("docs/") && keep(path)
            })
            .collect()
    }

    /// Gives `input` the keys with Tab (showing the details first when it's one of theirs) and
    /// replaces its text by typing.
    fn set_field(app: &mut App, input: Input, text: &str) {
        if input != Input::Query && !app.search_view.details {
            app.press(K::Char('j'), cmd() | M::SHIFT);
        }
        for _ in 0..4 {
            if !app.search_view.list_focused && app.search_view.input == input {
                break;
            }
            app.press(K::Tab, M::NONE);
        }
        assert!(!app.search_view.list_focused && app.search_view.input == input);
        app.press(K::End, M::NONE);
        for _ in 0..app.search_view.input_mut().text.chars().count() {
            app.press(K::Backspace, M::NONE);
        }
        app.type_text(text);
    }

    /// Runs a palette command by its title.
    fn run_command(app: &mut App, title: &str) {
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text(title);
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, title);
        app.press(K::Enter, M::NONE);
    }

    /// Waits for the debounced search of the view's current fields to finish.
    async fn finish(app: &mut App) -> Vec<String> {
        app.search_tick(Instant::now() + DEBOUNCE);
        let want = app.search_view.query();
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.query == want))
                .await,
            "the search of {want:?}"
        );
        reference::hits(app.search.as_ref().unwrap())
    }

    /// Searches for "needle" through the Search view's inputs, typed.
    async fn search(app: &mut App, include: &str, exclude: &str) -> Vec<String> {
        app.press(K::Char('f'), cmd() | M::SHIFT);
        set_field(app, Input::Query, "needle");
        set_field(app, Input::Include, include);
        set_field(app, Input::Exclude, exclude);
        finish(app).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn include_and_exclude_narrow_the_search_like_the_reference() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        type Keep = fn(&str) -> bool;
        let cases: [(&str, &str, Keep); 10] = [
            ("", "", |_| true),
            // A bare folder name: that folder anywhere, everything under it.
            ("src", "", |p| p.starts_with("src/") || p.contains("/src/")),
            // A path: relative to the root.
            ("./src", "", |p| p.starts_with("src/")),
            ("src/app", "", |p| p.starts_with("src/app/")),
            ("src/**/*.rs", "", |p| p.starts_with("src/") && p.ends_with(".rs")),
            // `*.md` at any depth; a list.
            ("*.md", "", |p| p.ends_with(".md")),
            ("tests, README.md", "", |p| p.starts_with("tests/") || p == "README.md"),
            ("", "*.rs", |p| !p.ends_with(".rs")),
            ("", "gen, tests/", |p| {
                !p.starts_with("src/gen/") && !p.starts_with("tests/")
            }),
            // On top of `.workspace`'s excludes: `docs` stays out even when included.
            ("docs", "", |_| false),
        ];
        for (include, exclude, keep) in cases {
            let got = search(&mut app, include, exclude).await;
            let want = expected(&root, keep);
            assert_eq!(got, want, "include {include:?}, exclude {exclude:?}");
            if include.is_empty() && exclude.is_empty() {
                assert_eq!(want.len(), 9, "every needle outside docs/");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exclude_beats_include() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        assert!(search(&mut app, "src", "src").await.is_empty());
        let draw_term = draw(&mut app);
        let side = app.hits.regions.side.unwrap();
        let text: String = (side.y..side.y + side.height)
            .map(|y| text_at(&draw_term, side.x, y, side.width))
            .collect();
        assert!(text.contains("No results found"), "{text}");
        let got = search(&mut app, "*.rs", "app").await;
        assert_eq!(got, expected(&root, |p| p.ends_with(".rs") && !p.contains("app/")));
        assert!(!got.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn negation_takes_matches_back_within_a_field() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        // Include: `!` takes paths out of the field's others.
        let got = search(&mut app, "./src, !gen, !*.md", "").await;
        let want = expected(&root, |p| {
            p.starts_with("src/") && !p.starts_with("src/gen/") && !p.ends_with(".md")
        });
        assert_eq!(got, want);
        assert_eq!(want.len(), 4);
        // Exclude: `!` takes paths back.
        let got = search(&mut app, "", "src, !src/app").await;
        assert_eq!(
            got,
            expected(&root, |p| !(p.starts_with("src/") || p.contains("/src/"))
                || p.starts_with("src/app/"))
        );
        // Only negations in include: everything else.
        let got = search(&mut app, "!src", "").await;
        assert_eq!(
            got,
            expected(&root, |p| !(p.starts_with("src/") || p.contains("/src/")))
        );
    }

    /// Dirty buffers are searched from their text and filtered by the same rules, also ones
    /// whose file is gone from disk.
    #[tokio::test(flavor = "multi_thread")]
    async fn dirty_buffers_are_filtered_the_same_way() {
        let (_dir, root, state) = project();
        write(&root, "src/new.rs", "\n");
        write(&root, "lib/new.rs", "\n");
        let mut app = open_app(&root, &state).await;
        for rel in ["src/main.rs", "src/new.rs", "lib/new.rs"] {
            app.test_open(Buffer::open(&root.join(rel)).unwrap());
            app.type_text("needle ");
        }
        fs::remove_file(root.join("src/new.rs")).unwrap();
        fs::remove_file(root.join("lib/new.rs")).unwrap();
        assert!(
            app.settle(|a| a.buffers.iter().filter(|(_, b)| b.deleted).count() == 2)
                .await
        );
        assert_eq!(app.buffers.iter().filter(|(_, b)| b.is_dirty()).count(), 3);

        let dirty = |p: &str| match p {
            "src/main.rs" => vec!["src/main.rs:1:0-6".to_string(), "src/main.rs:1:19-25".into()],
            "src/new.rs" => vec!["src/new.rs:1:0-6".into()],
            "lib/new.rs" => vec!["lib/new.rs:1:0-6".into()],
            _ => unreachable!(),
        };
        // The reference reads the disk: swap in the buffers' matches (main.rs's moved over).
        let with_dirty = |keep: fn(&str) -> bool| {
            let mut want: Vec<String> = expected(&root, keep)
                .into_iter()
                .filter(|h| !h.starts_with("src/main.rs:"))
                .collect();
            for p in ["src/main.rs", "src/new.rs", "lib/new.rs"] {
                if keep(p) {
                    want.extend(dirty(p));
                }
            }
            want.sort_by(|a, b| a.split(':').next().cmp(&b.split(':').next()).then(a.cmp(b)));
            want
        };
        let mut got = search(&mut app, "./src", "").await;
        got.sort_by(|a, b| a.split(':').next().cmp(&b.split(':').next()).then(a.cmp(b)));
        assert_eq!(got, with_dirty(|p| p.starts_with("src/")));
        assert!(got.iter().any(|h| h.starts_with("src/new.rs:")));
        assert!(!got.iter().any(|h| h.starts_with("lib/")));

        // An include prunes no folder: the walk reaches the dirty `src/main.rs`, which the
        // filter must still refuse.
        let mut got = search(&mut app, "lib", "").await;
        got.sort_by(|a, b| a.split(':').next().cmp(&b.split(':').next()).then(a.cmp(b)));
        assert_eq!(got, with_dirty(|p| p.starts_with("lib/")));
        assert!(got.iter().any(|h| h.starts_with("lib/new.rs:")));

        let mut got = search(&mut app, "", "src").await;
        got.sort_by(|a, b| a.split(':').next().cmp(&b.split(':').next()).then(a.cmp(b)));
        assert_eq!(got, with_dirty(|p| !(p.starts_with("src/") || p.contains("/src/"))));
        assert!(got.iter().any(|h| h.starts_with("lib/new.rs:")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_invalid_glob_shows_an_error_and_runs_nothing() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        assert!(!search(&mut app, "", "").await.is_empty());
        set_field(&mut app, Input::Include, "src, {a");
        app.search_tick(Instant::now() + DEBOUNCE);
        let error = app
            .search_view
            .error
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(error.starts_with("files to include: {a: "), "{error}");
        assert!(app.search.is_none(), "nothing runs");
        let term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        let y = hit.inputs[2].y + 1;
        let side = app.hits.regions.side.unwrap();
        assert!(text_at(&term, side.x, y, side.width).contains("files to include: {a"));

        set_field(&mut app, Input::Exclude, "[z-a]");
        app.search_tick(Instant::now() + DEBOUNCE);
        assert!(app.search_view.error.is_some());
        set_field(&mut app, Input::Include, "src");
        set_field(&mut app, Input::Exclude, "");
        let got = finish(&mut app).await;
        assert!(app.search_view.error.is_none());
        assert_eq!(got, expected(&root, |p| p.starts_with("src/") || p.contains("/src/")));
    }

    /// Hidden, the include / exclude fields still apply (as VS Code); the `…` button stays lit
    /// while they do, and a click on it shows them again.
    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_fields_still_apply() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        search(&mut app, "tests", "").await;
        let term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        let off = app.theme.get("sideBar").patch(app.theme.get("find.option"));
        let lit = app.theme.get("sideBar").patch(app.theme.get("find.option.active"));
        let style = |term: &Terminal<TestBackend>, r: ratatui::layout::Rect| term.backend().buffer()[(r.x, r.y)].fg;
        let (off, lit) = (off.fg.unwrap(), lit.fg.unwrap());
        assert_eq!(style(&term, hit.details), lit, "shown");

        app.press(K::Char('j'), cmd() | M::SHIFT);
        assert!(!app.search_view.details);
        assert_eq!(app.search_view.input, Input::Query, "the query gets the keys back");
        set_field(&mut app, Input::Query, "NEEDLE");
        let got = finish(&mut app).await;
        assert_eq!(got, expected(&root, |p| p.starts_with("tests/")));
        let term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        assert_eq!(hit.inputs[1], ratatui::layout::Rect::default(), "not drawn");
        assert_eq!(hit.list.y, hit.inputs[0].y + 3, "the summary, a blank row, the list");
        assert_eq!(style(&term, hit.details), lit, "lit: a hidden filter applies");

        app.mouse(Mouse::Down(MouseButton::Left), hit.details.x, hit.details.y);
        assert!(app.search_view.details);
        assert_eq!(app.search_view.input, Input::Include);
        app.press(K::End, M::NONE);
        for _ in 0..5 {
            app.press(K::Backspace, M::NONE);
        }
        assert!(app.search_view.include.text.is_empty());
        app.press(K::Char('j'), cmd() | M::SHIFT);
        let term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        assert_ne!(off, lit);
        assert_eq!(style(&term, hit.details), off, "hidden and empty");
        let got = finish(&mut app).await;
        assert_eq!(got, expected(&root, |_| true));
    }

    /// The inputs are drawn under the query, take the keys by Tab / Shift+Tab, a click or the
    /// list's typing, and get pastes; their edits wait for the debounce; the results list and its
    /// scroll bar start below them.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_details_inputs_take_keys_clicks_and_pastes() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("needle");
        finish(&mut app).await;
        app.press(K::Char('j'), cmd() | M::SHIFT);
        let mut term = draw(&mut app);
        let hit = app.hits.search.clone().unwrap();
        let [q, inc, exc] = hit.inputs;
        assert_eq!((inc.y, exc.y), (q.y + 1, q.y + 2));
        assert_eq!(text_at(&term, inc.x, inc.y, 16), "files to include");
        assert_eq!(text_at(&term, exc.x, exc.y, 16), "files to exclude");
        assert_eq!(text_at(&term, hit.details.x, hit.details.y, 1), "…");
        assert!(hit.list.y > exc.y);
        let track = app
            .hits
            .scrollbar(crate::layout::ScrollTarget::Side(SidePanel::Search))
            .unwrap()
            .track;
        assert_eq!(
            (track.y, track.height),
            (hit.list.y, hit.list.height),
            "beside the list only"
        );
        assert_eq!(
            term.get_cursor_position().unwrap(),
            ratatui::layout::Position { x: inc.x, y: inc.y },
            "the cursor in the include input"
        );

        // A paste goes to the focused input and waits for the debounce.
        let generation = app.search.as_ref().unwrap().generation;
        app.handle_test_event(Event::Paste("tests\n".into()));
        assert_eq!(app.search_view.include.text, "tests");
        assert_eq!(app.search_view.query.text, "needle");
        assert!(app.search_view.deadline().is_some());
        assert_eq!(app.search.as_ref().unwrap().generation, generation);
        assert_eq!(finish(&mut app).await, expected(&root, |p| p.starts_with("tests/")));

        // Tab: exclude, the list, the query; Shift+Tab back.
        app.press(K::Tab, M::NONE);
        assert_eq!(app.search_view.input, Input::Exclude);
        app.press(K::Tab, M::NONE);
        assert!(app.search_view.list_focused);
        app.press(K::Tab, M::NONE);
        assert_eq!(
            (app.search_view.list_focused, app.search_view.input),
            (false, Input::Query)
        );
        app.press(K::Tab, M::SHIFT);
        assert!(app.search_view.list_focused);
        // Typing in the list goes to the input that last had the keys.
        app.press(K::Tab, M::SHIFT);
        assert_eq!(app.search_view.input, Input::Exclude);
        app.press(K::Down, M::NONE);
        assert!(app.search_view.list_focused, "Down: the list");
        app.type_text("x");
        assert_eq!(app.search_view.exclude.text, "x");
        // A click on an input gives it the keys.
        draw(&mut app);
        app.mouse(Mouse::Down(MouseButton::Left), q.x + 2, q.y);
        assert_eq!((app.ui.focus, app.search_view.input), (Focus::SidePanel, Input::Query));
        app.handle_test_event(Event::Paste("!".into()));
        assert_eq!(app.search_view.query.text, "needle!");
    }

    /// "Find in Folder…": a right-click on an explorer folder (a file's still shows its history)
    /// or the command on the selected folder: the Search view, its details shown, the folder as
    /// the files to include, the query focused, searched now.
    #[tokio::test(flavor = "multi_thread")]
    async fn find_in_folder_from_the_explorer() {
        let (_dir, root, state) = project();
        let mut app = open_app(&root, &state).await;
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("needle");
        finish(&mut app).await;
        assert!(!app.search_view.details);

        let explorer = |app: &mut App| {
            app.press(K::Char('e'), cmd() | M::SHIFT);
            draw(app);
        };
        let row_of = |app: &App, rel: &str| -> u16 {
            let i = app
                .explorer
                .rows()
                .iter()
                .position(|r| r.entry.path == root.join(rel))
                .unwrap();
            app.hits.explorer_rows.y + (i - app.explorer.scroll) as u16
        };
        // A file's row: its history, the Search view untouched.
        explorer(&mut app);
        let y = row_of(&app, "README.md");
        app.mouse(Mouse::Down(MouseButton::Right), app.hits.explorer_rows.x + 2, y);
        assert_eq!(app.ui.side_panel, SidePanel::Explorer);
        assert!(app.search_view.include.text.is_empty());

        let y = row_of(&app, "src");
        app.mouse(Mouse::Down(MouseButton::Right), app.hits.explorer_rows.x + 2, y);
        assert_eq!((app.ui.focus, app.ui.side_panel), (Focus::SidePanel, SidePanel::Search));
        assert_eq!(app.search_view.include.text, "./src");
        assert!(app.search_view.details && !app.search_view.list_focused);
        assert_eq!(app.search_view.input, Input::Query);
        assert!(app.search_view.query.all_selected, "typing replaces the query");
        assert!(app.search_view.deadline().is_none(), "searched at once");
        let want = app.search_view.query();
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.query == want))
                .await
        );
        let got = reference::hits(app.search.as_ref().unwrap());
        assert_eq!(got, expected(&root, |p| p.starts_with("src/")));

        // The command, on a folder deeper down (expanded first) and on a file.
        explorer(&mut app);
        let y = row_of(&app, "lib");
        app.mouse(Mouse::Down(MouseButton::Left), app.hits.explorer_rows.x + 2, y);
        assert!(
            app.settle(|a| a.explorer.rows().iter().any(|r| r.entry.path.ends_with("lib/src")))
                .await
        );
        app.explorer.selected = Some(root.join("lib/src"));
        run_command(&mut app, "Explorer: Find in Folder…");
        assert_eq!(app.search_view.include.text, "./lib/src");
        let want = app.search_view.query();
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.query == want))
                .await
        );
        let got = reference::hits(app.search.as_ref().unwrap());
        assert_eq!(got, expected(&root, |p| p.starts_with("lib/src/")));

        explorer(&mut app);
        app.explorer.selected = Some(root.join("README.md"));
        run_command(&mut app, "Explorer: Find in Folder…");
        assert_eq!(app.search_view.include.text, "./lib/src", "a file: unchanged");
        assert!(app.message.as_ref().is_some_and(|m| m.text.contains("is a file")));
    }
}

mod rust_language_server {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::{Value, json};

    use crate::app::{App, Prompt};
    use crate::buffer::Buffer;
    use crate::buffer::pos::{self, ByteIdx, PosEncoding};
    use crate::diagnostics::Severity;
    use crate::lsp::fake::{FakeLauncher, FakeOptions};
    use crate::lsp::rpc::CONTENT_MODIFIED;
    use crate::lsp::{Launcher, ProcessLauncher, State};
    use crate::milestone9_tests::{REAL_SERVERS, children_named};
    use crate::selection::{Selection, Selections};
    use crate::syntax::Language;
    use crate::workspace::trust::{Trust, TrustStore};
    use crate::workspace::{self, Workspace};

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A workspace `<dir>/proj` with `files` (folders made as needed), its trust store in
    /// `<dir>/state`.
    fn project(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
        let root = workspace::normalize(dir).join("proj");
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        root
    }

    /// The app on `root` with servers started by `launcher`, no file open yet.
    fn open_app(root: &Path, launcher: Arc<dyn Launcher>) -> App {
        let trust = TrustStore::load(root.parent().unwrap().join("state/trust.toml"));
        let mut app = App::new(Workspace::load(root.to_path_buf()), trust, None, true);
        app.lsp.set_launcher(launcher);
        app.test_start();
        app.test_plain_whitespace();
        app
    }

    fn open(app: &mut App, path: &Path) {
        app.test_open(Buffer::open(path).unwrap());
    }

    fn encoding(app: &App) -> Option<PosEncoding> {
        app.lsp.server_for(Language::Rust).and_then(|s| s.encoding())
    }

    fn message(app: &App) -> Option<String> {
        app.message.as_ref().map(|m| m.text.clone())
    }

    fn text(app: &App) -> String {
        app.active_buffer().unwrap().rope.to_string()
    }

    fn put_cursor(app: &mut App, at: usize) {
        let (_, view) = app.active_mut().unwrap();
        view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
    }

    fn cursor_line(app: &App) -> usize {
        let buf = app.active_buffer().unwrap();
        let at = app.group().active_tab().unwrap().view.selections.primary().head;
        pos::line_of(&buf.rope, at)
    }

    fn active_path(app: &App) -> Option<PathBuf> {
        app.active_buffer().map(|b| b.path.clone())
    }

    fn format_document(app: &mut App) {
        if cfg!(target_os = "macos") {
            app.press(K::Char('f'), M::SHIFT | M::ALT);
        } else {
            app.press(K::Char('i'), M::CONTROL | M::SHIFT);
        }
    }

    fn show_hover(app: &mut App) {
        app.press(K::Char('i'), cmd() | M::ALT);
    }

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(120, 16)).unwrap()
    }

    fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    }

    fn status(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let y = buf.area.height - 1;
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// `settle`, for up to `secs` seconds.
    async fn settle_for(app: &mut App, secs: u64, done: impl Fn(&App) -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if app.settle(&done).await {
                return true;
            }
        }
        false
    }

    const MAIN: &str = "fn helper() -> i32 {\n    1\n}\n\nfn main() {\n    let _ = helper();\n}\n";

    /// A crate with `src/main.rs` open and its fake server (UTF-8) running.
    async fn fake_project(dir: &Path, workspace: Option<&str>) -> (App, Arc<FakeLauncher>, PathBuf) {
        let mut files = vec![("Cargo.toml", "[package]\nname = \"tiny\"\n"), ("src/main.rs", MAIN)];
        if let Some(ws) = workspace {
            files.push((".workspace", ws));
        }
        let root = project(dir, &files);
        let fake = FakeLauncher::new(FakeOptions {
            utf8: true,
            ..Default::default()
        });
        let mut app = open_app(&root, fake.clone());
        open(&mut app, &root.join("src/main.rs"));
        (app, fake, root)
    }

    /// A Rust file starts the built-in `rust-analyzer` (no arguments, no
    /// `initializationOptions`) with the workspace root as its root; the document goes out as
    /// `rust`. Starting is quiet.
    #[tokio::test(flavor = "multi_thread")]
    async fn rust_files_start_rust_analyzer_without_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, root) = fake_project(dir.path(), None).await;
        assert!(app.settle(|a| encoding(a).is_some()).await);
        assert_eq!(fake.launches(), ["rust-analyzer"]);
        let s = app.lsp.server_for(Language::Rust).unwrap();
        assert_eq!((s.config.command.as_str(), s.config.args.len()), ("rust-analyzer", 0));
        assert!(!s.config.from_workspace);
        let init = fake.first("initialize").unwrap();
        assert_eq!(init["params"]["rootUri"], crate::lsp::uri::from_path(&root).as_str());
        assert!(
            init["params"].get("initializationOptions").is_none_or(Value::is_null),
            "{init}"
        );
        assert!(app.settle(|_| fake.first("textDocument/didOpen").is_some()).await);
        let open = fake.first("textDocument/didOpen").unwrap();
        assert_eq!(open["params"]["textDocument"]["languageId"], "rust");
        assert_eq!(encoding(&app), Some(PosEncoding::Utf8));
        assert_eq!(message(&app), None, "starting is quiet");
        app.shutdown_servers().await;
    }

    /// `.workspace` `[lsp.rust]` replaces the built-in once the workspace is trusted (its
    /// `init_options` go out as `initializationOptions`); restricted, the built-in runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_lsp_rust_overrides_the_built_in() {
        let ws =
            "[lsp.rust]\ncommand = \"/opt/ra/rust-analyzer\"\ninit_options = { check = { command = \"clippy\" } }\n";
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _) = fake_project(dir.path(), Some(ws)).await;
        assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
        app.press(K::Char('t'), M::NONE);
        assert_eq!(app.trust, Trust::Trusted);
        assert!(app.settle(|a| encoding(a).is_some()).await);
        assert_eq!(fake.launches(), ["rust-analyzer"], "named after the command's file");
        let s = app.lsp.server_for(Language::Rust).unwrap();
        assert_eq!(s.config.command, "/opt/ra/rust-analyzer");
        assert!(s.config.from_workspace);
        let init = fake.first("initialize").unwrap();
        assert_eq!(
            init["params"]["initializationOptions"],
            json!({ "check": { "command": "clippy" } })
        );
        app.shutdown_servers().await;

        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _) = fake_project(dir.path(), Some(ws)).await;
        app.press(K::Esc, M::NONE);
        assert_eq!(app.trust, Trust::Restricted);
        assert!(app.settle(|a| encoding(a).is_some()).await);
        let s = app.lsp.server_for(Language::Rust).unwrap();
        assert_eq!(s.config.command, "rust-analyzer", "restricted: the built-in");
        assert!(
            fake.first("initialize").unwrap()["params"]
                .get("initializationOptions")
                .is_none_or(Value::is_null)
        );
        app.shutdown_servers().await;
    }

    fn progress(token: &str, value: Value) -> Value {
        json!({ "token": token, "value": value })
    }

    /// `window/workDoneProgress/create` is answered; `$/progress` begin / report / end show the
    /// latest unfinished work in the status bar after the problem counts ("rust-analyzer:
    /// Indexing 40% 3/25 (core)"), gone once it ends; a server that dies takes its progress along.
    #[tokio::test(flavor = "multi_thread")]
    async fn progress_shows_in_the_status_bar_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _) = fake_project(dir.path(), None).await;
        assert!(app.settle(|a| encoding(a).is_some()).await);
        // The fake asks during `initialize`; its answer is recorded when the fake reads it.
        assert!(
            app.settle(|_| fake.answers().get("progress") == Some(&Value::Null))
                .await,
            "create answered"
        );
        let mut term = terminal();
        let notify = |app: &App, params: Value| {
            app.lsp
                .server_for(Language::Rust)
                .unwrap()
                .notify("test/progress", params);
        };
        let shows = |app: &App, want: Option<&str>| {
            let want = want.map(String::from);
            app.lsp.progress() == want
        };

        notify(
            &app,
            progress(
                "rustAnalyzer/Indexing",
                json!({ "kind": "begin", "title": "Indexing", "percentage": 0, "cancellable": false }),
            ),
        );
        assert!(app.settle(|a| shows(a, Some("rust-analyzer: Indexing 0%"))).await);
        notify(
            &app,
            progress(
                "rustAnalyzer/Indexing",
                json!({ "kind": "report", "message": "3/25 (core)", "percentage": 40 }),
            ),
        );
        let indexing = "rust-analyzer: Indexing 40% 3/25 (core)";
        assert!(app.settle(|a| shows(a, Some(indexing))).await);
        draw(&mut term, &mut app);
        let bar = status(&term);
        let counts = bar.find("⊗ 0").expect("problem counts");
        let at = bar.find(indexing).unwrap_or_else(|| panic!("{bar}"));
        assert!(counts < at, "after the counts: {bar}");

        // A second piece of work shows while it runs, then the first again.
        notify(
            &app,
            progress(
                "rustAnalyzer/flycheck/0",
                json!({ "kind": "begin", "title": "cargo check" }),
            ),
        );
        assert!(app.settle(|a| shows(a, Some("rust-analyzer: cargo check"))).await);
        draw(&mut term, &mut app);
        assert!(status(&term).contains("rust-analyzer: cargo check"));
        notify(&app, progress("rustAnalyzer/flycheck/0", json!({ "kind": "end" })));
        assert!(app.settle(|a| shows(a, Some(indexing))).await);

        // A message shows after the progress.
        app.press(K::Char('s'), cmd());
        assert!(app.settle(|a| message(a).is_some_and(|m| m.starts_with("Saved"))).await);
        draw(&mut term, &mut app);
        let bar = status(&term);
        assert!(bar.find(indexing).unwrap() < bar.find("Saved").unwrap(), "{bar}");

        notify(&app, progress("rustAnalyzer/Indexing", json!({ "kind": "end" })));
        assert!(app.settle(|a| shows(a, None)).await);
        draw(&mut term, &mut app);
        assert!(!status(&term).contains("rust-analyzer:"), "{}", status(&term));

        // Unfinished work goes with a crashed server.
        notify(&app, progress("t", json!({ "kind": "begin", "title": "Loading" })));
        assert!(app.settle(|a| shows(a, Some("rust-analyzer: Loading"))).await);
        app.lsp
            .server_for(Language::Rust)
            .unwrap()
            .notify("test/crash", Value::Null);
        assert!(
            app.settle(|a| matches!(a.lsp.server_for(Language::Rust).unwrap().state, State::Failed))
                .await
        );
        assert_eq!(app.lsp.progress(), None);
        let s = app.lsp.server_for(Language::Rust).unwrap();
        assert_eq!(s.progress.latest(), None, "dropped, not just hidden");
        draw(&mut term, &mut app);
        assert!(!status(&term).contains("Loading"), "{}", status(&term));
        app.shutdown_servers().await;
    }

    /// Answers of ContentModified (-32801: rust-analyzer before its workspace is loaded, or
    /// mid-edit) are dropped quietly: no "not found", "failed" or "No suggestions." message.
    /// Other errors are still reported.
    #[tokio::test(flavor = "multi_thread")]
    async fn content_modified_answers_are_dropped_quietly() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _) = fake_project(dir.path(), None).await;
        assert!(app.settle(|a| encoding(a).is_some()).await);
        for method in [
            "textDocument/definition",
            "textDocument/references",
            "textDocument/hover",
            "textDocument/completion",
            "textDocument/formatting",
            "textDocument/prepareRename",
        ] {
            fake.fail(method, CONTENT_MODIFIED, "content modified");
        }
        put_cursor(&mut app, MAIN.rfind("helper").unwrap() + 1);
        // Each request's key (`None`: format document's, by platform).
        let keys = [
            ("textDocument/definition", Some((K::F(12), M::NONE))),
            ("textDocument/references", Some((K::F(12), M::SHIFT))),
            ("textDocument/hover", Some((K::Char('i'), cmd() | M::ALT))),
            ("textDocument/completion", Some((K::Char(' '), M::CONTROL))),
            ("textDocument/prepareRename", Some((K::F(2), M::NONE))),
            ("textDocument/formatting", None),
        ];
        for (method, key) in keys {
            match key {
                Some((key, mods)) => app.press(key, mods),
                None => format_document(&mut app),
            }
            assert!(
                app.settle(|a| fake.requests(method).len() == 1 && a.pending_requests().is_empty())
                    .await,
                "{method} answered"
            );
            app.pump();
            assert_eq!(message(&app), None, "{method}");
            assert!(app.prompt.is_none(), "{method}");
        }
        assert!(app.hover.popup.is_none());
        assert!(app.completion.ask.is_none());
        assert!(app.locations.is_none());

        // Any other error is still an error.
        fake.fail("textDocument/formatting", -32603, "rustfmt failed");
        format_document(&mut app);
        assert!(
            app.settle(|a| message(a).as_deref() == Some("Format Document failed: rustfmt failed"))
                .await,
            "{:?}",
            message(&app)
        );
        app.shutdown_servers().await;
    }

    /// A server that dies before answering `initialize` (a rustup proxy whose toolchain lacks the
    /// component) says why in the error message: the last non-blank line of its stderr, written
    /// after it closed its output.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_startup_crash_quotes_the_servers_last_words() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("src/main.rs", MAIN)]);
        let mut app = open_app(&root, Arc::new(ProcessLauncher));
        let script = "exec 1>&-; echo 'info: syncing' >&2; \
                      echo \"error: Unknown binary 'rust-analyzer' in official toolchain\" >&2; echo >&2; exit 1";
        app.lsp.builtins.rust_analyzer = crate::lsp::ServerConfig::new("sh", &["-c", script]);
        open(&mut app, &root.join("src/main.rs"));
        let failed = |a: &App| {
            a.lsp
                .server_for(Language::Rust)
                .is_some_and(|s| matches!(s.state, State::Failed))
        };
        assert!(app.settle(failed).await, "{:?}", message(&app));
        let text = message(&app).unwrap_or_default();
        assert!(
            text.contains("exited during initialize: error: Unknown binary 'rust-analyzer' in official toolchain"),
            "{text}"
        );
        assert!(!text.contains("syncing"), "only the last line: {text}");
        app.shutdown_servers().await;
    }

    // ---- the real rust-analyzer ----

    const CARGO_TOML: &str = "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
    const RA_MAIN: &str = "mod util;\n\nuse util::Point;\n\nfn   helper_fmt( )->i32{0}\n\nfn main() {\n    let pt = Point { xcoord: 1, ycoord: 2 };\n    let t = util::total(&pt) + helper_fmt();\n    let _ = t;\n}\n";
    const RA_MAIN_FORMATTED: &str = "mod util;\n\nuse util::Point;\n\nfn helper_fmt() -> i32 {\n    0\n}\n\nfn main() {\n    let pt = Point {\n        xcoord: 1,\n        ycoord: 2,\n    };\n    let t = util::total(&pt) + helper_fmt();\n    let _ = t;\n}\n";
    const RA_UTIL: &str = "/// A point.\npub struct Point {\n    pub xcoord: i32,\n    pub ycoord: i32,\n}\n\n/// Adds the coordinates.\npub fn total(p: &Point) -> i32 {\n    p.xcoord + p.ycoord\n}\n";

    fn rustc_errors(app: &App, path: &Path) -> Vec<String> {
        let Some(id) = app.buffers.find_path(path) else {
            return Vec::new();
        };
        let buf = app.buffers.get(id).unwrap();
        let text = buf.rope.to_string();
        buf.diagnostics
            .all()
            .iter()
            .filter(|m| m.diag.severity == Severity::Error && m.diag.source.as_deref() == Some("rustc"))
            .map(|m| text[m.start.0..m.end.0].to_string())
            .collect()
    }

    /// Cmd+Z until the active buffer is `want` again (at most `steps` times).
    fn undo_to(app: &mut App, want: &str, steps: usize) {
        for _ in 0..steps {
            if text(app) == want {
                return;
            }
            app.press(K::Char('z'), cmd());
        }
        assert_eq!(text(app), want, "undone");
    }

    async fn save(app: &mut App) {
        app.press(K::Char('s'), cmd());
        assert!(
            app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await,
            "saved; {:?}",
            message(app)
        );
    }

    /// The v1 features against the real `rust-analyzer` (the built-in command, run through the
    /// rustup proxy: the crate pins the repo's toolchain, which has the component) on a tiny
    /// crate with no dependencies: work-done progress while it loads, hover, go to definition,
    /// references, completion, `cargo check` errors on save (source `rustc`, pushed) and gone
    /// after the fix is saved, rename across files and format document (rustfmt). Requests
    /// answered before it's ready (nothing, ContentModified) are retried. Skipped with a note
    /// when `rust-analyzer` doesn't run there.
    #[tokio::test(flavor = "multi_thread")]
    async fn real_rust_analyzer_on_a_cargo_crate() {
        let _one_at_a_time = REAL_SERVERS.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let toolchain = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("rust-toolchain.toml")).unwrap();
        let root = project(
            dir.path(),
            &[
                ("Cargo.toml", CARGO_TOML),
                ("rust-toolchain.toml", &toolchain),
                ("src/main.rs", RA_MAIN),
                ("src/util.rs", RA_UTIL),
            ],
        );
        let runs = std::process::Command::new("rust-analyzer")
            .arg("--version")
            .current_dir(&root)
            .output()
            .is_ok_and(|o| o.status.success());
        if !runs {
            eprintln!(
                "real_rust_analyzer_on_a_cargo_crate: skipped, rust-analyzer doesn't run (`rustup component add rust-analyzer`)"
            );
            return;
        }
        let mut app = open_app(&root, Arc::new(ProcessLauncher));
        // Its own target directory: its `cargo check` never waits on (or for) another build.
        let target = workspace::normalize(dir.path()).join("target");
        app.lsp.builtins.rust_analyzer.init_options = Some(json!({ "cargo": { "targetDir": target } }));
        let main = root.join("src/main.rs");
        let util = root.join("src/util.rs");
        open(&mut app, &main);
        assert!(
            settle_for(&mut app, 60, |a| encoding(a).is_some()).await,
            "rust-analyzer initialized; message {:?}",
            message(&app)
        );
        assert_eq!(encoding(&app), Some(PosEncoding::Utf8));
        if let Some(n) = children_named("rust-analyzer") {
            assert_eq!(n, 1, "rust-analyzer running");
        }
        let begun = |a: &App| a.lsp.server_for(Language::Rust).is_some_and(|s| s.progress.begun() > 0);
        assert!(settle_for(&mut app, 120, begun).await, "rust-analyzer reports progress");

        // Hover (asked until it has loaded the crate).
        let at = RA_MAIN.find("total").unwrap() + 1;
        put_cursor(&mut app, at);
        let hover = "pub fn total(p: &Point) -> i32";
        let hovered = |a: &App| {
            a.hover
                .popup
                .as_ref()
                .is_some_and(|p| p.lines.iter().any(|l| l.text.contains(hover)))
        };
        let t = Instant::now();
        while !hovered(&app) && t.elapsed() < Duration::from_secs(180) {
            show_hover(&mut app);
            settle_for(&mut app, 3, hovered).await;
        }
        assert!(
            hovered(&app),
            "rust-analyzer hover: {:?}; message {:?}",
            app.hover.popup.as_ref().map(|p| &p.lines),
            message(&app)
        );
        app.press(K::Esc, M::NONE);

        // Go to definition (asked again while it answers nothing or ContentModified), and back.
        let went = |a: &App| active_path(a).as_deref() == Some(util.as_path());
        let t = Instant::now();
        while !went(&app) && t.elapsed() < Duration::from_secs(60) {
            put_cursor(&mut app, at);
            app.press(K::F(12), M::NONE);
            settle_for(&mut app, 3, went).await;
        }
        assert!(went(&app), "went to the definition; message {:?}", message(&app));
        assert_eq!(cursor_line(&app), 7);
        while active_path(&app).as_deref() != Some(main.as_path()) {
            app.press(K::PageUp, M::CONTROL);
        }

        // References: the call and the definition, in two files.
        let both = |a: &App| a.locations.as_ref().is_some_and(|l| l.files.len() == 2);
        let t = Instant::now();
        while !both(&app) && t.elapsed() < Duration::from_secs(60) {
            put_cursor(&mut app, at);
            app.press(K::F(12), M::SHIFT);
            settle_for(&mut app, 3, both).await;
            app.press(K::Char('1'), cmd());
        }
        assert!(
            both(&app),
            "references: {:?}; message {:?}",
            app.locations.as_ref().map(|l| &l.files),
            message(&app)
        );

        // Completion after `pt.`: the field, inserted with Enter.
        let after = RA_MAIN.find("(&pt").unwrap() + 4;
        let listed = |a: &App| {
            a.completion
                .popup
                .as_ref()
                .is_some_and(|p| p.shown.iter().any(|&i| p.items[i].label.trim() == "ycoord"))
        };
        let t = Instant::now();
        while !listed(&app) && t.elapsed() < Duration::from_secs(60) {
            app.press(K::Esc, M::NONE);
            undo_to(&mut app, RA_MAIN, 4);
            put_cursor(&mut app, after);
            app.type_text(".");
            settle_for(&mut app, 5, listed).await;
        }
        assert!(listed(&app), "completion; message {:?}", message(&app));
        let p = app.completion.popup.as_mut().unwrap();
        p.selected = p
            .shown
            .iter()
            .position(|&i| p.items[i].label.trim() == "ycoord")
            .unwrap();
        app.press(K::Enter, M::NONE);
        assert!(text(&app).contains("(&pt.ycoord)"), "{}", text(&app));
        undo_to(&mut app, RA_MAIN, 4);

        // `cargo check` on save: a type error, reported by rustc; gone once the fix is saved.
        let line = RA_MAIN.find("    let _ = t;").unwrap();
        put_cursor(&mut app, line);
        app.type_text("let bad: u32 = \"s\";\n    ");
        save(&mut app).await;
        assert!(
            settle_for(&mut app, 180, |a| rustc_errors(a, &main).iter().any(|e| e == "\"s\"")).await,
            "cargo check's error: {:?}; message {:?}",
            app.active_buffer().unwrap().diagnostics.all(),
            message(&app)
        );
        assert!(target.join("debug").is_dir(), "checked in its own target directory");
        // The check's progress ends (and the status bar clears) before the fix is saved: a save
        // during a check restarts it, which may keep its stale errors.
        assert!(
            settle_for(&mut app, 120, |a| a.lsp.progress().is_none()).await,
            "work done: {:?}",
            app.lsp.progress()
        );
        undo_to(&mut app, RA_MAIN, 4);
        save(&mut app).await;
        assert!(
            settle_for(&mut app, 180, |a| rustc_errors(a, &main).is_empty()).await,
            "cleared after the fix: {:?}; progress {:?}; message {:?}; text {:?}",
            rustc_errors(&app, &main),
            app.lsp.progress(),
            message(&app),
            text(&app)
        );

        // Rename across both files (asked again while it answers ContentModified, which is
        // dropped quietly: rust-analyzer re-analysing after the save).
        let renamed = |a: &App| {
            [&main, &util].iter().all(|p| {
                a.buffers.find_path(p).is_some_and(|id| {
                    let t = a.buffers.get(id).unwrap().rope.to_string();
                    t.contains("sum_of") && !t.contains("total")
                })
            })
        };
        let t = Instant::now();
        while !renamed(&app) && t.elapsed() < Duration::from_secs(90) {
            put_cursor(&mut app, at);
            app.press(K::F(2), M::NONE);
            if !settle_for(&mut app, 3, |a| a.prompt.is_some()).await {
                continue;
            }
            app.type_text("sum_of");
            app.press(K::Enter, M::NONE);
            settle_for(&mut app, 5, renamed).await;
        }
        assert!(renamed(&app), "renamed; message {:?}", message(&app));
        assert_eq!(active_path(&app).as_deref(), Some(main.as_path()));
        let renamed_text = text(&app);

        // Format document (rustfmt): one undo step.
        let want = RA_MAIN_FORMATTED.replace("total", "sum_of");
        let t = Instant::now();
        while text(&app) != want && t.elapsed() < Duration::from_secs(60) {
            format_document(&mut app);
            settle_for(&mut app, 5, |a| text(a) == want).await;
        }
        assert_eq!(text(&app), want, "formatted; message {:?}", message(&app));
        app.press(K::Char('z'), cmd());
        assert_eq!(text(&app), renamed_text, "one undo step");

        assert!(matches!(
            app.lsp.server_for(Language::Rust).unwrap().state,
            State::Running { .. }
        ));
        app.press(K::Char('q'), cmd());
        app.shutdown_servers().await;
        if let Some(n) = children_named("rust-analyzer") {
            assert_eq!(n, 0, "no rust-analyzer left");
        }
    }
}
