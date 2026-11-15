//! What the Code Review panel needs from the local clone: the branch and its remote
//! ([`local_info`]), the merge base of a merge request ([`merge_base`]) and the files changed
//! between it and the working tree ([`changes`], with `show`'s rename detection). jed never
//! fetches: an object that isn't in the clone is [`RangeError::Missing`].
//!
//! Blocking (`Repo::open` per call): call from `spawn_blocking`.

use std::collections::HashSet;

use git2::{Delta, DiffFindOptions, ObjectType, Oid, Repository};

use super::Repo;

/// The checked-out branch and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInfo {
    /// HEAD is detached (or unborn): there is no branch to look a merge request up by.
    pub detached: bool,
    /// The branch name on the remote: its upstream's when it has one, else the local name.
    pub head_branch: String,
    /// The remote the branch pushes to (its upstream's), else `origin`, else the first remote.
    pub remote: Option<String>,
    /// That remote's URL.
    pub remote_url: Option<String>,
}

/// How local HEAD relates to the merge request's head commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadRelation {
    #[default]
    Same,
    /// Local has commits the merge request lacks: push.
    Ahead,
    /// The merge request has commits local lacks: pull.
    Behind,
    Diverged,
    /// The merge request's head isn't in the clone (or the repository can't say).
    Unknown,
}

/// Blocking: how HEAD relates to `mr_head`.
pub fn head_relation(repo: &Repo, mr_head: &str) -> HeadRelation {
    let Ok(r) = repo.open() else {
        return HeadRelation::Unknown;
    };
    let (Ok(head), Ok(theirs)) = (
        r.head().and_then(|h| h.peel_to_commit()).map(|c| c.id()),
        Oid::from_str(mr_head),
    ) else {
        return HeadRelation::Unknown;
    };
    if head == theirs {
        return HeadRelation::Same;
    }
    match r.graph_ahead_behind(head, theirs) {
        Ok((0, _)) => HeadRelation::Behind,
        Ok((_, 0)) => HeadRelation::Ahead,
        Ok(_) => HeadRelation::Diverged,
        Err(_) => HeadRelation::Unknown,
    }
}

/// Why the range can't be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeError {
    /// A commit or tree isn't in the clone: `git fetch` brings it.
    Missing,
    Git(String),
}

impl From<git2::Error> for RangeError {
    fn from(e: git2::Error) -> Self {
        if e.code() == git2::ErrorCode::NotFound {
            RangeError::Missing
        } else {
            RangeError::Git(e.message().to_string())
        }
    }
}

impl std::fmt::Display for RangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeError::Missing => write!(f, "the merge request's commits are not in this clone: run `git fetch`"),
            RangeError::Git(m) => write!(f, "git: {m}"),
        }
    }
}

/// One file changed between the merge base and the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// A renamed or copied file's old path.
    pub old_path: Option<String>,
    /// `A`, `M`, `D`, `R`, `C` or `T` (git's letters).
    pub status: char,
    pub additions: usize,
    pub deletions: usize,
    /// A hash of the file's current content (the work tree's), `"deleted"` for a deleted file:
    /// the viewed mark holds while it stays the same.
    pub hash: String,
}

/// Blocking: the branch HEAD is on and its remote.
pub fn local_info(repo: &Repo) -> Result<LocalInfo, git2::Error> {
    let r = repo.open()?;
    let (name, detached) = match r.head() {
        Ok(h) if !r.head_detached()? && h.is_branch() => (h.shorthand().unwrap_or("").to_string(), false),
        _ => (String::new(), true),
    };
    let mut head_branch = name.clone();
    let mut remote = None;
    if !detached {
        let refname = format!("refs/heads/{name}");
        remote = r
            .branch_upstream_remote(&refname)
            .ok()
            .and_then(|b| b.as_str().ok().map(String::from));
        if let (Some(rem), Ok(up)) = (&remote, r.branch_upstream_name(&refname))
            && let Ok(up) = up.as_str()
            && let Some(short) = up.strip_prefix(&format!("refs/remotes/{rem}/"))
        {
            head_branch = short.to_string();
        }
    }
    if remote.is_none() {
        let names: Vec<String> = r.remotes()?.iter().flatten().flatten().map(String::from).collect();
        remote = names.iter().find(|n| *n == "origin").or(names.first()).cloned();
    }
    let remote_url = remote
        .as_deref()
        .and_then(|n| r.find_remote(n).ok())
        .and_then(|r| r.url().ok().map(String::from));
    Ok(LocalInfo {
        detached,
        head_branch,
        remote,
        remote_url,
    })
}

