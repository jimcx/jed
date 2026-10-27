---
id: pty
name: PTY smoke tests
summary: end-to-end tests running the real binary in a pseudo-terminal
status: open
priority: 0
dependencies: []
---

A few end-to-end tests that run the real `jed` binary (`env!("CARGO_BIN_EXE_jed")`) in a pseudo-terminal (`portable-pty`, already a dependency) and parse what it writes with `vt100` (as task runs do), so the bytes crossterm emits and the bytes a terminal sends are tested, not just `TestBackend`: (1) after start the vt100 screen equals the `TestBackend` frame of the same state (text and colours, cell by cell; also after an edit and a scroll, to catch stale cells from ratatui's diff flush); (2) real input sequences reach the right commands — legacy and kitty-protocol (`CSI u`) keys incl. `Cmd+!` / Ctrl+@, bracketed paste, SGR mouse clicks and wheel; (3) start / quit leaves the terminal as it was (raw mode off, alt screen left, mouse / bracketed paste / kitty flags popped: check the mode-setting sequences in the output and the PTY's termios), and so does a forced panic (a test-only `JED_TEST_PANIC` hook or similar), which also leaves its crash backup; (4) wide graphemes (CJK, emoji, combining marks) land in the columns `pos::display_grapheme` says. Isolated `XDG_STATE_HOME` / `XDG_CONFIG_HOME` per test (they're child processes, so setting them is fine), a fixed size (e.g. 100×30), waits on screen contents with a timeout (no sleeps), `TERM=xterm-256color` and `COLORTERM=truecolor`; skip with a note where PTYs aren't available. Not a cross-emulator matrix: vt100 is one emulator; termless (TypeScript; xterm.js / Ghostty backends) or esctest could be added later if wide-character or protocol differences between terminals become a real problem

  - Needs: `portable-pty` and `vt100` (task runs), the panic hook and crash backups (M13), `terminal.rs`'s mode sequences
  - Done when: the four kinds of test above pass on Linux and macOS and are skipped with a note where PTYs aren't available
  - Subtasks (one agent and one commit each):
    - pty-a: the harness (spawn the binary in a PTY with isolated dirs, a vt100 parser, wait-for-screen with a timeout, the skip) and test (1), screen equals the `TestBackend` frame, plus (4), wide graphemes
    - pty-b: test (2), real input sequences: legacy and kitty keys, bracketed paste, SGR mouse
    - pty-c: test (3), the terminal restored on quit and on a forced panic (the test-only panic hook, the crash backup); gaps and hand checks; marks the task ✅
