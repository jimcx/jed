---
id: milestone_7
name: Workspace search
summary: plain text, case-sensitive and regex search across the workspace
status: done
priority: 0
dependencies: []
---

   - Needs: the `search` matcher (M6), excludes and the walk (M5), the side panel and its Search activity icon (M2)
   - Needed by: find references can show its results in the same list (M9)
   - Search view in the side panel; Cmd+Shift+F focuses it, seeded with the selection
   - Plain text by default; case-sensitive and regex toggles, with the same keys as the find widget (Cmd+Alt+C/R, Alt+C/R on Linux; whole word is "Later"; a `search` key context like M6's `find` / `replace`)
   - `ignore` + `grep-searcher` in `spawn_blocking`; results stream in; a new query cancels the old one
   - Dirty buffers searched from memory
   - Results grouped by file; Enter or click opens the match in the focused group; F4 / Shift+F4 step through
   - Cap at 20,000 matches
   - Done when: searching this repo with all three modes gives the same hits as `rg` (with `-F` / `-s` / regex); typing in the query never stalls the UI; an unsaved edit shows up in the results
   - Tasks (one agent and one commit each):
     - ✅ 7a: search engine, no UI: a `workspace_search` module that walks the root with the shared `ignore` walker (excludes, `.jed/`, skip binary) and runs `grep-searcher` with `search::Matcher` in `spawn_blocking`; dirty buffers searched from memory instead of disk; a cancel token; results stream back as `AppEvent` batches grouped by file (line, columns, line text); 20,000 cap with a truncated flag. Tests compare all three modes against a naive reference search (no `rg` binary on the Linux box; check once by hand against `rg -F` / `-s` / regex on this repo)
       - `workspace_search.rs`: `matcher(&Query)`, `start(Job, send) -> Handle` (cancel on drop), `Batch { generation, files, done, truncated }` every ≤50 ms, `Results` model (sorted by path, drops stale generations); `app/workspace_search.rs`: `App::start_search`, `app.search`, `AppEvent::SearchResults`
       - Binary skip uses the buffer's own rule (`file::looks_binary`); BOM stripped, lossy UTF-8; lone-CR files and dirty buffers go through the rope path (same results); unsaved files inside the root are searched too
       - `LineMatch` keeps full-line byte ranges plus a ≤300-byte `text` window (`text_start`, `text_ranges()` for drawing)
       - `Matcher::new_single_line` (a pattern naming a line break matches nothing) and `Matcher::each_in_line` (grep_matcher's `find_iter` is quadratic on long lines)
       - Checked by hand against `rg --count-matches` on this repo (8 patterns, three modes): identical
       - M6's debug-only timing bounds raised (5 s; typing 900 ms, under the 1 s undo-grouping pause); release budgets unchanged
     - ✅ 7b: Search view: the side panel's Search activity icon and view, a query `TextField` with case / regex toggle buttons, a `search` key context (Cmd+Alt+C/R, Alt+C/R on Linux), Cmd+Shift+F focuses it seeded with the selection; typing restarts the search after a short debounce and cancels the old one; results drawn grouped by file with counts and the match highlighted; regex error under the input; "results truncated" note
       - `search_view.rs` (model: query, own case / regex options separate from the find widget's, error, 150 ms debounce, scroll anchored on (file, row), line preview) and `ui/search.rs`; `Focus::SidePanel` maps to `Context::Search` while the panel shows Search
       - Commands `search.findInFiles` (Cmd+Shift+F; seeds from a one-line selection, else re-runs the last query), `search.toggleCase` / `search.toggleRegex`, `search.refresh` (Enter), `search.focusEditor` (Esc); clickable `Aa` / `.*`; mouse wheel scrolls
       - Timers: `App::next_timer` / `on_timer` serve both the fs debounce and the search debounce, in `run` and `settle` alike
       - 10 MB workspace: a query key + frame 0.4 ms avg, 2.3 ms worst (release)
       - Not done: whole-word toggle, collapsing file rows (later)
       - Still to check by hand: Cmd+Shift+F reaches jed in each terminal (iTerm2 may bind ⇧⌘F to Find Globally; WezTerm may need `key = 'F'`); on Linux Ctrl+Shift+F needs the kitty protocol (else it arrives as Ctrl+F, the find widget)
     - ✅ 7c: navigating results: Up/Down in the list, Enter or click opens the match in the focused group with it selected, F4 / Shift+F4 step through results from anywhere; end-to-end "done when" tests (the three modes, an unsaved edit in the results, typing never stalls); marks M7 ✅
       - The list's selection is (file, row in file) plus the match index, stable while results stream in; new `searchResults` key context (falls back to `search`); 10 `search.*` list / open / next / previous commands
       - Enter and double click open the match in the last focused group and focus the editor; single click and F4 keep focus; Enter on a file row opens its first match; typing or pasting in the list goes back to the query
       - Opening with a position: `App::opening` (by path) holds a `Goto` (`Cursor` for `file:line:col`, `Match { line, range }` for search) until the file loads; clamped to the current text; centred with `reveal_range`
       - Done-when tests: three modes vs an independent reference (`workspace_search::reference`) on a copy of `src/` and `docs/`, driven through the view; an unsaved edit found and opened; a 5-key burst during a 10 MB search (0.2 ms + frame, release) ends with only the final query's hits
       - Not done: collapsing file rows, whole word, replace across files, include / exclude globs (all "Later"); F4 across two files that are still loading can leave the later-finishing one active
       - Still to check by hand: F4 / Shift+F4 reach jed in each terminal (some desktops take F4); Enter, click and double click in a real terminal; results open in the last focused split
