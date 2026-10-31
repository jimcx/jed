//! The border layer: every border line on screen is drawn here, in one pass, by whoever sees all of them.
//!
//! Widgets draw only inside their rect and never a border glyph. The owners of a border (the side panel's
//! edge, the panel and terminal rules, the group dividers, a side-by-side diff's centre line, a side
//! section's divider row) report a [`Segment`] each, and [`Borders::apply`] draws all of them after the
//! regions are filled and before the overlays. A cell where lines meet gets the matching junction
//! (`┬ ┴ ├ ┤ ┼`) instead of a plain `│` / `─`.
//!
//! Which cell gets which glyph and which segment's style depends only on the segments, so it is resolved
//! when they change (resize, split, panel toggle, divider drag) and reused on every other frame; each frame
//! just rewrites the cells, since the buffer starts blank.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use std::collections::HashMap;

use crate::layout::SplitDir;
use crate::theme::Theme;

const N: u8 = 1;
const S: u8 = 2;
const W: u8 = 4;
const E: u8 = 8;

/// A border line: a one-cell-wide strip. `Right` is a vertical line (`│`), `Down` a horizontal one (`─`).
/// Its style is the theme's `base` (the region it sits in, when the key holds only a foreground) patched
/// with `key`. Where segments overlap, the later one's style wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub rect: Rect,
    pub dir: SplitDir,
    pub base: Option<&'static str>,
    pub key: &'static str,
}

impl Segment {
    pub fn new(rect: Rect, dir: SplitDir, base: Option<&'static str>, key: &'static str) -> Segment {
        Segment { rect, dir, base, key }
    }

    fn style(&self, theme: &Theme) -> Style {
        let border = theme.get(self.key);
        self.base.map_or(border, |b| theme.get(b).patch(border))
    }
}

#[derive(Default)]
pub struct Borders {
    key: Vec<Segment>,
    /// Every border cell: its place, glyph and the index of the segment whose style it takes.
    cells: Vec<(u16, u16, char, usize)>,
    /// How many times the cells were resolved (tests check the cache works).
    pub resolved: u32,
}

impl Borders {
    /// Draws `segments`; they are resolved again only when they differ from the last frame's.
    pub fn apply(&mut self, f: &mut Frame, segments: Vec<Segment>, theme: &Theme) {
        if segments != self.key {
            self.cells = resolve(&segments);
            self.key = segments;
            self.resolved += 1;
        }
        let styles: Vec<Style> = self.key.iter().map(|s| s.style(theme)).collect();
        let area = f.area();
        let out = f.buffer_mut();
        for &(x, y, c, seg) in &self.cells {
            if x < area.x + area.width && y < area.y + area.height {
                out[(x, y)].set_char(c).set_style(styles[seg]);
            }
        }
    }
}

/// The glyph for a cell joined in the directions of `mask`.
fn glyph(mask: u8) -> Option<char> {
    Some(match mask {
        m if m == N | S | W | E => '┼',
        m if m == N | S | E => '├',
        m if m == N | S | W => '┤',
        m if m == W | E | S => '┬',
        m if m == W | E | N => '┴',
        m if m & (N | S) != 0 && m & (W | E) == 0 => '│',
        m if m & (W | E) != 0 && m & (N | S) == 0 => '─',
        _ => return None,
    })
}

