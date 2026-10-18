//! The Explorer view's "Git History" section in the app (`app.git_history`, a
//! `git_history::GitHistory`; drawn by `ui/side.rs`): it follows the active file (a diff tab
//! counts as its file), reads the file's commits in pages of `git::log::PAGE` off the UI thread
//! as the list scrolls, reloads page 1 when HEAD moves (a commit, checkout or reset outside
//! jed), and handles the section's keys (`Context::GitHistory`) and mouse (its divider, header
//! and rows). Nothing is read without a repository and a path inside it: the section says
//! "No history".

use std::path::{Path, PathBuf};

use super::{App, AppEvent};
use crate::command::GitHistoryCommand as C;
use crate::git::Repo;
use crate::git::log;
use crate::layout::SidePanel;
use crate::scrollbar::Scrollable;

impl App {
    /// Each loop turn (`start`, `run`, `pump` / `settle`): follows the active file, reloads the
    /// history after HEAD moved, and reads the next page once the list is near its end. Nothing
    /// without a tokio runtime.
    pub(super) fn sync_git_history(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let path = self.active_path();
        if path != self.git_history.path {
            let found = path.as_deref().and_then(|p| self.history_repo(p));
            self.git_history.reset(path);
            if let Some((repo, rel)) = found {
                self.history_page(repo, rel, 0);
            }
            return;
        }
        if self.git_history.stale && !self.git_history.loading {
            self.git_history.stale = false;
            let Some(path) = self.git_history.path.clone() else {
                return;
            };
            match self.history_repo(&path) {
                // The old rows stay up until the first page replaces them.
                Some((repo, rel)) => self.history_page(repo, rel, 0),
                None => self.git_history.reset(Some(path)),
            }
            return;
        }
        if self.git_history.loading || self.git_history.error.is_some() || !self.git_history.more {
            return;
        }
        // The next page when the end of the list is on screen (or a page didn't fill it).
        let len = self.git_history.commits.len();
        if len == 0 {
            return;
        }
        let height = self.git_history.height.max(1);
        if self.git_history.scroll() + height + 1 < len {
            return;
        }
        let Some(path) = self.git_history.path.clone() else {
            return;
        };
        match self.history_repo(&path) {
            Some((repo, rel)) => self.history_page(repo, rel, len),
            None => self.git_history.reset(Some(path)),
        }
    }

    /// The repository and `path` relative to its work tree (`None`: no repository yet, or the
    /// path is outside it).
    fn history_repo(&self, path: &Path) -> Option<(Repo, PathBuf)> {
        let status = self.git.status.as_ref()?;
        let rel = status.repo.relative(path)?;
        Some((status.repo.clone(), rel))
    }

    /// Reads `skip`'s page of the active file's commits off the UI thread. Older generations'
    /// results are dropped (`git_history_done`).
    fn history_page(&mut self, repo: Repo, rel: PathBuf, skip: usize) {
        let Some(path) = self.git_history.path.clone() else {
            return;
        };
        let generation = self.git_history.generation;
        self.git_history.loading = true;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = log::commits(&repo, &rel, skip, log::PAGE).map_err(|e| e.message().to_string());
            let _ = tx.send(AppEvent::GitHistory {
                path,
                skip,
                generation,
                result,
            });
        });
    }

    /// A page arrived: the first replaces the list (keeping the selected commit when it is still
    /// there, else selecting the first; the top only goes back to 0 when the first commit
    /// changed, so a commit made outside jed shows), later ones append.
    pub(super) fn git_history_done(
        &mut self,
        path: PathBuf,
        skip: usize,
        generation: u64,
        result: Result<log::Page, String>,
    ) {
        if generation != self.git_history.generation || self.git_history.path.as_ref() != Some(&path) {
            return;
        }
        self.git_history.loading = false;
        match result {
            Ok(page) => {
                self.git_history.error = None;
                if skip == 0 {
                    let old_first = self.git_history.commits.first().map(|c| c.id.clone());
                    let reload = !self.git_history.commits.is_empty();
                    let selected = self.git_history.selected().map(str::to_string);
                    self.git_history.commits = page.commits;
                    let first = self.git_history.commits.first().map(|c| c.id.clone());
                    if reload {
                        // Keep the selection while its commit is still there (it may have
                        // moved down); else the first, so a commit made outside jed shows.
                        let there = selected
                            .as_deref()
                            .is_some_and(|id| self.git_history.commits.iter().any(|c| c.id == id));
                        self.git_history
                            .set_selected(if there { selected } else { first.clone() });
                        if old_first != first {
                            // A new top commit: show it (and the top of the list).
                            self.git_history.scrollable().set_top(0);
                        }
                    } else {
                        // A file's first page: nothing selected until a click or a key.
                        self.git_history.set_selected(None);
                        self.git_history.scrollable().set_top(0);
                    }
                    self.git_history.more = page.more;
                } else if skip == self.git_history.commits.len() {
                    // Pages only ever follow the loaded list: no overlap, no duplicates.
                    self.git_history.commits.extend(page.commits);
                    self.git_history.more = page.more;
                }
            }
            Err(e) => {
                self.git_history.error = Some(e);
                self.git_history.more = false;
            }
        }
    }

    /// A status read found a different HEAD (or repository, or lost it): the next sync reloads
    /// the first page.
    pub(super) fn git_history_head_changed(&mut self) {
        self.git_history.stale = true;
    }

    pub(super) fn git_history_command(&mut self, cmd: C) {
        let page = self.git_history.height.max(1) as isize;
        match cmd {
            C::Up => self.git_history.move_selection(-1),
            C::Down => self.git_history.move_selection(1),
            C::PageUp => self.git_history.move_selection(-page),
            C::PageDown => self.git_history.move_selection(page),
            C::First => self.git_history.first(),
            C::Last => self.git_history.last(),
            C::Open => {
                // Nothing selected (the section was just focused): Enter opens the first
                // commit, as a click on it would.
                if self.git_history.selected().is_none() {
                    self.git_history.select_row(0);
                }
                let Some(id) = self.git_history.selected().map(str::to_string) else {
                    return;
                };
                let file = self.git_history_file();
                self.open_commit(&id, file);
            }
            C::FocusTree => self.git_history.set_focused(false),
        }
    }

    /// The history's file (the active file), work-tree-relative: what a commit tab opened from
    /// the section highlights and opens first (`None`: unknown).
    pub(super) fn git_history_file(&self) -> Option<String> {
        let path = self.git_history.path.as_ref()?;
        let (_, rel) = self.history_repo(path)?;
        Some(rel.to_string_lossy().into_owned())
    }

    /// The history section has the keys (the side panel shows the Explorer and the section was
    /// clicked or focused).
    pub(super) fn git_history_focused(&self) -> bool {
        self.ui.focus == super::Focus::SidePanel
            && self.ui.side_panel == SidePanel::Explorer
            && self.git_history.focused()
    }
}
