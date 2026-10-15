# ui

Drawing only — but `render` is not pure: it takes `&mut App` and writes back state that input handling relies on.

- `mod.rs::render` computes `layout::compute` regions, draws activity bar → side panel → each group (tab bar + editor) from `groups.tree.layout` → dividers → status bar → palette → dialog (last, on top). It builds a fresh `HitMap` and stores it in `app.hits`; mouse handling uses the *previous* frame's map. Below 30×4 it draws nothing.
- Each widget's `render` returns what it drew for hit-testing (`activity` → icon rows, `side` → explorer row area, `tabs` → `Vec<TabHit>`, `editor` → text rect without the gutter); add new clickable things the same way (a `Hit` variant in `layout.rs`, filled here).
- `editor::render` mutates the `View`: sets `height`/`width` (used by page movement), re-anchors `set_top_line` (the buffer may have changed), and applies `reveal_range` (`reveal_in_center`, Cmd+D), else `reveal_pos` (`scroll_to`, minimal scroll to a position: add cursor above/below, box select), else `reveal_cursor` via `scroll_to_cursor`, clearing them. Commands that move cursors set `view.reveal_cursor = true` and let the next frame scroll.
- `side::render` sets `explorer.height` (used for page up/down) and clamps `explorer.scroll` to the row count.
- Text is drawn grapheme by grapheme through `pos::display_grapheme` — the same function column maths uses, so cursor placement and drawing never disagree. Don't measure buffer text any other way (labels in tabs/status/dialogs use `unicode_width` directly).
- Selections: only `selections.overlapping(viewport)` are looked at (there can be 60k); one index walks that sorted slice while bytes are visited in order; a selection over a line break paints one cell past the line end. Secondary cursors are reversed cells (`editor.cursor.secondary`); the terminal cursor marks the primary, only in the focused group with no prompt or palette open.
- Styles: start from the widget's base key (`editor`, `sideBar`, `dialog`, `statusBar`) and `patch` modifier keys on top (`editor.selection`, `gutter.active`, …). Every key comes from `Theme::dark_plus`; never construct `Color`s here.
- `palette::render` draws the open palette (top centre: input, optional hint, up to 14 rows with fuzzy-match highlights and right-aligned bindings), sets `palette.height`, clamps `palette.scroll`, puts the terminal cursor in its input and returns `PaletteHit` (box + rows) for `hits.palette`. The editor and explorer show no focus/cursor while it's open.
- `status.rs`: `Ln x, Col y` for one selection, `N selections` for several (as VS Code).
- `side.rs` shows a placeholder for `SidePanel::Search` / `SourceControl` (panels not built yet).
