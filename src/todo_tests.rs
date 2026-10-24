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
//! - `revert_hunks`: a red Revert button (`diffEditor.revertAction`) on each change's first row
//!   of an unstaged diff (Index ↔ Working Tree) — the left column's right end side by side, left
//!   of Stage inline — and "Git: Revert Change at Cursor": an open file's buffer gets the index's
//!   lines back as `git checkout -p` would write them (dirty, disk and index untouched, one undo
//!   step each), refused when the buffer changed since the diff; a file not open is rewritten on
//!   disk (index untouched, not opened, its text before kept in the file history), refused when
//!   it changed; none on staged or Compare with Disk diffs, refused for an untracked file.
//! - `back_forward`: Go Back / Go Forward (Ctrl+- / Ctrl+Shift+-; Linux Ctrl+Alt+-) return to
//!   the places before jumps — quick open, go to line, Cmd+Home / End, switching tabs, F8, go to
//!   definition (here and in a file not open), a Search view result, a Problems / Locations list
//!   row, quick open's `:line` and `file:line` in the same file — with their selection and
//!   scroll, and forward again; a new jump drops the forward entries; a move of 10+ lines in one
//!   step is recorded (10 lines are, 9 aren't), edits aren't; edits before an entry (and a
//!   reload from disk) move it; a closed tab's file is reopened, a file deleted from disk
//!   skipped; a closed group's place opens in the active group; a diff tab counts as its file;
//!   at most 50 entries.
//! - `diff_syntax`: diff tabs draw both sides in the file's syntax colours (token foregrounds),
//!   side by side and inline, a block comment over several lines coloured where only its middle
//!   line changed, under the removed / inserted line and changed-part backgrounds; plain over
//!   `syntax_max_bytes` and for an unknown extension; `.workspace` associations pick the
//!   language; a comment opened above a scrolled view coloured; an edit of the open buffer
//!   re-coloured after the diff's reload.
//! - `indent_lines`: Tab with a selection over lines (or a whole line) indents every line it
//!   touches (empty ones skipped; one ending at a line's start doesn't take it), a cursor or
//!   part of a line still gets a tab; Shift+Tab outdents (also at a cursor), Cmd+] / Cmd+[
//!   (Linux Ctrl) indent / outdent whatever the selection; to the next / previous tab stop
//!   (fewer spaces than a level outdent to none); tabs from `insert_spaces = false` or a
//!   Makefile; selections keep their text and direction; several selections, each line once;
//!   one undo step each, none when nothing changes; refused on a read-only buffer; the
//!   line-command budget at 61k selections (release).
//! - `resize_pointer`: the mouse pointer is a resize arrow (OSC 22: `ew-resize` / `ns-resize`)
//!   over the side panel's edge, the bottom panel's top border and split dividers (by
//!   direction), kept while dragging one wherever the pointer goes, back to the default
//!   elsewhere and during a text drag.
//! - `diff_open_file`: "Git: Toggle Changes / File" switches between a file and its diff tab
//!   (the group's own, else its changes), each landing on the other's cursor line; the diff
//!   header's Open File button opens the file there, also when it isn't open yet.
//! - `first_reveal`: a list's selection revealed before it was ever drawn scrolls on its first
//!   draw, the least that shows it: Ctrl+Shift+G keeps "Changes" above the first file, the
//!   Explorer's start file far down a long folder lands on the bottom row, and the Search
//!   view's results, Problems, Locations (Shift+F12), the Tools view and Git History keep the
//!   row above the selected one in view.
//! - `cursor_style`: `cursor_style` (`line` default / `block` / `underline`) and
//!   `cursor_blinking` (default true): secondary cursors drawn underlined (in
//!   `editor.cursor.secondaryUnderline`) for `line` / `underline`, reversed for `block`, mid-line
//!   and at a line's end; a `.workspace` reload sends the new DECSCUSR once (the bytes enter /
//!   restore write: `terminal`'s unit tests); a task's own DECSCUSR stays in its terminal.
//! - `move_tabs`: "View: Move Editor into Next / Previous Group" (Ctrl+Cmd+Right / Left; Linux
//!   Ctrl+Alt+Right / Left) moves the active tab, its selections and scroll kept, into the
//!   neighbouring group in reading order (right of its active tab), which gets the focus (also
//!   from the explorer); with no next group into a new split to the right, a group's only tab
//!   with none stays put, no previous group does nothing; a group left empty closes; into a group showing the same buffer the tab there takes the
//!   moved view (no duplicate); diff tabs move too. "View: Move Editor Left / Right"
//!   (Cmd+Shift+PageUp / PageDown; Linux Ctrl) swaps it with its neighbour, no wrap, still
//!   active and shown in a full tab bar.
//! - `text_field_keys`: text fields take the editor's keys — Shift+Home then typing replaces the
//!   find query (matches follow), line start / end keys (Home / End; Cmd+Left / Right, Ctrl+A / E
//!   on macOS; Ctrl+Home / End on Linux), word deletes and Home / End in the Search view's query,
//!   Cmd+A / C / X in the find widget (`app.clipboard`; nothing copied without a selection; Linux
//!   Ctrl+V pastes), global and find-context `keybindings.toml` rebinds applying in the field (not
//!   the editor), global non-field commands still running from a field (Cmd+B, Cmd+Shift+P), the
//!   palette and a rename prompt's field, the selected range drawn selected (find widget and
//!   prompt), a long query scrolled to the cursor and a short one drawn from the input's start.
//! - `mru`: the command palette and quick open list the last 20 commands picked from the palette
//!   and the last 20 files opened first while their input is empty, most recent first, without
//!   duplicates (a file's walk row kept out by its recent row); a typed query keeps the fuzzy
//!   score; a key-bound command isn't recorded; both lists stop at 20; a failed open isn't
//!   recorded, and a file opened through a `..` spelling is normalized so it can't duplicate.
//! - `window_identity`: `.workspace` `[window] title` names the window `jed <title>` (the OSC 2
//!   title only; the user vars and OSC 7 keep the root) and `status_bar` tints the status bar
//!   from `Theme`'s `statusBar.<colour>` keys; each colour draws, live reload updates both
//!   (the title re-sent, not pushed) and a bad colour is reported with the last good kept.
//! - `term_mouse`: mouse events reach a shell or task tab's program as xterm's mouse input when
//!   it asked for them (clicks, drags, moves and the wheel as SGR bytes, the run's own cells), a
//!   left click still focusing it; with the mouse off nothing reaches the program and the wheel
//!   scrolls the scrollback, as it does while the view is scrolled back.

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
        app.press_file_end();
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
        app.press_file_end();
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "    ab  c d ");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn boundary_skips_single_spaces_between_words() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"boundary\"\n", "", TEXT);
        app.press_file_end();
        let term = draw(&mut app);
        assert_eq!(row(&app, &term, 0, 12), "→   ab··c d·");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn trailing_draws_only_whitespace_at_line_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _, _) = open_app(&dir, "render_whitespace = \"trailing\"\n", "", "\ta b  \n  \nx\n");
        app.press_file_end();
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
        app.press_file_end();
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
        // The secondary: an underlined cell (`cursor_style = "line"`), not in the dim colour
        // (under `block` it's reversed: the dim colour would be its background).
        let secondary = &cells(&app, &term, other, 2)[1];
        assert!(secondary.modifier.contains(Modifier::UNDERLINED));
        assert_ne!(secondary.fg, dim(&app), "the secondary cursor isn't dimmed");
        assert_eq!(secondary.sym, "·");

        // Unfocused, no terminal cursor marks the primary: its `·` is dimmed like the others.
        app.test_execute(Command::FocusExplorer);
        let term = draw(&mut app);
        assert_eq!(cells(&app, &term, line, 2)[1].fg, dim(&app));
        let secondary = &cells(&app, &term, other, 2)[1];
        assert!(secondary.modifier.contains(Modifier::UNDERLINED));
        assert_ne!(secondary.fg, dim(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn toggle_flips_between_none_and_the_setting_for_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let user = "render_whitespace = \"boundary\"\n";
        let (mut app, root, user_file) = open_app(&dir, user, "", TEXT);
        app.press_file_end();
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
        app.press_file_end();
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
        app.press_file_end();
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
        app.press_file_end();
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

pub(crate) mod side_scroll_bars {
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
    pub(crate) struct List {
        pub(crate) target: ScrollTarget,
        pub(crate) rows: fn(&App) -> usize,
        pub(crate) top: fn(&App) -> usize,
        pub(crate) selection: fn(&App) -> String,
    }

    /// The shared checks: the drawn thumb follows the list's scroll; hover, press-and-drag
    /// (off the bar too, past the end: the wheel's end), a press on the track, the wheel over
    /// the bar; the list's selection and the focus never change.
    pub(crate) fn drag_the_bar(app: &mut App, l: &List) {
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
        app.press_file_start();
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
        assert!(app.search_view.query.selection().is_some(), "typing replaces the query");
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

mod revert_hunks {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use crate::app::{App, InitialFile};
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::diff_view::Pair;
    use crate::git::test_repo::{commit_all, git, git_with_input, init};
    use crate::history;
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    const DOWN: Mouse = Mouse::Down(MouseButton::Left);

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A temp dir with `proj/` (the workspace, a repository with `f.txt` = `original`
    /// committed, then `changed` on disk) and `state/` (the trust store, outside the repo).
    fn project(original: &str, changed: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        init(&root);
        fs::write(root.join("f.txt"), original).unwrap();
        commit_all(&root, "one");
        fs::write(root.join("f.txt"), changed).unwrap();
        (dir, root, state)
    }

    fn numbered(n: usize) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    /// Lines 0..40 with line 2 changed, a line added after line 20 and line 35 deleted.
    fn three_changes() -> (String, String) {
        let original = numbered(40);
        let changed = original
            .replace("line 2\n", "two\n")
            .replace("line 20\n", "twenty\nmore\n")
            .replace("line 35\n", "");
        (original, changed)
    }

    /// The app on `root`, with `file` open in the editor if given.
    async fn open_app(root: &Path, state: &Path, file: Option<&str>) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let trust = TrustStore::load(state.join("trust.toml"));
        let initial = file.map(|f| InitialFile {
            buffer: Buffer::open(&root.join(f)).unwrap(),
            goto: None,
        });
        let mut app = App::new(ws, trust, initial, true);
        app.test_start();
        app.test_plain_whitespace();
        assert!(
            app.settle(|a| !a.git.running() && a.git.status.as_ref().is_some_and(|s| !s.entries.is_empty()))
                .await
        );
        app
    }

    /// Waits until every diff tab is loaded and current and the git status read.
    async fn settle_diffs(app: &mut App) -> bool {
        app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await
    }

    fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    fn diff_hit(app: &App) -> crate::layout::DiffHit {
        app.hits.group(app.groups.active).unwrap().diff.clone().unwrap()
    }

    fn revert_button(app: &App, k: usize) -> Rect {
        diff_hit(app)
            .reverts
            .iter()
            .find(|(_, h)| *h == k)
            .map(|(r, _)| *r)
            .expect("its Revert button")
    }

    fn cell_text(term: &Terminal<TestBackend>, r: Rect) -> String {
        let buf = term.backend().buffer();
        (r.x..r.x + r.width).map(|x| buf[(x, r.y)].symbol()).collect()
    }

    /// What `git checkout -p` leaves in a copy of the repository answering `answers`, and the
    /// copy's index.
    fn checkout_p(root: &Path, answers: &str) -> (String, String) {
        let to = root.with_file_name(format!("ref-{}", answers.replace('\n', "")));
        let ok = std::process::Command::new("cp")
            .arg("-a")
            .arg(root)
            .arg(&to)
            .status()
            .unwrap();
        assert!(ok.success());
        git_with_input(&to, &["checkout", "-p", "--", "f.txt"], Some(answers));
        (fs::read_to_string(to.join("f.txt")).unwrap(), index(&to))
    }

    fn index(root: &Path) -> String {
        git(root, &["ls-files", "-s"]) + &git(root, &["diff", "--cached"])
    }

    fn buffer_text(app: &App) -> String {
        app.buffers.iter().next().unwrap().1.rope.to_string()
    }

    fn message(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revert_hunks_in_an_open_buffer() {
        let (original, changed) = three_changes();
        let (_d, root, state) = project(&original, &changed);
        let (want, want_index) = checkout_p(&root, "n\ny\nn\n");
        let index_before = index(&root);
        assert_eq!(want_index, index_before);
        let mut app = open_app(&root, &state, Some("f.txt")).await;
        app.test_execute(Command::OpenChange);
        assert!(settle_diffs(&mut app).await);

        // Side by side: Revert at the right end of the left (index) column, on each change's
        // first row, red; Stage at the right end of the row.
        let term = draw(&mut app, 150, 60);
        let hit = diff_hit(&app);
        assert_eq!(hit.reverts.len(), 3);
        assert_eq!(hit.buttons.len(), 3);
        let middle = hit.rows.x + hit.rows.width / 2;
        for ((r, k), (s, sk)) in hit.reverts.iter().zip(&hit.buttons) {
            assert_eq!((k, r.y), (sk, s.y), "the same row as Stage");
            assert_eq!(cell_text(&term, *r), " Revert ");
            assert!(
                r.x + r.width <= middle + 1 && r.x + r.width >= middle - 1,
                "{r:?} {middle}"
            );
            assert!(s.x > middle);
            let cell = &term.backend().buffer()[(r.x + 1, r.y)];
            let style = app.theme.get("diffEditor.revertAction");
            assert_eq!((Some(cell.fg), Some(cell.bg)), (style.fg, style.bg));
        }

        // Its middle change: the buffer, as `git checkout -p` leaves the file; nothing else.
        let r = revert_button(&app, 1);
        app.mouse(DOWN, r.x + 1, r.y);
        assert_eq!(buffer_text(&app), want);
        assert_eq!(message(&app), "Reverted the change at line 21 of f.txt (not saved)");
        let buf = app.buffers.iter().next().unwrap().1;
        assert!(buf.is_dirty(), "not saved");
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            changed,
            "disk untouched"
        );
        assert_eq!(index(&root), index_before, "index untouched");
        assert!(
            app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 2)
                .await,
            "the diff follows the buffer"
        );
        assert!(app.group().active_diff().is_some(), "the diff tab stays active");

        // The command, at the cursor (the next change: the deletion), in the one-column layout.
        app.press(K::F(7), M::NONE);
        let k = app.group().active_diff().unwrap().hunk_at_cursor();
        assert_eq!(k, Some(1));
        draw(&mut app, 80, 60);
        assert!(app.group().active_diff().unwrap().inline);
        let hit = diff_hit(&app);
        let (r, s) = (
            revert_button(&app, 1),
            hit.buttons.iter().find(|(_, h)| *h == 1).unwrap().0,
        );
        assert!(r.y == s.y && r.x + r.width < s.x, "inline: left of Stage");
        app.test_execute(Command::RevertHunk);
        assert_eq!(buffer_text(&app), want.replace("line 34\n", "line 34\nline 35\n"));

        // One undo step each, in the file's text tab.
        app.press(K::PageUp, M::CONTROL);
        assert!(app.group().active_tab().is_some());
        app.press(K::Char('z'), cmd());
        assert_eq!(buffer_text(&app), want);
        app.press(K::Char('z'), cmd());
        assert_eq!(buffer_text(&app), changed);
        assert!(!app.buffers.iter().next().unwrap().1.is_dirty());

        // Edited since the diff was taken: refused until it reads the buffer again.
        app.press(K::PageDown, M::CONTROL);
        assert!(settle_diffs(&mut app).await);
        app.press(K::F(7), M::NONE);
        assert!(app.group().active_diff().unwrap().hunk_at_cursor().is_some());
        app.press(K::PageUp, M::CONTROL);
        app.press_file_start();
        app.type_text("x");
        app.press(K::PageDown, M::CONTROL);
        assert!(app.group().active_diff().is_some());
        app.test_execute(Command::RevertHunk);
        assert_eq!(
            message(&app),
            "Can't revert the change in f.txt: the file changed, try again"
        );
        assert_eq!(buffer_text(&app), format!("x{changed}"));
        assert!(
            app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 4)
                .await
        );
        app.press_file_start();
        app.test_execute(Command::RevertHunk);
        assert_eq!(buffer_text(&app), changed, "line 0 back");
        assert_eq!(message(&app), "Reverted the change at line 1 of f.txt (not saved)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn revert_hunks_of_a_file_not_open() {
        let (original, changed) = three_changes();
        let (_d, root, state) = project(&original, &changed);
        let (want, _) = checkout_p(&root, "y\nn\nn\n");
        let index_before = index(&root);
        let mut app = open_app(&root, &state, None).await;
        app.open_diff(root.join("f.txt"), Pair::UNSTAGED);
        assert!(settle_diffs(&mut app).await);

        // The file moved on since the diff was read: refused, nothing written.
        let moved_on = changed.replace("line 10\n", "ten\n");
        fs::write(root.join("f.txt"), &moved_on).unwrap();
        draw(&mut app, 150, 60);
        let r = revert_button(&app, 0);
        app.mouse(DOWN, r.x + 1, r.y);
        assert!(app.settle(|a| a.diffs.staging == 0 && a.message.is_some()).await);
        assert_eq!(
            message(&app),
            "Can't revert the change in f.txt: the file changed, try again"
        );
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), moved_on);
        fs::write(root.join("f.txt"), &changed).unwrap();
        assert!(
            app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 3)
                .await
        );

        // The first change: the file on disk as `git checkout -p` leaves it, the index as it was,
        // not opened; the text before kept in the file history.
        draw(&mut app, 150, 60);
        let r = revert_button(&app, 0);
        app.mouse(DOWN, r.x + 1, r.y);
        assert!(
            app.settle(|a| a.diffs.staging == 0 && a.file_history.running() == 0)
                .await
        );
        assert_eq!(message(&app), "Reverted the change at line 3 of f.txt");
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), want);
        assert_eq!(index(&root), index_before);
        assert_eq!(app.buffers.iter().count(), 0, "not opened");
        let entries = history::entries(&root, Path::new("f.txt")).unwrap();
        assert_eq!(entries.len(), 1);
        let kept = history::read(&root, Path::new("f.txt"), &entries[0].id).unwrap();
        assert_eq!(String::from_utf8(kept).unwrap(), changed);
        assert!(
            app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 2)
                .await,
            "the diff reads the file again"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_revert_on_other_diffs_or_untracked_files() {
        let (original, changed) = three_changes();
        let (_d, root, state) = project(&original, &changed);
        git(&root, &["add", "f.txt"]);
        fs::write(root.join("new.txt"), "new\n").unwrap();
        let mut app = open_app(&root, &state, Some("f.txt")).await;

        // Staged changes (HEAD ↔ Index): Unstage buttons, no Revert; the command says why.
        app.open_diff(root.join("f.txt"), Pair::STAGED);
        assert!(settle_diffs(&mut app).await);
        let term = draw(&mut app, 150, 60);
        let hit = diff_hit(&app);
        assert_eq!(hit.buttons.len(), 3);
        assert!(hit.reverts.is_empty());
        let screen: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(!screen.contains("Revert"));
        app.test_execute(Command::RevertHunk);
        assert!(message(&app).starts_with("Only working tree changes can be reverted"));

        // The buffer against the disk (Compare with Disk): none either.
        app.test_execute(Command::CompareWithSaved);
        assert!(settle_diffs(&mut app).await);
        draw(&mut app, 150, 60);
        assert!(diff_hit(&app).reverts.is_empty());

        // An untracked file: nothing in the index to go back to.
        app.open_diff(root.join("new.txt"), Pair::UNSTAGED);
        assert!(settle_diffs(&mut app).await);
        draw(&mut app, 150, 60);
        let r = revert_button(&app, 0);
        app.mouse(DOWN, r.x + 1, r.y);
        assert_eq!(
            message(&app),
            "Can't revert the change in new.txt: it isn't in the index"
        );
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "new\n");

        // In a text tab: where to do it.
        app.press(K::PageUp, M::CONTROL);
        while app.group().active_tab().is_none() {
            app.press(K::PageUp, M::CONTROL);
        }
        app.test_execute(Command::RevertHunk);
        assert!(message(&app).contains("Git: Open Changes"));
        assert_eq!(buffer_text(&app), changed);
    }

    /// The text of the open buffer of `path`.
    fn text_of(app: &App, path: &Path) -> String {
        app.buffers.get(id_of(app, path)).unwrap().rope.to_string()
    }

    fn id_of(app: &App, path: &Path) -> crate::buffer::BufferId {
        app.buffers.iter().find(|(_, b)| b.path == path).unwrap().0
    }

    /// Open buffers that can't be reverted: read-only, untracked (its whole text would go),
    /// with merge conflicts. Each is left as it was, with a message.
    #[tokio::test(flavor = "multi_thread")]
    async fn no_revert_in_read_only_untracked_or_conflicted_buffers() {
        let (original, changed) = three_changes();
        let (_d, root, state) = project(&original, &changed);
        fs::write(root.join("new.txt"), "new\n").unwrap();
        // `c.txt` unmerged: conflict stages in the index, markers on disk.
        const MARKERS: &str = "<<<<<<<\nours\n=======\ntheirs\n>>>>>>>\n";
        fs::write(root.join("c.txt"), MARKERS).unwrap();
        let blob = |text: &str| git_with_input(&root, &["hash-object", "-w", "--stdin"], Some(text));
        let (base, ours, theirs) = (blob("base\n"), blob("ours\n"), blob("theirs\n"));
        let info = format!(
            "100644 {} 1\tc.txt\n100644 {} 2\tc.txt\n100644 {} 3\tc.txt\n",
            base.trim(),
            ours.trim(),
            theirs.trim()
        );
        git_with_input(&root, &["update-index", "--index-info"], Some(&info));
        let mut app = open_app(&root, &state, Some("f.txt")).await;
        app.test_open(Buffer::open(&root.join("new.txt")).unwrap());
        app.test_open(Buffer::open(&root.join("c.txt")).unwrap());
        assert!(
            app.settle(|a| a.git.status.as_ref().is_some_and(|s| s.entries.len() == 3))
                .await
        );

        // Read-only: refused with its reason.
        let f = root.join("f.txt");
        let id = id_of(&app, &f);
        app.buffers.get_mut(id).unwrap().read_only = Some("locked".into());
        app.open_diff(f.clone(), Pair::UNSTAGED);
        assert!(settle_diffs(&mut app).await);
        app.test_execute(Command::RevertHunk);
        assert_eq!(message(&app), "Read-only: locked");
        assert_eq!(text_of(&app, &f), changed);
        assert!(!app.buffers.get(id).unwrap().is_dirty());

        // Untracked and conflicted, though open: their buttons refuse.
        for (name, why, text) in [
            ("new.txt", "it isn't in the index", "new\n"),
            ("c.txt", "it has merge conflicts", MARKERS),
        ] {
            let path = root.join(name);
            app.open_diff(path.clone(), Pair::UNSTAGED);
            assert!(settle_diffs(&mut app).await);
            draw(&mut app, 150, 60);
            let r = revert_button(&app, 0);
            app.mouse(DOWN, r.x + 1, r.y);
            assert_eq!(message(&app), format!("Can't revert the change in {name}: {why}"));
            assert_eq!(text_of(&app, &path), text, "{name}");
        }
    }
}

