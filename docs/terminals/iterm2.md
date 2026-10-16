# iTerm2

Least certain of the five: iTerm2's Cmd shortcuts are macOS menu items, and its docs don't say how Cmd combos are forwarded once released. Use the checklist and report what you find.

Checked against:
- https://iterm2.com/documentation-preferences-profiles-keys.html (key reporting, Option keys, key binding actions)
- https://iterm2.com/documentation-csiu.html ("CSI u mode is no longer recommended… implement the Kitty key reporting protocol")

Version on this machine when written: iTerm2 3.6.10 (settings below not yet tried on it).

## Settings (no config file; done in the GUI)

**Settings → Profiles → (your profile) → Keys → General**
1. Turn on **Apps can change how keys are reported** ("apps running in the terminal can change the key reporting mode": this is what lets jed switch on the kitty protocol).
2. Leave **Report modifiers using CSI u** off (the docs say to use the kitty protocol instead).
3. **Left Option key** and **Right Option key**: **Esc+** (the docs: "For most users, selecting "+Esc" here is the right choice"). This is iTerm2's Option-as-Alt.
4. The **Left/Right Control/Command** reporting settings here only apply "when running a program that uses the Kitty keyboard protocol"; leave Command reported as Command.

## Releasing Cmd keys

iTerm2's Cmd shortcuts (Close Cmd+W, Quit Cmd+Q, Select All Cmd+A, tab selection Cmd+1..9, …) are app menu items, not terminal key bindings, so there's no "unbind" setting for them.

What can be done:
- **Menu items:** System Settings → Keyboard → Keyboard Shortcuts → App Shortcuts → `+`, application iTerm2, enter the exact menu item title (e.g. `Close`, `Quit iTerm2`, `Select All`) and give it a different shortcut (e.g. Cmd+Ctrl+Alt+W). That frees the original Cmd key from the menu.
- **iTerm2's own key bindings** (Settings → Keys → Key Bindings, and per profile): remove any default entries that use jed's Cmd keys (e.g. Cmd+Left/Right or Cmd+Alt+arrows for tab/pane navigation, if present in your version).

Leave `Paste` on Cmd+V: iTerm2's paste is how text gets into jed.

Not possible (as far as the docs say): releasing a Cmd key without remapping its menu item.

## What you lose

Whatever menu items you remap move to the new shortcuts in every iTerm2 window. iTerm2 profiles can't scope this to jed (menu shortcuts are app-wide).

## Checklist

- [ ] "Apps can change how keys are reported" on; jed's start-up warning about the kitty protocol is gone
- [ ] Left/Right Option set to Esc+; Alt+Left logs `ALT`
- [ ] Menu items for every jed Cmd key you need remapped (start with `Close`, `Quit iTerm2`, `Select All`, `Copy`, `Cut`, `Undo`, `Save`, tab selection 1–3, `Split Vertically with Current Profile` (Cmd+D, jed's add next match), `Use Transparency` (Cmd+U, jed's cursor undo; check your version's View menu), the split-pane selection items on Cmd+Alt+Up/Down (jed's add cursor above/below), whatever holds Cmd+P / Cmd+Shift+P, e.g. `Print`, `Find…` (Cmd+F, jed's find widget), `Find Cursor` (Cmd+/, jed's comment line), `Jump to Selection` (Cmd+J, jed's toggle panel) and `Set Mark` (Cmd+Shift+M, jed's toggle Problems))
- [ ] **Check by hand:** each key in the [table](README.md#cmd-bindings-jed-uses) logs with `SUPER`; note which ones iTerm2 still keeps
- [ ] Until verified, close jed's tabs with the mouse (×) and quit with Ctrl+Q
