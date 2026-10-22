//! Git status, kept current off the UI thread: one `git::refresh` at a time in
//! `spawn_blocking`, started at startup and after file system events that can change it
//! (debounced with them, `FS_DEBOUNCE`); events arriving while one runs are coalesced into one
//! more run after it.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use super::{App, AppEvent, SLOW_TASK};
use crate::git::{self, Refreshed};

/// The repository's last status and the refresh in flight.
#[derive(Default)]
pub struct GitState {
    /// The last status read; `None` before the first one finishes, or with no repository (git
    /// features off).
    pub status: Option<git::Status>,
    /// A refresh is running.
    running: bool,
    /// What the next refresh must look at, once the running one ends: `Some(None)` = read
    /// regardless, `Some(Some(paths))` = only if one of these can change the status.
    queued: Option<Option<Vec<PathBuf>>>,
    /// The git directory being watched besides the workspace root (it was outside it).
    watched: Option<PathBuf>,
}

impl GitState {
    /// A refresh is running or queued (tests wait for it to end).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> bool {
        self.running || self.queued.is_some()
    }
}

impl App {
    /// Reads the status again: unconditionally (`changed: None`), or if one of the changed paths
    /// can change it (not `.gitignore`d, not git's objects / logs / locks). Coalesced with a
    /// running refresh.
    pub(super) fn refresh_git(&mut self, changed: Option<Vec<PathBuf>>) {
        // Single file mode reads no git status (no branch, gutter markers or blame).
        if self.mode.is_single() {
            return;
        }
        if self.git.running {
            self.git.queued = Some(match (self.git.queued.take(), changed) {
                (None, c) => c,
                (Some(Some(mut a)), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                _ => None,
            });
            return;
        }
        self.git.running = true;
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let result = git::refresh(&root, changed.as_deref()).map_err(|e| e.message().to_string());
            let _ = tx.send(AppEvent::GitRefreshed {
                result,
                elapsed: t.elapsed(),
            });
        });
    }

    /// File system events (batched): refreshes the status if any can change it. Before a
    /// repository is known, only a `.git` appearing (`git init`) starts a look — or any event
    /// while the first look runs (it may have read the files before the change).
    pub(super) fn git_paths_changed(&mut self, paths: &[PathBuf]) {
        match &self.git.status {
            Some(s) => {
                let relevant: Vec<PathBuf> = paths.iter().filter(|p| s.repo.affects_status(p)).cloned().collect();
                if !relevant.is_empty() {
                    self.refresh_git(Some(relevant));
                }
            }
            None if self.git.running || paths.iter().any(|p| in_dot_git(p)) => self.refresh_git(None),
            None => {}
        }
    }

    pub(super) fn git_refreshed(&mut self, result: Result<Refreshed, String>, elapsed: Duration) {
        self.git.running = false;
        let head_before = self.git_head();
        if elapsed > SLOW_TASK {
            tracing::warn!(elapsed_ms = elapsed.as_millis() as u64, "slow git status");
        }
        match result {
            Ok(Refreshed::NoRepo) => {
                if self.git.status.take().is_some() {
                    tracing::info!("no git repository any more");
                    self.quick_diff_index_changed();
                    self.blame_head_changed();
                    self.diffs_index_changed();
                    self.scm_status_changed();
                }
            }
            Ok(Refreshed::Unchanged) => {}
            Ok(Refreshed::Status(s)) => {
                tracing::debug!(
                    head = s.head.name(),
                    changed = s.entries.len(),
                    elapsed_ms = elapsed.as_millis() as u64,
                    "git status"
                );
                self.watch_git_dir(&s.repo.git_dir);
                self.git.status = Some(*s);
                // The index may have changed: the gutter markers and diff tabs compare against it.
                self.quick_diff_index_changed();
                self.blame_head_changed();
                self.diffs_index_changed();
                self.scm_status_changed();
            }
            Err(e) => tracing::warn!("git status failed: {e}"),
        }
        // A commit, checkout, reset or another repository: the file's history is stale.
        if self.git_head() != head_before {
            self.git_history_head_changed();
        }
        if let Some(changed) = self.git.queued.take() {
            self.refresh_git(changed);
        }
    }

    /// What the history section reloads on: the repository and its HEAD commit (or unborn).
    fn git_head(&self) -> Option<(PathBuf, Option<String>)> {
        self.git
            .status
            .as_ref()
            .map(|s| (s.repo.workdir.clone(), s.head_id.clone()))
    }

    /// Watches the git directory when the root's watch doesn't cover it (the workspace is a
    /// subdirectory of the work tree), so staging and branch switches outside jed are seen.
    fn watch_git_dir(&mut self, git_dir: &Path) {
        if git_dir.starts_with(&self.workspace.root) || self.git.watched.as_deref() == Some(git_dir) {
            return;
        }
        let Some(w) = &mut self.watcher else { return };
        match w.watch(git_dir) {
            Ok(()) => tracing::info!(dir = %git_dir.display(), "watching the git directory"),
            Err(e) => tracing::warn!(dir = %git_dir.display(), "can't watch the git directory: {e}"),
        }
        self.git.watched = Some(git_dir.to_path_buf());
    }
}

