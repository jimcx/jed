//! Drawing a terminal screen into a ratatui buffer: the `vt100` cells with their look (`draw_cells`,
//! each colour mapped through `Theme::terminal_cell`), the mouse selection in `editor.selection`'s
//! background, the search's matches and bar (`draw_search`), and the terminal cursor (`draw_screen`);
//! `render_shell` draws a shell tab's screen in an area. The app draws its own frame (tab strip,
//! info line, scroll bar) and calls these. The colours come from the theme's keys in `KEYS`: an
//! app's theme must define them (`terminal.*` and the widget keys are in `jtui::theme::widget_entries`).

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::run::Run;
use crate::search::{self, TermSearch};
use crate::select::{top_row, viewport_row};
use crate::shell::ShellTab;

/// The theme keys the drawing reads (besides `terminal_cell`'s `terminal.*` ones, `render_shell`'s
/// `editor` and `terminal.background`): an app's theme must define each, or `Theme::get` logs
/// `missing theme key`.
pub const KEYS: &[&str] = &[
    "editor",
    "editor.selection",
    "editor.findMatchHighlight",
    "editor.findMatch",
    "dialog",
    "dialog.input",
    "palette.detail",
    "find.option",
    "find.option.active",
    "terminal.background",
];

/// Draws the cells of `screen` seen from row `top` down into `area` of `buf`, each cell cleared
/// first (what was drawn there before is gone) and then given its symbol (not for a wide
/// character's continuation cell, which keeps the blank) and its colours and modifiers. A cell
/// the screen doesn't have (past its rows or columns) stays cleared. Nothing is copied.
pub fn draw_cells(buf: &mut ratatui::buffer::Buffer, area: Rect, screen: &vt100::Screen, top: u16) {
    use ratatui::style::{Color, Modifier, Style};
    let map = |color: vt100::Color| match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    };
    for row in 0..area.height {
        for col in 0..area.width {
            let out = &mut buf[(area.x + col, area.y + row)];
            out.reset();
            let Some(c) = screen.cell(row + top, col) else {
                continue;
            };
            if c.has_contents() {
                out.set_symbol(c.contents());
            }
            let mut modifier = Modifier::empty();
            for (on, m) in [
                (c.bold(), Modifier::BOLD),
                (c.italic(), Modifier::ITALIC),
                (c.underline(), Modifier::UNDERLINED),
                (c.inverse(), Modifier::REVERSED),
                (c.dim(), Modifier::DIM),
            ] {
                if on {
                    modifier |= m;
                }
            }
            out.set_style(
                Style::reset()
                    .fg(map(c.fgcolor()))
                    .bg(map(c.bgcolor()))
                    .add_modifier(modifier),
            );
        }
    }
}

/// The terminal cursor's `(row, col)` in an area drawn from row `top` of `screen`.
pub fn cursor_in(screen: &vt100::Screen, top: u16) -> (u16, u16) {
    let (row, col) = screen.cursor_position();
    (row.saturating_sub(top), col)
}

/// Draws `run`'s screen in `area` (the caller fills its background): the cells from `top_row`
/// (`draw_cells`), every cell through `theme.terminal_cell`, the
/// mouse selection (`select`) in `editor.selection`'s background, and the terminal cursor
/// while `focused`, running, live and not hidden. Returns the scrollback (0 = the live screen).
/// `status()` is read before `screen()`: the spawn holds the state lock while it takes the
/// screen's (the reverse order deadlocks on a run that is still starting).
pub fn draw_screen(
    f: &mut Frame,
    area: Rect,
    run: &Run,
    theme: &jtui::theme::Theme,
    focused: bool,
    search: Option<&TermSearch>,
) -> usize {
    let running = run.status().is_running();
    let parser = run.screen();
    let top = top_row(parser.screen(), area.height);
    let out = f.buffer_mut();
    draw_cells(out, area, parser.screen(), top);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            theme.terminal_cell(&mut out[(x, y)]);
        }
    }
    if let Some(selection) = run.selection()
        && let Some(bg) = theme.get("editor.selection").bg
    {
        // `contents_between`'s cells: the first row from its start column, the middle rows the
        // whole width, the last up to its end column; only the rows drawn here are painted.
        let ((start_line, start_col), (end_line, end_col)) = selection.rows();
        let back = parser.screen().scrollback();
        let (start_row, end_row) = (viewport_row(start_line, back), viewport_row(end_line, back));
        let (first, last) = (top as isize, (top + area.height) as isize);
        for row in start_row.max(first)..=end_row.min(last - 1) {
            let (from, to) = match (row == start_row, row == end_row) {
                (true, true) => (start_col, end_col),
                (true, false) => (start_col, area.width),
                (false, true) => (0, end_col),
                (false, false) => (0, area.width),
            };
            let y = area.y + (row - first) as u16;
            for col in from..to.min(area.width) {
                out[(area.x + col, y)].set_bg(bg);
            }
        }
    }
    let back = parser.screen().scrollback();
    if let Some(search) = search {
        draw_search(f, area, parser.screen(), top, search, theme, focused);
    }
    if focused && search.is_none() && running && back == 0 && !parser.screen().hide_cursor() {
        let (row, col) = cursor_in(parser.screen(), top);
        if row < area.height && col < area.width {
            f.set_cursor_position(Position {
                x: area.x + col,
                y: area.y + row,
            });
        }
    }
    back
}

