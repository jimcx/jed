//! Opt-in shell integration: the zsh / bash / fish snippets written into the state dir, and the
//! line that sources one from a shell's init file. Each snippet opens with a guard on the app's
//! IPC variable (`Names::guard`), so sourcing it anywhere else is a no-op; inside the app its preexec hook sends `133;B` and the
//! command's first word as `OSC 0` (after `633;E;<command line>`, VS Code's escaping), and its
//! precmd hook `133;D;<exit code>` and OSC 7 (`file://host/<cwd>`) — what
//! `run::TitleCallback` turns into the tab's title and glyph and a ring of commands. `133;A` goes at the very
//! start of the prompt (zsh: the precmd hook puts it at the start of `PS1`; bash: a hook at the
//! end of `PROMPT_COMMAND` does; fish: a `fish_prompt` wrapper), so Terminal: Clear keeps the
//! whole prompt, all its lines. The bash precmd hook is prepended to the user's `PROMPT_COMMAND`,
//! which keeps running with the real `$?`.
//!
//! The snippets are templates (`@@APP@@`, `@@GUARD@@`, `@@PREFIX@@`) rendered from the app's
//! `Names`; no app is named here. No app state and no drawing: the app's wizard shows this data.
//! Every write is blocking (call it from `spawn_blocking`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const ZSH: &str = r#"# @@APP@@ shell integration (sourced from ~/.zshrc)
[ -n "$@@GUARD@@" ] || return
# 633;E carries the command line, \ ; and control bytes escaped (\\ and \xNN).
@@PREFIX@@_esc() {
  local s=$1 c i
  @@PREFIX@@_e=$s
  case "$s" in *[\\\;[:cntrl:]]*) ;; *) return;; esac
  @@PREFIX@@_e=
  for ((i=1;i<=${#s};i++)); do
    c=${s[i]}
    case $c in
      \\) @@PREFIX@@_e+='\\';;
      ';'|[[:cntrl:]]) printf -v c '\\x%02x' "'$c"; @@PREFIX@@_e+=$c;;
      *) @@PREFIX@@_e+=$c;;
    esac
  done
}
@@PREFIX@@_preexec() {
  printf '\033]133;B\007'
  @@PREFIX@@_esc "$1"; printf '\033]633;E;%s\007' "$@@PREFIX@@_e"
  printf '\033]0;%s\007' "${1%% *}"
}
@@PREFIX@@_precmd() {
  local s=$? p=${PWD//\%/%25}
  printf '\033]133;D;%s\007' "$s"
  p=${p// /%20}; printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$p"
  # 133;A starts PS1, so every line of the prompt follows it (added again if a theme rebuilt PS1).
  case "$PS1" in *'133;A'*) ;; *) PS1=$'%{\033]133;A\007%}'"$PS1";; esac
  return $s
}
autoload -Uz add-zsh-hook; add-zsh-hook preexec @@PREFIX@@_preexec; add-zsh-hook precmd @@PREFIX@@_precmd
"#;

