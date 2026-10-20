//! A list grouped by file, as the Search view's results and the Problems panel show them: each
//! group (a file, sorted by path) is a row followed by one row per item in it. Rows are named by
//! a `RowKey` (the file's path, the row within it), so the list's top row and its selection stay
//! on what they showed when groups arrive, go or grow above them. No drawing.

use std::path::{Path, PathBuf};

/// A row by what it shows: a file (by path) and the row within it (0 = the file's own row,
/// `k` = its `k`-th item).
pub type RowKey = (PathBuf, usize);

/// One group of the list. Groups are sorted by `path`.
pub trait Group {
    fn path(&self) -> &Path;
    /// Item rows under the file's own row.
    fn items(&self) -> usize;
}

fn group_rows<G: Group>(g: &G) -> usize {
    1 + g.items()
}

/// Rows in the whole list.
pub fn row_count<G: Group>(groups: &[G]) -> usize {
    groups.iter().map(group_rows).sum()
}

/// The row index of `key`; `Err` with where its file would be when it's not in the list.
pub fn index_of<G: Group>(groups: &[G], (path, offset): &RowKey) -> Result<usize, usize> {
    let i = groups.partition_point(|g| g.path() < path.as_path());
    let before: usize = groups[..i].iter().map(group_rows).sum();
    match groups.get(i) {
        Some(g) if g.path() == path => Ok(before + (*offset).min(g.items())),
        _ => Err(before),
    }
}

/// The key of row `row` (which must be in the list).
pub fn key_at<G: Group>(groups: &[G], row: usize) -> RowKey {
    let mut skip = row;
    for g in groups {
        let n = group_rows(g);
        if skip < n {
            return (g.path().to_path_buf(), skip);
        }
        skip -= n;
    }
    unreachable!("row {row} past the end of the list")
}

/// The group a key names, by binary search.
pub fn group<'a, G: Group>(groups: &'a [G], path: &Path) -> Option<(usize, &'a G)> {
    let i = groups.binary_search_by(|g| g.path().cmp(path)).ok()?;
    Some((i, &groups[i]))
}

/// Up to `n` rows from row `from`, as (group index, row in the group); only the groups skipped
/// are looked at past those shown.
pub fn rows<G: Group>(groups: &[G], from: usize, n: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(n.min(1024));
    let mut skip = from;
    for (i, g) in groups.iter().enumerate() {
        if out.len() >= n {
            break;
        }
        let rows = group_rows(g);
        if skip >= rows {
            skip -= rows;
            continue;
        }
        let first = skip;
        skip = 0;
        out.extend((first..rows).take(n - out.len()).map(|offset| (i, offset)));
    }
    out
}

/// A grouped list's scroll position and selection, both kept by key.
#[derive(Debug, Clone, Default)]
pub struct ListState {
    /// The top row; `None` is the top of the list.
    top: Option<RowKey>,
    selected: Option<RowKey>,
    /// Rows shown in the last frame (set by drawing, `set_height`); 0 until the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
}

/// A grouped list's scroll bar: its state with its groups (`ListState::scrollable`).
pub struct GroupedScroll<'a, G> {
    state: &'a mut ListState,
    groups: &'a [G],
}

impl<G: Group> crate::scrollbar::Scrollable for GroupedScroll<'_, G> {
    /// Per group, not per row: the row count, the top key's index by binary search.
    fn scroll(&self) -> crate::scrollbar::Scroll {
        let rows = row_count(self.groups);
        crate::scrollbar::Scroll::rows(rows, self.state.height, self.state.scroll(self.groups))
    }

    fn set_top(&mut self, top: usize) {
        self.state.set_scroll(self.groups, top);
    }
}

impl ListState {
    /// This list's scroll bar over `groups`.
    pub fn scrollable<'a, G: Group>(&'a mut self, groups: &'a [G]) -> GroupedScroll<'a, G> {
        GroupedScroll { state: self, groups }
    }

    /// Back to the top, nothing selected.
    pub fn reset(&mut self) {
        self.top = None;
        self.selected = None;
        self.reveal_pending = false;
    }

    /// The row index at the top of the list.
    pub fn scroll<G: Group>(&self, groups: &[G]) -> usize {
        let Some(key) = &self.top else { return 0 };
        index_of(groups, key).unwrap_or_else(|i| i)
    }

