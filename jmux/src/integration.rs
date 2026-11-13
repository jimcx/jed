//! jmux's shell integration (`jterm::shell_integration` with jmux's names): checking whether a
//! shell has it, the one-line notice when it doesn't, and installing it (the snippets into the
//! state dir, the line that sources one into the shell's init file). Used by the server (the first
//! shell's check, `Command::InstallShellIntegration`) and by `jmux install-shell-integration`.
//! Every directory comes in through [`Dirs`], so tests use temporary ones and never the real home.

use std::path::{Path, PathBuf};

use jterm::shell_integration::{self, Names, Status};

/// What jmux's snippets name: the header comment, the guard variable (`$JMUX`, set only inside
/// jmux's terminals) and the functions' prefix.
pub const NAMES: Names = Names {
    app: crate::APP,
    guard: "JMUX",
    prefix: "_jmux",
};

/// Where the integration lives on disk.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, else `~/.config` (fish's init file).
    pub config_home: PathBuf,
    /// Where the snippets are written.
    pub state: PathBuf,
}

impl Dirs {
    /// The user's own directories.
    pub fn real() -> Dirs {
        Dirs {
            home: jtui::paths::home(),
            config_home: jtui::paths::config_home(),
            state: jtui::paths::state_dir(crate::APP),
        }
    }
}

/// Whether `shell_base`'s integration is installed and current; `None` for a shell without one.
///
/// Blocking: call from `spawn_blocking`.
pub fn check(dirs: &Dirs, shell_base: &str) -> Option<Status> {
    shell_integration::status(&NAMES, &dirs.home, &dirs.config_home, &dirs.state, shell_base)
}

/// The notice for a shell whose integration is missing or out of date; `None` when it's current or
/// the shell has none.
pub fn notice(shell_base: &str, status: Option<Status>) -> Option<String> {
    let what = match status? {
        Status::NotInstalled => "isn't installed",
        Status::Stale => "is out of date",
        Status::UpToDate => return None,
    };
    Some(format!(
        "Shell integration for {shell_base} {what}: run 'jmux install-shell-integration'"
    ))
}

/// What an install did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    /// The line was added to the init file.
    Added(PathBuf),
    /// The line was already there; the snippet was rewritten.
    Refreshed(PathBuf),
}

impl Installed {
    /// The one line that says it (`~` for the home directory).
    pub fn message(&self, home: &Path) -> String {
        match self {
            Installed::Added(init) => format!(
                "Added to {} — open a new terminal to use it",
                jtui::paths::display(init, home)
            ),
            Installed::Refreshed(init) => format!(
                "Updated the snippet; {} already sources it — open a new terminal to use it",
                jtui::paths::display(init, home)
            ),
        }
    }
}

/// Writes the snippets and, unless the init file already sources the snippet, appends the line.
/// A shell with no snippet (`sh`) is an error naming the ones that have one.
///
/// Blocking: call from `spawn_blocking`.
pub fn install(dirs: &Dirs, shell_base: &str) -> Result<Installed, String> {
    // Two installs at once (the command twice, quickly) must not both append the line.
    static INSTALLING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALLING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(init) = shell_integration::init_for(&dirs.home, &dirs.config_home, &dirs.state, shell_base) else {
        return Err(format!(
            "No shell integration for '{shell_base}': zsh, bash and fish have one"
        ));
    };
    let status = check(dirs, shell_base);
    shell_integration::write_snippets(&NAMES, &dirs.state)?;
    if matches!(status, Some(Status::Stale | Status::UpToDate)) {
        return Ok(Installed::Refreshed(init.init));
    }
    shell_integration::append_line(&init.init, &init.source)?;
    Ok(Installed::Added(init.init))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(root: &Path) -> Dirs {
        Dirs {
            home: root.join("home"),
            config_home: root.join("home/.config"),
            state: root.join("state"),
        }
    }

    #[test]
    fn the_notice_names_the_problem_and_the_fix() {
        let n = |s| notice("zsh", s);
        assert!(n(Some(Status::NotInstalled)).unwrap().contains("isn't installed"));
        assert!(n(Some(Status::Stale)).unwrap().contains("out of date"));
        assert!(
            n(Some(Status::NotInstalled))
                .unwrap()
                .contains("jmux install-shell-integration")
        );
        assert_eq!(n(Some(Status::UpToDate)), None);
        assert_eq!(n(None), None);
    }

    #[test]
    fn install_writes_the_snippets_and_appends_the_line_once() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(&d.home).unwrap();
        std::fs::write(d.home.join(".bashrc"), "alias x=y").unwrap();
        assert_eq!(check(&d, "bash"), Some(Status::NotInstalled));
        let done = install(&d, "bash").unwrap();
        assert_eq!(done, Installed::Added(d.home.join(".bashrc")));
        assert!(done.message(&d.home).contains("~/.bashrc"));
        let rc = std::fs::read_to_string(d.home.join(".bashrc")).unwrap();
        assert!(
            rc.starts_with("alias x=y\nsource '") && rc.contains("shell-integration.bash"),
            "{rc}"
        );
        let snippet = std::fs::read_to_string(d.state.join("shell-integration.bash")).unwrap();
        assert!(snippet.contains("JMUX"), "the guard is jmux's");
        assert_eq!(check(&d, "bash"), Some(Status::UpToDate));
        // Again: refreshed, the line not added twice.
        assert_eq!(
            install(&d, "bash").unwrap(),
            Installed::Refreshed(d.home.join(".bashrc"))
        );
        let rc2 = std::fs::read_to_string(d.home.join(".bashrc")).unwrap();
        assert_eq!(rc, rc2);
    }

    #[test]
    fn installs_at_the_same_time_add_the_line_once() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let d = d.clone();
                std::thread::spawn(move || install(&d, "bash").unwrap())
            })
            .collect();
        let added = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|r| matches!(r, Installed::Added(_)))
            .count();
        assert_eq!(added, 1);
        let rc = std::fs::read_to_string(d.home.join(".bashrc")).unwrap();
        assert_eq!(rc.lines().count(), 1, "{rc}");
    }

    #[test]
    fn a_stale_snippet_is_rewritten_without_a_second_line() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        install(&d, "zsh").unwrap();
        std::fs::write(d.state.join("shell-integration.zsh"), "old").unwrap();
        assert_eq!(check(&d, "zsh"), Some(Status::Stale));
        install(&d, "zsh").unwrap();
        assert_eq!(check(&d, "zsh"), Some(Status::UpToDate));
        let rc = std::fs::read_to_string(d.home.join(".zshrc")).unwrap();
        assert_eq!(rc.lines().count(), 1);
    }

    #[test]
    fn a_shell_without_a_snippet_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let e = install(&d, "sh").unwrap_err();
        assert!(e.contains("zsh, bash and fish"), "{e}");
        assert!(!d.state.exists(), "nothing was written");
        assert_eq!(check(&d, "sh"), None);
    }
}
