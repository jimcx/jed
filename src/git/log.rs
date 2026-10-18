//! A file's commits from HEAD, as `git log -- <path>` lists them: newest first, with git's
//! default history simplification (no drawing, no app state; blocking: opens the repository with
//! `Repo::open`).
//!
//! The walk is git's (`revision.c`'s `try_to_simplify_commit` with `simplify_history` and
//! `prune`): a commit whose path entry at its tree — mode and blob id, absent being a state of
//! its own — is TREESAME to a parent's is not shown, and only that parent is walked (a merge:
//! the first TREESAME parent, later parents dropped); a commit TREESAME to no parent is shown and
//! every parent is walked. A root commit is shown iff its tree has the path (it is TREESAME to
//! the empty tree otherwise). The order is git's default: commits by committer date, newest
//! first, parents added to the queue when their child is taken (equal dates keep the order they
//! were added in), each commit once.
//!
//! A page is one blocking call: [`commits`] takes a skip and a limit (the app uses [`PAGE`]) and
//! stops the walk once it has one commit past the page, so a long history costs only the commits
//! before it.
#![allow(dead_code)] // the Git History section reads these (git_history-b)

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::Path;

use git2::{ErrorCode, Oid, Repository, Tree};

use super::Repo;

/// The commits one page holds (the app reads history this many at a time).
pub const PAGE: usize = 200;

/// One commit as the history list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The full object id (hex).
    pub id: String,
    /// Git's unique abbreviation (`git log --oneline`'s), at least 7 hex digits.
    pub short: String,
    /// The first line of the message.
    pub summary: String,
    /// The author's name.
    pub author: String,
    /// The committer's time, Unix seconds (the order's key).
    pub time: i64,
}

/// One page of a file's history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub commits: Vec<Commit>,
    /// The history goes on after this page.
    pub more: bool,
}

/// Blocking: the commits of HEAD that touched `path` (work-tree relative), newest first, from
/// `skip` on, at most `limit` of them, with git's default history simplification. No commits (an
/// unborn HEAD): an empty page.
pub fn commits(repo: &Repo, path: &Path, skip: usize, limit: usize) -> Result<Page, git2::Error> {
    let r = repo.open()?;
    let tip = match r.head() {
        Ok(head) => head.peel_to_commit()?.id(),
        Err(e) if e.code() == ErrorCode::UnbornBranch || e.code() == ErrorCode::NotFound => {
            return Ok(Page::default());
        }
        Err(e) => return Err(e),
    };
    Walk {
        repo: &r,
        path,
        entries: HashMap::new(),
    }
    .page(tip, skip, limit)
}

struct Walk<'a> {
    repo: &'a Repository,
    path: &'a Path,
    /// Each parsed commit's path entry, so a parent shared by several children reads its tree
    /// once.
    entries: HashMap<Oid, Option<(i32, Oid)>>,
}

impl Walk<'_> {
    fn page(&mut self, tip: Oid, skip: usize, limit: usize) -> Result<Page, git2::Error> {
        let want = skip.saturating_add(limit).saturating_add(1);
        // git's `revs->commits`: by date, newest first; an item added later goes after the
        // items of its date, so a parent is never taken before its child at equal dates.
        let mut heap: BinaryHeap<(i64, Reverse<u64>, Oid)> = BinaryHeap::new();
        let mut queued: HashSet<Oid> = HashSet::new();
        let mut seq = 0u64;
        let mut shown: Vec<Commit> = Vec::new();
        heap.push((self.repo.find_commit(tip)?.time().seconds(), Reverse(seq), tip));
        queued.insert(tip);
        seq += 1;
        while let Some((_, _, id)) = heap.pop() {
            let commit = self.repo.find_commit(id)?;
            let (walk, show) = self.simplify(&commit)?;
            for parent in walk {
                if queued.insert(parent) {
                    let time = self.repo.find_commit(parent)?.time().seconds();
                    heap.push((time, Reverse(seq), parent));
                    seq += 1;
                }
            }
            if show {
                shown.push(describe(&commit)?);
                if shown.len() >= want {
                    break;
                }
            }
        }
        let more = shown.len() > skip.saturating_add(limit);
        Ok(Page {
            commits: shown.into_iter().skip(skip).take(limit).collect(),
            more,
        })
    }

    /// Whether git shows this commit, and the parents the walk goes on through.
    fn simplify(&mut self, commit: &git2::Commit) -> Result<(Vec<Oid>, bool), git2::Error> {
        if commit.parent_count() == 0 {
            let tree = commit.tree()?;
            return Ok((Vec::new(), self.entry_of(&tree)?.is_some()));
        }
        let own = self.entry(commit.id())?;
        let mut treesame = None;
        let mut parents = Vec::with_capacity(commit.parent_count());
        for parent in commit.parents() {
            let id = parent.id();
            if treesame.is_none() && self.entry(id)? == own {
                treesame = Some(id);
            }
            parents.push(id);
        }
        match treesame {
            // TREESAME to a parent: hidden, and the walk follows that parent alone (git drops
            // the parents on either side of it too).
            Some(parent) => Ok((vec![parent], false)),
            // TREESAME to none: shown, every parent walked.
            None => Ok((parents, true)),
        }
    }

    fn entry(&mut self, id: Oid) -> Result<Option<(i32, Oid)>, git2::Error> {
        if let Some(entry) = self.entries.get(&id) {
            return Ok(*entry);
        }
        let tree = self.repo.find_commit(id)?.tree()?;
        let entry = self.entry_of(&tree)?;
        self.entries.insert(id, entry);
        Ok(entry)
    }

    /// The path's tree entry as (mode, blob id); `None` when the tree doesn't have it.
    fn entry_of(&self, tree: &Tree) -> Result<Option<(i32, Oid)>, git2::Error> {
        match tree.get_path(self.path) {
            Ok(entry) => Ok(Some((entry.filemode(), entry.id()))),
            Err(_) => Ok(None),
        }
    }
}

