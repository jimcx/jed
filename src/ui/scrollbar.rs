//! Scroll bars: one column (a row for the editors' horizontal bar) at the right edge of an editor
//! or a list, the background of what it scrolls with the thumb (`scrollbar::Scroll::thumb`) as
//! cells of `scrollbarSlider.*`; an editor group also keeps a column left of its bar for the
//! changed lines' marks (`render_marks`). The thumb is placed in half cells: a cell it covers
//! only half of is a half block (`▀` / `▄` vertically, `▄` on the horizontal bar, which is half a
//! row thick).

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

/// A change as the editor group's marks column draws it: the lines of a text tab (a deletion is
/// the line above it; the screen rows while wrapping) or the rows of a diff tab it covers, and
/// the theme key it is coloured in. Distinct from `quick_diff::Mark` (a change's bytes):
/// [`Scroll::cell`] projects this onto a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// The lines (a text tab; its rows while wrapping) or rows (a diff tab), in the content's
    /// own numbering.
    pub lines: std::ops::Range<usize>,
    pub key: &'static str,
}

/// Takes the last column off `area` for a bar (or an editor group's marks column; the column is
/// always kept, so nothing reflows when a thumb or a mark appears); `None` when `area` is
/// narrower than two columns.
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
        // A lower-half block in the slider colour on the track's background: half the height.
        let style = slider(look, base, theme);
        let half = Style {
            fg: style.bg,
            bg: base.bg,
            ..style
        };
        for x in rect.left()..rect.right() {
            if let Some(cell) = out.cell_mut((x, rect.y)) {
                cell.set_symbol("▄").set_style(half);
            }
        }
    }
    ScrollBarHit { track, thumb }
}

/// Draws the bar on `track` (one column, on `base`: the background of what it scrolls) for
/// `scroll`; returns it for the mouse. The changed lines' marks have a column of their own
/// ([`render_marks`]), so the thumb never paints over them.
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
    // The thumb is placed in half rows: a cell it covers only half of is a half block.
    let thumb = scroll.vthumb(track.height);
    if let (Some(t), Some(halves)) = (thumb, scroll.vthumb_halves(track.height)) {
        let style = slider(look, base, theme);
        let half = Style {
            fg: style.bg,
            bg: base.bg,
            ..style
        };
        let (first, end) = (halves.start, halves.start + halves.len);
        for c in t.start..t.start + t.len {
            let Some(cell) = out.cell_mut((track.x, track.y + c)) else {
                continue;
            };
            let (top, bottom) = (2 * c >= first && 2 * c < end, 2 * c + 1 >= first && 2 * c + 1 < end);
            match (top, bottom) {
                (true, true) => {
                    cell.set_style(style);
                }
                (true, false) => {
                    cell.set_symbol("▀").set_style(half);
                }
                _ => {
                    cell.set_symbol("▄").set_style(half);
                }
            }
        }
    }
    ScrollBarHit { track, thumb }
}

/// Fills `track` (a marks column of its own, left of the scroll bar's) with the cells of the
/// lines the changes cover, on `base`. The column is kept even when there are no marks, so a
/// short text still shows them and nothing reflows when they appear or go. The gutter keys carry
/// their colour in the foreground (the gutter draws a glyph in it), so a blank cell — no glyph —
/// takes it as its background.
pub fn render_marks(out: &mut Cells, track: Rect, scroll: Scroll, base: Style, theme: &Theme, marks: &[Mark]) {
    out.set_style(track, base);
    if track.height == 0 {
        return;
    }
    // A best-effort cache: consecutive marks of the same key reuse one theme lookup. It isn't a
    // guarantee — merged staged/unstaged marks of a kind interleave by position, so runs of the
    // same key only happen where they happen to be contiguous.
    let mut last: Option<(&str, Style)> = None;
    for mark in marks.iter().filter(|m| !m.lines.is_empty()) {
        let fill = match last {
            Some((key, fill)) if key == mark.key => fill,
            _ => {
                let fill = base.patch(Style::default().bg(theme.get(mark.key).fg.unwrap_or_default()));
                last = Some((mark.key, fill));
                fill
            }
        };
        let first = scroll.cell(track.height, mark.lines.start);
        let last_cell = scroll.cell(track.height, mark.lines.end - 1);
        if first == last_cell {
            out[(track.x, track.y + first)].set_style(fill);
        } else {
            out.set_style(
                Rect {
                    y: track.y + first,
                    height: last_cell - first + 1,
                    ..track
                },
                fill,
            );
        }
    }
}
