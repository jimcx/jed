//! A task tab's terminal in the bottom panel: the run's `vt100` screen drawn with `tui-term`,
//! its colours mapped through the theme (`Theme::terminal_cell`: Dark+'s terminal palette).
//!
//! Render-time state written back: every running task's PTY is resized to the tab's area
//! (`layout::task_area`), and `app.tasks.size` is set to it. A screen taller than the area (an
//! ended run that started bigger) is drawn from the row that keeps its last line in view.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use tui_term::widget::{Cursor, PseudoTerminal};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
use crate::layout;
use crate::shell::ShellTab;
use crate::tasks::{Run, RunId};
use crate::theme::Theme;

/// A `vt100` screen seen from row `top` down (`tui-term` draws from row 0).
struct Shifted<'a> {
    screen: &'a vt100::Screen,
    top: u16,
}

impl tui_term::widget::Screen for Shifted<'_> {
    type C = vt100::Cell;

    fn cell(&self, row: u16, col: u16) -> Option<&vt100::Cell> {
        self.screen.cell(row + self.top, col)
    }

    fn hide_cursor(&self) -> bool {
        self.screen.hide_cursor()
    }

    fn cursor_position(&self) -> (u16, u16) {
        let (row, col) = self.screen.cursor_position();
        (row.saturating_sub(self.top), col)
    }
}

/// The first row to draw of a `screen` shown in `height` rows: 0 when it fits, else as low as
/// keeps its last non-blank row and the cursor in view.
fn top_row(screen: &vt100::Screen, height: u16) -> u16 {
    let (rows, cols) = screen.size();
    if rows <= height {
        return 0;
    }
    let last = screen
        .rows(0, cols)
        .enumerate()
        .filter(|(_, text)| !text.trim_end().is_empty())
        .map(|(i, _)| i as u16)
        .last()
        .unwrap_or(0)
        .max(screen.cursor_position().0);
    (last + 1).saturating_sub(height).min(rows - height)
}

/// Draws `run`'s screen in `area` (the caller fills its background): `Shifted` from `top_row`
/// through `PseudoTerminal` (its drawn cursor off), every cell through `theme.terminal_cell`, and
/// the terminal cursor while `focused`, running, live and not hidden. Returns the scrollback (0 =
/// the live screen). `status()` is read before `screen()`: the spawn holds the state lock while it
/// takes the screen's (the reverse order deadlocks on a run that is still starting).
fn draw_screen(f: &mut Frame, area: Rect, run: &Run, theme: &Theme, focused: bool) -> usize {
    let running = run.status().is_running();
    let parser = run.screen();
    let screen = Shifted {
        screen: parser.screen(),
        top: top_row(parser.screen(), area.height),
    };
    // The terminal cursor (not tui-term's drawn one) while the tab has the keys.
    let term = PseudoTerminal::new(&screen).cursor(Cursor::default().visibility(false));
    f.render_widget(term, area);
    let out = f.buffer_mut();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            theme.terminal_cell(&mut out[(x, y)]);
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
/// dimmed.
pub fn render(f: &mut Frame, panel: Rect, info: Rect, app: &mut App, id: RunId) {
    let content = layout::task_area(panel);
    if content.width == 0 || content.height == 0 {
        return;
    }
    app.tasks.size = (content.height, content.width);
    for run in app.tasks.runs() {
        if run.status().is_running() {
            run.resize(content.height, content.width);
        }
    }
    let Some(run) = app.tasks.get(id) else { return };
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
}

/// Draws a shell tab's screen in `area` (the whole editor area: no tab strip or info line, no
/// scroll bar column). The caller resizes the run to `area` each frame; `focused` shows the
/// terminal cursor only on the live screen, as a task tab does. No `App`: the caller still holds
/// the group mutably.
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

    #[test]
    fn a_taller_screen_keeps_its_last_line_in_view() {
        let mut p = vt100::Parser::new(10, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        assert_eq!(top_row(p.screen(), 10), 0, "fits");
        assert_eq!(top_row(p.screen(), 4), 0, "the output is at the top");
        assert_eq!(top_row(p.screen(), 2), 1);
        p.process(b"\r\n\r\n\r\n\r\nseven\r\n");
        assert_eq!(top_row(p.screen(), 4), 4, "the cursor below the last line");
        let shifted = Shifted {
            screen: p.screen(),
            top: 4,
        };
        let cell = tui_term::widget::Screen::cell(&shifted, 2, 0).unwrap();
        assert_eq!(cell.contents(), "s");
        assert_eq!(tui_term::widget::Screen::cursor_position(&shifted), (3, 0));
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
