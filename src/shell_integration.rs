//! Opt-in shell integration: the zsh / bash / fish snippets written into the state dir, and the
//! line that sources one from a shell's init file. Each snippet opens with a `JED_IPC` guard, so
//! sourcing it anywhere else is a no-op; inside jed its preexec hook sends `133;B` and the
//! command's first word as `OSC 0`, and its precmd hook `133;D;<exit code>` — what
//! `tasks::run::TitleCallback` turns into the tab's title and glyph.
//!
//! No app state and no drawing: "Terminal: Install Shell Integration" (`app/shell.rs`) opens the
//! wizard with this data and `app/prompt.rs` copies or appends the line; `ui/dialog.rs` draws it.
//! Every write is blocking (call it from `spawn_blocking`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const ZSH: &str = r#"# jed shell integration (sourced from ~/.zshrc)
[ -n "$JED_IPC" ] || return
_jed_preexec() { printf '\033]133;B\007'; printf '\033]0;%s\007' "${1%% *}"; }
_jed_precmd() { printf '\033]133;D;%s\007' "$?"; }
autoload -Uz add-zsh-hook; add-zsh-hook preexec _jed_preexec; add-zsh-hook precmd _jed_precmd
"#;

const BASH: &str = r#"# jed shell integration (sourced from ~/.bashrc)
[ -n "$JED_IPC" ] || return
_jed_preexec() {
  case "$1" in PROMPT_COMMAND=*) return;; esac
  local pc
  for pc in "${PROMPT_COMMAND[@]}"; do
    [ "$1" = "$pc" ] && return
  done
  printf '\033]133;B\007'; printf '\033]0;%s\007' "${1%% *}"
}
_jed_precmd() { printf '\033]133;D;%s\007' "$?"; }
PROMPT_COMMAND=_jed_precmd
trap '_jed_preexec "$BASH_COMMAND"' DEBUG
"#;

const FISH: &str = r#"# jed shell integration (sourced from ~/.config/fish/config.fish)
set -q JED_IPC; or return
function _jed_preexec --on-event fish_preexec
  printf '\033]133;B\007'; printf '\033]0;%s\007' (string split -m1 ' ' $argv)[1]
end
function _jed_precmd --on-event fish_postexec
  printf '\033]133;D;%s\007' $status
end
"#;

/// A snippet: the shell it's for (`zsh`), its file in the state dir, and its text.
pub struct Snippet {
    pub shell: &'static str,
    pub file: &'static str,
    pub text: &'static str,
}

/// The three snippets, in the order `write_snippets` writes them.
pub const SNIPPETS: [Snippet; 3] = [
    Snippet {
        shell: "zsh",
        file: "shell-integration.zsh",
        text: ZSH,
    },
    Snippet {
        shell: "bash",
        file: "shell-integration.bash",
        text: BASH,
    },
    Snippet {
        shell: "fish",
        file: "shell-integration.fish",
        text: FISH,
    },
];

/// Where to install the integration for a shell: the snippet to source, the init file to source it
/// from, and the one line that does.
pub struct Init {
    pub snippet: PathBuf,
    pub init: PathBuf,
    pub source: String,
}

/// The snippet of `shell_base` (`zsh`, `bash`, `fish`) in `state_dir`.
pub fn snippet_path(state_dir: &Path, shell_base: &str) -> Option<PathBuf> {
    let snippet = SNIPPETS.iter().find(|s| s.shell == shell_base)?;
    Some(state_dir.join(snippet.file))
}

/// The init file `shell_base` reads at start (`~/.zshrc`, `~/.bashrc`,
/// `~/.config/fish/config.fish` — `config_home` is `$XDG_CONFIG_HOME`, else `home/.config`), the
/// snippet in `state_dir` and the line sourcing it; `None` for a shell jed has no snippet for
/// (`sh`, …). Pure: no file is read or written.
pub fn init_for(home: &Path, config_home: &Path, state_dir: &Path, shell_base: &str) -> Option<Init> {
    let snippet = snippet_path(state_dir, shell_base)?;
    let init = match shell_base {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bashrc"),
        "fish" => config_home.join("fish").join("config.fish"),
        _ => return None,
    };
    Some(Init {
        source: format!("source {}", quote(&snippet)),
        snippet,
        init,
    })
}

