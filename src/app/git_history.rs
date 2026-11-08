//! The Explorer view's "Git History" section in the app (`app.git_history`, a
//! `git_history::GitHistory`; drawn by `ui/side.rs`): it follows the active file (a diff tab
//! counts as its file), reads the file's commits in pages of `git::log::PAGE` off the UI thread
//! as the list scrolls, reloads page 1 when HEAD moves (a commit, checkout or reset outside
//! jed), and handles the section's keys (`Context::GitHistory`) and mouse (its divider, header
//! and rows). Nothing is read without a repository and a path inside it: the section says
//! "No history".

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;

use super::git::GitEvent;
use super::{App, AppEvent};
use crate::command::GitHistoryCommand as C;
use crate::git::Repo;
use crate::git::log;
use crate::git_history::{GitHistory, HistoryList, HistoryStream};
use crate::layout::SidePanel;
use crate::scrollbar::Scrollable;
use crate::side_sections::{Section, dragged_height, split};

/// A fresh list's staged commits become the rows (see `App::history_chunk` for the selection and
/// top rules).
fn replace_rows(h: &mut GitHistory, list: &HistoryList) {
    let old_first = h.commits.first().map(|c| c.id.clone());
    let reload = !h.commits.is_empty();
    let selected = h.selected().map(str::to_string);
    h.commits = std::mem::take(&mut h.staged);
    h.fresh = false;
    let first = h.commits.first().map(|c| c.id.clone());
    let there = selected
        .as_deref()
        .is_some_and(|id| h.commits.iter().any(|c| c.id == id));
    if reload {
        match (list, there) {
            (_, true) => {}
            (HistoryList::Branch, false) => h.set_selected(None),
            (HistoryList::File(_), false) => h.set_selected(first.clone()),
        }
        if old_first != first {
            h.scrollable().set_top(0);
        }
    } else {
        h.set_selected(None);
        h.scrollable().set_top(0);
    }
}

/// Test hooks on the walk jobs: counts walks started and jobs exited, and holds a job after each
/// chunk it sends until released (so a test sees a list mid-walk without timing luck).
#[cfg(test)]
#[derive(Default)]
pub struct StreamProbe {
    pub walks: std::sync::atomic::AtomicUsize,
    pub exited: std::sync::atomic::AtomicUsize,
    /// Chunks the jobs sent.
    pub sent: std::sync::atomic::AtomicUsize,
    held: std::sync::Mutex<bool>,
    released: std::sync::Condvar,
}

#[cfg(test)]
impl StreamProbe {
    /// Jobs stop after each chunk they send until `hold(false)`.
    pub fn hold(&self, on: bool) {
        *self.held.lock().unwrap() = on;
        self.released.notify_all();
    }

    fn wait_if_held(&self) {
        let mut held = self.held.lock().unwrap();
        while *held {
            held = self.released.wait(held).unwrap();
        }
    }
}

impl App {
    /// Each loop turn (`start`, `run`, `pump` / `settle`): follows the active file, restarts the
    /// walk after HEAD moved, and asks it for another page once the list is near its end. Only
    /// while the section is on screen: hidden or collapsed, a new file or HEAD only marks it
    /// `stale`, read when it shows again. Nothing without a tokio runtime.
    pub(super) fn sync_git_history(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let path = self.active_path();
        let shown = self.ui.side_visible && self.ui.side_panel == SidePanel::Explorer && !self.git_history.collapsed();
        if path != self.git_history.path {
            let found = path.as_deref().and_then(|p| self.history_repo(p));
            self.git_history.reset(path.clone());
            match (found, path) {
                (Some((repo, rel)), Some(path)) if shown => {
                    self.start_history(HistoryList::File(path), repo, Some(rel))
                }
                (Some(_), _) => self.git_history.stale = true,
                (None, _) => {}
            }
            return;
        }
        if !shown {
            return;
        }
        if self.git_history.stale {
            self.git_history.stale = false;
            let Some(path) = self.git_history.path.clone() else {
                return;
            };
            match self.history_repo(&path) {
                // The old rows stay up until the new walk has covered them.
                Some((repo, rel)) => self.start_history(HistoryList::File(path), repo, Some(rel)),
                None => self.git_history.reset(Some(path)),
            }
            return;
        }
        self.git_history.want_more();
    }

    /// The repository and `path` relative to its work tree (`None`: no repository yet, or the
    /// path is outside it).
    fn history_repo(&self, path: &Path) -> Option<(Repo, PathBuf)> {
        let status = self.git.status.as_ref()?;
        let rel = status.repo.relative(path)?;
        Some((status.repo.clone(), rel))
    }

