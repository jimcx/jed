//! The Source Control view's commit history pane (`app.branch_history`, a
//! `git_history::GitHistory` without a file; drawn under the changes by `ui/scm.rs` through
//! `ui/side.rs`'s history rows): the current branch's commits (`git::log::branch`), read in pages
//! of `git::log::PAGE` off the UI thread as the list scrolls, reloaded when HEAD moves. Only read
//! while the pane is on screen (side bar shown, Source Control view, not collapsed). A click on a
//! row opens the commit tab (`open_commit`), as a Git History row does; the pane's header,
//! divider, wheel and bar are the Explorer section's (`Hit::History*`, told apart by the shown
//! view: `App::side_history`).

use super::git::GitEvent;
use super::{App, AppEvent};
use crate::git::log;
use crate::git_history::GitHistory;
use crate::layout::SidePanel;
use crate::scrollbar::Scrollable;
use crate::side_sections::{Section, dragged_height, split};

impl App {
    /// The history list the side panel shows: the branch's under Source Control, else the
    /// active file's.
    pub(super) fn side_history(&mut self) -> &mut GitHistory {
        if self.ui.side_panel == SidePanel::SourceControl {
            &mut self.branch_history
        } else {
            &mut self.git_history
        }
    }

    /// Each loop turn: reads the branch's first page when the pane shows (or after HEAD
    /// moved), and the next once the list is near its end. Nothing without a runtime or a
    /// repository.
    pub(super) fn sync_branch_history(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.git.status.is_none() {
            let h = &mut self.branch_history;
            if !h.commits.is_empty() || h.loading {
                h.reset(None);
            }
            h.stale = true;
            return;
        }
        let shown =
            self.ui.side_visible && self.ui.side_panel == SidePanel::SourceControl && !self.branch_history.collapsed();
        if !shown || self.branch_history.loading {
            return;
        }
        let h = &self.branch_history;
        let skip = if h.stale {
            0
        } else if h.error.is_none() && h.more && h.scroll() + h.height.max(1) + 1 >= h.commits.len() {
            h.commits.len()
        } else {
            return;
        };
        self.branch_history.stale = false;
        self.branch_page(skip);
    }

    fn branch_page(&mut self, skip: usize) {
        let Some(repo) = self.git.status.as_ref().map(|s| s.repo.clone()) else {
            return;
        };
        let generation = self.branch_history.generation;
        self.branch_history.loading = true;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = log::branch(&repo, skip, log::PAGE).map_err(|e| e.message().to_string());
            let _ = tx.send(AppEvent::Git(GitEvent::BranchHistory {
                skip,
                generation,
                result,
            }));
        });
    }

    /// A page arrived: the first replaces the list (keeping the selected commit while it is
    /// there), later ones append.
    pub(super) fn branch_history_done(&mut self, skip: usize, generation: u64, result: Result<log::Page, String>) {
        let h = &mut self.branch_history;
        if generation != h.generation {
            return;
        }
        h.loading = false;
        match result {
            Ok(page) => {
                h.error = None;
                if skip == 0 {
                    let old_first = h.commits.first().map(|c| c.id.clone());
                    let selected = h.selected().map(str::to_string);
                    h.commits = page.commits;
                    h.more = page.more;
                    if selected.is_some_and(|id| !h.commits.iter().any(|c| c.id == id)) {
                        h.set_selected(None);
                    }
                    if old_first != h.commits.first().map(|c| c.id.clone()) {
                        h.scrollable().set_top(0);
                    }
                } else if skip == h.commits.len() {
                    h.commits.extend(page.commits);
                    h.more = page.more;
                }
            }
            Err(e) => {
                h.error = Some(e);
                h.more = false;
            }
        }
    }

    /// HEAD (or the repository) changed: the next sync reloads the first page.
    pub(super) fn branch_history_head_changed(&mut self) {
        self.branch_history.stale = true;
    }

    /// The Source Control view's list and the pane under it, as `side_sections::split` takes
    /// them.
    pub fn scm_sections(&self) -> [Section; 1] {
        [Section {
            height: self.branch_history.section_height,
            collapsed: self.branch_history.collapsed(),
        }]
    }

    /// A drag of the pane's divider with the pointer on `row`: the pane reaches the panel's
    /// last row.
    pub(super) fn drag_scm_section(&mut self, row: u16) {
        let Some(side) = self.hits.regions.side else { return };
        let bottom = side.y + side.height;
        let body = bottom.saturating_sub(side.y + 2);
        let (_, heights) = split(body, &self.scm_sections());
        let want = bottom.saturating_sub(row);
        self.branch_history.section_height = dragged_height(body, &heights, 0, want);
    }
}
