//! Vertical scroll bars (the editors', the side bar's and the bottom panel's lists), the model
//! (no drawing: `ui::scrollbar`; `app/scrollbar.rs` drives the mouse): where the thumb goes on a
//! track for a scroll position, and back. Plain cells, no sub-cell glyphs. Everything is O(1):
//! the caller gives the line or row count. Anything scrolled gets a bar by implementing
//! [`Scrollable`] (on its state, or a borrow of it with what it needs). Sideways scrolling's
//! extent is a [`ScrollWidth`] (text and diff views).

/// Something a scroll bar scrolls: a text view, a diff, a list. Implemented next to each list's
/// model; `App::with_scrollable` (`app/scrollbar.rs`) picks the one a `ScrollTarget` names, and
/// drawing builds the same one for the thumb. `scroll` must not walk the list's rows (a count
/// and an index: lists that keep their top by key convert with a binary search).
pub trait Scrollable {
    /// Where it is now (`Scroll::rows` for a list whose last page stays full, `Scroll::text`).
    fn scroll(&self) -> Scroll;
    /// Puts row `top` at the top, clamped as its own scrolling clamps (the wheel's end).
    fn set_top(&mut self, top: usize);
}

/// A scroll position: the first row shown, the rows on screen and the largest first row the
/// view can scroll to. The thumb's length is the screen's share of `max_top + height` (every
/// row the viewport can cover), its position `top`'s share of `max_top`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub top: usize,
    pub height: usize,
    pub max_top: usize,
    /// Whether the bar shows a thumb (VS Code's `scrollbar.vertical: auto`).
    pub shown: bool,
}

/// The thumb on a track: its first cell and its length, in cells from the track's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thumb {
    pub start: u16,
    pub len: u16,
}

impl Thumb {
    pub fn contains(&self, cell: u16) -> bool {
        (self.start..self.start + self.len).contains(&cell)
    }
}

impl Scroll {
    /// A text view of `lines` lines (`View::set_top_line` lets the last line reach the top, as
    /// VS Code's `scrollBeyondLastLine`): shown when the text is taller than the view, or the
    /// view is scrolled (so it can be dragged back).
    pub fn text(lines: usize, height: usize, top: usize) -> Self {
        let lines = lines.max(1);
        let height = height.max(1);
        Self {
            top: top.min(lines - 1),
            height,
            max_top: lines - 1,
            shown: lines > height || top > 0,
        }
    }

    /// `rows` rows whose last page stays full (a diff tab): shown when they don't all fit.
    pub fn rows(rows: usize, height: usize, top: usize) -> Self {
        let height = height.max(1);
        let max_top = rows.saturating_sub(height);
        Self {
            top: top.min(max_top),
            height,
            max_top,
            shown: max_top > 0,
        }
    }

    /// The columns of a view `width` wide (`left_col` its first) with `scroll_width` for its
    /// extent: the horizontal bar. Shown while the widest line drawn is wider than the view (or
    /// the view is scrolled, so it can be dragged back); a view past the end after a reveal
    /// keeps its `left_col` as the largest.
    pub fn columns(scroll_width: &ScrollWidth, width: usize, left_col: usize) -> Self {
        let width = width.max(1);
        Self {
            top: left_col,
            height: width,
            max_top: scroll_width.max_left(width).max(left_col),
            shown: scroll_width.wants_bar(width, left_col),
        }
    }

    /// The thumb on a track of `track` cells; `None` when not shown.
    pub fn thumb(&self, track: u16) -> Option<Thumb> {
        if !self.shown || track == 0 {
            return None;
        }
        let t = track as u64;
        let (top, height, max_top) = (self.top as u64, self.height as u64, self.max_top as u64);
        let content = max_top + height;
        let mut len = ((t * height + content / 2) / content).clamp(1, t);
        // Room to move whenever there's somewhere to scroll.
        if max_top > 0 && t >= 2 {
            len = len.min(t - 1);
        }
        let room = t - len;
        let mut start = (room * top + max_top / 2).checked_div(max_top).unwrap_or(0);
        // A view off the very top or bottom never shows its thumb touching that end.
        if room >= 2 {
            start = start.clamp(u64::from(top > 0), room - u64::from(top < max_top));
        }
        Some(Thumb {
            start: start as u16,
            len: len as u16,
        })
    }

