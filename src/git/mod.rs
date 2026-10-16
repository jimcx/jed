//! Git, view and stage only (`git2`). No drawing and no app state here: every function that
//! touches the repository is blocking (call from `spawn_blocking`) and opens a fresh
//! `Repository` (it is `Send`, not `Sync`), so nothing is shared between tasks.
//!
//! - [`Repo`]: where a repository is (discovery from the workspace root, which may be a
//!   subdirectory of the work tree)
//! - [`Status`] ([`Status::read`]): the branch and every changed path, staged / unstaged /
//!   untracked (`.gitignore`d files left out), plus the explorer's decorations
//! - [`diff`]: hunks between two texts, applying / reverting one
//! - [`stage`]: the texts to compare (index, HEAD, disk) and staging / unstaging a file or a hunk

pub mod diff;
pub mod stage;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use git2::{ErrorCode, Repository, Status as Flags, StatusOptions};

#[allow(unused_imports)] // for 10d–10e
pub use diff::{Diff, Hunk, HunkKind};

/// Where a repository with a work tree is. Plain data: open it per task with [`Repo::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The work tree's top (canonical, so it compares with the workspace root's paths).
    pub workdir: PathBuf,
    /// The `.git` directory (for a linked work tree, its own one under the main `.git`).
    pub git_dir: PathBuf,
}

impl Repo {
    /// Blocking: the repository containing `path` (itself or an ancestor). `None` when there is
    /// none, or only a bare one.
    pub fn discover(path: &Path) -> Option<Repo> {
        let repo = match Repository::discover(path) {
            Ok(r) => r,
            Err(e) if e.code() == ErrorCode::NotFound => return None,
            Err(e) => {
                tracing::warn!(path = %path.display(), "can't open the git repository: {e}");
                return None;
            }
        };
        let workdir = repo.workdir()?;
        Some(Repo {
            workdir: canonical(workdir),
            git_dir: canonical(repo.path()),
        })
    }

    /// Blocking: opens the repository.
    pub fn open(&self) -> Result<Repository, git2::Error> {
        Repository::open(&self.git_dir)
    }

    /// `path` relative to the work tree (`None` outside it).
    pub fn relative(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.workdir).ok().map(Path::to_path_buf)
    }

    /// Whether a change to `path` (from the file watcher) can change the status: anything in
    /// the work tree outside `.git`, and in `.git` the index, HEAD and refs — not objects, logs
    /// or lock files (a lock is renamed over the file it guards, which is seen).
    pub fn affects_status(&self, path: &Path) -> bool {
        if let Ok(rest) = path.strip_prefix(&self.git_dir) {
            let first = rest.components().next();
            let lock = path.extension().is_some_and(|e| e == "lock");
            return !lock && !matches!(first, Some(Component::Normal(c)) if c == "objects" || c == "logs");
        }
        path.starts_with(&self.workdir)
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What HEAD is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// No commits yet: the branch HEAD names.
    Unborn(String),
    /// Detached: the commit's short id (8 hex digits, as VS Code shows it).
    Detached(String),
}

impl Head {
    /// The status bar's text: the branch name or short commit id.
    pub fn name(&self) -> &str {
        match self {
            Head::Branch(s) | Head::Unborn(s) | Head::Detached(s) => s,
        }
    }
}

/// How a path differs on one side (index vs HEAD, or work tree vs index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    /// Work tree only: not in the index.
    Untracked,
    /// Work tree only: unmerged (the index has conflict stages).
    Conflicted,
}

/// One changed path. `staged`: index vs HEAD; `unstaged`: work tree vs index (untracked and
/// conflicted files are `unstaged` only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the work tree.
    pub path: PathBuf,
    pub staged: Option<Change>,
    pub unstaged: Option<Change>,
}

