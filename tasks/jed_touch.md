---
id: jed_touch
name: Touch layout
summary: "`jed --touch` (or `touch_layout = true`) draws taller, wider tab bars with bigger close and scroll targets for a touch screen, using jmux's touch metrics"
status: open
priority: 0
dependencies: [jmux_touch]
---

jed's version of [jmux_touch](jmux_touch.md). It pads every group's tab bar (editor groups and the terminal area's) so a finger can hit a tab, its `×` and the scroll arrows. jed has no attach, so the layout is chosen when jed starts: `--touch` on the command line, or `touch_layout = true` in `settings.toml` for a machine that always has a touch screen. The setting applies on Reload Settings.

  - Needs: `jmux_touch` (`jtui::tabs::TabMetrics`, `NORMAL` / `TOUCH`, and the rules for which rows and cells a tap hits); `jed/src/main.rs` (args, `USAGE`), settings ([adding a setting](../docs/dev/conventions.md#settings): `jed/src/config/settings.rs`, `edit::Settings`, the sample file), `jed/src/ui/mod.rs` (each group's `tab_bar` / `editor_area` split, with `tabs_at_bottom`), `jed/src/ui/tabs.rs` (labels, `●` / `×`, the group number, `‹` / `›`, the drop marker), `jed/src/layout.rs` (`TabHit`, `TabArrows`, the group's `tab_bar` rect and the code that uses it), tab drag (`drag_tabs`), and the places that work out an editor's or shell's size before a frame is drawn
  - The switch: `--touch` sets it for this run and wins over the setting. A file opened through the socket into a running jed doesn't change it. `touch_layout` (bool, default `false`) in `[settings]`, with no per-language override.
  - Layout: every tab bar reads `TabMetrics` from one helper (`App::tab_metrics()`) in place of the hard-coded 1 row. Do the same as jmux: 3 rows with the label on the middle one; 2 cells of padding at each end and around `×` (or `●` on an unsaved tab, which still turns into `×` on hover); a 3-cell close target; 3-cell `‹` / `›`; the 3-cell `+` at the bar's right end (`TabMetrics::new_tab`) opening a tab in that group; the drop marker on all 3 rows. The group number sits on the label row. The bar's height comes from one function used by the frame and by whatever sizes editors and shells, so a shell's PTY size, an editor's visible lines and the scroll bars agree with what's drawn.
  - Mouse: `TabHit` gets a close range, as jmux's. Clicks, middle-click close, the context menu, the wheel on the bar and drag already go through `tab_bar.contains(p)`; check that each works on the pad rows. The tab tooltip (`ui/tab_tip.rs`) is placed below or above the taller bar.
  - Not in this task: the activity bar, side bar rows, the status bar, the editor's text, menus, the palette.
  - Done when: with `--touch` or the setting, every group's tab bar (editor and terminal area, top and bottom) is 3 rows with padded tabs; tapping any row of a tab focuses it, the close range closes it (with the unsaved-changes prompt as today), the arrows scroll an overflowing bar, and a tab drags to another group and onto a split edge; editors and shells are sized to the area left over; turning the setting on or off and running Reload Settings redraws without a restart; with neither set nothing changes; `scripts/check` is green. Tests in `jed/src/todo_tests.rs` (`mod jed_touch`, tests named `jed_touch_*`): the bars drawn on a `TestBackend`, hits on every row and each close cell, the arrows, a drag, a shell's size, the setting's reload; `parse` tests in `main.rs`.
  - Docs: the setting in `config/settings.rs`'s `//!` table and the sample `settings.toml`, `--touch` in `USAGE` and the README, PLAN.md's Features line for tabs.
  - Deferred: taking the layout on from jmux automatically (an env variable jmux's `--touch` client sets for new shells; it would need a way to reach shells already running); taller rows in the explorer, Source Control and the palette.
  - Still to check by hand: jed with `--touch` on a touch laptop's terminal and through a tablet's SSH app, editor and terminal tabs; drag between groups by finger.
