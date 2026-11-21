---
id: jmux_touch_terminal
name: "jmux: touch gestures in a terminal"
summary: in the touch layout a finger swipe in a terminal scrolls its scrollback, a tap clicks, and a hold then drag selects text, as jed's touch layout does in a shell
status: done
priority: 0
dependencies: [jmux_touch_gestures, jed_touch]
---

See [jmux_touch_gestures](jmux_touch_gestures.md) (its Deferred line: the same gestures over a terminal) and [jed_touch](jed_touch.md) (subtask b, the gestures in jed's shells, which this copies).

With `--touch` a finger drag in a terminal selects text at once, as a mouse drag does, so there is no way to scroll back by finger in an SSH app that sends a swipe as a drag. jed's touch layout does it the phone's way; jmux does the same.

  - Needs: `jmux/src/app/touch.rs` (`touch_press`, `hold`, `press_moved`, `press_released`), `jmux/src/app.rs` (`pointer_event`'s `Hit::Terminal` arm, `terminal_mouse`, `select_press`, `Drag::Press`).
  - A left press without modifiers on a terminal's text in the touch layout is a pending `Drag::Press` (holding the press event) with the usual `HOLD` timer. A swipe scrolls the scrollback by the rows moved, the text following the finger; a program that reports the mouse (and the view not scrolled back) gets one wheel event per row at the press instead. A tap replays the press and release: a click to such a program, else an empty selection the release drops; double / triple tap select a word / line. A hold replays the press, so a drag from there selects as a mouse drag does (or is reported to the program). A modifier (a hardware keyboard's Ctrl+tap on a link, Shift to extend) acts as a click. A plain client is unchanged.
  - Done when: with `--touch`, a swipe in a terminal scrolls its scrollback and selects nothing; a hold then drag selects and a tap clears it; a double tap selects a word; a tap focuses another group; a program that reports the mouse gets wheel events from a swipe and a click from a tap; a plain client's press still selects at once; `scripts/check` is green. Tests in `jmux/src/jmux_touch_terminal_tests.rs` (`jmux_touch_terminal_*`).
  - Docs: `jmux/README.md` (the `--touch` row), `jmux/PLAN.md` (the touch gestures line), [crates](../docs/dev/crates.md) (`app/touch`).
- ✅ Done. `terminal_pointer` is the `Hit::Terminal` arm (focus, link click, `terminal_mouse`), which a terminal's tap and hold replay; `reports_mouse(group)` picks wheel events or the scrollback for a swipe; `Drag::Press` holds the press `event`.
  - Deferred: a hold released without moving does nothing (jmux has no context menu to open; jed opens nothing in a shell either); horizontal swipes are ignored; `HOLD` / `SLOP` as settings.
  - Still to check by hand: a phone's SSH app: whether a long press reaches jmux as a still press or the app takes it for its own selection menu; a swipe in `less` and in vim.
