//! The Explorer's "File History" section's model (no drawing; `app/history.rs` drives
//! it, `ui/side.rs` draws it): the active file's saved snapshots, newest first, above "Git
//! History" — the scroll, selection, collapse, focus and divider state live here, the rows are
//! the entries read off-thread by the app (`history::entries`).
//!
//! Like Git History the list follows the active file (a diff tab counts as its file). The
//! selection is an entry id, so it survives a re-read after a save added an entry.

use std::path::PathBuf;

use crate::history::Entry;
use crate::scrollbar::{Scroll, Scrollable};

/// The section's height (divider, header and rows) before it is dragged.
pub const DEFAULT_SECTION_HEIGHT: u16 = 6;

#[derive(Debug, Clone)]
pub struct FileHistoryView {
    /// The file the list is for (absolute); `None`: no file is open (or a log / commit tab).
    pub path: Option<PathBuf>,
    /// The file's entries, newest first.
    pub entries: Vec<Entry>,
    /// Entries are being read.
    pub loading: bool,
    /// The last read's error (drawn instead of the rows).
    pub error: Option<String>,
    /// Numbers the file's list and its reads: a reset or a history change drops older results.
    pub generation: u64,
    /// The file's history changed (or it was never read, being hidden): read when shown.
    pub stale: bool,
    /// Rows the list drew in the last frame; 0 until the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
    /// The section's height as dragged, clamped by drawing (`side_sections`).
    pub section_height: u16,
    collapsed: bool,
    /// The section, not the tree, has the keys while the side panel is focused.
    focused: bool,
    top: usize,
    /// The selected entry's id.
    selected: Option<String>,
}

impl Default for FileHistoryView {
    fn default() -> Self {
        Self {
            path: None,
            entries: Vec::new(),
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

/// The section's scroll bar: its state (`FileHistoryView::scrollable`).
pub struct FileHistoryScroll<'a> {
    view: &'a mut FileHistoryView,
}

impl Scrollable for FileHistoryScroll<'_> {
    fn scroll(&self) -> Scroll {
        Scroll::rows(self.view.entries.len(), self.view.height, self.view.scroll())
    }

    fn set_top(&mut self, top: usize) {
        self.view.top = top;
        self.view.clamp();
    }
}

impl FileHistoryView {
    pub fn scrollable(&mut self) -> FileHistoryScroll<'_> {
        FileHistoryScroll { view: self }
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The selected entry's id.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected row's index, if the entry is still in the list.
    pub fn selected_row(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.entries.iter().position(|e| e.id == id)
    }

    /// Selects row `row` (clamped; nothing when there are no rows) and scrolls to it.
    pub fn select_row(&mut self, row: usize) {
        let Some(entry) = self.entries.get(row.min(self.entries.len().saturating_sub(1))) else {
            self.selected = None;
            return;
        };
        self.selected = Some(entry.id.clone());
        self.reveal_selected();
    }

    /// Moves the selection by `delta` rows (clamped); nothing selected selects the first row.
    pub fn move_selection(&mut self, delta: isize) {
        match self.selected_row() {
            Some(row) => {
                let row = row
                    .saturating_add_signed(delta)
                    .min(self.entries.len().saturating_sub(1));
                self.select_row(row);
            }
            None => self.select_row(0),
        }
    }

    pub fn first(&mut self) {
        self.select_row(0);
    }

    pub fn last(&mut self) {
        self.select_row(self.entries.len().saturating_sub(1));
    }

    /// The top row, clamped so the last row can reach the bottom.
    pub fn scroll(&self) -> usize {
        self.top.min(self.entries.len().saturating_sub(self.height.max(1)))
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

    /// A new active file (or none): clears the list and its error, keeping the collapse, focus
    /// and size; nothing is selected.
    pub fn reset(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.entries.clear();
        self.loading = false;
        self.error = None;
        self.stale = false;
        self.top = 0;
        self.selected = None;
        self.generation += 1;
    }

    /// A re-read's entries replace the list; the selection stays while its entry is there.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        if self.selected_row().is_none() {
            self.selected = None;
        }
        self.clamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            time: 0,
            size: 1,
            hash: String::new(),
        }
    }

    fn model(n: usize) -> FileHistoryView {
        FileHistoryView {
            entries: (0..n).map(|i| entry(&format!("e{i}"))).collect(),
            height: 5,
            ..FileHistoryView::default()
        }
    }

    #[test]
    fn starts_open_and_unfocused() {
        let mut v = FileHistoryView::default();
        assert!(!v.collapsed() && !v.focused());
        assert_eq!(v.section_height, DEFAULT_SECTION_HEIGHT);
        v.toggle_collapsed();
        assert!(v.collapsed());
    }

    #[test]
    fn selection_moves_and_reveals_the_least() {
        let mut v = model(20);
        v.move_selection(1);
        assert_eq!(v.selected_row(), Some(0), "nothing selected: the first row");
        v.move_selection(7);
        assert_eq!((v.selected_row(), v.scroll()), (Some(7), 3));
        v.move_selection(-100);
        assert_eq!((v.selected_row(), v.scroll()), (Some(0), 0));
        v.last();
        assert_eq!((v.selected_row(), v.scroll()), (Some(19), 15));
        v.first();
        assert_eq!((v.selected_row(), v.scroll(), v.selected()), (Some(0), 0, Some("e0")));
    }

    #[test]
    fn reveal_before_the_first_draw_waits_for_set_height() {
        let mut v = model(20);
        v.height = 0;
        v.select_row(10);
        assert_eq!(v.top, 0);
        v.set_height(4);
        assert_eq!(v.scroll(), 7);
    }

    #[test]
    fn re_read_keeps_the_selection_by_id() {
        let mut v = model(3);
        v.select_row(1);
        let mut entries = vec![entry("new")];
        entries.extend(v.entries.clone());
        v.set_entries(entries);
        assert_eq!(v.selected_row(), Some(2));
        v.set_entries(vec![entry("x")]);
        assert_eq!(v.selected(), None);
    }

    #[test]
    fn scrolling_clamps_and_reset_clears() {
        let mut v = model(8);
        v.scroll_by(100);
        assert_eq!(v.scroll(), 3);
        v.scrollable().set_top(1);
        assert_eq!(v.scroll(), 1);
        assert_eq!(v.scrollable().scroll(), Scroll::rows(8, 5, 1));
        let g = v.generation;
        v.reset(None);
        assert!(v.entries.is_empty() && v.generation == g + 1);
    }
}
