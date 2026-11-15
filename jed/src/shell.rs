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

#[cfg(test)]
mod tests {
    use super::*;
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
