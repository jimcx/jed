//! The user's own shell in a tab (`Tab::Shell`): a `tasks::Run` on a PTY, started as a terminal
//! emulator starts one (interactive, no `-c`, no rc-skipping flags), so the user's regular init
//! (`.zshrc` / `.bashrc` / `config.fish`) runs; `JED_IPC` lets `git commit` and `jed <file>` from
//! it open in this jed.
//!
//! No drawing (`ui::task::render_shell` draws the screen) and no app state (`app/shell.rs` opens
//! it, numbers its title, sends it keys and pastes, hands its events on and hangs it up at quit).
//! Its exit note is `tasks::end_note("Shell", …)`.

use std::path::Path;

use crate::tasks::templates::{self, Spec};
use crate::tasks::{Run, RunId, Sink};

/// A shell tab: its run, the title the tab bar shows, and the program's name (`zsh`), which
/// numbers the next shell of the same program.
pub struct ShellTab {
    pub run: Run,
    pub title: String,
    /// The shell's file name ("zsh", "sh"), for "zsh (2)".
    pub base: String,
}

impl ShellTab {
    /// Starts `spec` in a `rows` × `cols` PTY, titled `title` (`App::new_shell` numbers it: the
    /// first shell of a program is its name).
    pub fn new(id: RunId, spec: Spec, rows: u16, cols: u16, sink: Sink, scrollback: usize, title: String) -> ShellTab {
        let base = spec.name.clone();
        ShellTab {
            run: Run::start(id, spec, rows, cols, sink, scrollback),
            title,
            base,
        }
    }
}

/// The tab bar's name for the `number`th shell of `base`: `zsh` for the first, `zsh (2)` next.
pub fn title(base: &str, number: usize) -> String {
    if number <= 1 {
        base.to_string()
    } else {
        format!("{base} ({number})")
    }
}

/// A shell's name in the tab bar and a run's `name`: the program's file name.
pub fn base(program: &Path) -> String {
    program.file_name().map_or_else(
        || program.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// What a shell is started with: `program` alone (no `-c`, no `--norc` / `--noprofile`: the
/// regular init runs), in `root`, with a colour terminal and `JED_IPC` when there's a socket.
pub fn spec(program: &Path, root: &Path, ipc: Option<&Path>) -> Spec {
    let mut env: Vec<(String, String)> = templates::DEFAULT_ENV
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    if let Some(socket) = ipc {
        env.push((crate::ipc::ENV.into(), socket.to_string_lossy().into_owned()));
    }
    Spec {
        name: base(program),
        command: program.to_string_lossy().into_owned(),
        argv: vec![program.as_os_str().to_os_string()],
        cwd: root.to_path_buf(),
        env,
    }
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
    fn titles_number_repeated_programs() {
        assert_eq!(title("zsh", 1), "zsh");
        assert_eq!(title("zsh", 2), "zsh (2)");
        assert_eq!(title("zsh", 12), "zsh (12)");
        assert_eq!(title("sh", 0), "sh");
    }

    #[test]
    fn base_is_the_program_file_name() {
        assert_eq!(base(Path::new("/bin/zsh")), "zsh");
        assert_eq!(base(Path::new("/usr/local/bin/fish")), "fish");
        assert_eq!(base(Path::new("sh")), "sh");
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
    fn the_spec_runs_the_shell_as_a_terminal_does() {
        let root = PathBuf::from("/work");
        let none = self::spec(Path::new("/bin/zsh"), &root, None);
        assert_eq!(none.name, "zsh");
        assert_eq!(none.argv, [std::ffi::OsString::from("/bin/zsh")], "no -c, no rc flags");
        assert_eq!(none.cwd, root);
        assert_eq!(
            none.env,
            [
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string())
            ]
        );

        let socket = PathBuf::from("/run/user/1/jed.sock");
        let spec = self::spec(Path::new("/bin/bash"), &root, Some(&socket));
        assert_eq!(spec.name, "bash");
        assert_eq!(
            spec.env.last(),
            Some(&("JED_IPC".to_string(), "/run/user/1/jed.sock".to_string()))
        );
    }
}
