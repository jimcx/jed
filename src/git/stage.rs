//! The texts git compares (HEAD, the index, the file on disk), their diffs, and staging /
//! unstaging a whole file or one hunk. Everything here is blocking (`spawn_blocking`) and opens
//! its own `Repository`. Paths are relative to the work tree.
//!
//! A hunk is staged by writing a new index blob: the index text with that hunk of the
//! index → disk diff applied (unstaging: the index text with that hunk of the HEAD → index
//! diff reverted), keeping the entry's mode. With [`GIT_CONTEXT`](super::diff::GIT_CONTEXT)
//! hunks that is what `git add -p` / `git reset -p` write for the same hunk. Texts are compared
//! as stored (no clean filters such as `core.autocrlf`).

#![allow(dead_code)] // 10d–10e (diff view, Source Control) call the rest

use std::fmt;
use std::path::Path;

use git2::{IndexEntry, IndexTime, Oid, Repository};

use super::Repo;
use super::diff::{self, Diff, Hunk};

#[derive(Debug)]
pub enum Error {
    Git(git2::Error),
    Io(std::io::Error),
    /// The hunk isn't in the diff any more: the file or the index changed since it was shown.
    Stale,
    /// The file is binary: only whole-file staging.
    Binary,
    /// The file has merge conflicts: resolve them (stage the whole file) first.
    Conflicted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Git(e) => write!(f, "{}", e.message()),
            Error::Io(e) => write!(f, "{e}"),
            Error::Stale => write!(f, "the file changed since the diff was shown"),
            Error::Binary => write!(f, "binary files can only be staged whole"),
            Error::Conflicted => write!(f, "the file has merge conflicts"),
        }
    }
}

impl std::error::Error for Error {}

impl From<git2::Error> for Error {
    fn from(e: git2::Error) -> Self {
        Error::Git(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Which pair of texts to compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The file on disk against the index (what staging adds).
    Unstaged,
    /// The index against HEAD (what unstaging takes back).
    Staged,
}

/// A text and its git file mode (`0o100644`, `0o100755`, `0o120000` for a symlink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

/// Two texts of one file (a missing side is empty) and the hunks between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    pub diff: Diff,
}

/// Blocking: the file's text in the index (`None` when not in it, or only conflict stages).
pub fn index_text(repo: &Repo, path: &Path) -> Result<Option<Text>, Error> {
    let r = repo.open()?;
    in_index(&r, path)
}

/// Blocking: the file's text in HEAD's tree (`None` when not in it, or no commits yet).
pub fn head_text(repo: &Repo, path: &Path) -> Result<Option<Text>, Error> {
    let r = repo.open()?;
    in_head(&r, path)
}

/// Blocking: the file on disk (`None` when missing; a symlink's target).
pub fn disk_text(repo: &Repo, path: &Path) -> Result<Option<Text>, Error> {
    let full = repo.workdir.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full)?;
        return Ok(Some(Text {
            bytes: target.as_os_str().as_encoded_bytes().to_vec(),
            mode: 0o120000,
        }));
    }
    if meta.is_dir() {
        return Ok(None);
    }
    Ok(Some(Text {
        bytes: std::fs::read(&full)?,
        mode: if executable(&meta) { 0o100755 } else { 0o100644 },
    }))
}

#[cfg(unix)]
fn executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

/// Blocking: one file's diff, `Unstaged` (index → disk) or `Staged` (HEAD → index), with
/// `context` lines around each change (0 for the gutter, `GIT_CONTEXT` for `git add -p`'s hunks).
pub fn file_diff(repo: &Repo, path: &Path, side: Side, context: u32) -> Result<FileDiff, Error> {
    let r = repo.open()?;
    let (old, new) = match side {
        Side::Unstaged => (in_index(&r, path)?, disk_text(repo, path)?),
        Side::Staged => (in_head(&r, path)?, in_index(&r, path)?),
    };
    let old = old.map(|t| t.bytes).unwrap_or_default();
    let new = new.map(|t| t.bytes).unwrap_or_default();
    let diff = diff::diff(&old, &new, context)?;
    Ok(FileDiff { old, new, diff })
}

/// Blocking: stages the whole file (`git add`, or `git rm --cached` when it's gone from disk).
pub fn stage_file(repo: &Repo, path: &Path) -> Result<(), Error> {
    let r = repo.open()?;
    let mut index = r.index()?;
    if disk_text(repo, path)?.is_some() {
        index.add_path(path)?;
    } else {
        index.remove_path(path)?;
    }
    index.write()?;
    tracing::info!(path = %path.display(), "staged file");
    Ok(())
}