mod back_forward {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::{Value, json};

    use crate::app::{App, InitialFile};
    use crate::buffer::Buffer;
    use crate::buffer::pos;
    use crate::lsp::fake::{FakeLauncher, FakeOptions};
    use crate::navigation::MAX;
    use crate::syntax::Language;
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn back(app: &mut App) {
        if cfg!(target_os = "macos") {
            app.press(K::Char('-'), M::CONTROL);
        } else {
            app.press(K::Char('-'), M::CONTROL | M::ALT);
        }
    }

    /// As a kitty-protocol terminal with alternate keys reports Ctrl+Shift+-.
    fn forward(app: &mut App) {
        app.press(K::Char('_'), M::CONTROL | M::SHIFT);
    }

    fn numbered(name: &str, lines: usize) -> String {
        (1..=lines).map(|i| format!("{name} {i}\n")).collect()
    }

    /// A workspace `<dir>/proj` with `files`, its trust store in `<dir>/state`.
    fn project(dir: &Path, files: &[(&str, String)]) -> PathBuf {
        let root = workspace::normalize(dir).join("proj");
        fs::create_dir_all(&root).unwrap();
        for (name, text) in files {
            fs::write(root.join(name), text).unwrap();
        }
        root
    }

    fn trust(root: &Path) -> TrustStore {
        TrustStore::load(root.parent().unwrap().join("state/trust.toml"))
    }

    /// The app on `root` with `first` open, drawn once.
    fn open_app(root: &Path, first: &str) -> App {
        let initial = InitialFile {
            buffer: Buffer::open(&root.join(first)).unwrap(),
            goto: None,
        };
        let mut app = App::new(Workspace::load(root.to_path_buf()), trust(root), Some(initial), true);
        app.test_start();
        draw(&mut app);
        app
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// The active editor: its file's name, the primary head's 0-based line and byte column.
    fn place(app: &App) -> (String, usize, usize) {
        let buf = app.active_buffer().expect("a text tab");
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        let line = pos::line_of(&buf.rope, head);
        (buf.name(), line, head.0 - pos::line_start(&buf.rope, line).0)
    }

    fn at(name: &str, line: usize) -> (String, usize, usize) {
        (name.to_string(), line, 0)
    }

    fn top(app: &App) -> usize {
        app.group().active_tab().unwrap().view.top_line()
    }

    /// The history's entries as (file name, line of the head): `usize::MAX` for a diff tab's or
    /// a file not open.
    fn entries(app: &App) -> Vec<(String, usize)> {
        app.nav
            .history
            .entries()
            .iter()
            .map(|e| {
                let name = e.path.file_name().unwrap().to_string_lossy().into_owned();
                let line = match (e.pos, app.buffers.find_path(&e.path)) {
                    (Some(p), Some(id)) => pos::line_of(&app.buffers.get(id).unwrap().rope, p.head),
                    _ => usize::MAX,
                };
                (name, line)
            })
            .collect()
    }

    fn e(name: &str, line: usize) -> (String, usize) {
        (name.to_string(), line)
    }

    /// Ctrl+G, the 1-based line, Enter.
    fn go_to_line(app: &mut App, line: usize) {
        app.press(K::Char('g'), M::CONTROL);
        app.type_text(&line.to_string());
        app.press(K::Enter, M::NONE);
    }

    /// Cmd+P, the file's name, Enter once it's listed; waits until it's the active tab.
    async fn quick_open(app: &mut App, name: &str) {
        app.press(K::Char('p'), cmd());
        app.type_text(name);
        let listed = app
            .settle(|a| {
                a.palette
                    .as_ref()
                    .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
            })
            .await;
        assert!(listed, "quick open lists {name}");
        app.press(K::Enter, M::NONE);
        let opened = app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == name))
            .await;
        assert!(opened, "{name} opened");
    }

    fn has_tab(app: &App, name: &str) -> bool {
        app.group().tabs.iter().any(|t| {
            t.buffer()
                .and_then(|b| app.buffers.get(b))
                .is_some_and(|b| b.name() == name)
        })
    }

    fn is_active(app: &App, name: &str) -> bool {
        app.active_buffer().is_some_and(|b| b.name() == name)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quick_open_and_go_to_line_go_back_and_forward_with_selection_and_scroll() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(
            dir.path(),
            &[("a.txt", numbered("a", 100)), ("b.txt", numbered("b", 100))],
        );
        let mut app = open_app(&root, "a.txt");
        go_to_line(&mut app, 50);
        draw(&mut app);
        let top_a = top(&app);
        assert!(top_a > 0, "scrolled to line 50");
        assert_eq!(place(&app), at("a.txt", 49));
        quick_open(&mut app, "b.txt").await;
        assert_eq!(entries(&app), [e("a.txt", 0), e("a.txt", 49)]);

        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 49));
        assert_eq!(top(&app), top_a, "the scroll comes back");
        assert_eq!(app.ui.focus, crate::app::Focus::Editor);
        back(&mut app);
        assert_eq!((place(&app), top(&app)), (at("a.txt", 0), 0));
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 0), "nothing further back");

        forward(&mut app);
        assert_eq!((place(&app), top(&app)), (at("a.txt", 49), top_a));
        forward(&mut app);
        assert_eq!(place(&app), at("b.txt", 0));
        forward(&mut app);
        assert_eq!(place(&app), at("b.txt", 0), "nothing further forward");
        assert_eq!(app.group().tabs.len(), 2, "no tab opened twice");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_jump_after_going_back_drops_the_forward_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 100))]);
        let mut app = open_app(&root, "a.txt");
        go_to_line(&mut app, 30);
        go_to_line(&mut app, 60);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 29));
        go_to_line(&mut app, 80);
        assert_eq!(entries(&app), [e("a.txt", 0), e("a.txt", 29)], "60 dropped");
        forward(&mut app);
        assert_eq!(place(&app), at("a.txt", 79), "nothing forward");
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 29));
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 0));
        forward(&mut app);
        forward(&mut app);
        assert_eq!(place(&app), at("a.txt", 79));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn moves_of_ten_lines_or_more_in_one_step_are_recorded_shorter_ones_and_edits_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 200))]);
        let mut app = open_app(&root, "a.txt");
        for _ in 0..12 {
            app.press(K::Down, M::NONE);
        }
        assert_eq!(place(&app), at("a.txt", 12));
        assert!(entries(&app).is_empty(), "{:?}", entries(&app));
        app.press(K::PageDown, M::NONE);
        let paged = place(&app).1;
        assert!(paged >= 22, "a page down: {paged}");
        assert_eq!(entries(&app), [e("a.txt", 12)]);
        // A click far away (scrolled down with the wheel first) is a move too.
        draw(&mut app);
        let text = app.hits.groups[0].text;
        for _ in 0..5 {
            app.mouse(MouseEventKind::ScrollDown, text.x, text.y);
        }
        assert!(entries(&app).len() == 1, "scrolling isn't a move");
        draw(&mut app);
        let y = text.y + text.height - 1;
        app.mouse(MouseEventKind::Down(MouseButton::Left), text.x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), text.x, y);
        let clicked = place(&app).1;
        assert!(clicked >= paged + 10, "{clicked}");
        assert_eq!(entries(&app), [e("a.txt", 12), e("a.txt", paged)]);
        back(&mut app);
        assert_eq!(place(&app).1, paged);
        forward(&mut app);
        assert_eq!(place(&app).1, clicked);

        // Edits moving the cursor far in one step (a 12-line paste, its undo) record nothing:
        // checked while live (after a jump), where a record would append.
        go_to_line(&mut app, 150);
        let (before, index) = (entries(&app), app.nav.history.index());
        assert_eq!(index, before.len(), "live");
        app.handle_test_event(crossterm::event::Event::Paste("x\n".repeat(12)));
        assert_eq!(place(&app).1, 161);
        app.press(K::Char('z'), cmd());
        assert_eq!(place(&app).1, 149);
        assert_eq!((entries(&app), app.nav.history.index()), (before, index));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_of_nine_lines_is_not_recorded_one_of_ten_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 100))]);
        let mut app = open_app(&root, "a.txt");
        let text = app.hits.groups[0].text;
        let click = |app: &mut App, row: u16| {
            app.mouse(MouseEventKind::Down(MouseButton::Left), text.x, text.y + row);
            app.mouse(MouseEventKind::Up(MouseButton::Left), text.x, text.y + row);
        };
        click(&mut app, 9);
        assert_eq!(place(&app), at("a.txt", 9));
        assert!(entries(&app).is_empty(), "{:?}", entries(&app));
        click(&mut app, 19);
        assert_eq!(place(&app), at("a.txt", 19));
        assert_eq!(entries(&app), [e("a.txt", 9)]);
        // Up again by exactly ten.
        click(&mut app, 9);
        assert_eq!(entries(&app), [e("a.txt", 9), e("a.txt", 19)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quick_open_go_to_line_and_a_file_position_in_the_same_file_are_jumps() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 30))]);
        let mut app = open_app(&root, "a.txt");
        // `:3` in quick open: two lines down.
        app.press(K::Char('p'), cmd());
        app.type_text(":3");
        app.press(K::Enter, M::NONE);
        assert_eq!(place(&app), at("a.txt", 2));
        assert_eq!(entries(&app), [e("a.txt", 0)]);
        // `a.txt:5`, the file already open and active.
        app.press(K::Char('p'), cmd());
        app.type_text("a.txt:5");
        let listed = app
            .settle(|a| {
                a.palette
                    .as_ref()
                    .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == "a.txt"))
            })
            .await;
        let rows = app
            .palette
            .as_ref()
            .map(|p| p.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>());
        assert!(listed, "quick open lists a.txt: {rows:?}");
        app.press(K::Enter, M::NONE);
        assert_eq!(place(&app), at("a.txt", 4));
        assert_eq!(entries(&app), [e("a.txt", 0), e("a.txt", 2)]);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 2));
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_reload_from_disk_moves_the_entries_of_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 100))]);
        let mut app = open_app(&root, "a.txt");
        go_to_line(&mut app, 30);
        go_to_line(&mut app, 60);
        assert_eq!(entries(&app), [e("a.txt", 0), e("a.txt", 29)]);
        // Five lines added at the top on disk; the clean buffer reloads.
        fs::write(
            root.join("a.txt"),
            format!("{}{}", numbered("new", 5), numbered("a", 100)),
        )
        .unwrap();
        let reloaded = app
            .settle(|a| a.active_buffer().unwrap().rope.to_string().starts_with("new 1\n"))
            .await;
        assert!(reloaded);
        assert_eq!(entries(&app), [e("a.txt", 0), e("a.txt", 34)]);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 34));
        let buf = app.active_buffer().unwrap();
        assert_eq!(
            buf.rope
                .slice(pos::line_start(&buf.rope, 34).0..)
                .to_string()
                .lines()
                .next(),
            Some("a 30")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cmd_home_and_end_are_jumps_even_in_a_short_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 4))]);
        let mut app = open_app(&root, "a.txt");
        app.press(K::Down, M::NONE);
        app.press_file_end();
        assert_eq!(place(&app), at("a.txt", 4));
        app.press_file_start();
        assert_eq!(entries(&app), [e("a.txt", 1), e("a.txt", 4)]);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 4));
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn switching_tabs_records_the_place_left() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(
            dir.path(),
            &[("a.txt", numbered("a", 50)), ("b.txt", numbered("b", 50))],
        );
        let mut app = open_app(&root, "a.txt");
        quick_open(&mut app, "b.txt").await;
        go_to_line(&mut app, 7);
        app.press(K::PageUp, M::CONTROL);
        assert_eq!(place(&app), at("a.txt", 0));
        assert_eq!(entries(&app), [e("a.txt", 0), e("b.txt", 0), e("b.txt", 6)]);
        back(&mut app);
        assert_eq!(place(&app), at("b.txt", 6));
        back(&mut app);
        assert_eq!(place(&app), at("b.txt", 0));
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 0));
        forward(&mut app);
        forward(&mut app);
        assert_eq!(place(&app), at("b.txt", 6));
        forward(&mut app);
        assert_eq!(place(&app), at("a.txt", 0));
        assert_eq!(app.group().tabs.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn edits_before_an_entry_move_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 100))]);
        let mut app = open_app(&root, "a.txt");
        go_to_line(&mut app, 50);
        app.press(K::Right, M::NONE);
        app.press(K::Right, M::NONE);
        go_to_line(&mut app, 1);
        // Three lines typed above line 50: two here, one in another view of the buffer (a
        // split, which records the place left in the first group, at line 2).
        app.type_text("new\n\n");
        app.press(K::Char('\\'), cmd());
        app.press(K::Enter, M::NONE);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 2));
        back(&mut app);
        assert_eq!(place(&app), ("a.txt".to_string(), 52, 2));
        let buf = app.active_buffer().unwrap();
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        assert!(buf.rope.slice(head.0..).to_string().starts_with("50\n"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn going_back_to_a_closed_tab_reopens_its_file_at_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(
            dir.path(),
            &[("a.txt", numbered("a", 100)), ("b.txt", numbered("b", 100))],
        );
        let mut app = open_app(&root, "a.txt");
        quick_open(&mut app, "b.txt").await;
        go_to_line(&mut app, 70);
        draw(&mut app);
        let top_b = top(&app);
        app.press(K::Char('w'), cmd());
        assert_eq!(place(&app), at("a.txt", 0));
        assert!(!has_tab(&app, "b.txt") && app.buffers.find_path(&root.join("b.txt")).is_none());

        back(&mut app);
        assert!(app.settle(|a| is_active(a, "b.txt")).await);
        assert_eq!((place(&app), top(&app)), (at("b.txt", 69), top_b));
        assert_eq!(app.ui.focus, crate::app::Focus::Editor);
        // Its arrival is no jump of its own.
        assert_eq!(
            entries(&app),
            [e("a.txt", 0), e("b.txt", 0), e("b.txt", 69), e("a.txt", 0)]
        );
        forward(&mut app);
        assert_eq!(place(&app), at("a.txt", 0));
        back(&mut app);
        assert_eq!(place(&app), at("b.txt", 69));
        assert_eq!(app.group().tabs.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_deleted_from_disk_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            ("a.txt", numbered("a", 50)),
            ("b.txt", numbered("b", 50)),
            ("c.txt", numbered("c", 50)),
        ];
        let root = project(dir.path(), &files);
        let mut app = open_app(&root, "a.txt");
        quick_open(&mut app, "b.txt").await;
        go_to_line(&mut app, 20);
        quick_open(&mut app, "c.txt").await;
        // b's tab closed with a middle click (c stays active), then b deleted.
        draw(&mut app);
        let t = app.hits.groups[0].tabs.iter().find(|t| t.index == 1).copied().unwrap();
        let y = app.hits.groups[0].tab_bar.y;
        app.mouse(MouseEventKind::Down(MouseButton::Middle), t.x0 + 1, y);
        assert!(!has_tab(&app, "b.txt"));
        assert_eq!(place(&app), at("c.txt", 0));
        fs::remove_file(root.join("b.txt")).unwrap();
        assert_eq!(entries(&app).len(), 3);

        back(&mut app);
        assert!(app.settle(|a| is_active(a, "a.txt")).await);
        assert_eq!(place(&app), at("a.txt", 0));
        assert!(
            app.buffers.find_path(&root.join("b.txt")).is_none(),
            "no empty buffer for it"
        );
        assert!(!has_tab(&app, "b.txt"));
        assert_eq!(entries(&app), [e("a.txt", 0), e("c.txt", 0)]);
        forward(&mut app);
        assert_eq!(place(&app), at("c.txt", 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_diff_tab_counts_as_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(
            dir.path(),
            &[("a.txt", numbered("a", 10)), ("b.txt", numbered("b", 10))],
        );
        let mut app = open_app(&root, "a.txt");
        // Switching between a file and its diff tab is no jump.
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text("File: Compare with Disk");
        app.press(K::Enter, M::NONE);
        assert!(app.group().active_diff().is_some());
        app.press(K::PageUp, M::CONTROL);
        app.press(K::PageDown, M::CONTROL);
        assert!(app.group().active_diff().is_some());
        assert!(entries(&app).is_empty(), "{:?}", entries(&app));
        quick_open(&mut app, "b.txt").await;
        assert_eq!(entries(&app), [e("a.txt", usize::MAX)]);
        back(&mut app);
        let diff = app.group().active_diff().expect("the diff tab");
        assert_eq!(diff.path, root.join("a.txt"));
        forward(&mut app);
        assert_eq!(place(&app), at("b.txt", 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_place_in_a_closed_group_opens_in_the_active_group() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 100))]);
        let mut app = open_app(&root, "a.txt");
        app.press(K::Char('\\'), cmd());
        let second = app.groups.active;
        go_to_line(&mut app, 30);
        // Closing the second group's only tab closes the group.
        app.press(K::Char('w'), cmd());
        assert_eq!(app.groups.len(), 1);
        assert_ne!(app.groups.active, second);
        assert_eq!(place(&app), at("a.txt", 0));
        back(&mut app);
        assert_eq!(app.groups.len(), 1);
        assert_eq!(place(&app), at("a.txt", 29));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_history_keeps_the_last_fifty_places() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.txt", numbered("a", 200))]);
        let mut app = open_app(&root, "a.txt");
        for i in 1..=60 {
            go_to_line(&mut app, i * 3);
        }
        assert_eq!(entries(&app).len(), MAX);
        // The oldest kept: the place before the 11th jump (line 30).
        assert_eq!(entries(&app)[0], e("a.txt", 29));
        let mut steps = 0;
        loop {
            let before = place(&app);
            back(&mut app);
            if place(&app) == before {
                break;
            }
            steps += 1;
        }
        // Going back stored the current place too, dropping the oldest once more.
        assert_eq!(steps, MAX - 1);
        assert_eq!(place(&app), at("a.txt", 32));
    }

    /// A C++ project's app with the fake server, `a.cpp` open.
    async fn lsp_app(root: &Path) -> (App, std::sync::Arc<FakeLauncher>) {
        let fake = FakeLauncher::new(FakeOptions {
            utf8: true,
            ..Default::default()
        });
        let mut app = App::new(Workspace::load(root.to_path_buf()), trust(root), None, true);
        app.lsp.set_launcher(fake.clone());
        app.test_start();
        app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
        draw(&mut app);
        let ready = app
            .settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await;
        assert!(ready, "the fake server started");
        (app, fake)
    }

    fn uri(path: &Path) -> String {
        crate::lsp::uri::from_path(path).as_str().to_string()
    }

    fn range(line: u32, from: u32, to: u32) -> Value {
        json!({ "start": { "line": line, "character": from }, "end": { "line": line, "character": to } })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn f8_and_go_to_definition_go_back_and_forward() {
        let dir = tempfile::tempdir().unwrap();
        let a_text = format!(
            "int target = 1;\nint use = target;\nint x = 0;\n{}",
            numbered("// a", 40)
        );
        let root = project(dir.path(), &[("a.cpp", a_text), ("b.cpp", numbered("// b", 40))]);
        let (mut app, fake) = lsp_app(&root).await;
        let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));

        // F8 to a problem two lines down: a jump, though a short one.
        let params = json!({
            "uri": uri(&a),
            "diagnostics": [{ "range": range(2, 4, 5), "severity": 1, "message": "bad x" }],
        });
        app.lsp
            .server_for(Language::Cpp)
            .unwrap()
            .notify("test/publish", params);
        assert!(
            app.settle(|a| a.active_buffer().unwrap().diagnostics.all().len() == 1)
                .await
        );
        app.press(K::F(8), M::NONE);
        assert_eq!(place(&app), ("a.cpp".to_string(), 2, 4));
        back(&mut app);
        assert_eq!(place(&app), at("a.cpp", 0));
        forward(&mut app);
        assert_eq!(place(&app), ("a.cpp".to_string(), 2, 4));

        // Go to definition in the same file.
        app.press(K::Up, M::NONE);
        app.press(K::End, M::NONE);
        let use_at = place(&app);
        assert_eq!(use_at.1, 1);
        fake.respond(
            "textDocument/definition",
            json!({ "uri": uri(&a), "range": range(0, 4, 10) }),
        );
        app.press(K::F(12), M::NONE);
        assert!(app.settle(|a| place(a) == ("a.cpp".to_string(), 0, 4)).await);
        back(&mut app);
        assert_eq!(place(&app), use_at);

        // In a file not open.
        fake.respond(
            "textDocument/definition",
            json!({ "uri": uri(&b), "range": range(30, 3, 4) }),
        );
        app.press(K::F(12), M::NONE);
        assert!(app.settle(|a| a.active_buffer().is_some_and(|x| x.path == b)).await);
        assert_eq!(place(&app), ("b.cpp".to_string(), 30, 3));
        back(&mut app);
        assert_eq!(place(&app), use_at);
        forward(&mut app);
        assert_eq!(place(&app), ("b.cpp".to_string(), 30, 3));
        app.shutdown_servers().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opening_a_problem_or_a_reference_from_their_lists_is_a_jump() {
        let dir = tempfile::tempdir().unwrap();
        let a_text = format!(
            "int target = 1;\nint use = target;\nint x = 0;\n{}",
            numbered("// a", 40)
        );
        let root = project(dir.path(), &[("a.cpp", a_text)]);
        let (mut app, fake) = lsp_app(&root).await;
        let a = root.join("a.cpp");
        let params = json!({
            "uri": uri(&a),
            "diagnostics": [{ "range": range(2, 4, 5), "severity": 1, "message": "bad x" }],
        });
        app.lsp
            .server_for(Language::Cpp)
            .unwrap()
            .notify("test/publish", params);
        assert!(
            app.settle(|a| a.active_buffer().unwrap().diagnostics.all().len() == 1)
                .await
        );
        // The Problems list (Cmd+Shift+M, its file row selected): Enter opens the first problem.
        app.press(K::Down, M::NONE);
        app.press(K::Char('m'), cmd() | M::SHIFT);
        assert_eq!(app.ui.focus, crate::app::Focus::Panel);
        app.press(K::Enter, M::NONE);
        assert_eq!(place(&app), ("a.cpp".to_string(), 2, 5));
        assert_eq!(entries(&app), [e("a.cpp", 1)]);
        back(&mut app);
        assert_eq!(place(&app), at("a.cpp", 1));

        // The Locations list of references: Enter opens the selected one.
        app.press(K::End, M::NONE);
        let use_at = place(&app);
        fake.respond(
            "textDocument/references",
            json!([
                { "uri": uri(&a), "range": range(0, 4, 10) },
                { "uri": uri(&a), "range": range(1, 10, 16) },
            ]),
        );
        app.press(K::F(12), M::SHIFT);
        assert!(app.settle(|a| a.locations.is_some()).await);
        assert_eq!(app.ui.focus, crate::app::Focus::Panel);
        app.press(K::Home, M::NONE);
        app.press(K::Down, M::NONE);
        app.press(K::Enter, M::NONE);
        assert_eq!(place(&app), ("a.cpp".to_string(), 0, 4));
        back(&mut app);
        assert_eq!(place(&app), use_at);
        app.shutdown_servers().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opening_a_search_result_goes_back_to_where_it_was_opened_from() {
        let dir = tempfile::tempdir().unwrap();
        let mut b_text = numbered("b", 60);
        b_text.push_str("the needle\n");
        let root = project(dir.path(), &[("a.txt", numbered("a", 60)), ("b.txt", b_text)]);
        let mut app = open_app(&root, "a.txt");
        go_to_line(&mut app, 5);
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("needle");
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.total == 1))
                .await
        );
        app.press(K::F(4), M::NONE);
        assert!(app.settle(|a| is_active(a, "b.txt")).await);
        assert_eq!(place(&app).1, 60);
        back(&mut app);
        assert_eq!(place(&app), at("a.txt", 4));
        assert_eq!(app.ui.focus, crate::app::Focus::Editor);
        forward(&mut app);
        assert_eq!(place(&app).1, 60);
    }
}

