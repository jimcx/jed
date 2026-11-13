//! Drawing a `TextField` as one input row: scrolled sideways to keep the cursor in view, with a
//! placeholder when empty and the selection in its own style (`TextField::offset_at_col` maps a
//! click back through the returned scroll).

use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

use crate::field::TextField;

/// One input as drawn: where its cursor is, and the sideways scroll used to keep the cursor
/// in view (the first drawn character's display column).
#[derive(Debug, Clone, Copy)]
pub struct DrawnField {
    pub cursor: Position,
    pub scroll: usize,
}

/// One input, scrolled sideways to keep the cursor in view (all of it from column 0 when it
/// fits; a placeholder when empty), its selected text in `selected`; returns where its cursor is
/// and its scroll, for mapping mouse columns.
pub fn draw_field(
    out: &mut ratatui::buffer::Buffer,
    r: Rect,
    field: &TextField,
    placeholder: &str,
    style: Style,
    selected: Style,
    dim: Style,
) -> DrawnField {
    out.set_style(r, style);
    if field.text.is_empty() {
        // Dimmed, but on the field's own fill: `dim` is the dialog's / side bar's colour, whose
        // background would otherwise paint the hint a different shade from the input.
        let hint = Style {
            bg: style.bg,
            ..style.patch(dim)
        };
        out.set_stringn(r.x, r.y, placeholder, r.width as usize, hint);
        return DrawnField {
            cursor: Position { x: r.x, y: r.y },
            scroll: 0,
        };
    }
    let width = r.width as usize;
    let cursor_col: usize = field.text[..field.cursor].chars().map(|c| c.width().unwrap_or(0)).sum();
    let scroll = (cursor_col + 1).saturating_sub(width);
    let sel = field.selection().unwrap_or_default();
    let mut col = 0;
    for (i, c) in field.text.char_indices() {
        let w = c.width().unwrap_or(0);
        if col >= scroll && col + w - scroll <= width {
            let s = if sel.contains(&i) { selected } else { style };
            out.set_stringn(r.x + (col - scroll) as u16, r.y, c.to_string(), w, s);
        }
        col += w;
    }
    DrawnField {
        cursor: Position {
            x: r.x + (cursor_col - scroll).min(width.saturating_sub(1)) as u16,
            y: r.y,
        },
        scroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    /// The hint is dimmed text, but the field's background stays behind it (not the dialog's).
    #[test]
    fn placeholder_keeps_the_fields_background() {
        let input = Style::default().fg(Color::White).bg(Color::Blue);
        let dim = Style::default().fg(Color::Gray).bg(Color::Red);
        let mut out = Buffer::empty(Rect::new(0, 0, 12, 1));
        draw_field(
            &mut out,
            Rect::new(0, 0, 12, 1),
            &TextField::default(),
            "Find",
            input,
            input,
            dim,
        );
        assert_eq!(out[(0, 0)].bg, Color::Blue, "the placeholder keeps the field's fill");
        assert_eq!(out[(0, 0)].fg, Color::Gray, "and stays dimmed");
    }
}