/// `'path'`: single-quoted, a quote in the path as `'\''`, which POSIX shells and fish read alike.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Writes the three snippets into `state_dir` (created if missing), each replacing the last one.
///
/// Blocking: call from `spawn_blocking`.
pub fn write_snippets(state_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(state_dir).map_err(|e| format!("can't create {}: {e}", state_dir.display()))?;
    for snippet in &SNIPPETS {
        let path = state_dir.join(snippet.file);
        fs::write(&path, snippet.text).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Appends `line` to `init` (the file and its folder created when missing) as one `O_APPEND`
/// write: a newline first when the file has text and doesn't end in one, then `line` and a
/// newline. An init file that already has the line gets it again; sourcing it twice is harmless.
///
/// Blocking: call from `spawn_blocking`.
pub fn append_line(init: &Path, line: &str) -> Result<(), String> {
    if let Some(dir) = init.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    let newline_first = match fs::read(init) {
        Ok(bytes) => !bytes.is_empty() && !bytes.ends_with(b"\n"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("can't read {}: {e}", init.display())),
    };
    let mut text = String::new();
    if newline_first {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(init)
        .map_err(|e| format!("can't open {}: {e}", init.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("can't append to {}: {e}", init.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Each snippet is guarded by `JED_IPC` and sends what the parser reads: `133;B` and the
    /// command's `OSC 0` first word from preexec, `133;D;<code>` from precmd (fish: `$status`).
    #[test]
    fn snippets_are_guarded_and_send_the_osc_codes() {
        for snippet in &SNIPPETS {
            let text = snippet.text;
            assert!(text.contains("JED_IPC"), "{}: no guard", snippet.file);
            assert!(
                text.contains("133;B") && text.contains(r"\033]0;%s"),
                "{}: preexec sends 133;B and the OSC 0 title",
                snippet.file
            );
            assert!(text.contains("133;D;"), "{}: precmd sends 133;D", snippet.file);
            assert!(text.ends_with('\n'), "{}: a whole file", snippet.file);
        }
        let guard = |shell: &str| SNIPPETS.iter().find(|s| s.shell == shell).map(|s| s.text).unwrap();
        assert!(guard("zsh").contains(r#"[ -n "$JED_IPC" ] || return"#));
        assert!(guard("bash").contains(r#"[ -n "$JED_IPC" ] || return"#));
        // Bash's DEBUG trap also fires for every PROMPT_COMMAND hook at the prompt (jed's own
        // `_jed_precmd` and any other shell's — systemd's `__systemd_osc_context_*`, starship's,
        // …): preexec skips any command that names one, or the tab would flash them as running
        // commands at every prompt. `PROMPT_COMMAND=…` assignments are skipped too.
        assert!(guard("bash").contains(r#"for pc in "${PROMPT_COMMAND[@]}"#));
        assert!(guard("bash").contains("PROMPT_COMMAND=*) return"));
        assert!(guard("fish").contains("set -q JED_IPC; or return"));
        assert!(guard("fish").contains("$status"));
        assert_eq!(
            SNIPPETS.map(|s| s.file),
            [
                "shell-integration.zsh",
                "shell-integration.bash",
                "shell-integration.fish"
            ]
        );
    }

    /// The detected shell's init file and the line that sources its snippet; `sh` has neither.
    #[test]
    fn init_for_finds_the_init_file_and_the_source_line() {
        let home = PathBuf::from("/home/me");
        let config = PathBuf::from("/home/me/.config");
        let state = PathBuf::from("/home/me/.local/state/jed");

        let zsh = init_for(&home, &config, &state, "zsh").expect("zsh");
        assert_eq!(zsh.init, PathBuf::from("/home/me/.zshrc"));
        assert_eq!(zsh.snippet, state.join("shell-integration.zsh"));
        assert_eq!(zsh.source, "source '/home/me/.local/state/jed/shell-integration.zsh'");

        let bash = init_for(&home, &config, &state, "bash").expect("bash");
        assert_eq!(bash.init, PathBuf::from("/home/me/.bashrc"));
        assert_eq!(bash.source, "source '/home/me/.local/state/jed/shell-integration.bash'");

        let fish = init_for(&home, &config, &state, "fish").expect("fish");
        assert_eq!(fish.init, PathBuf::from("/home/me/.config/fish/config.fish"));
        assert_eq!(fish.source, "source '/home/me/.local/state/jed/shell-integration.fish'");

        // A shell jed has no snippet for, and a program that only looks like one.
        assert!(init_for(&home, &config, &state, "sh").is_none());
        assert!(init_for(&home, &config, &state, "dash").is_none());
        assert!(init_for(&home, &config, &state, "/bin/zsh").is_none());
        assert!(init_for(&home, &config, &state, "").is_none());
    }

    /// A path with a quote in it stays one shell word.
    #[test]
    fn the_source_line_quotes_the_snippet_path() {
        let home = PathBuf::from("/home/o'brien");
        let init = init_for(&home, &home, Path::new("/state"), "zsh").unwrap();
        assert_eq!(init.source, r#"source '/state/shell-integration.zsh'"#);
        assert_eq!(quote(Path::new("/a b/it's.zsh")), r#"'/a b/it'\''s.zsh'"#);
    }

    /// The state dir gets all three snippets, replacing older ones.
    #[test]
    fn write_snippets_writes_the_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state").join("jed");
        write_snippets(&state).unwrap();
        for snippet in &SNIPPETS {
            let written = fs::read_to_string(state.join(snippet.file)).unwrap();
            assert_eq!(written, snippet.text);
        }

        fs::write(state.join("shell-integration.zsh"), "stale").unwrap();
        write_snippets(&state).unwrap();
        assert_eq!(fs::read_to_string(state.join("shell-integration.zsh")).unwrap(), ZSH);
    }

    /// The line goes in as its own line: a newline first for a file not ending in one, nothing for
    /// an empty or already terminated file, and the folder is made when missing.
    #[test]
    fn append_line_puts_the_line_on_its_own_line() {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("home").join(".bashrc");

        // Missing file and folder: created with just the line.
        append_line(&init, "source '/state/shell-integration.bash'").unwrap();
        assert_eq!(
            fs::read_to_string(&init).unwrap(),
            "source '/state/shell-integration.bash'\n"
        );

        // Text not ending in a newline: one is added first.
        fs::write(&init, "export A=1").unwrap();
        append_line(&init, "source x").unwrap();
        assert_eq!(fs::read_to_string(&init).unwrap(), "export A=1\nsource x\n");

        // Already terminated: the line follows the last one directly.
        fs::write(&init, "export A=1\n").unwrap();
        append_line(&init, "source x").unwrap();
        assert_eq!(fs::read_to_string(&init).unwrap(), "export A=1\nsource x\n");

        // Empty: no leading blank line.
        fs::write(&init, "").unwrap();
        append_line(&init, "source x").unwrap();
        assert_eq!(fs::read_to_string(&init).unwrap(), "source x\n");
    }
}
