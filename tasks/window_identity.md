---
id: window_identity
name: Window identity from `.workspace`
summary: `[window]` `title` and `status_bar` name the window and tint the status bar
status: done
priority: 0
dependencies: []
---

  - `workspace/config.rs` — `WindowConfig { title: Option<String>, status_bar: StatusBarColor }` (default `blue`; `red` / `orange` / `yellow` / `green` / `cyan` / `blue` / `purple` / `magenta` / `grey`, or `auto`: one of 120 computed colours (30 hues × 4 saturation / lightness pairs, `Theme::status_bar_auto`, not themeable) from a FNV-1a hash of the root relative to `$HOME`, `ui/status.rs`'s `status_style`; `#[serde(rename_all = "lowercase")]`), `StatusBarColor::theme_key()` → the `statusBar.<name>` theme key
  - `terminal.rs` — `Identity` keeps the default `jed <root>` / `jed <file>` plus a `[window] title` override; `title_sequence()` is the OSC 2 title alone (control characters stripped), `jed_workspace` and OSC 7 keep the real root so Recent Workspaces still resolves the window; `App::sync_window_title` re-sends just OSC 2 (no title-stack push) on a `.workspace` reload
  - `theme.rs` — nine `statusBar.<colour>` keys in `Theme::dark_plus`, white text, `statusBar.blue` = the existing `#007ACC`; `ui/status.rs` looks up the active config's key, the warning / error / restricted cells keep their own keys
  - Neither names a command, so no trust prompt (as `[settings]`); an unknown colour or key is a `.workspace` parse error and the default kept
  - Tests: parsing (default / title-only, all nine colours, an unknown colour and key), the title sequence with an override (and reload re-sending without a push), a `TestBackend` frame per colour, live reload updating both, and a bad colour keeping the last good
  - Deferred: `Theme::light_plus` (a future theme) — only `dark_plus` gained the keys
  - Still to check by hand: the OSC 2 title override in a real terminal (kitty / iTerm2 / WezTerm)
