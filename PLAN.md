# red — rust editor

A terminal text editor with a VS Code-style layout and keybindings. Not modal, nothing like vim.

## Goals
- Works the way VS Code does, inside a terminal
- Mouse and keyboard both fully supported
- Starts fast, stays light on memory
- Smooth editing on files up to 1 MB
- Platforms: macOS first, Linux later
- Terminals: only those that implement the kitty keyboard protocol — see [Terminals](#terminals)

## Non-goals
- Vim emulation or modal editing
- A plugin system (for now)
- Windows support
- Code folding
- Git beyond viewing and staging (commit, branch, push, …)
- Chorded keybindings (e.g. Cmd+K Cmd+D); every binding is a single key combo
- Terminals without the kitty keyboard protocol (stretch goal)
- _TBD_

## Stack
| Concern | Choice | Notes |
|---|---|---|
| TUI rendering | `ratatui` | |
| Terminal I/O and input | `crossterm` | mouse, key events, kitty keyboard protocol, bracketed paste |
| Text buffer | `ropey` `=2.0.0-beta.1` | rope, byte-indexed; still beta: pin exact version, only `buffer` touches its API |
| Graphemes | `unicode-segmentation` | cursor movement and deletion by grapheme cluster |
| Display width | `unicode-width` | `DisplayCol`: wide CJK, emoji, zero-width |
| Syntax highlighting | `tree-sitter` | lightweight and incremental; needs a grammar per language |
| LSP | `lsp-types` + `tokio` | JSON-RPC over stdio |
| Git | `git2` | mature libgit2 bindings |
| Diff | `imara-diff` | buffer/file diffs for the diff view |
| File watching | `notify` | |
| Config | TOML via `serde` | `~/.config/red/` |
| Fuzzy finding | `nucleo` | Cmd/Ctrl+P, command palette |
| Search | `regex` + `ignore` + `grep-searcher` + `grep-matcher` | find in file, Cmd+D, workspace search share one matcher (`search::Matcher` implements `grep_matcher::Matcher`; no `grep-regex`, whose word rules differ) |
| Task terminals | `portable-pty` + `vt100` + `tui-term` | PTY, VT parser, ratatui widget |
| Clipboard | `arboard` + OSC 52 | local clipboard; OSC 52 over SSH |
| Logging | `tracing` + `tracing-subscriber` + `tracing-appender` | file logs, rotation |

## Layout
```
┌──┬────────────┬──────────────────────┬──────────────────────┐
│A │ Side panel │ Editor group 1       │ Editor group 2       │
│c │ (explorer, │ [tab][tab][tab]      │ [tab][tab]           │
│t │  search,   │                      │                      │
│i │  git)      │                      │                      │
│v │            │                      │                      │
│i │            ├──────────────────────┴──────────────────────┤
│t │            │ Panel (problems / tasks)                    │
│y │            │                                             │
├──┴────────────┴─────────────────────────────────────────────┤
│ Status bar: branch · diagnostics · Ln/Col · lang · encoding │
└─────────────────────────────────────────────────────────────┘
```
- **Activity bar** (left edge): icons for Explorer, Search, Source Control
- **Side panel**: switches with the activity bar; can be toggled and resized
- **Editor area**: groups with tabs, split horizontally or vertically
- **Bottom panel**: Problems list, task terminals
- **Status bar**: git branch, diagnostic counts, cursor position, language mode, line ending

## Features

### Workspace
- A workspace is a root directory, optionally with a `.workspace` file (TOML) at its root
- Opening:
  - `red <dir>`: workspace root is `<dir>`
  - `red <file>` / `red <file>:<line>[:<col>]`: walk up from the file to the nearest `.workspace`; else use the file's directory
  - `red` with no args: current directory
- `.workspace` contents:
  - Settings overrides (tab size, wrap, …) on top of `~/.config/red/`
  - LSP servers per language (command, args, init options)
  - Task templates (`[tasks]`); merged with global `~/.config/red/tasks.toml`, workspace wins on name clash
  - Excluded paths for explorer, quick open and search (on top of `.gitignore`)
  - File history limits (`[history]`) — see [File history](#file-history)
  - File associations (e.g. `SConstruct`, `*.scons` → Python)
- Meant to be committed: no per-user state in it
- Per-user session state (open tabs, splits, layout) lives in `~/.local/state/red/`, keyed by workspace path
- Reloaded live when `.workspace` changes (via `notify`)
- Workspace root is the LSP `rootUri`, task default cwd and search root

**Workspace trust**
- `.workspace` can name commands (LSP servers, tasks); a cloned repo must not run them without asking
- First open of a workspace whose `.workspace` defines commands: trust dialog
  - Lists every command it would run
  - Choices: Trust / Restricted mode
- Restricted mode:
  - No LSP servers or tasks from `.workspace`; servers and tasks from global config still run
  - Editing, git, search all work
  - Status bar shows "Restricted"
- Trust is per user, stored in `~/.local/state/red/trust.toml`, keyed by workspace path + SHA-256 of the command entries
  - Both answers are stored, so Restricted isn't asked again on every open
  - Esc = Restricted
- Re-prompt when the command entries in `.workspace` change
- Command palette: "Manage Workspace Trust"

**Recent workspaces**
- Every start records the workspace root in `~/.local/state/red/recent.toml`: most recent first, no duplicates, capped at 50
- "Open Recent Workspace" in the command palette (Ctrl+R, as VS Code's Open Recent): a fuzzy list in the palette's input widget
  - Each row: folder name, then the path with `~` for home; the current workspace is left out
  - Folders that no longer exist are dropped from the list when it opens
  - Rows already open in a kitty window are marked
- Picking one:
  - Already open in red in a kitty window (any tab, any OS window): focus that window
  - Else, inside kitty with remote control: open a new kitty OS window running `red <path>`, with that folder as its cwd
  - Else (another terminal, or remote control off): status-bar message with the path and the `red <path>` command; nothing else happens
  - red never switches its own workspace in place
- Kitty integration (other terminals are ignored for now):
  - Inside kitty when `KITTY_WINDOW_ID` is set
  - red marks its window by setting a kitty user var `red_workspace` (OSC 1337 `SetUserVar`, base64 path) at startup, and clears it on exit and in the panic hook
  - Finding windows: `kitten @ ls` → JSON → the window whose `user_vars.red_workspace` is the path; focus by id with `kitten @ focus-window --match id:<id>`
    - Match on the user var, not the window title or cwd, so a shell `cd`'d into the folder isn't mistaken for red
  - New OS window: `kitten @ launch --type=os-window --cwd <path> red <path>`
  - `kitten` runs off the UI thread with a short timeout; failure (remote control off, `kitten` missing) falls back to the message, and is logged
  - Needs `allow_remote_control yes` (or `socket-only` with `listen_on`) in `kitty.conf`: documented in `docs/terminals/kitty.md`

### Files
- Encoding: UTF-8 only in v1; keep a UTF-8 BOM if present; other encodings open read-only with a warning
- Binary files (NUL bytes) are not opened
- Line endings: detect LF/CRLF per file, keep on save, show in status bar
- Keep (or not) the final newline as it was
- Save is atomic: write temp file in same dir → fsync → rename; keep permissions; write through symlinks to the target
- Saves run off the UI thread; each buffer tracks "save in flight"
- Changed on disk (via `notify`):
  - Ignore events caused by our own saves
  - Clean buffer: reload silently, keep cursors and scroll
  - Dirty buffer: banner — reload / keep mine / show diff
  - Deleted: mark tab as deleted; save recreates it
- Unsaved changes on close tab / quit: prompt save / discard / cancel
- Crash recovery: dirty buffers backed up periodically to `~/.local/state/red/backup/`; offer restore on next start

### File history
Go back to any earlier save of a file, independent of git.

**Storage**
- Per workspace, in `<root>/.red/history/`
- One folder per file, named by a hash of its workspace-relative path (no deep or long paths)
  - `entries.toml`: relative path + list of entries (id, time, size, content hash)
  - One snapshot file per entry, exact saved bytes (BOM and line endings kept)
- red creates `.red/.gitignore` containing `*`, so history is never committed
- `.red/` hidden from explorer, quick open and search; watcher events under it ignored
- Files outside the workspace: no history

**Recording**
- On every successful save: snapshot the bytes just written
- Skip if identical to the newest snapshot (content hash)
- Written off the UI thread, after the save's rename completes; a history failure never fails the save (log + status message)
- Explorer rename: history moves with the file; delete: history kept

**Limits** (`.workspace` `[history]`)
- `max_entries` per file (default 50; oldest dropped)
- `max_file_size` (default 5 MB; larger files not snapshotted)
- `enabled` (default true)

**UI**
- Command palette: "File History: Show" for the active file
- List of entries, newest first, with relative time and size ("3 min ago · 12 KB")
- Select an entry:
  - Compare: side-by-side diff, entry vs current buffer (reuses the git diff view)
  - Restore: replaces the buffer text as one undoable edit; nothing written until you save
- Also reachable from the explorer (on a file) and the tab bar

### Explorer (file tree)
- Lazy-loaded directory tree
- Keyboard: arrows to move, Enter to open, Left/Right to collapse/expand
- Mouse: click to select (folders toggle), double-click to open, scroll
- New, rename and delete for files and folders
  - Keys: Enter open, Space open keeping focus, F2 rename, Delete / Cmd+Backspace delete, Cmd+Alt+N new file, Cmd+Alt+Shift+N new folder (Ctrl+Alt on Linux)
  - New file/folder names may contain `/` to create nested folders
  - Delete moves to the system Trash
- Respects `.gitignore`
- Live refresh through `notify`
- Git status colouring on entries

### Editor
- Multiple cursors — see [Multi-cursor](#multi-cursor)
- Undo/redo with grouped edits
  - Consecutive typing is one step
  - A new step starts when: the cursor moves, ~1 s pause, switching between inserting and deleting
  - Paste, cut and each multi-cursor command are one step each
  - Each step stores the selections from before and after it
- Select with mouse drag, Shift+arrows, double/triple-click
- Clipboard — see [Clipboard](#clipboard)
- Find/replace in the file (regex, case, whole word)
  - Same matcher and options as [Workspace search](#workspace-search) and Cmd+D
  - Opens seeded with the selection, or the word under the cursor
  - Highlights every match in view; shows "3 of 41"; Enter / Shift+Enter next / previous
  - Replace one, or replace all as one undo step
  - Alt+Enter turns every match into a selection (as VS Code)
- Auto-indent, bracket matching, auto-close of pairs
- Line numbers, current-line highlight, soft wrap toggle
- Minimap: see [Stretch goals](#stretch-goals)

### Clipboard
- Local: system clipboard through `arboard`
- Over SSH (`SSH_TTY` set) or if `arboard` fails: copy through OSC 52
- Paste from the terminal (Cmd+V) arrives as a bracketed-paste event; handle it the same as clipboard paste
  - macOS: red leaves Cmd+V to the terminal, so paste works the same locally and over SSH (most terminals block OSC 52 reads)
  - Linux: Ctrl+V reads `arboard`; the terminal's own Ctrl+Shift+V still arrives as bracketed paste
  - Cmd+C / Cmd+X must reach red (the terminal's copy would take screen text, not the selection): add them to `docs/terminals/`
- Copy or cut of a whole line (empty selection) marks the clipboard entry; pasting it inserts above the cursor's line (VS Code behaviour)
- Copy/cut with an empty selection takes the whole line (VS Code behaviour)
- Multi-cursor copy/paste rules: see [Multi-cursor](#multi-cursor)

### Multi-cursor
The VS Code behaviour, especially Cmd+D and Cmd+Shift+L, is a must-have.

**Model**
- A view holds a list of selections, never a single cursor; one cursor is just a list of length 1
- Each selection has an `anchor` and a `head` (`ByteIdx` — see [Text positions](#text-positions)), plus a remembered `DisplayCol` for up/down movement
- One selection is primary: it's the one scrolled into view and the one shown in the status bar
- Keep the list sorted; after every command merge selections that overlap or sit at the same position
  - Two non-empty selections that only touch stay separate (e.g. Cmd+D on `a` in `aaaa` gives 4); a cursor touching a selection merges into it (VS Code)
- Apply an edit at every selection in one transaction, working from the last to the first so earlier offsets stay valid
- One undo step covers the whole multi-cursor edit and restores the selection list from before it; redo restores the selection list from after it

**Commands**
| Action | macOS | Linux |
|---|---|---|
| Add selection at next match | Cmd+D | Ctrl+D |
| Select all matches | Cmd+Shift+L | Ctrl+Shift+L |
| Skip this match, go to next | Cmd+Alt+D | Ctrl+Alt+D |
| Undo last cursor change | Cmd+U | Ctrl+U |
| Add cursor above / below | Cmd+Alt+Up/Down | Ctrl+Alt+Up/Down (_clashes with GNOME workspace switch_) |
| Cursor at each selected line's end | Shift+Alt+I | Shift+Alt+I |
| Add / remove cursor | Alt+Click | Alt+Click |
| Box (column) select, mouse | Shift+Alt+drag | Shift+Alt+drag |
| Box (column) select, keyboard | Cmd+Shift+Alt+arrows | Ctrl+Shift+Alt+arrows |
| Back to one cursor | Esc | Esc |

- Shift+drag is often grabbed by the terminal for its own selection, so the keyboard box select is the reliable path

**Cmd+D in detail (copy VS Code exactly)**
- With an empty cursor: select the word under it, and match **whole words only** from then on
- With text selected: search for that text literally, with the find widget's case and whole-word options (both off by default, so case-insensitive — VS Code's `findOptions` mode)
- Search starts after the most recently added selection and wraps around the file
- Skip matches that are already selected
- Scroll to show the new selection
- Cmd+Shift+L uses the same matching rules but adds every match at once

**Editing at many cursors**
- Typing, Backspace, Delete, Enter, Tab and indent all run at every cursor
- Word and line movement (Alt/Cmd+arrows, Home/End) move every cursor
- Auto-close pairs and auto-indent apply at each cursor
- Copy joins the selections with newlines
- Paste: if the clipboard has as many lines as there are cursors, give each cursor one line; otherwise paste the whole thing at every cursor
- LSP completion is inserted at every cursor

**Rendering**
- The terminal has only one hardware cursor: put it on the primary selection and draw the others as reversed cells
- Draw a selection background for every selection
- Status bar shows "N selections" when there's more than one

**Performance**
- Cmd+Shift+L on a 1 MB file can create a lot of selections: find matches on the rope without copying it to a string, and apply all edits as one batch
- _TBD: cap the number of selections? VS Code has no cap_

### Splits
- Split right / split down
  - Layout is a binary split tree (`layout::SplitTree`); 1-cell divider lines between groups
  - New group shows the active tab's buffer, same cursor and scroll, and gets focus
- Focus groups by keyboard and mouse
  - Cmd+1/2/3 in reading order; one past the last group splits right (as VS Code)
  - Click in a group's tab bar or text focuses it; mouse wheel scrolls the group under the mouse without focusing
- Resize splits by dragging dividers
  - Minimum group size 12 cols × 4 rows
- Same buffer open in several views
  - Edits (incl. undo/redo) map every other view's selections and viewport through the changes
  - Cursors at an insertion point in other views stay before the inserted text
- Closing a group's last tab closes the group (never the last group)
- Unsaved-changes prompt only when closing the last tab showing the buffer, across all groups
- Inactive group's active tab drawn dimmer; terminal cursor only in the focused group

### Syntax highlighting
- tree-sitter grammars + `highlights.scm` queries per language
- Start with C/C++ and Python grammars to match the first LSPs
- Re-parse incrementally on edit
- Initial parse off the UI thread; show plain text until it's ready
- Run highlight queries on the visible range (plus a margin) only
- Above 1 MB: best effort; highlighting off past a configurable limit
- One built-in dark theme, modelled on VS Code Dark+
- Truecolor; fall back to 256 colours where the terminal lacks it
- Custom themes: see [Stretch goals](#stretch-goals)
- Detect language from file extension and shebang

### LSP
- First servers: `clangd`, `pyright`
- SCons: treat `SConstruct`, `SConscript` and `*.scons` as Python
  - SCons injects globals (`Environment`, `Import`, …) that pyright flags as undefined; ship a `__builtins__.pyi` with them (pyright treats its names as built-ins)
  - _Verify early: pyright analyzes files without a `.py` extension when opened with `languageId: python`_
- Server per language set in config / `.workspace`
- Incremental text sync (`didChange` with ranges), never full text per keystroke
- Request `positionEncoding: utf-8`; fall back to UTF-16 — see [Text positions](#text-positions)
- v1 features:
  - Diagnostics (squiggles plus the Problems list)
  - Completion
  - Hover
  - Go to definition (F12)
  - Find references (Shift+F12)
  - Rename symbol (F2)
  - Format document
- Later: code actions, signature help, inlay hints, semantic tokens

### Git (view and stage only)
Everything else (commit, branch, push, pull, …) is done from the command line.
- Source Control panel: staged, unstaged and untracked files, kept up to date through `notify`
- Gutter markers for added, modified and deleted lines
- Diff view, one file at a time:
  - Open it by clicking or pressing Enter on a file in the Source Control panel, or with "Open Changes" in the command palette for the current file
  - An unstaged file shows the working tree against the index; a staged file shows the index against HEAD
  - Side by side (target terminal width > 120 columns)
  - Opens in an editor tab, so it can be split like any other tab
  - Jump to next / previous change
- Stage / unstage a whole file or a single hunk
  - From the diff view and from the gutter
  - Hunks always come from the file on disk, never the buffer
  - Before staging: no outstanding writes to the file
    - Buffer dirty → prompt to save first
    - Save in flight → wait for it to finish
    - Then recompute the diff from disk and stage
- Current branch shown in the status bar (read-only)

### Tasks (throwaway terminals)
- A terminal is spawned for one task, then thrown away when it finishes
- Templates: name, command, cwd, env — in `.workspace` and/or `~/.config/red/tasks.toml` (see [Workspace](#workspace))
- Run from the command palette: "Run Task" (_TBD: shortcut_)
- Each run gets its own tab in the bottom panel
- Runs in a PTY, so tools keep their colours and progress bars
- On exit: show the exit code and keep the output until the tab is closed
- Kill a running task
- Out of scope: a general-purpose interactive shell
- Later: parse compiler errors into the Problems list (`file:line:col`)

### Command palette and quick open
- Cmd/Ctrl+Shift+P: command palette — every named `Command`, with its current binding
- Cmd/Ctrl+P: quick open file (fuzzy); accepts `file:line[:col]` like the command line
- Ctrl+G: go to line, in the same input widget
- Quick open skips what the explorer skips: `.gitignore`, `.workspace` excludes, `.red/`

### Workspace search
- Cmd/Ctrl+Shift+F: Search view in the side panel (the activity bar's Search icon)
- Modes, toggled by buttons in the input or by key (see [Keybindings](#keybindings-vs-code-defaults)):
  - Plain text (default): the query matches literally
  - Case sensitive: off by default
  - Regex: Rust `regex` syntax; an invalid pattern shows the error under the input and nothing runs
  - Toggles combine (e.g. case-sensitive regex)
- Same matcher (`search` module) as find in file, so each option means the same thing in both
- Scope: workspace root; skips `.gitignore`d files, `.workspace` excludes, `.red/` and binary files
- Open buffers with unsaved changes are searched from the buffer, not the disk
- Off the UI thread (`ignore` walker + `grep-searcher` in `spawn_blocking`)
  - Results stream in as files finish
  - Typing restarts the search (short debounce) and cancels the running one
- Results:
  - Grouped by file with a match count; each line shows its match highlighted
  - Click or Enter opens the file in the focused group with the match selected
  - F4 / Shift+F4: next / previous result
  - Capped at 20,000 matches, with a "results truncated" note (as VS Code)
- Matches stay within one line in v1
- Later: replace across files, include/exclude glob fields, whole-word toggle

## Keybindings (VS Code defaults)
| Action | macOS | Linux |
|---|---|---|
| Command palette | Cmd+Shift+P | Ctrl+Shift+P |
| Quick open | Cmd+P | Ctrl+P |
| Save | Cmd+S | Ctrl+S |
| Close tab | Cmd+W | Ctrl+W |
| Toggle sidebar | Cmd+B | Ctrl+B |
| Focus explorer | Cmd+Shift+E | Ctrl+Shift+E |
| Source control | Ctrl+Shift+G | Ctrl+Shift+G |
| Search in workspace | Cmd+Shift+F | Ctrl+Shift+F |
| Split editor right / down | Cmd+\ / Cmd+Alt+\ | Ctrl+\ / Ctrl+Alt+\ |
| Focus group 1/2/3 | Cmd+1/2/3 | Ctrl+1/2/3 |
| Find / replace | Cmd+F / Cmd+Alt+F | Ctrl+F / Ctrl+H |
| Toggle case / whole word / regex (find, search) | Cmd+Alt+C / W / R | Alt+C / W / R |
| Next / previous search result | F4 / Shift+F4 | F4 / Shift+F4 |
| Go to line | Ctrl+G | Ctrl+G |
| Open recent workspace | Ctrl+R | Ctrl+R |
| Comment line | Cmd+/ | Ctrl+/ |
| Move line | Alt+Up/Down | Alt+Up/Down |
| Duplicate line | Shift+Alt+Up/Down | Shift+Alt+Up/Down |
| Undo / redo | Cmd+Z / Cmd+Shift+Z | Ctrl+Z / Ctrl+Shift+Z |
| Cut / copy / paste | Cmd+X / Cmd+C / Cmd+V | Ctrl+X / Ctrl+C / Ctrl+V |
| Add next match / all matches | Cmd+D, Cmd+Shift+L | Ctrl+D, Ctrl+Shift+L |
| Toggle panel | Cmd+J | Ctrl+J |

- Configurable through `keybindings.toml`
- Single key combos only — no chords
- A binding can be limited to a context (editor, find widget, palette, explorer, …) so keys like Esc do the right thing
- macOS uses Cmd wherever VS Code on macOS does (a few stay Ctrl, as in VS Code: Go to line, Source control)
- Linux uses Ctrl

### Terminals
- Kitty keyboard protocol required; without it Ctrl+Shift+X = Ctrl+X, Ctrl+H = Backspace, Ctrl+J = Enter, Ctrl+3 = Esc
- Supported: Kitty, Ghostty, WezTerm, iTerm2 (CSI u on), Alacritty
- Startup: query protocol support (`CSI ? u`); if missing, show a warning that some keys won't work
- Cmd arrives as the Super modifier; crossterm reads it
- Terminals grab many Cmd keys themselves (Cmd+C/V/X/Z/W/Q/T/N; Cmd+K clears screen; Cmd+D splits panes in iTerm2/Ghostty)
  - Ship tested config snippets per supported terminal (`docs/terminals/`) that pass through every Cmd key in the default keymap
  - Also clashes: Cmd+1/2/3 switch terminal tabs, Cmd+F opens terminal find, Cmd+J/B/P in some terminals
  - Snippets also turn on Option-as-Alt on macOS (otherwise Option types ∂, ˆ, …)
- Basic terminals (Terminal.app, etc.): see [Stretch goals](#stretch-goals)

## Architecture
- `app` — event loop, owns all state, sends commands where they need to go
- `workspace` — root detection, `.workspace` loading and live reload, session state paths
- `buffer` — rope, undo history, file I/O, text position types and conversions
- `view` — selection list, scroll; many views can share one buffer
- `selection` — selection list type, merge and transform rules; every command works on the list from day one
- `layout` — tree of splits and groups, maps mouse hits to regions
- `ui` — ratatui widgets (tree, tabs, editor, status bar, palette)
- `clipboard` — `arboard` / OSC 52 behind one interface
- `search` — the one matcher (plain / regex, case, whole word) over a rope; used by find, Cmd+D and workspace search
- `syntax` — highlighting, cached per line or range
- `lsp` — client manager, a tokio task per server, channels back into the app
- `git` — status, diff, stage/unstage files and hunks
- `config` — settings, keybindings
- `history` — file history: snapshot on save, index, retention, restore
- `theme` — maps highlight names and UI elements to colours; built-in dark theme only for now
- Every user action is a named `Command`, so keybindings and the palette go through the same path

### Text positions
- One set of position types, used by every module — no raw `usize` offsets across module boundaries
- Newtypes so mixing units is a compile error:
  - `ByteIdx` — UTF-8 byte offset; **the canonical unit** (selections, edits, undo, views, tree-sitter, search, git hunks)
    - Always on a char boundary; cursors always on a grapheme boundary
  - `LspPos` — LSP line + column in the negotiated encoding (UTF-8 = no conversion; UTF-16 fallback converts)
  - `DisplayCol` — screen column: tabs expanded, wide CJK = 2, grapheme clusters (emoji, combining marks) as one unit
- All conversions live in `buffer::pos`; nothing else converts
- Convert at the boundary: on the way in to / out of tree-sitter, LSP, git, rendering

### Async and blocking work
- tokio runtime for LSP, file watching, git, file I/O, search
- UI thread never blocks; blocking work goes to `tokio::task::spawn_blocking`:
  - `git2` calls (status, diff, stage)
  - File reads and atomic saves
  - Workspace search and directory walks (`ignore`, `grep-searcher`)
  - tree-sitter initial parse
- `git2::Repository` is `Send` but not `Sync`: open one per blocking task, or own it on a single git thread
- `notify` callbacks are synchronous: forward events into a tokio channel
- Main loop: `tokio::select!` over crossterm `EventStream` and app channels
- Render when state is dirty; coalesce bursts of events into one frame; no fixed tick

### Logging
- `tracing` everywhere; never write to stdout/stderr while the UI is up (corrupts the screen)
- Log file: `~/.local/state/red/logs/red.YYYY-MM-DD.log`, rotated daily, keep last 7
- Level from `--log-level` or the `RED_LOG` env var (`EnvFilter` syntax, e.g. `RED_LOG=red::lsp=trace,red::git=debug`); default `info`
- Targets are module paths (`red::app`, `red::buffer`, …) so each can be turned up alone; plus `input` and `panic`
- LSP:
  - `lsp::wire` at `trace`: every JSON-RPC message in and out, tagged with server name
  - Server stderr captured and logged under `lsp::stderr`
- Timing:
  - Span per `Command` with duration
  - Warn on slow frames (> 16 ms render) and slow blocking tasks
- Key and mouse events at `trace` under `input` (for keybinding / terminal debugging)
- Command palette: "Open Log File", "Set Log Level"

### Panic hook
- Installed at startup, before entering raw mode
- On panic:
  - Back up dirty buffers to `~/.local/state/red/backup/` (best effort)
  - Restore the terminal: pop kitty keyboard flags, disable mouse capture and bracketed paste, leave alternate screen, disable raw mode
  - Write the panic message and backtrace to the log
  - Then print them to stderr

## Milestones
1. ✅ **Skeleton**: open a file, edit, save, scroll, status bar
   - Selection list from the start; undo transactions and grouping
   - Text position types (`buffer::pos`); grapheme-aware movement, display width
   - Files: UTF-8 check (keep BOM), refuse binary, detect and keep LF/CRLF, keep final newline
   - Unsaved-changes prompt on quit
   - Kitty keyboard protocol on; terminal check at startup
   - Panic hook; logging; atomic save
   - Done when: a 1 MB CRLF file can be opened, edited, undone/redone and saved back byte-identical, with no lag; a panic leaves the terminal usable
2. ✅ **Layout**: workspace root, `.workspace` loading and trust dialog, activity bar, explorer tree, tabs, mouse support
   - Deferred: "Manage Workspace Trust" (needs the palette, M5), git colouring in the explorer (M10), external changes to open files (M10)
3. ✅ **Splits**: editor groups, resizing, focus switching
   - Split down is Cmd+Alt+\\ (no chords, so not VS Code's Cmd+K Cmd+\\)
   - Deferred: moving tabs between groups, drag-and-drop tabs, keyboard resize of splits (M14)

Each milestone below that adds a Cmd binding also updates `docs/terminals/` (the `terminal_docs_list_every_cmd_binding` test fails otherwise) — see [Terminals](#terminals).

4. ✅ **Clipboard**: copy, cut, paste — see [Clipboard](#clipboard)
   - Needs: selection list and undo grouping (M1), bracketed paste (M1, `edit::paste`)
   - Needed by: multi-cursor copy/paste (M6), line commands (M6)
   - `clipboard` module: `arboard` locally; OSC 52 when `SSH_TTY` is set or `arboard` fails
   - Copy / cut commands (Cmd+C / Cmd+X); cut is one undo step, like paste
   - Empty selection copies / cuts the whole line; pasting that line inserts above the cursor's line
   - Copy works on the whole selection list from the start (joined with newlines), so M6 only adds the paste spreading
   - Paste: Cmd+V stays with the terminal on macOS (bracketed paste); Ctrl+V reads `arboard` on Linux
   - Terminal snippets: pass Cmd+C / Cmd+X through
   - Done when: copy in red → paste in another app → copy there → paste back into red, both locally and over SSH; empty-selection copy/cut/paste matches VS Code
   - Implemented; automated tests cover the editing rules with an in-memory clipboard. Still to check by hand: the round trip with another app locally (`arboard`) and over SSH (OSC 52), incl. inside tmux
5. ✅ **Palette**: command palette, quick open, keybinding config with contexts — see [Command palette and quick open](#command-palette-and-quick-open), [Keybindings](#keybindings-vs-code-defaults)
   - Needs: named `Command`s (M1), workspace excludes and the explorer's `ignore` walk (M2)
   - Needed by: the find widget and Search view (M6, M7 key contexts), F2 meaning rename-file in the explorer and rename-symbol in the editor (M9), palette commands in M9–M11
   - Command palette over every `Command`, fuzzy via `nucleo`, showing each one's binding
   - Quick open: walk off the UI thread, fuzzy, `file:line[:col]`; skips `.red/`
   - Go to line (Ctrl+G) in the same input widget
   - `keybindings.toml`: user bindings over the defaults, each limited to a context (editor, find widget, palette, explorer, search, …)
   - Keymaps stay data, chosen at startup (keeps [Basic terminals](#basic-terminals) open)
   - Commands owed from earlier: "Manage Workspace Trust" (M2), "Open Log File", "Set Log Level" ([Logging](#logging))
   - Done when: every command can be run from the palette; a binding in `keybindings.toml` replaces the default and shows in the palette; Esc closes the palette and does nothing else
   - Also: F1 opens the palette (VS Code default); the explorer hides `.red/` too (shared walker with quick open)
   - Deferred: live preview for go to line; unbinding in `keybindings.toml`; live reload of `keybindings.toml` (M14); find-widget and search contexts arrive with M6 / M7
   - Still to check by hand: Cmd+P / Cmd+Shift+P reach red in each terminal (Ghostty and WezTerm palettes, iTerm2 Print menu); quick open speed on a large repo
6. ✅ **Multi-cursor and find**: Cmd+D, Cmd+Shift+L, add above/below, Alt+Click, box select, paste spread across cursors, find/replace in the file — see [Multi-cursor](#multi-cursor), [Editor](#editor)
   - Needs: selection list (M1), clipboard (M4), key contexts (M5)
   - Needed by: workspace search (M7, the matcher), auto-close and auto-indent at every cursor (M8), completion at every cursor (M9)
   - `search` module first: plain / regex, case, whole word, matching on the rope without copying it into a string
     - Cmd+D, Cmd+Shift+L and the find widget all use it
   - Every command in the multi-cursor table; Cmd+D matching exactly as VS Code
   - Find/replace widget (Cmd+F / Cmd+Alt+F): case / whole word / regex toggles; replace all is one undo step; Alt+Enter selects every match
   - Line commands at every cursor: move line (Alt+Up/Down), duplicate line (Shift+Alt+Up/Down)
   - Paste spreads one line per cursor when the counts match ([Clipboard](#clipboard))
   - Rendering: extra cursors as reversed cells, a selection background for each, "N selections" in the status bar
   - Same buffer in several groups (M3): every view's selections still map through multi-cursor edits
   - Done when: Cmd+Shift+L on a 1 MB file with tens of thousands of matches, then typing, stays smooth; one undo restores the selection list from before; replace-all on that file is one undo step
   - Tasks (one agent and one commit each):
     - ✅ 6a: `search` module — `search::Matcher` (plain / regex, case, whole word as VS Code), find next / previous with wrap, find all / in range, line by line on the rope; implements `grep_matcher::Matcher` for M7
     - ✅ 6b: Cmd+D, Cmd+Shift+L, Cmd+Alt+D, Cmd+U (cursor undo), Esc to one cursor; drawing extra cursors and selections, "N selections"; the 1 MB performance target
       - `multicursor::Session` (VS Code's MultiCursorSession) and `app/cursors.rs`; `Selections` tracks the newest selection; `View::CursorHistory` (50 states, cleared by any text change)
       - Session ends on any other selection change, Cmd+U, or focus leaving the editor; skips matches already selected (VS Code would merge)
       - Perf: `ChangeSet::mapper()` (binary search) for other views, one-pass rope rebuild from 256 changes, editor draws only visible selections; 1 MB / 63k matches: Cmd+Shift+L 3 ms, 5.5 ms a key, undo 15 ms
       - Not done: cursor redo (no VS Code default key); word characters are `is_word_char`, not VS Code's `wordSeparators`
       - Still to check by hand: Cmd+D / Cmd+Shift+L / Cmd+Alt+D / Cmd+U reach red in each terminal (Ghostty and iTerm2 split on Cmd+D; iTerm2's Cmd+U menu item is unverified)
     - ✅ 6c: add cursor above / below, Shift+Alt+I, Alt+Click, box select (mouse and keyboard), paste spread across cursors, move / duplicate line at every cursor
       - `column.rs` (add cursor above / below, cursors at line ends, box select as VS Code's ColumnSelection) and `lines.rs` (move / copy lines as blocks, one undo step, no-op adds none); `clipboard::Copied { text, whole_line, pieces }`; `pos::Line` / `LineWalker` / `line_bounds` and an ASCII fast path for 60k-cursor speed
       - Paste spread as VS Code's `multiCursorPaste: spread`; copying from several empty cursors is no longer whole-line (VS Code marks only one cursor), changing M4's behaviour
       - Also (separate commit before 6c): typing at 63k cursors sped up for slower machines (one-string `ChangeSet`, `Arc` change sets, copy-on-write `Selections`)
       - Not done: move lines doesn't re-indent (M8); middle-button box select; a mix of empty and non-empty selections copies `""` for the empty ones (VS Code copies their lines) and cut leaves those lines
       - Still to check by hand: Shift+Alt+drag and Alt+Click reach red (terminals keep these for their own selection); Cmd+Alt+Up/Down vs Ghostty / iTerm2 pane keys; Cmd+Shift+Alt+arrows / PageUp / PageDown arrive with Super; Shift+Alt+I needs Option-as-Alt; on Linux GNOME takes Ctrl+Alt+Up/Down and Ctrl+Shift+Alt+arrows
     - ✅ 6d: find / replace widget (Cmd+F / Cmd+Alt+F) and its key context; owns the find options that Cmd+D on selected text borrows (`App::find_options`); replace all as one undo step; Alt+Enter selects every match; marks M6 ✅
       - `find.rs` (model), `app/find.rs`, `ui/find.rs`; `find` and `replace` key contexts (replace → find → global) for `keybindings.toml`; match cache keyed on `Buffer::revision()`, drawn only in view; `search::Replacement` (`$1`, `$&`, `$$`, `\n` …)
       - Keys: Cmd+F / Cmd+Alt+F (Ctrl+F / Ctrl+H on Linux), Enter / Shift+Enter and F3 / Shift+F3, toggles Cmd+Alt+C/W/R (Alt+C/W/R on Linux) also from the editor, Cmd+Shift+1 replace one, Cmd+Alt+Enter replace all (one undo step), Alt+Enter selects every match; first Esc in the editor closes the widget
       - `KeyCombo` maps a shifted US-layout digit with Cmd/Ctrl/Alt back to digit + Shift (`cmd+shift+1`); on non-US layouts with `!` on its own key, Ctrl+`!` runs Replace One
       - 1 MB / 63k matches (Linux i7-2600K): find key + frame 10 ms, replace all + frame 11 ms, undo 4 ms
       - Not done: Cmd+G / Cmd+Shift+G (Ctrl+G is go to line on Linux); undo inside the widget's inputs; find/replace history, preserve case, find in selection; a match under the widget isn't scrolled clear; one widget for the app rather than per editor; the count doesn't refresh on background events (e.g. a file opening) until the next key
       - Still to check by hand: Cmd+F vs each terminal's own search (Ghostty, WezTerm, Alacritty, iTerm2's Find… menu); Cmd+Shift+1 arrives as Cmd+`!`; Cmd+Alt+C/W/R/F/Enter need Option-as-Alt; Alt+Enter and Shift+Enter keep their modifiers; on Linux Ctrl+H needs the kitty protocol (else Backspace)
7. ✅ **Workspace search**: plain text, case-sensitive and regex search across the workspace — see [Workspace search](#workspace-search)
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
     - ✅ 7a: search engine, no UI: a `workspace_search` module that walks the root with the shared `ignore` walker (excludes, `.red/`, skip binary) and runs `grep-searcher` with `search::Matcher` in `spawn_blocking`; dirty buffers searched from memory instead of disk; a cancel token; results stream back as `AppEvent` batches grouped by file (line, columns, line text); 20,000 cap with a truncated flag. Tests compare all three modes against a naive reference search (no `rg` binary on the Linux box; check once by hand against `rg -F` / `-s` / regex on this repo)
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
       - Still to check by hand: Cmd+Shift+F reaches red in each terminal (iTerm2 may bind ⇧⌘F to Find Globally; WezTerm may need `key = 'F'`); on Linux Ctrl+Shift+F needs the kitty protocol (else it arrives as Ctrl+F, the find widget)
     - ✅ 7c: navigating results: Up/Down in the list, Enter or click opens the match in the focused group with it selected, F4 / Shift+F4 step through results from anywhere; end-to-end "done when" tests (the three modes, an unsaved edit in the results, typing never stalls); marks M7 ✅
       - The list's selection is (file, row in file) plus the match index, stable while results stream in; new `searchResults` key context (falls back to `search`); 10 `search.*` list / open / next / previous commands
       - Enter and double click open the match in the last focused group and focus the editor; single click and F4 keep focus; Enter on a file row opens its first match; typing or pasting in the list goes back to the query
       - Opening with a position: `App::opening` (by path) holds a `Goto` (`Cursor` for `file:line:col`, `Match { line, range }` for search) until the file loads; clamped to the current text; centred with `reveal_range`
       - Done-when tests: three modes vs an independent reference (`workspace_search::reference`) on a copy of `src/` and `docs/`, driven through the view; an unsaved edit found and opened; a 5-key burst during a 10 MB search (0.2 ms + frame, release) ends with only the final query's hits
       - Not done: collapsing file rows, whole word, replace across files, include / exclude globs (all "Later"); F4 across two files that are still loading can leave the later-finishing one active
       - Still to check by hand: F4 / Shift+F4 reach red in each terminal (some desktops take F4); Enter, click and double click in a real terminal; results open in the last focused split
8. **Highlighting**: grammars and the built-in dark theme — see [Syntax highlighting](#syntax-highlighting), [Themes](#themes)
   - Needs: byte-indexed edits (M1) to feed tree-sitter's incremental parse; `theme` keys (M1); multi-cursor (M6) so the language-aware editing below runs at every cursor
   - Needed by: LSP (M9, language detection gives the `languageId`)
   - Language detection: file extension, shebang, `.workspace` file associations (`SConstruct` → Python); language mode in the status bar
   - C/C++ and Python grammars with `highlights.scm`; initial parse off the UI thread, incremental after
   - Then Rust, Markdown, YAML, JSON, XML, TOML, shell, CMake, Makefile and Dockerfile (8f)
   - Highlight queries on the visible range plus a margin; off past the size limit
   - Dark+ theme through named keys only; truecolor with 256-colour fallback
   - Language-aware editing: comment line (Cmd+/), auto-indent, bracket matching, auto-close pairs
   - Keep [Minimap](#minimap) open: highlight spans can be read for any line range
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
       - red's own query additions in `syntax/queries/` (control keywords purple, builtin types / constants, `this` / `self`, …); 20 Dark+ token keys in the theme
       - While a background reparse runs, the last spans are mapped through each edit (text typed at a span's edge stays plain until the new tree), so colours never flash off
       - 1 MB C++ (release, i7-2600K): open draws plain text in 1 ms and stays under 3 ms a frame during the 620 ms parse; typing avg 5.3 ms, worst ~11 ms; paging worst 2.2 ms; a 1 MB single line ~10 ms
       - Not done: an unclosed `/*` isn't coloured as a comment (tree-sitter error recovery); C macro calls and goto labels; semantic highlighting
       - Still to check by hand: colours against VS Code Dark+; 256-colour mode with `COLORTERM` unset; a big C++ file opening plain then coloured without a stall
     - ✅ 8c: comment line (Cmd+/) and auto-close pairs (brackets and quotes, type-over, delete the pair, wrap a selection) at every cursor, from a per-language config (comment tokens, pairs)
       - `syntax/config.rs` `LanguageConfig` (VS Code's C/C++, Python and plaintext language configurations: comment token, auto-close pairs with `notIn`, surround pairs, `autoCloseBefore`, Python string prefixes); `comment.rs` (toggle per selection as VS Code's LineCommentCommand); `pairs.rs` (type-over → auto-close → wrap; `delete_pair`; `View::auto_closed` tracks red-inserted closers so only those are typed over / deleted, dropped on any other change or a line break inside)
       - `[settings] auto_close_pairs` (default true) turns off auto-close, type-over, pair delete and wrap; comment line unaffected
       - 63k cursors (release): `(` 9 ms, type-over 10 ms, pair Backspace 11 ms; Cmd+/ on 84k lines ≤ 46 ms
       - Not done (VS Code differences): no auto-close before an unmatched closing bracket (needs bracket matching, 8e); no stand-in-character quote check; without a current tree the "not in string / comment" rule is skipped; comment indentation isn't rounded to the indent size
       - Still to check by hand: Cmd+/ reaches red (iTerm2's Find Cursor menu item; layouts where `/` is shifted may send Cmd+Shift+7; on Linux without the kitty protocol Ctrl+/ arrives as Ctrl+7)
     - ✅ 8d: user settings file: `~/.config/red/settings.toml` (`paths::config_dir()`, the same `[settings]` keys as `.workspace`) loaded at startup like `keybindings.toml`, errors reported via notify; `.workspace` `[settings]` override it per key; `auto_close_pairs` (added in 8c) and the other settings work from either. Live reload stays in M14
       - `config/settings.rs`: `SettingsOverrides` (shared with `.workspace`), `layer(user, workspace)` is the one precedence function (defaults ← `settings.toml` ← `.workspace`, per key); `TEMPLATE` documents every key (a compile-time check keeps it complete)
       - `settings.toml` is lenient (bad keys / types reported and skipped, a syntax error keeps the defaults); `.workspace` stays strict; load errors from settings and keybindings show together after start (`App::report_errors`)
       - "Preferences: Open User Settings" (palette) creates the file from the template if missing (never overwrites) and opens it
     - ✅ 8e: auto-indent (Enter keeps the indent, indents after an opening bracket or Python `:`, outdents on a closing bracket) and bracket matching (highlight the pair at the cursor) at every cursor, each with an on / off setting
       - `indent.rs` (Enter: carry the indent up to the cursor, +1 after a real-token opener, closer on its own line between a pair, C/C++ `indentationRules` (increase / decrease patterns), Python block `:` and dedent after `return` / `pass` / …; outdent a typed closer to its opener's indent; Python `else` / `elif` / `except` / `finally` dedent); `brackets.rs` (bracket after the cursor first, then before; tree tokens with a ±16 KB text scan fallback; up to 64 on-screen cursors; cached in `View::brackets`; `editor.bracketMatch`); `syntax/tokens.rs` forward-only token walk
       - Settings `auto_indent` and `match_brackets` (default true), from `settings.toml` or `.workspace`
       - 65k cursors (release): Enter ≤ 53 ms, `}` outdent 17 ms; bracket-matching frame 3.4 ms
       - Not done (VS Code differences): C++ `onEnterRules` continuing `// ` comments and outdenting after a brace-less `if` / `for` / `while`; no indentation normalisation or `trimAutoWhitespace`; outdent applies per cursor; without a tree brackets in strings count; Go to Bracket; no auto-close before an unmatched closer
     - 8f: more languages: Rust, Markdown, YAML, JSON (and JSONC), XML, TOML, Bash / shell, CMake, Makefile, Dockerfile — each a tree-sitter grammar crate compatible with the pinned tree-sitter version, detection (extensions, file names like `Makefile` / `CMakeLists.txt` / `Cargo.lock` / `Dockerfile` / `Containerfile` / `*.dockerfile`, shebangs like `sh` / `bash`), a `highlights.scm` mapped onto the existing Dark+ keys (adding keys only where needed, e.g. Markdown headings / emphasis / links, XML tags / attributes), and a `LanguageConfig` (comment tokens, pairs) so comment line and auto-close work; a language with no usable grammar crate is dropped and noted; marks M8 ✅
9. **LSP**: clangd and pyright — see [LSP](#lsp), [Text positions](#text-positions)
   - Needs: language detection (M8), `LspPos` in `buffer::pos` (M1), workspace trust (M2: servers named in `.workspace` only run when trusted), key contexts (M5), multi-cursor (M6), the results list (M7)
   - First: check that pyright analyzes files without a `.py` extension when opened with `languageId: python`
   - One tokio task per server; `positionEncoding: utf-8` with a UTF-16 fallback; incremental `didChange`
   - Bottom panel built here: Problems list; Cmd+J toggles it (Tasks, M12, adds its own tabs)
   - Diagnostics as squiggles, in Problems and as counts in the status bar
   - Completion (inserted at every cursor), hover, go to definition (F12), find references (Shift+F12), rename symbol (F2), format document
   - SCons: ship the `__builtins__.pyi`
   - Logging: `lsp::wire` and `lsp::stderr`
   - Done when: on a C++ project with `compile_commands.json` and on an SCons project, diagnostics follow edits without full-text syncs; all seven features work in both
10. **Git**: SCM panel, gutter markers, side-by-side diff view, stage/unstage hunks — see [Git](#git-view-and-stage-only)
    - Needs: the `notify` watcher (M2), off-thread saves with "save in flight" (M1), editor tabs that split (M3), the side panel (M2)
    - Needed by: file history (M11 reuses the diff view)
    - `git2` work on a single git thread or a fresh `Repository` per blocking task ([Async](#async-and-blocking-work))
    - Source Control panel; gutter markers; diff view as an editor tab with next/previous change
    - Stage / unstage file or hunk, always from disk after any pending save
    - Owed from M2: git colouring in the explorer; branch in the status bar
    - Files changed on disk ([Files](#files)), owed from M2: ignore our own saves, reload clean buffers silently, banner for dirty ones (reload / keep mine / show diff, using the diff view), deleted-file tabs
    - Done when: staging a single hunk from the gutter gives the same index as `git add -p`; changing a file outside red updates the gutter, the panel and the open buffer
11. **File history**: snapshot on save, history list, compare (diff view), restore — see [File history](#file-history)
    - Needs: the atomic save path (M1), the palette (M5), the diff view (M10), explorer rename (M2)
    - `history` module: `.red/history/` with its `.gitignore`, snapshot after the save's rename, skip duplicates, `[history]` limits
    - Ignore watcher events under `.red/` (the explorer and quick open already hide it, M5; search skips it, M7)
    - "File History: Show" in the palette, explorer and tab bar; compare; restore as one undo step
    - Done when: a history write failure never fails a save; restore and then undo gives back the exact buffer
12. **Tasks**: templates, PTY runner, task tabs in the bottom panel — see [Tasks](#tasks-throwaway-terminals)
    - Needs: workspace trust (M2), the palette (M5), the bottom panel (M9)
    - Templates from `.workspace` `[tasks]` and `~/.config/red/tasks.toml`, workspace wins on a clash
    - "Run Task" from the palette; one tab per run; PTY via `portable-pty` + `vt100` + `tui-term`
    - Exit code shown, output kept until the tab closes; kill a running task
    - Done when: a colourful build (e.g. `cargo build`) renders as in a real terminal; restricted mode runs no `.workspace` task
13. **Sessions and recent workspaces**: restore open tabs, splits and layout per workspace; jump between workspaces — see [Workspace](#workspace) (incl. Recent workspaces), [Files](#files)
    - Needs: the split tree (M3), state paths (M2), the palette (M5), the panic hook (M1)
    - Session state in `~/.local/state/red/`, keyed by workspace path
    - Crash recovery: back up dirty buffers periodically to `~/.local/state/red/backup/` (the panic hook already writes there); offer restore on next start
    - Recent workspaces: `recent.toml`; "Open Recent Workspace" (Ctrl+R) in the palette
    - Kitty: `red_workspace` user var on start / exit / panic; `kitten @ ls` to find and focus red's window, `kitten @ launch` for a new tab; message fallback outside kitty
    - Tests drive the kitty side through a fake `kitten` on `PATH` (canned `ls` JSON, records its arguments)
    - Done when: quit and reopen gives back the same tabs, splits, cursors and scroll; killing the terminal loses at most one backup interval of edits; picking a recent workspace open in another kitty tab or OS window focuses it, and one not open starts red in a new kitty OS window
14. **Polish**: settings, the rest of the editor list, performance
    - Settings: live reload of `settings.toml` and `.workspace` overrides (the files themselves load at startup since M8, 8d)
    - Soft wrap toggle (the view must stop assuming one screen row per line: up/down, mouse hits, scroll)
    - Owed from M3: move tabs between groups, drag-and-drop tabs, keyboard resize of splits
    - Performance against the [Goals](#goals): startup time, memory, 1 MB files; check the slow-frame warnings in the log
    - Done when: no warnings over 16 ms in a day of normal use

## Stretch goals
Very low priority. Don't build these yet, but don't make design choices that would block them.

### Basic terminals
- Terminals without the kitty keyboard protocol (Terminal.app, plain xterm, …)
- Needs a separate fallback keymap: many Ctrl/Ctrl+Shift combos can't be told apart, Cmd isn't sent at all
- **Keep in mind when designing:**
  - Keymaps are data, selected at startup; no key combos hard-coded in commands

### Minimap
- Drawn with Braille characters (2×4 dots per cell), about 4 lines of code per row
- Coloured with the main syntax colour in each cell
- Shaded band for what's on screen; click or drag to scroll
- Scrolls along with the editor on long files
- Marks where each cursor is
- Setting to turn it on and off
- **Keep in mind when designing:**
  - `layout`: an editor view can reserve a column on its right edge
  - `syntax`: highlight spans can be read for any range of lines (lazily, cached), not just the visible ones

### Themes
- Load themes from files; include light themes
- _TBD: format — import VS Code JSON themes? own TOML?_
- **Keep in mind when designing:**
  - No colours hard-coded in widgets: everything looks up a named key (`editor.background`, `keyword`, `gutter.added`, …) in the theme
  - The built-in dark theme is just the first theme, defined through the same keys

## TODO
Broad items not tied to a milestone.

- **CI: Gitea Actions pipeline on Linux** (no macOS runner available)
  - `.gitea/workflows/ci.yml`, on push and pull request
  - Steps: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release`
  - Toolchain from `rust-toolchain.toml`; cache `~/.cargo` and `target/`
  - Linux exercises the Ctrl keymap and inotify watcher; macOS paths (Cmd keymap, FSEvents, Trash) stay untested in CI
    - Keep running `cargo test` locally on macOS before pushing
- **Terminal config snippets** (do soon: terminals grab Cmd keys, Cmd+W can close the terminal tab with unsaved work)
  - `docs/terminals/<terminal>.md` for Kitty, Ghostty, WezTerm, iTerm2, Alacritty
  - Each: pass through every Cmd binding in the default keymap; turn on Option-as-Alt
  - Update the snippets whenever a Cmd binding is added (keymap change → snippet change)
  - Per-terminal check: press every binding and confirm it arrives (`RED_LOG=input=trace`)
  - Done: `docs/terminals/` (README + kitty, ghostty, wezterm, iterm2, alacritty); unit test `terminal_docs_list_every_cmd_binding` fails if the README table and the macOS Cmd keymap differ
  - Still to do: hand-check each terminal with its checklist (iTerm2 least certain)

- **Remove the `/milestones` command** (`.claude/commands/milestones.md`) once the last milestone is done; it only drives the agent workflow

## Open questions
- _Add your own_
