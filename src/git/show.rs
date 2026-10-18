//! A commit as `git show --stat --format=medium` prints it: its header (id, merge parents,
//! author, author date in the commit's own offset), its message and the per-file stat against
//! its first parent (a root commit against the empty tree; a merge against its first parent,
//! which is what `git show --stat` prints). No drawing, no app state, no theme:
//! [`git_show::render`](crate::git_show::render) turns a [`Show`] into lines with kinds.
//!
//! Blocking (`Repo::open` per call). Renames come from libgit2's `find_similar` with git's
//! defaults (rename detection on, a 50% similarity threshold), file order is the diff's — git's
//! tree order. A binary file's sizes are its blobs' sizes; a binary whose two sides are the same
//! blob (a mode-only change) has none, which git prints as `Bin` alone. Binary here is git's
//! NUL-in-the-first-8000-bytes rule; `.gitattributes` binary attributes and diff drivers are not
//! consulted.
#![allow(dead_code)] // the commit tab reads these (git_history-c)

use std::path::Path;

use git2::{Delta, DiffFindOptions, Oid, Repository, Tree};

use super::Repo;

/// One commit, as the commit tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Show {
    /// The full object id (hex).
    pub id: String,
    /// The parents' unique abbreviations, first parent first (the `Merge:` line uses them when
    /// there is more than one).
    pub parents: Vec<String>,
    /// The author's name and email.
    pub author: String,
    pub email: String,
    /// The author date (what the header prints): Unix seconds and the offset in minutes.
    pub time: i64,
    pub offset: i32,
    /// The whole message, as stored.
    pub message: String,
    /// Every changed file, in the diff's order.
    pub files: Vec<File>,
}

/// One file's change in a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The path after the commit (a rename's new path).
    pub path: String,
    /// The path before it, for a rename.
    pub old_path: Option<String>,
    pub content: Content,
}

/// What changed in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Lines inserted and deleted.
    Text { added: usize, deleted: usize },
    /// A binary file: its old and new sizes, or `None` when both sides are the same blob (a
    /// mode-only change), which git prints as `Bin` without sizes.
    Binary(Option<(u64, u64)>),
}