/// Blocking: the commit the review diff starts from: `forge_base` (the forge's merge base) when it
/// gives one, else the merge base of HEAD and the base branch (`base_sha`, else the remote's
/// `base_name`). The commit must exist locally.
pub fn merge_base(
    repo: &Repo,
    forge_base: Option<&str>,
    base_sha: &str,
    base_name: &str,
    remote: Option<&str>,
) -> Result<String, RangeError> {
    let r = repo.open()?;
    if let Some(sha) = forge_base.filter(|s| !s.is_empty()) {
        let id = Oid::from_str(sha).map_err(|_| RangeError::Git(format!("bad commit id {sha}")))?;
        return Ok(r.find_commit(id)?.id().to_string());
    }
    let head = r.head()?.peel_to_commit()?.id();
    let base = Oid::from_str(base_sha)
        .ok()
        .and_then(|id| r.find_commit(id).ok())
        .or_else(|| {
            let remotes = remote.into_iter().chain(["origin"]);
            remotes
                .filter_map(|rem| r.revparse_single(&format!("refs/remotes/{rem}/{base_name}")).ok())
                .find_map(|o| o.peel_to_commit().ok())
        })
        .ok_or(RangeError::Missing)?;
    Ok(r.merge_base(head, base.id())?.to_string())
}

/// `diff` with renames detected, as `FileChange`s for the deltas `keep(path, old_path)` accepts.
fn collect(
    r: &Repository,
    mut diff: git2::Diff,
    keep: &dyn Fn(&str, Option<&str>) -> bool,
) -> Result<Vec<FileChange>, RangeError> {
    let mut find = DiffFindOptions::new();
    find.renames(true).rename_threshold(50);
    diff.find_similar(Some(&mut find))?;
    let mut out = Vec::with_capacity(diff.deltas().len());
    for (i, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            Delta::Added | Delta::Untracked => 'A',
            Delta::Deleted => 'D',
            Delta::Renamed => 'R',
            Delta::Copied => 'C',
            Delta::Typechange => 'T',
            Delta::Modified => 'M',
            _ => continue,
        };
        let new = delta.new_file().path().or_else(|| delta.old_file().path());
        let Some(new) = new else { continue };
        let path = new.to_string_lossy().into_owned();
        let old_path = matches!(status, 'R' | 'C')
            .then(|| delta.old_file().path().map(|p| p.to_string_lossy().into_owned()))
            .flatten();
        if !keep(&path, old_path.as_deref()) {
            continue;
        }
        let (additions, deletions) = match git2::Patch::from_diff(&diff, i)? {
            Some(patch) => {
                let (_, a, d) = patch.line_stats()?;
                (a, d)
            }
            None => (0, 0),
        };
        out.push(FileChange {
            hash: content_hash(r, &path, status),
            path,
            old_path,
            status,
            additions,
            deletions,
        });
    }
    Ok(out)
}

/// The paths of `files`, new and old.
fn paths(files: &[FileChange]) -> HashSet<&str> {
    files
        .iter()
        .flat_map(|f| [Some(f.path.as_str()), f.old_path.as_deref()])
        .flatten()
        .collect()
}