mod diff_syntax {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use crate::app::{App, InitialFile};
    use crate::buffer::Buffer;
    use crate::diff_view::Pair;
    use crate::git::test_repo::{commit_all, init};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    const ORIGINAL: &str = "fn one() {}\n/* alpha\nbeta\ngamma */\nfn main() {}\n";
    const CHANGED: &str = "fn two() {}\n/* alpha\nBETA\ngamma */\nfn main() {}\n";

    /// A temp dir with `proj/` (a repository with `file` = `ORIGINAL` committed, `CHANGED` on
    /// disk, and `workspace` as `.workspace` when given) and `state/` (outside the repo).
    fn project(file: &str, workspace: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf) {
        project_with(file, workspace, ORIGINAL, CHANGED)
    }

    /// `project` with `original` committed and `changed` on disk.
    fn project_with(
        file: &str,
        workspace: Option<&str>,
        original: &str,
        changed: &str,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        init(&root);
        if let Some(w) = workspace {
            fs::write(root.join(".workspace"), w).unwrap();
        }
        fs::write(root.join(file), original).unwrap();
        commit_all(&root, "one");
        fs::write(root.join(file), changed).unwrap();
        (dir, root, state)
    }

    /// The app on `root` (with `file` open if given), its unstaged diff of `diffed` open and
    /// loaded.
    async fn open_diff(root: &Path, state: &Path, file: Option<&str>, diffed: &str) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let trust = TrustStore::load(state.join("trust.toml"));
        let initial = file.map(|f| InitialFile {
            buffer: Buffer::open(&root.join(f)).unwrap(),
            goto: None,
        });
        let mut app = App::new(ws, trust, initial, true);
        app.test_start();
        app.test_plain_whitespace();
        assert!(
            app.settle(|a| !a.git.running() && a.git.status.as_ref().is_some_and(|s| !s.entries.is_empty()))
                .await
        );
        app.open_diff(root.join(diffed), Pair::UNSTAGED);
        assert!(settle_diffs(&mut app).await);
        app
    }

    async fn settle_diffs(app: &mut App) -> bool {
        app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await
    }

    fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// Where `needle` starts on screen: exactly one place.
    fn at(term: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        let found = every(term, needle);
        assert_eq!(found.len(), 1, "{needle:?} at {found:?}");
        found[0]
    }

    /// Every place `needle` starts on screen.
    fn every(term: &Terminal<TestBackend>, needle: &str) -> Vec<(u16, u16)> {
        let found = every_or_none(term, needle);
        assert!(!found.is_empty(), "{needle:?} not drawn");
        found
    }

    /// Every place `needle` starts on screen, maybe none.
    fn every_or_none(term: &Terminal<TestBackend>, needle: &str) -> Vec<(u16, u16)> {
        let buf = term.backend().buffer();
        let mut found = Vec::new();
        for y in 0..buf.area.height {
            let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            let n: Vec<String> = needle.chars().map(String::from).collect();
            for x in 0..row.len().saturating_sub(n.len() - 1) {
                if row[x..x + n.len()].iter().zip(&n).all(|(a, b)| a == b) {
                    found.push((x as u16, y));
                }
            }
        }
        found
    }

    fn fg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Color {
        term.backend().buffer()[(x, y)].fg
    }

    fn bg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Color {
        term.backend().buffer()[(x, y)].bg
    }

    fn key_fg(app: &App, key: &str) -> Color {
        app.theme.get(key).fg.unwrap()
    }

    fn key_bg(app: &App, key: &str) -> Color {
        app.theme.get(key).bg.unwrap()
    }

    fn right(at: (u16, u16), n: u16) -> (u16, u16) {
        (at.0 + n, at.1)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn both_sides_coloured_side_by_side_and_inline_under_the_diff_backgrounds() {
        let (_d, root, state) = project("a.rs", None);
        let mut app = open_diff(&root, &state, None, "a.rs").await;
        let (keyword, comment, function) = (
            key_fg(&app, "keyword"),
            key_fg(&app, "comment"),
            key_fg(&app, "function"),
        );
        let (removed, inserted) = (
            key_bg(&app, "diffEditor.removedLineBackground"),
            key_bg(&app, "diffEditor.insertedLineBackground"),
        );
        let (removed_text, inserted_text) = (
            key_bg(&app, "diffEditor.removedTextBackground"),
            key_bg(&app, "diffEditor.insertedTextBackground"),
        );
        let editor = key_bg(&app, "editor");
        for (w, inline) in [(150, false), (80, true)] {
            let term = draw(&mut app, w, 24);
            assert_eq!(app.group().active_diff().unwrap().inline, inline);
            let (old, new) = (at(&term, "fn one"), at(&term, "fn two"));
            if inline {
                assert_eq!(old.0, new.0);
                assert!(old.1 < new.1, "the old line above the new one");
            } else {
                assert!(old.0 < new.0, "the old line on the left");
            }
            // The keyword in its colour on the line's background; the changed part (the name)
            // in its token colour on the changed-text background.
            assert_eq!((fg(&term, old), bg(&term, old)), (keyword, removed), "inline {inline}");
            assert_eq!((fg(&term, new), bg(&term, new)), (keyword, inserted), "inline {inline}");
            // (Inline draws no changed parts: the line's background.)
            let (removed_text, inserted_text) = if inline {
                (removed, inserted)
            } else {
                (removed_text, inserted_text)
            };
            let (name_old, name_new) = (right(old, 3), right(new, 3));
            assert_eq!((fg(&term, name_old), bg(&term, name_old)), (function, removed_text));
            assert_eq!((fg(&term, name_new), bg(&term, name_new)), (function, inserted_text));
            // A comment's middle line changed alone is still a comment: both sides parsed whole.
            let (beta, big) = (at(&term, "beta"), at(&term, "BETA"));
            assert_eq!((fg(&term, beta), bg(&term, beta)), (comment, removed_text));
            assert_eq!((fg(&term, big), bg(&term, big)), (comment, inserted_text));
            // Unchanged lines (on both sides, or once inline): token colours on the editor's
            // background.
            let shown = if inline { 1 } else { 2 };
            let gamma = every(&term, "gamma");
            assert_eq!(gamma.len(), shown);
            assert!(gamma.iter().all(|&c| fg(&term, c) == comment));
            let main = every(&term, "fn main");
            assert_eq!(main.len(), shown);
            for c in main {
                assert_eq!((fg(&term, c), bg(&term, c)), (keyword, editor), "inline {inline}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plain_over_the_size_limit() {
        let (_d, root, state) = project("a.rs", Some("[settings]\nsyntax_max_bytes = 20\n"));
        let mut app = open_diff(&root, &state, None, "a.rs").await;
        assert_eq!(app.settings.syntax_max_bytes, 20);
        let term = draw(&mut app, 150, 24);
        let plain = key_fg(&app, "editor");
        for text in ["fn one", "fn two", "beta", "BETA", "fn main"] {
            for c in every(&term, text) {
                assert_eq!(fg(&term, c), plain, "{text}");
            }
        }
        assert_eq!(
            bg(&term, at(&term, "fn one")),
            key_bg(&app, "diffEditor.removedLineBackground")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plain_for_an_unknown_extension() {
        let (_d, root, state) = project("a.zzz", None);
        let mut app = open_diff(&root, &state, None, "a.zzz").await;
        let term = draw(&mut app, 150, 24);
        let plain = key_fg(&app, "editor");
        for text in ["fn one", "fn two", "beta", "BETA", "fn main"] {
            for c in every(&term, text) {
                assert_eq!(fg(&term, c), plain, "{text}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_edit_of_the_open_buffer_is_recoloured_after_the_reload() {
        let (_d, root, state) = project("a.rs", None);
        let mut app = open_diff(&root, &state, Some("a.rs"), "a.rs").await;
        let term = draw(&mut app, 150, 24);
        assert_eq!(fg(&term, at(&term, "fn two")), key_fg(&app, "keyword"));
        // Comment out the changed line in the buffer, unsaved.
        app.press(K::PageUp, M::CONTROL);
        assert_eq!(app.active_buffer().unwrap().path, root.join("a.rs"));
        app.type_text("// ");
        app.pump();
        assert!(app.diffs.deadline().is_some());
        assert!(settle_diffs(&mut app).await);
        app.press(K::PageDown, M::CONTROL);
        assert!(app.group().active_diff().is_some());
        let term = draw(&mut app, 150, 24);
        let new = at(&term, "// fn two");
        assert_eq!(fg(&term, right(new, 3)), key_fg(&app, "comment"));
        // The left side (the index) is unchanged.
        assert_eq!(fg(&term, at(&term, "fn one")), key_fg(&app, "keyword"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_associations_pick_the_language() {
        let ws = "[files]\nassociations = { \"*.zzz\" = \"rust\" }\n";
        let (_d, root, state) = project("a.zzz", Some(ws));
        let mut app = open_diff(&root, &state, None, "a.zzz").await;
        let term = draw(&mut app, 150, 24);
        let (keyword, comment) = (key_fg(&app, "keyword"), key_fg(&app, "comment"));
        assert_eq!(fg(&term, at(&term, "fn one")), keyword);
        assert_eq!(fg(&term, at(&term, "BETA")), comment);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_comment_opened_above_a_scrolled_view_is_coloured() {
        // Line 1 opens a comment closed 40 lines on; only the line after it changes.
        let comment_lines: String = (0..40).map(|i| format!("c{i:02}\n")).collect();
        let text = |name: &str| format!("fn f() {{}}\n/*\n{comment_lines}*/\nfn {name}() {{}}\nfn main() {{}}\n");
        let (_d, root, state) = project_with("a.rs", None, &text("g"), &text("h"));
        let mut app = open_diff(&root, &state, None, "a.rs").await;
        let (keyword, comment, editor) = (key_fg(&app, "keyword"), key_fg(&app, "comment"), key_fg(&app, "editor"));
        for (w, inline) in [(150, false), (80, true)] {
            draw(&mut app, w, 40);
            let d = app.group_mut().active_diff_mut().unwrap();
            assert_eq!(d.inline, inline);
            // Scrolled to the end: the last page, starting inside the comment.
            d.top = 0;
            d.scroll_by(1000);
            assert!((4..20).contains(&d.top), "top {} inline {inline}", d.top);
            let term = draw(&mut app, w, 40);
            // Neither the comment's opening line nor the code before it is on screen.
            assert!(every_or_none(&term, "/*").is_empty() && every_or_none(&term, "fn f").is_empty());
            let shown = if inline { 1 } else { 2 };
            for line in ["c20", "c30", "c39"] {
                let cells = every(&term, line);
                assert_eq!(cells.len(), shown, "{line}");
                assert!(cells.iter().all(|&c| fg(&term, c) == comment), "{line} inline {inline}");
            }
            // The digits of the line numbers stay plain gutter text, not comment-coloured.
            let (x, y) = every(&term, "c20")[0];
            assert_ne!(fg(&term, (x - 2, y)), comment);
            // After the comment: code again, on both sides.
            for c in every(&term, "fn main") {
                assert_eq!(fg(&term, c), keyword, "inline {inline}");
            }
            assert_eq!(fg(&term, at(&term, "fn g")), keyword);
            assert_eq!(fg(&term, at(&term, "fn h")), keyword);
            assert_ne!(keyword, editor);
        }
    }
}

mod indent_lines {
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::buffer::pos::ByteIdx;
    use crate::milestone_tests::test_app;
    use crate::pairs::tests::show;
    use crate::selection::{Selection, Selections};
    use crate::syntax::Language;

    /// A line command at 60k selections plus a frame, as milestone 6's line commands.
    const BUDGET: Duration = Duration::from_millis(80);
    /// The same in debug builds: only catches going quadratic.
    const DEBUG_BOUND: Duration = Duration::from_secs(5);

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// The app on `marked`'s text (`|` cursors, `«…»` selections, as `pairs::tests::setup`).
    fn open(dir: &tempfile::TempDir, marked: &str) -> App {
        let (mut text, mut list, mut anchor) = (String::new(), Vec::new(), None);
        for c in marked.chars() {
            match c {
                '|' => list.push(Selection::cursor(ByteIdx(text.len()))),
                '«' => anchor = Some(text.len()),
                '»' => list.push(Selection::new(ByteIdx(anchor.take().unwrap()), ByteIdx(text.len()))),
                c => text.push(c),
            }
        }
        let mut app = test_app(dir.path(), Buffer::from_str(&text));
        app.active_mut().unwrap().1.selections = Selections::new(list, 0);
        app
    }

    fn shown(app: &mut App) -> String {
        let (buf, view) = app.active_mut().unwrap();
        show(buf, view)
    }

    fn tab(app: &mut App) {
        app.press(K::Tab, M::NONE);
    }

    /// Shift+Tab as terminals report it.
    fn shift_tab(app: &mut App) {
        app.press(K::BackTab, M::SHIFT);
    }

    fn indent(app: &mut App) {
        app.press(K::Char(']'), cmd());
    }

    fn outdent(app: &mut App) {
        app.press(K::Char('['), cmd());
    }

    fn undo(app: &mut App) {
        app.press(K::Char('z'), cmd());
    }

    #[test]
    fn tab_over_lines_indents_every_touched_line_but_empty_ones() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "fn f() {\n«a();\n\n  b();\nc»();\n}\n");
        tab(&mut app);
        assert_eq!(shown(&mut app), "fn f() {\n«    a();\n\n    b();\n    c»();\n}\n");
        tab(&mut app);
        assert_eq!(
            shown(&mut app),
            "fn f() {\n«        a();\n\n        b();\n        c»();\n}\n"
        );
        // A selection ending at a line's start doesn't take that line.
        let mut app = open(&dir, "«a\nb\n»c\n");
        tab(&mut app);
        assert_eq!(shown(&mut app), "«    a\n    b\n»c\n");
        // A whole single line is indented too (VS Code).
        let mut app = open(&dir, "«ab»\ncd");
        tab(&mut app);
        assert_eq!(shown(&mut app), "«    ab»\ncd");
    }

    #[test]
    fn tab_at_a_cursor_or_part_of_a_line_still_inserts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "ab|c\n«de»f");
        tab(&mut app);
        assert_eq!(shown(&mut app), "ab  |c\n    |f");
        // Typing after it joins its undo step (an insert, as before).
        app.type_text("x");
        undo(&mut app);
        assert_eq!(shown(&mut app), "ab|c\n«de»f");
    }

    #[test]
    fn shift_tab_outdents_also_at_an_empty_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "        a|b\n");
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "    a|b\n");
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "a|b\n");
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "a|b\n", "nothing left to remove");
        // That last one made no undo step: two undos are back at the start.
        undo(&mut app);
        undo(&mut app);
        assert_eq!(shown(&mut app), "        a|b\n");
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    #[test]
    fn cmd_brackets_indent_and_outdent_regardless_of_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "x = |1\n");
        indent(&mut app);
        assert_eq!(shown(&mut app), "    x = |1\n");
        indent(&mut app);
        assert_eq!(shown(&mut app), "        x = |1\n");
        outdent(&mut app);
        assert_eq!(shown(&mut app), "    x = |1\n");
        // Part of a line: its line, the selection kept.
        let mut app = open(&dir, "a«b»c\n");
        indent(&mut app);
        assert_eq!(shown(&mut app), "    a«b»c\n");
        outdent(&mut app);
        assert_eq!(shown(&mut app), "a«b»c\n");
        // A cursor alone on an empty line indents it.
        let mut app = open(&dir, "a\n|\nb");
        indent(&mut app);
        assert_eq!(shown(&mut app), "a\n    |\nb");
    }

    #[test]
    fn partial_levels_go_to_the_next_or_previous_stop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "«  a\n      b\n   c»");
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "«a\n    b\nc»");
        let mut app = open(&dir, "«  a\n      b»");
        indent(&mut app);
        assert_eq!(shown(&mut app), "«    a\n        b»");
    }

    #[test]
    fn tabs_when_the_settings_or_the_language_say_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".workspace"), "[settings]\ninsert_spaces = false\n").unwrap();
        let mut app = open(&dir, "«a\n\tb»");
        assert!(!app.settings.insert_spaces);
        tab(&mut app);
        assert_eq!(shown(&mut app), "«\ta\n\t\tb»");
        shift_tab(&mut app);
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "«a\nb»");

        // A Makefile indents with tabs whatever the settings.
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "«all:\ncc»");
        assert!(app.settings.insert_spaces);
        app.active_mut().unwrap().0.syntax.set_language(Language::Makefile);
        indent(&mut app);
        assert_eq!(shown(&mut app), "«\tall:\n\tcc»");
        // Spaces there are normalised to tabs.
        let mut app = open(&dir, "        |x");
        app.active_mut().unwrap().0.syntax.set_language(Language::Makefile);
        outdent(&mut app);
        assert_eq!(shown(&mut app), "\t|x");
    }

    #[test]
    fn selections_keep_their_text_and_direction() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "  «ab\ncd»\n");
        tab(&mut app);
        assert_eq!(
            shown(&mut app),
            "  «  ab\n    cd»\n",
            "a start in the indentation stays put"
        );
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "«ab\ncd»\n");
        let mut app = open(&dir, "|ab\ncd\n");
        app.active_mut().unwrap().1.selections = Selections::single(Selection::new(ByteIdx(4), ByteIdx(1)));
        indent(&mut app);
        let s = *app.group().active_tab().unwrap().view.selections.primary();
        assert_eq!((s.anchor.0, s.head.0), (12, 5), "backwards, on the same text");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "    ab\n    cd\n");
    }

    #[test]
    fn every_selection_shifts_its_own_lines_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "a|b|\n  c\n|d\n\n«e\nf»");
        indent(&mut app);
        assert_eq!(shown(&mut app), "    a|b|\n  c\n    |d\n\n«    e\n    f»");
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "a|b|\n  c\n|d\n\n«e\nf»");
        // Tab: lines for the selection over lines, a tab at the cursor, in one step.
        let mut app = open(&dir, "x|y\n«a\nb»");
        tab(&mut app);
        assert_eq!(shown(&mut app), "x   |y\n«    a\n    b»");
        undo(&mut app);
        assert_eq!(shown(&mut app), "x|y\n«a\nb»");
    }

    #[test]
    fn one_undo_step_restores_text_and_selections() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "«a\n  b»\nc|\n");
        indent(&mut app);
        indent(&mut app);
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "«    a\n    b»\n    c|\n");
        undo(&mut app);
        assert_eq!(shown(&mut app), "«        a\n        b»\n        c|\n");
        undo(&mut app);
        assert_eq!(shown(&mut app), "«    a\n    b»\n    c|\n");
        undo(&mut app);
        assert_eq!(shown(&mut app), "«a\n  b»\nc|\n");
        assert!(!app.active_buffer().unwrap().is_dirty());
        app.press(K::Char('z'), cmd() | M::SHIFT);
        assert_eq!(shown(&mut app), "«    a\n    b»\n    c|\n");
    }

    #[test]
    fn shift_tab_in_the_find_widget_is_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "    a|b\n");
        app.test_execute(crate::command::Command::OpenReplace);
        assert_eq!(app.ui.focus, crate::app::Focus::Find);
        let field = app.find.field;
        shift_tab(&mut app);
        assert_ne!(app.find.field, field, "Shift+Tab switches the find widget's input");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "    ab\n");
    }

    #[test]
    fn a_read_only_buffer_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(&dir, "«a\nb»");
        app.active_mut().unwrap().0.read_only = Some("test".into());
        tab(&mut app);
        indent(&mut app);
        outdent(&mut app);
        shift_tab(&mut app);
        assert_eq!(shown(&mut app), "«a\nb»");
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    /// Indent, outdent and undo at the 1 MB file's 61k selections (31k lines), Tab and
    /// Shift+Tab at 31k selections over lines, each with a frame, within milestone 6's line-command budget (release only).
    #[test]
    fn milestone_indent_lines_budget_at_61k_selections() {
        let mut original = String::new();
        let mut i = 0;
        while original.len() < 1_000_000 {
            original.push_str(&format!("let item = value_{i}; // item\n\n"));
            i += 1;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path(), Buffer::from_str(&original));
        app.active_mut().unwrap().1.selections = Selections::single(Selection::cursor(ByteIdx(5)));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let draw = |term: &mut Terminal<TestBackend>, app: &mut App| {
            term.draw(|f| crate::ui::render(f, app)).unwrap();
        };
        draw(&mut term, &mut app);
        app.press(K::Char('l'), cmd() | M::SHIFT);
        let count = app.group().active_tab().unwrap().view.selections.len();
        assert_eq!(count, 2 * i);
        let text = |app: &App| app.active_buffer().unwrap().rope.to_string();
        let mut timed = |app: &mut App, what: &str, f: &dyn Fn(&mut App)| {
            let n = app.group().active_tab().unwrap().view.selections.len();
            let t = Instant::now();
            f(app);
            draw(&mut term, app);
            let took = t.elapsed();
            println!("{what} at {n} selections: {took:?}");
            assert!(took < DEBUG_BOUND, "{what} took {took:?}");
            if !cfg!(debug_assertions) {
                assert!(took < BUDGET, "{what} too slow: {took:?}");
            }
        };
        timed(&mut app, "indent lines", &indent);
        assert!(text(&app).starts_with("    let item = value_0; // item\n\n    let item = value_1;"));
        timed(&mut app, "outdent lines", &outdent);
        assert_eq!(text(&app), original);
        timed(&mut app, "undo", &undo);
        assert!(text(&app).starts_with("    let item"));
        undo(&mut app);
        assert_eq!(text(&app), original);
        // Each line's selections reach down onto the blank line: Tab indents the lines.
        app.press(K::Down, M::SHIFT);
        timed(&mut app, "tab over lines", &tab);
        assert!(text(&app).starts_with("    let item = value_0; // item\n\n    let item = value_1;"));
        timed(&mut app, "shift+tab", &shift_tab);
        assert_eq!(text(&app), original);
    }
}

