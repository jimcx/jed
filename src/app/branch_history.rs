//! The Source Control view's commit history pane (`app.branch_history`, a
//! `git_history::GitHistory` without a file; drawn under the changes by `ui/scm.rs` through
//! `ui/side.rs`'s history rows): the current branch's commits (`git::log::Walker` with no path),
//! streamed off the UI thread (`start_history`) as the list scrolls, restarted when HEAD moves. Only read
//! while the pane is on screen (side bar shown, Source Control view, not collapsed). A click on a
//! row opens the commit tab (`open_commit`), as a Git History row does; the pane's header,
//! divider, wheel and bar are the Explorer section's (`Hit::History*`, told apart by the shown
//! view: `App::side_history`).

use super::App;
use crate::git_history::{GitHistory, HistoryList};
use crate::layout::SidePanel;
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

    /// Each loop turn: starts the branch's walk when the pane shows (or after HEAD moved), and
    /// asks it for another page once the list is near its end. Nothing without a runtime or a
    /// repository.
    pub(super) fn sync_branch_history(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let Some(repo) = self.git.status.as_ref().map(|s| s.repo.clone()) else {
            let h = &mut self.branch_history;
            if !h.commits.is_empty() || h.loading {
                h.reset(None);
            }
            h.stale = true;
            return;
        };
        let shown =
            self.ui.side_visible && self.ui.side_panel == SidePanel::SourceControl && !self.branch_history.collapsed();
        if !shown {
            return;
        }
        if self.branch_history.stale {
            self.branch_history.stale = false;
            self.start_history(HistoryList::Branch, repo, None);
        } else {
            self.branch_history.want_more();
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
