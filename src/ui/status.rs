//! Status bar: file, message, cursor position, file format.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Level};
use crate::buffer::pos;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let buf = &app.buffer;
    let style = app.theme.get("statusBar");
    let out = f.buffer_mut();
    out.set_style(area, style);

    let name = buf
        .path
        .file_name()
        .map_or_else(|| buf.path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let dirty = if buf.is_dirty() { " ●" } else { "" };
    let read_only = if buf.read_only.is_some() { " [read-only]" } else { "" };
    let left = format!(" {name}{dirty}{read_only}  ");

    let head = app.view.selections.primary().head;
    let line = pos::line_of(&buf.rope, head) + 1;
    let col = pos::display_col(&buf.rope, head, app.settings.tab_width).0 + 1;
    let count = app.view.selections.len();
    let sels = if count > 1 {
        format!(" ({count} selections)")
    } else {
        String::new()
    };
    let indent = if app.settings.insert_spaces {
        format!("Spaces: {}", app.settings.tab_width)
    } else {
        format!("Tab Size: {}", app.settings.tab_width)
    };
    let encoding = if buf.bom { "UTF-8 with BOM" } else { "UTF-8" };
    let right = format!(
        "Ln {line}, Col {col}{sels}   {indent}   {encoding}   {} ",
        buf.eol.label()
    );

    let width = area.width as usize;
    let right_w = right.width().min(width);
    let right_x = area.x + (width - right_w) as u16;
    out.set_stringn(right_x, area.y, &right, right_w, style);
    let (x, _) = out.set_stringn(area.x, area.y, &left, width.saturating_sub(right_w), style);

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
