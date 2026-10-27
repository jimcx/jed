---
id: kitty_recent
name: Kitty integration for recent workspaces
summary: focus / open recent workspaces in kitty windows via remote control
status: open
priority: 0
dependencies: []
---

Deferred from M13 — see [Recent workspaces](../PLAN.md#workspace). `jed_workspace` user var (already set: Terminal window identity); `kitten @ ls` marks rows already open and focuses that window; `kitten @ launch --type=os-window --cwd <path> jed <path>` for one not open; off the UI thread with a timeout, message fallback; tests through a fake `kitten` on `PATH` (canned `ls` JSON, records its arguments); `docs/terminals/kitty.md` (`allow_remote_control`). Kitty isn't installed on the Linux test box: real-kitty behaviour is a hand check