/// Blocking: unstages the whole file (`git reset -- path`: the index entry back to HEAD's, or
/// removed with no commits yet / not in HEAD).
pub fn unstage_file(repo: &Repo, path: &Path) -> Result<(), Error> {
    let r = repo.open()?;
    let head = match r.head() {
        Ok(h) => Some(h.peel_to_commit()?),
        Err(_) => None,
    };
    r.reset_default(head.as_ref().map(|c| c.as_object()), [path])?;
    tracing::info!(path = %path.display(), "unstaged file");
    Ok(())
}

/// Blocking: stages hunk `k` of `shown` (an `Unstaged` [`file_diff`] — any context; with
/// `GIT_CONTEXT` the hunks are `git add -p`'s), checking first that the index and the file on
/// disk still hold `shown`'s texts (else `Stale`: diff again). A file gone from disk whose
/// deletion is staged leaves the index.
pub fn stage_hunk(repo: &Repo, path: &Path, shown: &FileDiff, k: usize) -> Result<(), Error> {
    let r = repo.open()?;
    let mut index = r.index()?;
    if conflicted(&index, path) {
        return Err(Error::Conflicted);
    }
    let entry = in_index(&r, path)?;
    let disk = disk_text(repo, path)?;
    let hunk = check(shown, k, &entry, &disk)?;
    let text = diff::apply(&shown.old, &shown.new, hunk);
    if disk.is_none() && text.is_empty() {
        index.remove_path(path)?;
    } else {
        let mode = entry.or(disk).map_or(0o100644, |t| t.mode);
        index.add_frombuffer(&new_entry(path, mode), &text)?;
    }
    index.write()?;
    tracing::info!(path = %path.display(), ?hunk, "staged hunk");
    Ok(())
}

/// Blocking: unstages hunk `k` of `shown` (a `Staged` [`file_diff`]), as `stage_hunk`. A file
/// not in HEAD whose whole addition is unstaged leaves the index (untracked again).
pub fn unstage_hunk(repo: &Repo, path: &Path, shown: &FileDiff, k: usize) -> Result<(), Error> {
    let r = repo.open()?;
    let mut index = r.index()?;
    if conflicted(&index, path) {
        return Err(Error::Conflicted);
    }
    let head = in_head(&r, path)?;
    let entry = in_index(&r, path)?;
    let hunk = check(shown, k, &head, &entry)?;
    let text = diff::revert(&shown.old, &shown.new, hunk);
    if head.is_none() && text.is_empty() {
        index.remove_path(path)?;
    } else {
        let mode = entry.or(head).map_or(0o100644, |t| t.mode);
        index.add_frombuffer(&new_entry(path, mode), &text)?;
    }
    index.write()?;
    tracing::info!(path = %path.display(), ?hunk, "unstaged hunk");
    Ok(())
}

/// What [`index_blob`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBlob {
    /// Not in the index (untracked, or only conflict stages).
    Missing,
    /// The blob is still the one asked about.
    Same,
    Text(Oid, Text),
}

/// Blocking: the file's index blob, unless it is still `known` (then not read).
pub fn index_blob(repo: &Repo, path: &Path, known: Option<Oid>) -> Result<IndexBlob, Error> {
    let r = repo.open()?;
    let index = r.index()?;
    let Some(e) = index.get_path(path, 0) else {
        return Ok(IndexBlob::Missing);
    };
    if Some(e.id) == known {
        return Ok(IndexBlob::Same);
    }
    let blob = r.find_blob(e.id)?;
    Ok(IndexBlob::Text(
        e.id,
        Text {
            bytes: blob.content().to_vec(),
            mode: e.mode,
        },
    ))
}

/// Blocking: stages the change the gutter shows on `line` (0-based) of the file on disk: the
/// context-0 hunk of index → disk there (`diff::hunk_at_line`), so exactly that change, as
/// VS Code's "Stage Selected Ranges"; the same index as `git add -p` answering yes to that hunk
/// when its `GIT_CONTEXT` hunk holds no other change, or after `s`plitting one that does. The
/// diff is taken again (once) if the file or index moved on in between. `None`: no change there.
pub fn stage_line(repo: &Repo, path: &Path, line: usize) -> Result<Option<Hunk>, Error> {
    stage_line_with(repo, path, line, || {})
}

