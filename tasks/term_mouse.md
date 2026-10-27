---
id: term_mouse
name: Mouse in the hosted terminal
summary: forward xterm mouse events to a shell or task tab's program
status: done
priority: 0
dependencies: []
---

  - `tasks/mouse.rs` (new) — `mouse::encode(event, mode, encoding)`, the inverse of `keys::encode`: a `MouseEvent` plus the screen's `mouse_protocol_mode()` / `mouse_protocol_encoding()` (the `vt100` crate already tracks 1000 / 1002 / 1003 and 1005 / 1006) → the bytes `Run::write` takes. SGR (1006) `CSI < b ; x ; y M` / release `m`, motion code 32 (1002) / 35 (1003), the wheel as buttons 64–67, Shift / Alt / Ctrl +4 / +8 / +16; X10 (1000) and UTF-8 (1005) `CSI M` + three bytes, press-only in `Press`, release code 3 in `PressRelease`, coordinates clamped to 223; `None` while the program hasn't asked for the mouse
  - `app/mouse.rs` — `terminal_run` (a shell tab `Hit::Editor(g)` with an active shell → its `run`; a task tab `Hit::Panel` + `PanelTab::Task` → that run) and `terminal_mouse`, called before jed's click / wheel handling: forwards through `Run::write`, coordinates `x − area.x` / `y − area.y` clamped, only on the live screen (`scrollback == 0`) and not for an ended run or while a jed drag is in progress; a left press still focuses; returns true so jed's handling stays out. With the mouse off the wheel still scrolls the scrollback
  - Tests: encoder unit tests (`sgr_press_and_release`, `sgr_motion_and_wheel`, `sgr_modifier_bits`, `x10_bytes`) and `todo_tests::term_mouse` end to end (a `/bin/cat` echo of the exact SGR bytes over a shell and a task tab, nothing while scrolled back, mouse-off changes nothing)
  - Also: `shell_tests`' helpers are `pub(crate)` so the task-tab test reuses them; a drag is only reported in 1002 / 1003, so the shell test enables `1000+1002+1006` to exercise one
  - Deferred: none
  - Still to check by hand: the wheel's up / down button sense was scripted against `vim` / `tmux` (64 = up, 65 = down) but not in a real terminal
  - Done when: `/bin/sh` + `printf '\033[?1000h\033[?1006h'; cat` echoes the exact SGR bytes for a click, drag and wheel over a shell and a task tab; with the mouse off clicks and the wheel change nothing (the wheel still scrolls); the encoder's unit tests cover SGR and X10 press / release / motion / wheel and the modifier bits
