---
id: milestone_12
name: Tasks
summary: templates, PTY runner, task tabs in the bottom panel
status: done
priority: 0
dependencies: []
---

    - Needs: workspace trust (M2), the palette (M5), the bottom panel (M9)
    - Templates from `.workspace` `[tasks]` and `~/.config/jed/tasks.toml`, workspace wins on a clash
    - "Run Task" from the palette; one tab per run; PTY via `portable-pty` + `vt100` + `tui-term`
    - Exit code shown, output kept until the tab closes; kill a running task
    - Done when: a colourful build (e.g. `cargo build`) renders as in a real terminal; restricted mode runs no `.workspace` task
    - Tasks (one agent and one commit each):
      - ✅ 12a: `tasks` module, no UI: templates from `.workspace` `[tasks]` and `~/.config/jed/tasks.toml` (workspace wins on a clash; restricted / undecided trust: no `.workspace` tasks), cwd / env resolution; the PTY runner (`portable-pty` + `vt100`): spawn off the UI thread, output parsed into a screen with scrollback, resize, exit code, kill; results reported through `AppEvent`s
        - `tasks/templates.rs`: `tasks.toml` uses `.workspace`'s `[tasks.<name>]` tables, read once at startup in `main` (lenient like `settings.toml`); `merge` (by name, workspace wins, `Source`), cwd (root / relative / `~`), env (`TERM=xterm-256color`, `COLORTERM=truecolor` unless set); `App::task_templates()` takes `.workspace` tasks only when `Trusted`
        - `tasks/run.rs`: `Run::start` (openpty + spawn in `spawn_blocking`, `$SHELL -c`, falling back to `/bin/sh`), a reader thread into a `vt100::Parser` (5000 lines of scrollback) behind a mutex, a `waitid(WNOWAIT)` waiter, `Event::{Started, Output (≤ one per 20 ms), Ended}`; `Status::{Starting, Running, Exited(Exit { code, signal, killed }), Failed}`; `resize`, `kill` (SIGHUP + SIGCONT to the group, SIGKILL after 1 s, also to leftovers holding the PTY), `force_kill`; dropping a `Run` kills it
        - `app.tasks` (`Tasks`: `run`, `get`, `close`, `size` 24×80 until 12b, `shutdown`); `AppEvent::Task`; `App::shutdown` stops servers and tasks together on quit
        - Not done (12b): input to the PTY, answers to terminal queries, `size` from the panel; daemons that `setsid` and keep the PTY keep its reader alive
      - ✅ 12b: "Run Task" in the palette (a picker of templates); one bottom-panel tab per run (`PanelTab` for task runs, drawn with `tui-term`, Dark+ colours), PTY resized to the panel, exit code shown when it ends, output kept until the tab closes; kill a running task; close a run's tab (killing it if running); keys while the panel has focus
        - `task.run` (palette, unbound: name, command, source; no templates / restricted mode → a message naming `.workspace` and `tasks.toml`), `task.kill` (the shown tab's run, else the newest running), `task.close` (Cmd+W in a task tab, the tab's `×`, middle click; kills if running)
        - `PanelTab::Task(id)` after Problems / Locations, marks `●` / `✓` / `✗ 2` / `✗ killed`; drawn with `tui-term` through `Theme::terminal_cell` (`terminal.*` keys, Dark+'s ANSI palette, bold → bright, RGB → 256 without truecolor); PTYs resized to the panel at draw time (only on change); an end note under the output and in the status bar
        - `Context::Task` (`task`): Shift+PageUp / Down, Ctrl+Shift+Up / Down, Ctrl+Home / End scroll the scrollback, Esc back to the editor; every other key and pastes go to the PTY as xterm encodes them (application cursor, bracketed paste), through a writer thread (never blocks the UI); global keys stay jed's
        - Not done: answers to terminal queries; keys typed while a run is still starting are dropped; Esc never reaches a task; no spinner; the tab strip doesn't scroll; ended runs aren't resized; no mouse selection / copy in a task
      - ✅ 12c: the done-when tests (a colourful build renders its colours and progress as in a real terminal; restricted mode runs no `.workspace` task), remaining gaps and hand checks; marks M12 ✅
        - Input written while a run is still starting is queued (`State::queued`) and sent first once it spawns; dropped if the start fails
        - Done-when tests: a real `cargo build --color=always` of a temp crate (own `CARGO_HOME` / target dir, cargo's env pinned) run from the picker, its drawn panel compared row by row (text and colours) with an independent terminal model fed a piped build, plus named colour checks (bold bright green Compiling / Finished, bright yellow warning) and the progress bar seen while running; a `\r` / `ESC[K` / cursor-movement progress script; restricted (dialog and stored) and undecided trust run no `.workspace` task (its marker never appears, `run_task` refuses, the user's same-named task runs instead), then trusting runs it
    - Deferred: answers to terminal queries (interactive tools like `htop` / `less` may misbehave); Esc never reaches a task; no spinner; the task tab strip doesn't scroll; ended runs aren't resized; no mouse selection / copy in a task tab; daemons that `setsid` and keep the PTY keep its reader alive; no Run Task key (still TBD); compiler errors into Problems (later)
    - Still to check by hand: `cargo build` of a real project with dependencies in kitty, iTerm2 and tmux (colours, progress bar, wide output wrapping); truecolor vs 256 colours; typing into a task right after picking it; Shift+PageUp / Down and Ctrl+Shift+Up / Down not grabbed by the terminal; Ctrl+C and a y/n prompt in a task; kill / close of a long build; quitting with a build running leaves no processes; the restricted-mode hint and running after Manage Workspace Trust; macOS `waitid` and process-group kills; a fish `$SHELL`
