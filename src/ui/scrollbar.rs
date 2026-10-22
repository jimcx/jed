//! Scroll bars: one column (a row for the editors' horizontal bar) at the right edge of an editor or a list, the background of
//! what it scrolls with the thumb (`scrollbar::Scroll::thumb`) as cells of `scrollbarSlider.*`.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::App;
use crate::layout::{ScrollBarHit, ScrollTarget};
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

/// How `target`'s thumb looks: dragged, under the pointer, or neither.
pub fn look(app: &App, target: ScrollTarget) -> Look {
    if app.scrollbar_dragged() == Some(target) {
        Look::Active
    } else if app.ui.scrollbar_hover == Some(target) {
        Look::Hover
    } else {
        Look::Normal
    }
}

/// Takes the last column off `area` for a bar (the column is always kept, so nothing reflows
/// when a thumb appears); `None` when `area` is narrower than two columns.
pub fn take_column(area: &mut Rect) -> Option<Rect> {
    if area.width < 2 || area.height == 0 {
        return None;
    }
    area.width -= 1;
    Some(Rect {
        x: area.x + area.width,
        width: 1,
        ..*area
    })
}

/// Takes the last row off `area` for the horizontal bar, as `take_column` does for the vertical.
/// `None` when `area` is shorter than two rows.
pub fn take_row(area: &mut Rect) -> Option<Rect> {
    if area.height < 2 || area.width == 0 {
        return None;
    }
    area.height -= 1;
    Some(Rect {
        y: area.y + area.height,
        height: 1,
        ..*area
    })
}

fn slider(look: Look, base: Style, theme: &Theme) -> Style {
    let key = match look {
        Look::Normal => "scrollbarSlider.background",
        Look::Hover => "scrollbarSlider.hoverBackground",
        Look::Active => "scrollbarSlider.activeBackground",
    };
    base.patch(theme.get(key))
}

/// Draws the horizontal bar on `track` (one row), as `render` does a column.
pub fn render_row(
    out: &mut Cells,
    track: Rect,
    mut scroll: Scroll,
    look: Look,
    base: Style,
    theme: &Theme,
) -> ScrollBarHit {
    out.set_style(track, base);
    scroll.shown |= look == Look::Active;
    let thumb = scroll.thumb(track.width);
    if let Some(t) = thumb {
        let rect = Rect {
            x: track.x + t.start,
            width: t.len,
            ..track
        };
        out.set_style(rect, slider(look, base, theme));
    }
    ScrollBarHit { track, thumb }
}

/// Draws the bar on `track` (one column, on `base`: the background of what it scrolls) for
/// `scroll`; returns it for the mouse.
pub fn render(
    out: &mut Cells,
    track: Rect,
    mut scroll: Scroll,
    look: Look,
    base: Style,
    theme: &Theme,
) -> ScrollBarHit {
    out.set_style(track, base);
    scroll.shown |= look == Look::Active;
    let thumb = scroll.thumb(track.height);
    if let Some(t) = thumb {
        let rect = Rect {
            y: track.y + t.start,
            height: t.len,
            ..track
        };
        out.set_style(rect, slider(look, base, theme));
    }
    ScrollBarHit { track, thumb }
}
