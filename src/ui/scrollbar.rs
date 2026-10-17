//! An editor group's vertical scroll bar: one column at the editor's right edge, the editor's
//! background with the thumb (`scrollbar::Scroll::thumb`) as cells of `scrollbarSlider.*`.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use crate::layout::ScrollBarHit;
use crate::scrollbar::Scroll;
use crate::theme::Theme;

/// Which slider colour the thumb takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Normal,
    /// The pointer is over the thumb.
    Hover,
    /// Being dragged (shown even when the drag reaches the top of a short text).
    Active,
}

/// Draws the bar on `track` (one column) for `scroll`; returns it for the mouse.
pub fn render(out: &mut Cells, track: Rect, mut scroll: Scroll, look: Look, theme: &Theme) -> ScrollBarHit {
    let base = theme.get("editor");
    out.set_style(track, base);
    scroll.shown |= look == Look::Active;
    let thumb = scroll.thumb(track.height);
    if let Some(t) = thumb {
        let key = match look {
            Look::Normal => "scrollbarSlider.background",
            Look::Hover => "scrollbarSlider.hoverBackground",
            Look::Active => "scrollbarSlider.activeBackground",
        };
        let rect = Rect {
            y: track.y + t.start,
            height: t.len,
            ..track
        };
        out.set_style(rect, base.patch(theme.get(key)));
    }
    ScrollBarHit { track, thumb }
}
