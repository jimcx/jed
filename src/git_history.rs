//! The Explorer's "Git History" section's model (no drawing; `app/git_history.rs` drives it,
//! `ui/side.rs` draws it): the active file's commits, newest first, as pages of
//! [`git::log::PAGE`](crate::git::log::PAGE) — the scroll, selection, collapse and divider
//! state live here, the rows are the loaded commits.
//!
//! The list follows the active file (a diff tab counts as its file) and is read off-thread by
//! the app; nothing here touches git. The scroll and selection are plain indices: paging only
//! ever appends, so loading a page never moves them.

use std::path::PathBuf;

use crate::git::log::Commit;
use crate::scrollbar::{Scroll, Scrollable};

/// The section's height (divider, header and rows) before it is dragged.
pub const DEFAULT_SECTION_HEIGHT: u16 = 10;
/// Rows the tree keeps when the section shows.
pub const MIN_TREE_HEIGHT: u16 = 3;
/// The section's own smallest height: its divider and header.
pub const MIN_SECTION_HEIGHT: u16 = 2;

#[derive(Debug, Clone)]
pub struct GitHistory {
    /// The file the list is for (absolute); `None`: no file is open (or it's a log tab).
    pub path: Option<PathBuf>,
    /// The loaded commits, newest first.
    pub commits: Vec<Commit>,
    /// The history goes on after the loaded commits.
    pub more: bool,
    /// A page is being read.
    pub loading: bool,
    /// The last page's error (drawn instead of the rows).
    pub error: Option<String>,
    /// Numbers the file's history and its page reads: a reset or a refresh drops older
    /// results.
    pub generation: u64,
    /// HEAD (or the repository) changed: the next sync reloads the first page.
    pub stale: bool,
    /// Rows the list drew in the last frame (page up / down, paging; `set_height`); 0 until
    /// the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
    /// The section's height (divider, header and rows) as dragged, clamped by drawing.
    pub section_height: u16,
    collapsed: bool,
    /// The section, not the tree, has the keys while the side panel is focused.
    focused: bool,
    /// The top row's index (clamped by drawing and scrolling).
    top: usize,
    /// The selected commit's id.
    selected: Option<String>,
}

impl Default for GitHistory {
    fn default() -> Self {
        Self {
            path: None,
            commits: Vec::new(),
            more: false,
            loading: false,
            error: None,
            generation: 0,
            stale: false,
            height: 0,
            reveal_pending: false,
            section_height: DEFAULT_SECTION_HEIGHT,
            collapsed: false,
            focused: false,
            top: 0,
            selected: None,
        }
    }
}

/// The section's scroll bar: its state (`GitHistory::scrollable`).
pub struct HistoryScroll<'a> {
    history: &'a mut GitHistory,
}

impl Scrollable for HistoryScroll<'_> {
    fn scroll(&self) -> Scroll {
        Scroll::rows(self.history.commits.len(), self.history.height, self.history.scroll())
    }

    fn set_top(&mut self, top: usize) {
        self.history.top = top;
        self.history.clamp();
    }
}

impl GitHistory {
    /// The loaded commits.
    pub fn rows(&self) -> &[Commit] {
        &self.commits
    }

    /// The section's scroll bar over the loaded commits.
    pub fn scrollable(&mut self) -> HistoryScroll<'_> {
        HistoryScroll { history: self }
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// The section's collapse, as a session restores it.
    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    /// The section (not the tree) has the keys.
    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The selected commit's id.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected row's index, if the commit is still in the list.
    pub fn selected_row(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.commits.iter().position(|c| c.id == id)
    }

    /// Sets the selected commit's id without scrolling to it (a reload keeps a selection whose
    /// row may be off screen).
    pub fn set_selected(&mut self, id: Option<String>) {
        self.selected = id;
    }

    /// Selects the commit `id` (false when it isn't loaded) and scrolls to it.
    #[cfg_attr(not(test), allow(dead_code))] // its tests select by id
    pub fn select(&mut self, id: &str) -> bool {
        match self.commits.iter().position(|c| c.id == id) {
            Some(row) => {
                self.select_row(row);
                true
            }
            None => false,
        }
    }

    /// Selects row `row` (clamped; nothing when there are no rows) and scrolls to it.
    pub fn select_row(&mut self, row: usize) {
        let Some(commit) = self.commits.get(row.min(self.commits.len().saturating_sub(1))) else {
            self.selected = None;
            return;
        };
        self.selected = Some(commit.id.clone());
        self.reveal_selected();
    }