const BASH: &str = r#"# @@APP@@ shell integration (sourced from ~/.bashrc)
[ -n "$@@GUARD@@" ] || return
# 633;E carries the command line, \ ; and control bytes escaped (\\ and \xNN).
@@PREFIX@@_esc() {
  local s=$1 c i
  @@PREFIX@@_e=$s
  case "$s" in *[\\\;[:cntrl:]]*) ;; *) return;; esac
  @@PREFIX@@_e=
  for ((i=0;i<${#s};i++)); do
    c=${s:i:1}
    case $c in
      \\) @@PREFIX@@_e+='\\';;
      ';'|[[:cntrl:]]) printf -v c '\\x%02x' "'$c"; @@PREFIX@@_e+=$c;;
      *) @@PREFIX@@_e+=$c;;
    esac
  done
}
@@PREFIX@@_preexec() {
  case "$1" in PROMPT_COMMAND=*|@@PREFIX@@_precmd|@@PREFIX@@_mark) return;; esac
  local pc p rest nl=$'\n'
  for pc in "${PROMPT_COMMAND[@]}"; do
    [ "$1" = "$pc" ] && return
    # Each `;`- or newline-separated piece, trimmed (no word splitting, so no globbing).
    rest="${pc//$nl/;};"
    while [ -n "$rest" ]; do
      p=${rest%%;*}; rest=${rest#*;}
      p=${p#"${p%%[![:space:]]*}"}; p=${p%"${p##*[![:space:]]}"}
      [ "$1" = "$p" ] && return
    done
  done
  printf '\033]133;B\007'
  @@PREFIX@@_esc "$1"; printf '\033]633;E;%s\007' "$@@PREFIX@@_e"
  printf '\033]0;%s\007' "${1%% *}"
}
@@PREFIX@@_precmd() {
  local s=$? p=${PWD//\%/%25}
  printf '\033]133;D;%s\007' "$s"
  p=${p// /%20}; printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$p"
  return $s
}
# 133;A starts PS1, so every line of the prompt follows it; \[ \] keep readline's width right.
@@PREFIX@@_mark() { local s=$?; case "$PS1" in *'133;A'*) ;; *) PS1='\[\e]133;A\a\]'"$PS1";; esac; return $s; }
# @@PREFIX@@_precmd runs first, so the user's own PROMPT_COMMAND (a string, or an array in bash 5.1+)
# sees the real $?; @@PREFIX@@_mark last, after any of it that rebuilds PS1 (`__git_ps1`, themes).
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
  *@@PREFIX@@_precmd*) ;;
  "declare -a"*) PROMPT_COMMAND=(@@PREFIX@@_precmd "${PROMPT_COMMAND[@]}" @@PREFIX@@_mark);;
  *) PROMPT_COMMAND="@@PREFIX@@_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"$'\n'@@PREFIX@@_mark;;
esac
trap '@@PREFIX@@_preexec "$BASH_COMMAND"' DEBUG
"#;

const FISH: &str = r#"# @@APP@@ shell integration (sourced from ~/.config/fish/config.fish)
set -q @@GUARD@@; or return
function @@PREFIX@@_preexec --on-event fish_preexec
  printf '\033]133;B\007'
  # 633;E: the command line with \ ; and newlines escaped (\\ and \xNN).
  set -l c (string replace -a '\\' '\\\\' -- $argv[1] | string replace -a ';' '\\x3b' | string join '\\x0a')
  printf '\033]633;E;%s\007' $c
  printf '\033]0;%s\007' (string split -m1 ' ' $argv)[1]
end
function @@PREFIX@@_precmd --on-event fish_postexec
  printf '\033]133;D;%s\007' $status
  printf '\033]7;file://%s%s\007' (hostname) (string escape --style=url -- $PWD)
end
# 133;A before the whole prompt: wrap fish_prompt (once), handing it the status it was called with.
function @@PREFIX@@_status; return $argv[1]; end
if not functions -q @@PREFIX@@_fish_prompt
  functions -c fish_prompt @@PREFIX@@_fish_prompt
  function fish_prompt
    set -l last $status
    printf '\033]133;A\007'
    @@PREFIX@@_status $last
    @@PREFIX@@_fish_prompt
  end
end
"#;

/// What the snippets name: the app (header comment), the environment variable that guards them
/// (set only inside the app's terminals) and the prefix of their shell functions (`_app`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Names<'a> {
    pub app: &'a str,
    pub guard: &'a str,
    pub prefix: &'a str,
}

/// A snippet: the shell it's for (`zsh`), its file in the state dir, and its text.
pub struct Snippet {
    pub shell: &'static str,
    pub file: &'static str,
    pub text: String,
}

/// Fills a template's placeholders; panics (a bug, caught by the tests) when one is left over.
fn render(template: &str, names: &Names) -> String {
    let text = template
        .replace("@@APP@@", names.app)
        .replace("@@GUARD@@", names.guard)
        .replace("@@PREFIX@@", names.prefix);
    assert!(
        !text.contains("@@"),
        "unfilled placeholder in a shell integration snippet"
    );
    text
}

/// The three snippets for `names`, in the order `write_snippets` writes them.
pub fn snippets(names: &Names) -> [Snippet; 3] {
    [
        Snippet {
            shell: "zsh",
            file: "shell-integration.zsh",
            text: render(ZSH, names),
        },
        Snippet {
            shell: "bash",
            file: "shell-integration.bash",
            text: render(BASH, names),
        },
        Snippet {
            shell: "fish",
            file: "shell-integration.fish",
            text: render(FISH, names),
        },
    ]
}

