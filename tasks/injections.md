---
id: injections
name: Nested languages (injections)
summary: a language's highlighter can run another language's over part of its text (cpp in Markdown fences), for built-in and plugin languages alike
status: open
priority: 0
dependencies: []
---

  - Needs: `syntax::highlight::compute_in` and the per-language `HighlightQuery`, `Language::all()` (built-ins plus registered plugins), syntax plugins (`PluginId::load`, `queries/`), `tree-sitter-md`
  - tree-sitter parses a sub-range in the host's own byte coordinates (`Parser::set_included_ranges`), so nested spans need no remapping; the query side is the usual `injections.scm` convention (`@injection.content`, `@injection.language`, `(#set! injection.language "c")`)
  - An injection pass in `compute_in`, beside the line passes (`git_commit_lines`, `ini_lines`): for each injection match overlapping the range (content starting above it included), resolve the language, skip it without a grammar or query (a plugin not loaded yet), parse the content with a per-thread cached `Parser` per language reading the rope chunk by chunk (`pos::chunk_from`), run that language's highlight query over the range and paint its captures over the host's bytes (inner over outer); recursion for the injected tree's own injections. The diff view, minimap and `highlight_text` all call `compute_in`, so they get nested colours too
  - Supported query subset: `@injection.content`, `@injection.language`, `#set! injection.language`, `#set! injection.combined`, `injection.include-children`; other properties logged at debug and ignored
  - `Language::from_injection_name(&str)`: lowercase, then `id()`, `name()`, a small alias table (`c++`, `rs`, `sh` / `bash` / `zsh`, `py`, `yml`, …), then extension detection; plugins add names with an optional `aliases` manifest key
  - Caps, so the UI thread never stalls: nesting depth ≤ 3, injected bytes per `compute` ≤ `MAX_INJECT_BYTES` (~16 KB; a bigger block stays in the host's colour), the same deadline / cancel check as `parse`
  - Deferred: auto-close, auto-indent, bracket matching and comment toggling inside embedded code (host-scoped), LSP for the snippet, caching injected trees across frames, Markdown's inline grammar (emphasis, code spans, links: the same mechanism with `tree_sitter_md::INLINE_LANGUAGE` as an internal language)
  - Done when: a `.md` with `cpp`, `rust`, `python` and `c++` fences colours each fence's code like a file of that language (also in a diff tab and the minimap), an unknown info string and a fence over the cap stay `markup.raw`, a fence opened above the viewport colours its visible part, typing inside a fence re-colours it within the frame budget, and a markdown plugin with `queries/injections.scm` nests both built-in languages and another plugin
  - Tasks (one agent and one commit each):
    - injections-a: core: the injection query (`InjectionQuery` beside `HighlightQuery`, built-ins' `queries/<lang>.injections.scm`, Markdown's fenced-code one), `Language::from_injection_name`, the pass in `highlight.rs`, parser cache and caps; Markdown fences → built-in languages; tests (exact spans for a fence per language, unknown info string, fence above the range, edit inside a fence, depth / size caps, `compute` equal to the standalone highlight of the fence's content); update `src/syntax/CLAUDE.md`, `docs/dev/modules.md`, the "jed has no injections" comments and `milestone_8.md`'s deferred line
    - injections-b: plugins: `PluginId::load` reads `queries/injections.scm` (missing = empty; a bad query reports file, line and column like the others), the `aliases` manifest key, the template under `examples/languages/` and `--check-language` output (injected languages found, unresolved names flagged); tests with `plugin::test_lib` (a plugin injecting a built-in, a built-in fence naming a plugin id); docs
    - injections-c (optional): Markdown's inline grammar (emphasis, code spans, links) through the same API
