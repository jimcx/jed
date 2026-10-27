---
id: shell_title
name: Shell tab shows the foreground process
summary: the tab name follows what runs in the shell, as tmux's `automatic-rename`
status: done
priority: 0
dependencies: []
---

  - `tasks/run.rs` — `Event::Foreground { id, name, idle }` and `Run::watch_foreground(sink)`: a tokio task polling every `FOREGROUND_POLL` (1 s), reporting only a change, ending when the run ends. The foreground process group: Linux `/proc/<pid>/stat`'s `tpgid` (the field after the parenthesised `comm`, via `rfind(')')` so a `)` in the name is fine) and the name from `/proc/<tpgid>/comm`; macOS `proc_pidinfo(PROC_PIDTBSDINFO)`'s `e_tpgid` and `proc_name`. `tpgid == <pid>` is idle
  - `shell.rs` — `ShellTab` gains `number` and `idle`; `title(base, number, foreground)` builds `zsh ○` at the prompt, `make ●` while a command runs, `zsh (2) ○` for the next shell (glyphs `RUNNING_GLYPH` / `IDLE_GLYPH`); the idle title always uses `base`
  - `app/shell.rs` — numbering uses the stable `ShellTab.number` (the smallest-free-number rule); `shell_event` renames on `Foreground` via `Groups::shell_tabs_mut`; exit notes still name the shell by `base`
  - `ui/tabs.rs` / `theme.rs` — the trailing glyph drawn in `tab.shellRunning` / `tab.shellIdle` theme keys
  - Tests: the unit title builder (`titles_follow_the_foreground_process_and_its_glyph`), `tasks/run.rs`'s `tpgid_is_the_field_after_the_parenthesised_comm`, and `shell_tests.rs` (`shell_title_follows_the_foreground_process`, `shell_running_titles_keep_the_per_program_number`) over a real PTY
  - Deferred: none
  - Still to check by hand: the macOS `proc_pidinfo` path compiles only on macOS (the names / signatures were checked against libc); a real terminal with zsh / vim / less
  - Done when: a shell at its prompt shows `zsh` (idle glyph); `sleep 30` turns the tab to `sleep` (running glyph) at once and back on return; `vim` / `less` do the same; two shells of one program keep distinct `(2)` titles; the title returns to the shell's when a command exits without printing (a prompt-less `cat`); tests cover the pure title builder and the real change over a PTY, on Linux and macOS