mod resize_pointer {
    use crossterm::event::{MouseButton, MouseEventKind as K};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::layout::{Divider, SplitDir};
    use crate::milestone_tests::test_app;
    use crate::terminal::Pointer;

    fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    }

    fn divider(app: &App, dir: SplitDir) -> Divider {
        app.hits.dividers.iter().find(|d| d.dir == dir).unwrap().clone()
    }

    #[test]
    fn resize_pointer_over_borders_and_while_dragging() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(dir.path(), Buffer::from_str("hello\n"));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        app.ui.side_visible = true;
        app.ui.panel_visible = true;
        app.test_execute(Command::SplitRight);
        app.test_execute(Command::SplitDown);
        draw(&mut app, &mut term);

        let side = app.hits.regions.side.unwrap();
        let panel = app.hits.regions.panel.unwrap();
        let right = divider(&app, SplitDir::Right).rect;
        let down = divider(&app, SplitDir::Down).rect;
        let border = side.x + side.width - 1;
        for (x, y, want) in [
            (border, side.y + 3, Pointer::ColResize),
            (panel.x + 10, panel.y, Pointer::RowResize),
            (right.x, right.y + 2, Pointer::ColResize),
            (down.x + 2, down.y, Pointer::RowResize),
            (panel.x + 10, panel.y + 2, Pointer::Default),
            (right.x + 5, down.y + 3, Pointer::Default),
        ] {
            app.mouse(K::Moved, x, y);
            assert_eq!(app.pointer, want, "at ({x}, {y})");
        }

        // A drag keeps its arrow wherever the pointer goes, and drops it on release away from
        // the border.
        app.mouse(K::Down(MouseButton::Left), border, side.y + 3);
        app.mouse(K::Drag(MouseButton::Left), border + 15, side.y + 10);
        assert_eq!(app.pointer, Pointer::ColResize);
        app.mouse(K::Up(MouseButton::Left), border + 15, side.y + 10);
        draw(&mut app, &mut term);
        assert_eq!(app.hits.regions.side.unwrap().width, side.width + 15);
        app.mouse(K::Moved, border + 15, side.y + 10);
        assert_eq!(app.pointer, Pointer::ColResize, "the border followed the pointer");
        app.mouse(K::Moved, border + 20, side.y + 10);
        assert_eq!(app.pointer, Pointer::Default);

        // A text drag in the editor never shows one, even crossing a divider.
        let text = app.hits.group(app.groups.active).unwrap().text;
        let right = divider(&app, SplitDir::Right).rect;
        app.mouse(K::Down(MouseButton::Left), text.x + 1, text.y);
        app.mouse(K::Drag(MouseButton::Left), right.x, right.y + 2);
        assert_eq!(app.pointer, Pointer::Default);
        app.mouse(K::Up(MouseButton::Left), right.x, right.y + 2);
    }
}

