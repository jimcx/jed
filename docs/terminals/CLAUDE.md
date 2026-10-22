# docs/terminals

Per-terminal config that releases jed's macOS Cmd keys and turns Option into Alt. Background: PLAN.md [Terminals](../../PLAN.md#terminals).

- `README.md` has the table **"Cmd bindings jed uses"**. The test `terminal_docs_list_every_cmd_binding` (`src/config/keymap.rs`) parses rows starting with `` | `cmd+ `` and compares them, as a set, with every SUPER binding in `Keymap::default_for(Platform::MacOs)` (all contexts). Missing or stale rows fail `cargo test`.
- Shifted digits are spelled as the digit + `shift` (`cmd+shift+1`): terminals report Cmd+`!`, which `KeyCombo::from_event` maps back.
- Spelling in the first column must match `KeyCombo::spec()` (the `keybindings.toml` spelling too): modifiers in order `cmd`, `ctrl`, `alt`, `shift`; lowercase key names (`left`, `backspace`, `f2`); `backslash` for `\`, `space` for ` `. One row per combo — a combo bound in two contexts is one row with both in "Where" (e.g. `cmd+down`, `cmd+backspace`).
- The test only checks the README. Each snippet must be updated by hand, in its own syntax, keeping the README's order:
  - `kitty.md` — `map --when-focus-on var:jed cmd+x` (no action = pass through while jed has focus: jed sets the `jed` user var; never `discard_event`)
  - `ghostty.md` — `keybind = cmd+arrow_left=unbind`
  - `wezterm.md` — entries in the Lua `release` table, `{ 'LeftArrow', 'CMD|SHIFT' }` (also turns on `enable_kitty_keyboard`)
  - `alacritty.md` — `{ key = "Left", mods = "Command|Shift", action = "ReceiveChar" }`
  - `iterm2.md` — no snippet: GUI steps to remap menu items; update its checklist if a key needs a new menu item freed
- Each terminal file also has a "defaults that clash with jed" note, a "What you lose" section and a "Checklist"; update them when a new key clashes with a terminal default.
- Never bind or release `cmd+v`: the terminal's paste reaches jed as a bracketed paste (`Event::Paste`).