/// [`stage_line`], running `between` after each diff, before staging (tests change the file
/// there).
fn stage_line_with(repo: &Repo, path: &Path, line: usize, mut between: impl FnMut()) -> Result<Option<Hunk>, Error> {
    let mut stale = false;
    loop {
        let shown = file_diff(repo, path, Side::Unstaged, 0)?;
        if shown.diff.binary {
            return Err(Error::Binary);
        }
        let Some(k) = diff::hunk_at_line(&shown.diff.hunks, line) else {
            return Ok(None);
        };
        between();
        match stage_hunk(repo, path, &shown, k) {
            Err(Error::Stale) if !stale => stale = true,
            r => return r.map(|()| Some(shown.diff.hunks[k].clone())),
        }
    }
}

/// Blocking: unstages the staged change (a context-0 hunk of HEAD → index) at `line` of
/// `worktree` (the buffer's text, whose lines the cursor is on): the line goes to the index
/// text's lines through the index → `worktree` diff (`diff::old_lines`), and the first staged
/// hunk touching those is unstaged, as `git reset -p` answering yes to it. `None`: no staged
/// change there.
pub fn unstage_line(repo: &Repo, path: &Path, worktree: &[u8], line: usize) -> Result<Option<Hunk>, Error> {
    let mut stale = false;
    loop {
        let index = index_text(repo, path)?.map(|t| t.bytes).unwrap_or_default();
        let unstaged = diff::diff(&index, worktree, 0)?;
        let lines = diff::old_lines(&unstaged.hunks, line);
        let shown = file_diff(repo, path, Side::Staged, 0)?;
        if shown.diff.binary {
            return Err(Error::Binary);
        }
        let Some(k) = shown.diff.hunks.iter().position(|h| diff::touches(&h.new, &lines)) else {
            return Ok(None);
        };
        match unstage_hunk(repo, path, &shown, k) {
            Err(Error::Stale) if !stale => stale = true,
            r => return r.map(|()| Some(shown.diff.hunks[k].clone())),
        }
    }
}

/// `shown`'s hunk `k`, if `old` and `new` are still its texts.
fn check<'a>(shown: &'a FileDiff, k: usize, old: &Option<Text>, new: &Option<Text>) -> Result<&'a Hunk, Error> {
    if shown.diff.binary {
        return Err(Error::Binary);
    }
    let same = |t: &Option<Text>, bytes: &[u8]| t.as_ref().map_or(&[][..], |t| &t.bytes) == bytes;
    match shown.diff.hunks.get(k) {
        Some(h) if same(old, &shown.old) && same(new, &shown.new) => Ok(h),
        _ => Err(Error::Stale),
    }
}

fn conflicted(index: &git2::Index, path: &Path) -> bool {
    (1..=3).any(|stage| index.get_path(path, stage).is_some())
}

fn in_index(r: &Repository, path: &Path) -> Result<Option<Text>, Error> {
    let index = r.index()?;
    let Some(e) = index.get_path(path, 0) else {
        return Ok(None);
    };
    let blob = r.find_blob(e.id)?;
    Ok(Some(Text {
        bytes: blob.content().to_vec(),
        mode: e.mode,
    }))
}