fn in_dot_git(p: &Path) -> bool {
    p.components().any(|c| c == Component::Normal(".git".as_ref()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::git::test_repo::{commit_all, init};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    /// A repository with one commit, and an app on it that isn't started (no watcher, no
    /// first refresh): the test drives `refresh_git` itself.
    fn setup() -> (tempfile::TempDir, PathBuf, App) {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let root = base.join("proj");
        fs::create_dir(&root).unwrap();
        init(&root);
        fs::write(root.join("a"), "a\n").unwrap();
        commit_all(&root, "one");
        let app = App::new(
            Workspace::load(root.clone()),
            TrustStore::load(base.join("trust.toml")),
            None,
            true,
        );
        (dir, root, app)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refreshes_while_one_runs_are_queued_and_merged() {
        let (_d, root, mut app) = setup();
        app.refresh_git(Some(vec![root.join("a")]));
        assert!(app.git.running && app.git.queued.is_none());
        app.refresh_git(Some(vec![root.join("b")]));
        app.refresh_git(Some(vec![root.join("c")]));
        assert_eq!(app.git.queued, Some(Some(vec![root.join("b"), root.join("c")])));
        // A read regardless wins over path lists, before or after it.
        app.refresh_git(None);
        app.refresh_git(Some(vec![root.join("d")]));
        assert_eq!(app.git.queued, Some(None));
        // The queued one runs when the first ends, then nothing is left.
        assert!(app.settle(|a| !a.git.running()).await);
        assert_eq!(app.git.status.as_ref().unwrap().head.name(), "main");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_queued_refresh_starts_when_the_running_one_ends() {
        let (_d, root, mut app) = setup();
        // As if a refresh that found nothing to do were ending, with one queued behind it.
        app.git.running = true;
        app.git.queued = Some(None);
        app.git_refreshed(Ok(Refreshed::Unchanged), Duration::ZERO);
        assert!(app.git.running, "the queued refresh started");
        assert!(app.git.queued.is_none());
        assert!(app.settle(|a| !a.git.running()).await);
        assert_eq!(app.git.status.as_ref().unwrap().repo.workdir, root);
        // With nothing queued, an ending refresh starts nothing.
        app.git.running = true;
        app.git_refreshed(Ok(Refreshed::Unchanged), Duration::ZERO);
        assert!(!app.git.running());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changes_during_the_first_look_are_not_lost() {
        let (_d, root, mut app) = setup();
        app.refresh_git(None);
        assert!(app.git.status.is_none());
        // A work tree file changes while the first status is read: it may have been read
        // already, so another read follows.
        app.git_paths_changed(&[root.join("a")]);
        assert_eq!(app.git.queued, Some(None));
        assert!(app.settle(|a| !a.git.running()).await);
        // Without a repository and nothing running, only a `.git` path starts a look.
        let mut idle = GitState::default();
        std::mem::swap(&mut app.git, &mut idle);
        app.git_paths_changed(&[root.join("a")]);
        assert!(!app.git.running());
        app.git_paths_changed(&[root.join(".git/HEAD")]);
        assert!(app.git.running());
        assert!(app.settle(|a| !a.git.running()).await);
    }
}
