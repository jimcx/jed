---
id: vhdl
name: 'VHDL: tree-sitter and VHDL-LS'
summary: VHDL files get syntax colours, comment / pair / indent behaviour and a language server (`vhdl_ls` from rust_hdl), as C++ / Python / Rust do
status: open
priority: 0
dependencies: []
---

  - Language: `Language::Vhdl` (id `vhdl`, name "VHDL", VS Code's VHDL extensions' id); detected by `.vhd` / `.vhdl` (case-insensitive, as the other extensions; `.vho` / `.vhf` too if VS Code's VHDL extension lists them — check); `.workspace` associations work as for any language
  - Grammar: the `tree-sitter-vhdl` crate (1.4.0, jpt13653903/tree-sitter-vhdl, MIT, on `tree-sitter-language` like the others; its `HIGHLIGHTS_QUERY` is the Neovim query) — check its captures map to `highlight::KEYS` / `ALIASES` (the every-capture test), add `queries/vhdl.scm` for Dark+'s colours (control keywords `if` / `case` / `loop` / `process` …, built-in types `std_logic` / `integer` …, `--` comments). VHDL is case-insensitive: keyword / type predicates must match any case (`(?i)`). Its `parser.c` is 8.4 MB: note the clean-build time it adds; `scope_of` must know its string / comment / character-literal node kinds (`scopes_in_every_language`)
  - Language config (`LanguageConfig`, VS Code's VHDL extension's `language-configuration.json`): line comment `--`; brackets `()` (VHDL has no `{}` / `[]` blocks in practice — check the extension); quotes `"` (and `'` not auto-closed: it's attributes and character literals, `clk'event`); block comments `/* */` (VHDL-2008) left to the grammar's scopes
  - Server: `vhdl_ls` as a built-in (`lsp::config` `Builtins`, like `clangd` / `ty` / `rust-analyzer`; `.workspace` `[lsp.vhdl]` still overrides), root = the workspace root. It reads its libraries from `vhdl_ls.toml` at the root (then `$VHDL_LS_CONFIG`, then `~/.vhdl_ls.toml`): check what it does without one (only open files? a warning?) and say so in the server docs; its standard libraries (`ieee`, `std`) ship with the release binary (`vhdl_libraries/` beside it) — check a `cargo install vhdl_ls` binary finds them. Diagnostics (pushed), hover, definition, references, completion, rename; check whether it formats (if not, Format Document says the server can't)
  - Install: not on this machine; `cargo install vhdl_ls` (or the GitHub release zip) — ask the user before installing. A missing binary gives the usual "couldn't start vhdl_ls" message with its stderr line
  - Tests: unit tests for detection, config and highlight spans for a VHDL snippet (entity / architecture / process, a `--` comment, a string, a `'event` attribute); a fake-server test for the built-in being picked for a `.vhd` file; a real-server test (like rust-analyzer's) on a two-file temp project with a `vhdl_ls.toml` — a diagnostic for an undeclared signal, hover and definition across files — skipped with a note when `vhdl_ls` isn't runnable
  - Needs: `syntax::Language` and its queries, `lsp::config` `Builtins`, the real-server test helpers (`REAL_SERVERS`, `children_named`)
  - Done when: a `.vhd` file opens coloured with `--` comment toggling and auto-closed `(` / `"`, and with `vhdl_ls` installed shows diagnostics, hover and go to definition across files in a project with a `vhdl_ls.toml`; the tests above pass
  - Subtasks (one agent and one commit each):
    - vhdl-a: the language: `Language::Vhdl`, detection, the grammar crate, highlight queries (+ `queries/vhdl.scm`), `scope_of` kinds, `LanguageConfig`; unit tests; docs (`src/syntax/CLAUDE.md`'s language lists)
    - vhdl-b: the server: the `vhdl_ls` built-in, the fake-server test, the real-server test (skipped without the binary), a `vhdl_ls.toml` note in the server docs; hand checks; marks the task ✅