impl Entry {
    /// The explorer's colour for this file: the work tree's change wins over the index's (VS
    /// Code's git decorations).
    pub fn decoration(&self) -> Option<Decoration> {
        let d = |c| match c {
            Change::Added => Decoration::Added,
            Change::Modified | Change::TypeChanged => Decoration::Modified,
            Change::Deleted => Decoration::Deleted,
            Change::Renamed => Decoration::Renamed,
            Change::Untracked => Decoration::Untracked,
            Change::Conflicted => Decoration::Conflicted,
        };
        self.unstaged.or(self.staged).map(d)
    }
}

/// An explorer entry's git colour (VS Code's `gitDecoration.*`), in rising importance: a
/// folder takes its most important file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decoration {
    Untracked,
    Renamed,
    Added,
    Deleted,
    Modified,
    Conflicted,
}

impl Decoration {
    pub fn theme_key(self) -> &'static str {
        match self {
            Decoration::Untracked => "gitDecoration.untrackedResourceForeground",
            Decoration::Renamed => "gitDecoration.renamedResourceForeground",
            Decoration::Added => "gitDecoration.addedResourceForeground",
            Decoration::Deleted => "gitDecoration.deletedResourceForeground",
            Decoration::Modified => "gitDecoration.modifiedResourceForeground",
            Decoration::Conflicted => "gitDecoration.conflictingResourceForeground",
        }
    }

    /// The letter VS Code shows beside a file.
    pub fn letter(self) -> char {
        match self {
            Decoration::Untracked => 'U',
            Decoration::Renamed => 'R',
            Decoration::Added => 'A',
            Decoration::Deleted => 'D',
            Decoration::Modified => 'M',
            Decoration::Conflicted => '!',
        }
    }
}

/// A repository's state at one moment.
#[derive(Debug, Clone)]
pub struct Status {
    pub repo: Repo,
    pub head: Head,
    /// Every changed path, sorted by path.
    pub entries: Vec<Entry>,
    /// Absolute path → colour, for changed files and every folder above one (inside the work
    /// tree).
    decorations: HashMap<PathBuf, Decoration>,
}

impl Status {
    /// Blocking: HEAD and every changed path (untracked folders listed file by file, ignored
    /// files left out, no rename detection).
    pub fn read(repo: &Repo) -> Result<Status, git2::Error> {
        let r = repo.open()?;
        let head = head(&r)?;
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .renames_head_to_index(false)
            .renames_index_to_workdir(false);
        let statuses = r.statuses(Some(&mut opts))?;
        let mut entries = Vec::with_capacity(statuses.len());
        for s in statuses.iter() {
            let path = PathBuf::from(String::from_utf8_lossy(s.path_bytes()).into_owned());
            let f = s.status();
            let (staged, unstaged) = changes(f);
            if staged.is_none() && unstaged.is_none() {
                continue;
            }
            entries.push(Entry { path, staged, unstaged });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Status::new(repo.clone(), head, entries))
    }

    pub fn new(repo: Repo, head: Head, entries: Vec<Entry>) -> Status {
        let mut decorations = HashMap::new();
        for e in &entries {
            let Some(d) = e.decoration() else { continue };
            let file = repo.workdir.join(&e.path);
            decorations.insert(file.clone(), d);
            for dir in file.ancestors().skip(1) {
                if !dir.starts_with(&repo.workdir) || dir == repo.workdir {
                    break;
                }
                let slot = decorations.entry(dir.to_path_buf()).or_insert(d);
                *slot = (*slot).max(d);
            }
        }
        Status {
            repo,
            head,
            entries,
            decorations,
        }
    }

    /// The colour of an explorer entry (absolute path): a changed file's, or a folder holding
    /// changes.
    pub fn decoration(&self, path: &Path) -> Option<Decoration> {
        self.decorations.get(path).copied()
    }

    /// The entry for `path` (relative to the work tree), if it changed.
    #[cfg_attr(not(test), allow(dead_code))] // the Source Control panel (10e)
    pub fn entry(&self, path: &Path) -> Option<&Entry> {
        let i = self.entries.binary_search_by(|e| e.path.as_path().cmp(path)).ok()?;
        Some(&self.entries[i])
    }