mod diff_open_file {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, MouseButton, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, InitialFile};
    use crate::buffer::Buffer;
    use crate::buffer::pos;
    use crate::command::Command;
    use crate::diff_view::Pair;
    use crate::git::test_repo::{commit_all, init};
    use crate::milestone10_tests::dirs;
    use crate::workspace::Workspace;
    use crate::workspace::trust::TrustStore;

    /// `proj/a.txt` (20 lines) committed, then lines 5 and 15 changed on disk.
    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (dir, root, state) = dirs();
        init(&root);
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        fs::write(root.join("a.txt"), lines.join("\n") + "\n").unwrap();
        commit_all(&root, "one");
        let mut changed = lines.clone();
        changed[5] = "five".into();
        changed[15] = "fifteen".into();
        fs::write(root.join("a.txt"), changed.join("\n") + "\n").unwrap();
        (dir, root, state)
    }

    async fn app(root: &Path, state: &Path, open: bool) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let trust = TrustStore::load(state.join("trust.toml"));
        let initial = open.then(|| InitialFile {
            buffer: Buffer::open(&root.join("a.txt")).unwrap(),
            goto: None,
        });
        let mut app = App::new(ws, trust, initial, true);
        app.test_start();
        app.test_plain_whitespace();
        assert!(
            app.settle(|a| !a.git.running() && a.git.status.as_ref().is_some_and(|s| !s.entries.is_empty()))
                .await
        );
        app
    }

    /// The active text tab's cursor line.
    fn file_line(app: &App) -> usize {
        let buf = app.active_buffer().expect("a text tab");
        pos::line_of(
            &buf.rope,
            app.group().active_tab().unwrap().view.selections.primary().head,
        )
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn diff_open_file_toggles_between_a_file_and_its_diff_at_the_same_line() {
        let (_dir, root, state) = project();
        let mut app = app(&root, &state, true).await;
        // The file at line 10 → its unstaged changes, on line 10 rather than the first change.
        app.test_execute(Command::GoToLine);
        app.type_text("11");
        app.press(K::Enter, crossterm::event::KeyModifiers::NONE);
        assert_eq!(file_line(&app), 10);
        app.test_execute(Command::ToggleChanges);
        assert!(app.settle(|a| a.diffs_settled()).await);
        let d = app.group().active_diff().expect("the diff tab");
        assert_eq!((d.pair, d.cursor_line()), (Pair::UNSTAGED, 10));
        draw(&mut app);

        // Down to line 12, then back to the file there.
        app.press(K::Down, crossterm::event::KeyModifiers::NONE);
        app.press(K::Down, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.group().active_diff().unwrap().cursor_line(), 12);
        app.test_execute(Command::ToggleChanges);
        assert_eq!(file_line(&app), 12);
        assert_eq!(app.group().tabs.len(), 2, "the file's own tab");

        // And again to the same diff tab (not a new one), at line 12.
        app.test_execute(Command::ToggleChanges);
        assert_eq!(app.group().tabs.len(), 2);
        assert_eq!(app.group().active_diff().unwrap().cursor_line(), 12);

        // The header's Open File button: the file at the diff's line (15).
        app.test_execute(Command::NextChange);
        assert_eq!(app.group().active_diff().unwrap().cursor_line(), 15);
        let term = draw(&mut app);
        let g = app.groups.active;
        let button = app
            .hits
            .group(g)
            .unwrap()
            .diff
            .as_ref()
            .unwrap()
            .open_file
            .expect("drawn");
        let drawn: String = (button.x..button.x + button.width)
            .map(|x| term.backend().buffer()[(x, button.y)].symbol().to_string())
            .collect();
        assert_eq!(drawn, " Open File ");
        app.mouse(MouseEventKind::Down(MouseButton::Left), button.x + 2, button.y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), button.x + 2, button.y);
        assert_eq!(file_line(&app), 15);
        // The diff tab is still there for the toggle back.
        assert_eq!(app.group().tabs.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn diff_open_file_opens_a_file_not_yet_open_at_the_diffs_line() {
        let (_dir, root, state) = project();
        let mut app = app(&root, &state, false).await;
        app.open_diff(root.join("a.txt"), Pair::UNSTAGED);
        assert!(app.settle(|a| a.diffs_settled()).await);
        assert_eq!(app.group().active_diff().unwrap().cursor_line(), 5, "the first change");
        let term = draw(&mut app);
        drop(term);
        let g = app.groups.active;
        let button = app.hits.group(g).unwrap().diff.as_ref().unwrap().open_file.unwrap();
        app.mouse(MouseEventKind::Down(MouseButton::Left), button.x, button.y);
        assert!(app.settle(|a| a.active_buffer().is_some()).await);
        assert_eq!(app.active_buffer().unwrap().path, root.join("a.txt"));
        assert_eq!(file_line(&app), 5);

        // A file without changes: Toggle says so and stays.
        fs::write(root.join("b.txt"), "b\n").unwrap();
        commit_all(&root, "two");
        assert!(app.settle(|a| !a.git.running()).await);
        app.test_execute(Command::QuickOpen);
        app.type_text("b.txt");
        assert!(
            app.settle(|a| a.palette.as_ref().is_some_and(|p| !p.rows.is_empty()))
                .await
        );
        app.press(K::Enter, crossterm::event::KeyModifiers::NONE);
        assert!(
            app.settle(|a| a.active_buffer().is_some_and(|b| b.path.ends_with("b.txt")))
                .await
        );
        app.test_execute(Command::ToggleChanges);
        assert!(app.group().active_diff().is_none());
        assert_eq!(app.message.as_ref().unwrap().text, "b.txt has no changes");
    }
}

mod first_reveal {
    use std::fs;
    use std::path::Path;
    use std::time::Instant;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    use crate::app::{App, Focus};
    use crate::buffer::Buffer;
    use crate::git::test_repo::{commit_all, init};
    use crate::layout::SidePanel;
    use crate::lsp::fake::FakeLauncher;
    use crate::milestone9_tests::{location, lsp_project, problems_project};
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};
    use crate::workspace::Workspace;
    use crate::workspace::trust::TrustStore;

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// The screen's row `y` from column `x`, `width` cells, trimmed.
    fn text(term: &Terminal<TestBackend>, x: u16, y: u16, width: u16) -> String {
        let buf = term.backend().buffer();
        (x..x + width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// The bottom panel's first list row (below its border and tab strip).
    fn first_panel_row(app: &App, term: &Terminal<TestBackend>) -> String {
        let panel = app.hits.regions.panel.expect("the panel drawn");
        text(term, panel.x, panel.y + 2, panel.width)
    }

    /// The app on `root` with no file open (the trust store in `state`).
    fn open_app(root: &Path, state: &Path) -> App {
        let ws = Workspace::load(root.to_path_buf());
        let mut app = App::new(ws, TrustStore::load(state.join("trust.toml")), None, true);
        app.test_start();
        app.test_plain_whitespace();
        app
    }

    /// A repository with `n` committed files `f00.txt`… each modified since.
    fn many_changes(root: &Path, n: usize) {
        init(root);
        for i in 0..n {
            fs::write(root.join(format!("f{i:02}.txt")), "a\n").unwrap();
        }
        commit_all(root, "one");
        for i in 0..n {
            fs::write(root.join(format!("f{i:02}.txt")), "b\n").unwrap();
        }
    }

    /// The first Ctrl+Shift+G on a list longer than the view selects the first file before the
    /// view was ever drawn: the "Changes" header above it stays in view.
    #[tokio::test(flavor = "multi_thread")]
    async fn source_control_keeps_the_changes_header_in_view() {
        let (_d, root, state) = dirs();
        many_changes(&root, 30);
        let mut app = open_app(&root, &state);
        assert!(settle_git(&mut app, |s| s.entries.len() == 30).await);
        app.press(K::Char('g'), M::CONTROL | M::SHIFT);
        let term = draw(&mut app, 100, 20);
        let hit = app.hits.scm.clone().expect("the Source Control view drawn");
        assert!(hit.list.height < 31, "the list doesn't fit");
        let row = |y| text(&term, hit.list.x, hit.list.y + y, hit.list.width);
        assert!(row(0).contains("Changes"), "{:?}", row(0));
        assert!(row(1).contains("f00.txt"), "{:?}", row(1));
    }

    /// End before the Source Control view was ever drawn: the first draw scrolls the last file
    /// up to the bottom row (the reveal waits for the drawn height, it isn't dropped).
    #[tokio::test(flavor = "multi_thread")]
    async fn source_control_end_before_the_first_draw_shows_the_last_file() {
        let (_d, root, state) = dirs();
        many_changes(&root, 30);
        let mut app = open_app(&root, &state);
        assert!(settle_git(&mut app, |s| s.entries.len() == 30).await);
        app.press(K::Char('g'), M::CONTROL | M::SHIFT);
        app.press(K::End, M::NONE);
        draw(&mut app, 100, 20);
        let hit = app.hits.scm.clone().expect("the Source Control view drawn");
        let rows = app.scm.rows(app.git.status.as_ref().unwrap());
        let last = rows.len() - 1;
        assert_eq!(app.scm.selected_row(&rows), Some(last));
        assert!(hit.rows.len() < rows.len(), "the list doesn't fit");
        assert_eq!(
            hit.rows.last(),
            Some(&rows[last].key()),
            "the last file on the bottom row"
        );
    }

    /// The Explorer reveals the file opened at start (far down a long folder) before the tree
    /// was ever drawn: the first draw scrolls the least that shows it, its row at the bottom.
    #[tokio::test(flavor = "multi_thread")]
    async fn explorer_reveals_the_start_file_with_the_rows_above_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        for i in 0..60 {
            fs::write(root.join(format!("f{i:02}.txt")), "x\n").unwrap();
        }
        let path = root.join("f30.txt");
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&path).unwrap());
        app.test_start();
        assert!(
            app.settle(|a| a.explorer.selected.as_deref() == Some(path.as_path()))
                .await
        );
        draw(&mut app, 100, 20);
        let height = app.explorer.height;
        assert!(height > 1 && height < 30, "{height}");
        assert_eq!(app.explorer.scroll, 31 - height, "f30.txt on the bottom row");
    }

    /// The Search view's results selected (Down from the query, Down again) before the list was
    /// drawn: the file's row above its first match stays in view.
    #[tokio::test(flavor = "multi_thread")]
    async fn search_results_keep_the_file_row_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::write(root.join("a.txt"), "needle\n".repeat(40)).unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
        app.test_start();
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("needle");
        app.search_tick(Instant::now() + crate::search_view::DEBOUNCE);
        let want = app.search_view.query();
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done && r.query == want))
                .await
        );
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        let term = draw(&mut app, 100, 20);
        let hit = app.hits.search.clone().expect("the Search view drawn");
        assert!(hit.list.height < 41, "the list doesn't fit");
        let results = app.search.as_ref().unwrap();
        assert_eq!(app.search_view.selected_row(results), Some(1));
        assert_eq!(app.search_view.scroll(results), 0);
        let row = text(&term, hit.list.x, hit.list.y, hit.list.width);
        assert!(row.contains("a.txt"), "{row:?}");
    }

    /// Cmd+Shift+M then Down before the Problems list was drawn: its first file's row stays in
    /// view above the selected problem.
    #[tokio::test(flavor = "multi_thread")]
    async fn problems_keep_the_file_row_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _fake, _root) = problems_project(dir.path()).await;
        // Four list rows for the fixture's six.
        app.ui.panel_height = 6;
        app.press(K::Char('m'), cmd() | M::SHIFT);
        assert_eq!(app.ui.focus, Focus::Panel);
        app.press(K::Down, M::NONE);
        let term = draw(&mut app, 130, 30);
        assert!(
            first_panel_row(&app, &term).starts_with("a.cpp"),
            "{:?}",
            first_panel_row(&app, &term)
        );
        app.shutdown_servers().await;
    }

    /// End before the Problems list was drawn: the first draw shows the last problem on the
    /// bottom row.
    #[tokio::test(flavor = "multi_thread")]
    async fn problems_end_before_the_first_draw_shows_the_last_row() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _fake, _root) = problems_project(dir.path()).await;
        app.ui.panel_height = 6;
        app.press(K::Char('m'), cmd() | M::SHIFT);
        app.press(K::End, M::NONE);
        draw(&mut app, 130, 30);
        let files = crate::problems::files(&app.lsp.diagnostics, &app.buffers);
        let rows = crate::grouped::row_count(&files);
        let height = app.problems.list.height;
        assert!(height > 1 && height < rows, "{height} of {rows}");
        assert_eq!(app.problems.list.selected_row(&files), Some(rows - 1));
        assert_eq!(
            app.problems.list.scroll(&files),
            rows - height,
            "the last row at the bottom"
        );
        app.shutdown_servers().await;
    }

    /// Shift+F12's answer fills the Locations list and selects its first place before the panel
    /// was ever drawn: the file's row above it stays in view.
    #[tokio::test(flavor = "multi_thread")]
    async fn locations_keep_the_file_row_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let text = "int target = 1;\n".repeat(8);
        let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", &text)], true).await;
        let a = root.join("a.cpp");
        let places: Vec<_> = (0..8).map(|line| location(&a, line, 4, 10)).collect();
        fake.respond("textDocument/references", json!(places));
        app.ui.panel_height = 6;
        app.press(K::F(12), M::SHIFT);
        assert!(app.settle(|a| a.locations.is_some()).await);
        assert!(app.settle(|a| a.locations.as_ref().unwrap().unread().is_empty()).await);
        let term = draw(&mut app, 130, 30);
        assert!(
            first_panel_row(&app, &term).starts_with("a.cpp"),
            "{:?}",
            first_panel_row(&app, &term)
        );
        let l = app.locations.as_ref().unwrap();
        assert_eq!(l.list.selected_row(&l.files), Some(1));
        app.shutdown_servers().await;
    }

    /// "Tools: Show" then Down before the Tools view was drawn: the first row stays in view.
    #[tokio::test(flavor = "multi_thread")]
    async fn tools_view_keeps_the_first_row_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeLauncher::new(Default::default());
        let (mut app, _root) = crate::tools_tests::tools_app(dir.path(), &fake, false);
        for _ in 0..6 {
            crate::tools_tests::run_ended(&mut app, "ok").await;
        }
        app.press(K::Char('u'), cmd() | M::SHIFT);
        app.press(K::Down, M::NONE);
        let term = draw(&mut app, 100, 8);
        let hit = app.hits.tools.clone().expect("the Tools view drawn");
        assert!(
            (hit.list.height as usize) < app.tools_rows().len(),
            "the list doesn't fit"
        );
        let rows = app.tools_rows();
        assert_eq!(app.tools_view.selected_row(&rows), Some(1));
        assert_eq!(app.tools_view.scroll(&rows), 0);
        assert_eq!(hit.rows.first(), Some(&rows[0].key()));
        drop(term);
    }

    /// End before the Tools view was drawn: the first draw shows the last row at the bottom.
    #[tokio::test(flavor = "multi_thread")]
    async fn tools_view_end_before_the_first_draw_shows_the_last_row() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeLauncher::new(Default::default());
        let (mut app, _root) = crate::tools_tests::tools_app(dir.path(), &fake, false);
        for _ in 0..6 {
            crate::tools_tests::run_ended(&mut app, "ok").await;
        }
        app.press(K::Char('u'), cmd() | M::SHIFT);
        app.press(K::End, M::NONE);
        draw(&mut app, 100, 8);
        let hit = app.hits.tools.clone().expect("the Tools view drawn");
        let rows = app.tools_rows();
        assert_eq!(app.tools_view.selected_row(&rows), Some(rows.len() - 1));
        assert!(hit.rows.len() < rows.len(), "the list doesn't fit");
        assert_eq!(hit.rows.last(), Some(&rows[rows.len() - 1].key()));
    }

    /// End in the Git History section before it was drawn: the first draw shows the last
    /// commit on the bottom row.
    #[tokio::test(flavor = "multi_thread")]
    async fn git_history_end_before_the_first_draw_shows_the_last_commit() {
        let (_d, root, state) = dirs();
        init(&root);
        for i in 0..20 {
            fs::write(root.join("a.txt"), format!("{i}\n")).unwrap();
            commit_all(&root, &format!("c{i}"));
        }
        let mut app = open_file_app(&root, &state, "a.txt");
        assert!(
            app.settle(|a| !a.git.running() && !a.git_history.loading && a.git_history.commits.len() == 20)
                .await
        );
        app.ui.focus = Focus::SidePanel;
        app.ui.side_panel = SidePanel::Explorer;
        app.git_history.set_focused(true);
        app.press(K::End, M::NONE);
        assert_eq!(app.git_history.selected_row(), Some(19));
        draw(&mut app, 100, 30);
        let height = app.git_history.height;
        assert!(height > 1 && height < 20, "{height}");
        assert_eq!(
            app.git_history.scroll(),
            20 - height,
            "the last commit on the bottom row"
        );
    }

    /// Down twice in the Git History section before it was drawn: the newest commit stays in
    /// view above the selected one.
    #[tokio::test(flavor = "multi_thread")]
    async fn git_history_keeps_the_first_commit_in_view() {
        let (_d, root, state) = dirs();
        init(&root);
        for i in 0..20 {
            fs::write(root.join("a.txt"), format!("{i}\n")).unwrap();
            commit_all(&root, &format!("c{i}"));
        }
        let mut app = open_file_app(&root, &state, "a.txt");
        assert!(
            app.settle(|a| !a.git.running() && !a.git_history.loading && a.git_history.commits.len() == 20)
                .await
        );
        app.ui.focus = Focus::SidePanel;
        app.ui.side_panel = SidePanel::Explorer;
        app.git_history.set_focused(true);
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        draw(&mut app, 100, 30);
        assert!(
            app.git_history.height > 1 && app.git_history.height < 20,
            "{}",
            app.git_history.height
        );
        assert_eq!(app.git_history.selected_row(), Some(1));
        assert_eq!(app.git_history.scroll(), 0);
    }
}

mod cursor_style {
    use std::fs;
    use std::path::PathBuf;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::config::settings;
    use crate::edit::CursorStyle;

    /// An app on `<dir>/ws` whose `.workspace` holds `workspace`, the user's `settings.toml`
    /// `[settings]` + `user`, with `a.txt` (two lines `ab`) open and a cursor after each `a`
    /// (the primary on line 0).
    fn open_app(dir: &tempfile::TempDir, user: &str, workspace: &str) -> (App, PathBuf) {
        let base = crate::workspace::normalize(dir.path());
        let user_file = base.join("config").join(settings::FILE_NAME);
        fs::create_dir_all(user_file.parent().unwrap()).unwrap();
        fs::write(&user_file, format!("[settings]\n{user}")).unwrap();
        let root = base.join("ws");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".workspace"), workspace).unwrap();
        let path = root.join("a.txt");
        fs::write(&path, "ab\nab\n").unwrap();
        let loaded = settings::load(&user_file);
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ws = crate::workspace::Workspace::load(root.clone()).with_user_settings(loaded.overrides);
        let trust = crate::workspace::trust::TrustStore::load(base.join("trust.toml"));
        let initial = crate::app::InitialFile {
            buffer: Buffer::open(&path).unwrap(),
            goto: None,
        };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.test_plain_whitespace();
        app.test_start();
        app.press(K::Right, M::NONE);
        app.test_execute(Command::AddCursorBelow);
        (app, root)
    }

    /// Line `line`'s second cell (the cursor's, on `b`) as drawn: symbol, modifiers, underline
    /// colour.
    fn cursor_cell(app: &mut App, line: u16) -> (String, Modifier, Option<ratatui::style::Color>) {
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let text = app.hits.groups[0].text;
        let c = &term.backend().buffer()[(text.x + 1, text.y + line)];
        (c.symbol().to_string(), c.modifier, Some(c.underline_color))
    }

    /// What `sync_cursor_shape` sends (as `run` does each turn).
    fn sent(app: &mut App) -> String {
        let mut out = Vec::new();
        app.sync_cursor_shape(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn line_by_default_draws_secondary_cursors_underlined() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "", "");
        assert_eq!(
            (app.settings.cursor_style, app.settings.cursor_blinking),
            (CursorStyle::Line, true)
        );
        let colour = app.theme.get("editor.cursor.secondaryUnderline").underline_color;
        assert!(colour.is_some());
        let (sym, modifier, underline) = cursor_cell(&mut app, 1);
        assert_eq!(sym, "b", "the char stays readable");
        assert!(modifier.contains(Modifier::UNDERLINED) && !modifier.contains(Modifier::REVERSED));
        assert_eq!(underline, colour);
        let (_, primary, _) = cursor_cell(&mut app, 0);
        assert!(
            !primary.contains(Modifier::UNDERLINED),
            "the terminal cursor marks the primary"
        );
        assert_eq!(sent(&mut app), "", "enter already sent the shape");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_style_draws_its_secondary_cursor() {
        for (style, reversed) in [("line", false), ("underline", false), ("block", true)] {
            let dir = tempfile::tempdir().unwrap();
            let (mut app, _) = open_app(&dir, &format!("cursor_style = \"{style}\"\n"), "");
            let (sym, modifier, _) = cursor_cell(&mut app, 1);
            assert_eq!(sym, "b", "{style}");
            assert_eq!(modifier.contains(Modifier::REVERSED), reversed, "{style}");
            assert_eq!(modifier.contains(Modifier::UNDERLINED), !reversed, "{style}");
            // At a line's end too (an empty cell).
            app.press(K::End, M::NONE);
            let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
            term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
            let text = app.hits.groups[0].text;
            let end = term.backend().buffer()[(text.x + 2, text.y + 1)].modifier;
            assert_eq!(end.contains(Modifier::REVERSED), reversed, "{style} at the end");
            assert_eq!(end.contains(Modifier::UNDERLINED), !reversed, "{style} at the end");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_workspace_reload_resends_the_cursor_shape() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, "cursor_blinking = false\n", "");
        assert_eq!(sent(&mut app), "");
        // A reload that changes another setting sends nothing.
        fs::write(root.join(".workspace"), "[settings]\ntab_width = 2\n").unwrap();
        assert!(app.settle(|a| a.settings.tab_width == 2).await);
        assert_eq!(sent(&mut app), "", "the shape didn't change");
        fs::write(root.join(".workspace"), "[settings]\ncursor_style = \"block\"\n").unwrap();
        assert!(app.settle(|a| a.settings.cursor_style == CursorStyle::Block).await);
        assert_eq!(sent(&mut app), "\x1b[2 q", "a steady block: the user's blinking kept");
        assert_eq!(sent(&mut app), "", "once");
        assert!(cursor_cell(&mut app, 1).1.contains(Modifier::REVERSED));

        fs::write(
            root.join(".workspace"),
            "[settings]\ncursor_style = \"underline\"\ncursor_blinking = true\n",
        )
        .unwrap();
        assert!(app.settle(|a| a.settings.cursor_style == CursorStyle::Underline).await);
        assert_eq!(sent(&mut app), "\x1b[3 q");
        // Reloaded with neither key: the user's settings again.
        fs::write(root.join(".workspace"), "").unwrap();
        assert!(app.settle(|a| a.settings.cursor_style == CursorStyle::Line).await);
        assert_eq!(sent(&mut app), "\x1b[6 q");
    }

    /// A task's DECSCUSR is taken by its own terminal (`vt100`): nothing of it shows, and jed's
    /// shape is left alone (task output never reaches the real terminal).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_tasks_cursor_shape_request_is_not_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = crate::milestone12_tests::dirs(&dir);
        let tasks = "[tasks.shape]\ncommand = \"printf 'a\\\\033[2 qb\\\\n'\"\n";
        let mut app = crate::milestone12_tests::open_app(&root, &state, Some(tasks));
        let id = app.run_task("shape").unwrap();
        assert!(crate::milestone12_tests::settle_ended(&mut app, id).await);
        let contents = app.tasks.get(id).unwrap().screen().screen().contents();
        assert!(contents.starts_with("ab\n"), "{contents:?}");
        assert_eq!(sent(&mut app), "", "jed's shape kept");
    }
}

mod move_tabs {
    use std::fs;
    use std::path::PathBuf;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, Focus, InitialFile};
    use crate::buffer::Buffer;
    use crate::diff_view::Pair;
    use crate::group::Tab;
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// Ctrl+Cmd on macOS, Ctrl+Alt on Linux.
    fn group_mods() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER | M::CONTROL
        } else {
            M::CONTROL | M::ALT
        }
    }

    fn to_next(app: &mut App) {
        app.press(K::Right, group_mods());
    }

    fn to_previous(app: &mut App) {
        app.press(K::Left, group_mods());
    }

    /// A workspace with `names` (40 numbered lines each), the first open, the others opened
    /// after it in the same group (the last active).
    fn open_app(dir: &tempfile::TempDir, names: &[&str]) -> (App, PathBuf) {
        let root = workspace::normalize(dir.path());
        for name in names {
            let text: String = (1..=40).map(|i| format!("{name} {i}\n")).collect();
            fs::write(root.join(name), text).unwrap();
        }
        let buffer = Buffer::open(&root.join(names[0])).unwrap();
        let ws = Workspace::load(root.clone());
        let trust = TrustStore::load(root.join("trust.toml"));
        let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
        app.test_start();
        for name in &names[1..] {
            app.test_open(Buffer::open(&root.join(name)).unwrap());
        }
        (app, root)
    }

    /// Each group's tabs (file names, `diff:` + name for a diff tab) in reading order.
    fn layout(app: &App) -> Vec<Vec<String>> {
        let name = |t: &Tab| match t {
            Tab::Text(t) => app.buffers.get(t.buffer).unwrap().name(),
            Tab::Diff(d) => format!("diff:{}", d.path.file_name().unwrap().to_string_lossy()),
            Tab::Hex(h) => format!("hex:{}", h.name()),
            Tab::Shell(s) => format!("shell:{}", s.title),
        };
        app.groups
            .ids()
            .into_iter()
            .map(|g| app.groups.get(g).unwrap().tabs.iter().map(name).collect())
            .collect()
    }

    fn active_name(app: &App) -> String {
        app.active_buffer().unwrap().name()
    }

    /// The active view's primary head (byte), top line and left column.
    fn view_state(app: &App) -> (usize, usize, usize) {
        let v = &app.group().active_tab().unwrap().view;
        (v.selections.primary().head.0, v.top_line(), v.left_col)
    }

    fn draw(app: &mut App, w: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn into_the_next_group_splits_right_keeping_the_view_and_back_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
        draw(&mut app, 80);
        // A place in b.txt: line 30 (scrolled), a selection.
        for _ in 0..29 {
            app.press(K::Down, M::NONE);
        }
        app.press(K::Right, M::SHIFT);
        draw(&mut app, 80);
        let before = view_state(&app);
        assert!(before.1 > 0, "scrolled");
        let first = app.groups.active;

        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]]);
        assert_ne!(app.groups.active, first, "the new group has the focus");
        assert_eq!(app.groups.ids()[1], app.groups.active, "split to the right");
        assert_eq!((active_name(&app), view_state(&app)), ("b.txt".into(), before));
        let sel = app.group().active_tab().unwrap().view.selections.primary();
        assert_ne!(sel.anchor, sel.head, "the selection kept");
        assert_eq!(app.ui.focus, Focus::Editor);

        // Its group's only tab with no group to the right: nothing.
        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]]);

        // Back into the first group, right of its active tab; the emptied group closes.
        to_previous(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt", "b.txt"]]);
        assert_eq!((app.groups.active, active_name(&app)), (first, "b.txt".into()));
        assert_eq!(view_state(&app), before);
        // No previous group: nothing.
        to_previous(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt", "b.txt"]]);
        // Undo still works on the moved tab's buffer.
        app.type_text("x");
        app.press(K::Char('z'), cmd());
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn groups_in_reading_order_landing_right_of_the_active_tab() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &["a", "b", "c", "d", "e"]);
        // From the explorer: the tab moves and the editor gets the focus.
        app.press(K::Char('e'), cmd() | M::SHIFT);
        assert_eq!(app.ui.focus, Focus::SidePanel);
        to_next(&mut app);
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(layout(&app), [vec!["a", "b", "c", "d"], vec!["e"]]);
        app.press(K::Char('1'), cmd());
        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a", "b", "c"], vec!["e", "d"]]);

        // Into the second group with its first tab active: right of it, not at the end.
        app.press(K::PageUp, M::CONTROL);
        assert_eq!(active_name(&app), "e");
        app.press(K::Char('1'), cmd());
        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a", "b"], vec!["e", "c", "d"]]);
        assert_eq!(active_name(&app), "c");

        // The second group's next is a new third one; the third's previous is the second.
        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a", "b"], vec!["e", "d"], vec!["c"]]);
        to_previous(&mut app);
        assert_eq!(layout(&app), [vec!["a", "b"], vec!["e", "d", "c"]]);
        assert_eq!(
            (app.groups.ids()[1], active_name(&app)),
            (app.groups.active, "c".into())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_only_tab_of_the_only_group_stays_put() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &["a.txt"]);
        to_next(&mut app);
        to_previous(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt"]]);
        assert_eq!(app.groups.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn into_a_group_showing_the_buffer_that_tab_takes_the_moved_view() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
        // Split: the second group shows b.txt too; back in the first, b.txt's cursor moves
        // elsewhere and it moves right.
        app.press(K::Char('\\'), cmd());
        let second = app.groups.active;
        app.press(K::Char('1'), cmd());
        let first = app.groups.active;
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        let moved = view_state(&app);

        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]]);
        assert_eq!(app.groups.active, second);
        assert_eq!(view_state(&app), moved, "the moved view, not the one that was there");
        assert_eq!(app.groups.tabs_showing(app.group().active_tab().unwrap().buffer), 1);

        // a.txt, the first group's last tab: its group closes.
        app.press(K::Char('1'), cmd());
        assert_eq!(app.groups.active, first);
        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["b.txt", "a.txt"]]);
        assert_eq!((app.groups.active, active_name(&app)), (second, "a.txt".into()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn diff_tabs_move_too() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, &["a.txt", "b.txt"]);
        app.open_diff(root.join("a.txt"), Pair::SAVED);
        let diff = |app: &App| app.group().active_diff().map(|d| (d.id, d.path.clone(), d.pair));
        let before = diff(&app).expect("a diff tab");
        app.group_mut().active_diff_mut().unwrap().left_col = 3;

        to_next(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt", "b.txt"], vec!["diff:a.txt"]]);
        assert_eq!(diff(&app), Some(before), "the same tab");
        assert_eq!(app.group().active_diff().unwrap().left_col, 3);
        assert!(
            app.settle(|a| a.group().active_diff().unwrap().doc.is_some()).await,
            "it loads"
        );
        to_previous(&mut app);
        assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "diff:a.txt"]]);
        assert!(app.group().active_diff().is_some());
    }

    fn move_left(app: &mut App) {
        app.press(K::PageUp, cmd() | M::SHIFT);
    }

    fn move_right(app: &mut App) {
        app.press(K::PageDown, cmd() | M::SHIFT);
    }

    /// The tab bar's row as drawn.
    fn tab_bar(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn left_and_right_swap_with_a_neighbour_without_wrapping() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["one.txt", "two.txt", "three.txt", "four.txt", "five.txt", "six.txt"];
        let (mut app, _) = open_app(&dir, &names);
        app.press(K::Down, M::NONE);
        let view = view_state(&app);
        // six.txt is last: right does nothing.
        move_right(&mut app);
        assert_eq!(layout(&app)[0], names);
        move_left(&mut app);
        move_left(&mut app);
        assert_eq!(
            layout(&app)[0],
            ["one.txt", "two.txt", "three.txt", "six.txt", "four.txt", "five.txt"]
        );
        assert_eq!((app.group().active, active_name(&app)), (3, "six.txt".into()));
        assert_eq!(view_state(&app), view);
        for _ in 0..5 {
            move_left(&mut app);
        }
        assert_eq!(layout(&app)[0][0], "six.txt", "stops at the start");
        assert_eq!(app.group().active, 0);

        // In a tab bar too narrow for all of them, the moved tab stays in view.
        let term = draw(&mut app, 40);
        assert!(tab_bar(&term).contains("six.txt"), "{}", tab_bar(&term));
        for _ in 0..5 {
            move_right(&mut app);
        }
        assert_eq!(layout(&app)[0][5], "six.txt");
        let term = draw(&mut app, 40);
        assert!(tab_bar(&term).contains("six.txt"), "{}", tab_bar(&term));
    }
}

