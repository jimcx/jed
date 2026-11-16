//! The user's own shell in a tab (`Tab::Shell`): `jterm::shell` (`ShellTab`, `title`, `base`)
//! re-exported, plus jed's `spec`, which sets `JED_IPC` so `git commit` and `jed <file>` from the
//! shell open in this jed. `app/shell.rs` opens the tab, numbers its title, sends it keys and
//! pastes, hands its events on and hangs it up at quit; `ui::task::render_shell` draws it. Its exit
//! note is `tasks::end_note("Shell", …)`.

use std::path::Path;

use crate::tasks::templates::Spec;

pub use jterm::shell::*;

/// What a shell is started with (`jterm::shell::spec`): `JED_IPC` set when there's a socket.
pub fn spec(program: &Path, root: &Path, ipc: Option<&Path>) -> Spec {
    jterm::shell::spec(program, root, ipc.map(|socket| (crate::ipc::ENV, socket)))
}

/// Most paths a paste can hold and still be taken for dropped files.
const MAX_DROPPED: usize = 64;

/// A paste that is files dragged in from the desktop, as the shell should see them: each path
/// shell-quoted, joined by spaces, a space after (as Terminal.app and iTerm2 type them), else
/// `None`. Some terminals (kitty) paste a drop as bare absolute paths, one per line, so a path
/// with a space split into words and several files ran as commands. A paste counts as a drop when
/// it is one absolute path, or one per line, and each exists (`exists`, `Path::exists` in the
/// app); one already escaped (`/a\ b`) names no file, so it goes as pasted.
pub fn dropped_paths(text: &str, exists: impl Fn(&Path) -> bool) -> Option<String> {
    let text = text.trim_end_matches(['\r', '\n']);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() || lines.len() > MAX_DROPPED {
        return None;
    }
    let dropped = |p: &str| p.starts_with('/') && !p.contains(|c: char| c.is_control()) && exists(Path::new(p));
    if !lines.iter().all(|l| dropped(l)) {
        return None;
    }
    let mut out = String::new();
    for path in lines {
        out.push_str(&crate::recent::shell_quote(path));
        out.push(' ');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_quoted_and_joined() {
        let exists = |p: &Path| p.starts_with("/Users/alice");
        let drop = |t: &str| dropped_paths(t, exists);
        assert_eq!(
            drop("/Users/alice/My File.txt").as_deref(),
            Some("'/Users/alice/My File.txt' ")
        );
        assert_eq!(
            drop("/Users/alice/plain.txt\n").as_deref(),
            Some("/Users/alice/plain.txt ")
        );
        assert_eq!(
            drop("/Users/alice/a b\r\n/Users/alice/it's\n").as_deref(),
            Some(r"'/Users/alice/a b' '/Users/alice/it'\''s' ")
        );
        // Not a drop: a missing file, a relative path, other text, an already-escaped path.
        assert_eq!(drop("/Users/alice/a\n/elsewhere/b"), None);
        assert_eq!(drop("alice/a b"), None);
        assert_eq!(drop("echo hi"), None);
        assert_eq!(drop(""), None);
        assert_eq!(
            dropped_paths(r"/Users/alice/a\ b", |p| p == Path::new("/Users/alice/a b")),
            None
        );
    }
    use crate::tasks::{Exit, Status};
    use std::path::PathBuf;

    /// The shell's exit note (`tasks::end_note` with the "Shell" noun).
    fn end_note(name: &str, status: &Status) -> Option<String> {
        crate::tasks::end_note("Shell", name, status)
    }

    #[test]
    fn end_notes_say_how_the_shell_ended() {
        assert_eq!(end_note("zsh", &Status::Starting), None);
        assert_eq!(end_note("zsh", &Status::Running), None);
        let exited = |code, signal, killed| end_note("zsh", &Status::Exited(Exit { code, signal, killed }));
        assert_eq!(
            exited(3, None, false).as_deref(),
            Some("Shell 'zsh' exited with code 3")
        );
        assert_eq!(
            exited(0, None, false).as_deref(),
            Some("Shell 'zsh' exited with code 0")
        );
        assert_eq!(exited(1, None, true).as_deref(), Some("Shell 'zsh' was killed"));
        assert_eq!(
            exited(1, Some("Hangup".into()), false).as_deref(),
            Some("Shell 'zsh' ended by signal: Hangup")
        );
        assert_eq!(
            end_note("zsh", &Status::Failed("/bin/zsh: not found".into())).as_deref(),
            Some("Shell 'zsh' failed to start: /bin/zsh: not found")
        );
    }

    #[test]
    fn the_spec_sets_jeds_ipc_variable() {
        let root = PathBuf::from("/work");
        assert!(
            !spec(Path::new("/bin/zsh"), &root, None)
                .env
                .iter()
                .any(|(k, _)| k == "JED_IPC")
        );
        let socket = PathBuf::from("/run/user/1/jed.sock");
        let spec = spec(Path::new("/bin/bash"), &root, Some(&socket));
        assert_eq!(
            spec.env.last(),
            Some(&("JED_IPC".to_string(), "/run/user/1/jed.sock".to_string()))
        );
    }
}
