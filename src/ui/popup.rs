//! Popups anchored at a buffer position in an editor view: the hover (`ui::hover`) and the
//! completion list (`ui::completion`). `cell_of` finds the anchor's screen cell, `place` the popup's rect
//! (clipped to the editor), `frame` draws its border and fill. The caller draws the content and
//! returns the rect for `HitMap` (mouse events on it don't fall through).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};
use crate::view::View;

/// Which side of the anchor's row a popup goes on when it fits there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Above (the hover, as VS Code).
    Above,
    /// Below (the completion list).
    Below,
}

/// The screen cell of `at` in `view`, whose text was drawn in `text`; `None` when its line is
/// off screen. A column scrolled off to either side is pulled to the text's edge.
pub fn cell_of(rope: &Rope, view: &View, text: Rect, at: ByteIdx, tab_width: usize) -> Option<(u16, u16)> {
    if text.width == 0 || text.height == 0 {
        return None;
    }
    let line = pos::line_of(rope, at);
    let top = view.top_line();
    if line < top || line >= top + text.height as usize {
        return None;
    }
    let col = pos::display_col(rope, at, tab_width).0;
    let x = col.saturating_sub(view.left_col).min(text.width as usize - 1);
    Some((text.x + x as u16, text.y + (line - top) as u16))
}

/// Where a `width` × `height` popup (border included) goes for the anchor cell in `area`: on the
/// `prefer` side of the anchor's row if it fits there, else on the other side if it fits there,
/// else on the side with more room, its height cut to that. Its left edge is at the anchor,
/// moved left to stay in `area` (its width cut to `area`'s). `None` without room for a border
/// and one row.
pub fn place(area: Rect, (ax, ay): (u16, u16), width: u16, height: u16, prefer: Side) -> Option<Rect> {
    let above = ay.saturating_sub(area.y);
    let below = (area.y + area.height).saturating_sub(ay + 1);
    let (first, second) = match prefer {
        Side::Above => ((Side::Above, above), (Side::Below, below)),
        Side::Below => ((Side::Below, below), (Side::Above, above)),
    };
    let (side, room) = if first.1 >= height {
        first
    } else if second.1 >= height {
        second
    } else if first.1 >= second.1 {
        first
    } else {
        second
    };
    let height = height.min(room);
    let width = width.min(area.width);
    if height < 3 || width < 3 {
        return None;
    }
    let y = match side {
        Side::Above => ay - height,
        Side::Below => ay + 1,
    };
    let x = ax.min(area.x + area.width - width).max(area.x);
    Some(Rect { x, y, width, height })
}

/// Fills `rect` and draws its border; returns the inside. Cells are reset first: styles only
/// patch, so the text's modifiers and underline colour (squiggles) would show through.
pub fn frame(out: &mut Buffer, rect: Rect, fill: Style, border: Style) -> Rect {
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            out[(x, y)].reset();
            out[(x, y)].set_symbol(" ").set_style(fill);
        }
    }
    let (r, b) = (rect.x + rect.width - 1, rect.y + rect.height - 1);
    let border = fill.patch(border);
    for x in rect.x + 1..r {
        out.set_stringn(x, rect.y, "─", 1, border);
        out.set_stringn(x, b, "─", 1, border);
    }
    for y in rect.y + 1..b {
        out.set_stringn(rect.x, y, "│", 1, border);
        out.set_stringn(r, y, "│", 1, border);
    }
    out.set_stringn(rect.x, rect.y, "┌", 1, border);
    out.set_stringn(r, rect.y, "┐", 1, border);
    out.set_stringn(rect.x, b, "└", 1, border);
    out.set_stringn(r, b, "┘", 1, border);
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width - 2,
        height: rect.height - 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_prefers_its_side_then_the_other_then_the_roomier() {
        let area = Rect::new(10, 0, 40, 20);
        // Room above row 8 for 6 rows: above, left edge at the anchor.
        assert_eq!(place(area, (15, 8), 12, 6, Side::Above), Some(Rect::new(15, 2, 12, 6)));
        // Not above row 3 (3 rows): below.
        assert_eq!(place(area, (15, 3), 12, 6, Side::Above), Some(Rect::new(15, 4, 12, 6)));
        assert_eq!(place(area, (15, 3), 12, 6, Side::Below), Some(Rect::new(15, 4, 12, 6)));
        // Below doesn't fit at row 17: above.
        assert_eq!(
            place(area, (15, 17), 12, 6, Side::Below),
            Some(Rect::new(15, 11, 12, 6))
        );
        // Fits neither side (8 above, 11 below): the roomier, cut.
        assert_eq!(
            place(area, (15, 8), 12, 15, Side::Above),
            Some(Rect::new(15, 9, 12, 11))
        );
        // Near the right edge: moved left; too wide: cut to the area.
        assert_eq!(place(area, (45, 8), 12, 6, Side::Above), Some(Rect::new(38, 2, 12, 6)));
        assert_eq!(place(area, (45, 8), 60, 6, Side::Above), Some(Rect::new(10, 2, 40, 6)));
        // No room for a border and a row.
        assert_eq!(place(Rect::new(0, 0, 40, 3), (5, 1), 12, 6, Side::Above), None);
    }

    #[test]
    fn anchor_cells_follow_the_view() {
        let rope = Rope::from_str("ab\n\tcd\nlast\n");
        let mut view = View::new();
        let text = Rect::new(5, 1, 10, 2);
        assert_eq!(cell_of(&rope, &view, text, ByteIdx(1), 4), Some((6, 1)));
        assert_eq!(cell_of(&rope, &view, text, ByteIdx(4), 4), Some((9, 2)), "after a tab");
        assert_eq!(cell_of(&rope, &view, text, ByteIdx(7), 4), None, "below the view");
        view.left_col = 3;
        assert_eq!(
            cell_of(&rope, &view, text, ByteIdx(1), 4),
            Some((5, 1)),
            "pulled to the edge"
        );
    }
}