    /// The status bar's text, as VS Code: the branch, `*` with unstaged changes (untracked files
    /// too), `+` with staged ones, `!` with conflicts.
    pub fn label(&self) -> String {
        let any = |f: fn(&Entry) -> bool| self.entries.iter().any(f);
        let mut s = self.head.name().to_string();
        if any(|e| e.unstaged.is_some_and(|c| c != Change::Conflicted)) {
            s.push('*');
        }
        if any(|e| e.staged.is_some()) {
            s.push('+');
        }
        if any(|e| e.unstaged == Some(Change::Conflicted)) {
            s.push('!');
        }
        s
    }
}

fn changes(f: Flags) -> (Option<Change>, Option<Change>) {
    if f.is_conflicted() {
        return (None, Some(Change::Conflicted));
    }
    let staged = if f.is_index_new() {
        Some(Change::Added)
    } else if f.is_index_modified() {
        Some(Change::Modified)
    } else if f.is_index_deleted() {
        Some(Change::Deleted)
    } else if f.is_index_renamed() {
        Some(Change::Renamed)
    } else if f.is_index_typechange() {
        Some(Change::TypeChanged)
    } else {
        None
    };
    let unstaged = if f.is_wt_new() {
        Some(Change::Untracked)
    } else if f.is_wt_modified() {
        Some(Change::Modified)
    } else if f.is_wt_deleted() {
        Some(Change::Deleted)
    } else if f.is_wt_renamed() {
        Some(Change::Renamed)
    } else if f.is_wt_typechange() {
        Some(Change::TypeChanged)
    } else {
        None
    };
    (staged, unstaged)
}

fn head(r: &Repository) -> Result<Head, git2::Error> {
    match r.head() {
        Ok(h) if r.head_detached()? => {
            let id = h.peel_to_commit()?.id().to_string();
            Ok(Head::Detached(id[..8].to_string()))
        }
        Ok(h) => Ok(Head::Branch(h.shorthand().unwrap_or("HEAD").to_string())),
        Err(e) if e.code() == ErrorCode::UnbornBranch || e.code() == ErrorCode::NotFound => {
            let head = r.find_reference("HEAD")?;
            let name = head.symbolic_target().ok().flatten().unwrap_or("HEAD");
            Ok(Head::Unborn(
                name.strip_prefix("refs/heads/").unwrap_or(name).to_string(),
            ))
        }
        Err(e) => Err(e),
    }
}

/// What a background refresh found.
#[derive(Debug, Clone)]
pub enum Refreshed {
    /// The workspace is in no repository: git features are off.
    NoRepo,
    /// Every changed path was ignored: the last status stands.
    Unchanged,
    Status(Box<Status>),
}

/// Blocking: the status of the repository holding `root`. `changed` (paths the file watcher
/// saw) skips the work when every one is `.gitignore`d or can't affect the status; `None`
/// reads it regardless (the first read).
pub fn refresh(root: &Path, changed: Option<&[PathBuf]>) -> Result<Refreshed, git2::Error> {
    let Some(repo) = Repo::discover(root) else {
        return Ok(Refreshed::NoRepo);
    };
    if let Some(changed) = changed
        && !needs_status(&repo, changed)?
    {
        return Ok(Refreshed::Unchanged);
    }
    Ok(Refreshed::Status(Box::new(Status::read(&repo)?)))
}

