//! A task tab's terminal in the bottom panel: the run's `vt100` screen drawn with `tui-term`,
//! its colours mapped through the theme (`Theme::terminal_cell`: Dark+'s terminal palette), with
//! the mouse selection (`tasks::select`) in `editor.selection`'s background over them.
//!
//! Render-time state written back: every running task's PTY is resized to the tab's area
//! (`layout::task_area`, its last column kept for the scroll bar `render` draws in
//! `layout::task_bar`), and `app.tasks.size` is set to it. A screen taller than the area (an
//! ended run that started bigger) is drawn from the row that keeps its last line in view.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use tui_term::widget::{Cursor, PseudoTerminal};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
use crate::layout::{self, PanelTab, ScrollBarHit, ScrollTarget};
use crate::scrollbar::Scrollable;
use crate::shell::ShellTab;
use crate::tasks::select::{top_row, viewport_row};
use crate::tasks::{Run, RunId};
use crate::theme::Theme;

/// A `vt100` cell for `tui-term` (which implements its `Cell` only for the `vt100` 0.16 it
/// depends on, not the fork's 0.18, so jed does it itself).
struct TermCell(vt100::Cell);

impl tui_term::widget::Cell for TermCell {
    fn has_contents(&self) -> bool {
        self.0.has_contents()
    }

    fn apply(&self, cell: &mut ratatui::buffer::Cell) {
        use ratatui::style::{Color, Modifier, Style};
        let c = &self.0;
        if c.has_contents() {
            cell.set_symbol(c.contents());
        }
        let map = |color: vt100::Color| match color {
            vt100::Color::Default => Color::Reset,
            vt100::Color::Idx(i) => Color::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        };
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
        cell.set_style(
            Style::reset()
                .fg(map(c.fgcolor()))
                .bg(map(c.bgcolor()))
                .add_modifier(modifier),
        );
    }
}

/// The `width` x `height` cells of a `vt100` screen seen from row `top` down (`tui-term` draws
/// from row 0), cloned (it hands out references to its own cell type).
struct Shifted {
    cells: Vec<Option<TermCell>>,
    width: u16,
    cursor: (u16, u16),
    hide_cursor: bool,
}

impl Shifted {
    fn new(screen: &vt100::Screen, top: u16, width: u16, height: u16) -> Self {
        let cells = (0..height)
            .flat_map(|row| (0..width).map(move |col| (row, col)))
            .map(|(row, col)| screen.cell(row + top, col).cloned().map(TermCell))
            .collect();
        let (row, col) = screen.cursor_position();
        Self {
            cells,
            width,
            cursor: (row.saturating_sub(top), col),
            hide_cursor: screen.hide_cursor(),
        }
    }
}

impl tui_term::widget::Screen for Shifted {
    type C = TermCell;

    fn cell(&self, row: u16, col: u16) -> Option<&TermCell> {
        if col >= self.width {
            return None;
        }
        self.cells
            .get(row as usize * self.width as usize + col as usize)?
            .as_ref()
    }

    fn hide_cursor(&self) -> bool {
        self.hide_cursor
    }

    fn cursor_position(&self) -> (u16, u16) {
        self.cursor
    }
}

/// Draws `run`'s screen in `area` (the caller fills its background): `Shifted` from `top_row`
/// through `PseudoTerminal` (its drawn cursor off), every cell through `theme.terminal_cell`, the
/// mouse selection (`tasks::select`) in `editor.selection`'s background, and the terminal cursor
/// while `focused`, running, live and not hidden. Returns the scrollback (0 = the live screen).
/// `status()` is read before `screen()`: the spawn holds the state lock while it takes the
/// screen's (the reverse order deadlocks on a run that is still starting).
fn draw_screen(f: &mut Frame, area: Rect, run: &Run, theme: &Theme, focused: bool) -> usize {
    let running = run.status().is_running();
    let parser = run.screen();
    let top = top_row(parser.screen(), area.height);
    let screen = Shifted::new(parser.screen(), top, area.width, area.height);
    // The terminal cursor (not tui-term's drawn one) while the tab has the keys.
    let term = PseudoTerminal::new(&screen).cursor(Cursor::default().visibility(false));
    f.render_widget(term, area);
    let out = f.buffer_mut();
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
    if focused && running && back == 0 && !parser.screen().hide_cursor() {
        let (row, col) = tui_term::widget::Screen::cursor_position(&screen);
        if row < area.height && col < area.width {
            f.set_cursor_position(Position {
                x: area.x + col,
                y: area.y + row,
            });
        }
    }
    back
}

