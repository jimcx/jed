---
id: terminal_identity
name: Terminal window identity
summary: the window title, kitty user vars and OSC 7
status: done
priority: 0
dependencies: []
---

  - `terminal::Identity` (built in `main`, passed to `terminal::enter`): on start, after the alt screen, pushes the title stack (`CSI 22;0t`) and sets the title (OSC 2) to `jed <root>` (`jed <file>` in single file mode; `~` for home, control characters stripped, not wrapped for tmux); OSC 1337 `SetUserVar` `jed=1` always and `jed_workspace=<base64 root bytes>` in workspace mode (through `passthrough`); OSC 7 `file://<host><percent-encoded root>` in workspace mode. `finish` (so `restore` and the panic hook) clears both vars and pops the title, gated by `IDENTITY_SET`
  - `docs/terminals/kitty.md`: the snippet's maps are `map --when-focus-on var:jed cmd+x`, so the Cmd keys go to jed only while a jed window has focus (`--when-focus-on` values are kitty search expressions; `cmdline:` only sees the window's launch command)
  - Also: `clipboard::base64` is `pub(crate)`; a non-UTF-8 root is sent as its bytes
  - Deferred: user-var notes in the WezTerm / iTerm2 docs; kitty desktop notifications (OSC 99) for finished tasks
  - Still to check by hand: in kitty the title reads `jed ~/…` and the shell's comes back after quitting; `kitten @ ls` shows `jed` / `jed_workspace` while jed runs and not after; Cmd+W is jed's in jed but kitty's in a shell window; a new kitty window from jed's starts in the root; inside tmux with `allow-passthrough on` the vars reach kitty; after a panic the vars are cleared (no test drives `restore`)