fn needs_status(repo: &Repo, changed: &[PathBuf]) -> Result<bool, git2::Error> {
    let r = repo.open()?;
    for p in changed {
        if !repo.affects_status(p) {
            continue;
        }
        let Some(rel) = repo.relative(p) else { return Ok(true) };
        if rel.starts_with(".git") || !r.is_path_ignored(&rel).unwrap_or(false) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
pub mod test_repo {
    //! Temp repositories driven by the `git` CLI, for this module's tests and milestone 10's.

    use std::path::Path;
    use std::process::Command;

    /// Runs `git args` in `dir` (a fixed identity, no user config), returning stdout; panics on
    /// failure.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        git_with_input(dir, args, None)
    }

    /// `git` with `input` on stdin (`git add -p`'s answers).
    pub fn git_with_input(dir: &Path, args: &[&str], input: Option<&str>) -> String {
        use std::io::Write;
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("git runs");
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// `git init -b main` in `dir`.
    pub fn init(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
    }

    /// Stages everything and commits.
    pub fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
    }
}

#[cfg(test)]
mod tests {
    use super::test_repo::{commit_all, git, init};
    use super::*;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical(dir.path());
        (dir, root)
    }

    fn status(root: &Path) -> Status {
        match refresh(root, None).unwrap() {
            Refreshed::Status(s) => *s,
            other => panic!("no status: {other:?}"),
        }
    }

    #[test]
    fn no_repository() {
        let (_d, root) = tmp();
        assert!(Repo::discover(&root).is_none());
        assert!(matches!(refresh(&root, None).unwrap(), Refreshed::NoRepo));
    }

    #[test]
    fn unborn_head_and_untracked_files() {
        let (_d, root) = tmp();
        init(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/deep/a.rs"), "a\n").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/x"), "x").unwrap();
        std::fs::write(root.join("debug.log"), "x").unwrap();
        let s = status(&root);
        assert_eq!(s.head, Head::Unborn("main".into()));
        let paths: Vec<_> = s.entries.iter().map(|e| e.path.to_str().unwrap()).collect();
        // Ignored files left out; the untracked folder listed by file.
        assert_eq!(paths, [".gitignore", "src/deep/a.rs"]);
        assert_eq!(s.entries[1].unstaged, Some(Change::Untracked));
        assert_eq!(s.decoration(&root.join("src")), Some(Decoration::Untracked));
        assert_eq!(s.decoration(&root.join("src/deep/a.rs")), Some(Decoration::Untracked));
        assert_eq!(s.decoration(&root.join("target")), None);
        assert_eq!(s.label(), "main*");
    }

    #[test]
    fn staged_unstaged_and_folder_colours() {
        let (_d, root) = tmp();
        init(&root);
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "b\n").unwrap();
        std::fs::write(root.join("gone.txt"), "g\n").unwrap();
        commit_all(&root, "one");
        std::fs::write(root.join("src/a.rs"), "a2\n").unwrap();
        std::fs::write(root.join("new.txt"), "n\n").unwrap();
        git(&root, &["add", "new.txt"]);
        std::fs::write(root.join("src/b.rs"), "b2\n").unwrap();
        git(&root, &["add", "src/b.rs"]);
        std::fs::write(root.join("src/b.rs"), "b3\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        let s = status(&root);
        assert_eq!(s.head, Head::Branch("main".into()));
        let e = |p: &str| s.entry(Path::new(p)).cloned().unwrap();
        assert_eq!(
            (e("src/a.rs").staged, e("src/a.rs").unstaged),
            (None, Some(Change::Modified))
        );
        assert_eq!(e("new.txt").staged, Some(Change::Added));
        assert_eq!(
            (e("src/b.rs").staged, e("src/b.rs").unstaged),
            (Some(Change::Modified), Some(Change::Modified))
        );
        assert_eq!(e("gone.txt").unstaged, Some(Change::Deleted));
        assert_eq!(s.decoration(&root.join("new.txt")), Some(Decoration::Added));
        assert_eq!(s.decoration(&root.join("src")), Some(Decoration::Modified));
        assert_eq!(s.decoration(&root.join("gone.txt")), Some(Decoration::Deleted));
        assert_eq!(s.decoration(&root), None);
        assert_eq!(s.label(), "main*+");
    }

    /// A file's work tree change wins over its staged one; a folder takes its most important
    /// file's colour, whatever order they come in.
    #[test]
    fn decoration_precedence() {
        let (_d, root) = tmp();
        init(&root);
        std::fs::create_dir_all(root.join("d/e")).unwrap();
        std::fs::write(root.join("d/z.rs"), "z\n").unwrap();
        std::fs::write(root.join("d/e/m.rs"), "m\n").unwrap();
        commit_all(&root, "one");
        // `d/a.txt` untracked and `d/b.txt` added sort before the modified `d/e/m.rs`; `d/z.rs`
        // (deleted) after it.
        std::fs::write(root.join("d/a.txt"), "a\n").unwrap();
        std::fs::write(root.join("d/b.txt"), "b\n").unwrap();
        git(&root, &["add", "d/b.txt"]);
        std::fs::write(root.join("d/b.txt"), "b2\n").unwrap();
        std::fs::write(root.join("d/e/m.rs"), "m2\n").unwrap();
        std::fs::remove_file(root.join("d/z.rs")).unwrap();
        let s = status(&root);
        let b = s.entry(Path::new("d/b.txt")).unwrap();
        assert_eq!((b.staged, b.unstaged), (Some(Change::Added), Some(Change::Modified)));
        assert_eq!(s.decoration(&root.join("d/b.txt")), Some(Decoration::Modified));
        assert_eq!(s.decoration(&root.join("d/a.txt")), Some(Decoration::Untracked));
        assert_eq!(s.decoration(&root.join("d/e")), Some(Decoration::Modified));
        assert_eq!(s.decoration(&root.join("d")), Some(Decoration::Modified));
        assert_eq!(s.decoration(&root.join("d/e/m.rs")), Some(Decoration::Modified));
        assert_eq!(s.decoration(&root.join("d/z.rs")), Some(Decoration::Deleted));
        // Only untracked below: untracked; added beats untracked.
        let only = |entries: Vec<Entry>| Status::new(s.repo.clone(), s.head.clone(), entries);
        let e = |p: &str, staged, unstaged| Entry {
            path: p.into(),
            staged,
            unstaged,
        };
        let t = only(vec![
            e("x/a", None, Some(Change::Untracked)),
            e("x/b", Some(Change::Added), None),
            e("x/c", None, Some(Change::Untracked)),
        ]);
        assert_eq!(t.decoration(&root.join("x")), Some(Decoration::Added));
        let t = only(vec![
            e("x/a", None, Some(Change::Modified)),
            e("x/b", None, Some(Change::Conflicted)),
        ]);
        assert_eq!(t.decoration(&root.join("x")), Some(Decoration::Conflicted));
        assert_eq!(t.label(), "main*!");
    }

    #[test]
    fn detached_head_shows_the_short_commit() {
        let (_d, root) = tmp();
        init(&root);
        std::fs::write(root.join("a"), "a\n").unwrap();
        commit_all(&root, "one");
        let id = git(&root, &["rev-parse", "HEAD"]);
        git(&root, &["checkout", "-q", "--detach"]);
        assert_eq!(status(&root).head, Head::Detached(id[..8].to_string()));
    }

    #[test]
    fn root_in_a_subdirectory() {
        let (_d, top) = tmp();
        init(&top);
        std::fs::create_dir(top.join("sub")).unwrap();
        std::fs::write(top.join("sub/a"), "a\n").unwrap();
        let s = status(&top.join("sub"));
        assert_eq!(s.repo.workdir, top);
        assert_eq!(s.decoration(&top.join("sub/a")), Some(Decoration::Untracked));
        assert_eq!(s.decoration(&top.join("sub")), Some(Decoration::Untracked));
    }

    #[test]
    fn ignored_changes_skip_the_status() {
        let (_d, root) = tmp();
        init(&root);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let repo = Repo::discover(&root).unwrap();
        let ignored = [root.join("target/debug/x"), root.join(".git/objects/ab/cd")];
        assert!(matches!(refresh(&root, Some(&ignored)).unwrap(), Refreshed::Unchanged));
        for p in [root.join("src/a"), root.join(".git/index"), root.join(".git/HEAD")] {
            assert!(repo.affects_status(&p), "{p:?}");
            assert!(matches!(refresh(&root, Some(&[p])).unwrap(), Refreshed::Status(_)));
        }
        assert!(!repo.affects_status(&root.join(".git/index.lock")));
        assert!(!repo.affects_status(&root.join(".git/logs/HEAD")));
    }
}
