---
id: milestone_8
name: Highlighting
summary: grammars and the built-in dark theme
status: done
priority: 0
dependencies: []
---

   - Needs: byte-indexed edits (M1) to feed tree-sitter's incremental parse; `theme` keys (M1); multi-cursor (M6) so the language-aware editing below runs at every cursor
   - Needed by: LSP (M9, language detection gives the `languageId`)
   - Language detection: file extension, shebang, `.workspace` file associations (`SConstruct` → Python); language mode in the status bar
   - C/C++ and Python grammars with `highlights.scm`; initial parse off the UI thread, incremental after
   - Then Rust, Markdown, YAML, JSON, XML, TOML, shell, CMake, Makefile and Dockerfile (8f)
   - Highlight queries on the visible range plus a margin; off past the size limit
   - Dark+ theme through named keys only; truecolor with 256-colour fallback
   - Language-aware editing: comment line (Cmd+/), auto-indent, bracket matching, auto-close pairs
   - Keep [Minimap](../PLAN.md#minimap) open: highlight spans can be read for any line range
   - Done when: typing in a 1 MB C++ file never takes a frame over 16 ms; a newly opened file shows plain text first, then colours, with no freeze
   - Tasks (one agent and one commit each):
     - ✅ 8a: `syntax` module, no colours yet: language detection (extension, shebang, `.workspace` `[files]` associations), language mode in the status bar; tree-sitter with the C, C++ and Python grammars; initial parse in `spawn_blocking` reported by an `AppEvent`, incremental re-parse from each `ChangeSet` (tree-sitter `InputEdit`s, also through undo / redo); the size limit past which there is no tree
       - `syntax/` (`Language` with VS Code ids `c` / `cpp` / `python`, `Associations` via globset, `detect`; `Syntax` on `Buffer`: `tree()` is `Some` only when it matches the text exactly); `app/syntax.rs` `sync_syntax` once per loop turn; `AppEvent::Parsed`; `[settings] syntax_max_bytes` (default 8 MiB, 0 = off); language name in the status bar
       - `.h` is C++ (VS Code's default); SCons files are Python; shebangs through `env` (incl. `-S`)
       - `Buffer::edit` / `undo` / `redo` all go through `Buffer::apply`, which feeds InputEdits (LF-only rows via `pos::ts_point`); sets over 256 changes coalesce into one edit, and wide ones (>64 KB) drop the tree and reparse in the background (the old tree is freed off the UI thread)
       - Small edits reparse on the UI thread with a 12 ms stop (8 ms since 8b) (also checked in the read callback, so `/*` can't stall), then fall back to the background; edits during a background parse are queued (≤4096) and applied to its result
       - For 8b: `tree()` is `None` while a background reparse runs, so keep the last colours for drawing rather than flashing plain text
       - 1 MB C++ (release, i7-2600K): first parse 620 ms (background), a key avg 2.7 ms / worst 4.2 ms, `/*` 12.5 ms, a key at 63.7k cursors 4.6 ms
       - Not done: "Change Language Mode" command; tree-sitter's error recovery can give a slightly different tree incrementally than from scratch on badly broken code (inherent)
     - ✅ 8b: highlighting: `highlights.scm` queries over the visible range plus a margin, captures → named theme keys in Dark+ (truecolor, 256-colour fallback), drawn in the editor under selections / find matches; spans readable for any line range (Minimap); plain text until the tree arrives; the "done when" perf tests (typing in a 1 MB C++ file, frames under 16 ms; open shows plain text then colours)
       - `syntax/highlight.rs`: capture → theme key with longest-prefix fallback; queries compiled once and warmed off-thread; inner node wins, then the later pattern; `Syntax::spans(rope, lines)` caches up to 4 regions (±50 lines) per tree version — the Minimap API (ask for big ranges in pieces: queries stop at 16 KB from the first line, as VS Code stops tokenizing long lines)
       - jed's own query additions in `syntax/queries/` (control keywords purple, builtin types / constants, `this` / `self`, …); 20 Dark+ token keys in the theme
       - While a background reparse runs, the last spans are mapped through each edit (text typed at a span's edge stays plain until the new tree), so colours never flash off
       - 1 MB C++ (release, i7-2600K): open draws plain text in 1 ms and stays under 3 ms a frame during the 620 ms parse; typing avg 5.3 ms, worst ~11 ms; paging worst 2.2 ms; a 1 MB single line ~10 ms
       - Not done: an unclosed `/*` isn't coloured as a comment (tree-sitter error recovery); C macro calls and goto labels; semantic highlighting
       - Still to check by hand: colours against VS Code Dark+; 256-colour mode with `COLORTERM` unset; a big C++ file opening plain then coloured without a stall
     - ✅ 8c: comment line (Cmd+/) and auto-close pairs (brackets and quotes, type-over, delete the pair, wrap a selection) at every cursor, from a per-language config (comment tokens, pairs)
       - `syntax/config.rs` `LanguageConfig` (VS Code's C/C++, Python and plaintext language configurations: comment token, auto-close pairs with `notIn`, surround pairs, `autoCloseBefore`, Python string prefixes); `comment.rs` (toggle per selection as VS Code's LineCommentCommand); `pairs.rs` (type-over → auto-close → wrap; `delete_pair`; `View::auto_closed` tracks jed-inserted closers so only those are typed over / deleted, dropped on any other change or a line break inside)
       - `[settings] auto_close_pairs` (default true) turns off auto-close, type-over, pair delete and wrap; comment line unaffected
       - 63k cursors (release): `(` 9 ms, type-over 10 ms, pair Backspace 11 ms; Cmd+/ on 84k lines ≤ 46 ms
       - Not done (VS Code differences): no auto-close before an unmatched closing bracket (needs bracket matching, 8e); no stand-in-character quote check; without a current tree the "not in string / comment" rule is skipped; comment indentation isn't rounded to the indent size
       - Still to check by hand: Cmd+/ reaches jed (iTerm2's Find Cursor menu item; layouts where `/` is shifted may send Cmd+Shift+7; on Linux without the kitty protocol Ctrl+/ arrives as Ctrl+7)
     - ✅ 8d: user settings file: `~/.config/jed/settings.toml` (`paths::config_dir()`, the same `[settings]` keys as `.workspace`) loaded at startup like `keybindings.toml`, errors reported via notify; `.workspace` `[settings]` override it per key; `auto_close_pairs` (added in 8c) and the other settings work from either. Live reload is in the TODO list
       - `config/settings.rs`: `SettingsOverrides` (shared with `.workspace`), `layer(user, workspace)` is the one precedence function (defaults ← `settings.toml` ← `.workspace`, per key); `TEMPLATE` documents every key (a compile-time check keeps it complete)
       - `settings.toml` is lenient (bad keys / types reported and skipped, a syntax error keeps the defaults); `.workspace` stays strict; load errors from settings and keybindings show together after start (`App::report_errors`)
       - "Preferences: Open User Settings" (palette) creates the file from the template if missing (never overwrites) and opens it
     - ✅ 8e: auto-indent (Enter keeps the indent, indents after an opening bracket or Python `:`, outdents on a closing bracket) and bracket matching (highlight the pair at the cursor) at every cursor, each with an on / off setting
       - `indent.rs` (Enter: carry the indent up to the cursor, +1 after a real-token opener, closer on its own line between a pair, C/C++ `indentationRules` (increase / decrease patterns), Python block `:` and dedent after `return` / `pass` / …; outdent a typed closer to its opener's indent; Python `else` / `elif` / `except` / `finally` dedent); `brackets.rs` (bracket after the cursor first, then before; tree tokens with a ±16 KB text scan fallback; up to 64 on-screen cursors; cached in `View::brackets`; `editor.bracketMatch`); `syntax/tokens.rs` forward-only token walk
       - Settings `auto_indent` and `match_brackets` (default true), from `settings.toml` or `.workspace`
       - 65k cursors (release): Enter ≤ 53 ms, `}` outdent 17 ms; bracket-matching frame 3.4 ms
       - Not done (VS Code differences): C++ `onEnterRules` continuing `// ` comments and outdenting after a brace-less `if` / `for` / `while`; no indentation normalisation or `trimAutoWhitespace`; outdent applies per cursor; without a tree brackets in strings count; Go to Bracket; no auto-close before an unmatched closer
     - ✅ 8f: more languages: Rust, Markdown, YAML, JSON (and JSONC), XML, TOML, Bash / shell, CMake, Makefile, Dockerfile — each a tree-sitter grammar crate compatible with the pinned tree-sitter version, detection (extensions, file names like `Makefile` / `CMakeLists.txt` / `Cargo.lock` / `Dockerfile` / `Containerfile` / `*.dockerfile`, shebangs like `sh` / `bash`), a `highlights.scm` mapped onto the existing Dark+ keys (adding keys only where needed, e.g. Markdown headings / emphasis / links, XML tags / attributes), and a `LanguageConfig` (comment tokens, pairs) so comment line and auto-close work; a language with no usable grammar crate is dropped and noted; marks M8 ✅
       - Grammars: tree-sitter-rust, -md (block grammar), -yaml, -json (JSON and JSONC), -xml, -toml-ng, -bash, -cmake, -make, -containerfile (for Dockerfile: tree-sitter-dockerfile needs tree-sitter 0.20); none dropped
       - Detection (VS Code ids and lists): exact names (`Makefile`, `CMakeLists.txt`, `Dockerfile`, `Cargo.lock`, `tsconfig.json` (JSONC), shell dotfiles, …), name patterns (`Dockerfile.*`, `tsconfig.*.json`, `.vscode/*.json`), multi-dot suffixes, extensions, shell / `make` shebangs; associations still first; `.workspace` is TOML
       - Highlighting: each grammar's `highlights.scm` + jed's `queries/<lang>.scm` (CMake and TOML: jed's own only); `ALIASES` maps grammar captures onto existing keys; new keys `markup.heading` / `list` / `quote` / `raw` / `link`, `tag`, `tag.attribute`, `punctuation.tag` (YAML keys use `tag`, as Dark+)
       - `LanguageConfig` gained explicit `brackets`, `tree_tokens` (false for Markdown: brackets matched from the text) and `insert_spaces` (Makefiles: tabs, via `edit::Settings::for_language`); `Syntax::scope_of(kind)` is one string / comment / code table across grammars; `Tokens::node_at` finds comments whose text is in no leaf (Rust); Rust uses C's `indentationRules`, YAML indents after `:` / `-`
       - 1 MB files (release) in Rust, JSON, YAML, Markdown, XML, TOML, shell: open frames ≤ 4.5 ms, a key + re-parse + frame 9–11 ms, background parses 120–690 ms
       - Not done (VS Code differences): Markdown inline formatting (emphasis, code spans, links in paragraphs: needs injections) and embedded languages in fenced code; block-comment toggling (Markdown, XML have no Cmd+/); JSON has no `//`; no general per-language settings (only Makefile `insert_spaces`); `fish` shebang; `.xsl` / `.xslt`
       - Known: with the cursor on the root `{` of a 1 MB flat JSON object, tree bracket matching walks all its siblings (~6 ms, a ~14.5 ms frame); cap the partner search like the text scan's `WINDOW` (TODO)
       - Still to check by hand: colours against VS Code Dark+ per language (YAML keys, XML tag brackets, Markdown headings and link underline); a real Makefile's Tab and status bar; brackets in a Markdown paragraph