/// Where to install the integration for a shell: the snippet to source, the init file to source it
/// from, and the one line that does.
pub struct Init {
    pub snippet: PathBuf,
    pub init: PathBuf,
    pub source: String,
}

/// The snippet of `shell_base` (`zsh`, `bash`, `fish`) in `state_dir`.
pub fn snippet_path(state_dir: &Path, shell_base: &str) -> Option<PathBuf> {
    let file = match shell_base {
        "zsh" => "shell-integration.zsh",
        "bash" => "shell-integration.bash",
        "fish" => "shell-integration.fish",
        _ => return None,
    };
    Some(state_dir.join(file))
}

/// The init file `shell_base` reads at start (`~/.zshrc`, `~/.bashrc`,
/// `~/.config/fish/config.fish` — `config_home` is `$XDG_CONFIG_HOME`, else `home/.config`), the
/// snippet in `state_dir` and the line sourcing it; `None` for a shell with no snippet
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

/// How a shell's integration stands on disk (`status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The init file has no line sourcing the snippet (or nothing was ever written).
    NotInstalled,
    /// The line is there but the snippet file differs from the one the app embeds (or is missing).
    Stale,
    UpToDate,
}

/// Whether `shell_base`'s integration is installed and current; `None` for a shell without a
/// snippet. `NotInstalled` when no line of the init file is exactly (trimmed) the `source` line
/// `init_for` would append, so a hand-added variant reads `NotInstalled`; then `Stale` when the
/// snippet file's text isn't the embedded one (a missing file too: installing writes it).
///
/// Blocking: call from `spawn_blocking`.
pub fn status(names: &Names, home: &Path, config_home: &Path, state_dir: &Path, shell_base: &str) -> Option<Status> {
    let init = init_for(home, config_home, state_dir, shell_base)?;
    let sourced = fs::read_to_string(&init.init)
        .map(|text| text.lines().any(|l| l.trim() == init.source))
        .unwrap_or(false);
    if !sourced {
        return Some(Status::NotInstalled);
    }
    let embedded = snippets(names).into_iter().find(|s| s.shell == shell_base)?.text;
    match fs::read_to_string(&init.snippet) {
        Ok(text) if text == embedded => Some(Status::UpToDate),
        _ => Some(Status::Stale),
    }
}

