//! Git as `core.editor` (editor mode): recognising the message files git hands its editor, and
//! the work tree to use as the root. Pure but for [`root`]; `main` decides the mode from here
//! and `app::App::editor_hint` carries the status bar text.

use std::path::{Path, PathBuf};

/// Names of the message files git opens, whose git dir is the file's folder.
const MESSAGES: [&str; 6] = [
    "COMMIT_EDITMSG",
    "MERGE_MSG",
    "SQUASH_MSG",
    "TAG_EDITMSG",
    "EDIT_DESCRIPTION",
    "NOTES_EDITMSG",
];
const REBASE_TODO: &str = "git-rebase-todo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A commit, merge, tag or notes message.
    Message,
    /// The `git rebase -i` todo list.
    Rebase,
}

impl Kind {
    /// The status bar text in place of " Single file ".
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Message => " Save and close to commit · an empty message aborts ",
            Kind::Rebase => " Save and close to rebase · an empty list aborts ",
        }
    }
}

/// The git directory `path` is a message file of: its folder (`.git/`, `.git/worktrees/<name>/`,
/// a bare repo, `$GIT_DIR`) for the message names, the parent of `rebase-merge/` /
/// `rebase-apply/` for the todo list; in both cases the folder must hold `HEAD`.
pub fn git_dir(path: &Path) -> Option<(Kind, PathBuf)> {
    let name = path.file_name()?.to_str()?;
    let dir = path.parent()?;
    let (kind, git_dir) = if MESSAGES.contains(&name) {
        (Kind::Message, dir)
    } else if name == REBASE_TODO {
        let state = dir.file_name()?.to_str()?;
        if state != "rebase-merge" && state != "rebase-apply" {
            return None;
        }
        (Kind::Rebase, dir.parent()?)
    } else {
        return None;
    };
    git_dir.join("HEAD").is_file().then(|| (kind, git_dir.to_path_buf()))
}

pub fn kind(path: &Path) -> Option<Kind> {
    git_dir(path).map(|(kind, _)| kind)
}

/// The root for a message file: its repository's work tree (a worktree's own), a bare
/// repository's git dir. `None` when `path` isn't a message file or git can't open the directory.
///
/// Blocking: call before the runtime and the UI start (`main`)
pub fn root(path: &Path) -> Option<PathBuf> {
    let (_, git_dir) = git_dir(path)?;
    let repo = git2::Repository::open(&git_dir).ok()?;
    let root = repo.workdir().unwrap_or(&git_dir);
    Some(crate::workspace::normalize(root))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn message_files_need_head_beside_them() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        touch(&d.join(".git/HEAD"));
        for name in MESSAGES {
            assert_eq!(kind(&d.join(".git").join(name)), Some(Kind::Message), "{name}");
        }
        assert_eq!(kind(&d.join(".git/other")), None);
        assert_eq!(kind(&d.join("COMMIT_EDITMSG")), None, "no HEAD beside it");
        touch(&d.join("plain/COMMIT_EDITMSG"));
        assert_eq!(kind(&d.join("plain/COMMIT_EDITMSG")), None);
        // A linked worktree's git dir.
        touch(&d.join(".git/worktrees/w/HEAD"));
        assert_eq!(kind(&d.join(".git/worktrees/w/COMMIT_EDITMSG")), Some(Kind::Message));
    }

    #[test]
    fn rebase_todo_sits_in_the_rebase_dir() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        touch(&d.join(".git/HEAD"));
        for state in ["rebase-merge", "rebase-apply"] {
            let todo = d.join(".git").join(state).join(REBASE_TODO);
            assert_eq!(kind(&todo), Some(Kind::Rebase), "{state}");
            assert_eq!(git_dir(&todo).unwrap().1, d.join(".git"));
        }
        assert_eq!(kind(&d.join(".git").join(REBASE_TODO)), None, "not in a rebase dir");
        assert_eq!(kind(&d.join(".git/other/git-rebase-todo")), None);
        touch(&d.join("x/rebase-merge/git-rebase-todo"));
        assert_eq!(kind(&d.join("x/rebase-merge/git-rebase-todo")), None, "no HEAD");
    }

    #[test]
    fn root_is_the_work_tree_or_the_bare_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = crate::workspace::normalize(dir.path());
        git2::Repository::init(base.join("repo")).unwrap();
        let msg = base.join("repo/.git/COMMIT_EDITMSG");
        touch(&msg);
        assert_eq!(root(&msg), Some(base.join("repo")));

        git2::Repository::init_bare(base.join("bare.git")).unwrap();
        let msg = base.join("bare.git/COMMIT_EDITMSG");
        touch(&msg);
        assert_eq!(root(&msg), Some(base.join("bare.git")));

        assert_eq!(root(&base.join("repo/a.txt")), None);
    }

    /// A git dir away from its work tree (`core.worktree`): a submodule's `.git/modules/<name>/`,
    /// `git init --separate-git-dir`, `$GIT_DIR`; and a message reached through a symlink.
    #[test]
    fn root_of_a_separate_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = crate::workspace::normalize(dir.path());
        let git_dir = base.join("super/.git/modules/sub");
        fs::create_dir_all(base.join("super/sub")).unwrap();
        let mut opts = git2::RepositoryInitOptions::new();
        opts.no_dotgit_dir(true).workdir_path(&base.join("super/sub"));
        git2::Repository::init_opts(&git_dir, &opts).unwrap();
        let msg = git_dir.join("COMMIT_EDITMSG");
        touch(&msg);
        assert_eq!(kind(&msg), Some(Kind::Message));
        assert_eq!(root(&msg), Some(base.join("super/sub")));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&git_dir, base.join("link")).unwrap();
            let linked = crate::workspace::normalize(&base.join("link/COMMIT_EDITMSG"));
            assert_eq!(root(&linked), Some(base.join("super/sub")));
        }
    }

    #[test]
    fn root_of_a_linked_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let base = crate::workspace::normalize(dir.path());
        let repo = git2::Repository::init(base.join("repo")).unwrap();
        let sig = git2::Signature::now("a", "a@b.c").unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        repo.worktree("w", &base.join("wt"), None).unwrap();
        let msg = base.join("repo/.git/worktrees/w/COMMIT_EDITMSG");
        touch(&msg);
        assert_eq!(root(&msg), Some(base.join("wt")));
    }
}
