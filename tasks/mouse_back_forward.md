---
id: mouse_back_forward
name: Mouse back / forward buttons
summary: the mouse's back / forward buttons run `nav.back` / `nav.forward`
status: done
priority: 0
dependencies: []
---

  - A crossterm fork (github.com/jimcx/crossterm, branch `fwdback`, wired via `[patch.crates-io]`): upstream 0.29 drops SGR buttons 8 / 9 in `parse_cb` ("We do not support other buttons"). The fork adds `MouseButton::Back` / `Forward` and four `parse_cb` arms — `(8, false)` / `(9, false)` → `Down`, `(8, true)` / `(9, true)` → `Drag` (motion is on via 1002 / 1003) — so a back / forward press arrives as `Down(Back / Forward)` and a release (trailing `m`) as `Up`, instead of a parse error. On a crossterm upgrade, rebase `fwdback` and re-apply the patch (see the comment in `Cargo.toml`)
  - `src/app/mouse.rs` `mouse_event`: `Down(Back / Forward)` run `nav.back` / `nav.forward` before anything looks at the hit, so they navigate wherever the pointer is (editor, terminal, popup); `Drag` / `Up` of back / forward are consumed, never forwarded to a hosted terminal program as a phantom button-8 / 9 report
  - `src/tasks/mouse.rs` `button_code`: `Back => 128`, `Forward => 129` (the SGR codes; only for the exhaustive match — back / forward never reach the encoder at runtime)
  - Tests: `todo_tests::mouse_back_forward` (go back, go forward, a no-history no-op), `todo_tests::term_mouse::back_and_forward_drags_and_releases_do_not_reach_the_program`, and `tasks::mouse::tests::sgr_back_and_forward`; the fork also carries its own `parse_csi_sgr_mouse_back_forward` unit test (not run by `scripts/check` — the fork is a path dep, not a workspace member)
  - Also: removed the fork's `unused_parens` warning in `terminal/sys/unix.rs` (path deps are linted, unlike crates.io deps)
  - Deferred: none
  - Still to check by hand: real back / forward clicks in kitty, Ghostty, WezTerm, iTerm2 and Alacritty — not every terminal maps its side buttons to SGR 128 / 129 (or on every mouse), so where a terminal doesn't send them the feature simply doesn't fire