    /// Starts `list`'s walk (`rel`: the file, work-tree relative; `None`: the branch) in one
    /// kept-alive job, replacing a running one (its flag is set, and its in-flight chunks are
    /// dropped: the generation changes). The job sends `GitEvent::HistoryChunk`s and, once it has
    /// sent what the list asked for, waits for `GitHistory::want_more`'s requests. The old rows
    /// stay up until the new commits hold the selected one (else the first) or reach a page (`history_chunk`).
    pub(super) fn start_history(&mut self, list: HistoryList, repo: Repo, rel: Option<PathBuf>) {
        let h = match list {
            HistoryList::File(_) => &mut self.git_history,
            HistoryList::Branch => &mut self.branch_history,
        };
        h.stream = None;
        h.generation += 1;
        h.fresh = true;
        h.staged.clear();
        h.loading = true;
        let generation = h.generation;
        let wanted = log::PAGE.max(h.commits.len());
        let (want_tx, want_rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        h.stream = Some(HistoryStream::new(cancel.clone(), want_tx, wanted));
        let tx = self.tx.clone();
        #[cfg(test)]
        let probe = self.stream_probe.clone();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            probe.walks.fetch_add(1, Ordering::SeqCst);
            let send = |result: Result<log::Chunk, String>| {
                let sent = tx
                    .send(AppEvent::Git(GitEvent::HistoryChunk {
                        list: list.clone(),
                        generation,
                        result,
                    }))
                    .is_ok();
                #[cfg(test)]
                {
                    probe.sent.fetch_add(1, Ordering::SeqCst);
                    probe.wait_if_held();
                }
                sent
            };
            match log::Walker::new(&repo, rel.as_deref()) {
                Ok(walker) => log::stream(walker, wanted, &want_rx, &cancel, send),
                Err(e) => {
                    send(Err(e.message().to_string()));
                }
            }
            #[cfg(test)]
            probe.exited.fetch_add(1, Ordering::SeqCst);
        });
    }

    /// A chunk of a list's walk arrived. The first replaces the list once it covers the rows
    /// that were there (a reload; at once for an empty list): keeping the selected commit while
    /// it is still there — the Explorer's section selects the first otherwise, so a commit made
    /// outside jed shows; the branch pane selects nothing — and the top only going back to 0
    /// when the first commit changed. A file's first list selects nothing. Later chunks append.
    pub(super) fn history_chunk(&mut self, list: HistoryList, generation: u64, result: Result<log::Chunk, String>) {
        let h = match &list {
            HistoryList::File(path) if self.git_history.path.as_ref() == Some(path) => &mut self.git_history,
            HistoryList::File(_) => return,
            HistoryList::Branch => &mut self.branch_history,
        };
        if generation != h.generation {
            return;
        }
        let wanted = h.stream.as_ref().map_or(0, HistoryStream::wanted);
        match result {
            Ok(chunk) => {
                h.error = None;
                h.more = !chunk.done;
                if h.fresh {
                    h.staged.extend(chunk.commits);
                    // Swap once the new rows hold the commit the selection / top rules look for
                    // (the selected, else the first of the old list), or fill a page.
                    let anchor = h.selected().or_else(|| h.commits.first().map(|c| c.id.as_str()));
                    let covered = anchor.is_none_or(|id| h.staged.iter().any(|c| c.id == id));
                    if chunk.done || covered || h.staged.len() >= log::PAGE {
                        replace_rows(h, &list);
                    }
                } else {
                    h.commits.extend(chunk.commits);
                }
                h.loading = h.fresh || (!chunk.done && h.commits.len() < wanted);
                if chunk.done {
                    h.stream = None;
                }
            }
            Err(e) => {
                h.error = Some(e);
                h.more = false;
                h.loading = false;
                h.fresh = false;
                h.staged.clear();
                h.stream = None;
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

    /// The Explorer's sections under the tree, top to bottom (File History, Git History), as
    /// `side_sections::split` takes them.
    pub fn side_sections(&self) -> [Section; 2] {
        [
            Section {
                height: self.file_history_view.section_height,
                collapsed: self.file_history_view.collapsed(),
            },
            Section {
                height: self.git_history.section_height,
                collapsed: self.git_history.collapsed(),
            },
        ]
    }

    /// A divider drag of section `i` (see `side_sections`) with the pointer on `row`: the
    /// section reaches the body's end (the side panel's last row, above the status bar), less the
    /// sections below it.
    pub(super) fn drag_side_section(&mut self, i: usize, row: u16) {
        let Some(side) = self.hits.regions.side else { return };
        let bottom = side.y + side.height;
        let body = bottom.saturating_sub(side.y + 2);
        let (_, heights) = split(body, &self.side_sections());
        let below: u16 = heights[i + 1..].iter().sum();
        let want = bottom.saturating_sub(below).saturating_sub(row);
        let height = dragged_height(body, &heights, i, want);
        match i {
            0 => self.file_history_view.section_height = height,
            _ => self.git_history.section_height = height,
        }
    }

    /// The history section has the keys (the side panel shows the Explorer and the section was
    /// clicked or focused).
    pub(super) fn git_history_focused(&self) -> bool {
        self.ui.focus == super::Focus::SidePanel
            && self.ui.side_panel == SidePanel::Explorer
            && self.git_history.focused()
    }
}