/// Blocking: the merge request's files: those of the diff from the commit `base` to `head` (the
/// merge request's head commit when it is in the clone, else local HEAD), as they stand in the
/// working tree (index included, so local edits show with their counts; untracked files never do).
/// An MR file the work tree has no difference for (local HEAD behind the MR) is listed with the
/// MR's own change. Renames detected as in `show`; a rename's old path counts too.
pub fn changes(repo: &Repo, base: &str, head: Option<&str>) -> Result<Vec<FileChange>, RangeError> {
    let r = repo.open()?;
    let id = Oid::from_str(base).map_err(|e| RangeError::Git(e.message().into()))?;
    let tree = r.find_commit(id)?.tree()?;
    let commit = match head
        .and_then(|h| Oid::from_str(h).ok())
        .and_then(|id| r.find_commit(id).ok())
    {
        Some(c) => c,
        None => r.head()?.peel_to_commit()?,
    };
    let mr = collect(
        &r,
        r.diff_tree_to_tree(Some(&tree), Some(&commit.tree()?), None)?,
        &|_, _| true,
    )?;
    let wanted = paths(&mr);
    let work = r.diff_tree_to_workdir_with_index(Some(&tree), None)?;
    let mut out = collect(&r, work, &|p, o| {
        wanted.contains(p) || o.is_some_and(|o| wanted.contains(o))
    })?;
    let shown: HashSet<String> = paths(&out).into_iter().map(String::from).collect();
    let head_tree = commit.tree()?;
    for mut f in mr {
        if shown.contains(&f.path) || f.old_path.as_ref().is_some_and(|o| shown.contains(o)) {
            continue;
        }
        // Not in the work tree: the viewed mark follows the MR's own blob.
        if f.status != 'D' {
            f.hash = head_tree
                .get_path(std::path::Path::new(&f.path))
                .map(|e| e.id().to_string())
                .unwrap_or_default();
        }
        out.push(f);
    }
    Ok(out)
}

fn content_hash(r: &Repository, path: &str, status: char) -> String {
    if status == 'D' {
        return "deleted".into();
    }
    let Some(dir) = r.workdir() else { return String::new() };
    Oid::hash_file(ObjectType::Blob, dir.join(path))
        .map(|o| o.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::test_repo::{commit_all, git, init};
    use super::*;

    fn repo(dir: &std::path::Path) -> Repo {
        Repo::discover(dir).unwrap()
    }

    #[test]
    fn changes_since_the_merge_base_with_renames_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        init(p);
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
        std::fs::write(p.join("gone.txt"), "x\n").unwrap();
        commit_all(p, "base");
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::rename(p.join("a.txt"), p.join("b.txt")).unwrap();
        std::fs::remove_file(p.join("gone.txt")).unwrap();
        std::fs::write(p.join("new.txt"), "n\n").unwrap();
        commit_all(p, "work");
        std::fs::write(p.join("b.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
        let r = repo(p);
        let base = git(p, &["rev-parse", "main"]).trim().to_string();
        let found = merge_base(&r, None, &base, "main", None).unwrap();
        assert_eq!(found, base);
        let files = changes(&r, &found, None).unwrap();
        let by = |n: &str| files.iter().find(|f| f.path == n).unwrap();
        assert_eq!(by("b.txt").status, 'R');
        assert_eq!(by("b.txt").old_path.as_deref(), Some("a.txt"));
        assert_eq!(by("b.txt").additions, 1);
        assert_eq!(by("gone.txt").status, 'D');
        assert_eq!(by("gone.txt").hash, "deleted");
        assert_eq!(by("new.txt").status, 'A');
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn a_missing_commit_is_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        std::fs::write(dir.path().join("a"), "a\n").unwrap();
        commit_all(dir.path(), "c");
        let r = repo(dir.path());
        let bogus = "1234567890123456789012345678901234567890";
        assert_eq!(merge_base(&r, Some(bogus), "", "main", None), Err(RangeError::Missing));
        assert_eq!(merge_base(&r, None, bogus, "nope", None), Err(RangeError::Missing));
    }

    #[test]
    fn the_branch_follows_its_upstream() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        init(p);
        std::fs::write(p.join("a"), "a\n").unwrap();
        commit_all(p, "c");
        let url = "ssh://git@git.example.com:2200/alice/project.git";
        git(p, &["remote", "add", "origin", url]);
        git(p, &["checkout", "-q", "-b", "local-name"]);
        git(p, &["update-ref", "refs/remotes/origin/remote-name", "HEAD"]);
        git(p, &["config", "branch.local-name.remote", "origin"]);
        git(p, &["config", "branch.local-name.merge", "refs/heads/remote-name"]);
        let info = local_info(&repo(p)).unwrap();
        assert!(!info.detached);
        assert_eq!(info.head_branch, "remote-name");
        assert_eq!(info.remote.as_deref(), Some("origin"));
        assert_eq!(info.remote_url.as_deref(), Some(url));
        git(p, &["checkout", "-q", "--detach"]);
        assert!(local_info(&repo(p)).unwrap().detached);
    }
}
