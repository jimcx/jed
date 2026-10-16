//! The palette: input, hint and list, at the top centre as VS Code's quick input.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::layout::PaletteHit;
use crate::palette::Mode;

/// List rows shown at once.
const MAX_ROWS: u16 = 14;

/// Draws the palette if it's open; returns where, for the mouse.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Option<PaletteHit> {
    let theme = &app.theme;
    let p = app.palette.as_mut()?;
    let base = theme.get("dialog");
    let dim = base.patch(theme.get("palette.detail"));
    let hl = theme.get("palette.highlight");
    let input = base.patch(theme.get("dialog.input"));

    let width = (area.width * 3 / 5).clamp(60.min(area.width), area.width);
    let hint_rows = u16::from(p.hint.is_some());
    // Borders, input, hint; whatever is left (up to MAX_ROWS) for the list.
    let room = area.height.saturating_sub(3 + hint_rows + 1);
    let list_cap = room.min(MAX_ROWS);
    let shown = (p.rows.len() as u16).min(list_cap);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + 1.min(area.height),
        width,
        height: (3 + hint_rows + shown).min(area.height),
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(base.patch(theme.get("dialog.border")))
        .style(base);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let out = f.buffer_mut();
    let w = inner.width as usize;
    // Input row, full width.
    out.set_style(Rect { height: 1, ..inner }, input);
    let placeholder = match p.mode() {
        Mode::Files if p.field.text.is_empty() => Some("Search files by name (> for commands, : for a line)"),
        Mode::LogLevel if p.field.text.is_empty() => Some("Log level or filter"),
        Mode::History if p.field.text.is_empty() => Some("Pick an entry (type to filter by time or size)"),
        Mode::Tasks if p.field.text.is_empty() => Some("Select the task to run"),
        _ => None,
    };
    match placeholder {
        Some(text) => out.set_stringn(inner.x + 1, inner.y, text, w.saturating_sub(1), input.patch(dim)),
        None => out.set_stringn(inner.x + 1, inner.y, &p.field.text, w.saturating_sub(1), input),
    };
    let cursor_x = inner.x + 1 + p.field.text[..p.field.cursor].width() as u16;
    f.set_cursor_position(Position {
        x: cursor_x.min(inner.x + inner.width - 1),
        y: inner.y,
    });

    let out = f.buffer_mut();
    let mut y = inner.y + 1;
    if let Some(hint) = &p.hint
        && y < inner.y + inner.height
    {
        out.set_stringn(inner.x + 1, y, hint, w.saturating_sub(1), dim);
        y += 1;
    }

    let rows_area = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: (inner.y + inner.height).saturating_sub(y),
    };
    p.height = list_cap.max(1) as usize;
    p.scroll = p.scroll.min(p.rows.len().saturating_sub(p.height));
    let selected_style = theme.get("list.active");
    for (i, row) in p.rows.iter().enumerate().skip(p.scroll).take(rows_area.height as usize) {
        let ry = rows_area.y + (i - p.scroll) as u16;
        let selected = i == p.selected;
        let row_base = if selected { base.patch(selected_style) } else { base };
        let row_rect = Rect {
            y: ry,
            height: 1,
            ..rows_area
        };
        out.set_style(row_rect, row_base);

        let keys_w = row.keys.as_ref().map_or(0, |k| k.width() + 1);
        let mut spans = vec![Span::styled(" ", row_base)];
        spans.extend(highlighted(&row.label, &row.label_hits, row_base, row_base.patch(hl)));
        if !row.detail.is_empty() {
            let detail_base = row_base.patch(theme.get("palette.detail"));
            spans.push(Span::styled("  ", detail_base));
            spans.extend(highlighted(
                &row.detail,
                &row.detail_hits,
                detail_base,
                detail_base.patch(hl),
            ));
        }
        let room = w.saturating_sub(keys_w);
        out.set_line(rows_area.x, ry, &Line::from(spans), room as u16);
        if let Some(keys) = &row.keys
            && keys_w < w
        {
            let kx = rows_area.x + (w - keys_w) as u16;
            out.set_stringn(kx, ry, keys, keys_w, row_base.patch(theme.get("palette.detail")));
        }
    }
    Some(PaletteHit {
        area: rect,
        rows: rows_area,
    })
}

/// `text` as spans, with the chars at `hits` (sorted char indices) in `hit`.
fn highlighted(text: &str, hits: &[usize], normal: Style, hit: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_hit = false;
    let mut next = hits.iter().peekable();
    for (i, c) in text.chars().enumerate() {
        let is_hit = next.peek() == Some(&&i);
        if is_hit {
            next.next();
        }
        if is_hit != run_hit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_hit { hit } else { normal },
            ));
        }
        run_hit = is_hit;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_hit { hit } else { normal }));
    }
    spans
}