    /// The track cell the line (a text) or row (a diff) `i` of the whole content projects to on
    /// `track` cells: where the thumb would start if `i` were the top (its length's rounding
    /// aside). Independent of `top`, so a mark stays put as the view scrolls; the thumb draws
    /// over it. Content shorter than the view spreads over the top of the track (its share of
    /// `max_top + height`), as the thumb's maths does.
    pub fn cell(&self, track: u16, i: usize) -> u16 {
        let content = (self.max_top + self.height).max(1) as u64;
        let i = i.min(content as usize - 1) as u64;
        let cell = (i * u64::from(track) + content / 2) / content;
        cell.min(u64::from(track.saturating_sub(1))) as u16
    }

    /// The first row that puts the thumb's start at `start` (cells from the track's start,
    /// clamped to the track) — the smallest such row, else the next one; `top` when the thumb
    /// can't move. As if `shown` (a drag goes on when it reaches the top of a short text).
    pub fn top_at(&self, track: u16, start: i32) -> usize {
        let this = Self { shown: true, ..*self };
        let Some(thumb) = this.thumb(track) else {
            return self.top;
        };
        let room = (track - thumb.len) as u64;
        if room == 0 {
            return self.top;
        }
        let s = start.clamp(0, room as i32) as u16;
        let max_top = self.max_top as u64;
        let start_of = |top: u64| this.with_top(top as usize).thumb(track).map_or(0, |t| t.start);
        // The smallest row drawn at `s` or below it (binary search: `start_of` never decreases).
        let (mut lo, mut hi) = (0, max_top);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if start_of(mid) < s {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let top = lo;
        top as usize
    }

    fn with_top(&self, top: usize) -> Self {
        Self { top, ..*self }
    }
}

/// A view's scroll width for sideways scrolling: the widest line drawn in it so far (display
/// columns). Drawing measures the visible lines only (never the whole text) and it only grows,
/// as VS Code's does, so it doesn't shrink as the view scrolls vertically past a long line;
/// `reset` when the view's text is reloaded. Not saved in sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollWidth {
    widest: usize,
}

impl ScrollWidth {
    /// A drawn line `width` columns wide.
    pub fn measure(&mut self, width: usize) {
        self.widest = self.widest.max(width);
    }

    pub fn reset(&mut self) {
        self.widest = 0;
    }

    /// The widest line drawn so far.
    pub fn widest(&self) -> usize {
        self.widest
    }

    /// Whether the horizontal bar's row is kept: a line wider than the view has been drawn, or
    /// the view is scrolled.
    pub fn wants_bar(&self, width: usize, left_col: usize) -> bool {
        self.widest() > width || left_col > 0
    }

    /// The largest first column scrolling right reaches in a view `width` columns wide: the
    /// widest line's end in view with one blank column after it (as the vertical last page).
    pub fn max_left(&self, width: usize) -> usize {
        (self.widest + 1).saturating_sub(width.max(1))
    }

    /// `left_col` scrolled by `delta` columns: never below 0, and scrolling right stops at
    /// `max_left` (a view already past it, after a reveal, stays put).
    pub fn scroll(&self, left_col: usize, delta: isize, width: usize) -> usize {
        let col = left_col.saturating_add_signed(delta);
        if delta > 0 {
            col.min(self.max_left(width).max(left_col))
        } else {
            col
        }
    }
}

/// The columns of a text or diff view as a [`Scrollable`] (the horizontal bar): the view's
/// `left_col` and `scroll_width`, `width` columns wide.
pub struct Columns<'a> {
    pub left_col: &'a mut usize,
    pub scroll_width: ScrollWidth,
    pub width: usize,
}

