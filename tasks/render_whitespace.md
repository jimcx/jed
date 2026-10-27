---
id: render_whitespace
name: Render whitespace
summary: "VS Code's `editor.renderWhitespace` setting, plus a View: Toggle Render Whitespace command"
status: done
priority: 0
dependencies: []
---

  - `edit::RenderWhitespace` (`Settings::render_whitespace`, both settings files); drawn in `ui/editor.rs` (`shows_whitespace`, `Overlay::whitespace`): only U+0020 and tab (NBSP draws as itself), a tab's `→` in its first cell, `editorWhitespace.foreground` over token colours, under selection / find backgrounds, not at secondary cursors or the focused primary; `view.toggleRenderWhitespace` flips `UiState::whitespace_toggled` (session-only, survives `.workspace` reloads), read via `App::render_whitespace()`
  - Tests: `todo_tests::render_whitespace`; older end-to-end tests that read drawn text call `App::test_plain_whitespace()`; the M1 / M6 1 MB frame-budget tests assert they draw with `all`
  - Still to check by hand: the dim colour on the current line (256 colours: 237 on 235) and a block cursor over `·` in a real terminal; the release budget run on an idle machine (it missed by < 1 ms under a load of ~9 from a kernel build, the same with whitespace off)
