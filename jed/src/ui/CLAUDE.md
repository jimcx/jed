# ui

Drawing only — but `render` is not pure: it takes `&mut App` and writes back state that input handling relies on.

- `mod.rs::render` computes `layout::compute` regions and draws back to front: activity bar → side panel → each group from `groups.tree.layout` (tab bar, changed-on-disk banner, editor / diff / hex / shell, marks column and scroll bar), then the terminal area's groups by the same loop → dividers → bottom panel → borders → status bar → find widget → popups (gutter peek, hover, completion, code actions) → palette → cheat sheet → dialog (last, on top); the exact order and each `GroupHit` field are in docs/dev/ui.md. It builds a fresh `HitMap` and stores it in `app.hits`; mouse handling uses the *previous* frame's map. Below 30×4 it draws nothing.
- Each widget's `render` returns what it drew for hit-testing (`activity` → icon rows, `side` → `SideHits` (explorer row area, the Search view's `SearchHit`, the Source Control view's `ScmHit`, the Tools view's `ToolsHit`), `tabs` → `TabBarHit` (each drawn `TabHit`, and the `TabArrows` while the tabs overflow the bar), `editor` → text rect without the gutter); add new clickable things the same way (a `Hit` variant in `layout.rs`, filled here).
- `editor::render` mutates the `View`: sets `height` / `width` / `wrapping` / `revision` / `tab_width` (page movement and scrolling read them), re-anchors the top line, and applies `reveal_range`, else `reveal_pos`, else `reveal_cursor`, clearing them. Commands that move cursors set `view.reveal_cursor = true` and let the next frame scroll.
- Text is drawn grapheme by grapheme through `pos::display_grapheme` — the same function column maths uses, so cursor placement and drawing never disagree. Don't measure buffer text any other way (labels in tabs/status/dialogs use `unicode_width` directly).
- Selections: only `selections.overlapping(viewport)` are looked at (there can be 60k); one index walks that sorted slice while bytes are visited in order; a selection over a line break paints one cell past the line end. Secondary cursors follow `cursor_style`: underlined cells (`editor.cursor.secondaryUnderline`) for `line` / `underline`, reversed (`editor.cursor.secondary`) for `block`; the terminal cursor marks the primary, only in the focused group with no prompt or palette open.
- Styles: start from the widget's base key (`editor`, `sideBar`, `dialog`, `statusBar.<colour>`) and `patch` modifier keys on top (`editor.selection`, `gutter.active`, …). Every key comes from `Theme::dark_plus`; never construct `Color`s here.

One line per widget or editor layer; [docs/dev/ui.md](../../../docs/dev/ui.md) has each in full (keys, layout, hits, render-time state) and the full form of the rules above — read the entries for what you touch, and add detail there, not here.

- `activity.rs` — the activity bar's icons (greyed, `activityBar.disabled`, when `Mode::allows_view` says no: single file mode)
- `tabs.rs` — tab bars: overflow scrolling with `‹` / `›`, revealing the active tab, title styles and a shell title's state glyph, a dragged tab's drop marker, the group's number with two or more groups; the bar's rows / padding come from `TabMetrics` (touch layout: 3 rows, `+`, 3-cell targets, `layout::bar_and_rest` is the one place that splits a group's area)
- `editor.rs` — the editor (takes an `Overlay { focused, find, whitespace, blame, wrapping }`), layered per grapheme in one pass, each layer walked with one index over sorted on-screen data (syntax spans, brackets, squiggles, whitespace, occurrences, find matches, selections, blame, gutter markers, merge conflicts); with `wrapping` it draws `wrap::Wrap`'s rows of each line
- `find.rs` — the find / replace widget (top right of the active editor), `draw_field` (re-exported from `jtui::field_draw`)
- `side.rs` — the side panel: explorer rows with git colours, the Explorer's split with the "Git History" section (divider, header, commit rows)
- `search.rs` — the Search view: inputs, toggles, details, results list
- `scm.rs` — the Source Control view: branch line, sections, rows, `+` / `−` buttons on the hovered row (the title row's `↻` refresh is drawn by `side.rs`)
- `tools.rs` — the Tools view: server / task / jed rows, state marks, error badges
- `review.rs` — the Code Review panel: summary, FILES rows (viewed mark, status letter, `+a -d`), CONVERSATIONS, PENDING (drafts, held back ones named), empty states, errors
- `palette.rs` — the palette (input, hint, ≤ 14 rows with match highlights and bindings)
- `cheatsheet.rs` — the keyboard cheat sheet overlay over the editor area (columns, scroll bar, `cheatSheet.heading`)
- `keys.rs` — the Keyboard Shortcuts view overlay: search box (or the `Press key combination…` record hint), rows in key / title / context / source columns, selection, scroll bar (`ScrollTarget::Keys`)
- `popup.rs` — placing popups at a buffer position (`cell_of`, `place`): shared by hover and completion
- `context_menu.rs` — the context menu: borderless popup, hints from the keymap, `hits.context_menu`
- `hover.rs`, `completion.rs`, `code_action.rs` — the hover popup, the completion list, the code action list (the lightbulb is drawn in `mod.rs`)
- `quick_diff.rs` — the gutter's peek (removed lines, Stage, `×` close)
- `hex.rs` — a hex tab: fitted (`HexTab::fit`), the visible rows as `hex::row` with the `hexEditor.*` keys, find matches, selection and cursor in both columns, placeholders for pages not loaded yet
- `markdown.rs` — a Markdown preview tab: fitted to the width (re-rendered), visible rows of the `markdown::Doc` styled by `Theme::markdown_style`, code rows on `markdown.codeblock`, selection and caret
- `diff.rs` — a diff tab: the caret's selection (`editor.selection`, over the diff backgrounds, under token colours) and terminal cursor, `DiffHit::text_x` / `divider` for the mouse, header, side by side or inline rows, Stage / Revert buttons, syntax colours
- `borders.rs` — re-export of `jtui::borders`, the border layer: every `│` / `─` on screen is drawn here, never by a widget (widgets leave the border cells blank and `mod.rs` collects a `Segment` per border; a side section's divider row goes in `SideHits::rules`); junctions where lines meet
- `panel.rs` — the bottom panel: border, tab strip with badges, Problems / Locations lists
- `task.rs` — a task tab's frame (resizing PTYs to it: render-time state) around the screen `jterm::draw` draws; `render_shell` (re-exported from `jterm::draw`) draws a shell tab's screen in the editor area
- `status.rs` — the status bar: Restricted badge, branch, file, merge conflict count, problem counts, progress, selection occurrences (find closed), position / language
- `banner.rs` — the changed-on-disk banner and its buttons
- `dialog.rs` — the prompts
- `splash.rs` — a group with no tabs
- `scrollbar.rs` — `take_column`, `look`, `render`, `render_marks` (+ `take_row`, `render_row`: the editors' horizontal bar): every scrolled area's one-column bar; an editor group's marks column (left of the bar, always reserved) draws the changed lines' marks
