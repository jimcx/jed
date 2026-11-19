---
id: fake_shell
name: Fake shell for tests
summary: a small test-only shell (`fakesh`) with a fixed prompt and the few commands the tests type, so jed's and jmux's terminal tests stop depending on whether `/bin/sh` is bash or dash, the user is root, or readline is ready
status: done
priority: 0
dependencies: []
---

About 23 test files (jed's `shell`, `term_*`, `clear_scrollback`, `todo`, `claude_ide`, …; jmux's `jmux_server`, `jmux_commands`, `jmux_layout`, `jmux_tabs_at_bottom`, `jmux_touch`, `jmux_config`, `command_driven`) start a real `/bin/sh` and read its screen. They break on things that are the shell's business, not jed's or jmux's: the prompt is `$ `, `sh-5.3$ `, or `#` as root (6e95e48 fixed three jmux tests that timed out in a root CI container); bash's readline turns bracketed paste on only once it draws its prompt, so a paste before it races (80df21f); macOS's `/bin/sh` is bash, Linux's often dash, and Ctrl+C draws a new prompt on one and echoes `^C` on the other. What these tests check is jed's and jmux's part: bytes from a key, paste or command reach the PTY, the output reaches the screen, the program's modes (bracketed paste, application cursor) are honoured, prompt marks feed the command list, the process ending closes the tab. A fake shell with fixed behaviour checks the same and runs the same on every machine and user.

