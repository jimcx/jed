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
use crate::tasks::RunId;

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
    let running = run.status().is_running();
    let theme = &app.theme;
    let base = theme.get("panel");
    let below_tabs = Rect {
        y: content.y,
        height: content.height,
        ..panel
    };
    f.buffer_mut()
        .set_style(below_tabs, base.patch(theme.get("terminal.background")));

    let parser = run.screen();
    let screen = Shifted {
        screen: parser.screen(),
        top: top_row(parser.screen(), content.height),
    };
    // The terminal cursor (not tui-term's drawn one) while the tab has the keys.
    let term = PseudoTerminal::new(&screen).cursor(Cursor::default().visibility(false));
    f.render_widget(term, content);
    let out = f.buffer_mut();
    for y in content.y..content.y + content.height {
        for x in content.x..content.x + content.width {
            theme.terminal_cell(&mut out[(x, y)]);
        }
    }
    let back = parser.screen().scrollback();
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
        out.set_stringn(x, info.y, &note, w as usize, base.patch(theme.get("panel.dim")));
    }
    if focused && running && back == 0 && !parser.screen().hide_cursor() {
        let (row, col) = tui_term::widget::Screen::cursor_position(&screen);
        if row < content.height && col < content.width {
            f.set_cursor_position(Position {
                x: content.x + col,
                y: content.y + row,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