fn describe(commit: &git2::Commit) -> Result<Commit, git2::Error> {
    let message = String::from_utf8_lossy(commit.message_bytes()).into_owned();
    let short = commit.as_object().short_id()?;
    Ok(Commit {
        id: commit.id().to_string(),
        short: short.as_str().unwrap_or_default().to_string(),
        summary: message.lines().next().unwrap_or_default().to_string(),
        author: String::from_utf8_lossy(commit.author().name_bytes()).into_owned(),
        time: commit.time().seconds(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::super::test_repo::{git, init};
    use super::*;

    /// `git commit` with fixed author and committer dates (the order tests need distinct ones).
    fn commit_at(dir: &Path, date: &str, message: &str) {
        git(dir, &["add", "-A"]);
        let out = Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        init(&root);
        (dir, root)
    }

    /// Runs `git args` with fixed author and committer dates (a merge, a commit).
    fn git_at(dir: &Path, date: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn write(root: &Path, path: &str, text: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    /// The path's commits as git reports them.
    fn cli(root: &Path, path: &str) -> Vec<String> {
        git(root, &["log", "--format=%H", "--", path])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Every page of the path's history, as the app would read it.
    fn pages(repo: &Repo, path: &str, limit: usize) -> Vec<String> {
        let mut got = Vec::new();
        let mut skip = 0;
        loop {
            let page = commits(repo, Path::new(path), skip, limit).unwrap();
            let n = page.commits.len();
            got.extend(page.commits.into_iter().map(|c| c.id));
            if !page.more {
                break;
            }
            assert!(n > 0, "a page claiming more must hold commits");
            skip += n;
        }
        got
    }

    #[test]
    fn a_linears_files_commits_newest_first() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "one\n");
        write(&root, "b.txt", "b\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        write(&root, "a.txt", "one\ntwo\n");
        write(&root, "b.txt", "b2\n");
        commit_at(&root, "2026-01-02T00:00:00+0000", "second");
        write(&root, "a.txt", "one\ntwo\nthree\n");
        commit_at(&root, "2026-01-03T00:00:00+0000", "third");
        let repo = Repo::discover(&root).unwrap();
        let got = pages(&repo, "a.txt", PAGE);
        assert_eq!(got, cli(&root, "a.txt"));
        assert_eq!(got.len(), 3, "the newest first: {}", got[0]);
        assert_eq!(got[0], git(&root, &["rev-parse", "HEAD"]).trim());
        // b.txt: the first and second commits only.
        assert_eq!(pages(&repo, "b.txt", PAGE), cli(&root, "b.txt"));
        assert_eq!(pages(&repo, "b.txt", PAGE).len(), 2);
        // The model's fields.
        let page = commits(&repo, Path::new("a.txt"), 0, 1).unwrap();
        let c = &page.commits[0];
        assert_eq!(c.summary, "third");
        assert_eq!(c.author, "t");
        assert_eq!(
            c.time,
            git(&root, &["show", "-s", "--format=%ct", &c.id])
                .trim()
                .parse::<i64>()
                .unwrap()
        );
        assert_eq!(c.short, c.id[..c.short.len()]);
        assert_eq!(c.short, git(&root, &["rev-parse", "--short", &c.id]).trim());
        assert!(page.more);
    }

    #[test]
    fn a_path_not_in_head_has_no_history() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "a\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        let repo = Repo::discover(&root).unwrap();
        for path in ["missing.txt", "dir/missing.txt"] {
            assert_eq!(
                commits(&repo, Path::new(path), 0, PAGE).unwrap(),
                Page::default(),
                "{path}"
            );
            assert!(cli(&root, path).is_empty());
        }
        // No commits yet: nothing at all.
        let (_d2, empty) = tmp();
        let repo = Repo::discover(&empty).unwrap();
        assert_eq!(commits(&repo, Path::new("a.txt"), 0, PAGE).unwrap(), Page::default());
        // A path HEAD no longer has (deleted since): its whole history, the deletion included.
        let (_d3, gone) = tmp();
        write(&gone, "gone.txt", "a\n");
        commit_at(&gone, "2026-01-01T00:00:00+0000", "root");
        std::fs::remove_file(gone.join("gone.txt")).unwrap();
        commit_at(&gone, "2026-01-02T00:00:00+0000", "deletes");
        let repo = Repo::discover(&gone).unwrap();
        assert_eq!(pages(&repo, "gone.txt", PAGE), cli(&gone, "gone.txt"));
        assert_eq!(pages(&repo, "gone.txt", PAGE).len(), 2);
    }

    #[test]
    fn a_root_commit_is_shown_only_when_it_added_the_path() {
        let (_d, root) = tmp();
        write(&root, "old.txt", "a\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        write(&root, "new.txt", "n\n");
        commit_at(&root, "2026-01-02T00:00:00+0000", "adds new");
        let repo = Repo::discover(&root).unwrap();
        for path in ["old.txt", "new.txt"] {
            assert_eq!(pages(&repo, path, PAGE), cli(&root, path), "{path}");
        }
        assert_eq!(pages(&repo, "new.txt", PAGE).len(), 1, "not the root");
        assert_eq!(pages(&repo, "old.txt", PAGE).len(), 1, "only the root");
    }

    /// A mode-only change is not TREESAME (git compares the entry's mode as well as its blob).
    #[test]
    fn a_mode_only_change_is_shown() {
        let (_d, root) = tmp();
        write(&root, "a.sh", "echo hi\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("a.sh");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        commit_at(&root, "2026-01-02T00:00:00+0000", "mode only");
        let repo = Repo::discover(&root).unwrap();
        let shown = pages(&repo, "a.sh", PAGE);
        assert_eq!(shown, cli(&root, "a.sh"));
        assert_eq!(shown.len(), 2, "the chmod is a change");
    }

    /// Two branches and a merge: the merge is TREESAME to one parent for the paths the side
    /// branch changed (and for the other, to the main branch), and is hidden like git hides it.
    #[test]
    fn simplifies_a_merge_like_git() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "a\n");
        write(&root, "f.txt", "f\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", "a\nside\n");
        commit_at(&root, "2026-01-02T00:00:00+0000", "side changes a");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "f.txt", "f\nmain\n");
        commit_at(&root, "2026-01-03T00:00:00+0000", "main changes f");
        git_at(
            &root,
            "2026-01-04T00:00:00+0000",
            &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
        );
        let repo = Repo::discover(&root).unwrap();
        for path in ["a.txt", "f.txt"] {
            assert_eq!(pages(&repo, path, PAGE), cli(&root, path), "{path}");
        }
        // The merge is hidden from both: a.txt's history is the side commit and the root; f's
        // is the main commit and the root.
        assert_eq!(pages(&repo, "a.txt", PAGE).len(), 2);
        assert_eq!(pages(&repo, "f.txt", PAGE).len(), 2);
        // A later commit touching both paths shows, and the simplified walk behind it (the
        // hidden merge) still lists both branches' commits.
        write(&root, "a.txt", "a\nside\nmain\n");
        write(&root, "f.txt", "f\nmain\nmerged\n");
        commit_at(&root, "2026-01-05T00:00:00+0000", "later");
        let repo = Repo::discover(&root).unwrap();
        assert_eq!(pages(&repo, "a.txt", PAGE), cli(&root, "a.txt"));
        assert_eq!(
            pages(&repo, "a.txt", PAGE).len(),
            3,
            "the later commit, the side's, the root"
        );
        assert_eq!(pages(&repo, "f.txt", PAGE), cli(&root, "f.txt"));
    }

    /// A merge resolved by keeping the first parent's text is TREESAME to it: git follows that
    /// parent alone, so the side branch's commit — which did change the path — is dropped.
    #[test]
    fn a_treesame_merge_drops_the_other_branchs_commits() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "a\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", "side\n");
        commit_at(&root, "2026-01-02T00:00:00+0000", "side changes a");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "a.txt", "main\n");
        commit_at(&root, "2026-01-03T00:00:00+0000", "main changes a");
        // `ours`: the merge's tree is the first parent's, so it is TREESAME to it.
        git_at(
            &root,
            "2026-01-04T00:00:00+0000",
            &["merge", "-q", "--no-ff", "-s", "ours", "side", "-m", "merge side"],
        );
        let repo = Repo::discover(&root).unwrap();
        let shown = pages(&repo, "a.txt", PAGE);
        assert_eq!(shown, cli(&root, "a.txt"));
        assert_eq!(shown.len(), 2, "the merge and the side branch are hidden");
        assert!(!shown.contains(&git(&root, &["rev-parse", "side"]).trim().to_string()));
    }

    /// A merge that changed the path relative to every parent (a conflict resolved some other
    /// way) is shown, and every parent's history is walked.
    #[test]
    fn a_merge_treesame_to_no_parent_is_shown() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "a\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", "side\n");
        commit_at(&root, "2026-01-02T00:00:00+0000", "side changes a");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "a.txt", "main\n");
        commit_at(&root, "2026-01-03T00:00:00+0000", "main changes a");
        // A merge whose tree is neither parent's: `-s ours` then amend with a new text keeps
        // both parents.
        git_at(
            &root,
            "2026-01-04T00:00:00+0000",
            &["merge", "-q", "--no-ff", "-s", "ours", "side", "-m", "merge side"],
        );
        write(&root, "a.txt", "resolved\n");
        git(&root, &["add", "-A"]);
        git_at(
            &root,
            "2026-01-04T00:00:00+0000",
            &["commit", "-q", "--amend", "--no-edit"],
        );
        let repo = Repo::discover(&root).unwrap();
        let shown = pages(&repo, "a.txt", PAGE);
        assert_eq!(shown, cli(&root, "a.txt"));
        assert_eq!(shown.len(), 4, "the merge, both branches, the root");
        assert_eq!(shown[0], git(&root, &["rev-parse", "HEAD"]).trim());
    }

    /// No `--follow`: a renamed path's history starts at the rename.
    #[test]
    fn a_renamed_path_does_not_follow_the_rename() {
        let (_d, root) = tmp();
        write(&root, "a.txt", "a\n");
        commit_at(&root, "2026-01-01T00:00:00+0000", "root");
        git(&root, &["mv", "a.txt", "b.txt"]);
        commit_at(&root, "2026-01-02T00:00:00+0000", "renames a");
        let repo = Repo::discover(&root).unwrap();
        let shown = pages(&repo, "b.txt", PAGE);
        assert_eq!(shown, cli(&root, "b.txt"));
        assert_eq!(shown.len(), 1, "only the rename, not the root");
        assert_eq!(pages(&repo, "a.txt", PAGE), cli(&root, "a.txt"));
    }

    /// The same dates on every commit: the queue's order is git's (each commit in the order it
    /// was added, and a parent never before its child).
    #[test]
    fn equal_dates_keep_gits_order() {
        let (_d, root) = tmp();
        let date = "2026-01-01T00:00:00+0000";
        write(&root, "a.txt", "a\n");
        write(&root, "f.txt", "f\n");
        commit_at(&root, date, "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", "a\nside\n");
        commit_at(&root, date, "side");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "f.txt", "f\nmain\n");
        commit_at(&root, date, "main");
        git_at(&root, date, &["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        let repo = Repo::discover(&root).unwrap();
        for path in ["a.txt", "f.txt"] {
            assert_eq!(pages(&repo, path, PAGE), cli(&root, path), "{path}");
        }
    }

    /// Paging is the same list split: no duplicates, the same order, `more` only while commits
    /// follow.
    #[test]
    fn pages_split_the_history_without_duplicates() {
        let (_d, root) = tmp();
        for i in 0..7 {
            write(&root, "a.txt", &format!("{i}\n"));
            commit_at(
                &root,
                &format!("2026-01-0{}T00:00:00+0000", i + 1),
                &format!("commit {i}"),
            );
        }
        let repo = Repo::discover(&root).unwrap();
        let all = cli(&root, "a.txt");
        assert_eq!(all.len(), 7);
        assert_eq!(pages(&repo, "a.txt", 3), all);
        assert_eq!(commits(&repo, Path::new("a.txt"), 0, 3).unwrap().commits.len(), 3);
        let second = commits(&repo, Path::new("a.txt"), 3, 3).unwrap();
        assert_eq!(second.commits.len(), 3);
        assert!(second.more);
        let last = commits(&repo, Path::new("a.txt"), 6, 3).unwrap();
        assert_eq!(last.commits.len(), 1);
        assert!(!last.more);
        let past = commits(&repo, Path::new("a.txt"), 20, 3).unwrap();
        assert_eq!(past, Page::default());
        assert!(!past.more);
    }
}
