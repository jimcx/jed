---
id: settings
name: Settings files
summary: '"Preferences: Open User / Workspace Settings" open `settings.toml` and `.workspace`, with settings-aware syntax highlighting and completion'
status: open
priority: 0
dependencies: []
---

  - Needs: `Command::OpenUserSettings` (8d), the cheat sheet (`cheatsheet.rs`), `config::settings::TEMPLATE` and `WorkspaceConfig` (M8), completion's popup and insertion (M9), the TOML grammar and its highlight pass (`highlight.rs`)
  - Open user settings (Cmd+, / Ctrl+,) and workspace settings (Cmd+Shift+, / Ctrl+Shift+, — VS Code's), the latter writing a commented `.workspace` template on first open; both listed in the cheat sheet; the Cmd bindings update `docs/terminals/`
  - Settings-aware highlighting over the TOML tree for the two config files: known keys and valid values coloured, unknown keys and bad values marked `invalid`
  - Settings completion without a server: keys and value enums (booleans, `render_whitespace` / `cursor_style` strings, powers of two) from a settings schema, offered by Ctrl+Space and quick suggestions in the two config files
  - Done when: Cmd+, / Cmd+Shift+, open (and create from their templates) the two files, with the bindings in the cheat sheet and the terminal docs; in `settings.toml` and `.workspace`, typing a key or value offers the schema's completions that insert at every cursor, and a file with an unknown key / bad value shows it `invalid` while known keys and valid values are coloured; neither completion nor the settings highlighting fires in a `Cargo.toml`
  - Subtasks (one agent and one commit each):
    - settings-a: `Command::OpenWorkspaceSettings`, opening `.workspace` from a commented template (`WorkspaceConfig` template + `create_if_missing`, as `config::settings::TEMPLATE`), via a new `AppEvent`
    - settings-b: the keybindings (Cmd+, / Cmd+Shift+, on macOS, Ctrl variants on Linux, with the shifted-`,` → `<` normalisation in `KeyCombo::from_event`) and `docs/terminals/` (the README table and every snippet)
    - settings-c: the two commands in the cheat sheet's groups; the shortcut and template tests, including the done-when's open-and-create cases
    - settings-d: the settings schema (tables, keys, value kinds) and identifying the two config files (a buffer flag set by path), shared by the highlighter and completer
    - settings-e: settings-aware highlighting over the TOML tree (known keys and valid values coloured, unknown keys and bad values `invalid`)
    - settings-f: settings completion without a server (keys and value enums from the schema, Ctrl+Space and quick suggestions, insert at every cursor); the done-when tests, gaps and hand checks; marks the task ✅
