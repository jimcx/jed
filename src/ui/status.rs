//! Status bar: trust mode, file, message, cursor position, file format.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Level};
use crate::buffer::pos;
use crate::workspace::trust::Trust;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let style = app.theme.get("statusBar");
    let out = f.buffer_mut();
    out.set_style(area, style);
    let width = area.width as usize;
    let mut x = area.x;

    if app.trust == Trust::Restricted {
        let (nx, _) = out.set_stringn(x, area.y, " Restricted ", width, app.theme.get("statusBar.restricted"));
        x = nx;
    }

    let mut right_x = area.x + area.width;
    if let (Some(buf), Some(tab)) = (app.active_buffer(), app.group().active_tab()) {
        let dirty = if buf.is_dirty() { " ●" } else { "" };
        let flag = if buf.deleted {
            " [deleted]"
        } else if buf.read_only.is_some() {
            " [read-only]"
        } else {
            ""
        };
        let left = format!(" {}{dirty}{flag}  ", buf.name());

        // As VS Code: the position for one selection, the count for several.
        let count = tab.view.selections.len();
        let position = if count > 1 {
            format!("{count} selections")
        } else {
            let head = tab.view.selections.primary().head;
            let line = pos::line_of(&buf.rope, head) + 1;
            let col = pos::display_col(&buf.rope, head, app.settings.tab_width).0 + 1;
            format!("Ln {line}, Col {col}")
        };
        let indent = if app.settings.insert_spaces {
            format!("Spaces: {}", app.settings.tab_width)
        } else {
            format!("Tab Size: {}", app.settings.tab_width)
        };
        let encoding = if buf.bom { "UTF-8 with BOM" } else { "UTF-8" };
        let right = format!("{position}   {indent}   {encoding}   {} ", buf.eol.label());

        let right_w = right.width().min(width);
        right_x = area.x + (width - right_w) as u16;
        out.set_stringn(right_x, area.y, &right, right_w, style);
        let room = right_x.saturating_sub(x) as usize;
        x = out.set_stringn(x, area.y, &left, room, style).0;
    }

    if let Some(msg) = &app.message {
        let msg_style = match msg.level {
            Level::Info => style,
            Level::Warning => app.theme.get("statusBar.warning"),
            Level::Error => app.theme.get("statusBar.error"),
        };
        let room = right_x.saturating_sub(x + 1) as usize;
        out.set_stringn(x, area.y, format!(" {} ", msg.text), room, msg_style);
    }
}
