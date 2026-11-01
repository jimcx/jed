---
id: conf_files
name: Conf / config / INI files
summary: `.conf`, `.config`, `.ini` and `.cfg` files open with syntax colours and Cmd+/ comment toggling
status: done
priority: 0
dependencies: []
---

See [Syntax highlighting](../PLAN.md#syntax-highlighting), [Keybindings](../PLAN.md#keybindings-vs-code-defaults).

  - Language: a new `Language::Ini` (id `ini`, name "INI", VS Code's id for the family), in the same places as the other grammar languages (`syntax::Language`, its config, `scope_of` kinds, highlight query in `queries/`)
  - Detection: by extension, case-insensitive, in `language.rs`'s extension table: `conf`, `config`, `ini`, `cfg`, plus `properties` if it parses well; `.workspace` associations still win, as for any language. Names with no extension that are conventionally INI-style (`.editorconfig`, `.gitconfig`, `.npmrc`, `.pylintrc`, `setup.cfg` is covered by `cfg`) are decided by the implementer — list the ones added in the notes. A plain `.config` directory or a path under `~/.config` is not a match: only the file's own name counts. Nothing is sniffed from the text
  - Highlighting: section headers (`[section]`), keys, values, `=` / `:` separators, quoted strings, numbers and booleans, and comments. Pick the grammar and say why in the notes: a tree-sitter INI crate if one exists on a compatible `tree-sitter-language` (check licence, size of `parser.c` and clean-build time), otherwise a small hand-written line highlighter in `highlight.rs`'s style; real `.conf` files are loose (nginx, sshd, systemd, sysctl, `key value` with no separator), so a file the grammar half-parses must still colour comments and `[sections]` rather than turning into error noise. Use existing capture aliases and theme keys, none new unless a gap shows
  - Comments: both `#` and `;` start a comment (full-line only; INI has no reliable trailing-comment rule). Cmd+/ (Ctrl+/ on Linux) toggles line comments through the existing `Command` and `LanguageConfig`, like every other language: `#` is the toggle token written, a line starting with either `#` or `;` counts as already commented when toggling off, and mixed selections follow the usual rule (all commented → uncomment, else comment). No block comment
  - Language config: no auto-closing pairs beyond quotes and `[` `]`; no auto-indent after a line; `tab_width` and `insert_spaces` come from `settings.for_buffer` as usual
  - Needs: `syntax::Language` and its queries, `highlight.rs` aliases, the comment-toggle command (`LanguageConfig` comment tokens), `docs/dev/conventions.md`'s settings rule
  - Tests: detection (`nginx.conf`, `app.CONFIG`, `a/b/setup.cfg`, `x.ini`, a `.config/foo` path that is not matched); highlight spans for a snippet with a section, `key = value`, `key: value`, a quoted string, a `#` and a `;` comment, and a `key value` line; comment toggle on one line, on several selections, and on a mix of `#` / `;` lines; `src/syntax/CLAUDE.md`'s language list updated
  - Done when: opening `x.conf`, `x.config`, `x.ini` and `x.cfg` shows sections, keys, values and comments in distinct colours and "INI" in the status bar; Cmd+/ comments and uncomments the selected lines with `#`, and uncomments lines that start with `;`; the tests above pass

Done:
  - Built on `tree-sitter-ini` 1.4.0 (Apache-2.0, 25 KB `parser.c`) for the parse tree only (`scope_of`); the grammar rejects `key: value` and bare `key value` lines, so colours come from `highlight::ini_lines`, a per-line pass over the requested range (`queries/ini.scm` is just `(comment) @comment`)
  - Detection: `conf`, `config`, `ini`, `cfg`, `properties`; names `.editorconfig`, `.gitconfig`, `.gitmodules`, `.npmrc`, `.pylintrc`, `.flake8`
  - `LanguageConfig::alt_line_comment` (`;` for INI, `None` elsewhere): `comment::toggle` counts it as commented when toggling off
  - Also: `[` / `]` are INI `brackets`, so Enter between `[|]` indents (as in VS Code); Enter after `[core]` does not. FLAKE debugging added to `term_select_mouse_reporting_keeps_every_event` (failed twice in full runs: scrollback 3, expected 2)
  - Still to check by hand: open real `x.conf` / `x.config` / `x.ini` / `x.cfg` files (colours, "INI" in the status bar) and Cmd+/ in a session
