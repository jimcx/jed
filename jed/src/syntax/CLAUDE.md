# syntax

Language detection, parse trees and highlight spans. No drawing (the editor draws the spans). Summary only: [docs/dev/syntax.md](../../../docs/dev/syntax.md) has every type, state and rule — read the entry for the file you touch, and add detail there, not here.

- `language.rs` — `Language` (`BUILT_IN`, `all()`, `Plugin(PluginId)`), detection by file name / extension / shebang, grammars and queries per language
- `plugin.rs` — syntax plugins: manifests (`~/.config/jed/languages/<id>/language.toml`) and the global registry
- `build.rs` — building a plugin's grammar (blocking; `--check-language` shares it)
- `authoring.rs` — `jed --new-language` / `--check-language`
- `config.rs` — `LanguageConfig`: comments, brackets, auto-close and surround pairs, indent rules
- `mod.rs` — `Syntax` (one per `Buffer`): states (`Off`, `TooLarge`, `Wanted`, `Parsing`, `Ready`), `update` (inline re-parse or a `ParseJob`), `scope_before`
- `highlight.rs` — `KEYS`, capture → theme key mapping, `Highlights` (cached `Region`s), the settings and git commit passes
- `tokens.rs` — `Tokens`: a cursor walk over the tree's leaves for increasing positions
- `tests.rs` — drives `Syntax` through a `Buffer` without the app

Rules:

- **Parse trees:** `buf.syntax.tree()` is `Some` only while the tree matches the text exactly; after an edit and during a background parse it is `None`. Drawing never reads the tree: it asks `buf.syntax.spans(rope, lines)`, which keeps the last colours until the new tree arrives.
- tree-sitter `Point`s come from `pos::ts_point`; parses read the rope through `pos::chunk_from` (no copy).
- Inline re-parses are bounded (`INLINE_EDITS`, `INLINE_BYTES`, `INLINE_TIME` 8 ms); everything else is a `ParseJob` for `spawn_blocking`. Queries compile off the UI thread (`warm`).
- The plugin registry is global and append-only: tests register unique ids, never assume it is empty, and claim no extension, file name or shebang another test detects.
- Every capture name maps to a key in `KEYS` (each in `Theme::dark_plus`) or is listed as intentionally plain; a test checks every language's query.
- Logging under `jed::syntax`: parse times at debug, `slow parse` warning over 500 ms.
