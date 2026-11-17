---
id: jmux_touch
name: "jmux: touch layout"
summary: "`jmux attach --touch` (also `jmux --touch` and `jmux new --touch`) draws taller, wider tab bars with bigger close and scroll targets for a touch screen, for as long as that client stays attached"
status: done
priority: 0
dependencies: [jmux_config, jmux_tabs_at_bottom]
---

See [jmux's plan](../jmux/PLAN.md): [Attach and detach](../jmux/PLAN.md#attach-and-detach), [Terminals, tabs and splits](../jmux/PLAN.md#terminals-tabs-and-splits).

On a touch-screen device (a tablet or phone running an SSH terminal app, or a touch laptop) a one-row tab bar with a one-cell `×` is hard to hit with a finger. The touch layout pads the tabs. It belongs to the client, not the session: the same session is used from a desktop terminal and later from a tablet. So it is chosen when attaching, not in `settings.toml`, and it lasts until that client detaches or another client takes the session over.

  - Needs: `jmux_config`, `jmux_tabs_at_bottom` (`ui::bar_and_term`, the one place the bar's rect is worked out); `jmux/src/cli.rs` (`Cmd`, `parse`, `USAGE`), `jmux/src/main.rs` (attach paths), `jmux/src/protocol.rs` (`ClientMsg::Hello`), `jmux/src/client.rs` (builds `Hello`), `jmux/src/app.rs` (`hello`, `term_size`), `jmux/src/ui.rs` (`TAB_ROWS`, `bar_and_term`), `jmux/src/tabbar.rs` (tab widths, `×`, `‹` / `›`), `jmux/src/hit.rs` (`TabHit`, `TabArrows`, the group's `bar` rect), `jmux/src/tab_drag.rs` (drop gap)
  - The argument: `--touch` on `jmux attach`, `jmux new` (it attaches too) and bare `jmux`; refused on `ls`, `kill-session`, `rename-session` and `install-shell-integration`. `USAGE` and `jmux/README.md` say what it does.
  - Protocol: `Hello` gains `touch: bool` with `#[serde(default)]`. `PROTOCOL_VERSION` stays 1, so an upgraded client can still attach to a session started by the old server. That old server ignores the field and draws the normal layout. Write this down in `protocol.rs` (version 1 = `touch` optional).
  - The server keeps `touch` per attached client (`App::touch`, set in `hello`, cleared on detach / takeover). A takeover by a client without `--touch` goes back to the normal layout on its first frame.
  - Layout, from one value. Add `jtui::tabs::TabMetrics { rows, pad, close, arrow }` with `NORMAL` (1 row, today's ` title × `, 1-cell targets) and `TOUCH`. It lives in `jtui` so jed's touch layout (`jed_touch`) uses the same numbers. `TOUCH` to start with:
    - the bar is 3 rows: the title on the middle row, the rows above and below filled with the tab's own background, so the whole 3-row block reads as one tab
    - each tab is `  title  ×  `: 2 cells of padding at each end and around `×`
    - the close target is 3 cells wide (`×` and the cell each side of it) across all 3 rows. The rest of the tab focuses it. A drag starts anywhere on the tab
    - `‹` / `›` are 3 cells wide when the tabs overflow
    - the drop marker of a dragged tab spans all 3 rows
  - `ui::bar_and_term` takes the metrics in place of `TAB_ROWS`. The bar is `rows` tall (fewer in a group shorter than that), at the top or the bottom per `tabs_at_bottom`. `App::term_size` reads the same function, so a new shell's size and the frame agree. Attaching with `--touch` takes 2 rows from each group's terminal, and the shells get a resize, as they do for any attach at a new size.
  - Mouse: `tabbar::render` returns `TabHit` with the close range (`close_x0..close_x1`) instead of one `close_x`, and `TabArrows` with ranges. Tab clicks, the wheel on the bar and `tab_drag` already test `g.bar.contains(at)`, so the taller bar needs no change of its own there. Check that clicks on the top and bottom pad rows hit the tab.
  - Not in this task (see Deferred): the side bar's rows, dividers, the status bar, and anything inside a terminal.
  - Done when: `jmux attach --touch` (and `jmux --touch`, `jmux new --touch`) draws 3-row bars with padded tabs, at the top and with `tabs_at_bottom`; tapping any row of a tab focuses it, tapping within a cell of `×` closes it (with the running-command confirmation as today), the 3-cell arrows scroll an overflowing bar, and dragging a tab to another bar or to a terminal half works; the shells are 2 rows shorter and are told so; a plain `jmux attach` taking the session over draws the normal layout again; an old client's `Hello` without `touch` is accepted; `--touch` on `ls` is refused; `scripts/check` is green. Tests in `jmux/src/jmux_touch_tests.rs`, named `jmux_touch_*` (a `//!` doc, a `#[cfg(test)] mod` line in `main.rs`): the bar drawn on a `TestBackend` at both positions, hits on every row and on each close cell, the arrows, a drag, the takeover, and the shell's size. Unit tests for `parse` in `cli.rs` and for `Hello` without `touch` in `protocol.rs`.
  - Docs: `jmux/README.md` (the flag, under attaching), `jmux/PLAN.md` (`--touch` in Sessions and in the `Hello` line), [crates](../docs/dev/crates.md) (`jtui::tabs`).
  - Deferred (unless it turns out cheap): a `touch` setting or `$JMUX_TOUCH` to make it the default for one machine's client; a command that turns it on and off while attached; taller side bar rows and a wider divider grab area; passing the touch layout on to a jed started in a jmux terminal (`jed_touch` can read an env variable jmux sets).
  - Still to check by hand: attach with `--touch` from a tablet's SSH app (e.g. Termius or Blink) and from a phone in portrait; whether a tap arrives as a click, and a finger drag as a mouse drag, in each app; whether 3 rows and 2 cells of padding are enough there.
- ✅ Done. `jmux attach --touch`, `jmux new --touch` and `jmux --touch` draw 3-row tab bars with `  title  ×  ` tabs, 3-cell close targets on every row and 3-cell scroll arrows, for as long as that client stays attached.
  - Layout comes from `jtui::tabs::TabMetrics` (`NORMAL`, `TOUCH`; helpers `tab_width`, `text_row`, `close_reach`, `arrow_reach`), which `jed_touch` can reuse. `ui::bar_and_term` / `term_area` take the metrics (the bar is clamped to the group's height); `App::tab_metrics()` picks them from `App::touch`, set by each `Hello` and cleared on detach or a dropped connection. `TabHit` has `close_x0..close_x1`; `TabArrows` holds `Range<u16>`s (no longer `Copy`).
  - Also: `--touch` rides on `client::Terminal.touch`, so `client::attach` keeps its signature; `--touch` as the first argument is accepted only alone (`jmux --touch attach` is an error, `jmux attach --touch` isn't).
  - Deferred: as listed above (a `touch` setting / `$JMUX_TOUCH`, a toggle while attached, taller side bar rows and divider grabs, passing it to jed).
  - Still to check by hand: as listed above (a tablet's and a phone's SSH app: taps as clicks, finger drags as drags, whether 3 rows and 2 cells of padding are enough).