    /// Puts row `row` (clamped so the last row can reach the bottom) at the top.
    pub fn set_scroll<G: Group>(&mut self, groups: &[G], row: usize) {
        let row = row.min(row_count(groups).saturating_sub(self.height.max(1)));
        self.top = (row > 0).then(|| key_at(groups, row));
    }

    pub fn scroll_by<G: Group>(&mut self, groups: &[G], delta: isize) {
        let row = self.scroll(groups).saturating_add_signed(delta);
        self.set_scroll(groups, row);
    }

    /// Re-clamps the scroll (the list or its height may have changed), keeping the top row.
    pub fn clamp<G: Group>(&mut self, groups: &[G]) {
        let top = self.scroll(groups);
        self.set_scroll(groups, top);
    }

    /// The selected row's key, whether or not it's still in the list.
    pub fn selected(&self) -> Option<&RowKey> {
        self.selected.as_ref()
    }

    /// The selected row's index, if a row is selected and its file is still in the list.
    pub fn selected_row<G: Group>(&self, groups: &[G]) -> Option<usize> {
        index_of(groups, self.selected.as_ref()?).ok()
    }

    /// Selects row `row` (clamped to the list; nothing when it's empty) and scrolls to it.
    pub fn select<G: Group>(&mut self, groups: &[G], row: usize) {
        let n = row_count(groups);
        self.selected = (n > 0).then(|| key_at(groups, row.min(n - 1)));
        self.reveal_selected(groups);
    }

    /// Selects the row `key` names (scrolled to), if its file is in the list.
    pub fn select_key<G: Group>(&mut self, groups: &[G], key: &RowKey) -> bool {
        if index_of(groups, key).is_err() {
            return false;
        }
        self.selected = Some(key.clone());
        self.reveal_selected(groups);
        true
    }

    /// Moves the selection by `delta` rows (clamped), or selects the first row when nothing is
    /// and `delta` is positive. False when moving up from the first row (or with nothing
    /// selected): the caller may give the keys back to whatever is above the list.
    pub fn move_selection<G: Group>(&mut self, groups: &[G], delta: isize) -> bool {
        match self.selected_row(groups) {
            Some(0) | None if delta < 0 => false,
            None => {
                self.select(groups, 0);
                true
            }
            Some(row) => {
                self.select(groups, row.saturating_add_signed(delta));
                true
            }
        }
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height<G: Group>(&mut self, groups: &[G], height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.reveal_selected(groups);
        }
    }

