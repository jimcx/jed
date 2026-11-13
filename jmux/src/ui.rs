//! Drawing the server's `App` into a frame: the shell above a one-line status bar.

use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, DETACH_HINT};

/// Rows the status bar takes.
pub const STATUS_ROWS: u16 = 1;

/// The area the shell gets in a `rows` × `cols` screen.
pub fn shell_area(rows: u16, cols: u16) -> Rect {
    Rect::new(0, 0, cols, rows.saturating_sub(STATUS_ROWS))
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let shell_area = shell_area(area.height, area.width);
    if let Some(shell) = &app.shell {
        // The run follows the area every frame (a no-op while the size is the same).
        shell.run.resize(shell_area.height, shell_area.width);
        jterm::draw::render_shell(f, shell_area, shell, &app.theme, true, None);
    }
    if area.height > shell_area.height {
        let bar = Rect::new(0, shell_area.height, area.width, STATUS_ROWS);
        let title = app.shell.as_ref().map_or("", |s| s.title.as_str());
        status_bar(f, bar, &app.name, title, &app.theme);
    }
}

/// ` <session>  <title>` on the left, the detach hint on the right.
fn status_bar(f: &mut Frame, bar: Rect, name: &str, title: &str, theme: &Theme) {
    let (style, detail) = (theme.get("statusBar"), theme.get("statusBar.detail"));
    let buf = f.buffer_mut();
    buf.set_style(bar, style);
    let width = bar.width as usize;
    let hint = format!("{DETACH_HINT} ");
    let left = format!(" {name}");
    let (x, _) = buf.set_stringn(bar.x, bar.y, &left, width, style);
    let right_x = bar.x + bar.width.saturating_sub(hint.len() as u16);
    if right_x > x + 2 {
        buf.set_stringn(right_x, bar.y, &hint, width, detail);
        let room = (right_x - x).saturating_sub(3) as usize;
        buf.set_stringn(x + 2, bar.y, title, room, detail);
    }
}