/// Blocking: the commit `id` (a full hex object id) as `git show --stat` shows it. A merge's
/// stat is against its first parent; a root commit's against the empty tree.
pub fn read(repo: &Repo, id: &str) -> Result<Show, git2::Error> {
    let r = repo.open()?;
    let commit = r.find_commit(Oid::from_str(id)?)?;
    let tree = commit.tree()?;
    let parent = match commit.parent_count() {
        0 => None,
        _ => Some(commit.parent(0)?),
    };
    let old_tree = parent.as_ref().map(|p| p.tree()).transpose()?;
    let mut diff = r.diff_tree_to_tree(old_tree.as_ref(), Some(&tree), None)?;
    let mut find = DiffFindOptions::new();
    find.renames(true).rename_threshold(50);
    diff.find_similar(Some(&mut find))?;
    let mut files = Vec::with_capacity(diff.deltas().len());
    for (i, delta) in diff.deltas().enumerate() {
        let new_path = delta.new_file().path().or_else(|| delta.old_file().path());
        let old_path = delta.old_file().path();
        let renamed = matches!(delta.status(), Delta::Renamed | Delta::Copied);
        let old = blob(&r, old_tree.as_ref(), old_path);
        let new = blob(&r, Some(&tree), new_path);
        let binary = old.is_some_and(|b| b.2) || new.is_some_and(|b| b.2);
        let content = if binary {
            Content::Binary(match (old, new) {
                // The same blob on both sides (a mode-only change): no sizes.
                (Some((old_id, ..)), Some((new_id, ..))) if old_id == new_id => None,
                (old, new) => Some((old.map_or(0, |b| b.1), new.map_or(0, |b| b.1))),
            })
        } else {
            let (added, deleted) = match git2::Patch::from_diff(&diff, i)? {
                Some(patch) => {
                    let (_, added, deleted) = patch.line_stats()?;
                    (added, deleted)
                }
                None => (0, 0),
            };
            Content::Text { added, deleted }
        };
        files.push(File {
            path: new_path
                .or(old_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            old_path: renamed
                .then(|| old_path.map(|p| p.to_string_lossy().into_owned()))
                .flatten(),
            content,
        });
    }
    let mut parents = Vec::with_capacity(commit.parent_count());
    for p in commit.parents() {
        parents.push(p.as_object().short_id()?.as_str().unwrap_or_default().to_string());
    }
    let author = commit.author();
    Ok(Show {
        id: commit.id().to_string(),
        parents,
        author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
        email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
        time: author.when().seconds(),
        offset: author.when().offset_minutes(),
        message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
        files,
    })
}

/// A blob at `path` in `tree`: its id, size and whether git calls it binary (a NUL in its first
/// 8000 bytes, `buffer_is_binary`). `None` when the tree (or the file in it) is absent, or the
/// entry isn't a blob (a submodule).
fn blob(r: &Repository, tree: Option<&Tree>, path: Option<&Path>) -> Option<(Oid, u64, bool)> {
    let entry = tree?.get_path(path?).ok()?;
    let blob = r.find_blob(entry.id()).ok()?;
    let bytes = blob.content();
    Some((entry.id(), bytes.len() as u64, is_binary(bytes)))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::super::test_repo::{git, init};

    use super::*;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        init(&root);
        (dir, root)
    }

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    }

    fn commit(root: &Path, message: &str) -> String {
        git(root, &["add", "-A"]);
        git(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        git(root, &["rev-parse", "HEAD"]).trim().to_string()
    }

    #[test]
    fn a_root_commit_with_text_and_binary_files() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\nb\nc\n");
        write(&root, "b.txt", b"one\ntwo\n");
        write(&root, "bin.dat", b"\x00\x01\x02binary");
        let id = commit(&root, "first");
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert_eq!(show.id, id);
        assert!(show.parents.is_empty());
        assert_eq!((show.author.as_str(), show.email.as_str()), ("t", "t@example.com"));
        assert_eq!(show.message, "first\n");
        let files: Vec<_> = show
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.old_path.as_deref(), f.content))
            .collect();
        assert_eq!(
            files,
            [
                ("a.txt", None, Content::Text { added: 3, deleted: 0 }),
                ("b.txt", None, Content::Text { added: 2, deleted: 0 }),
                ("bin.dat", None, Content::Binary(Some((0, 9)))),
            ]
        );
    }

    #[test]
    fn a_modification_and_its_counts() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"1\n2\n3\n4\n");
        write(&root, "b.txt", b"keep\n");
        commit(&root, "first");
        write(&root, "a.txt", b"1\nchanged\n3\n");
        write(&root, "b.txt", b"keep\nmore\n");
        let id = commit(&root, "second");
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert_eq!(show.parents.len(), 1);
        let a = show.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(a.content, Content::Text { added: 1, deleted: 2 });
        let b = show.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!(b.content, Content::Text { added: 1, deleted: 0 });
    }

    #[test]
    fn renames_mode_changes_and_binaries() {
        let (_d, root) = tmp();
        write(&root, "src/a/old_name.txt", b"one\ntwo\n");
        write(&root, "src/mode.sh", b"hello\n");
        write(&root, "bin.dat", b"\x00\x01\x02\x03binarydata");
        commit(&root, "first");
        std::fs::rename(root.join("src/a/old_name.txt"), root.join("src/a/new_name.txt")).unwrap();
        std::fs::rename(root.join("src/mode.sh"), root.join("src/a/renamed_mode.sh")).unwrap();
        write(&root, "bin.dat", b"\x00\x01\x02\x04binarydata");
        let id = commit(&root, "renames");
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        let renamed = show.files.iter().find(|f| f.path == "src/a/new_name.txt").unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("src/a/old_name.txt"));
        assert_eq!(renamed.content, Content::Text { added: 0, deleted: 0 });
        let moved = show.files.iter().find(|f| f.path == "src/a/renamed_mode.sh").unwrap();
        assert_eq!(moved.old_path.as_deref(), Some("src/mode.sh"));
        assert_eq!(moved.content, Content::Text { added: 0, deleted: 0 });
        let binary = show.files.iter().find(|f| f.path == "bin.dat").unwrap();
        assert_eq!(binary.content, Content::Binary(Some((14, 14))));
        // Mode-only text change: no lines.
        git(&root, &["add", "-A"]);
        let path = root.join("src/a/new_name.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let id = commit(&root, "mode only");
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert_eq!(show.files.len(), 1);
        assert_eq!(show.files[0].content, Content::Text { added: 0, deleted: 0 });
    }

    #[test]
    fn a_binary_mode_only_change_has_no_sizes() {
        let (_d, root) = tmp();
        write(&root, "bin.dat", b"\x00\x01\x02\x03");
        commit(&root, "first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("bin.dat");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        let id = commit(&root, "binary mode");
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert_eq!(show.files.len(), 1);
        assert_eq!(show.files[0].content, Content::Binary(None));
    }

    #[test]
    fn a_merge_is_diffed_against_its_first_parent() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        write(&root, "f.txt", b"f\n");
        commit(&root, "root");
        git(&root, &["checkout", "-q", "-b", "side"]);
        write(&root, "a.txt", b"a\nside\n");
        commit(&root, "side");
        git(&root, &["checkout", "-q", "main"]);
        write(&root, "f.txt", b"f\nmain\n");
        commit(&root, "main");
        git(&root, &["merge", "-q", "--no-ff", "side", "-m", "merge side"]);
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert_eq!(show.parents.len(), 2);
        assert_eq!(show.message, "merge side\n");
        // The side branch's change shows; the first parent's doesn't.
        let paths: Vec<&str> = show.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt"]);
    }

    #[test]
    fn an_empty_commit_has_no_files() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        commit(&root, "root");
        git(&root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        assert!(show.files.is_empty());
        assert_eq!(show.message, "empty\n");
    }

    /// The author date is what the header shows (not the committer date's).
    #[test]
    fn the_header_date_is_the_authors() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        git(&root, &["add", "-A"]);
        let out = Command::new("git")
            .args(["commit", "-q", "-m", "dated"])
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", "2026-10-17T14:03:09+0530")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let id = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let show = read(&Repo::discover(&root).unwrap(), &id).unwrap();
        // The author date (2026-10-17T14:03:09+0530), not the committer's (2020).
        let author_time = git(&root, &["show", "-s", "--format=%at", &id]).trim().parse().unwrap();
        assert_eq!((show.time, show.offset), (author_time, 330));
    }

    #[test]
    fn a_missing_commit_is_an_error() {
        let (_d, root) = tmp();
        write(&root, "a.txt", b"a\n");
        commit(&root, "root");
        assert!(read(&Repo::discover(&root).unwrap(), &"0".repeat(40)).is_err());
        assert!(read(&Repo::discover(&root).unwrap(), "nonsense").is_err());
    }
}
