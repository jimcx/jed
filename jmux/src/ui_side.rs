//! Drawing the commands side bar (jed's `ui/tools.rs` is its model): the filter box on the first
//! row, the history's rows below it (glyph, command line, the terminal's tab title, how long it
//! ran or has been running, with its age when there is room), a scroll bar in the last column,
//! and a line saying what is missing while there are no rows. Writes the `SideHit`.

use std::time::Instant;

use jtui::scrollbar::Scroll;
use jtui::scrollbar_draw::{self, Look};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Drag};
use crate::command::Command;
use crate::hit::SideHit;
use crate::side::{self, row_cols};

/// Width from which a finished row also shows how long ago it ended.
const AGE_FROM: usize = 44;

/// The theme key of a row's glyph.
fn glyph_key(glyph: char) -> &'static str {
    match glyph {
        '●' => "commands.running",
        '✓' => "commands.ok",
        '✗' => "commands.failed",
        _ => "sideBar.dim",
    }
}

/// Draws the bar in `area` (its border column, `border`, is drawn by the caller).
pub fn render(f: &mut Frame, app: &mut App, area: Rect, border: Rect) -> SideHit {
    let mut hit = SideHit {
        area,
        border,
        ..SideHit::default()
    };
    if area.width < 4 || area.height < 2 {
        return hit;
    }
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let dim = base.patch(theme.get("sideBar.dim"));
    let focused = app.side.focused && app.confirm.is_none();
    let scrolling = matches!(app.drag, Some(Drag::SideScroll { .. }));
    let out = f.buffer_mut();
    out.set_style(area, base);

    // The filter box.
    let field = Rect::new(area.x + 1, area.y, area.width - 2, 1);
    let input = theme.get("dialog.input");
    let selected = theme.get("editor.selection");
    let drawn = jtui::field_draw::draw_field(out, field, &app.side.filter, "Filter", input, selected, dim);
    hit.filter = field;
    hit.filter_scroll = drawn.scroll;
    if focused {
        f.set_cursor_position(Position::new(drawn.cursor.x, drawn.cursor.y));
    }
    let out = f.buffer_mut();

    // The rows.
    let mut list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let track = scrollbar_draw::take_column(&mut list);
    let height = list.height as usize;
    app.side.view_rows = height;
    app.history.reveal_selected(height);
    app.history.scroll_by(0, height);
    let rows = app.history.visible();
    let first = app.history.scroll();
    let selected_key = app.history.selected().map(|r| r.key());
    let now = Instant::now();
    if let Some(track) = track {
        let look = if scrolling { Look::Active } else { Look::Normal };
        let scroll = Scroll::rows(rows.len(), height, first);
        hit.scroll = scrollbar_draw::render(out, track, scroll, look, base, theme);
    }
    hit.list = list;
    hit.first = first;
    if rows.is_empty() {
        let text = if app.history.is_empty() {
            let key = app.key_label(Command::InstallShellIntegration);
            side::empty_text(app.integration_missing, &key)
        } else {
            "No matching commands".to_string()
        };
        for (i, line) in side::wrap(&text, list.width.saturating_sub(2) as usize)
            .iter()
            .take(height)
            .enumerate()
        {
            out.set_stringn(list.x + 1, list.y + i as u16, line, list.width as usize - 1, dim);
        }
        return hit;
    }
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };
    let width = list.width as usize;
    for (i, row) in rows.iter().enumerate().skip(first).take(height) {
        let y = list.y + (i - first) as u16;
        let rect = Rect { y, height: 1, ..list };
        let mut style = base;
        if selected_key == Some(row.key()) {
            style = base.patch(selected_style);
            out.set_style(rect, style);
        }
        let closed = theme.get("commands.closed");
        let (text, mark, faint) = if row.closed {
            (style.patch(closed), style.patch(closed), style.patch(closed))
        } else {
            (
                style,
                style.patch(theme.get(glyph_key(row.glyph()))),
                style.patch(theme.get("sideBar.dim")),
            )
        };
        let time = if row.running() {
            row.duration_text(now)
        } else {
            match row.age_text(now) {
                Some(age) if width >= AGE_FROM => format!("{} · {age}", row.duration_text(now)),
                _ => row.duration_text(now),
            }
        };
        let cols = row_cols(width, time.width(), row.title.width());
        let x = list.x;
        out.set_stringn(x + 1, y, row.glyph().to_string(), 1, mark);
        out.set_stringn(x + 3, y, side::fit(row.label(), cols.command), cols.command, text);
        if let Some((tx, tw)) = cols.title {
            out.set_stringn(x + tx as u16, y, side::fit(&row.title, tw), tw, faint);
        }
        out.set_stringn(x + cols.time as u16, y, &time, width.saturating_sub(cols.time), faint);
        hit.shown += 1;
    }
    hit
}