/// Draws run `id` in the panel `panel` (border row included); `info` is what's left of the tab
/// strip's row: the "scrolled back" note, else the command (and its folder when not the root),
/// dimmed. Returns the run's scroll bar (`None` when the area is too small).
pub fn render(f: &mut Frame, panel: Rect, info: Rect, app: &mut App, id: RunId) -> Option<ScrollBarHit> {
    let content = layout::task_area(panel);
    if content.width == 0 || content.height == 0 {
        return None;
    }
    app.tasks.size = (content.height, content.width);
    for run in app.tasks.runs() {
        if run.status().is_running() {
            run.resize(content.height, content.width);
        }
    }
    let run = app.tasks.get(id)?;
    let focused =
        app.ui.focus == Focus::Panel && app.prompt.is_none() && app.palette.is_none() && app.cheat_sheet.is_none();
    let theme = &app.theme;
    let base = theme.get("panel");
    let below_tabs = Rect {
        y: content.y,
        height: content.height,
        ..panel
    };
    f.buffer_mut()
        .set_style(below_tabs, base.patch(theme.get("terminal.background")));

    let back = draw_screen(f, content, run, theme, focused);
    // The scroll bar in the panel's last column (`task_area` kept it), over the terminal's
    // background.
    let look = super::scrollbar::look(app, ScrollTarget::Panel(PanelTab::Task(id)));
    let bar = super::scrollbar::render(
        f.buffer_mut(),
        layout::task_bar(panel),
        run.scrollable().scroll(),
        look,
        base.patch(theme.get("terminal.background")),
        theme,
    );
    let note = if back > 0 {
        format!("{back} lines back (Shift+PageDown, Ctrl+End)")
    } else {
        let command = run.command.replace(['\n', '\r', '\t'], " ");
        match run.cwd.strip_prefix(&app.workspace.root) {
            Ok(rel) if rel.as_os_str().is_empty() => command,
            Ok(rel) => format!("{command}  (in {})", rel.display()),
            Err(_) => format!("{command}  (in {})", run.cwd.display()),
        }
    };
    if info.width > 4 {
        let w = (note.width() as u16).min(info.width - 4);
        let x = info.x + info.width - w - 1;
        f.buffer_mut()
            .set_stringn(x, info.y, &note, w as usize, base.patch(theme.get("panel.dim")));
    }
    Some(bar)
}

/// Draws a shell tab's screen in `area` (the editor area less its scroll bar column: no tab
/// strip or info line). The caller resizes the run to `area` each frame and draws the bar;
/// `focused` shows the terminal cursor only on the live screen, as a task tab does. No `App`: the
/// caller still holds the group mutably.
pub fn render_shell(f: &mut Frame, area: Rect, shell: &ShellTab, theme: &Theme, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.buffer_mut()
        .set_style(area, theme.get("editor").patch(theme.get("terminal.background")));
    draw_screen(f, area, &shell.run, theme, focused);
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::tasks::{RunId, Status};

    /// A taller screen is drawn from `top_row` (`tasks::select`): row 2 of the widget is screen
    /// row 6, and the cursor moves up with it.
    #[test]
    fn a_shifted_screen_draws_from_its_top_row() {
        let mut p = crate::tasks::select::test_parser(10, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        p.process(b"\r\n\r\n\r\n\r\nseven\r\n");
        let shifted = Shifted::new(p.screen(), 4, 20, 6);
        let cell = tui_term::widget::Screen::cell(&shifted, 2, 0).unwrap();
        assert_eq!(cell.0.contents(), "s");
        assert_eq!(tui_term::widget::Screen::cursor_position(&shifted), (3, 0));
    }

    /// `TermCell` hands every cell's look to the buffer as tui-term's own `vt100` mapping did:
    /// indexed and RGB colours, default as `Reset`, bold / italic / underline / inverse / dim, a
    /// style reset between cells, a wide character with its continuation cell left alone, and
    /// `Shifted`'s window (`top`, the area's width) cut from the screen.
    #[test]
    fn term_cells_reach_the_buffer_with_their_look() {
        use ratatui::style::{Color, Modifier};
        use ratatui::widgets::Widget;
        let mut p = crate::tasks::select::test_parser(4, 10, 0);
        p.process(b"skip\r\n");
        p.process(b"\x1b[1;3;4;7;31;48;5;200mA\x1b[0m\x1b[2;38;2;1;2;3mB\x1b[0m\xe4\xb8\xadx\r\n");
        p.process(b"0123456789");
        let shifted = Shifted::new(p.screen(), 1, 8, 2);
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        // what was drawn there before: each cell's style is replaced, not patched
        buf.set_style(
            area,
            ratatui::style::Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
        PseudoTerminal::new(&shifted)
            .cursor(Cursor::default().visibility(false))
            .render(area, &mut buf);

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
        assert!(tui_term::widget::Screen::cell(&shifted, 0, 8).is_none());
        assert!(tui_term::widget::Screen::cell(&shifted, 2, 0).is_none());
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
            1,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while shell.run.status() != Status::Running {
            assert!(std::time::Instant::now() < deadline, "the shell started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let theme = Theme::dark_plus(true);
        std::thread::scope(|s| {
            let _state = shell.run.test_hold_state();
            let (started, rx) = std::sync::mpsc::channel();
            let (draw_shell, draw_theme) = (&shell, &theme);
            s.spawn(move || {
                let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 10)).unwrap();
                started.send(()).unwrap();
                term.draw(|f| render_shell(f, Rect::new(0, 0, 20, 10), draw_shell, draw_theme, true))
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