fn in_head(r: &Repository, path: &Path) -> Result<Option<Text>, Error> {
    let tree = match r.head() {
        Ok(h) => h.peel_to_tree()?,
        Err(_) => return Ok(None),
    };
    let entry = match tree.get_path(path) {
        Ok(e) => e,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Ok(blob) = r.find_blob(entry.id()) else {
        return Ok(None);
    };
    Ok(Some(Text {
        bytes: blob.content().to_vec(),
        mode: entry.filemode() as u32,
    }))
}

/// An index entry for new content: no stat data, so git re-reads the file on disk to compare
/// (as `git apply --cached` leaves it); `add_frombuffer` fills in the id and size.
fn new_entry(path: &Path, mode: u32) -> IndexEntry {
    let path = path.to_string_lossy().replace('\\', "/").into_bytes();
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: Oid::ZERO_SHA1,
        flags: 0,
        flags_extended: 0,
        path,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::diff::GIT_CONTEXT;
    use super::super::test_repo::{commit_all, git, git_with_input, init};
    use super::*;

    /// A repository with `files` committed (empty `files`: no commit).
    fn repo(files: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("a");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        for (name, text) in files {
            std::fs::write(root.join(name), text).unwrap();
        }
        if !files.is_empty() {
            commit_all(&root, "one");
        }
        (dir, root)
    }

    /// A copy of the repository at `root`, beside it.
    fn copy(root: &Path) -> PathBuf {
        let to = root.with_file_name("b");
        let ok = std::process::Command::new("cp")
            .arg("-a")
            .arg(root)
            .arg(&to)
            .status()
            .unwrap();
        assert!(ok.success());
        to
    }

    fn r(root: &Path) -> Repo {
        Repo::discover(root).unwrap()
    }

    /// What the index holds: `git ls-files -s` (mode, blob, stage, path) and the cached diff.
    fn index_state(root: &Path) -> (String, String) {
        (
            git(root, &["ls-files", "-s"]),
            git(root, &["diff", "--cached", "--no-color"]),
        )
    }

    /// Stages hunk `k` of `path` with red in a copy of the repo and with `git add -p` (answers
    /// `answers`) in the original; the indexes must match.
    fn same_as_add_p(root: &Path, path: &str, k: usize, answers: &str) {
        let other = copy(root);
        let repo = r(&other);
        let d = file_diff(&repo, Path::new(path), Side::Unstaged, GIT_CONTEXT).unwrap();
        stage_hunk(&repo, Path::new(path), &d, k).unwrap();
        git_with_input(root, &["add", "-p", "--", path], Some(answers));
        assert_eq!(index_state(&other), index_state(root));
        // Status agrees too (the zeroed stat data makes git re-read the file).
        assert_eq!(
            git(&other, &["status", "--porcelain"]),
            git(root, &["status", "--porcelain"])
        );
    }

    fn numbered(n: usize) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn stage_middle_hunk_like_add_p() {
        let text = numbered(40);
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let changed = text
            .replace("line 2\n", "two\n")
            .replace("line 20\n", "twenty\nmore\n")
            .replace("line 35\n", "");
        std::fs::write(root.join("f.txt"), changed).unwrap();
        same_as_add_p(&root, "f.txt", 1, "n\ny\nn\n");
    }

    #[test]
    fn stage_hunk_without_final_newline_like_add_p() {
        let text = format!("{}last", numbered(12));
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let changed = format!("{}last line\nadded", numbered(12).replace("line 0\n", "zero\n"));
        std::fs::write(root.join("f.txt"), changed).unwrap();
        same_as_add_p(&root, "f.txt", 1, "n\ny\n");
    }

    #[test]
    fn stage_hunk_adding_a_final_newline_like_add_p() {
        let text = format!("{}last", numbered(3));
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        std::fs::write(root.join("f.txt"), format!("{}last\n", numbered(3))).unwrap();
        same_as_add_p(&root, "f.txt", 0, "y\n");
    }

    #[test]
    fn stage_crlf_hunk_like_add_p() {
        let text = numbered(30).replace('\n', "\r\n");
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let changed = text.replace("line 3\r\n", "three\r\n").replace("line 25\r\n", "x\r\n");
        std::fs::write(root.join("f.txt"), changed).unwrap();
        same_as_add_p(&root, "f.txt", 0, "y\nn\n");
    }

    #[cfg(unix)]
    #[test]
    fn stage_hunk_keeps_the_executable_mode() {
        use std::os::unix::fs::PermissionsExt;
        let text = numbered(20);
        let (_d, root) = repo(&[("run.sh", text.as_bytes())]);
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        commit_all(&root, "exec");
        std::fs::write(
            root.join("run.sh"),
            text.replace("line 1\n", "one\n").replace("line 18\n", "x\n"),
        )
        .unwrap();
        same_as_add_p(&root, "run.sh", 0, "y\nn\n");
        assert!(git(&root, &["ls-files", "-s"]).starts_with("100755"));
    }

    /// A mode change on disk is its own question in `git add -p`; staging a content hunk keeps
    /// the index's mode.
    #[cfg(unix)]
    #[test]
    fn stage_hunk_keeps_the_index_mode_when_the_disk_mode_changed() {
        use std::os::unix::fs::PermissionsExt;
        let text = numbered(20);
        let (_d, root) = repo(&[("run.sh", text.as_bytes())]);
        std::fs::write(root.join("run.sh"), text.replace("line 1\n", "one\n")).unwrap();
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Mode change: no; the hunk: yes.
        same_as_add_p(&root, "run.sh", 0, "n\ny\n");
        assert!(git(&root, &["ls-files", "-s"]).starts_with("100644"));
    }

    /// The same for `git reset -p`: a staged mode change stays when a content hunk is unstaged.
    #[cfg(unix)]
    #[test]
    fn unstage_hunk_keeps_the_index_mode() {
        use std::os::unix::fs::PermissionsExt;
        let text = numbered(20);
        let (_d, root) = repo(&[("run.sh", text.as_bytes())]);
        std::fs::write(root.join("run.sh"), text.replace("line 1\n", "one\n")).unwrap();
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&root, &["add", "run.sh"]);
        let other = copy(&root);
        let repo = r(&other);
        let d = file_diff(&repo, Path::new("run.sh"), Side::Staged, GIT_CONTEXT).unwrap();
        unstage_hunk(&repo, Path::new("run.sh"), &d, 0).unwrap();
        git_with_input(&root, &["reset", "-q", "-p", "--", "run.sh"], Some("n\ny\n"));
        assert_eq!(index_state(&other), index_state(&root));
        assert!(git(&root, &["ls-files", "-s"]).starts_with("100755"));
    }

    #[test]
    fn stage_hunk_of_an_intent_to_add_file_like_add_p() {
        let (_d, root) = repo(&[("other", b"o\n")]);
        std::fs::write(root.join("new.txt"), numbered(5)).unwrap();
        git(&root, &["add", "-N", "new.txt"]);
        same_as_add_p(&root, "new.txt", 0, "y\n");
    }

    #[test]
    fn stage_deleted_file_like_add_p() {
        let (_d, root) = repo(&[("gone.txt", numbered(5).as_bytes()), ("keep", b"k\n")]);
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        same_as_add_p(&root, "gone.txt", 0, "y\n");
        assert!(!git(&root, &["ls-files"]).contains("gone.txt"));
    }

    #[test]
    fn stage_hunk_of_an_untracked_file_adds_it() {
        let (_d, root) = repo(&[]);
        std::fs::write(root.join("new.txt"), "a\nb\n").unwrap();
        let repo = r(&root);
        let d = file_diff(&repo, Path::new("new.txt"), Side::Unstaged, GIT_CONTEXT).unwrap();
        assert_eq!(d.diff.hunks, vec![Hunk { old: 0..0, new: 0..2 }]);
        stage_hunk(&repo, Path::new("new.txt"), &d, 0).unwrap();
        assert_eq!(
            index_text(&repo, Path::new("new.txt")).unwrap(),
            Some(Text {
                bytes: b"a\nb\n".to_vec(),
                mode: 0o100644
            })
        );
        assert_eq!(git(&root, &["status", "--porcelain"]), "A  new.txt\n");
    }

    #[test]
    fn unstage_hunk_like_reset_p() {
        let text = numbered(40);
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let changed = text.replace("line 2\n", "two\n").replace("line 30\n", "thirty\n");
        std::fs::write(root.join("f.txt"), changed).unwrap();
        git(&root, &["add", "f.txt"]);
        let other = copy(&root);
        let repo = r(&other);
        let d = file_diff(&repo, Path::new("f.txt"), Side::Staged, GIT_CONTEXT).unwrap();
        assert_eq!(d.diff.hunks.len(), 2);
        unstage_hunk(&repo, Path::new("f.txt"), &d, 0).unwrap();
        git_with_input(&root, &["reset", "-q", "-p", "--", "f.txt"], Some("y\nn\n"));
        assert_eq!(index_state(&other), index_state(&root));
    }

    #[test]
    fn unstage_whole_addition_makes_it_untracked() {
        let (_d, root) = repo(&[]);
        std::fs::write(root.join("new.txt"), "a\n").unwrap();
        git(&root, &["add", "new.txt"]);
        let repo = r(&root);
        let d = file_diff(&repo, Path::new("new.txt"), Side::Staged, GIT_CONTEXT).unwrap();
        unstage_hunk(&repo, Path::new("new.txt"), &d, 0).unwrap();
        assert_eq!(git(&root, &["status", "--porcelain"]), "?? new.txt\n");
    }

    #[test]
    fn unstage_hunk_of_a_staged_deletion_restores_the_entry() {
        let (_d, root) = repo(&[("f", b"a\nb\n")]);
        git(&root, &["rm", "-q", "--cached", "f"]);
        let repo = r(&root);
        let d = file_diff(&repo, Path::new("f"), Side::Staged, GIT_CONTEXT).unwrap();
        unstage_hunk(&repo, Path::new("f"), &d, 0).unwrap();
        assert_eq!(git(&root, &["status", "--porcelain"]), "");
    }

    #[test]
    fn whole_file_stage_and_unstage() {
        let (_d, root) = repo(&[("f", b"a\n"), ("g", b"g\n")]);
        std::fs::write(root.join("f"), "b\n").unwrap();
        std::fs::remove_file(root.join("g")).unwrap();
        std::fs::write(root.join("h"), "h\n").unwrap();
        let repo = r(&root);
        for p in ["f", "g", "h"] {
            stage_file(&repo, Path::new(p)).unwrap();
        }
        assert_eq!(git(&root, &["status", "--porcelain"]), "M  f\nD  g\nA  h\n");
        for p in ["f", "g", "h"] {
            unstage_file(&repo, Path::new(p)).unwrap();
        }
        assert_eq!(git(&root, &["status", "--porcelain"]), " M f\n D g\n?? h\n");
    }

    #[test]
    fn unstage_file_without_commits() {
        let (_d, root) = repo(&[]);
        std::fs::write(root.join("h"), "h\n").unwrap();
        let repo = r(&root);
        stage_file(&repo, Path::new("h")).unwrap();
        unstage_file(&repo, Path::new("h")).unwrap();
        assert_eq!(git(&root, &["status", "--porcelain"]), "?? h\n");
    }

    #[test]
    fn stale_and_binary_hunks_are_refused() {
        let (_d, root) = repo(&[("f", b"a\nb\n"), ("bin", b"a\0b")]);
        std::fs::write(root.join("f"), "a\nB\n").unwrap();
        std::fs::write(root.join("bin"), b"a\0c").unwrap();
        let repo = r(&root);
        let d = file_diff(&repo, Path::new("f"), Side::Unstaged, GIT_CONTEXT).unwrap();
        std::fs::write(root.join("f"), "A\nB\n").unwrap();
        let e = stage_hunk(&repo, Path::new("f"), &d, 0).unwrap_err();
        assert!(matches!(e, Error::Stale), "{e}");
        let d = file_diff(&repo, Path::new("bin"), Side::Unstaged, GIT_CONTEXT).unwrap();
        assert!(d.diff.binary);
        let e = stage_hunk(&repo, Path::new("bin"), &d, 0).unwrap_err();
        assert!(matches!(e, Error::Binary), "{e}");
        // A hunk that isn't there.
        let d = file_diff(&repo, Path::new("f"), Side::Unstaged, GIT_CONTEXT).unwrap();
        assert!(matches!(stage_hunk(&repo, Path::new("f"), &d, 1), Err(Error::Stale)));
        // Refusals leave the index alone.
        assert_eq!(git(&root, &["diff", "--cached"]), "");
    }

    /// The other side changing makes a hunk stale too: the index (staging), HEAD (unstaging).
    #[test]
    fn index_or_head_changes_make_hunks_stale() {
        let (_d, root) = repo(&[("f", b"a\nb\nc\n")]);
        std::fs::write(root.join("f"), "a\nB\nc\n").unwrap();
        let repo = r(&root);
        let unstaged = file_diff(&repo, Path::new("f"), Side::Unstaged, GIT_CONTEXT).unwrap();
        std::fs::write(root.join("f"), "A\nb\nc\n").unwrap();
        git(&root, &["add", "f"]);
        std::fs::write(root.join("f"), "a\nB\nc\n").unwrap();
        let before = index_state(&root);
        let e = stage_hunk(&repo, Path::new("f"), &unstaged, 0).unwrap_err();
        assert!(matches!(e, Error::Stale), "{e}");
        assert_eq!(index_state(&root), before);

        let staged = file_diff(&repo, Path::new("f"), Side::Staged, GIT_CONTEXT).unwrap();
        // HEAD moves (the staged text committed): the index is the same, the diff isn't.
        git(&root, &["commit", "-q", "-m", "two"]);
        let e = unstage_hunk(&repo, Path::new("f"), &staged, 0).unwrap_err();
        assert!(matches!(e, Error::Stale), "{e}");
        // The index moves.
        git(&root, &["reset", "-q", "HEAD~"]);
        let staged = file_diff(&repo, Path::new("f"), Side::Staged, GIT_CONTEXT).unwrap();
        assert!(staged.diff.hunks.is_empty());
        git(&root, &["add", "f"]);
        let shown = FileDiff {
            old: b"a\nb\nc\n".to_vec(),
            new: b"A\nb\nc\n".to_vec(),
            diff: diff::diff(b"a\nb\nc\n", b"A\nb\nc\n", GIT_CONTEXT).unwrap(),
        };
        let e = unstage_hunk(&repo, Path::new("f"), &shown, 0).unwrap_err();
        assert!(matches!(e, Error::Stale), "{e}");
    }

    /// An unmerged file (conflict stages in the index) can't be staged by hunk; its status is
    /// conflicted and the status bar shows `!`.
    #[test]
    fn conflicted_files_are_refused() {
        let (_d, root) = repo(&[("f", b"base\n"), ("g", b"g\n")]);
        let blob = |text: &str| git_with_input(&root, &["hash-object", "-w", "--stdin"], Some(text));
        let (base, ours, theirs) = (blob("base\n"), blob("ours\n"), blob("theirs\n"));
        git(&root, &["rm", "-q", "--cached", "f"]);
        let info = format!(
            "100644 {} 1\tf\n100644 {} 2\tf\n100644 {} 3\tf\n",
            base.trim(),
            ours.trim(),
            theirs.trim()
        );
        git_with_input(&root, &["update-index", "--index-info"], Some(&info));
        std::fs::write(root.join("f"), "<<<<<<<\nours\n=======\ntheirs\n>>>>>>>\n").unwrap();
        let repo = r(&root);
        let s = super::super::Status::read(&repo).unwrap();
        let e = s.entry(Path::new("f")).unwrap();
        assert_eq!((e.staged, e.unstaged), (None, Some(super::super::Change::Conflicted)));
        assert_eq!(
            s.decoration(&root.join("f")),
            Some(super::super::Decoration::Conflicted)
        );
        assert_eq!(s.label(), "main!");
        let d = file_diff(&repo, Path::new("f"), Side::Unstaged, GIT_CONTEXT).unwrap();
        let e = stage_hunk(&repo, Path::new("f"), &d, 0).unwrap_err();
        assert!(matches!(e, Error::Conflicted), "{e}");
        let e = unstage_hunk(&repo, Path::new("f"), &d, 0).unwrap_err();
        assert!(matches!(e, Error::Conflicted), "{e}");
        // Staging the whole file resolves it.
        stage_file(&repo, Path::new("f")).unwrap();
        assert_eq!(git(&root, &["status", "--porcelain"]), "M  f\n");
    }

    /// Staging by line equals `git add -p` for a change alone in its hunk, and `s`plit + `y` for
    /// one that `add -p` merges with a neighbour.
    #[test]
    fn stage_line_like_add_p() {
        let text = numbered(40);
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let changed = text
            .replace("line 2\n", "two\n")
            .replace("line 4\n", "four\n")
            .replace("line 20\n", "twenty\nmore\n")
            .replace("line 35\n", "");
        std::fs::write(root.join("f.txt"), &changed).unwrap();
        // Line 21 ("more") is in the second `add -p` hunk.
        let other = copy(&root);
        let staged = stage_line(&r(&other), Path::new("f.txt"), 21).unwrap();
        assert_eq!(
            staged,
            Some(Hunk {
                old: 20..21,
                new: 20..22
            })
        );
        git_with_input(&root, &["add", "-p", "--", "f.txt"], Some("n\ny\nn\n"));
        assert_eq!(index_state(&other), index_state(&root));
        std::fs::remove_dir_all(&other).unwrap();
        git(&root, &["reset", "-q"]);
        // Line 4: `add -p` shows lines 2 and 4 as one hunk; split it, skip the first.
        let other = copy(&root);
        stage_line(&r(&other), Path::new("f.txt"), 4).unwrap().unwrap();
        git_with_input(&root, &["add", "-p", "--", "f.txt"], Some("s\nn\ny\nn\nn\n"));
        assert_eq!(index_state(&other), index_state(&root));
        std::fs::remove_dir_all(&other).unwrap();
        git(&root, &["reset", "-q"]);
        // The deletion (old line 35) is drawn below line 35 of the new text ("line 34").
        let other = copy(&root);
        stage_line(&r(&other), Path::new("f.txt"), 35).unwrap().unwrap();
        git_with_input(&root, &["add", "-p", "--", "f.txt"], Some("n\nn\ny\n"));
        assert_eq!(index_state(&other), index_state(&root));
        // No change on line 10.
        assert_eq!(stage_line(&r(&root), Path::new("f.txt"), 10).unwrap(), None);
    }

    /// The file changing between the diff and the staging: diffed again once, then `Stale`.
    #[test]
    fn stage_line_diffs_again_when_the_file_moved_on() {
        let text = numbered(10);
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        std::fs::write(root.join("f.txt"), text.replace("line 5\n", "five\n")).unwrap();
        // Another line changed after the first diff: diffed again, only line 5's change staged.
        let moved = text.replace("line 5\n", "five\n").replace("line 8\n", "eight\n");
        let mut calls = 0;
        let got = stage_line_with(&r(&root), Path::new("f.txt"), 5, || {
            calls += 1;
            if calls == 1 {
                std::fs::write(root.join("f.txt"), &moved).unwrap();
            }
        });
        assert_eq!(calls, 2, "diffed again");
        assert_eq!(got.unwrap(), Some(Hunk { old: 5..6, new: 5..6 }));
        let cached = git(&root, &["diff", "--cached"]);
        assert!(cached.contains("+five") && !cached.contains("eight"), "{cached}");
        // Moving on every time: refused after the second try, nothing staged.
        git(&root, &["reset", "-q"]);
        let mut n = 0;
        let got = stage_line_with(&r(&root), Path::new("f.txt"), 5, || {
            n += 1;
            std::fs::write(root.join("f.txt"), moved.replace("eight", &format!("eight {n}"))).unwrap();
        });
        assert!(matches!(got, Err(Error::Stale)), "{got:?}");
        assert_eq!(n, 2);
        assert_eq!(git(&root, &["diff", "--cached"]), "");
    }

    #[test]
    fn unstage_line_like_reset_p() {
        let text = numbered(40);
        let (_d, root) = repo(&[("f.txt", text.as_bytes())]);
        let staged = text.replace("line 2\n", "two\n").replace("line 30\n", "thirty\n");
        std::fs::write(root.join("f.txt"), &staged).unwrap();
        git(&root, &["add", "f.txt"]);
        // The work tree (the buffer) has two more lines at the top: line 32 is "thirty".
        let worktree = format!("new\nnew\n{staged}");
        std::fs::write(root.join("f.txt"), &worktree).unwrap();
        let other = copy(&root);
        let repo = r(&other);
        assert_eq!(
            unstage_line(&repo, Path::new("f.txt"), worktree.as_bytes(), 10).unwrap(),
            None
        );
        let h = unstage_line(&repo, Path::new("f.txt"), worktree.as_bytes(), 32).unwrap();
        assert_eq!(
            h,
            Some(Hunk {
                old: 30..31,
                new: 30..31
            })
        );
        git_with_input(&root, &["reset", "-q", "-p", "--", "f.txt"], Some("n\ny\n"));
        assert_eq!(index_state(&other), index_state(&root));
    }

    #[test]
    fn index_blob_skips_a_known_blob() {
        let (_d, root) = repo(&[("f", b"a\n")]);
        let repo = r(&root);
        let IndexBlob::Text(id, t) = index_blob(&repo, Path::new("f"), None).unwrap() else {
            panic!("in the index")
        };
        assert_eq!(t.bytes, b"a\n");
        assert_eq!(index_blob(&repo, Path::new("f"), Some(id)).unwrap(), IndexBlob::Same);
        assert_eq!(index_blob(&repo, Path::new("g"), None).unwrap(), IndexBlob::Missing);
    }

    #[test]
    fn zero_context_hunk_stages_one_change() {
        let text = numbered(10);
        let (_d, root) = repo(&[("f", text.as_bytes())]);
        std::fs::write(
            root.join("f"),
            text.replace("line 2\n", "two\n").replace("line 4\n", "four\n"),
        )
        .unwrap();
        let repo = r(&root);
        let d = file_diff(&repo, Path::new("f"), Side::Unstaged, 0).unwrap();
        assert_eq!(d.diff.hunks.len(), 2);
        stage_hunk(&repo, Path::new("f"), &d, 1).unwrap();
        let staged = index_text(&repo, Path::new("f")).unwrap().unwrap().bytes;
        assert_eq!(String::from_utf8(staged).unwrap(), text.replace("line 4\n", "four\n"));
    }
}