Most of what looks like shell behaviour is the terminal line discipline's (the kernel's), so a fake in cooked mode gets it for free: echo of typeahead, Backspace and Ctrl+U, Ctrl+D at an empty line as end of input, Ctrl+C as SIGINT to the foreground process group (the PTY spawn makes the shell session leader). Only bracketed paste is readline's, and the fake turns it on itself.

  - Needs: `jterm::testing` (the prompt helpers, `shows_prompt` / `ends_with_prompt` / `after_prompt` / `line_is`), `jterm::run` / `shell::spec` (how a shell is spawned), `tasks.shell` and `App::open_shell` (jed's shell tabs read `tasks.shell` in tests), jmux's `app::Config::program` and the harnesses (`jmux/src/test_harness.rs`, `jmux_server_tests`' `TestServer` / `sh_pinned_exe` / `jmux_exe()`), `jed/src/shell_tests.rs` (`shell_script`, `dirs`)
  - The program (`fakesh`):
    - a binary target built by `cargo test --workspace` and found by the tests the way `jmux_server_tests::jmux_exe()` finds `jmux` (next to the test executable), via `jterm::testing::fake_shell() -> PathBuf`, which asserts it exists with a message saying how to build it. Where it lives is the first decision: a tiny workspace crate (`crates/fakesh`, std + libc only) is the likely answer; check that `scripts/check test <filter>` builds it too, or make `fake_shell()` say what to run
    - prints `ESC[?2004h` (bracketed paste on), then its prompt `$ ` (fixed, root or not), and reads lines in cooked mode. A line's `ESC[200~` / `ESC[201~` markers are removed and remembered, so a test can ask whether the last line came as a bracketed paste (e.g. `pasted` prints `yes` / `no`)
    - SIGINT: a new line and a fresh prompt (the line is dropped). End of input at an empty line: exits 0. `exit [n]` exits with `n`
    - the commands the migrated tests type, and no more: find them with a grep of the `type_text` / `write` / `paste` calls in the files above. Today that is `echo`, `printf` (`%s`, `%d`, `\n`, `\r`, `\033` / `\e`, octal escapes: the jmux command-list tests print OSC 133 / 633 marks with it), `pwd`, `cd`, `seq`, `stty size` (the PTY's size), `sleep`, `$VAR` / `"$VAR"` expansion and `$((a+b))`. Words split on spaces with `'…'` and `"…"` quoting; `;` runs two commands. An unknown command prints `fakesh: <cmd>: not found` and sets `$?` to 127
    - `kill -TERM $$` and other signal tests keep their `shell_script` scripts (they test the program, not the shell)
    - its own unit tests (parsing, escapes, expansion) in the crate, and one PTY test of the prompt, a paste, Ctrl+C and Ctrl+D through `jterm::run`
  - Migration: every test above that starts `/bin/sh` (`tasks.shell = "/bin/sh"`, `Harness::new("/bin/sh", ..)`, `program: "/bin/sh"`, `SHELL=/bin/sh` in `sh_pinned_exe`) starts `fake_shell()` instead, except:
    - tests of the shell integration snippets and `jmux install-shell-integration` (they source a real init file; the `zsh` symlinks to `/bin/sh` in `jmux_commands_tests`)
    - one or two smoke tests per app that start the real `/bin/sh` and check a prompt appears and `echo` runs, so PTY setup, the environment and session leadership are still checked against a real shell
    - each kept real-shell test says why in a comment, and the file's `//!` doc lists them
  - Then simplify what the fake makes fixed: waits for "a prompt, `$` or `#`" become waits for `$ `; comments about bash vs dash, root, readline's bracketed paste and typeahead races go; `jmux_server_pastes_into_the_shell` checks that the paste arrived bracketed (from `pasted`) instead of only that it ran. Keep `jterm::testing`'s prompt helpers for the real-shell tests
  - Docs: `docs/dev/testing.md` (a section: terminal tests run `fake_shell()`; which tests keep a real shell and why; how to add a command to the fake), the `crates/` line in `docs/dev/crates.md`, the new crate's line in the root `CLAUDE.md`'s workspace sentence if it gets one
  - Done when:
    - `scripts/check` passes as a normal user, as root (`unshare -r scripts/check` on Linux), with `/bin/sh` as dash and as bash, and on macOS CI
    - no migrated test mentions `sh-5.3`, `#` prompts or readline; the only `/bin/sh` shells left are the listed real-shell tests
    - `jmux_server_pastes_into_the_shell` fails if jmux stops wrapping a paste when the program asked for bracketed paste
    - adding a command a test needs means one match arm in `fakesh` and a unit test there
  - Subtasks (one agent and one commit each):
    - ✅ fake_shell-a: the `fakesh` program and its tests, `jterm::testing::fake_shell()`, the docs; migrate jmux's tests (server, commands, layout, tabs at bottom, touch, config, command-driven) and keep its real-shell smoke test
      - `crates/fakesh` (std + libc; `Shell::command` in `src/shell.rs` is the match arm per command; also `read [VAR]`, `pasted`, `true`/`false`/`:`, `NAME=value`, `${V-x}` / `${V+x}`); built by `cargo test --workspace` because it has an integration test (`tests/pty.rs`); `jterm::testing::fake_shell()` and `FAKE_PROMPT`
      - `pasted` reports the line before it; SIGINT is blocked around the flag check and waited for in `pselect`, so none is lost
      - jmux tab titles are now `fakesh`; tests needing raw-mode programs (`stty raw`, `dd`, `vi`) run a `#!/bin/sh` script as the tab's program (`Harness::with_program`) instead of typing into a shell
      - real `/bin/sh` left in jmux: `jmux_server_runs_a_real_shell` and the zsh-symlink integration tests; `jmux/tests/cli.rs` keeps `SHELL=/bin/sh` (it never types)
      - for b: `COUNT_TO_200`'s `while` loop → `seq`; `cat` / `cat -v` / `more` and the alt-screen `printf …; read x` lines need a `shell_script` program or a check; jed tab titles become `fakesh ○`
    - ✅ fake_shell-b: migrate jed's terminal tests (`shell`, `term_area`, `term_link`, `term_search`, `term_select`, `clear_scrollback`, `drag_tabs`, `git_marks`, `claude_ide`, `todo_tests`' shell ones, `command_driven`, `app/session.rs`), keep its real-shell smoke test, run the root and dash/bash checks, mark the task ✅
      - jed's terminal tests run `fake_shell()`; `COUNT_TO_200` is `seq 0 199`; titles and notes say `fakesh`; `fakesh -c LINE` (task commands run through `tasks.shell`); `clear_scrollback`'s `cat -v` test runs a `shell_script` program; new `shell_paste_reaches_the_fake_shell_bracketed`
      - real `/bin/sh` left in jed: `shell_runs_a_real_shell` (smoke, via `shell_tests::real_shell`), the jed / git runs from a shell (`shell_jed_command_opens_the_file_in_this_jed`, `shell_git_commit_opens_its_message_here`: the fake never forks), `claude_ide_new_session_runs_claude_in_a_jed_shell` (a stand-in `claude` on `PATH`), the bash / zsh integration-snippet tests
  - ✅ Task done. What it changed: jed's and jmux's terminal tests run `crates/fakesh` (fixed `$ ` prompt, bracketed paste on from the start, cooked mode) instead of `/bin/sh`; only the listed real-shell tests still depend on the system shell.
    - Also: the paste tests check the paste arrived bracketed (`pasted`); a lost-SIGINT race in the fake was fixed with `pselect`; raw-mode programs (`stty raw`, `dd`, `vi`, `cat -v`) run as the tab's program from a script.
    - Deferred: the done-when run with `/bin/sh` as dash (not installed on the build host; skipped at the user's request); the two jed-binary real-shell tests skip unless `target/<profile>/jed` was built beforehand (pre-existing).
    - Still to check by hand: macOS CI passes.
  - Deferred:
    - a fake for other programs the tests start (`git` as an editor, language servers), if they turn out to cause the same kind of flake