/// Every border cell with its glyph. A cell connects the way its own line runs and towards each
/// neighbour that has a line running into it (a vertical one above or below, a horizontal one beside).
fn resolve(segments: &[Segment]) -> Vec<(u16, u16, char, usize)> {
    // Per cell: bit 1 = a vertical line passes through, bit 2 = a horizontal one; and the last segment there.
    let mut lines: HashMap<(u16, u16), (u8, usize)> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        let bit = if s.dir == SplitDir::Right { 1 } else { 2 };
        for y in s.rect.y..s.rect.y + s.rect.height {
            for x in s.rect.x..s.rect.x + s.rect.width {
                let e = lines.entry((x, y)).or_default();
                *e = (e.0 | bit, i);
            }
        }
    }
    let at = |x: Option<u16>, y: Option<u16>| x.zip(y).and_then(|p| lines.get(&p)).map_or(0, |e| e.0);
    let mut cells = Vec::with_capacity(lines.len());
    for (&(x, y), &(own, seg)) in &lines {
        let mut mask = 0;
        if own & 1 != 0 {
            mask |= N | S;
        }
        if own & 2 != 0 {
            mask |= W | E;
        }
        if at(Some(x), y.checked_sub(1)) & 1 != 0 {
            mask |= N;
        }
        if at(Some(x), y.checked_add(1)) & 1 != 0 {
            mask |= S;
        }
        if at(x.checked_sub(1), Some(y)) & 2 != 0 {
            mask |= W;
        }
        if at(x.checked_add(1), Some(y)) & 2 != 0 {
            mask |= E;
        }
        if let Some(c) = glyph(mask) {
            cells.push((x, y, c, seg));
        }
    }
    cells.sort_unstable();
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn seg(rect: Rect, dir: SplitDir) -> Segment {
        Segment::new(rect, dir, None, "editorGroup.border")
    }

    /// Only the junction cells (the plain lines left out).
    fn junctions(segments: &[Segment]) -> Vec<(u16, u16, char)> {
        resolve(segments)
            .into_iter()
            .filter(|c| !matches!(c.2, '│' | '─'))
            .map(|(x, y, c, _)| (x, y, c))
            .collect()
    }

    #[test]
    fn glyph_table() {
        assert_eq!(glyph(N | S | E), Some('├'));
        assert_eq!(glyph(N | S | W), Some('┤'));
        assert_eq!(glyph(W | E | S), Some('┬'));
        assert_eq!(glyph(W | E | N), Some('┴'));
        assert_eq!(glyph(N | S | W | E), Some('┼'));
        assert_eq!(glyph(N | S), Some('│'));
        assert_eq!(glyph(W | E), Some('─'));
        assert_eq!(glyph(0), None);
    }

    #[test]
    fn lines_are_drawn_plain_and_junctions_join() {
        let cells = resolve(&[seg(r(2, 0, 1, 3), SplitDir::Right), seg(r(0, 5, 4, 1), SplitDir::Down)]);
        assert_eq!(cells.len(), 7);
        assert!(
            cells.iter().all(|c| matches!(c.2, '│' | '─')),
            "not touching: {cells:?}"
        );
        // A divider on rows 0..5 above a rule on row 5.
        let j = junctions(&[
            seg(r(10, 0, 1, 5), SplitDir::Right),
            seg(r(3, 5, 20, 1), SplitDir::Down),
        ]);
        assert_eq!(j, vec![(10, 5, '┴')]);
    }

    #[test]
    fn rule_beside_a_vertical_border_and_crossings() {
        // A side edge on column 4 against a rule starting at column 5.
        let j = junctions(&[seg(r(4, 0, 1, 9), SplitDir::Right), seg(r(5, 3, 10, 1), SplitDir::Down)]);
        assert_eq!(j, vec![(4, 3, '├')]);
        let j = junctions(&[
            seg(r(5, 0, 1, 10), SplitDir::Right),
            seg(r(0, 4, 11, 1), SplitDir::Down),
        ]);
        assert_eq!(j, vec![(5, 4, '┼')]);
        // A vertical divider hanging from a rule.
        let j = junctions(&[seg(r(0, 2, 20, 1), SplitDir::Down), seg(r(8, 3, 1, 4), SplitDir::Right)]);
        assert_eq!(j, vec![(8, 2, '┬')]);
    }

    #[test]
    fn cells_take_their_own_segments_style_and_the_cache_resolves_once() {
        let theme = Theme::dark_plus(true);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 10)).unwrap();
        let mut cache = Borders::default();
        let a = vec![
            Segment::new(r(10, 0, 1, 5), SplitDir::Right, None, "editorGroup.border"),
            Segment::new(r(0, 5, 30, 1), SplitDir::Down, Some("panel"), "panel.border"),
        ];
        for _ in 0..3 {
            term.draw(|f| cache.apply(f, a.clone(), &theme)).unwrap();
        }
        assert_eq!(cache.resolved, 1);
        let b = term.backend().buffer();
        assert_eq!(b[(10, 2)].symbol(), "│");
        assert_eq!(b[(10, 5)].symbol(), "┴");
        assert_eq!(b[(3, 5)].symbol(), "─");
        assert_eq!(b[(10, 2)].fg, theme.get("editorGroup.border").fg.unwrap());
        assert_eq!(b[(3, 5)].fg, theme.get("panel.border").fg.unwrap());
        let moved = vec![a[0].clone().tap(|s| s.rect.x = 12), a[1].clone()];
        term.draw(|f| cache.apply(f, moved, &theme)).unwrap();
        assert_eq!(cache.resolved, 2);
    }

    trait Tap: Sized {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl Tap for Segment {}
}