/// `'path'`: single-quoted, a quote in the path as `'\''`, which POSIX shells and fish read alike.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Writes the three snippets into `state_dir` (created if missing), each replacing the last one.
///
/// Blocking: call from `spawn_blocking`.
pub fn write_snippets(names: &Names, state_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(state_dir).map_err(|e| format!("can't create {}: {e}", state_dir.display()))?;
    for snippet in snippets(names) {
        let path = state_dir.join(snippet.file);
        fs::write(&path, &snippet.text).map_err(|e| format!("can't write {}: {e}", path.display()))?;
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

    const NAMES: Names = Names {
        app: "alice",
        guard: "ALICE_IPC",
        prefix: "_alice",
    };

    fn snippet(shell: &str) -> String {
        snippets(&NAMES).into_iter().find(|s| s.shell == shell).unwrap().text
    }

    /// The names are the only app-specific text: nothing of another app's and no placeholder is left,
    /// and the header names the app and the shell's init file.
    #[test]
    fn the_names_fill_every_placeholder() {
        for snippet in snippets(&NAMES) {
            assert!(!snippet.text.contains("@@"), "{}", snippet.file);
            assert!(!snippet.text.to_lowercase().contains("jed"), "{}", snippet.file);
            assert!(snippet.text.starts_with("# alice shell integration (sourced from "));
        }
        let other = Names {
            app: "bob",
            guard: "BOB_IPC",
            prefix: "_bob",
        };
        let zsh = &snippets(&other)[0].text;
        assert!(zsh.contains(r#"[ -n "$BOB_IPC" ] || return"#) && zsh.contains("_bob_preexec"));
        assert!(!zsh.contains("alice") && !zsh.contains("ALICE"));
    }

    /// Each snippet is guarded by `ALICE_IPC` and sends what the parser reads: `133;B` and the
    /// command's `OSC 0` first word from preexec, `133;D;<code>` from precmd (fish: `$status`).
    #[test]
    fn snippets_are_guarded_and_send_the_osc_codes() {
        for snippet in &snippets(&NAMES) {
            let text = &snippet.text;
            assert!(text.contains("ALICE_IPC"), "{}: no guard", snippet.file);
            assert!(
                text.contains("133;B") && text.contains(r"\033]0;%s"),
                "{}: preexec sends 133;B and the OSC 0 title",
                snippet.file
            );
            assert!(text.contains("133;D;"), "{}: precmd sends 133;D", snippet.file);
            assert!(
                text.contains("133;A"),
                "{}: sends 133;A before the prompt",
                snippet.file
            );
            assert!(
                text.contains("633;E;") && text.contains("]7;file://"),
                "{}: sends the command line and the directory",
                snippet.file
            );
            assert!(text.ends_with('\n'), "{}: a whole file", snippet.file);
        }
        let guard = snippet;
        assert!(guard("zsh").contains(r#"[ -n "$ALICE_IPC" ] || return"#));
        assert!(guard("bash").contains(r#"[ -n "$ALICE_IPC" ] || return"#));
        // Bash's DEBUG trap also fires for every PROMPT_COMMAND hook at the prompt (the app's own
        // `_alice_precmd` and any other shell's — systemd's `__systemd_osc_context_*`, starship's,
        // …): preexec skips any command that names one, or the tab would flash them as running
        // commands at every prompt. `PROMPT_COMMAND=…` assignments are skipped too.
        assert!(guard("bash").contains(r#"for pc in "${PROMPT_COMMAND[@]}"#));
        assert!(guard("bash").contains("PROMPT_COMMAND=*|_alice_precmd|_alice_mark) return"));
        // The mark leads PS1 (zsh, bash: `\[ \]` for readline), once; bash keeps the user's hooks
        // between its own two (the real bash tests in `clear_scrollback_tests` run them).
        assert!(guard("zsh").contains("*'133;A'*) ;; *) PS1="));
        assert!(guard("bash").contains(r"PS1='\[\e]133;A\a\]'"));
        assert!(guard("bash").contains(r#"_alice_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"$'\n'_alice_mark"#));
        assert!(!guard("bash").contains("PROMPT_COMMAND=_alice_precmd\n"));
        assert!(guard("fish").contains("functions -c fish_prompt _alice_fish_prompt"));
        assert!(guard("fish").contains("set -q ALICE_IPC; or return"));
        assert!(guard("fish").contains("$status"));
        assert_eq!(
            snippets(&NAMES).map(|s| s.file),
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
        let state = PathBuf::from("/home/me/.local/state/alice");

        let zsh = init_for(&home, &config, &state, "zsh").expect("zsh");
        assert_eq!(zsh.init, PathBuf::from("/home/me/.zshrc"));
        assert_eq!(zsh.snippet, state.join("shell-integration.zsh"));
        assert_eq!(zsh.source, "source '/home/me/.local/state/alice/shell-integration.zsh'");

        let bash = init_for(&home, &config, &state, "bash").expect("bash");
        assert_eq!(bash.init, PathBuf::from("/home/me/.bashrc"));
        assert_eq!(
            bash.source,
            "source '/home/me/.local/state/alice/shell-integration.bash'"
        );

        let fish = init_for(&home, &config, &state, "fish").expect("fish");
        assert_eq!(fish.init, PathBuf::from("/home/me/.config/fish/config.fish"));
        assert_eq!(
            fish.source,
            "source '/home/me/.local/state/alice/shell-integration.fish'"
        );

        // A shell with no snippet, and a program that only looks like one.
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
        let state = dir.path().join("state").join("alice");
        write_snippets(&NAMES, &state).unwrap();
        for snippet in &snippets(&NAMES) {
            let written = fs::read_to_string(state.join(snippet.file)).unwrap();
            assert_eq!(written, snippet.text);
        }

        fs::write(state.join("shell-integration.zsh"), "stale").unwrap();
        write_snippets(&NAMES, &state).unwrap();
        assert_eq!(
            fs::read_to_string(state.join("shell-integration.zsh")).unwrap(),
            snippet("zsh")
        );
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

    /// Each `Status`, the fish init path, a missing state dir, a near-miss source line and `sh`.
    #[test]
    fn status_reads_the_init_file_and_the_snippet() {
        let dir = tempfile::tempdir().unwrap();
        let (home, cfg, state) = (dir.path().join("h"), dir.path().join("c"), dir.path().join("s"));
        let st = |shell: &str| status(&NAMES, &home, &cfg, &state, shell);
        assert_eq!(st("sh"), None);
        // Nothing written: no state dir, no init file.
        assert_eq!(st("zsh"), Some(Status::NotInstalled));
        write_snippets(&NAMES, &state).unwrap();
        assert_eq!(st("zsh"), Some(Status::NotInstalled));
        let zsh = init_for(&home, &cfg, &state, "zsh").unwrap();
        // A source line of another path is a near-miss.
        append_line(&zsh.init, "source '/elsewhere/shell-integration.zsh'").unwrap();
        assert_eq!(st("zsh"), Some(Status::NotInstalled));
        append_line(&zsh.init, &format!("  {}  ", zsh.source)).unwrap();
        assert_eq!(st("zsh"), Some(Status::UpToDate));
        fs::write(&zsh.snippet, "old").unwrap();
        assert_eq!(st("zsh"), Some(Status::Stale));
        fs::remove_file(&zsh.snippet).unwrap();
        assert_eq!(st("zsh"), Some(Status::Stale));
        // fish reads `config_home/fish/config.fish`.
        let fish = init_for(&home, &cfg, &state, "fish").unwrap();
        assert!(fish.init.starts_with(&cfg));
        write_snippets(&NAMES, &state).unwrap();
        append_line(&fish.init, &fish.source).unwrap();
        assert_eq!(st("fish"), Some(Status::UpToDate));
        assert_eq!(st("bash"), Some(Status::NotInstalled));
    }

    /// A real interactive bash (skipped without one) sources the snippet rendered with another
    /// app's names: with `ALICE_IPC` in the run's environment its hooks record commands
    /// (`133;B` .. `133;D;3`), without it the guard makes the snippet a no-op.
    #[tokio::test(flavor = "multi_thread")]
    async fn another_apps_names_work_in_a_real_bash() {
        use crate::run::{EnvNames, Event, Run, RunId, SCROLLBACK, Sink, StartOptions};
        use crate::spec::Spec;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let Some(bash) = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        else {
            eprintln!("skipped another_apps_names_work_in_a_real_bash: needs bash");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        write_snippets(&NAMES, dir.path()).unwrap();
        let snippet = dir.path().join("shell-integration.bash");
        let options = StartOptions {
            env: EnvNames {
                ipc: "ALICE_IPC",
                terminal: "ALICE_TERMINAL",
            },
            command_ring: 50,
        };
        let start = |id: u64, guarded: bool| {
            let mut env = vec![("TERM".to_string(), "xterm-256color".to_string())];
            if guarded {
                env.push(("ALICE_IPC".to_string(), "/socket".to_string()));
            }
            let spec = Spec {
                name: "bash".into(),
                command: "bash".into(),
                argv: vec![bash.clone().into(), "--norc".into(), "--noprofile".into(), "-i".into()],
                cwd: dir.path().to_path_buf(),
                env,
                start_gate: None,
            };
            let sink: Sink = Arc::new(|_: Event| {});
            Run::start(RunId(id), spec, 24, 80, sink, SCROLLBACK, options)
        };
        let wait = async |what: &str, cond: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cond() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };

        let run = start(1, true);
        wait("the shell", &|| run.status().is_running()).await;
        run.write(format!("source '{}'\n", snippet.display()).as_bytes());
        run.write(b"echo hi; (exit 3)\n");
        let seen = || run.commands().iter().any(|c| c.exit == Some(3));
        let t0 = Instant::now();
        while !seen() && t0.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(seen(), "{:?}\n{}", run.commands(), run.screen().screen().contents());

        // No ALICE_IPC: sourcing returns at once, so no hook runs and nothing is recorded; a later
        // command's output shows the shell took the input.
        let run = start(2, false);
        wait("the shell", &|| run.status().is_running()).await;
        run.write(format!("source '{}'\n", snippet.display()).as_bytes());
        run.write(b"echo hi; (exit 3)\n");
        run.write(b"echo done_$((1+1))\n");
        wait("the marker", &|| {
            run.screen().screen().contents().lines().any(|l| l == "done_2")
        })
        .await;
        assert!(run.commands().is_empty(), "{:?}", run.commands());
    }
}