mod text_field_keys {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, InputKind, Prompt};
    use crate::buffer::Buffer;
    use crate::config::keymap::{self, Keymap, Platform};

    const MAC: bool = cfg!(target_os = "macos");

    fn cmd() -> M {
        if MAC { M::SUPER } else { M::CONTROL }
    }

    /// Word moves and deletes: Alt on macOS, Ctrl on Linux.
    fn word() -> M {
        if MAC { M::ALT } else { M::CONTROL }
    }

    /// An app on `<dir>/ws` with `a.txt` (`text`) open, the cursor at its start.
    fn open_app(dir: &tempfile::TempDir, text: &str) -> (App, PathBuf) {
        let root = crate::workspace::normalize(dir.path()).join("ws");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a.txt");
        std::fs::write(&path, text).unwrap();
        let ws = crate::workspace::Workspace::load(root.clone());
        let trust = crate::workspace::trust::TrustStore::load(dir.path().join("trust.toml"));
        let initial = crate::app::InitialFile {
            buffer: Buffer::open(&path).unwrap(),
            goto: None,
        };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.test_start();
        app.test_plain_whitespace();
        (app, root)
    }

    fn query(app: &App) -> (&str, usize) {
        (app.find.query.text.as_str(), app.find.query.cursor)
    }

    fn match_count(app: &App) -> usize {
        app.find.matches().len()
    }

    /// Each key that moves a field's cursor to its start / end on this platform, as the editor's
    /// line start / end (Home / End everywhere; Cmd+Left / Right and Ctrl+A / E on macOS,
    /// Ctrl+Home / End on Linux).
    fn start_keys() -> Vec<(K, M)> {
        let mut keys = vec![(K::Home, M::NONE)];
        if MAC {
            keys.extend([(K::Left, M::SUPER), (K::Char('a'), M::CONTROL)]);
        } else {
            keys.push((K::Home, M::CONTROL));
        }
        keys
    }

    fn end_keys() -> Vec<(K, M)> {
        let mut keys = vec![(K::End, M::NONE)];
        if MAC {
            keys.extend([(K::Right, M::SUPER), (K::Char('e'), M::CONTROL)]);
        } else {
            keys.push((K::End, M::CONTROL));
        }
        keys
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_shift_home_then_typing_replaces_the_find_query() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "foo bar foo baz\n");
        app.press(K::Char('f'), cmd());
        assert_eq!(
            (query(&app), match_count(&app)),
            (("foo", 3), 2),
            "seeded from the word"
        );
        app.press(K::End, M::NONE);
        assert_eq!(app.find.query.selection(), None, "End collapses the opening selection");
        app.press(K::Home, M::SHIFT);
        assert_eq!(app.find.query.selection(), Some(0..3));
        app.type_text("ba");
        assert_eq!((query(&app), match_count(&app)), (("ba", 2), 2), "bar and baz");
        app.press(K::Left, M::SHIFT);
        app.type_text("r");
        assert_eq!((query(&app), match_count(&app)), (("br", 2), 0));
        // Shift+Left twice then Right: collapsed to the selection's end.
        app.press(K::Left, M::SHIFT);
        app.press(K::Left, M::SHIFT);
        app.press(K::Right, M::NONE);
        assert_eq!((query(&app), app.find.query.selection()), (("br", 2), None));
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "foo bar foo baz\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_line_start_and_end_keys_in_the_find_widget() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('f'), cmd());
        app.type_text("ab");
        for (code, mods) in start_keys() {
            app.press(K::End, M::NONE);
            app.press(code, mods);
            assert_eq!(query(&app), ("ab", 0), "{code:?} {mods:?}");
            app.press(code, mods | M::SHIFT);
            assert_eq!(app.find.query.selection(), None, "{code:?} {mods:?}: already there");
        }
        for (code, mods) in end_keys() {
            app.press(K::Home, M::NONE);
            app.press(code, mods | M::SHIFT);
            assert_eq!(
                (query(&app), app.find.query.selection()),
                (("ab", 2), Some(0..2)),
                "{code:?}"
            );
            app.press(code, mods);
            assert_eq!((query(&app), app.find.query.selection()), (("ab", 2), None), "{code:?}");
        }
        assert_eq!(
            app.active_buffer().unwrap().rope.to_string(),
            "x\n",
            "the editor untouched"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_home_end_and_word_delete_in_the_search_query() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('f'), cmd() | M::SHIFT);
        app.type_text("foo.bar baz");
        app.press(K::Backspace, word());
        assert_eq!(app.search_view.query.text, "foo.bar ");
        app.press(K::Backspace, word());
        assert_eq!(app.search_view.query.text, "foo.");
        app.press(K::Home, M::NONE);
        app.type_text("<");
        app.press(K::End, M::NONE);
        app.type_text(">");
        assert_eq!(app.search_view.query.text, "<foo.>");
        app.press(K::Left, word());
        app.press(K::Delete, word());
        assert_eq!(
            app.search_view.query.text, "<foo",
            "the punctuation run `.>` after the cursor"
        );
        assert!(app.search_view.deadline().is_some(), "an edit re-searches");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "x\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_select_all_copy_cut_in_the_find_widget() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "needle hay\n");
        app.press(K::Char('f'), cmd());
        app.press(K::End, M::NONE);
        // Nothing selected: Cmd+C copies nothing (not the editor's whole line).
        app.press(K::Char('c'), cmd());
        assert_eq!(app.clipboard.read().unwrap(), "");
        app.press(K::Char('a'), cmd());
        app.press(K::Char('c'), cmd());
        assert_eq!(app.clipboard.read().unwrap(), "needle");
        assert_eq!(query(&app), ("needle", 6));
        app.press(K::Left, word());
        app.press(K::Right, M::SHIFT);
        app.press(K::Char('x'), cmd());
        assert_eq!(app.clipboard.read().unwrap(), "n");
        assert_eq!(query(&app), ("eedle", 0));
        assert_eq!(match_count(&app), 1);
        if !MAC {
            // Linux Ctrl+V pastes the clipboard into the field (macOS's Cmd+V is the terminal's).
            app.press(K::End, M::NONE);
            app.press(K::Char('v'), M::CONTROL);
            assert_eq!(query(&app), ("eedlen", 6));
        }
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "needle hay\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_keybindings_toml_rebinds_apply_in_a_field() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "xyz\n");
        let config = dir.path().join(keymap::FILE_NAME);
        // A global binding of the editor's line start, and line end bound in the find context.
        let toml = "[[binding]]\nkey = \"ctrl+j\"\ncommand = \"cursor.home\"\n\n\
                    [[binding]]\nkey = \"ctrl+k\"\ncommand = \"cursor.end\"\ncontext = \"find\"\n";
        std::fs::write(&config, toml).unwrap();
        let (km, errors) = Keymap::load(&config, Platform::current());
        assert!(errors.is_empty(), "{errors:?}");
        app.set_keymap(km, errors);
        // The editor's cursor at its line's end, so a line start run on it would show.
        app.press(K::End, M::NONE);
        app.press(K::Char('j'), M::CONTROL);
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        assert_eq!(head.0, 0, "ctrl+j is the editor's line start too");
        // (End lost cursor.end, rebound in the find context: Right moves the editor back.)
        for _ in 0..3 {
            app.press(K::Right, M::NONE);
        }
        app.press(K::Char('f'), cmd());
        let head = app.group().active_tab().unwrap().view.selections.primary().head;
        assert_eq!(head.0, 3);
        app.type_text("ab");
        app.press(K::Home, M::NONE);
        assert_eq!(query(&app), ("ab", 2), "Home lost the command");
        app.press(K::Char('j'), M::CONTROL);
        assert_eq!(query(&app), ("ab", 0));
        app.press(K::Char('k'), M::CONTROL);
        assert_eq!(query(&app), ("ab", 2));
        let after = app.group().active_tab().unwrap().view.selections.primary().head;
        assert_eq!(after, head, "the editor's cursor stayed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_in_the_palette_and_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('p'), cmd());
        app.type_text("src main");
        app.press(K::Backspace, word());
        assert_eq!(app.palette.as_ref().unwrap().field.text, "src ");
        app.press(K::Home, M::SHIFT);
        app.type_text("lib");
        assert_eq!(app.palette.as_ref().unwrap().field.text, "lib");
        app.press(K::Esc, M::NONE);

        // A prompt's field: the explorer's Rename (F2 on the open file's row), its name prefilled.
        app.press(K::Char('e'), cmd() | M::SHIFT);
        assert!(app.settle(|a| !a.explorer.rows().is_empty()).await);
        app.press(K::Home, M::NONE);
        app.press(K::F(2), M::NONE);
        assert!(matches!(&app.prompt, Some(Prompt::Input(i)) if matches!(i.kind, InputKind::Rename { .. })));
        app.press(K::Home, M::NONE);
        app.press(K::Right, M::SHIFT);
        app.type_text("b");
        app.press(K::Right, word());
        app.press(K::Right, word());
        app.press(K::End, M::SHIFT);
        assert!(
            matches!(&app.prompt, Some(Prompt::Input(i)) if i.field.text == "b.txt" && i.field.selection().is_none())
        );
        app.press(K::Left, word());
        app.press(K::End, M::SHIFT);
        app.press(K::Backspace, M::NONE);
        assert!(
            matches!(&app.prompt, Some(Prompt::Input(i)) if i.field.text == "b."),
            "{}",
            prompt_text(&app)
        );
    }

    fn prompt_text(app: &App) -> String {
        match &app.prompt {
            Some(Prompt::Input(i)) => i.field.text.clone(),
            _ => "no input prompt".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_the_selected_range_is_drawn_selected() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('f'), cmd());
        app.type_text("qwerty");
        app.press(K::Left, M::NONE);
        app.press(K::Left, M::SHIFT);
        app.press(K::Left, M::SHIFT);
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let (x, y) = (0..buf.area.height)
            .find_map(|y| {
                let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                let at = row.windows(6).position(|w| w.concat() == "qwerty")?;
                Some((at as u16, y))
            })
            .expect("the query drawn");
        let selection = app.theme.get("editor.selection").bg;
        let selected: Vec<bool> = (0..6).map(|i| buf[(x + i, y)].bg == selection.unwrap()).collect();
        assert_eq!(selected, [false, false, false, true, true, false], "\"rt\" selected");
    }

    /// The terminal's rows as text.
    fn rows(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_a_long_query_scrolls_to_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('f'), cmd());
        let long: String = ('a'..='z').chain('A'..='Z').collect();
        app.type_text(&long);
        let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let y = rows(&term)
            .iter()
            .position(|r| r.contains("XYZ"))
            .expect("the query's end drawn");
        let top = rows(&term)[y].clone();
        assert!(!top.contains("abc"), "scrolled to the end: {top}");
        app.press(K::Home, M::NONE);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let top = rows(&term)[y].clone();
        assert!(top.contains("abcdef") && !top.contains("XYZ"), "from the start: {top}");
        let first = top.find("abcdef").unwrap();
        // A short query fits: all of it from the input's first column, whatever the cursor.
        app.press(K::Char('a'), cmd());
        app.type_text("short");
        app.press(K::Left, M::NONE);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let top = rows(&term)[y].clone();
        let at = top.find("short").expect("the query drawn");
        assert_eq!(at, first, "at the input's first column: {top}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_global_commands_still_run_in_a_field() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "xyz\n");
        app.press(K::End, M::NONE);
        app.press(K::Char('f'), cmd());
        app.type_text("ab");
        app.press(K::Left, M::NONE);
        let before = app.group().active_tab().unwrap().view.selections.clone();
        // Select all, and Shift+End, are the field's: the editor's selections stay.
        app.press(K::Char('a'), cmd());
        assert_eq!(app.find.query.selection(), Some(0..2));
        app.press(K::Left, M::NONE);
        app.press(K::End, M::SHIFT);
        assert_eq!(app.find.query.selection(), Some(0..2));
        assert_eq!(
            app.group().active_tab().unwrap().view.selections,
            before,
            "the editor's selections untouched"
        );
        // A global binding without a field meaning still runs (Cmd+B: the sidebar).
        let side = app.ui.side_visible;
        app.press(K::Char('b'), cmd());
        assert_ne!(app.ui.side_visible, side, "Cmd+B ran");
        assert_eq!(app.ui.focus, crate::app::Focus::Find, "the field keeps the keys");
        // Cmd+Shift+P opens the palette over it; its field then has the keys.
        app.press(K::Char('p'), cmd() | M::SHIFT);
        assert!(app.palette.is_some());
        app.type_text("zz");
        app.press(K::Left, M::SHIFT);
        app.press(K::Backspace, M::NONE);
        assert_eq!(
            app.palette.as_ref().unwrap().field.text,
            ">z",
            "the palette's field, not the query"
        );
        assert_eq!(query(&app), ("ab", 2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_field_keys_a_prompts_selection_is_drawn_selected() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, "x\n");
        app.press(K::Char('e'), cmd() | M::SHIFT);
        assert!(app.settle(|a| !a.explorer.rows().is_empty()).await);
        app.press(K::Home, M::NONE);
        app.press(K::F(2), M::NONE);
        app.press(K::Home, M::NONE);
        app.press(K::Right, M::SHIFT);
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        // The dialog's input: two rows below its title (other rows show `a.txt` too).
        let title = rows(&term)
            .iter()
            .position(|r| r.contains("Rename ws/a.txt"))
            .expect("the prompt");
        let y = title as u16 + 2;
        let row: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        let x = row
            .windows(5)
            .position(|w| w.concat() == "a.txt")
            .expect("the prompt's field drawn") as u16;
        let selection = app.theme.get("editor.selection").bg.unwrap();
        let selected: Vec<bool> = (0..5).map(|i| buf[(x + i, y)].bg == selection).collect();
        assert_eq!(selected, [true, false, false, false, false], "\"a\" selected");
    }
}

/// Keyboard cheat sheet overlay: Shift+F1 opens it over the editor area, built from the live
/// keymap (a rebound command shows its new key, an unbound one is absent), the context it opens
/// from first; it scrolls; Esc, Shift+F1 or a click outside close it; other keys are swallowed.
mod cheat_sheet {
    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::config::keymap::{self, Context, Keymap, Platform};

    fn open_app(dir: &tempfile::TempDir) -> App {
        open_with(dir, "abc\n")
    }