    /// Moves the selection by `delta` rows (clamped; Up at the top stays); nothing selected
    /// selects the first row.
    pub fn move_selection(&mut self, delta: isize) {
        match self.selected_row() {
            Some(row) => {
                let row = row
                    .saturating_add_signed(delta)
                    .min(self.commits.len().saturating_sub(1));
                self.select_row(row);
            }
            None => self.select_row(0),
        }
    }

    pub fn first(&mut self) {
        self.select_row(0);
    }

    pub fn last(&mut self) {
        self.select_row(self.commits.len().saturating_sub(1));
    }

    /// The top row, clamped so the last row can reach the bottom.
    pub fn scroll(&self) -> usize {
        self.top.min(self.commits.len().saturating_sub(self.height.max(1)))
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.top = self.scroll().saturating_add_signed(delta);
        self.clamp();
    }

    /// Re-clamps the scroll (the list or its height changed).
    pub fn clamp(&mut self) {
        self.top = self.scroll();
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height(&mut self, height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.reveal_selected();
        }
    }

    /// Scrolls the least that shows the selected row; before the first draw (no height yet),
    /// waits for `set_height`.
    pub fn reveal_selected(&mut self) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
        let Some(row) = self.selected_row() else { return };
        let (top, height) = (self.scroll(), self.height);
        if row < top {
            self.top = row;
        } else if row >= top + height {
            self.top = row + 1 - height;
        }
    }

    /// A new active file (or none): clears the list and its error, keeping the collapse, focus,
    /// size and selection state out of it — a new file starts with nothing selected.
    pub fn reset(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.commits.clear();
        self.more = false;
        self.loading = false;
        self.error = None;
        self.stale = false;
        self.top = 0;
        self.selected = None;
        self.generation += 1;
    }

    /// Splits a body of `height` rows into the tree's rows and the section's (divider, header
    /// and commits): `section_height`, clamped so the tree keeps [`MIN_TREE_HEIGHT`] rows (only
    /// for this body: a taller one gets the dragged height back). `None` when not both fit: the
    /// tree takes the body. A collapsed section is just its divider and header.
    pub fn split(&self, height: u16) -> Option<(u16, u16)> {
        if height < MIN_TREE_HEIGHT + MIN_SECTION_HEIGHT {
            return None;
        }
        if self.collapsed {
            return Some((height - MIN_SECTION_HEIGHT, MIN_SECTION_HEIGHT));
        }
        let section = self.section_height.clamp(MIN_SECTION_HEIGHT, height - MIN_TREE_HEIGHT);
        Some((height - section, section))
    }

    /// Sets the section's height from a divider drag: `want` rows for a body of `body` rows,
    /// clamped so the tree keeps [`MIN_TREE_HEIGHT`].
    pub fn set_section_height(&mut self, want: u16, body: u16) {
        if body < MIN_TREE_HEIGHT + MIN_SECTION_HEIGHT {
            return;
        }
        self.section_height = want.clamp(MIN_SECTION_HEIGHT, body - MIN_TREE_HEIGHT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str) -> Commit {
        Commit {
            id: id.into(),
            short: id[..7].into(),
            summary: format!("commit {id}"),
            author: "t".into(),
            time: 0,
        }
    }

    fn model(n: usize) -> GitHistory {
        GitHistory {
            commits: (0..n).map(|i| commit(&format!("{i:040}"))).collect(),
            height: 5,
            ..GitHistory::default()
        }
    }

    #[test]
    fn selection_moves_and_reveals_the_least() {
        let mut h = model(20);
        h.move_selection(1);
        assert_eq!(h.selected_row(), Some(0), "nothing selected: the first row");
        assert_eq!(h.scroll(), 0);
        h.move_selection(7);
        assert_eq!((h.selected_row(), h.scroll()), (Some(7), 3));
        h.move_selection(-100);
        assert_eq!((h.selected_row(), h.scroll()), (Some(0), 0), "Up at the top stays");
        h.move_selection(100);
        assert_eq!((h.selected_row(), h.scroll()), (Some(19), 15));
        h.first();
        assert_eq!((h.selected_row(), h.scroll()), (Some(0), 0));
        h.last();
        assert_eq!((h.selected_row(), h.scroll()), (Some(19), 15));
        assert!(h.select(&format!("{:040}", 12)));
        assert_eq!((h.selected_row(), h.scroll()), (Some(12), 12));
        assert!(!h.select("nope"));
        assert_eq!(h.selected_row(), Some(12), "an unknown id changes nothing");
    }

    #[test]
    fn scroll_by_and_the_scroll_bar_clamp() {
        let mut h = model(20);
        h.scroll_by(-100);
        assert_eq!(h.scroll(), 0);
        h.scroll_by(100);
        assert_eq!(h.scroll(), 15);
        h.scrollable().set_top(7);
        assert_eq!(h.scroll(), 7);
        assert_eq!(h.selected_row(), None, "scrolling never selects");
        let s = h.scrollable().scroll();
        assert_eq!((s.top, s.height, s.max_top, s.shown), (7, 5, 15, true));
        // Nothing selected: the first row, the top unchanged where it shows.
        h.move_selection(1);
        assert_eq!((h.selected_row(), h.scroll()), (Some(0), 0));
    }

    #[test]
    fn collapsing_and_the_divider_split_the_body() {
        let mut h = model(20);
        assert_eq!(h.split(20), Some((10, 10)), "the default 10");
        assert_eq!(h.section_height, 10);
        // Too short for both: the tree gets it all.
        assert_eq!(h.split(4), None);
        // The tree keeps at least three rows: the section is clamped, for that body only.
        assert_eq!(h.split(8), Some((3, 5)));
        assert_eq!(h.section_height, 10);
        assert_eq!(h.split(20), Some((10, 10)), "a taller body: the height back");
        // A dragged height is clamped both ways.
        h.set_section_height(100, 20);
        assert_eq!(h.section_height, 17);
        h.set_section_height(0, 20);
        assert_eq!(h.section_height, 2);
        h.set_section_height(9, 20);
        assert_eq!(h.split(20), Some((11, 9)));
        // Collapsed: just the divider and header; the dragged height comes back.
        h.toggle_collapsed();
        assert_eq!(h.split(20), Some((18, 2)));
        assert_eq!(h.section_height, 9);
        h.toggle_collapsed();
        assert_eq!(h.split(20), Some((11, 9)));
        // A body too short for both while collapsed is hidden too.
        assert_eq!(h.split(3), None);
    }

    #[test]
    fn reset_clears_the_list_but_keeps_the_section() {
        let mut h = model(20);
        h.height = 7;
        h.select(&format!("{:040}", 4));
        h.scroll_by(10);
        h.more = true;
        h.loading = true;
        h.error = Some("boom".into());
        h.stale = true;
        h.toggle_collapsed();
        h.set_focused(true);
        let path = PathBuf::from("/r/a.txt");
        h.reset(Some(path.clone()));
        assert_eq!(h.path, Some(path));
        assert!(h.rows().is_empty());
        assert!(!h.more && !h.loading && !h.stale && h.error.is_none());
        assert_eq!((h.selected(), h.scroll(), h.height), (None, 0, 7));
        assert!(h.collapsed() && h.focused(), "the section's state is kept");
        assert_eq!(h.generation, 1, "older pages are dropped");
        h.reset(None);
        assert_eq!(h.generation, 2);
        assert_eq!(h.path, None);
        // A selection gone from a shrunk list isn't reported.
        let mut h = model(5);
        h.select(&format!("{:040}", 4));
        h.commits.truncate(2);
        assert_eq!(h.selected_row(), None);
    }

    /// Before the first draw (no height) a reveal waits for `set_height`.
    #[test]
    fn a_reveal_before_the_first_draw_waits_for_the_height() {
        let mut h = GitHistory { height: 0, ..model(20) };
        h.select_row(0);
        h.move_selection(1);
        h.set_height(5);
        assert_eq!((h.scroll(), h.selected_row()), (0, Some(1)));

        let mut h = GitHistory { height: 0, ..model(20) };
        h.select_row(9);
        assert_eq!(h.top, 0);
        h.set_height(5);
        assert_eq!(h.scroll(), 5, "the selected row at the bottom");
        h.top = 0;
        h.set_height(4);
        assert_eq!(h.scroll(), 0, "a resize doesn't reveal again");
        // Hidden (no rows drawn) again: the next reveal waits for the next draw.
        h.set_height(0);
        h.select_row(15);
        assert_eq!(h.top, 0);
        h.set_height(5);
        assert_eq!(h.scroll(), 11);
    }
}
