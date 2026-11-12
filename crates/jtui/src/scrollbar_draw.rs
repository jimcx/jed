//! Scroll bars: one column (a row for the editors' horizontal bar) at the right edge of an editor
//! or a list, the background of what it scrolls with the thumb (`Scroll::thumb`) as
//! cells of `scrollbarSlider.*`; an editor group also keeps a column left of its bar for the
//! changed lines' marks (`render_marks`). The thumb is placed in half cells: a cell it covers
//! only half of is a half block (`▀` / `▄` vertically, `▄` on the horizontal bar, which is half a
//! row thick).

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::scrollbar::{Scroll, Thumb};
use crate::style::Styles;

/// A scroll bar as drawn: its track (one column, or one row for a horizontal bar) and the thumb on it
/// (`None`: nothing to scroll), in cells from the track's start.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollBarHit {
    pub track: Rect,
    pub thumb: Option<Thumb>,
}

/// Which slider colour the thumb takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Normal,
    /// The pointer is over the thumb.
    Hover,
    /// Being dragged (shown even when the drag reaches the top of a short text).
    Active,
}

/// A change as the editor group's marks column draws it: the lines of a text tab (a deletion is
/// the line above it; the screen rows while wrapping) or the rows of a diff tab it covers, and
/// the theme key it is coloured in. Distinct from the app's byte-range marks:
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

fn slider(look: Look, base: Style, theme: &impl Styles) -> Style {
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
    theme: &impl Styles,
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
    theme: &impl Styles,
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
pub fn render_marks(out: &mut Cells, track: Rect, scroll: Scroll, base: Style, theme: &impl Styles, marks: &[Mark]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    struct Keys;
    impl Styles for Keys {
        fn get(&self, key: &str) -> Style {
            match key {
                "scrollbarSlider.background" => Style::default().bg(Color::Red),
                "scrollbarSlider.hoverBackground" => Style::default().bg(Color::Green),
                "scrollbarSlider.activeBackground" => Style::default().bg(Color::Blue),
                "mark" => Style::default().fg(Color::Yellow),
                _ => Style::default(),
            }
        }
    }

    #[test]
    fn take_column_and_row_keep_one_cell_of_content() {
        let mut a = Rect::new(2, 3, 10, 5);
        assert_eq!(take_column(&mut a), Some(Rect::new(11, 3, 1, 5)));
        assert_eq!(a, Rect::new(2, 3, 9, 5));
        assert_eq!(take_row(&mut a), Some(Rect::new(2, 7, 9, 1)));
        assert_eq!(a, Rect::new(2, 3, 9, 4));
        let mut narrow = Rect::new(0, 0, 1, 5);
        assert_eq!(take_column(&mut narrow), None);
        assert_eq!(narrow, Rect::new(0, 0, 1, 5), "unchanged when refused");
        let mut short = Rect::new(0, 0, 5, 1);
        assert_eq!(take_row(&mut short), None);
    }

    #[test]
    fn render_paints_the_thumb_in_the_look_s_colour_and_reports_it() {
        let track = Rect::new(0, 0, 1, 10);
        for (look, colour) in [
            (Look::Normal, Color::Red),
            (Look::Hover, Color::Green),
            (Look::Active, Color::Blue),
        ] {
            let mut out = Cells::empty(track);
            let scroll = Scroll::rows(40, 10, 0);
            let hit = render(&mut out, track, scroll, look, Style::default(), &Keys);
            let thumb = hit.thumb.expect("a thumb");
            assert_eq!(hit.track, track);
            assert_eq!(out[(0, thumb.start)].bg, colour, "{look:?}");
            assert_ne!(
                out[(0, 9)].bg,
                colour,
                "{look:?}: the track below the thumb stays plain"
            );
        }
        let mut out = Cells::empty(track);
        let hit = render(
            &mut out,
            track,
            Scroll::rows(5, 10, 0),
            Look::Normal,
            Style::default(),
            &Keys,
        );
        assert_eq!(hit.thumb, None, "nothing to scroll: no thumb");
        assert_ne!(out[(0, 0)].bg, Color::Red);
    }

    #[test]
    fn render_row_draws_lower_half_blocks() {
        let track = Rect::new(0, 0, 10, 1);
        let mut out = Cells::empty(track);
        let hit = render_row(
            &mut out,
            track,
            Scroll::rows(40, 10, 0),
            Look::Normal,
            Style::default(),
            &Keys,
        );
        let t = hit.thumb.expect("a thumb");
        assert_eq!(out[(t.start, 0)].symbol(), "▄");
        assert_eq!(out[(t.start, 0)].fg, Color::Red);
        assert_ne!(out[(9, 0)].symbol(), "▄");
    }

    #[test]
    fn marks_colour_their_rows_with_the_key_s_foreground() {
        let track = Rect::new(0, 0, 1, 10);
        let mut out = Cells::empty(track);
        let marks = [
            Mark {
                lines: 2..3,
                key: "mark",
            },
            Mark {
                lines: 5..5,
                key: "mark",
            },
        ];
        render_marks(
            &mut out,
            track,
            Scroll::rows(10, 10, 0),
            Style::default(),
            &Keys,
            &marks,
        );
        assert_eq!(out[(0, 2)].bg, Color::Yellow);
        assert_ne!(out[(0, 1)].bg, Color::Yellow);
        assert_ne!(out[(0, 5)].bg, Color::Yellow, "an empty range draws nothing");
    }
}