    fn open_with(dir: &tempfile::TempDir, text: &str) -> App {
        let root = crate::workspace::normalize(dir.path());
        let path = root.join("a.txt");
        std::fs::write(&path, text).unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&path).unwrap());
        app.test_start();
        app
    }

    /// The screen's rows as text.
    fn draw(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn line_with<'a>(screen: &'a [String], text: &str) -> Option<&'a String> {
        screen.iter().find(|l| l.contains(text))
    }

    fn shift_f1(app: &mut App) {
        app.press(K::F(1), M::SHIFT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_shows_the_live_keymap() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        // Quick open moved to Ctrl+G, which takes it from Go to Line (now bound to nothing).
        let config = dir.path().join(keymap::FILE_NAME);
        std::fs::write(
            &config,
            "[[binding]]\nkey = \"ctrl+g\"\ncommand = \"palette.quickOpen\"\n",
        )
        .unwrap();
        let (km, errors) = Keymap::load(&config, Platform::current());
        assert!(errors.is_empty(), "{errors:?}");
        app.set_keymap(km, errors);

        assert!(line_with(&draw(&mut app, 200, 50), "Keyboard Cheat Sheet").is_none());
        shift_f1(&mut app);
        assert!(app.cheat_sheet.is_some());
        let screen = draw(&mut app, 200, 50);
        assert!(line_with(&screen, "Keyboard Cheat Sheet").is_some(), "{screen:#?}");
        let quick_open = line_with(&screen, &Command::QuickOpen.title()).expect("quick open listed");
        let at = quick_open.find(&Command::QuickOpen.title()).unwrap();
        assert!(quick_open[..at].trim_end().ends_with("Ctrl+G"), "{quick_open}");
        assert!(line_with(&screen, &Command::GoToLine.title()).is_none(), "{screen:#?}");
        assert!(line_with(&screen, "Shift+F1").is_some(), "{screen:#?}");
        for heading in [
            "Cursor Movement",
            "Selection",
            "Going Places",
            "Tabs and Groups",
            "Panels",
        ] {
            assert!(line_with(&screen, heading).is_some(), "{heading}");
        }
        // Wide enough for several columns: two headings share a row.
        assert!(
            screen
                .iter()
                .any(|l| l.contains("Cursor Movement") && l.contains("Going Places")),
            "{screen:#?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_swallows_keys_and_closes_with_esc_or_shift_f1() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        shift_f1(&mut app);
        app.type_text("x");
        app.press(K::Enter, M::NONE);
        app.press(
            K::Char('p'),
            if cfg!(target_os = "macos") {
                M::SUPER
            } else {
                M::CONTROL
            },
        );
        app.handle_test_event(crossterm::event::Event::Paste("pasted".into()));
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "abc\n");
        assert!(app.palette.is_none());
        assert!(app.cheat_sheet.is_some());
        app.press(K::Esc, M::NONE);
        assert!(app.cheat_sheet.is_none());
        shift_f1(&mut app);
        assert!(app.cheat_sheet.is_some());
        shift_f1(&mut app);
        assert!(app.cheat_sheet.is_none());
        assert!(line_with(&draw(&mut app, 120, 40), "Keyboard Cheat Sheet").is_none());
        // Typing works again.
        app.type_text("x");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "xabc\n");
        // From the palette too.
        app.test_execute(Command::ShowCommands);
        app.test_execute(Command::CheatSheet);
        assert!(app.palette.is_none());
        assert!(app.cheat_sheet.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_closes_on_a_click_outside() {
        let dir = tempfile::tempdir().unwrap();
        // Long lines everywhere, so any click in the editor lands on text and moves the cursor.
        let text: String = (0..60).map(|i| format!("{i:02} {}\n", "x".repeat(200))).collect();
        let mut app = open_with(&dir, &text);
        app.test_plain_whitespace();
        shift_f1(&mut app);
        draw(&mut app, 120, 40);
        let rect = app.hits.cheat_sheet.unwrap().rect;
        let primary = |app: &App| *app.group().active_tab().unwrap().view.selections.primary();
        let start = primary(&app);
        // Inside (over the editor's text underneath): nothing happens.
        app.mouse(Mouse::Down(MouseButton::Left), rect.x + 2, rect.y + 2);
        app.mouse(Mouse::Up(MouseButton::Left), rect.x + 2, rect.y + 2);
        assert!(app.cheat_sheet.is_some());
        assert_eq!(primary(&app), start, "a click inside reached the editor");
        // The wheel scrolls the sheet, never the editor under it.
        app.mouse(Mouse::ScrollDown, rect.x.saturating_sub(1), rect.y + 1);
        assert_eq!(app.group().active_tab().unwrap().view.top_line(), 0);
        // Outside (on the editor's text): closes it and moves no cursor.
        let (x, y) = (rect.x.saturating_sub(1), rect.y + 1);
        app.mouse(Mouse::Down(MouseButton::Left), x, y);
        app.mouse(Mouse::Up(MouseButton::Left), x, y);
        assert!(app.cheat_sheet.is_none());
        assert_eq!(primary(&app), start, "the closing click reached the editor");
        // Control: the same click with the sheet closed does move the cursor.
        draw(&mut app, 120, 40);
        app.mouse(Mouse::Down(MouseButton::Left), x, y);
        app.mouse(Mouse::Up(MouseButton::Left), x, y);
        assert_ne!(primary(&app), start, "the click spot isn't on the editor's text");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_from_the_find_widget_shows_finds_keys_and_esc_closes_only_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::OpenFind);
        assert_eq!(app.ui.focus, crate::app::Focus::Find);
        let query = app.find.query.text.clone();
        shift_f1(&mut app);
        assert_eq!(app.cheat_sheet.as_ref().unwrap().context, Context::Find);
        let screen = draw(&mut app, 160, 50);
        let rect = app.hits.cheat_sheet.unwrap().rect;
        let first: String = screen[rect.y as usize + 1]
            .chars()
            .skip(rect.x as usize + 2)
            .take(4)
            .collect();
        assert_eq!(first, "Find", "{screen:#?}");
        // Typed text and a paste reach neither the find input nor the buffer.
        app.type_text("zz");
        app.handle_test_event(crossterm::event::Event::Paste("pasted".into()));
        assert_eq!(app.find.query.text, query, "keys reached the find input");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "abc\n");
        // Esc closes the sheet only: the find widget stays, focused.
        app.press(K::Esc, M::NONE);
        assert!(app.cheat_sheet.is_none());
        assert!(app.find.visible);
        assert_eq!(app.ui.focus, crate::app::Focus::Find);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_opens_with_the_explorers_keys_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        app.test_execute(Command::FocusExplorer);
        shift_f1(&mut app);
        assert_eq!(app.cheat_sheet.as_ref().unwrap().context, Context::Explorer);
        let screen = draw(&mut app, 160, 50);
        let rect = app.hits.cheat_sheet.unwrap().rect;
        let first = &screen[rect.y as usize + 1];
        let col: String = first.chars().skip(rect.x as usize + 2).take(8).collect();
        assert_eq!(col, "Explorer", "{screen:#?}");
        // Closing leaves the focus where it was.
        app.press(K::Esc, M::NONE);
        assert!(app.cheat_sheet.is_none());
        assert_eq!(app.ui.focus, crate::app::Focus::SidePanel);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cheat_sheet_scrolls_in_one_column_on_a_narrow_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open_app(&dir);
        shift_f1(&mut app);
        let screen = draw(&mut app, 60, 20);
        let sheet = app.cheat_sheet.as_ref().unwrap();
        assert!(sheet.rows > sheet.height, "taller than the screen");
        let bar = app.hits.cheat_sheet.unwrap().scrollbar.expect("a scroll bar");
        assert!(bar.thumb.is_some_and(|t| t.start == 0));
        assert!(
            !screen
                .iter()
                .any(|l| l.contains("Cursor Movement") && l.contains("Selection"))
        );
        // The wheel scrolls it.
        app.mouse(Mouse::ScrollDown, bar.track.x - 3, bar.track.y + 1);
        assert_eq!(app.cheat_sheet.as_ref().unwrap().top, 3);
        // Keys do too.
        app.press(K::Down, M::NONE);
        assert_eq!(app.cheat_sheet.as_ref().unwrap().top, 4);
        app.press(K::End, M::NONE);
        draw(&mut app, 60, 20);
        let sheet = app.cheat_sheet.as_ref().unwrap();
        assert_eq!(sheet.top, sheet.rows - sheet.height);
        let thumb = app.hits.cheat_sheet.unwrap().scrollbar.unwrap().thumb.unwrap();
        assert_eq!(thumb.start + thumb.len, bar.track.height);
        // Dragging the thumb to the top scrolls back.
        let y = bar.track.y + thumb.start;
        app.mouse(Mouse::Down(MouseButton::Left), bar.track.x, y);
        app.mouse(
            Mouse::Drag(MouseButton::Left),
            bar.track.x,
            bar.track.y.saturating_sub(5),
        );
        app.mouse(Mouse::Up(MouseButton::Left), bar.track.x, bar.track.y);
        assert_eq!(app.cheat_sheet.as_ref().unwrap().top, 0);
        assert!(app.cheat_sheet.is_some());
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "abc\n");
    }
}

/// Single file mode (`jed -s <file>`): the file's `.workspace` settings and colours, but no trust
/// dialog, server, listing, session or recent entry; every side view and the bottom panel greyed
/// out / refused with the notice; typing, Cmd+S and Cmd+W save and quit; a change on disk still
/// reloads or shows the banner.
mod single_file {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind as Mouse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, InitialFile, Mode};
    use crate::buffer::Buffer;
    use crate::command::Command;
    use crate::lsp::fake::{FakeLauncher, FakeOptions};
    use crate::workspace::Workspace;
    use crate::workspace::trust::TrustStore;

    const NOTICE: &str = "Not available in single file mode";
    const WORKSPACE: &str = "[settings]\ntab_width = 3\n\n[tasks.build]\ncommand = \"echo build\"\n";

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A repo-like root with a `.workspace` (settings, a task: trust undecided) and `a.cpp`; the
    /// state dir beside it.
    fn dirs(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let base = crate::workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(root.join(".workspace"), WORKSPACE).unwrap();
        fs::write(root.join("src/a.cpp"), "int main() { return 0; }\n").unwrap();
        fs::write(root.join("src/b.cpp"), "int b;\n").unwrap();
        (root, state)
    }

    /// `main`'s single file start: the root from the file, a missing file empty, a fake C++
    /// server plugged in (which workspace mode would start), the state dir set (which workspace
    /// mode would write the session and the recent entry into).
    fn open_single(path: &Path, state: &Path) -> (App, std::sync::Arc<FakeLauncher>) {
        open_in(Mode::SingleFile, path, state)
    }

    fn open_in(mode: Mode, path: &Path, state: &Path) -> (App, std::sync::Arc<FakeLauncher>) {
        let ws = Workspace::load(crate::workspace::root_for_file(path));
        let buffer = Buffer::open_with(path, ws.settings().indentation()).unwrap_or_else(|_| Buffer::new_file(path));
        let initial = Some(InitialFile { buffer, goto: None });
        let mut app = App::new_in(mode, ws, TrustStore::load(state.join("trust.toml")), initial, true);
        app.state_dir = Some(state.to_path_buf());
        let fake = FakeLauncher::new(FakeOptions::default());
        app.lsp.set_launcher(fake.clone());
        app.test_start();
        app.test_plain_whitespace();
        (app, fake)
    }

    fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    fn row(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_opens_just_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let session = crate::session::file(&state, &root);
        fs::create_dir_all(session.parent().unwrap()).unwrap();
        fs::write(&session, "not touched").unwrap();
        let (mut app, fake) = open_single(&root.join("src/a.cpp"), &state);

        assert_eq!(app.workspace.root, root, "the root walks up to the .workspace");
        assert!(app.prompt.is_none(), "no trust dialog");
        assert_eq!(app.settings.tab_width, 3, "the .workspace's settings apply");
        assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
        app.pump();
        assert!(fake.launches().is_empty(), "no language server: {:?}", fake.launches());
        assert!(!app.ui.side_visible && !app.explorer.is_loaded(&root), "nothing listed");
        assert!(app.git.status.is_none());

        // Even a restricted workspace shows no badge: single file mode runs nothing to trust.
        app.trust = crate::workspace::trust::Trust::Restricted;
        let term = draw(&mut app, 80, 10);
        let status = row(&term, 9);
        assert!(status.starts_with(" Single file "), "{status:?}");
        assert!(!status.contains("Restricted"), "{status:?}");

        app.test_execute(Command::Quit);
        assert!(app.quitting());
        app.shutdown().await;
        assert_eq!(
            fs::read_to_string(&session).unwrap(),
            "not touched",
            "the session file untouched"
        );
        assert!(!crate::recent::file(&state).exists(), "no recent-workspace entry");
    }

    /// The same start in workspace mode does what single file mode leaves out: so the checks
    /// above can fail.
    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_workspace_mode_does_what_single_mode_leaves_out() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let session = crate::session::file(&state, &root);
        fs::create_dir_all(session.parent().unwrap()).unwrap();
        fs::write(&session, "not touched").unwrap();
        let (mut app, fake) = open_in(Mode::Workspace, &root.join("src/a.cpp"), &state);
        assert!(app.prompt.is_some(), "the trust dialog");
        app.press(K::Esc, M::NONE);
        assert!(app.ui.side_visible);
        assert!(app.settle(|_| !fake.launches().is_empty()).await, "a language server");
        assert!(app.settle(|a| a.explorer.is_loaded(&root)).await, "the root listed");
        assert!(
            app.settle(|_| crate::recent::file(&state).exists()).await,
            "a recent entry"
        );
        app.message = None;
        app.press(K::Char('b'), cmd());
        assert!(
            !app.ui.side_visible && app.message.is_none(),
            "Cmd+B works: side {} message {:?}",
            app.ui.side_visible,
            app.message.as_ref().map(|m| &m.text)
        );

        // Closing the last tab leaves jed open.
        app.press(K::Char('w'), cmd());
        assert!(
            app.group().tabs.is_empty(),
            "tabs {} prompt {} message {:?}",
            app.group().tabs.len(),
            app.prompt.is_some(),
            app.message.as_ref().map(|m| &m.text)
        );
        assert!(!app.quitting(), "workspace mode stays open with no tab");

        // A quit while the session's restore runs saves none (`save_session`): wait for it.
        assert!(app.settle(|a| !a.session.loading()).await, "the session restore done");
        app.test_execute(Command::Quit);
        assert!(app.quitting());
        app.shutdown().await;
        assert_ne!(
            fs::read_to_string(&session).unwrap(),
            "not touched",
            "the session written"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_refuses_the_side_and_bottom_panels() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let (mut app, _fake) = open_single(&root.join("src/a.cpp"), &state);
        let keys = [
            (K::Char('b'), cmd()),
            (K::Char('e'), cmd() | M::SHIFT),
            (K::Char('f'), cmd() | M::SHIFT),
            (K::Char('g'), M::CONTROL | M::SHIFT),
            (K::Char('j'), cmd()),
            (K::Char('p'), cmd()),
        ];
        for (code, mods) in keys {
            app.message = None;
            app.press(code, mods);
            assert!(!app.ui.side_visible && !app.ui.panel_visible, "{code:?} {mods:?}");
            assert!(app.palette.is_none(), "{code:?} {mods:?}: no quick open");
            assert_eq!(
                app.message.as_ref().map(|m| m.text.as_str()),
                Some(NOTICE),
                "{code:?} {mods:?}"
            );
        }

        // The icons do nothing when clicked.
        draw(&mut app, 80, 20);
        let icons = app.hits.icons.clone();
        assert_eq!(icons.len(), 4);
        for (y, _) in icons {
            app.mouse(Mouse::Down(MouseButton::Left), 0, y);
            app.mouse(Mouse::Up(MouseButton::Left), 0, y);
            assert!(!app.ui.side_visible, "icon at row {y}");
        }

        // The palette leaves them out, and its file mode finds nothing.
        app.test_execute(Command::ShowCommands);
        let titles: Vec<String> = app
            .palette
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect();
        assert!(!titles.is_empty());
        for gone in [
            Command::ToggleSidebar,
            Command::FindInFiles,
            Command::TogglePanel,
            Command::RunTask,
        ] {
            assert!(!titles.contains(&gone.title().to_string()), "{} listed", gone.title());
        }
        assert!(titles.contains(&Command::Save.title().to_string()));
        app.press(K::Backspace, M::NONE);
        assert!(app.palette.as_ref().unwrap().rows.is_empty());
        assert_eq!(app.palette.as_ref().unwrap().hint.as_deref(), Some(NOTICE));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_greys_out_the_activity_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let (mut app, _fake) = open_single(&root.join("src/a.cpp"), &state);
        let term = draw(&mut app, 80, 20);
        let disabled = app
            .theme
            .get("activityBar")
            .patch(app.theme.get("activityBar.disabled"));
        assert_ne!(disabled.fg, app.theme.get("activityBar").fg);
        let buf = term.backend().buffer();
        assert_eq!(app.hits.icons.len(), 4);
        for (y, panel) in app.hits.icons.clone() {
            let cell = &buf[(1, y)];
            assert_ne!(cell.symbol(), " ", "{panel:?}'s icon drawn");
            assert_eq!(cell.fg, disabled.fg.unwrap(), "{panel:?}'s icon greyed");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_creates_a_new_file_and_quits_on_the_last_close() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let path = root.join(".newrc");
        let (mut app, _fake) = open_single(&path, &state);
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), "");
        app.type_text("x = 1");
        app.press(K::Char('s'), cmd());
        assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = 1");
        app.press(K::Char('w'), cmd());
        assert!(app.quitting(), "closing the last tab quits");
    }

    /// Closing a tab while another is left (a diff against disk) doesn't quit.
    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_closing_a_diff_tab_keeps_the_file_open() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let (mut app, _fake) = open_single(&root.join("src/a.cpp"), &state);
        app.type_text("// ");
        app.test_execute(Command::CompareWithSaved);
        assert!(app.settle(|a| a.active_buffer().is_none()).await, "a diff tab active");
        app.press(K::Char('w'), cmd());
        assert!(!app.quitting() && app.prompt.is_none());
        assert!(app.active_buffer().is_some(), "back on the file");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_asks_before_closing_unsaved_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let path = root.join("src/a.cpp");
        let (mut app, _fake) = open_single(&path, &state);
        app.type_text("// ");
        app.press(K::Char('w'), cmd());
        assert!(app.prompt.is_some() && !app.quitting(), "the save prompt first");
        app.press(K::Char('s'), M::NONE);
        assert!(app.settle(|a| a.quitting()).await, "saved, then closed and quit");
        assert_eq!(fs::read_to_string(&path).unwrap(), "// int main() { return 0; }\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_file_mode_follows_changes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = dirs(&dir);
        let path = root.join("src/a.cpp");
        let (mut app, _fake) = open_single(&path, &state);
        // A clean buffer reloads.
        fs::write(&path, "int main() { return 1; }\n").unwrap();
        assert!(
            app.settle(|a| a.active_buffer().unwrap().rope == "int main() { return 1; }\n")
                .await
        );
        // Another file in its folder isn't watched.
        fs::write(root.join("src/b.cpp"), "int c;\n").unwrap();
        // An edited one gets the banner.
        app.type_text("x");
        fs::write(&path, "int main() { return 2; }\n").unwrap();
        assert!(app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await);
        assert!(
            !app.fs_flushed.contains(&root.join("src/b.cpp")),
            "{:?}",
            app.fs_flushed
        );
    }
}

