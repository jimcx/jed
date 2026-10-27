---
id: diff_syntax
name: Syntax highlighting in the diff view
summary: diff tabs draw both sides with the file's syntax colours
status: done
priority: 0
dependencies: []
---

  - Language from the tab's path (`syntax::detect`, `.workspace` associations); each side parsed with tree-sitter off the UI thread in `diff_view::load` (the side's whole text, not its lines on their own, so multi-line strings / comments colour right), highlight spans kept per side in the `Doc` by line; above `syntax_max_bytes` (or binary): plain text as now
  - A live right side (the open buffer) could reuse the buffer's own `Syntax` spans instead of a second parse — decide; either way a reload after an edit re-colours (the diff already reloads after `DELAY`)
  - Token colours are foreground only: the removed / inserted line backgrounds and the changed-parts text backgrounds stay on top (VS Code); the `╱` filler and line numbers unchanged
  - Drawing stays O(visible rows): spans looked up for the visible rows only, walked with one index like the editor's
  - Decided: no reuse of the live buffer's spans; both sides parsed in `diff_view::load` (`syntax::highlight_text(language, text)`, blocking), the text exactly as drawn (BOM dropped, lossy UTF-8, line breaks kept); spans flat per side in `Lines` (`spans`, `start`, `first_span`), `Doc::highlight`; `Job` carries `language` and `syntax_max_bytes` (from `sync_diffs`): the open buffer's language, else the path (`syntax::detect_path`, `.workspace` associations), else either side's shebang (`detect_shebang`); over the limit, binary or no grammar: plain
  - `ui/diff.rs` walks each side's spans with one index from a binary search at the first visible line; token styles are foreground / modifiers only, under the changed-part backgrounds; line numbers and `╱` filler unchanged
  - Also: `DiffTab::set_layout` keeps a cursor on a change's first row on that change's first row in the other layout (the first load's landing no longer depends on whether the tab was drawn inline first)
  - Deferred: the inline layout still has no changed-part highlight; a `.workspace` reload changing associations / `syntax_max_bytes` re-colours only at the next reload; a superseded load's parse isn't cancelled; a side near the 8 MB default limit can take seconds to colour (off-thread)
  - Still to check by hand: Open Changes on a Rust / Python file with a block comment or docstring changed in the middle, at both widths; File History Compare and the banner's Show Diff; 256-colour terminals
