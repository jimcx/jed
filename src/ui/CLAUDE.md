# ui

Drawing only — but `render` is not pure: it takes `&mut App` and writes back state that input handling relies on.

- `mod.rs::render` computes `layout::compute` regions, draws activity bar → side panel → commit tabs fitted to their groups' text width (`fit_commit_tabs` → `App::fit_commit_tabs`, `editor::text_width`) → each group (tab bar, the changed-on-disk banner when its active buffer — or the open buffer of its active diff tab's file — has a `disk_conflict` (`banner::render` on the editor's first row; the editor gets the rest, `GroupHit::banner`), editor or diff view (`diff::render`, `GroupHit::diff`), a text or diff tab's scroll bar in the editor's last column (`scrollbar::render`, `GroupHit::scrollbar`; the editor / diff, `GroupHit::editor` and the find widget get the rest)) from `groups.tree.layout` → dividers → the bottom panel (`regions.panel`, when `ui.panel_visible`) → status bar → find widget (over the active group's editor) → the gutter's peek (`quick_diff::render`; `hits.quick_diff`) → hover popup (`hover::render`, over its group's editor; `hits.hover`) → completion list (`completion::render`; `hits.completion`) → code action list (`code_action::render`; `hits.code_actions`) → palette → cheat sheet (`hits.cheat_sheet`) → dialog (last, on top). It builds a fresh `HitMap` and stores it in `app.hits`; mouse handling uses the *previous* frame's map. Below 30×4 it draws nothing.
- Each widget's `render` returns what it drew for hit-testing (`activity` → icon rows, `side` → `SideHits` (explorer row area, the Search view's `SearchHit`, the Source Control view's `ScmHit`, the Tools view's `ToolsHit`), `tabs` → `TabBarHit` (each drawn `TabHit`, and the `TabArrows` while the tabs overflow the bar), `editor` → text rect without the gutter); add new clickable things the same way (a `Hit` variant in `layout.rs`, filled here).
- `editor::render` mutates the `View`: sets `height`/`width` (used by page movement), re-anchors `set_top_line` (the buffer may have changed), and applies `reveal_range` (`reveal_in_center`, Cmd+D), else `reveal_pos` (`scroll_to`, minimal scroll to a position: add cursor above/below, box select), else `reveal_cursor` via `scroll_to_cursor`, clearing them. Commands that move cursors set `view.reveal_cursor = true` and let the next frame scroll.
- Text is drawn grapheme by grapheme through `pos::display_grapheme` — the same function column maths uses, so cursor placement and drawing never disagree. Don't measure buffer text any other way (labels in tabs/status/dialogs use `unicode_width` directly).
- Selections: only `selections.overlapping(viewport)` are looked at (there can be 60k); one index walks that sorted slice while bytes are visited in order; a selection over a line break paints one cell past the line end. Secondary cursors follow `cursor_style`: underlined cells (`editor.cursor.secondaryUnderline`) for `line` / `underline`, reversed (`editor.cursor.secondary`) for `block`; the terminal cursor marks the primary, only in the focused group with no prompt or palette open.
- Styles: start from the widget's base key (`editor`, `sideBar`, `dialog`, `statusBar`) and `patch` modifier keys on top (`editor.selection`, `gutter.active`, …). Every key comes from `Theme::dark_plus`; never construct `Color`s here.

One line per widget or editor layer; [docs/dev/ui.md](../../docs/dev/ui.md) has each in full (keys, layout, hits, render-time state) — read the entries for what you touch, and add detail there, not here.

- `activity.rs` — the activity bar's icons
- `tabs.rs` — tab bars: overflow scrolling with `‹` / `›`, revealing the active tab, deleted / diff / dirty titles
- `editor.rs` — the editor (takes an `Overlay { focused, find, whitespace, blame }`), layered per grapheme in one pass, each walked with one index over sorted on-screen data: syntax spans (`buf.syntax.spans`), matched brackets, diagnostics' squiggles, rendered whitespace, find matches, selections, inline blame on the primary's line, gutter quick-diff markers
- `find.rs` — the find / replace widget (top right of the active editor), `draw_field` (shared one-line input drawing)
- `side.rs` — the side panel: explorer rows with git colours, the Explorer's split with the "Git History" section (divider, header, commit rows)
- `search.rs` — the Search view: inputs, toggles, details, results list
- `scm.rs` — the Source Control view: branch line, sections, rows, `+` / `−` buttons on the hovered row
- `tools.rs` — the Tools view: server / task / jed rows, state marks, error badges
- `palette.rs` — the palette (input, hint, ≤ 14 rows with match highlights and bindings)
- `cheatsheet.rs` — the keyboard cheat sheet overlay over the editor area (columns, scroll bar, `cheatSheet.heading`)
- `popup.rs` — placing popups at a buffer position (`cell_of`, `place`): shared by hover and completion
- `hover.rs`, `completion.rs`, `code_action.rs` — the hover popup, the completion list, the code action list (the lightbulb is drawn in `mod.rs`)
- `quick_diff.rs` — the gutter's peek (removed lines, Stage, `×` close)
- `hex.rs` — a hex tab: fitted (`HexTab::fit`), the visible rows as `hex::row` with the `hexEditor.*` keys, find matches, selection and cursor in both columns
- `diff.rs` — a diff tab: header, side by side or inline rows, Stage / Revert buttons, syntax colours
- `panel.rs` — the bottom panel: border, tab strip with badges, Problems / Locations lists
- `task.rs` — a task tab's terminal (`tui-term`, the theme's `terminal_cell`), resizing PTYs to it (render-time state)
- `status.rs` — the status bar: Restricted badge, branch, file, problem counts, progress, position / language
- `banner.rs` — the changed-on-disk banner and its buttons
- `dialog.rs` — the prompts
- `splash.rs` — a group with no tabs
- `scrollbar.rs` — `take_column`, `look`, `render`: every scrolled area's one-column bar