/// The command palette and quick open list what was used recently, most recent first, while the
/// input is empty.
mod mru {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode as K, KeyModifiers as M};

    use crate::app::{App, InitialFile};
    use crate::buffer::Buffer;
    use crate::command::{Command, Motion};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// A workspace `<dir>/proj` with `files`, its trust store in `<dir>/state`.
    fn project(dir: &Path, files: &[&str]) -> PathBuf {
        let root = workspace::normalize(dir).join("proj");
        for name in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, format!("{name}\n")).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// The app on `root` with `first` open.
    fn open_app(root: &Path, first: &str) -> App {
        let initial = InitialFile {
            buffer: Buffer::open(&root.join(first)).unwrap(),
            goto: None,
        };
        let trust = TrustStore::load(root.parent().unwrap().join("state/trust.toml"));
        let mut app = App::new(Workspace::load(root.to_path_buf()), trust, Some(initial), true);
        app.test_start();
        app
    }

    /// Runs the palette command titled `title` (Cmd+Shift+P).
    fn palette_command(app: &mut App, title: &str) {
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text(title);
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, title);
        app.press(K::Enter, M::NONE);
        assert!(app.palette.is_none());
    }

    /// Cmd+P, the file's name, Enter once it's listed; waits until it's the active tab.
    async fn quick_open(app: &mut App, name: &str) {
        app.press(K::Char('p'), cmd());
        app.type_text(name);
        let listed = app
            .settle(|a| {
                a.palette
                    .as_ref()
                    .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
            })
            .await;
        assert!(listed, "quick open lists {name}");
        app.press(K::Enter, M::NONE);
        let opened = app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == name))
            .await;
        assert!(opened, "{name} opened");
    }

    /// Cmd+Shift+P on an empty query: the rows' labels, then Esc.
    fn command_rows(app: &mut App) -> Vec<String> {
        app.press(K::Char('p'), cmd() | M::SHIFT);
        let labels = app
            .palette
            .as_ref()
            .expect("the palette is open")
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect();
        app.press(K::Esc, M::NONE);
        assert!(app.palette.is_none());
        labels
    }

    /// Cmd+P on an empty query once the walk has filled the list: (label, detail) rows, then Esc.
    async fn file_rows(app: &mut App, n: usize) -> Vec<(String, String)> {
        app.press(K::Char('p'), cmd());
        let filled = app
            .settle(|a| a.palette.as_ref().is_some_and(|p| p.rows.len() >= n))
            .await;
        assert!(filled, "quick open lists {n} rows");
        let rows = app
            .palette
            .as_ref()
            .expect("the palette is open")
            .rows
            .iter()
            .map(|r| (r.label.clone(), r.detail.clone()))
            .collect();
        app.press(K::Esc, M::NONE);
        assert!(app.palette.is_none());
        rows
    }

    fn move_left() -> Command {
        Command::Move {
            motion: Motion::Left,
            select: false,
        }
    }

    fn move_right() -> Command {
        Command::Move {
            motion: Motion::Right,
            select: false,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn picked_commands_come_back_first_in_recency_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &["a.txt"]);
        let mut app = open_app(&root, "a.txt");
        palette_command(&mut app, "Cursor: Move Right");
        palette_command(&mut app, "Cursor: Move Left");
        assert_eq!(app.mru.commands, [move_left(), move_right()]);
        let rows = command_rows(&mut app);
        assert_eq!(&rows[..2], ["Cursor: Move Left", "Cursor: Move Right"]);
        assert_eq!(
            rows.iter().filter(|l| *l == "Cursor: Move Left").count(),
            1,
            "no duplicate"
        );
        let rest: Vec<&str> = rows[2..].iter().map(String::as_str).collect();
        assert!(
            rest.windows(2).all(|w| w[0] <= w[1]),
            "the rest stays alphabetical: {:?}",
            &rest[..rest.len().min(4)]
        );
        // A typed query keeps the fuzzy score: "Move Left" ranks the match, recently used or not.
        app.press(K::Char('p'), cmd() | M::SHIFT);
        app.type_text("Save");
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, "File: Save");
        app.press(K::Esc, M::NONE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opened_files_come_back_first_in_quick_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &["a.txt", "src/b.txt", "src/c.txt"]);
        let mut app = open_app(&root, "a.txt");
        quick_open(&mut app, "b.txt").await;
        quick_open(&mut app, "c.txt").await;
        let rows = file_rows(&mut app, 3).await;
        assert_eq!(
            &rows[..2],
            [("c.txt".to_string(), "src".into()), ("b.txt".to_string(), "src".into())]
        );
        // The walk's rows follow, without the files the recent rows show.
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["c.txt", "b.txt", "a.txt"]);
        assert_eq!(labels.iter().filter(|l| **l == "b.txt").count(), 1, "no duplicate");
        assert_eq!(app.mru.files, [PathBuf::from("src/c.txt"), PathBuf::from("src/b.txt")]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn files_stop_at_twenty() {
        let names: Vec<String> = (1..=22).map(|i| format!("f{i:02}.txt")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &names);
        let mut app = open_app(&root, "f01.txt");
        for name in &names[1..] {
            quick_open(&mut app, name).await;
        }
        assert_eq!(app.mru.files.len(), 20);
        assert_eq!(app.mru.files[0], PathBuf::from("f22.txt"));
        assert_eq!(app.mru.files[19], PathBuf::from("f03.txt"));
        assert!(!app.mru.files.contains(&PathBuf::from("f01.txt")), "the oldest went");
        assert!(!app.mru.files.contains(&PathBuf::from("f02.txt")));
        let rows = file_rows(&mut app, names.len()).await;
        let recent: Vec<&str> = rows.iter().take(20).map(|(l, _)| l.as_str()).collect();
        assert_eq!(recent[0], "f22.txt");
        assert_eq!(recent[19], "f03.txt");
        assert_eq!(rows.len(), names.len(), "20 recent + the 2 walk rows left");
        assert_eq!(rows.iter().filter(|(l, _)| l == "f22.txt").count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_run_from_a_key_stays_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &["a.txt"]);
        let mut app = open_app(&root, "a.txt");
        app.press(K::Char('b'), cmd());
        assert_eq!(app.last_command, Some(Command::ToggleSidebar));
        assert!(app.mru.commands.is_empty(), "a key's command isn't recently used");
        palette_command(&mut app, "Cursor: Move Right");
        assert_eq!(app.mru.commands, [move_right()]);
        assert!(!app.mru.commands.contains(&Command::ToggleSidebar));
        let rows = command_rows(&mut app);
        assert_eq!(rows[0], "Cursor: Move Right");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_open_is_not_recently_used() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &["a.txt"]);
        // A genuinely unopenable path (mode 000), outside the workspace so the walk can't list it.
        // (A missing file opens as an empty buffer in jed, so it is a successful open.)
        let unreadable = dir.path().join("unreadable.txt");
        fs::write(&unreadable, "x\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let mut app = open_app(&root, "a.txt");
        app.test_open_file(unreadable.clone());
        let failed = app
            .settle(|a| a.message.as_ref().is_some_and(|m| m.text.contains("unreadable.txt")))
            .await;
        assert!(failed, "the failed open reports an error");
        assert!(app.mru.files.is_empty(), "a failed open isn't recently used");
        // Quick open's empty list has no recent row for it either.
        let rows = file_rows(&mut app, 1).await;
        assert!(rows.iter().all(|(l, _)| l != "unreadable.txt"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_path_opened_through_a_different_spelling_is_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &["src/a.txt", "src/b.txt"]);
        let mut app = open_app(&root, "src/b.txt");
        // Open a.txt through a `..` spelling; the record normalizes it to `src/a.txt` (the
        // walk's spelling), so quick open doesn't show it twice.
        app.test_open_file(root.join("src/../src/a.txt"));
        let opened = app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "a.txt"))
            .await;
        assert!(opened, "a.txt opened");
        assert_eq!(app.mru.files, [PathBuf::from("src/a.txt")], "stored normalized");
        let rows = file_rows(&mut app, 2).await;
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["a.txt", "b.txt"]);
        assert_eq!(labels.iter().filter(|l| **l == "a.txt").count(), 1, "no duplicate");
    }
}

mod window_identity {
    use std::fs;
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::workspace::Workspace;
    use crate::workspace::config::StatusBarColor;
    use crate::workspace::trust::TrustStore;

    /// An app on `<dir>/ws` with `a.txt` open and `config` as its `.workspace`.
    fn open_app(dir: &tempfile::TempDir, config: &str) -> (App, PathBuf) {
        let base = crate::workspace::normalize(dir.path());
        let root = base.join("ws");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".workspace"), config).unwrap();
        let path = root.join("a.txt");
        fs::write(&path, "ab\n").unwrap();
        let ws = Workspace::load(root.clone());
        let trust = TrustStore::load(base.join("trust.toml"));
        let initial = crate::app::InitialFile {
            buffer: Buffer::open(&path).unwrap(),
            goto: None,
        };
        let mut app = App::new(ws, trust, Some(initial), true);
        app.test_start();
        (app, root)
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term
    }

    /// The status bar's background at its right end (never a warning / error / restricted cell).
    fn status_bg(term: &Terminal<TestBackend>) -> Color {
        let area = term.backend().buffer().area;
        term.backend().buffer()[(area.right() - 2, area.bottom() - 1)].bg
    }

    /// Whether any status bar cell has `bg` (the warning / error message cells' background).
    fn status_has_bg(term: &Terminal<TestBackend>, bg: Color) -> bool {
        let area = term.backend().buffer().area;
        let y = area.bottom() - 1;
        (area.x..area.right()).any(|x| term.backend().buffer()[(x, y)].bg == bg)
    }

    /// What `sync_window_title` sends (as `run` does each turn).
    fn sent_title(app: &mut App) -> String {
        let mut out = Vec::new();
        app.sync_window_title(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_status_bar_colour_is_drawn() {
        for colour in StatusBarColor::ALL {
            let dir = tempfile::tempdir().unwrap();
            let (mut app, _) = open_app(&dir, &format!("[window]\nstatus_bar = \"{}\"\n", colour.name()));
            assert_eq!(app.workspace.config.window.status_bar, colour);
            let term = draw(&mut app);
            assert_eq!(
                status_bg(&term),
                app.theme.get(colour.theme_key()).bg.unwrap(),
                "{}",
                colour.name()
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_reload_updates_the_title_and_the_colour() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, "[window]\ntitle = \"api\"\nstatus_bar = \"green\"\n");
        assert_eq!(
            app.identity.title_sequence(),
            "\x1b]2;jed api\x1b\\",
            "the start's title"
        );
        assert_eq!(sent_title(&mut app), "", "enter sent it already");
        assert_eq!(status_bg(&draw(&mut app)), app.theme.get("statusBar.green").bg.unwrap());

        fs::write(
            root.join(".workspace"),
            "[window]\ntitle = \"web\"\nstatus_bar = \"red\"\n",
        )
        .unwrap();
        assert!(
            app.settle(|a| a.workspace.config.window.status_bar == StatusBarColor::Red)
                .await
        );
        assert_eq!(sent_title(&mut app), "\x1b]2;jed web\x1b\\", "re-sent, not pushed");
        assert_eq!(sent_title(&mut app), "", "once");
        assert_eq!(status_bg(&draw(&mut app)), app.theme.get("statusBar.red").bg.unwrap());

        // A reload changing only another setting re-sends nothing.
        fs::write(
            root.join(".workspace"),
            "[window]\ntitle = \"web\"\nstatus_bar = \"red\"\n[settings]\ntab_width = 2\n",
        )
        .unwrap();
        assert!(app.settle(|a| a.settings.tab_width == 2).await);
        assert_eq!(sent_title(&mut app), "", "the title didn't change");

        // The override gone: the root's title and the default colour.
        fs::write(root.join(".workspace"), "").unwrap();
        assert!(app.settle(|a| a.identity.title_override().is_none()).await);
        let default = format!(
            "\x1b]2;jed {}\x1b\\",
            crate::recent::display(&root, &crate::paths::home())
        );
        assert_eq!(sent_title(&mut app), default);
        assert_eq!(
            status_bg(&draw(&mut app)),
            app.theme.get("statusBar.blue").bg.unwrap(),
            "the default colour"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_bad_colour_is_reported_and_keeps_the_last_good() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, "[window]\nstatus_bar = \"green\"\n");
        fs::write(root.join(".workspace"), "[window]\nstatus_bar = \"pink\"\n").unwrap();
        assert!(app.settle(|a| a.workspace.config_error.is_some()).await);
        assert_eq!(
            app.workspace.config.window.status_bar,
            StatusBarColor::Green,
            "the last good value kept"
        );
        assert_eq!(status_bg(&draw(&mut app)), app.theme.get("statusBar.green").bg.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn warning_and_error_cells_keep_their_colours() {
        // A tinted bar recolours only the base cells: the warning and error messages keep their
        // own keys (warning's amber on a red bar, error's red on a green bar).
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, "[window]\nstatus_bar = \"red\"\n");
        let warn = app.theme.get("statusBar.warning").bg.unwrap();
        let red = app.theme.get("statusBar.red").bg.unwrap();
        assert_ne!(warn, red, "warning and the base differ");
        app.message = Some(crate::app::Message {
            text: "careful".into(),
            level: crate::app::Level::Warning,
        });
        let term = draw(&mut app);
        assert!(status_has_bg(&term, warn), "the warning cell kept its colour");
        assert!(status_has_bg(&term, red), "the rest stays red");

        fs::write(root.join(".workspace"), "[window]\nstatus_bar = \"green\"\n").unwrap();
        assert!(
            app.settle(|a| a.workspace.config.window.status_bar == StatusBarColor::Green)
                .await
        );
        let err = app.theme.get("statusBar.error").bg.unwrap();
        let green = app.theme.get("statusBar.green").bg.unwrap();
        assert_ne!(err, green, "error and the base differ");
        app.message = Some(crate::app::Message {
            text: "broken".into(),
            level: crate::app::Level::Error,
        });
        let term = draw(&mut app);
        assert!(status_has_bg(&term, err), "the error cell kept its colour");
        assert!(status_has_bg(&term, green), "the rest stays green");
    }
}

/// Mouse in the hosted terminal (`term_mouse`): a shell or task tab whose program asked for the
/// mouse (`CSI ? 1000 / 1002 / 1003 h`, SGR `1006`) gets jed's clicks, drags, moves and wheel as
/// xterm's mouse input at the run's own cells — what `cat` echoes back — its left click also
/// focusing what it lands on; with the mouse off nothing reaches the program and the wheel still
/// scrolls the scrollback, and so does the wheel while the view is scrolled back. The encoder's
/// bytes are `tasks::mouse`'s unit tests.
mod term_mouse {
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
    use vt100::{MouseProtocolEncoding, MouseProtocolMode};

    use crate::app::App;
    use crate::command::Command;
    use crate::layout;
    use crate::shell_tests::{COUNT_TO_200, dirs, draw, screen_text, scrollback, shell_app};
    use crate::tasks::RunId;

    /// `command` (else nothing), then the shell's `printf` turning the mouse on in `modes`
    /// (`CSI ? n h`) and `cat`, which echoes every byte the mouse sends at it (ESC as `^[`).
    fn echo_mouse(command: Option<&str>, modes: &[u16]) -> Vec<u8> {
        let modes: String = modes.iter().map(|m| format!("\\033[?{m}h")).collect();
        let command = command.map(|c| format!("{c}; ")).unwrap_or_default();
        format!("{command}printf '{modes}'; cat\n").into_bytes()
    }

    /// The task whose template asks for the mouse in `modes` and then echoes what it gets.
    fn mouse_task(modes: &[u16]) -> String {
        let modes: String = modes.iter().map(|m| format!("\\033[?{m}h")).collect();
        format!("[tasks.mouse]\ncommand = '''printf '{modes}'; cat'''\n")
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// The shell's mode is on and its encoding SGR: the two the tests below assert.
    fn sgr(app: &App, mode: MouseProtocolMode) -> bool {
        let run = &app.group().active_shell().unwrap().run;
        let screen = run.screen();
        screen.screen().mouse_protocol_mode() == mode
            && screen.screen().mouse_protocol_encoding() == MouseProtocolEncoding::Sgr
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shell_mouse_clicks_drags_and_the_wheel_reach_its_program() {
        let (_d, root, state) = dirs();
        let mut app = shell_app(&root, &state);
        app.tasks.shell = "/bin/sh".into();
        app.test_execute(Command::NewShell);
        assert!(
            app.group()
                .active_shell()
                .unwrap()
                .run
                .write(&echo_mouse(Some(COUNT_TO_200), &[1000, 1002, 1006]))
        );
        assert!(
            app.settle(|a| sgr(a, MouseProtocolMode::ButtonMotion)).await,
            "the program asked for press / release, motion and SGR:\n{}",
            screen_text(&app)
        );

        let term = draw(&mut app, 100, 30);
        let editor = app.hits.group(app.groups.active).unwrap().editor;
        drop(term);
        // The editor's cell (2, 1) is the run's cell (3, 2).
        let (x, y) = (editor.x + 2, editor.y + 1);
        // A left click also focuses the group, as jed's own clicks do.
        app.ui.focus = crate::app::Focus::SidePanel;
        click(&mut app, x, y);
        assert_eq!(
            app.ui.focus,
            crate::app::Focus::Editor,
            "the click focused the shell's group"
        );
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x + 1, y);
        app.mouse(MouseEventKind::ScrollUp, x + 1, y);
        app.mouse(MouseEventKind::ScrollDown, x + 1, y);
        assert!(
            app.settle(|a| {
                let text = screen_text(a);
                text.contains("^[[<0;3;2M")
                    && text.contains("^[[<0;3;2m")
                    && text.contains("^[[<32;4;2M")
                    && text.contains("^[[<64;4;2M")
                    && text.contains("^[[<65;4;2M")
            })
            .await,
            "the cat echoed the click, the drag and the wheel as SGR bytes:\n{}",
            screen_text(&app)
        );
        assert_eq!(
            scrollback(&app),
            0,
            "the wheel went to the program, not through the scrollback"
        );

        // Scrolled back, a click stays in jed and the wheel scrolls the scrollback again.
        let live = screen_text(&app);
        app.press(KeyCode::PageUp, KeyModifiers::SHIFT);
        let back = scrollback(&app);
        assert!(back > 0, "Shift+PageUp went back:\n{}", screen_text(&app));
        click(&mut app, x, y);
        app.mouse(MouseEventKind::ScrollUp, x, y);
        assert_eq!(scrollback(&app), back + 3, "the wheel scrolled the scrollback");
        app.press(KeyCode::End, KeyModifiers::CONTROL);
        assert_eq!(
            screen_text(&app),
            live,
            "no mouse byte reached the program while scrolled back"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn task_mouse_clicks_and_motion_reach_its_program() {
        let dir = tempfile::tempdir().unwrap();
        let (root, state) = crate::milestone12_tests::dirs(&dir);
        let mut app = crate::milestone12_tests::open_app(&root, &state, Some(&mouse_task(&[1000, 1003, 1006])));
        // A frame first, as the app always has: the run starts at its tab's size.
        drop(draw(&mut app, 100, 30));
        let id: RunId = app.run_task("mouse").unwrap();
        assert!(
            app.settle(|a| {
                let screen = a.tasks.get(id).unwrap().screen();
                screen.screen().mouse_protocol_mode() == MouseProtocolMode::AnyMotion
                    && screen.screen().mouse_protocol_encoding() == MouseProtocolEncoding::Sgr
            })
            .await,
            "the task asked for any motion and SGR:\n{}",
            task_text(&app, id)
        );

        let term = draw(&mut app, 100, 30);
        let area = layout::task_area(app.hits.regions.panel.unwrap());
        drop(term);
        // The task area's cell (4, 2) is the run's cell (5, 3).
        let (x, y) = (area.x + 4, area.y + 2);
        app.ui.focus = crate::app::Focus::SidePanel;
        click(&mut app, x, y);
        // Any-motion mode sends moves with no button held (code 35).
        app.mouse(MouseEventKind::Moved, x + 2, y);
        app.mouse(MouseEventKind::ScrollDown, x + 2, y);
        assert!(
            app.settle(|a| {
                let text = task_text(a, id);
                text.contains("^[[<0;5;3M")
                    && text.contains("^[[<0;5;3m")
                    && text.contains("^[[<35;7;3M")
                    && text.contains("^[[<65;7;3M")
            })
            .await,
            "the task echoed the click, the move and the wheel as SGR bytes:\n{}",
            task_text(&app, id)
        );
        let run = app.tasks.get(id).unwrap();
        assert_eq!(run.screen().screen().scrollback(), 0, "the wheel went to the task");
        assert_eq!(
            app.ui.focus,
            crate::app::Focus::Panel,
            "the click kept the task tab's keys"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mouse_off_leaves_clicks_alone_and_the_wheel_scrolls() {
        let (_d, root, state) = dirs();
        let mut app = shell_app(&root, &state);
        app.tasks.shell = "/bin/sh".into();
        app.test_execute(Command::NewShell);
        assert!(
            app.group()
                .active_shell()
                .unwrap()
                .run
                .write(format!("{COUNT_TO_200}\n").as_bytes())
        );
        assert!(
            app.settle(|a| screen_text(a).lines().any(|l| l.trim_end().ends_with("199")))
                .await,
            "the command's output arrived:\n{}",
            screen_text(&app)
        );
        assert_eq!(
            app.group()
                .active_shell()
                .unwrap()
                .run
                .screen()
                .screen()
                .mouse_protocol_mode(),
            MouseProtocolMode::None,
            "the program never asked for the mouse"
        );

        let term = draw(&mut app, 100, 30);
        let editor = app.hits.group(app.groups.active).unwrap().editor;
        drop(term);
        let (x, y) = (editor.x + 2, editor.y + 1);
        let before = screen_text(&app);
        click(&mut app, x, y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x + 1, y);
        assert_eq!(
            screen_text(&app),
            before,
            "clicks reached neither the shell's program nor its output"
        );
        assert!(!screen_text(&app).contains("^[["), "nothing was sent as a mouse report");

        // The wheel is jed's: it still scrolls the shell's scrollback.
        app.mouse(MouseEventKind::ScrollUp, x, y);
        assert_eq!(scrollback(&app), 3, "the wheel scrolled the scrollback");
        assert!(
            !screen_text(&app).contains("^[["),
            "the wheel sent no mouse report:\n{}",
            screen_text(&app)
        );
    }

    fn task_text(app: &App, id: RunId) -> String {
        app.tasks.get(id).unwrap().screen().screen().contents()
    }
}
