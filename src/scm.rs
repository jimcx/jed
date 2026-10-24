//! The Source Control view's model (no drawing; `app/scm.rs` drives it, `ui/scm.rs` draws it):
//! the git status as VS Code's sections — "Merge Changes" (conflicted files, only while there
//! are any), "Staged Changes" (index against HEAD, only while it has files) and "Changes" (the
//! work tree against the index: modified, deleted, untracked `U` files, VS Code's
//! `git.untrackedChanges: mixed`; conflicted `!` files are in Merge Changes instead) — each a
//! header row (title, count, collapsible) followed by its files sorted by path.
//!
//! The scroll and selection are kept by [`RowKey`] (section + path), not by row number, so a
//! status read that adds or drops files above them doesn't move them. `grouped::ListState`
//! doesn't fit: its keys are a group's path and an item *offset*, and here the items (files)
//! are what must be followed, the groups being three fixed sections. When the selected file
//! leaves its section (staged, resolved, or its change undone), the selection goes to the next
//! file in that section (by path), else its last, else the section's header, else the first row.

use std::path::PathBuf;

use crate::diff_view::Pair;
use crate::git::{Change, Entry, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    MergeChanges,
    Staged,
    Changes,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::MergeChanges, Section::Staged, Section::Changes];

    pub fn title(self) -> &'static str {
        match self {
            Section::MergeChanges => "Merge Changes",
            Section::Staged => "Staged Changes",
            Section::Changes => "Changes",
        }
    }

    /// The entry's change shown in this section.
    pub fn change(self, e: &Entry) -> Option<Change> {
        match self {
            Section::MergeChanges => (e.unstaged == Some(Change::Conflicted)).then_some(Change::Conflicted),
            Section::Staged => e.staged,
            Section::Changes => e.unstaged.filter(|c| *c != Change::Conflicted),
        }
    }

    /// What a file's diff compares: the index against HEAD, or the work tree against the index
    /// (an untracked file: everything added).
    pub fn pair(self) -> Pair {
        match self {
            // Unused: opening a conflicted file opens the file, not a diff.
            Section::MergeChanges => Pair::UNSTAGED,
            Section::Staged => Pair::STAGED,
            Section::Changes => Pair::UNSTAGED,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A row by what it shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    Header(Section),
    /// A file (relative to the work tree) in a section.
    File(Section, PathBuf),
}

impl RowKey {
    pub fn section(&self) -> Section {
        match self {
            RowKey::Header(s) | RowKey::File(s, _) => *s,
        }
    }
}

/// One row of the list, borrowing the status.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Header {
        section: Section,
        count: usize,
        collapsed: bool,
    },
    File {
        section: Section,
        entry: &'a Entry,
        change: Change,
    },
}

impl Row<'_> {
    pub fn section(&self) -> Section {
        match self {
            Row::Header { section, .. } | Row::File { section, .. } => *section,
        }
    }

    pub fn key(&self) -> RowKey {
        match self {
            Row::Header { section, .. } => RowKey::Header(*section),
            Row::File { section, entry, .. } => RowKey::File(*section, entry.path.clone()),
        }
    }

    /// Whether this row is the one `key` names (no allocation).
    pub fn is(&self, key: &RowKey) -> bool {
        match (self, key) {
            (Row::Header { section, .. }, RowKey::Header(s)) => section == s,
            (Row::File { section, entry, .. }, RowKey::File(s, p)) => section == s && entry.path == *p,
            _ => false,
        }
    }
}

/// The files of `section`, in path order.
pub fn files(status: &Status, section: Section) -> impl Iterator<Item = (&Entry, Change)> {
    status
        .entries
        .iter()
        .filter_map(move |e| section.change(e).map(|c| (e, c)))
}

/// The view's state: collapsed sections, scroll, selection, the row under the mouse.
#[derive(Debug, Clone, Default)]
pub struct ScmView {
    collapsed: [bool; 3],
    /// The top row's index (clamped by drawing).
    top: usize,
    selected: Option<RowKey>,
    /// Rows shown in the last frame (set by drawing, `set_height`; page up / down); 0 until
    /// the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
    /// The row under the mouse pointer (its buttons are drawn), from the last move.
    pub hover: Option<RowKey>,
}

/// The view's scroll bar: its state with its rows (`ScmView::scrollable`).
pub struct ScmScroll<'a, 'r> {
    view: &'a mut ScmView,
    rows: &'a [Row<'r>],
}