/// The search bar's width at most (`" query  Aa  12 of 345 "`).
pub const SEARCH_WIDTH: u16 = 44;

/// The search's matches on the rows shown (`editor.findMatchHighlight`, the current one in
/// `editor.findMatch`) and its bar at the top right of `area`: the query, the case toggle and
/// "3 of 12". With the keys (`focused`) the terminal cursor sits in the query.
pub fn draw_search(
    f: &mut Frame,
    area: Rect,
    screen: &vt100::Screen,
    top: u16,
    search: &TermSearch,
    theme: &jtui::theme::Theme,
    focused: bool,
) {
    let back = screen.scrollback();
    let out = f.buffer_mut();
    let (all, current) = (theme.get("editor.findMatchHighlight"), theme.get("editor.findMatch"));
    for m in search::find_visible(screen, &search.field.text, search.case) {
        let row = viewport_row(m.line, back) - top as isize;
        if !(0..area.height as isize).contains(&row) {
            continue;
        }
        let style = if search.current == Some(m) { current } else { all };
        if let Some(bg) = style.bg {
            for col in m.start..m.end.min(area.width) {
                out[(area.x + col, area.y + row as u16)].set_bg(bg);
            }
        }
    }
    if area.width < 20 {
        return;
    }
    let width = area.width.min(SEARCH_WIDTH);
    let rect = Rect::new(area.x + area.width - width, area.y, width, 1);
    let base = theme.get("dialog");
    let input = base.patch(theme.get("dialog.input"));
    let dim = base.patch(theme.get("palette.detail"));
    f.render_widget(ratatui::widgets::Clear, rect);
    let out = f.buffer_mut();
    out.set_style(rect, base);
    let count = search.label();
    let count_w = (count.width() as u16).min(12);
    // " [query] Aa count "
    let field_w = width.saturating_sub(1 + 3 + count_w + 2).max(4);
    let field = Rect::new(rect.x + 1, rect.y, field_w, 1);
    let drawn = jtui::field_draw::draw_field(
        out,
        field,
        &search.field,
        "Find in terminal",
        input,
        input.patch(theme.get("editor.selection")),
        dim,
    );
    let toggle = if search.case {
        "find.option.active"
    } else {
        "find.option"
    };
    out.set_stringn(field.x + field_w + 1, rect.y, "Aa", 2, base.patch(theme.get(toggle)));
    out.set_stringn(field.x + field_w + 4, rect.y, &count, count_w as usize, dim);
    if focused {
        f.set_cursor_position(drawn.cursor);
    }
}