impl Scrollable for Columns<'_> {
    fn scroll(&self) -> Scroll {
        Scroll::columns(&self.scroll_width, self.width, *self.left_col)
    }

    /// Never past the widest line's end (a view already past it, after a reveal, may be dragged
    /// back but not further).
    fn set_top(&mut self, top: usize) {
        *self.left_col = top.min(self.scroll_width.max_left(self.width).max(*self.left_col));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn columns_scroll_and_clamp() {
        let mut w = super::ScrollWidth::default();
        w.measure(100);
        let s = super::Scroll::columns(&w, 80, 0);
        assert!(s.shown && s.max_top == 21 && s.height == 80);
        let mut left = 5;
        let mut c = super::Columns {
            left_col: &mut left,
            scroll_width: w,
            width: 80,
        };
        super::Scrollable::set_top(&mut c, 500);
        assert_eq!(left, 21);
        assert!(!super::Scroll::columns(&w, 100, 0).shown, "fits: no bar");
        assert!(
            super::Scroll::columns(&w, 100, 3).shown,
            "scrolled: can be dragged back"
        );
        assert_eq!(
            super::Scroll::columns(&w, 80, 40).max_top,
            40,
            "past the end stays reachable"
        );
    }

    #[test]
    fn scroll_width_grows_and_clamps() {
        let mut w = super::ScrollWidth::default();
        w.measure(100);
        w.measure(30);
        assert_eq!(w.widest(), 100, "only grows");
        assert_eq!(w.max_left(80), 21, "one blank column after the widest line's end");
        assert_eq!(w.scroll(0, 4, 80), 4);
        assert_eq!(w.scroll(20, 4, 80), 21);
        assert_eq!(w.scroll(30, 4, 80), 30, "past the end (a reveal): stays put");
        assert_eq!(w.scroll(30, -4, 80), 26);
        assert_eq!(w.scroll(2, -4, 80), 0);
        w.reset();
        assert_eq!(
            (w.widest(), w.max_left(80), w.scroll(0, 4, 80)),
            (0, 0, 0),
            "fits: no sideways scroll"
        );
    }

    use super::*;

    fn thumb(s: Scroll, track: u16) -> (u16, u16) {
        let t = s.thumb(track).unwrap();
        (t.start, t.len)
    }

    #[test]
    fn a_text_shorter_than_the_view_shows_no_thumb_until_scrolled() {
        assert_eq!(Scroll::text(10, 20, 0).thumb(20), None);
        assert_eq!(Scroll::text(20, 20, 0).thumb(20), None, "exactly fits");
        // Scrolled (the wheel goes down to the last line): a thumb to drag back with.
        let s = Scroll::text(10, 20, 3);
        assert!(s.shown);
        let (start, len) = thumb(s, 20);
        assert!(start > 0 && start + len <= 20 && len < 20, "{start} {len}");
        assert_eq!(Scroll::rows(10, 20, 0).thumb(20), None);
        assert_eq!(Scroll::rows(20, 20, 0).thumb(20), None);
    }

    #[test]
    fn the_thumb_spans_the_screens_share_and_moves_with_the_top() {
        // 100 lines, 20 on screen, the last line may go to the top: 119 rows of content.
        let s = |top| Scroll::text(100, 20, top);
        assert_eq!(thumb(s(0), 20), (0, 3));
        assert_eq!(thumb(s(99), 20), (17, 3), "the end of the track at the last line");
        assert_eq!(thumb(s(50), 20).0, 9, "halfway");
        // Just off either end: one cell away from it.
        assert_eq!(thumb(s(1), 20).0, 1);
        assert_eq!(thumb(s(98), 20).0, 16);
        // A diff's rows stop at a full last page.
        let r = |top| Scroll::rows(100, 20, top);
        assert_eq!(thumb(r(0), 20), (0, 4));
        assert_eq!(thumb(r(80), 20), (16, 4));
        assert_eq!(r(500).top, 80, "clamped");
    }

    #[test]
    fn a_huge_text_keeps_a_one_cell_thumb() {
        let s = Scroll::text(10_000_000, 40, 5_000_000);
        assert_eq!(thumb(s, 40), (20, 1));
        assert_eq!(thumb(Scroll::text(10_000_000, 40, 0), 40), (0, 1));
        assert_eq!(thumb(Scroll::text(10_000_000, 40, 9_999_999), 40), (39, 1));
    }

    #[test]
    fn a_barely_scrollable_text_leaves_room_to_move() {
        // 21 rows in 20: the thumb never fills the track.
        assert_eq!(thumb(Scroll::rows(21, 20, 0), 20), (0, 19));
        assert_eq!(thumb(Scroll::rows(21, 20, 1), 20), (1, 19));
        // 21 lines of text in 20 rows: the last line can reach the top, so half the content.
        assert_eq!(thumb(Scroll::text(21, 20, 0), 20), (0, 10));
        assert_eq!(thumb(Scroll::text(21, 20, 20), 20), (10, 10));
        // A track shorter than the view (rounding alone would fill it): one cell left.
        assert_eq!(thumb(Scroll::rows(41, 40, 0), 10), (0, 9));
        assert_eq!(thumb(Scroll::rows(41, 40, 1), 10), (1, 9));
        // A one-cell track: the thumb is the track.
        assert_eq!(thumb(Scroll::text(1000, 1, 500), 1), (0, 1));
        assert_eq!(Scroll::text(1000, 1, 500).top_at(1, 0), 500, "nowhere to move");
    }

    #[test]
    fn a_line_projects_to_its_cell_over_the_whole_content() {
        // 100 lines in 20 rows on a 20-cell track: the last line's cell is where its thumb
        // starts, and the first is at the top.
        let s = Scroll::text(100, 20, 0);
        assert_eq!(s.thumb(20).unwrap().start, 0);
        assert_eq!(s.cell(20, 0), 0);
        assert_eq!(s.with_top(99).thumb(20).unwrap().start, 17);
        assert_eq!(s.cell(20, 99), 17);
        // Monotone, inside the track, and independent of the top (a mark stays put as you
        // scroll).
        let mut last = 0;
        for i in 0..100 {
            let cell = s.cell(20, i);
            assert!(cell >= last && cell < 20, "line {i}: {cell}");
            last = cell;
            assert_eq!(cell, s.with_top(50).cell(20, i), "line {i}");
        }
        // Fewer rows than cells: spread over the content, clamped at the track's end.
        let r = Scroll::rows(200, 20, 0);
        assert_eq!((r.cell(20, 0), r.cell(20, 199)), (0, 19));
        assert_eq!(r.cell(20, 10_000), 19, "clamped");
        // Content that fits still projects (the bar may show no thumb).
        let short = Scroll::rows(10, 20, 0);
        assert_eq!((short.cell(20, 0), short.cell(20, 9)), (0, 9));
        assert_eq!(short.cell(20, 10_000), 19, "clamped");
        // No track: cell 0.
        assert_eq!(Scroll::text(10, 1, 0).cell(0, 5), 0);
        assert_eq!(Scroll::rows(0, 20, 0).cell(20, 0), 0);
    }

    #[test]
    fn dragging_to_a_cell_scrolls_to_the_first_row_drawn_there() {
        for (lines, height, track) in [(100, 20, 20), (1000, 30, 30), (25, 20, 20), (7, 40, 40), (5000, 9, 9)] {
            // Dragging a short text's thumb to the top keeps it (`top_at` as if shown).
            let at = |top| Scroll {
                shown: true,
                ..Scroll::text(lines, height, top)
            };
            let room = track - at(1).thumb(track).unwrap().len;
            for s in 0..=room {
                let top = at(0).top_at(track, s as i32);
                assert_eq!(at(top).thumb(track).unwrap().start, s, "{lines} {height}: cell {s}");
                if top > 0 {
                    assert!(at(top - 1).thumb(track).unwrap().start < s, "the first such row");
                }
            }
            assert_eq!(at(5).top_at(track, -3), 0, "above the track: the top");
            assert_eq!(at(0).top_at(track, 1000), lines - 1, "below it: the last line");
        }
        // Fewer rows than cells: a cell no row draws at goes to the next row.
        let s = Scroll::rows(23, 20, 0);
        assert_eq!(thumb(s, 20), (0, 17));
        assert_eq!(s.top_at(20, 3), 3);
    }
}