impl crate::scrollbar::Scrollable for ScmScroll<'_, '_> {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.rows.len(), self.view.height, self.view.scroll(self.rows))
    }

    fn set_top(&mut self, top: usize) {
        self.view.top = top;
        self.view.clamp(self.rows);
    }
}

impl ScmView {
    /// The list's scroll bar over `rows` (this view's `rows(status)`).
    pub fn scrollable<'a, 'r>(&'a mut self, rows: &'a [Row<'r>]) -> ScmScroll<'a, 'r> {
        ScmScroll { view: self, rows }
    }

    pub fn collapsed(&self, section: Section) -> bool {
        self.collapsed[section.index()]
    }

    pub fn toggle(&mut self, section: Section) {
        self.collapsed[section.index()] ^= true;
    }

    pub fn set_collapsed(&mut self, section: Section, collapsed: bool) {
        self.collapsed[section.index()] = collapsed;
    }

    /// Every row: Merge Changes (only with files), Staged Changes (only with files), then
    /// Changes (always); none at all when nothing changed.
    pub fn rows<'a>(&self, status: &'a Status) -> Vec<Row<'a>> {
        let mut rows = Vec::new();
        if status.entries.is_empty() {
            return rows;
        }
        for section in Section::ALL {
            let count = files(status, section).count();
            if count == 0 && section != Section::Changes {
                continue;
            }
            let collapsed = self.collapsed(section);
            rows.push(Row::Header {
                section,
                count,
                collapsed,
            });
            if !collapsed {
                rows.extend(files(status, section).map(|(entry, change)| Row::File { section, entry, change }));
            }
        }
        rows
    }

    /// The selected row's index: the row its key names, else (the file left its section) the
    /// next file there by path, else its last file, else the section's header, else the first
    /// row. `None` with nothing selected or no rows.
    pub fn selected_row(&self, rows: &[Row]) -> Option<usize> {
        let key = self.selected.as_ref()?;
        if rows.is_empty() {
            return None;
        }
        if let Some(i) = rows.iter().position(|r| r.is(key)) {
            return Some(i);
        }
        let section = key.section();
        let mut last = None;
        for (i, r) in rows.iter().enumerate() {
            if let Row::File { section: s, entry, .. } = r
                && *s == section
            {
                if let RowKey::File(_, path) = key
                    && entry.path > *path
                {
                    return Some(i);
                }
                last = Some(i);
            }
        }
        let header = rows.iter().position(|r| r.is(&RowKey::Header(section)));
        Some(last.or(header).unwrap_or(0))
    }

    /// The selected row itself.
    pub fn selected<'a>(&self, rows: &[Row<'a>]) -> Option<Row<'a>> {
        self.selected_row(rows).map(|i| rows[i])
    }

    /// Whether something is selected (its row may have gone).
    pub fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    /// Selects row `row` (clamped; nothing when there are none) and scrolls to it.
    pub fn select(&mut self, rows: &[Row], row: usize) {
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        self.selected = Some(rows[row.min(rows.len() - 1)].key());
        self.reveal_selected(rows);
    }

    /// Selects the row `key` names, if it's in the list.
    pub fn select_key(&mut self, rows: &[Row], key: &RowKey) -> bool {
        match rows.iter().position(|r| r.is(key)) {
            Some(i) => {
                self.select(rows, i);
                true
            }
            None => false,
        }
    }

    /// The selection follows to where `selected_row` puts it (after a status read), so a file
    /// coming back later doesn't pull it back.
    pub fn settle(&mut self, rows: &[Row]) {
        if self.selected.is_some() {
            match self.selected_row(rows) {
                Some(i) => {
                    let keep = self.top;
                    self.select(rows, i);
                    self.top = keep;
                }
                None => self.selected = None,
            }
        }
    }

    /// Moves the selection by `delta` rows (clamped); nothing selected selects the first row.
    pub fn move_selection(&mut self, rows: &[Row], delta: isize) {
        match self.selected_row(rows) {
            Some(i) => self.select(rows, i.saturating_add_signed(delta)),
            None => self.select(rows, 0),
        }
    }

    /// The top row (clamped so the last row can reach the bottom).
    pub fn scroll(&self, rows: &[Row]) -> usize {
        self.top.min(rows.len().saturating_sub(self.height.max(1)))
    }

    pub fn scroll_by(&mut self, rows: &[Row], delta: isize) {
        self.top = self.scroll(rows).saturating_add_signed(delta);
        self.top = self.scroll(rows);
    }

    /// Re-clamps the scroll (the list or its height changed).
    pub fn clamp(&mut self, rows: &[Row]) {
        self.top = self.scroll(rows);
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height(&mut self, rows: &[Row], height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.reveal_selected(rows);
        }
    }

    /// Scrolls the least that shows the selected row; before the first draw (no height yet),
    /// waits for `set_height`.
    pub fn reveal_selected(&mut self, rows: &[Row]) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
        let Some(row) = self.selected_row(rows) else { return };
        let (top, height) = (self.scroll(rows), self.height);
        if row < top {
            self.top = row;
        } else if row >= top + height {
            self.top = row + 1 - height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Head, Repo};

    fn status(entries: &[(&str, Option<Change>, Option<Change>)]) -> Status {
        let repo = Repo {
            workdir: PathBuf::from("/r"),
            git_dir: PathBuf::from("/r/.git"),
        };
        let entries = entries
            .iter()
            .map(|(p, staged, unstaged)| Entry {
                path: PathBuf::from(p),
                staged: *staged,
                unstaged: *unstaged,
            })
            .collect();
        Status::new(repo, Head::Branch("main".into()), entries)
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Header { section, count, .. } => format!("{} {count}", section.title()),
                Row::File { entry, change, .. } => {
                    format!("  {} {}", entry.path.display(), change.decoration().letter())
                }
            })
            .collect()
    }

    const M: Option<Change> = Some(Change::Modified);
    const A: Option<Change> = Some(Change::Added);
    const U: Option<Change> = Some(Change::Untracked);
    const D: Option<Change> = Some(Change::Deleted);
    const C: Option<Change> = Some(Change::Conflicted);

    #[test]
    fn sections_counts_and_collapsing() {
        let s = status(&[
            ("a", None, M),
            ("b", A, M),
            ("c", None, U),
            ("d", D, None),
            ("e", None, C),
        ]);
        let mut v = ScmView::default();
        assert_eq!(
            keys(&v.rows(&s)),
            [
                "Merge Changes 1",
                "  e !",
                "Staged Changes 2",
                "  b A",
                "  d D",
                "Changes 3",
                "  a M",
                "  b M",
                "  c U"
            ]
        );
        v.toggle(Section::Staged);
        assert_eq!(
            keys(&v.rows(&s)),
            [
                "Merge Changes 1",
                "  e !",
                "Staged Changes 2",
                "Changes 3",
                "  a M",
                "  b M",
                "  c U"
            ]
        );
        // Nothing staged: no staged section; nothing at all: no rows.
        let s = status(&[("a", None, M)]);
        assert_eq!(keys(&v.rows(&s)), ["Changes 1", "  a M"]);
        assert!(v.rows(&status(&[])).is_empty());
        // Only staged files: an empty Changes section stays.
        let s = status(&[("a", M, None)]);
        v.toggle(Section::Staged);
        assert_eq!(keys(&v.rows(&s)), ["Staged Changes 1", "  a M", "Changes 0"]);
        // Only a conflict: Merge Changes first, then the empty Changes.
        let s = status(&[("e", None, C)]);
        assert_eq!(keys(&v.rows(&s)), ["Merge Changes 1", "  e !", "Changes 0"]);
        assert_eq!(Section::MergeChanges.pair(), Pair::UNSTAGED);
    }

    #[test]
    fn selection_follows_its_file_and_falls_back_in_its_section() {
        let mut v = ScmView {
            height: 10,
            ..ScmView::default()
        };
        let s = status(&[("b", None, M), ("c", None, M), ("d", None, M)]);
        let rows = v.rows(&s);
        v.select(&rows, 2); // c
        // A file arrives above it, and a staged section: still on c.
        let s2 = status(&[("a", None, U), ("b", M, None), ("c", None, M), ("d", None, M)]);
        let rows = v.rows(&s2);
        assert_eq!(keys(&rows)[v.selected_row(&rows).unwrap()], "  c M");
        // c is staged: the next file in Changes (d, now at c's place) is selected.
        let s3 = status(&[("a", None, U), ("b", None, M), ("c", M, None), ("d", None, M)]);
        let rows = v.rows(&s3);
        v.settle(&rows);
        let i = v.selected_row(&rows).unwrap();
        assert_eq!((keys(&rows)[i].as_str(), i), ("  d M", 5));
        // c comes back: the selection stays on d (settled there).
        let rows = v.rows(&s2);
        assert_eq!(keys(&rows)[v.selected_row(&rows).unwrap()], "  d M");
        // The last file goes: the one before takes its place; then the header; nothing: none.
        let s4 = status(&[("b", None, M)]);
        let rows = v.rows(&s4);
        assert_eq!(keys(&rows)[v.selected_row(&rows).unwrap()], "  b M");
        let s5 = status(&[("b", M, None)]);
        let rows = v.rows(&s5);
        assert_eq!(keys(&rows)[v.selected_row(&rows).unwrap()], "Changes 0");
        let s6 = status(&[]);
        let rows = v.rows(&s6);
        assert_eq!(v.selected_row(&rows), None);
    }

    #[test]
    fn moving_and_scrolling() {
        let entries: Vec<(String, Option<Change>, Option<Change>)> =
            (0..20).map(|i| (format!("f{i:02}"), None, M)).collect();
        let entries: Vec<(&str, _, _)> = entries.iter().map(|(p, a, b)| (p.as_str(), *a, *b)).collect();
        let s = status(&entries);
        let mut v = ScmView {
            height: 5,
            ..ScmView::default()
        };
        let rows = v.rows(&s);
        v.move_selection(&rows, 1);
        assert_eq!(v.selected_row(&rows), Some(0), "nothing selected: the first row");
        v.move_selection(&rows, 7);
        assert_eq!((v.selected_row(&rows), v.scroll(&rows)), (Some(7), 3));
        v.move_selection(&rows, 100);
        assert_eq!((v.selected_row(&rows), v.scroll(&rows)), (Some(20), 16));
        v.scroll_by(&rows, -100);
        assert_eq!(v.scroll(&rows), 0);
        v.scroll_by(&rows, 100);
        assert_eq!(v.scroll(&rows), 16);
        assert!(v.select_key(&rows, &RowKey::Header(Section::Changes)));
        assert_eq!(v.scroll(&rows), 0);
        assert!(!v.select_key(&rows, &RowKey::Header(Section::Staged)));
    }

    /// The scroll bar's view: rows, height, top; `set_top` clamps as the wheel does and never
    /// moves the selection.
    #[test]
    fn the_scroll_bar_reads_and_sets_the_top() {
        use crate::scrollbar::Scrollable;
        let entries: Vec<String> = (0..20).map(|i| format!("f{i:02}")).collect();
        let entries: Vec<(&str, _, _)> = entries.iter().map(|p| (p.as_str(), None, M)).collect();
        let s = status(&entries);
        let mut v = ScmView {
            height: 5,
            ..ScmView::default()
        };
        let rows = v.rows(&s);
        v.select(&rows, 2);
        v.scrollable(&rows).set_top(7);
        assert_eq!(v.scroll(&rows), 7);
        let sc = v.scrollable(&rows).scroll();
        assert_eq!((sc.top, sc.height, sc.max_top, sc.shown), (7, 5, 16, true));
        v.scrollable(&rows).set_top(100);
        assert_eq!(v.top, 16, "clamped where the wheel ends");
        assert_eq!(v.selected_row(&rows), Some(2));
    }

    /// Before the first draw (no height) a reveal waits for `set_height`: the "Changes" header
    /// above the first file stays in view; a deep selection is scrolled to the least.
    #[test]
    fn a_reveal_before_the_first_draw_waits_for_the_height() {
        let entries: Vec<(String, Option<Change>, Option<Change>)> =
            (0..20).map(|i| (format!("f{i:02}"), None, M)).collect();
        let entries: Vec<(&str, _, _)> = entries.iter().map(|(p, a, b)| (p.as_str(), *a, *b)).collect();
        let s = status(&entries);
        let mut v = ScmView::default();
        let rows = v.rows(&s);
        v.select(&rows, 1);
        assert_eq!(v.top, 0);
        v.set_height(&rows, 5);
        assert_eq!((v.scroll(&rows), v.selected_row(&rows)), (0, Some(1)));

        let mut v = ScmView::default();
        v.select(&rows, 12);
        v.set_height(&rows, 5);
        assert_eq!(v.scroll(&rows), 8, "the selected row at the bottom");
        v.scroll_by(&rows, -8);
        v.set_height(&rows, 5);
        assert_eq!(v.scroll(&rows), 0, "only once");
    }
}