    /// Scrolls the least that shows the selected row; before the first draw (no height yet),
    /// waits for `set_height`.
    pub fn reveal_selected<G: Group>(&mut self, groups: &[G]) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
        let Some(row) = self.selected_row(groups) else { return };
        let (top, height) = (self.scroll(groups), self.height);
        if row < top {
            self.set_scroll(groups, row);
        } else if row >= top + height {
            self.set_scroll(groups, row + 1 - height);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A group for tests: a path and its item count.
    pub struct G(pub PathBuf, pub usize);

    impl Group for G {
        fn path(&self) -> &Path {
            &self.0
        }
        fn items(&self) -> usize {
            self.1
        }
    }

    pub fn groups(list: &[(&str, usize)]) -> Vec<G> {
        list.iter().map(|(p, n)| G(PathBuf::from(p), *n)).collect()
    }

    #[test]
    fn rows_keys_and_indexes() {
        let g = groups(&[("b", 2), ("d", 3)]);
        assert_eq!(row_count(&g), 7);
        assert_eq!(rows(&g, 2, 3), [(0, 2), (1, 0), (1, 1)]);
        assert_eq!(rows(&g, 6, 10), [(1, 3)]);
        assert!(rows(&g, 7, 10).is_empty());
        for row in 0..7 {
            assert_eq!(index_of(&g, &key_at(&g, row)), Ok(row));
        }
        assert_eq!(index_of(&g, &(PathBuf::from("c"), 1)), Err(3), "where c would go");
        assert_eq!(
            index_of(&g, &(PathBuf::from("b"), 9)),
            Ok(2),
            "a row past the end: the last"
        );
        assert_eq!(group(&g, Path::new("d")).map(|(i, _)| i), Some(1));
        assert!(group(&g, Path::new("c")).is_none());
    }

    /// The scroll bar's view of the list: rows, height and top by index; its end is the
    /// wheel's (the last row at the bottom); the selection never moves.
    #[test]
    fn the_scroll_bar_reads_and_sets_the_top_by_index() {
        use crate::scrollbar::Scrollable;
        let g = groups(&[("b", 2), ("d", 3)]);
        let mut l = ListState {
            height: 3,
            ..ListState::default()
        };
        l.select(&g, 1);
        l.set_scroll(&g, 2);
        let s = l.scrollable(&g).scroll();
        assert_eq!((s.top, s.height, s.max_top, s.shown), (2, 3, 4, true));
        l.scrollable(&g).set_top(100);
        assert_eq!(l.scroll(&g), 4);
        l.scroll_by(&g, 100);
        assert_eq!(l.scroll(&g), 4, "where the wheel ends");
        assert_eq!(l.selected_row(&g), Some(1));
        let fits = groups(&[("a", 1)]);
        assert!(!l.scrollable(&fits).scroll().shown, "fits: no thumb");
    }

    /// The top row and the selection stay on their keys when a group arrives above them.
    #[test]
    fn scroll_and_selection_follow_their_keys() {
        let mut g = groups(&[("b", 2), ("d", 3)]);
        let mut l = ListState {
            height: 2,
            ..ListState::default()
        };
        l.set_scroll(&g, 4);
        l.select(&g, 5);
        assert_eq!((l.scroll(&g), l.selected_row(&g)), (4, Some(5)));
        g.insert(0, G(PathBuf::from("a"), 4));
        assert_eq!((l.scroll(&g), l.selected_row(&g)), (9, Some(10)));
        l.scroll_by(&g, 100);
        assert_eq!(l.scroll(&g), row_count(&g) - 2, "the last row reaches the bottom");
        assert!(l.move_selection(&g, -100));
        assert_eq!(l.selected_row(&g), Some(0), "clamped to the first row");
        assert_eq!(l.scroll(&g), 0, "scrolled to it");
        assert!(!l.move_selection(&g, -1), "up from the first row: the caller decides");
        assert_eq!(l.selected_row(&g), Some(0));
        // The top row's group goes (its problems fixed): the top stays where it was, on what
        // came after it; the selection in it is gone.
        let mut g = groups(&[("a", 2), ("b", 2), ("c", 2), ("d", 2)]);
        l.height = 2;
        l.set_scroll(&g, 3);
        l.select(&g, 4);
        g.remove(1);
        assert_eq!((l.scroll(&g), l.selected_row(&g)), (3, None), "c's row now on top");
        l.clamp(&g);
        assert_eq!(l.scroll(&g), 3);
        assert!(l.move_selection(&g, 1), "nothing selected: Down selects the first row");
        assert_eq!(l.selected_row(&g), Some(0));
        l.reset();
        assert_eq!((l.scroll(&g), l.selected_row(&g)), (0, None));
        assert!(!l.select_key(&g, &(PathBuf::from("zz"), 0)));
    }

    /// Before the first draw (no height) a reveal waits: the first `set_height` scrolls the
    /// least that shows the selected row, once.
    #[test]
    fn a_reveal_before_the_first_draw_waits_for_the_height() {
        let g = groups(&[("b", 2), ("d", 3)]);
        let mut l = ListState::default();
        l.select(&g, 1);
        l.select(&g, 6);
        assert_eq!(l.scroll(&g), 0, "nothing scrolled without a height");
        l.set_height(&g, 3);
        assert_eq!((l.scroll(&g), l.selected_row(&g)), (4, Some(6)));
        l.set_scroll(&g, 0);
        l.set_height(&g, 3);
        assert_eq!(l.scroll(&g), 0, "only once");
        l.set_height(&g, 2);
        assert_eq!(l.scroll(&g), 0, "a resize doesn't reveal again");
        // Hidden (no rows drawn) again: the next reveal waits for the next draw.
        l.set_height(&g, 0);
        l.select(&g, 6);
        assert_eq!(l.scroll(&g), 0);
        l.set_height(&g, 3);
        assert_eq!(l.scroll(&g), 4);

        let mut l = ListState::default();
        l.select(&g, 1);
        l.set_height(&g, 3);
        assert_eq!(l.scroll(&g), 0, "the file's row above stays in view");
    }
}
