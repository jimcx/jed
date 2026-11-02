//! The Explorer's "File History" section's model (no drawing; `ui/side.rs` draws it): the
//! active file's saved snapshots, above "Git History". For now the collapse, the dragged height
//! and the focus; the rows come with the next subtask.

/// The section's height (divider, header and rows) before it is dragged.
pub const DEFAULT_SECTION_HEIGHT: u16 = 6;

#[derive(Debug, Clone)]
pub struct FileHistoryView {
    /// The section's height as dragged, clamped by drawing (`side_sections`).
    pub section_height: u16,
    collapsed: bool,
    /// The section, not the tree, has the keys while the side panel is focused.
    focused: bool,
}

impl Default for FileHistoryView {
    fn default() -> Self {
        Self {
            section_height: DEFAULT_SECTION_HEIGHT,
            collapsed: false,
            focused: false,
        }
    }
}

impl FileHistoryView {
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "read by the section's keys (history_view-c)"))]
    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_open_and_unfocused() {
        let mut v = FileHistoryView::default();
        assert!(!v.collapsed() && !v.focused());
        assert_eq!(v.section_height, DEFAULT_SECTION_HEIGHT);
        v.toggle_collapsed();
        assert!(v.collapsed());
    }
}
