//! Earlier versions of a file, opened in a read-only tab (`Buffer::version`): where a version's
//! bytes come from (`VersionSource`: a file history snapshot; a commit's blob joins it for a
//! commit's file) and the reads both `diff_view`'s history / commit sides and the version tab
//! use. No app state, nothing drawn.

use std::path::{Path, PathBuf};

use git2::{Oid, Repository};

use crate::history::{self, Snapshot};

/// Where a version's text comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VersionSource {
    /// A file history entry (`history::read_snapshot`).
    Snapshot(Snapshot),
    /// The file's blob in a commit; `rel` is its work-tree-relative path there.
    Commit { id: Oid, rel: PathBuf },
}

/// What a version tab (`Buffer::version`) shows and can be read again from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDoc {
    /// The file the version is of (absolute): its title, language and highlighting come from it,
    /// but nothing keyed by a path (saves, git, servers, backups) ever sees it.
    pub file: PathBuf,
    pub source: VersionSource,
    /// The title's parenthesis: `History: 3 min ago`.
    pub label: String,
}

impl VersionDoc {
    /// The version of `file` from `source`, labelled as of `now` (ms).
    pub fn new(file: PathBuf, source: VersionSource, now: u64) -> VersionDoc {
        let label = source.label(now);
        VersionDoc { file, source, label }
    }

    /// The tab's title: `a.rs (History: 3 min ago)`.
    pub fn title(&self) -> String {
        let name = self
            .file
            .file_name()
            .map_or_else(|| self.file.display().to_string(), |n| n.to_string_lossy().into_owned());
        format!("{name} ({})", self.label)
    }
}

impl VersionSource {
    /// The title label as of `now` (ms).
    pub fn label(&self, now: u64) -> String {
        match self {
            VersionSource::Snapshot(s) => format!("History: {}", history::ago(s.time, now)),
            VersionSource::Commit { id, .. } => crate::diff_view::short_commit(*id),
        }
    }

    /// The version's bytes of `file` (inside `root`).
    ///
    /// Blocking: call from spawn_blocking.
    pub fn read(&self, root: &Path, file: &Path) -> Result<Vec<u8>, String> {
        match self {
            VersionSource::Snapshot(s) => read_snapshot(root, file, *s),
            VersionSource::Commit { id, rel } => read_commit_file(root, *id, rel),
        }
    }
}

/// The bytes of the history entry `snapshot` of `file` (inside `root`).
///
/// Blocking: call from spawn_blocking.
pub fn read_snapshot(root: &Path, file: &Path, snapshot: Snapshot) -> Result<Vec<u8>, String> {
    let rel = history::relative(root, file).ok_or("outside the workspace")?;
    history::read_snapshot(root, &rel, snapshot)
}

/// The blob of `rel` in commit `id` of the repository containing `root`, as an error message
/// when the repository, the commit or the path is gone.
///
/// Blocking: call from spawn_blocking.
pub fn read_commit_file(root: &Path, id: Oid, rel: &Path) -> Result<Vec<u8>, String> {
    let short = crate::diff_view::short_commit(id);
    let repo = crate::git::Repo::discover(root).ok_or("not in a git repository")?;
    let repo = repo.open().map_err(|e| e.to_string())?;
    if repo.find_commit(id).is_err() {
        return Err(format!("commit {short} is gone"));
    }
    match commit_blob(&repo, id, rel).map_err(|e| e.to_string())? {
        Some(bytes) => Ok(bytes),
        None => Err(format!("{} isn't in commit {short}", rel.display())),
    }
}

/// The blob of `rel` in commit `id`'s tree; `None` when the commit has no such path (a deleted
/// file, or a root commit's empty-tree side).
///
/// Blocking: call from spawn_blocking.
pub fn commit_blob(repo: &Repository, id: Oid, rel: &Path) -> Result<Option<Vec<u8>>, git2::Error> {
    let tree = repo.find_commit(id)?.tree()?;
    Ok(match tree.get_path(rel) {
        Ok(entry) => repo.find_blob(entry.id()).ok().map(|b| b.content().to_vec()),
        Err(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_name_and_label() {
        let s = Snapshot { time: 1000, hash: 1 };
        let doc = VersionDoc::new("/w/src/a.rs".into(), VersionSource::Snapshot(s), 1000 + 3 * 60_000);
        assert_eq!(doc.title(), "a.rs (History: 3 min ago)");
    }

    #[test]
    fn outside_the_workspace_is_an_error() {
        let s = Snapshot { time: 1, hash: 1 };
        assert!(read_snapshot(Path::new("/w"), Path::new("/x/a.rs"), s).is_err());
    }
}
