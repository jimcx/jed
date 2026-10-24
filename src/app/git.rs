//! Git status, kept current off the UI thread: one `git::refresh` at a time in
//! `spawn_blocking`, started at startup and after file system events that can change it
//! (debounced with them, `FS_DEBOUNCE`); events arriving while one runs are coalesced into one
//! more run after it. The first read is just HEAD (`refresh_git_head`, µs), so the branch shows
//! at once; the full working-tree walk follows `STATUS_DEBOUNCE` later in the background (or on
//! demand, `ensure_status_loaded`), so the decorations pop in a moment after startup.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use super::{App, AppEvent, SLOW_TASK};
use crate::git::{self, Refreshed};

/// How long after the cheap head read the full working-tree walk is deferred: the branch shows
/// at once, and the decorations pop in this much later (in the background).
const STATUS_DEBOUNCE: Duration = Duration::from_millis(200);

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
    /// When the deferred full walk is due (set after the head read; cleared once it runs).
    status_deadline: Option<Instant>,
}

impl GitState {
    /// A refresh is running or queued (tests wait for it to end).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> bool {
        self.running || self.queued.is_some()
    }

    /// When the deferred full walk is due, if one is pending.
    pub fn status_deadline(&self) -> Option<Instant> {
        self.status_deadline
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
        // A full read supersedes the deferred one (the timer and `ensure_status_loaded` both
        // land here; clearing now keeps a second trigger from queuing a duplicate read).
        self.git.status_deadline = None;
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let result = crate::perf::time("git.status", || git::refresh(&root, changed.as_deref()))
                .map_err(|e| e.message().to_string());
            let _ = tx.send(AppEvent::GitRefreshed {
                result,
                elapsed: t.elapsed(),
            });
        });
    }

    /// Reads only the repository and HEAD (µs) for the status bar, before the full working-tree
    /// walk runs. Called at startup; the full status (`refresh_git`) runs on demand after that
    /// (`ensure_status_loaded`, or a file system event).
    pub(super) fn refresh_git_head(&mut self) {
        if self.mode.is_single() {
            return;
        }
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = crate::perf::time("git.head", || git::read_head(&root));
            let _ = tx.send(AppEvent::GitHead(result));
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
        self.git.status_deadline = None;
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
        // A commit, checkout, reset or another repository: the file's history is stale, and so
        // are the staged markers (they compare the index against HEAD).
        if self.git_head() != head_before {
            self.git_history_head_changed();
            self.quick_diff_head_changed();
        }
        if let Some(changed) = self.git.queued.take() {
            self.refresh_git(changed);
        }
    }

    /// `AppEvent::GitHead`: the cheap head read landed. It fills the status bar's branch, but
    /// never downgrades a full status that arrived first.
    pub(super) fn git_head_read(&mut self, result: Option<git::Status>) {
        let Some(head) = result else {
            // No repository, or HEAD unreadable: nothing to show until a full read says so.
            return;
        };
        self.watch_git_dir(&head.repo.git_dir);
        let head_before = self.git_head();
        if self.git.status.as_ref().is_none_or(|s| !s.loaded) {
            self.git.status = Some(head);
            // The branch is shown; the full walk follows shortly, in the background.
            self.git.status_deadline = Some(Instant::now() + STATUS_DEBOUNCE);
        }
        if self.git_head() != head_before {
            self.git_history_head_changed();
        }
    }

    /// The full status (`entries`, decorations) is wanted now: read it unless it's already
    /// loaded or one is running. The cheap head read leaves `loaded` false, so the Source
    /// Control view and change diffs call this to trigger the working-tree walk on demand (a
    /// huge repository's walk never runs until then or its deferred timer). Without a
    /// repository (or before the head read lands) there is nothing to read.
    pub(super) fn ensure_status_loaded(&mut self) {
        if self.git.status.as_ref().is_some_and(|s| !s.loaded) && !self.git.running() {
            self.refresh_git(None);
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

    #[tokio::test(flavor = "multi_thread")]
    async fn head_read_fills_the_branch_and_the_full_walk_runs_on_demand() {
        let (_d, root, mut app) = setup();
        fs::write(root.join("a"), "changed\n").unwrap();
        // The cheap head read gives the branch without walking the working tree.
        app.refresh_git_head();
        assert!(app.settle(|a| a.git.status.is_some()).await);
        let status = app.git.status.as_ref().unwrap();
        assert_eq!(status.head.name(), "main");
        assert!(!status.loaded, "only HEAD was read");
        assert!(status.entries.is_empty(), "no entries yet");
        // A full read is still `ensure_status_loaded` away (it never downgrades, so a
        // head-only status is what a demand upgrades from).
        app.ensure_status_loaded();
        assert!(app.settle(|a| a.git.status.as_ref().is_some_and(|s| s.loaded)).await);
        let status = app.git.status.as_ref().unwrap();
        assert!(status.loaded);
        assert_eq!(status.entries.len(), 1);
        assert!(status.entry(Path::new("a")).is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_head_read_after_a_full_status_does_not_downgrade_it() {
        let (_d, root, mut app) = setup();
        // A full status (the working-tree walk) has already loaded: it knows `a` changed.
        let repo = git::Repo::discover(&root).unwrap();
        let full = git::Status::new(
            repo.clone(),
            git::Head::Branch("main".into()),
            vec![git::Entry {
                path: PathBuf::from("a"),
                staged: Some(git::Change::Modified),
                unstaged: None,
            }],
        );
        app.git.status = Some(full);
        // The startup's cheap head read lands afterwards, with a different HEAD id: it must not
        // replace the loaded status with a head-only one (only `None` or a not-yet-loaded status
        // is upgraded).
        app.git_head_read(Some(git::Status::head_only(
            repo,
            git::Head::Branch("main".into()),
            Some("deadbeef".into()),
        )));
        let status = app.git.status.as_ref().unwrap();
        assert!(status.loaded, "the full status stays loaded");
        assert_eq!(status.entries.len(), 1, "its entries survive the late head read");
        assert!(status.entry(Path::new("a")).is_some());
        assert_eq!(status.head_id, None, "the head-only status's id did not overwrite it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_full_walk_is_deferred_but_runs_in_the_background() {
        let (_d, root, mut app) = setup();
        fs::write(root.join("a"), "changed\n").unwrap();
        // The cheap head read shows the branch at once and arms the deferred full walk.
        app.refresh_git_head();
        assert!(app.settle(|a| a.git.status.is_some()).await);
        assert!(!app.git.status.as_ref().unwrap().loaded, "only HEAD was read");
        assert!(app.git.status_deadline().is_some(), "the full walk is scheduled");
        // Let the timer fire: the full walk runs in the background and the change appears.
        assert!(app.settle(|a| a.git.status.as_ref().is_some_and(|s| s.loaded)).await);
        let status = app.git.status.as_ref().unwrap();
        assert_eq!(status.entries.len(), 1);
        assert!(status.entry(Path::new("a")).is_some());
        assert!(app.git.status_deadline().is_none(), "the deadline is cleared");
    }
}