/// Draws a shell tab's screen in `area` (the editor area less its scroll bar column: no tab
/// strip or info line). The caller resizes the run to `area` each frame and draws the bar;
/// `focused` shows the terminal cursor only on the live screen, as a task tab does. No `App`: the
/// caller still holds the group mutably.
pub fn render_shell(
    f: &mut Frame,
    area: Rect,
    shell: &ShellTab,
    theme: &jtui::theme::Theme,
    focused: bool,
    search: Option<&TermSearch>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.buffer_mut()
        .set_style(area, theme.get("editor").patch(theme.get("terminal.background")));
    draw_screen(
        f,
        area,
        &shell.run,
        theme,
        focused,
        search.filter(|s| s.run == shell.run.id),
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::run::{RunId, Status};

    const START_OPTIONS: crate::run::StartOptions = crate::run::StartOptions {
        env: crate::run::EnvNames {
            ipc: "TEST_IPC",
            terminal: "TEST_TERMINAL",
        },
        command_ring: 50,
    };

    /// The shared widget keys with Dark+ colours, as an app's theme starts from.
    fn theme() -> jtui::theme::Theme {
        jtui::theme::Theme::new(jtui::theme::widget_entries(true), true)
    }

    /// The widget keys plus distinct backgrounds for the editor keys the drawing reads, so a
    /// wrong key paints a wrong colour.
    fn full_theme() -> jtui::theme::Theme {
        use ratatui::style::{Color, Style};
        let bg = |r| Style::new().bg(Color::Rgb(r, 0, 0));
        let own = [
            ("editor", Style::new()),
            ("editor.selection", bg(10)),
            ("editor.findMatchHighlight", bg(20)),
            ("editor.findMatch", bg(30)),
            ("dialog", Style::new().bg(Color::Rgb(40, 0, 0))),
            ("palette.detail", Style::new().fg(Color::Rgb(0, 50, 0))),
            ("find.option", Style::new()),
            ("find.option.active", Style::new()),
        ];
        jtui::theme::Theme::new(jtui::theme::widget_entries(true).into_iter().chain(own), true)
    }

    /// The search paints its matches in `editor.findMatchHighlight` and the current one in
    /// `editor.findMatch`, only on the rows shown, and draws its bar at the top right.
    #[test]
    fn search_matches_and_bar_are_drawn() {
        use ratatui::style::Color;
        let mut p = crate::select::test_parser(4, 60, 0);
        p.process(b"pad\r\nfoo bar\r\nbar foo");
        let mut search = TermSearch::new(RunId(1));
        search.field.text = "foo".into();
        search.field.cursor = 3;
        let hits = search::find_visible(p.screen(), "foo", false);
        assert_eq!(hits.len(), 2);
        search.current = Some(hits[1]);
        search.count = 2;
        search.index = Some(1);
        let theme = full_theme();
        let area = Rect::new(0, 0, 60, 4);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 4)).unwrap();
        term.draw(|f| draw_search(f, area, p.screen(), 0, &search, &theme, true))
            .unwrap();
        let buf = term.backend().buffer();
        assert_eq!(buf[(0, 1)].bg, Color::Rgb(20, 0, 0), "the first match is a plain match");
        assert_eq!(buf[(2, 1)].bg, Color::Rgb(20, 0, 0));
        assert_eq!(buf[(3, 1)].bg, Color::Reset, "past the match nothing is painted");
        assert_eq!(
            buf[(4, 2)].bg,
            Color::Rgb(30, 0, 0),
            "the current match has its own colour"
        );
        assert_eq!(buf[(6, 2)].bg, Color::Rgb(30, 0, 0));
        // the bar (44 wide, top right): the dialog's background, the count in it
        let row: String = (0..60).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row.contains("2 of 2"), "{row:?}");
        assert_eq!(buf[(59, 0)].bg, Color::Rgb(40, 0, 0));
        assert_eq!(buf[(15, 0)].bg, Color::Reset);
        // focused: the terminal cursor sits in the query, after "foo"
        term.backend_mut()
            .assert_cursor_position(ratatui::layout::Position { x: 16 + 1 + 3, y: 0 });
    }

    /// A shell running `program` (`/bin/cat` stays up and prints nothing; `/bin/true` ends at once,
    /// waited for), `rows` x 20, with `scrollback` rows of history.
    async fn shell(program: &str, rows: u16, scrollback: usize) -> (ShellTab, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let shell = ShellTab::new(
            RunId(2),
            crate::shell::spec(Path::new(program), dir.path(), None),
            rows,
            20,
            Arc::new(|_| {}),
            scrollback,
            START_OPTIONS,
            1,
        );
        let ends = program == "/bin/true";
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while if ends {
            shell.run.status().is_running()
        } else {
            shell.run.status() != Status::Running
        } {
            assert!(std::time::Instant::now() < deadline, "the shell reached its state");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (shell, dir)
    }

    fn bg_row(buf: &ratatui::buffer::Buffer, y: u16) -> Vec<ratatui::style::Color> {
        (0..20).map(|x| buf[(x, y)].bg).collect()
    }

    /// The mouse selection is painted in `editor.selection`'s background: the first row from its
    /// start column, middle rows the whole width, the last up to its end column (exclusive); with
    /// the screen taller than the area, only the rows shown are painted.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_multi_row_selection_is_painted_row_by_row() {
        use ratatui::style::Color;
        let base = Color::Rgb(0x1E, 0x1E, 0x1E);
        let sel = Color::Rgb(10, 0, 0);
        let theme = full_theme();
        for other in ["editor.findMatch", "editor.findMatchHighlight"] {
            assert_ne!(
                theme.get(other).bg,
                Some(sel),
                "{other} is told apart from the selection"
            );
        }
        assert_eq!(theme.get("editor.selection").bg, Some(sel));
        let (shell, _dir) = shell("/bin/cat", 5, 0).await;
        shell.run.screen().process(b"a\r\nb\r\nc\r\nd\r\ne");
        let draw = |height: u16| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, height)).unwrap();
            term.draw(|f| render_shell(f, Rect::new(0, 0, 20, height), &shell, &theme, false, None))
                .unwrap();
            term.backend().buffer().clone()
        };
        let row = |from: u16, to: u16| -> Vec<Color> {
            (0..20)
                .map(|x| if (from..to).contains(&x) { sel } else { base })
                .collect()
        };
        // lines 0..=2, the whole screen shown
        shell.run.set_selection(crate::select::Selection {
            anchor: (0, 3),
            head: (2, 5),
        });
        let buf = draw(5);
        assert_eq!(bg_row(&buf, 0), row(3, 20), "the first row from its start column");
        assert_eq!(bg_row(&buf, 1), row(0, 20), "a middle row in full");
        assert_eq!(bg_row(&buf, 2), row(0, 5), "the last row up to its end column");
        assert_eq!(bg_row(&buf, 3), row(0, 0), "nothing after it");
        // 3 rows shown of 5, from screen row 2: line 1 is above the area; line 2 (a middle row)
        // is drawn whole and line 3 (the last) up to column 5
        shell.run.set_selection(crate::select::Selection {
            anchor: (1, 3),
            head: (3, 5),
        });
        let buf = draw(3);
        assert_eq!(buf[(0, 0)].symbol(), "c", "drawn from row 2");
        assert_eq!(
            bg_row(&buf, 0),
            row(0, 20),
            "the row above isn't drawn, the next is whole"
        );
        assert_eq!(bg_row(&buf, 1), row(0, 5));
        assert_eq!(bg_row(&buf, 2), row(0, 0));
        shell.run.kill();
    }

    /// The terminal cursor is shown at the screen's cursor only while focused, running and on the
    /// live screen: otherwise a cursor the backend already had is left alone (hidden).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_cursor_is_shown_only_when_focused_running_and_live() {
        use ratatui::layout::Position;
        let theme = full_theme();
        let known = Position { x: 17, y: 4 };
        let drawn = |shell: &ShellTab, focused: bool| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 5)).unwrap();
            term.set_cursor_position(known).unwrap();
            term.draw(|f| render_shell(f, Rect::new(0, 0, 20, 5), shell, &theme, focused, None))
                .unwrap();
            term.get_cursor_position().unwrap()
        };
        let (live, _dir) = shell("/bin/cat", 5, 100).await;
        live.run.screen().process(b"hello world\r\nsecond");
        assert_eq!(drawn(&live, true), Position { x: 6, y: 1 }, "focused, running, live");
        assert_eq!(drawn(&live, false), known, "not focused");
        // scrolled back: 20 lines went into the history
        for i in 0..20 {
            live.run.screen().process(format!("\r\nline {i}").as_bytes());
        }
        live.run.screen().screen_mut().set_scrollback(3);
        assert!(
            live.run.screen().screen().scrollback() > 0,
            "the screen is scrolled back"
        );
        assert_eq!(drawn(&live, true), known, "scrolled back");
        live.run.kill();

        let (ended, _dir2) = shell("/bin/true", 5, 0).await;
        assert!(!ended.run.status().is_running());
        assert_eq!(drawn(&ended, true), known, "not running");
    }

    /// A taller screen is drawn from `top_row` (`tasks::select`): row 2 of the widget is screen
    /// row 6, and the cursor moves up with it.
    #[test]
    fn a_shifted_screen_draws_from_its_top_row() {
        let mut p = crate::select::test_parser(10, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        p.process(b"\r\n\r\n\r\n\r\nseven\r\n");
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        draw_cells(&mut buf, area, p.screen(), 4);
        assert_eq!(buf[(0, 2)].symbol(), "s");
        assert_eq!(cursor_in(p.screen(), 4), (3, 0));
    }

    /// `draw_cells` hands every cell's look to the buffer:
    /// indexed and RGB colours, default as `Reset`, bold / italic / underline / inverse / dim, a
    /// style reset between cells, a wide character with its continuation cell left alone, and
    /// the window (`top`, the area's width) cut from the screen.
    #[test]
    fn term_cells_reach_the_buffer_with_their_look() {
        use ratatui::style::{Color, Modifier};
        let mut p = crate::select::test_parser(4, 10, 0);
        p.process(b"skip\r\n");
        p.process(b"\x1b[1;3;4;7;31;48;5;200mA\x1b[0m\x1b[2;38;2;1;2;3mB\x1b[0m\xe4\xb8\xadx\r\n");
        p.process(b"0123456789");
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        // what was drawn there before: each cell's style is replaced, not patched
        buf.set_style(
            area,
            ratatui::style::Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
        draw_cells(&mut buf, area, p.screen(), 1);

        let a = &buf[(0, 0)];
        assert_eq!(a.symbol(), "A");
        assert_eq!((a.fg, a.bg), (Color::Indexed(1), Color::Indexed(200)));
        // (vt100 keeps one intensity: bold and dim can't both be on)
        let all = Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED | Modifier::REVERSED;
        assert_eq!(a.modifier, all);

        let b = &buf[(1, 0)];
        assert_eq!(b.symbol(), "B");
        assert_eq!((b.fg, b.bg), (Color::Rgb(1, 2, 3), Color::Reset));
        assert_eq!(b.modifier, Modifier::DIM);

        assert_eq!(buf[(2, 0)].symbol(), "中");
        assert_eq!(buf[(3, 0)].symbol(), " ", "a continuation cell isn't drawn over");
        assert_eq!(buf[(4, 0)].symbol(), "x");
        assert_eq!(buf[(4, 0)].fg, Color::Reset);
        assert_eq!(buf[(4, 0)].modifier, Modifier::empty());
        // row 1 is the screen's row 2, cut at the area's 8 columns
        let row: String = (0..8).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert_eq!(row, "01234567");
        // an area wider and taller than the screen leaves the cells past it cleared
        let big = Rect::new(0, 0, 12, 5);
        let mut buf = ratatui::buffer::Buffer::empty(big);
        buf.set_style(big, ratatui::style::Style::default().fg(Color::Green));
        draw_cells(&mut buf, big, p.screen(), 1);
        assert_eq!(buf[(10, 0)].symbol(), " ");
        assert_eq!(buf[(10, 0)].fg, Color::Reset);
        assert_eq!(buf[(0, 4)].fg, Color::Reset);
    }

    /// A shell drawn while it is still starting must take the state lock before the screen's: the
    /// spawn holds the state while it takes the screen's, so the reverse order stopped both (the
    /// UI froze on a new shell's first frame). With the state held, the draw must wait for it
    /// without holding the screen.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_draw_waits_for_the_state_without_holding_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ShellTab::new(
            RunId(1),
            crate::shell::spec(Path::new("/bin/cat"), dir.path(), None),
            10,
            20,
            Arc::new(|_| {}),
            0,
            START_OPTIONS,
            1,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while shell.run.status() != Status::Running {
            assert!(std::time::Instant::now() < deadline, "the shell started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let theme = theme();
        std::thread::scope(|s| {
            let _state = shell.run.test_hold_state();
            let (started, rx) = std::sync::mpsc::channel();
            let (draw_shell, draw_theme) = (&shell, &theme);
            s.spawn(move || {
                let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 10)).unwrap();
                started.send(()).unwrap();
                term.draw(|f| render_shell(f, Rect::new(0, 0, 20, 10), draw_shell, draw_theme, true, None))
                    .unwrap();
            });
            rx.recv_timeout(Duration::from_secs(2)).expect("the draw started");
            // `/bin/cat` prints nothing, so only the draw could hold the screen here.
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(5));
                assert!(
                    shell.run.test_screen_free(),
                    "the draw held the screen while it waited for the state"
                );
            }
            // The closure ends, dropping the state guard; the scope then waits for the draw.
        });
        shell.run.kill();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while shell.run.status().is_running() {
            assert!(std::time::Instant::now() < deadline, "the shell ended");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
