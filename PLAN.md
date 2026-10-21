# jed — rust editor

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
| Config | TOML via `serde` | `~/.config/jed/` |
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
- **Activity bar** (left edge): icons for Explorer, Search, Source Control, Tools
- **Side panel**: switches with the activity bar; can be toggled and resized
- **Editor area**: groups with tabs, split horizontally or vertically
- **Bottom panel**: Problems list, task terminals
- **Status bar**: git branch, diagnostic counts, cursor position, language mode, line ending

## Features

### Workspace
- A workspace is a root directory, optionally with a `.workspace` file (TOML) at its root
- Opening:
  - `jed <dir>`: workspace root is `<dir>`
  - `jed <file>` / `jed <file>:<line>[:<col>]`: walk up from the file to the nearest `.workspace`; else use the file's directory
  - `jed` with no args: current directory
- `.workspace` contents:
  - Settings overrides (tab size, wrap, …) on top of `~/.config/jed/`
  - LSP servers per language (command, args, init options)
  - Task templates (`[tasks]`); merged with global `~/.config/jed/tasks.toml`, workspace wins on name clash
  - Excluded paths for explorer, quick open and search (on top of `.gitignore`)
  - File history limits (`[history]`) — see [File history](#file-history)
  - File associations (e.g. `SConstruct`, `*.scons` → Python)
- Meant to be committed: no per-user state in it
- Per-user session state (open tabs, splits, layout) lives in `~/.local/state/jed/`, keyed by workspace path
  - `sessions/<hash of the root>.json`, the root inside: groups and the split tree (ratios too), each group's tabs (text tabs by path with every selection and the scroll; git and history diff tabs by path and pair; task tabs never), the active tab and group, the side panel (shown, width, view) and the bottom panel (shown, height; back on Problems)
  - Written on quit, off the UI thread, atomically; read on start off the UI thread with the files it names; a missing file's tab is skipped (its group closes if left empty), a bad session file restores nothing; both logged
  - `jed <file>` restores the session, then opens the file on top in the active group (VS Code): onto its tab if the session has one there, keeping the session's cursor unless a line was given
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
- Trust is per user, stored in `~/.local/state/jed/trust.toml`, keyed by workspace path + SHA-256 of the command entries
  - Both answers are stored, so Restricted isn't asked again on every open
  - Esc = Restricted
- Re-prompt when the command entries in `.workspace` change
- Command palette: "Manage Workspace Trust"

**Recent workspaces**
- Every start records the workspace root in `~/.local/state/jed/recent.toml`: most recent first, no duplicates, capped at 50
- "Open Recent Workspace" in the command palette (Ctrl+R, as VS Code's Open Recent): a fuzzy list in the palette's input widget
  - Each row: folder name, then the path with `~` for home; the current workspace is left out
  - Folders that no longer exist are dropped from the list when it opens
  - Rows already open in a kitty window are marked
- Picking one:
  - Already open in jed in a kitty window (any tab, any OS window): focus that window
  - Else, inside kitty with remote control: open a new kitty OS window running `jed <path>`, with that folder as its cwd
  - Else (another terminal, or remote control off): status-bar message with the path and the `jed <path>` command; nothing else happens
  - jed never switches its own workspace in place
- Kitty integration (other terminals are ignored for now):
  - Inside kitty when `KITTY_WINDOW_ID` is set
  - jed marks its window by setting a kitty user var `jed_workspace` (OSC 1337 `SetUserVar`, base64 path) at startup, and clears it on exit and in the panic hook
  - Finding windows: `kitten @ ls` → JSON → the window whose `user_vars.jed_workspace` is the path; focus by id with `kitten @ focus-window --match id:<id>`
    - Match on the user var, not the window title or cwd, so a shell `cd`'d into the folder isn't mistaken for jed
  - New OS window: `kitten @ launch --type=os-window --cwd <path> jed <path>`
  - `kitten` runs off the UI thread with a short timeout; failure (remote control off, `kitten` missing) falls back to the message, and is logged
  - Needs `allow_remote_control yes` (or `socket-only` with `listen_on`) in `kitty.conf`: documented in `docs/terminals/kitty.md`

### Files
- Encoding: UTF-8 only in v1; keep a UTF-8 BOM if present; other encodings open read-only with a warning
- Binary files (NUL bytes) open read-only in a hex view — see [Binary files (hex view)](#binary-files-hex-view)
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
- Crash recovery: dirty buffers backed up periodically to `~/.local/state/jed/backup/`; offer restore on next start
- Untitled buffers, Save As and Save All (the [Untitled files and Save As](#named-milestones) milestone):
  - "File: New Untitled File" (Cmd+N) opens `Untitled-1` (the lowest free number): no path, Plain Text until saved or a language is picked ("Change Language Mode", a picker of `Language`s, also for files: overrides detection for that buffer)
  - Everything keyed by a path skips an untitled buffer: disk checks, the watcher, file history, git markers / blame, language servers (no `didOpen` until it has a path)
  - Save on an untitled buffer, and "File: Save As…" (Cmd+Shift+S) on any buffer, prompt for a path (relative to the root, prefilled with the current one; an existing file asks to overwrite); the buffer then takes the path: language re-detected (unless picked), every tab's title, LSP `didClose` / `didOpen`, the disk stamp, history and git markers follow; Save As leaves the old file as it was
  - "File: Save All" (Cmd+Alt+S): every dirty buffer, untitled ones prompting one at a time
  - Close / quit prompts include untitled buffers ("Untitled-1"); an empty untitled buffer closes without asking
  - Crash backups cover untitled buffers (keyed by an id instead of the path's hash, restored as untitled)
  - Sessions: _decide in the first task_ — leaning: empty untitled tabs aren't saved; non-empty ones are restored from their backup (VS Code's hot exit, minus the prompt), which makes the backup survive a clean quit

### Binary files (hex view)
A binary file opens read-only as `hexdump -C` would print it, instead of being refused.
- When: a file that `file::looks_binary` (a NUL in the first 8000 bytes) opens in a hex tab — from the explorer, quick open, the command line, a search / problem / location result, back / forward. "View: Reopen as Hex" (`view.reopenAsHex`, unbound, palette) shows any open file's bytes on disk the same way (a text file, or one with invalid UTF-8, now opened read-only and lossy); "View: Reopen as Text" goes back for a file that isn't binary
- Each row is `hexdump -C`'s: the offset (8 lowercase hex digits, more only when the file needs them), two spaces, the bytes in hex with one extra space after every 8, two spaces, then `|` the same bytes as ASCII `|` (0x20–0x7e as is, anything else `.`); one last row holds only the offset of the end (the file's size), as hexdump ends. No `*` squeezing of repeated rows: every row is `bytes_per_row` bytes from the one above, so row ↔ offset is plain arithmetic
- Bytes per row is always a power of two, in raw bytes (not hex digits): the largest one up to the `hex_bytes_per_line` setting (default 32; a power of two from 1 to 64, else reported and the default used) whose row fits the tab's width (16 bytes = hexdump's 78 columns; 32 = 144; narrower tabs drop to 8 / 4 / 2 / 1; below 1 byte's width the row is cut). Resizing keeps the cursor's byte and the top row's first byte in view (the top snaps down to a row start)
- Nothing is turned into a rope: the bytes stay as they are (`Arc<[u8]>`, shared by the tab's splits) and only the visible rows are formatted, so a 100 MB file costs its size once. Files over `HEX_MAX_BYTES` (256 MB) aren't opened (a message; paged reads are a TODO)
- A byte cursor, drawn in both columns (the column it's in as the terminal cursor, the other highlighted), and a selection (Shift + movement, mouse drag, Cmd+A): Left / Right one byte, Up / Down one row, Home / End the row's first / last byte, PageUp / PageDown, the file start / end keys; Tab switches the cursor between the hex and ASCII columns. A click on a byte in either column puts the cursor there (and in that column); the wheel and the scroll bar scroll. Cmd+C copies the selection (else the cursor's byte) as the column shows it: hex pairs separated by spaces, or the ASCII text with `.`s. Typing, paste, cut and undo do nothing (read-only: never dirty, never saved)
- Go to line (Ctrl+G) becomes "Go to offset" on a hex tab: hex (`0x1f0`, `1f0h`) or decimal (`496`); past the end → the last byte
- The status bar shows the cursor's offset in hex and decimal, the byte's value (`0x7f 127`), the selection's length, and the file's size; no language, encoding or EOL
- Find (Cmd+F) on a hex tab searches the bytes, with a mode toggle in the widget — `0x` beside Aa / ab / .* — "Find: Toggle Hex" (`find.toggleHex`, Cmd+Alt+X on macOS, Alt+X on Linux, as the other toggles; kept for the session, like case / whole word / regex):
  - ASCII mode: the query's UTF-8 bytes literally, Aa = case-sensitive for ASCII letters only (off: `a` matches `A`); whole word and regex don't apply on a hex tab (drawn disabled)
  - Hex mode: hex digit pairs, spaces ignored (`7f454c46`, `7f 45 4c 46`, `7F 45`); an odd count or a non-hex character is the widget's error ("hex bytes come in pairs", "not a hex digit: g"), as an invalid regex is
  - Switching mode converts a valid query (ASCII → its bytes as hex pairs; hex → text when every byte is printable ASCII, else the text stays and is re-read)
  - Opening find with a selection seeds the query from the selected bytes in the current mode (hex pairs, or the text when printable)
  - Matches run across row breaks (they're byte ranges, not text); each is highlighted in both columns, the current one as `editor.findMatch`, others `editor.findMatchHighlight`; Enter / Shift+Enter / F3 go to the next / previous one from the cursor, wrapping, scrolling it into view and selecting it. Every query is a fixed number of bytes, so a frame searches only its visible bytes (plus the query's length − 1 each side) for highlights, and next / previous searches straight from the cursor; the "3 of 41" count comes from a whole-file count off the UI thread (`memchr::memmem`; case-insensitive ASCII via a folded copy per chunk), capped at `HEX_MAX_COUNT` (100,000: "? of 100000+"), "…" until it arrives
  - No replace (the replace input stays hidden), no Alt+Enter select-all, no Cmd+D; workspace search still skips binary files
- Changed on disk: the tab re-reads the file (off-thread, by its `Stamp`) and keeps the cursor's offset, clamped; deleted → "(deleted)" as a text tab. No git gutter, blame, file history, language server, crash backup or auto-save: nothing about it is a text buffer
- Sessions keep a hex tab (path, cursor offset and column, top offset); one whose file is now text reopens as a hex tab still (it was asked for); a missing file drops it. Splits copy it; back / forward records it as a diff tab is (path only)
- Theme keys: `hexEditor.offset` (dim, as line numbers), `hexEditor.nonPrintable` (the `.` stand-ins and their bytes' hex, dimmed a little, so text in a binary stands out — hexyl's idea, still hexdump's characters), `hexEditor.border` (the `|`s); selection, cursor and find matches reuse the editor's keys
- Deferred: editing bytes (a writable hex editor: overwrite mode, insert / delete, undo, save) — the model keeps that door open (bytes behind one type, edits would be a byte `ChangeSet`); a regex in ASCII mode and `??` wildcards in hex mode (unbounded / variable-length matches need a different highlighting scheme); other column widths (offset base, word grouping 2 / 4 / 8, little-endian values) and a data inspector; `*` squeezing; files over 256 MB (paged reads with `pread`); a binary side in the diff view (it still says "binary")

### File history
Go back to any earlier save of a file, independent of git.

**Storage**
- Per workspace, in `<root>/.jed/history/`
- One folder per file, named by a hash of its workspace-relative path (no deep or long paths)
  - `entries.toml`: relative path + list of entries (id, time, size, content hash)
  - One snapshot file per entry, exact saved bytes (BOM and line endings kept)
- jed creates `.jed/.gitignore` containing `*`, so history is never committed
- `.jed/` hidden from explorer, quick open and search; watcher events under it ignored
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
  - Explorer: "Explorer: Show File History" (palette, on the selected file) or right-click on a file's row; tab bar: right-click on a tab (activates it); jed has no context menus, so the right-click runs the command directly
  - The file needn't be open: Compare shows the entry against the file on disk (the buffer once it opens); Restore opens the file first, then restores into the buffer

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
- "Find in Folder…" (`explorer.findInFolder`; right-click on a folder's row — a file's row shows its history): the Search view with that folder as its files to include (folders only, as VS Code)

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
- Line numbers, current-line highlight
- Soft wrap (the [Soft wrap](#named-milestones) milestone), VS Code's `editor.wordWrap`:
  - A `word_wrap` setting (`off` / `on`, default `off`) and "View: Toggle Word Wrap" (`view.toggleWordWrap`, Alt+Z as VS Code; session-only like the render whitespace toggle)
  - Lines wrap at the editor's width (left of the scroll bar), breaking after spaces, else between graphemes (wide chars and tabs never split); continuation rows keep the line's indentation (VS Code's `wrappingIndent: same`); the line number only on a line's first row; no horizontal scroll while wrapping
  - Up / Down, PageUp / PageDown and the goal column work on screen rows; Home / End go to the row's start / end first, then the line's (VS Code); add cursor above / below, box select and line commands stay per line
  - Scrolling, the scroll bar, mouse hits (click, drag, double click, Alt+Click), the find widget and popups (completion, hover) use screen rows; the view's top is a (line, row) pair, kept in sessions
  - Cost: rows per line computed lazily for what's drawn and cached per view by (revision, width), mapped through edits; the scroll bar's total from a per-line row count kept current incrementally; a 1 MB single-line file stays within the frame budget
  - Diff tabs and task tabs don't wrap
- Back / forward navigation (VS Code's Go Back / Go Forward): return the cursor to where it was before a jump, across files and tabs
  - A location is recorded on jumps: opening or switching files, go to line / definition / references, search and problem results, F4 / F8, Cmd+Home / End, and any click or move of 10+ lines in one step; edits record nothing
  - One history per window: entries are (group, file, primary selection, scroll) that follow edits, at most 50 (VS Code's); going back first records where you are; closed tabs reopen (a closed group's in the active one), files gone from disk are skipped, a diff tab counts as its file; going back then jumping elsewhere drops the forward entries
  - Keys: Ctrl+- / Ctrl+Shift+- (Linux Ctrl+Alt+- / Ctrl+Shift+-); mouse back / forward buttons (buttons 8 / 9) later (TODO list)
- Minimap: see [Stretch goals](#stretch-goals)

### Clipboard
- Local: system clipboard through `arboard`
- Over SSH (`SSH_TTY` set) or if `arboard` fails: copy through OSC 52
- Paste from the terminal (Cmd+V) arrives as a bracketed-paste event; handle it the same as clipboard paste
  - macOS: jed leaves Cmd+V to the terminal, so paste works the same locally and over SSH (most terminals block OSC 52 reads)
  - Linux: Ctrl+V reads `arboard`; the terminal's own Ctrl+Shift+V still arrives as bracketed paste
  - Cmd+C / Cmd+X must reach jed (the terminal's copy would take screen text, not the selection): add them to `docs/terminals/`
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
- First servers: `clangd`, `ty` (Astral's Rust type checker, `ty server`; chosen over pyright to avoid Node)
- SCons: treat `SConstruct`, `SConscript` and `*.scons` as Python
  - SCons injects globals (`Environment`, `Import`, …) that the server flags as undefined; ship a `__builtins__.pyi` with them if ty treats its names as built-ins (pyright does)
  - _Verify early: ty analyzes files without a `.py` extension when opened with `languageId: python`_
  - Best effort: if ty can't do either, SCons files get what ty gives them and it's noted as deferred
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
- Language features (the [Language features](#named-milestones) milestone):
  - Go to symbol in file: Cmd+Shift+O (`@` in quick open, VS Code): `textDocument/documentSymbol`, flattened with containers; rows: kind icon (like completion's), name, container dimmed; moving the selection previews the place (scrolls the editor, Esc restores), Enter goes there (recorded for back / forward); no server: a message (tree-sitter `tags` queries later)
  - Go to symbol in workspace: Cmd+T (`#` in quick open): `workspace/symbol`, re-requested as the query changes (debounced, older requests cancelled); same rows plus the file
  - Signature help: `textDocument/signatureHelp` on the server's trigger characters (`(`, `,`) and Cmd+Shift+Space: a popup above the cursor with the signature, the active parameter highlighted (`editorHoverWidget` colours, `popup::place`), its doc below; follows typing, closes on `)` / Esc / leaving the call; Up / Down cycles overloads while shown; coexists with the completion list (completion below, signature above)
  - Breadcrumbs and sticky scroll, a setting each (`breadcrumbs`, `sticky_scroll`), off by default: a row above the editor with the path and the symbols enclosing the cursor (the tree-sitter tree, else `documentSymbol`), a click on a crumb opening a picker of its siblings; sticky scroll pins the enclosing blocks' first lines at the top while scrolling (VS Code's)
- Diagnostics and quick fixes (the [Diagnostics and quick fixes](#named-milestones) milestone):
  - Diagnostics in hover (VS Code's): resting the mouse on a squiggle shows its diagnostics in the hover popup — today the message is only in the status bar (cursor line) and Problems. Every diagnostic of the buffer's `Marks` under the pointer (`overlapping(at, at + 1)`, plus empty marks at `at`, or at the line end for the char before, where they're drawn), most severe first: the severity icon in its colour (`Severity::icon` / `icon_key`), the message (plain text, multi-line kept, wrapped as hover lines are), then `source(code)` dimmed (`ty(unresolved-reference)`, a theme key such as `editorHoverWidget.source`); several separated by a rule; then a rule and the server's hover below when it gives one
    - The diagnostics show when the rest ends, without waiting for the server; the hover request still goes out and its answer is appended to that popup (same group / buffer / revision / pointer run). No server, no `hoverProvider`, or a null / failed answer leaves the diagnostics-only popup (no "No hover information"). The popup's range is the union of the marks' ranges and the word, so moving along a long squiggle keeps it open; edits, scrolling and tab switches close it as now
    - `app/hover.rs`'s `hover_target` returns `None` on whitespace and past a line's end: a cell carrying a squiggle counts (trailing-whitespace warnings, a squiggle on a space, an empty range at a line end), other whitespace still doesn't
    - "Show Hover" (`editor.showHover`, Cmd+Alt+I) does the same at the cursor; "No hover information" only when neither exists
    - The popup's lines gain kinds (e.g. `Kind::Diagnostic(Severity)` / `Kind::Source`, built by `hover::diagnostic_lines`), so `ui::hover` keeps wrapping, scrolling and sizing one list of lines
  - Code actions / quick fixes: Cmd+. (Ctrl+. on Linux) lists `textDocument/codeAction` for the cursor / selection with the diagnostics there (quick fixes first, preferred marked, disabled ones greyed with their reason) in a popup list at the cursor; applying one runs its `WorkspaceEdit` (`rename::parse_workspace_edit` / `App::apply_*` like rename, one undo step per file) and/or its `command` (`workspace/executeCommand`; server-sent `workspace/applyEdit` answered by applying it, where today `server.rs` refuses it); `codeAction/resolve` when the server needs it (`resolveSupport` for `edit`); a lightbulb `💡` in the gutter on the cursor line when actions exist (requested after a short rest, cached by revision)
    - The request's `context.diagnostics` are the server's own diagnostics as it published them (range, `code`, `data`: rust-analyzer and clangd find their fixes by them), so each published diagnostic keeps its raw JSON alongside `diagnostics::Diagnostic` (in `lsp::diagnostics::Item` / the `Arc`); only that server's diagnostics go to it
  - The hover's diagnostics section ends with a "Quick Fix…" row when the server offers fixes there (asked with the hover): clicking it opens the Cmd+. list for that diagnostic, as VS Code
- Snippets (the [Snippets](#named-milestones) milestone): completions whose `insertTextFormat` is a snippet keep their tab stops instead of being flattened by `completion::strip_snippet`
  - Advertise `snippetSupport: true` (and `insertReplaceSupport` as now); parse LSP / TextMate snippet syntax: `$1`, `${1:default}` (nested), `$0`, choices `${1|a,b|}` (the first choice inserted; the others as a completion-style list while on that stop), variables (`$TM_FILENAME`, `$TM_SELECTED_TEXT`, `$CLIPBOARD`, `$TM_LINE_NUMBER`, … resolved; unknown ones become a placeholder with their name, VS Code), `\$` / `\}` escapes; transforms (`${1/re/fmt/}`) parsed and applied to mirrors
  - A snippet session per view: each tab stop a set of byte ranges (every mirror of it, at every cursor the snippet was inserted at) mapped through edits like diagnostics' marks; the first stop selected (all its ranges as selections), Tab / Shift+Tab go to the next / previous stop, the last Tab (or `$0`) ends it with the cursor at `$0`; Esc, a click, or an edit / move outside the current stop's ranges ends it; typing in a stop edits every mirror at once (they're selections)
  - Placeholder ranges drawn with `editor.snippetTabstopHighlightBackground` (and `…FinalTabstopHighlightBorder` for `$0`); a `Context::Snippet` (parent `Editor`) holds Tab / Shift+Tab / Esc while a session runs, so Tab indents again afterwards
  - Insertion stays one undo step (`completion::insert`, multi-line text re-indented per line as now); undo inside a session ends it
  - Deferred: user snippets (`~/.config/jed/snippets/<language>.toml` and a picker, VS Code's "Insert Snippet"), snippets from code actions, nested sessions (a snippet completed inside a snippet replaces the outer session)
- Later: inlay hints, semantic tokens

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
  - An Open File button in its header opens the file at the cursor's line; "Git: Toggle Changes / File" (`git.toggleChanges`, unbound) switches between a file and its diff tab (the group's own, else its changes), each at the other's cursor line
- Stage / unstage a whole file or a single hunk
  - From the diff view and from the gutter
  - Hunks always come from the file on disk, never the buffer
  - Before staging: no outstanding writes to the file
    - Buffer dirty → prompt to save first
    - Save in flight → wait for it to finish
    - Then recompute the diff from disk and stage
- Git history (the [Git history](#named-milestones) milestone), read-only:
  - A "Git History" section in the Explorer view, under the tree (the side panel split vertically: a header row with the section's title, collapsible, and a draggable divider between the two; VS Code's Timeline): the commits that touched the active editor's file (a diff tab counts as its file), newest first — summary, author, relative date (`history::ago`), short id dimmed; nothing for a file outside a repository or untracked ("No history")
    - `git log -- <path>` on HEAD: a `git2` revwalk with the path filter (commits whose tree entry for the path differs from every parent's, as git's default history simplification), off the UI thread, read in pages of 200 as the list scrolls; refreshed when HEAD moves; renames not followed (`--follow`): the list stops at the commit that added the path
    - Selection, scrolling and the scroll bar like the other side lists (`Scrollable`); Enter or a click opens the commit
  - A commit tab (a read-only text tab, as a log tab: one buffer per commit, reused; the editor's cursor, selection, find, copy and splits): the commit as `git show --stat <commit>` prints it, with git's default colours (theme keys): `commit <full id>` (yellow) with `Merge: a b` for a merge, `Author: Name <email>`, `Date:` (git's default date format, the commit's own offset), the message indented four spaces, then the stat — ` path | 12 ++++++--` per file (insertions green, deletions red, the graph scaled to the tab's width as git scales to the terminal's; `Bin 0 -> 1234 bytes` for binary; renames `old => new` with git's default rename detection), then ` 3 files changed, 10 insertions(+), 2 deletions(-)`
    - Diffed against the first parent (what `git show --stat` prints for a merge too, git 2.53); a root commit against the empty tree. The `Merge:` header labels a merge's stat
    - Scrolls, selects and copies like a read-only editor (the text is plain; colours come from the line kinds); the file's own row in the stat is highlighted
    - Enter or a click on a stat row opens that file's change in a diff tab in the group to the right of the commit tab's (split right when there's none; reused for the next file clicked): the parent's text ↔ the commit's (`diff_view::Text::Commit`, a new side: a blob read from a commit's tree by path), titled `name (abc1234)`; no staging or revert buttons
  - Sessions keep commit tabs and commit diff tabs by commit id (dropped when the commit is gone), the section's collapsed state and divider position
- Merge conflicts (the [Merge conflicts](#named-milestones) milestone): editing a conflicted file, not merging (`git merge` / `rebase` stay on the command line)
  - A conflict is `<<<<<<<` … (`|||||||` base …, diff3 / zdiff3) `=======` … `>>>>>>>` at line starts, found by a line scan of the buffer (not the tree), cached by revision; nested or unterminated markers aren't a conflict
  - Drawn: the current block, the incoming block and the base block in VS Code's merge colours (`merge.currentHeaderBackground` / `.currentContentBackground`, `merge.incomingHeader…` / `…Content…`, `merge.commonHeader…` / `…Content…`), the marker lines labelled after their text (`(Current Change)` / `(Incoming Change)`, dim, as blame is drawn); an action row of buttons after the `<<<<<<<` line's text: Accept Current · Accept Incoming · Accept Both · Compare (clickable, `HitMap`), dropped from the right when they don't fit
  - Commands (palette, unbound unless noted): "Merge Conflict: Accept Current / Incoming / Both / None" for the conflict at each cursor (or every conflict a selection touches), "… Accept All Current / All Incoming / All Both" for the file, "Next / Previous Conflict" (Alt+F8 / Shift+Alt+F8 — check against F8's problems first), "Compare Current Conflict" (a diff tab of current ↔ incoming, read-only, no staging); each accept is one `EditKind::Other` step through `Buffer::edit`
  - Gutter / scroll bar: none in the milestone (git markers already show the region as changed); the status bar shows "2 conflicts" for a file with markers
  - Source Control: conflicted files in their own "Merge Changes" section above Staged (VS Code), `!` and `gitDecoration.conflictingResourceForeground`; opening one opens the file (not a diff); Stage File on a conflicted file = `git add` (marks it resolved), asking first when the buffer still has markers
- Discard and line-level staging (TODO list): "Git: Discard Changes" for a whole file and staging / unstaging / reverting only the selected lines — see the TODO items
- Current branch shown in the status bar (read-only)

### Tasks (throwaway terminals)
- A terminal is spawned for one task, then thrown away when it finishes
- Templates: name, command, cwd, env — in `.workspace` and/or `~/.config/jed/tasks.toml` (see [Workspace](#workspace))
- Run from the command palette: "Run Task" (_TBD: shortcut_)
- Each run gets its own tab in the bottom panel
- Runs in a PTY, so tools keep their colours and progress bars
- On exit: show the exit code and keep the output until the tab is closed
- Kill a running task
- Out of scope: a general-purpose interactive shell
- Later: parse compiler errors into the Problems list (`file:line:col`)

### Tools and logs
- A Tools view in the side panel (its own activity bar icon), like VS Code's Output panel: every tool jed runs, one row each with its state
  - Each language server: name / command, language, root, state (starting / running / failed / exited), its progress if any, a badge when its log has errors
  - Task runs: name and status; the row focuses the run's panel tab (no second copy of its output)
  - "jed": jed's own background work (git, file watcher, parses, workspace search, crash backups) as one log
  - Selection, scrolling and the scroll bar as the other side lists; Enter or a click opens the row's log
- A log is a read-only editor tab, one per tool, reused when opened again (e.g. `rust-analyzer (log)`): find, copy, select all, scrolling and splits work as in any editor; it follows the tail while the cursor is on the last line (as VS Code's Output), else stays put; never dirty, no undo, not typed into
- Each log is an in-memory ring (the last 10,000 lines / 2 MB, the oldest dropped), filled off the UI thread:
  - A language server: its stderr, its lifecycle (start with the command line, the server info from `initialize`, exit / crash with the code), `window/logMessage` / `window/showMessage` (with type); its JSON-RPC traffic only while a protocol trace is on for it
  - "jed": jed's own `tracing` events at the current log level (the same lines as the log file)
- Commands: "Tools: Show" (`tools.show`), "Tools: Open Log" (`tools.openLog`, a picker), "Tools: Clear Log", "Tools: Toggle Protocol Trace" and "Tools: Restart Server" (a language server: a failed one otherwise stays failed until jed restarts); unbound
- Cost: appends batched per frame; drawing reads only the visible lines, as any buffer
- Logs are transient in the editor: log tabs are never saved in sessions (a restart shows none) and the rings start empty
- On disk too: each tool's lines are also appended (off the UI thread, batched) to `<state dir>/logs/tools/<tool>.log` (e.g. `rust-analyzer.log`, `clangd.log`; the language server's name, else its command's file name), each line with time, pid and workspace root so several jed windows can share a file; rotated by size: past 4 MB the file becomes `<tool>.log.1` (older ones shifted, up to `.3`, the oldest removed), under a `File::lock` on `<tool>.log.lock` as `recent.toml` does; "jed" isn't written again — it is already the daily `jed.<date>.log`. A protocol trace goes to the file too while it's on. Write errors are reported once and logging carries on in memory

### Themes
- No colours hard-coded in widgets: everything looks up a named key (`editor.background`, `keyword`, `editorGutter.addedBackground`, …) in the theme; the built-in Dark+ is just the first theme, defined through the same keys
- The [Themes](#named-milestones) milestone:
  - A built-in Light+ (`Theme::light_plus`: every key `dark_plus` has — a test checks both define the same set — with Light+'s colours and its 256-colour fallback)
  - A `theme` setting (`dark` / `light` / a theme file's name, default `dark`), applied live on a `.workspace` reload
  - "Preferences: Color Theme": a picker of the built-in and file themes with live preview (moving the selection redraws with it, Esc restores the old one); Enter sets `theme` in the user's `settings.toml` (keeping its comments; created from the template if missing)
  - Theme files in `~/.config/jed/themes/`: _format decided in the milestone's theme-files task_ — leaning: VS Code's JSON themes (`colors` by jed's key names, `tokenColors` scopes mapped onto jed's capture names), so existing themes can be dropped in; keys a file lacks fall back to the built-in of its kind (dark / light); errors reported like `settings.toml`'s and the theme falls back
  - Task terminals' default colours and ANSI palette come from the theme too (`terminal.*`)

### Command palette and quick open
- Cmd/Ctrl+Shift+P: command palette — every named `Command`, with its current binding
- Cmd/Ctrl+P: quick open file (fuzzy); accepts `file:line[:col]` like the command line
- Ctrl+G: go to line, in the same input widget
- Quick open skips what the explorer skips: `.gitignore`, `.workspace` excludes, `.jed/`

### Workspace search
- Cmd/Ctrl+Shift+F: Search view in the side panel (the activity bar's Search icon)
- Modes, toggled by buttons in the input or by key (see [Keybindings](#keybindings-vs-code-defaults)):
  - Plain text (default): the query matches literally
  - Case sensitive: off by default
  - Regex: Rust `regex` syntax; an invalid pattern shows the error under the input and nothing runs
  - Toggles combine (e.g. case-sensitive regex)
- Same matcher (`search` module) as find in file, so each option means the same thing in both
- Scope: workspace root; skips `.gitignore`d files, `.workspace` excludes, `.jed/` and binary files
- Open buffers with unsaved changes are searched from the buffer, not the disk
- Files to include / exclude (VS Code's "Toggle Search Details": the `…` button or Cmd/Ctrl+Shift+J shows two inputs under the query; hidden, they still apply and `…` stays lit):
  - Comma-separated globs (`src/**`, `*.rs`, `{a,b}`), relative to the root; a pattern without `/` matches at any depth (`*.rs`, `src` = every folder named `src`), one with a `/` (or a leading `./`) from the root; a folder or path means everything under it (`src` = `src/**`)
  - `!` takes a pattern's matches back out of its own field (`src, !src/gen`); exclude beats include
  - On top of `.gitignore` and `.workspace` excludes; dirty buffers filtered the same way (one matcher, `path_filter`); an invalid glob shows its error like an invalid regex and nothing runs
  - Edits re-search after the debounce, like the query's; Tab / Shift+Tab go query → include → exclude → results
  - Search view state, like the query (not in sessions)
- Off the UI thread (`ignore` walker + `grep-searcher` in `spawn_blocking`)
  - Results stream in as files finish
  - Typing restarts the search (short debounce) and cancels the running one
- Results:
  - Grouped by file with a match count; each line shows its match highlighted
  - Click or Enter opens the file in the focused group with the match selected
  - F4 / Shift+F4: next / previous result
  - Capped at 20,000 matches, with a "results truncated" note (as VS Code)
- Matches stay within one line in v1
- Replace across files, the whole-word toggle and collapsing / dismissing results (the [Search and replace](#named-milestones) milestone):
  - Whole word (`ab|`, Cmd+Alt+W / Alt+W as in the find widget): the `search::Matcher`'s whole-word rule, already shared with find
  - Results: a file row's chevron (click, or Left / Right on it) collapses / expands it; "Dismiss" on a row (a `×` drawn on hover, Delete in the list) drops that match or file from the results (and from what Replace All touches); "Collapse All" / "Expand All" (palette); a re-search brings everything back
  - Replace: a second input under the query (VS Code's `>` toggle beside it, Cmd+Shift+H opens the view with it shown), the same `Replacement` templates as the find widget (`$1`, `\n`); while it's non-empty each result line shows the match struck through (`diffEditor.removedTextBackground`) followed by its replacement (`…insertedTextBackground`)
  - Replace one match / one file / all: buttons on hover of a line / file row and beside the input, plus keys in the list (Cmd+Shift+1 = this match or file, Cmd+Alt+Enter = all, VS Code's); Replace All with more than one file asks first ("Replace 41 occurrences across 6 files?")
  - Where the edit goes: an open buffer through `Buffer::edit` (one `EditKind::Other` step per buffer, left dirty, every view's selections followed); a file not open is rewritten off the UI thread — re-read, re-searched with the same matcher (so the replaced spans are what's there now, not the stale result), written atomically as a save is, recorded in file history (the before-text first, so it can be got back), skipped with a note when it's become binary / not UTF-8; one message with the totals and any files skipped. Results re-search afterwards
  - Click on a result while the replace input is non-empty opens a preview diff tab of that file (`diff_view::Text::Replaced` — the file with every shown match replaced — against its current text; no staging), as VS Code; otherwise it opens the file as now
  - Preserve case (`AB` toggle) deferred

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
| Move editor into next / previous group | Ctrl+Cmd+Right / Left | Ctrl+Alt+Right / Left |
| Move editor left / right (in its group) | Cmd+Shift+PageUp / PageDown | Ctrl+Shift+PageUp / PageDown |
| Find / replace | Cmd+F / Cmd+Alt+F | Ctrl+F / Ctrl+H |
| Toggle case / whole word / regex (find, search) | Cmd+Alt+C / W / R | Alt+C / W / R |
| Toggle hex / ASCII find (hex view) | Cmd+Alt+X | Alt+X |
| Next / previous search result | F4 / Shift+F4 | F4 / Shift+F4 |
| Toggle search details (files to include / exclude) | Cmd+Shift+J | Ctrl+Shift+J |
| Go to line | Ctrl+G | Ctrl+G |
| Open recent workspace | Ctrl+R | Ctrl+R |
| Comment line | Cmd+/ | Ctrl+/ |
| Indent / outdent line | Cmd+] / Cmd+[ (Tab / Shift+Tab over lines) | Ctrl+] / Ctrl+[ (Tab / Shift+Tab over lines) |
| Move line | Alt+Up/Down | Alt+Up/Down |
| Duplicate line | Shift+Alt+Up/Down | Shift+Alt+Up/Down |
| Undo / redo | Cmd+Z / Cmd+Shift+Z | Ctrl+Z / Ctrl+Shift+Z |
| Cut / copy / paste | Cmd+X / Cmd+C / Cmd+V | Ctrl+X / Ctrl+C / Ctrl+V |
| Add next match / all matches | Cmd+D, Cmd+Shift+L | Ctrl+D, Ctrl+Shift+L |
| Toggle panel | Cmd+J | Ctrl+J |
| Go back / go forward | Ctrl+- / Ctrl+Shift+- | Ctrl+Alt+- / Ctrl+Shift+- |
| Emacs-style keys in the editor (macOS's text keys): line start / end, left / right, up / down (in the completion list: its selection), delete left / right | Ctrl+A / E, Ctrl+B / F, Ctrl+P / N, Ctrl+H / D | — (Ctrl+D, Ctrl+H, … are VS Code's Linux commands) |

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
- Log file: `~/.local/state/jed/logs/jed.YYYY-MM-DD.log`, rotated daily, keep last 7
- Level from `--log-level` or the `JED_LOG` env var (`EnvFilter` syntax, e.g. `JED_LOG=jed::lsp=trace,jed::git=debug`); default `info`
- Targets are module paths (`jed::app`, `jed::buffer`, …) so each can be turned up alone; plus `input` and `panic`
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
  - Back up dirty buffers to `~/.local/state/jed/backup/` (best effort)
  - Restore the terminal: pop kitty keyboard flags, disable mouse capture and bracketed paste, leave alternate screen, disable raw mode
  - Write the panic message and backtrace to the log
  - Then print them to stderr

## Milestones
1. ✅ **Skeleton**: open a file, edit, save, scroll, status bar — [details](docs/plan-done.md)
2. ✅ **Layout**: workspace root, `.workspace` loading and trust dialog, activity bar, explorer tree, tabs, mouse support — [details](docs/plan-done.md)
3. ✅ **Splits**: editor groups, resizing, focus switching — [details](docs/plan-done.md)
4. ✅ **Clipboard**: copy, cut, paste — [details](docs/plan-done.md)
5. ✅ **Palette**: command palette, quick open, keybinding config with contexts — [details](docs/plan-done.md)
6. ✅ **Multi-cursor and find**: Cmd+D, Cmd+Shift+L, add above/below, Alt+Click, box select, paste spread across cursors, find/replace in the file — [details](docs/plan-done.md)
7. ✅ **Workspace search**: plain text, case-sensitive and regex search across the workspace — [details](docs/plan-done.md)
8. ✅ **Highlighting**: grammars and the built-in dark theme — [details](docs/plan-done.md)
9. ✅ **LSP**: clangd and ty — [details](docs/plan-done.md)
10. ✅ **Git**: SCM panel, gutter markers, side-by-side diff view, stage/unstage hunks — [details](docs/plan-done.md)
11. ✅ **File history**: snapshot on save, history list, compare (diff view), restore — [details](docs/plan-done.md)
12. ✅ **Tasks**: templates, PTY runner, task tabs in the bottom panel — [details](docs/plan-done.md)
13. ✅ **Sessions and recent workspaces**: restore open tabs, splits and layout per workspace; jump between workspaces — [details](docs/plan-done.md)

Each milestone that adds a Cmd binding also updates `docs/terminals/` (the `terminal_docs_list_every_cmd_binding` test fails otherwise) — see [Terminals](#terminals).

### Named milestones
Milestones 1–13 above are done and keep their numbers. Milestones planned after them have names, not numbers, and **no order**: build whichever is wanted next. Each has a short id (`tools`, `soft_wrap`, …) for its task ids (`tools-a`, `tools-b`, …), commit subjects (`Tools view and logs (tools-a): …`), test file (`src/<id>_tests.rs`) and test names (`<id>_*`). A finished one stays here as one line, marked ✅; its full entry moves to [docs/plan-done.md](docs/plan-done.md).

- ✅ **Tools view and logs** (`tools`): a side panel view of every tool jed runs, each with its log in a read-only editor tab — [details](docs/plan-done.md)
- **Soft wrap** (`soft_wrap`): wrap long lines at the editor's width, with movement, scrolling and the mouse on screen rows — see [Editor](#editor)
  - Needs: the editor view and drawing (`ui::editor`), its scroll bar, mouse hits, sessions (the view's top)
  - The view stops assuming one screen row per line: a wrap layout per view (rows per line, byte ↔ (row, column)), used by drawing, scrolling, the scroll bar, mouse hits, popups and the find widget
  - `word_wrap` setting and "View: Toggle Word Wrap" (Alt+Z)
  - Done when: with wrap on, a Markdown file of long paragraphs and a 1 MB minified JSON file show every character without horizontal scrolling; Up / Down / Home / End / PageDown, click, drag and the scroll bar work on wrapped rows; resizing a split rewraps it with the cursor kept in view; typing in the 1 MB line stays within the frame budget; toggling off returns to the unwrapped view at the same cursor
  - Tasks (one agent and one commit each):
    - soft_wrap-a: the wrap model (`wrap.rs`: a line's break points for a width — after spaces, else grapheme boundaries, wide chars and tabs whole, continuation indentation; byte ↔ (row, display column); a per-view cache by (revision, width) mapped through edits; the total row count kept incrementally); the `word_wrap` setting and the toggle command, not drawn yet; unit tests incl. a reference check against a slow per-line wrap
    - soft_wrap-b: drawing and scrolling on screen rows: the editor draws wrapped rows (gutter number on a line's first row; whitespace, selections, cursors, find matches, brackets, squiggles, git markers, blame on the right rows), the view's top as (line, row), reveal / wheel / scroll bar over rows, no horizontal scroll while wrapping
    - soft_wrap-c: movement and the mouse: Up / Down / PageUp / PageDown with the goal column on rows, Home / End row first, mouse hits (click, drag, double / triple click, Alt+Click) to positions in wrapped rows, the find widget and completion / hover popups placed at the wrapped cursor
    - soft_wrap-d: sessions keep the (line, row) top and the toggle; the 1 MB release budget; the done-when tests, gaps and hand checks; marks the milestone ✅
- **Untitled files and Save As** (`untitled`): new untitled buffers, Save As and Save All — see [Files](#files)
  - Needs: buffers and prompts (M1), sessions and crash backups (M13), language server document sync (M9), file history (M11), the watcher and disk checks (M10)
  - Untitled buffers (Cmd+N, "Change Language Mode"), Save As (Cmd+Shift+S) moving a buffer to a new path, Save All (Cmd+Alt+S); close / quit prompts, crash backups and sessions covering untitled buffers; Cmd bindings update `docs/terminals/`
  - Done when: Cmd+N, typing and Cmd+S ask for a path and write the file, the tab becomes that file with its highlighting and its language server; Save As to a new name leaves the old file untouched and the buffer follows the new one (git markers, history, title in every split); Save All saves every dirty buffer; quitting with an unsaved untitled buffer asks first; killing jed and restarting offers the untitled buffer's text back as an untitled buffer
  - Tasks (one agent and one commit each):
    - untitled-a: untitled buffers: a `Buffer` without a path (`Untitled-N`), Cmd+N, tab titles, Plain Text, "Change Language Mode"; every path-keyed feature skips them safely (disk checks, watcher, history, git, blame, LSP, sessions for now); close / quit prompts include them; decide the session behaviour (see [Files](#files))
    - untitled-b: Save As and Save All: the path prompt (overwrite check), Save on an untitled buffer runs it; the buffer takes the new path everywhere (language, titles, LSP close / open, disk stamp, watcher, history, git markers, other views); Save All
    - untitled-c: crash backups for untitled buffers (an id instead of the path hash, restored as untitled) and sessions per the decision; the done-when tests, gaps and hand checks; marks the milestone ✅
- **Language features** (`lang`): go to symbol, signature help and breadcrumbs / sticky scroll (code actions are in [Diagnostics and quick fixes](#named-milestones)) — see [LSP](#lsp)
  - Needs: language servers (M9), completion's popup, kind icons and request handling (`App::send_request`), quick open (M5), back / forward navigation (TODO list; jumps recorded when it's there)
  - Go to symbol in file (Cmd+Shift+O, `@`) and workspace (Cmd+T, `#`); signature help (trigger characters, Cmd+Shift+Space); breadcrumbs and sticky scroll (settings, off by default); Cmd bindings update `docs/terminals/`
  - Done when: with rust-analyzer on a small cargo project: Cmd+Shift+O lists the file's items with their containers and Enter jumps (Esc after moving the selection goes back); Cmd+T finds a struct in another file; typing `foo(` shows its signature with the active parameter moving on `,` and closing on `)`; with breadcrumbs on, the row shows path › impl › fn following the cursor; clangd does the same on a C file
  - Tasks (one agent and one commit each):
    - lang-a: go to symbol in file: `documentSymbol` (both shapes: `DocumentSymbol[]` tree, `SymbolInformation[]`) → a symbols model, the `@` quick open mode and Cmd+Shift+O, preview while moving, Esc restores
    - lang-b: go to symbol in workspace: `workspace/symbol`, `#` mode and Cmd+T, debounced re-requests with older ones cancelled, rows with the file
    - lang-c: signature help: the popup, trigger / retrigger characters, the active parameter, overloads, closing rules, next to the completion list
    - lang-d: breadcrumbs and sticky scroll (settings, off by default) from the tree-sitter tree / `documentSymbol`; the done-when tests (real rust-analyzer and clangd, skipped with a note when absent), gaps and hand checks; marks the milestone ✅
- **Diagnostics and quick fixes** (`fixes`): a squiggle's diagnostics in the hover popup, and code actions / quick fixes (Cmd+., Ctrl+. on Linux), as VS Code — see [LSP](#lsp)
  - Needs: diagnostics (M9: `Marks`, the store), hover (M9: `app/hover.rs`, `ui::hover`), the completion list's popup (M9), `App::send_request`, rename's workspace edits (`rename::parse_workspace_edit`, `App::apply_*`)
  - Diagnostics in hover; code actions with the Cmd+. list, the lightbulb, resolve, commands and server-sent `applyEdit`; a "Quick Fix…" row in the hover; Cmd bindings update `docs/terminals/`
  - Done when: with rust-analyzer on a small cargo project with an unused import and a misspelled type: resting the mouse on each squiggle shows its severity icon, message and source (before the server's hover, which follows below it), and on a squiggle at a line end or over whitespace too; Cmd+. (Ctrl+. on Linux) on the unused import lists "Remove unused import", Enter applies it as one undo step and the squiggle goes after the next publish; on the misspelled type it lists its fixes (the preferred one marked) and applying one that runs a command (`workspace/executeCommand` → `workspace/applyEdit`) edits the buffer; the hover's "Quick Fix…" row opens the same list; ty and clangd show their diagnostics in hover the same way, and a clangd fix (e.g. a missing `;` or include) applies; stopping the server clears its squiggles and the hover shows none of its diagnostics
  - Tasks (one agent and one commit each):
    - fixes-a: diagnostics in hover: `hover::diagnostic_lines` and the new line kinds (unit tests), the diagnostics section shown at the rest's end and the server's answer appended, `hover_target` on squiggled whitespace / line ends, the command at the cursor, theme keys; tests through the fake server (icon / message / source before the answer, both sections in order, no `hoverProvider` → diagnostics only, error before warning, whitespace and line-end squiggles, moving within / off, an edit closes, a mark mapped through an edit found at its new place)
    - fixes-b: code actions: raw diagnostics kept per item, `codeActionProvider` caps, a `code_action.rs` model (`CodeAction` | `Command` answers, kinds, preferred, disabled, sorting), the Cmd+. list popup (keys in its own context, like the completion list), applying edits and commands, `codeAction/resolve`, answering `workspace/applyEdit`, the lightbulb after a rest, the hover's "Quick Fix…" row; the fake server scripts `codeAction` / `executeCommand` and sends `applyEdit`
    - fixes-c: the done-when tests (real rust-analyzer, ty and clangd, skipped with a note when absent), gaps and hand checks; marks the milestone ✅
- **Themes** (`themes`): a built-in Light+, a `theme` setting, a picker with live preview and theme files — see [Themes](#themes)
  - Needs: `Theme` and its keys (M8), settings layering (M8), the palette's pickers (M5)
  - Light+ with the same keys as Dark+; `theme` setting; "Preferences: Color Theme" with live preview, saved to `settings.toml`; theme files from `~/.config/jed/themes/`
  - Done when: picking Light+ recolours everything — editor, syntax, side bar, panel, popups, diff view, task terminals — with no Dark+ colour left (a test draws every view under both and finds no colour of the other theme); Esc in the picker restores the old theme; the choice survives a restart; a theme file dropped into the themes folder is listed and applies, and a broken one is reported and falls back
  - Tasks (one agent and one commit each):
    - themes-a: Light+ (`Theme::light_plus`, the same-keys test, 256-colour fallback) and the `theme` setting (`dark` / `light`), live on a `.workspace` reload
    - themes-b: "Preferences: Color Theme": the picker with live preview, Esc restores, Enter writes `theme` to the user's `settings.toml` keeping comments
    - themes-c: theme files: decide the format first (see [Themes](#themes)), load from `~/.config/jed/themes/`, list in the picker, fallbacks and error reports; the done-when tests, gaps and hand checks; marks the milestone ✅
- ✅ **Git history** (`git_history`): a file's commits in the Explorer view, each commit shown as `git show --stat` in its own tab, and a file's change in a commit as a diff — [details](docs/plan-done.md)
- **Hex view** (`hex`): binary files open read-only as a `hexdump -C`, rows a power of two bytes wide, with byte find in ASCII or hex — see [Binary files (hex view)](#binary-files-hex-view)
  - Needs: the tab kinds (a third `group::Tab` variant, as `Diff`), the find widget (M6), scroll bars, sessions (M13)
  - A `Tab::Hex` with its own model and drawing, not a read-only text buffer: a dump is ~4.3× the file's bytes as text, its byte ranges aren't contiguous in that text (a match across rows, the two columns), and the editor's line / grapheme machinery has nothing to do here
  - Done when: opening `/bin/ls` (or a test's binary with every byte value and a NUL early) shows a tab whose rows match `hexdump -C` of it at 16 bytes per row cell for cell (offset, hex, `|ascii|`, the closing offset row), at a width that fits 16 but not 32; widening the tab gives 32-byte rows and narrowing it 8, then 4 — never another count — with the cursor's byte still shown; Ctrl+G `0x100` puts the cursor on offset 0x100 and the status bar says so; Cmd+F `ELF` in ASCII mode and `45 4c 46` in hex mode find the same match (the toggle converts one into the other), a match across a row break highlights in both columns of both rows, and the count equals a naive count; Cmd+C on a selection copies its hex pairs; the file rewritten on disk updates the tab; quit and reopen restores it at the same offset
  - Tasks (one agent and one commit each):
    - hex-a: the model, no UI: `hex.rs` — `HexDoc` (bytes `Arc<[u8]>`, `Stamp`), `Layout::fit(width, max, len)` (bytes per row: the largest power of two ≤ `max` whose row fits; offset digits; each part's columns: offset, every byte's hex cell, the ASCII column — and back: a screen column → byte + column), `row(doc, layout, offset) -> String` exactly as `hexdump -C` prints it (unit tests against the `hexdump` CLI's output when present, else a recorded fixture: every byte value, a short last row, an empty file, a file whose size needs 9 offset digits faked with a small layout), `parse_offset` (hex / decimal), and the byte search: `Query::{Ascii { text, case_sensitive }, Hex(bytes)}` (`parse_hex` with its errors, `convert` between modes), `find_next` / `find_prev` (wrapping) / `find_in(range)` / blocking `count(limit)`, all against a naive reference over random bytes; `hex_bytes_per_line` setting (`edit::Settings`, `SettingsOverrides`, doc table, `TEMPLATE`)
    - hex-b: the tab: `Tab::Hex(Box<HexTab>)` (path, doc, cursor / anchor offsets, column (hex / ASCII), top offset, the drawn `Layout`), opened instead of `LoadError::Binary`'s message (the load returns the bytes it read: no second read; `Opened` carries `Loaded::Binary`), reused per path like a text tab, "View: Reopen as Hex / Text"; `ui/hex.rs` draws only the visible rows with the theme keys, the cursor in both columns, the selection; `Context::Hex` (parent `Editor`, as `Diff`): `hex_command` takes the movement / selection / copy / select-all commands and Tab, everything else does nothing (`milestone_10_editor_commands_on_a_diff_tab_do_nothing`'s pattern: run `Command::all()` on a hex tab); mouse clicks / drags in either column, the wheel, `Scrollable` + the scroll bar; Go to Offset through the Ctrl+G input; the status bar's offset / value / selection / size; splits; tab title and the `(deleted)` mark; `has_file`-style skips for every file-keyed path (`active_path` on a hex tab is its path for Reveal / Copy Path, but `active_file_buffer` is `None`); the CLAUDE.md "Tabs" convention updated
    - hex-c: find on a hex tab: the widget's `0x` toggle (`FindCommand::ToggleHex`, Cmd+Alt+X / Alt+X; the macOS Cmd binding in `docs/terminals/README.md` and every snippet), the mode kept in `App::find_options`, case only (whole word / regex drawn disabled, replace hidden, Alt+Enter / Cmd+D nothing), conversion on toggle, seeding from a selection, visible-window highlights in both columns, next / previous from the cursor, the off-thread count (`AppEvent::HexCount` by tab id + query generation, stale ones dropped; "…" meanwhile, "100000+" past the cap)
    - hex-d: disk changes (the tab's stamp checked off-thread like a buffer's; changed → re-read, cursor and top clamped; deleted → `(deleted)`), sessions (`TabState::Hex { path, cursor, column, top }`, version unchanged: serde-tagged, older files parse), navigation entries (path only); the done-when tests (`src/hex_tests.rs`, `hex_*`; `hexdump` skipped with a note when missing), gaps and hand checks; marks the milestone ✅
  - Decided: power-of-two rows up to a setting rather than a free width, so offsets stay round (`…00`, `…20`) and `hexdump -C`'s 16 is one step; fixed-length queries only, so highlighting a frame never needs the whole file's matches; read-only — a binary is never written by jed
  - Still to check by hand: in kitty and on macOS, a real binary (`/bin/ls`, a PNG, a 100 MB file: open time, scrolling, a count over many matches) against `hexdump -C | less`; resizing through each power of two; Cmd+Alt+X reaching jed in each terminal snippet; a file that's text but opened as hex (`Reopen as Hex`) and back
- **Snippets** (`snippets`): LSP snippet completions with tab stops, mirrors and choices — see [LSP](#lsp)
  - Needs: completion (M9: `completion::insert`, the popup), multi-cursor selections, `ChangeSet` mapping (as `diagnostics::Marks`)
  - A snippet parser and a per-view snippet session (tab stops as mapped ranges, Tab / Shift+Tab, `$0`, ending rules), `snippetSupport` advertised, placeholders drawn
  - Done when: with rust-analyzer on a small cargo project, completing a function call inserts `name(arg1, arg2)` with `arg1` selected, Tab moves to `arg2`, Tab again leaves the cursor after `)`; a `match` / `for` postfix snippet with a mirrored name edits both places as one types; Esc ends the session and Tab indents again; the same completion at three cursors gives three sessions' worth of stops moving together; undo removes the insertion in one step; clangd's function-argument placeholders behave the same
  - Tasks (one agent and one commit each):
    - snippets-a: the model, no UI: `snippet.rs` — the parser (grammar above, a reference test against VS Code's documented examples), resolving variables, laying the text out at each cursor (indentation, EOL) and the stops' ranges; `Session` (stops → ranges per cursor, `map(&ChangeSet)`, next / previous, `contains(selections)` for the ending rule); unit tests
    - snippets-b: in the app: `snippetSupport: true`, `completion::insert` keeps the snippet and starts the session, `Context::Snippet` keys, ending rules (Esc, outside edits / moves, click, undo), choices' list, drawing and theme keys
    - snippets-c: the done-when tests (real rust-analyzer and clangd, skipped with a note when absent; the fake server's scripted snippet completions for the rest), the 61k-selection budget unaffected when no session runs, gaps and hand checks; marks the milestone ✅
- ✅ **Search and replace** (`replace`): replace across files from the Search view, plus its whole-word toggle and collapsing / dismissing results — [details](docs/plan-done.md)
- **Merge conflicts** (`conflicts`): conflict markers coloured with Accept Current / Incoming / Both in the editor, and a "Merge Changes" section in Source Control — see [Git](#git-view-and-stage-only)
  - Needs: git status (M10, `Change::Conflicted`), the editor's drawing after the text (inline blame's), the diff view, the Source Control view
  - A line-scan conflict model per buffer (by revision), drawing and the action row, accept / next / compare commands, the Source Control section and Stage File as resolve
  - Done when: after a real `git merge` with two conflicts in a file (and a diff3 one with `merge.conflictStyle = diff3`), the file shows both conflicts coloured with their action rows; clicking Accept Incoming on the first and pressing the Accept Current command on the second leaves exactly the text `git checkout --theirs` / `--ours` would give for those hunks, each one undo step; Next Conflict wraps; Compare opens current ↔ incoming; Source Control lists the file under "Merge Changes" until Stage File, which then shows it under Staged; a file with no markers and a 1 MB file draw within the frame budget
  - Tasks (one agent and one commit each):
    - conflicts-a: the model, no UI: `conflict.rs` — scan a rope for conflicts (two- and three-way, line starts, unterminated / nested refused) into ranges (markers, current, base, incoming), accept-as-`ChangeSet` for each choice, next / previous; cached per buffer by revision; unit tests against `git merge-file` output
    - conflicts-b: the editor: drawing, labels, the action row and its hits, the commands, the status bar count, Compare (a read-only diff of the two blocks)
    - conflicts-c: Source Control's "Merge Changes" section, opening a conflicted file, Stage File as resolve (with the markers-remain prompt); the done-when tests with a real `git merge`, gaps and hand checks; marks the milestone ✅

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

## TODO
Broad items not tied to a milestone.

- ✅ **CI** — [details](docs/plan-done.md)
- ✅ **Terminal config snippets** — [details](docs/plan-done.md)
- **Live reload of settings**: `settings.toml`, `.workspace` overrides and `keybindings.toml` reloaded when they change (the files themselves load at startup since M8, 8d)
- **Soft wrap**: now the [Soft wrap](#named-milestones) milestone
- ✅ **Move tabs** — [details](docs/plan-done.md)
- **Drag-and-drop tabs** (after Move tabs): drag a tab along its bar to reorder, onto another group's tab bar to move it there, or onto an editor's edge / centre to split / move into it (the drop zone shaded, a theme key); a drag that never leaves its tab is a click; Esc or a release outside cancels
- **Search ignored files toggle** ([Workspace search](#workspace-search)): a toggle button beside the "files to exclude" input (VS Code's "Use Exclude Settings and Ignore Files"), on by default; off, the search also walks files `.gitignore` (and `.git/info/exclude`, `.ignore`) leaves out. `.git/`, `.jed/` and binary files are always skipped, `.workspace` excludes still apply, dirty buffers filtered the same way. Search view state like the other toggles (not in sessions); toggling re-searches; lit while the details are hidden, as `…` is. Tests: a file under a gitignored `target/` found only with the toggle off
- **Keyboard resize of splits** (owed from M3): "View: Increase / Decrease Editor Width" and "… Height" (VS Code's, unbound; in the palette) move the active group's nearest divider in that direction by a step (a few cells, clamped like a mouse drag); the ratios are the session's as after a drag
- **Side panel resizing, finished**: the side panel already resizes by dragging its border (its last column: `Hit::SideBorder` → `Drag::SideBorder`, width in `ui.side_width`, kept in sessions), but it's undiscoverable and rough. Make it VS Code's sash:
  - The border column is drawn as a visible divider (a theme key, e.g. `sideBar.border`), lit on hover and while dragging (`sash.hoverBorder`); the hit area doesn't take the list's last column (scroll bars sit there since the side-bar scroll bars item) — the divider gets its own column, or the hit moves to the editor's first column, decide
  - A drag stores the clamped width (`layout::compute`'s max: the editor keeps `MIN_EDITOR_WIDTH`), so dragging back after overshooting the max moves the border at once (today the stored width can exceed the max and the border stays put until the pointer comes back); dragging below the minimum by a few more cells hides the panel (VS Code snaps it closed), dragging out from the activity bar edge reopens it
  - Double-click on the border resets the default width; the width survives toggling the panel off and on and a terminal resize (clamped when drawn, the stored width kept)
  - "View: Increase / Decrease Side Bar Width" (unbound, palette; a step of a few cells), sharing the step with Keyboard resize of splits
  - Tests: drag through `app.mouse` (hover colour, overshoot and back, snap closed, reopen, double-click reset), the commands, the session keeping the width
- ✅ **Back / forward navigation** — [details](docs/plan-done.md)
- **Mouse back / forward buttons** (after back / forward navigation): the mouse's back / forward buttons run `nav.back` / `nav.forward`. crossterm 0.29 drops SGR mouse buttons 8 / 9 (`parse_cb`: "We do not support other buttons"): needs a patched crossterm (a vendored fork via `[patch.crates-io]`) or jed's own parse of those reports before crossterm sees them — decide first; not every terminal sends them (check kitty, Ghostty, WezTerm, iTerm2, Alacritty)
- **Performance pass** against the [Goals](#goals): startup time, memory, 1 MB files; check the slow-frame warnings in the log; tree bracket matching on a 1 MB flat JSON root (see M8)
  - Done when: no warnings over 16 ms in a day of normal use
- ✅ **Render whitespace** — [details](docs/plan-done.md)
- ✅ **Folder filter in the Search view** — [details](docs/plan-done.md)
- **Feature-based end-to-end test files** (four steps, in order, each done as its own `/todo` by its bold name below): the `src/milestone*_tests.rs` files (and `src/todo_tests.rs`) are named after when the work was done, not what they test (`milestone2_tests.rs` = workspace, trust, explorer, tabs, file ops); move them to `src/tests/<feature>.rs` (e.g. `editing`, `multicursor`, `find`, `workspace_search`, `explorer`, `splits`, `palette`, `highlighting`, `lsp`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …), splitting a milestone's file where it spans features and merging tests of one feature from several milestones. Rules for every step:
  - Pure move and rename: no test logic changes; the test count stays the same (compare `cargo test -- --list` before / after)
  - Tests renamed from `milestone_N_*` / `<id>_*` to the behaviour; `done_when` tests keep a recognisable name (`done_when_*`), their file's `//!` doc saying which milestone's "Done when" they cover; the release budget tests become `budget_*` (CLAUDE.md's `cargo test --release milestone` commands change to `budget` in the step that renames the last of them)
  - Each step updates the CLAUDE.md references it moves (the Tests section's helper paths, the module map entry, nested CLAUDE.md files) and PLAN.md's references to test names
  - **Test support module**: `src/tests/mod.rs` and `src/tests/support.rs` with the shared helpers (`open_app`, `cmd`, `settle_git`, `draw_sized`, …, now duplicated or reached across files as `milestone10_tests::…`); callers updated, duplicates removed, no test moved yet
  - **Feature test files: editing** — M1, M4 and M6 (`milestone_tests.rs`, `milestone4_tests.rs`, `milestone6_tests.rs`) into `editing`, `clipboard`, `multicursor`, `find`, plus `todo_tests`' modules of those features
  - **Feature test files: workbench** — M2, M3, M5, M7 and M8 into `workspace`, `explorer`, `tabs`, `splits`, `palette`, `workspace_search`, `highlighting`, `settings`, plus `todo_tests`' modules of those features
  - **Feature test files: tools** — M9–M13 and the rest of `todo_tests` into `lsp`, `problems`, `completion`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …; then CLAUDE.md, `.claude/commands/milestones.md` and `todo.md` say new tests go in the feature's file (a named milestone's too, instead of `src/<id>_tests.rs`)
- ✅ **Rust language server** — [details](docs/plan-done.md)
- ✅ **Inline git blame** — [details](docs/plan-done.md)
- ✅ **Cursor style setting** — [details](docs/plan-done.md)
- ✅ **Syntax highlighting in the diff view** — [details](docs/plan-done.md)
- ✅ **Indent / outdent lines** — [details](docs/plan-done.md)
- **New untitled file, Save As, Save All**: now the [Untitled files and Save As](#named-milestones) milestone
- **Tab management commands**: reopen closed tab (Cmd+Shift+T; a stack of closed tabs per window with their view state, files reopened from disk), "Close All", "Close Others", "Close Saved" (in a group; prompts for unsaved like closing one), go to the Nth tab in the group (Ctrl+1–9 on macOS, Alt+1–9 on Linux, as VS Code; 9 = last); in the palette, and on the tab bar's right-click once there are context menus
- **More line commands**: delete line (Cmd+Shift+K), select line (Cmd+L, repeated: the next line too), join lines (Ctrl+J on macOS; unbound on Linux, where Ctrl+J is the panel), insert line below / above (Cmd+Enter / Cmd+Shift+Enter, with auto-indent), transform to upper / lower / title case, sort lines ascending / descending, delete duplicate lines; at every selection (`LineWalker`, one undo step each, the line-command budget); Cmd bindings update `docs/terminals/`
- **Expand / shrink selection** (Shift+Alt+Right / Left on Linux, Ctrl+Shift+Cmd+Right / Left on macOS, as VS Code): grow each selection to the enclosing tree-sitter node (word first, then string contents, the string, the bracket contents, the brackets, the statement, the block, …), shrink back through the same history (kept per view, cleared on other selection changes); without a tree: word, line, all; at every selection
- **Go to symbol, signature help, code actions, breadcrumbs / sticky scroll**: now the [Language features](#named-milestones) milestone
- **Keyboard Shortcuts view**: "Preferences: Keyboard Shortcuts" lists every command with its key(s) and context in the palette (fuzzy on name or key text, e.g. `ctrl+shift`; a "record keys" mode: the next key combo pressed filters to its bindings, VS Code's); shows where a binding came from (default / `keybindings.toml`) and user overrides; Enter on a row opens `keybindings.toml` with a `[[binding]]` stub for that command (creating the file like settings)
- **Light theme / theme files**: now the [Themes](#named-milestones) milestone
- **Word-based completion**: without a language server (plain text, YAML, TOML, Markdown, … or a server returning nothing), Ctrl+Space and quick suggestions offer the words in the open buffers (current buffer first, then others; VS Code's `editor.wordBasedSuggestions: matchingDocuments`), filtered with the same `completion::Filter`; collected off the UI thread per buffer and cached by revision; capped (e.g. 2 MB of text, 20k words); a setting to turn it off
- **Diagnostics in hover**: now the [Diagnostics and quick fixes](#named-milestones) milestone
- **Language server setup docs**: human-facing `docs/language-servers.md` (linked from a README / `docs/` index): which servers jed starts by itself (`clangd` for C / C++, `ty server` for Python, `rust-analyzer`) and how to install each on macOS (Homebrew, `uv tool install ty`, `rustup component add rust-analyzer`) and Linux (apt / dnf, uv, rustup); making them findable (`PATH`, a project `.venv`); clangd's `compile_commands.json` (CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, `bear -- make`, SCons) and `.clangd`; ty's project config (`pyproject.toml` / `ty.toml`, the venv) and the SCons support jed adds; adding any other server through `.workspace` `[lsp.<language id>]` (command, args, `initialization_options`, with worked examples, e.g. a YAML or TOML server) and why workspace trust asks first; what works (the v1 features) and the keys for each; troubleshooting (the status bar / Problems, `--log-level` and "Log: Open" with the `lsp::wire` / `lsp::stderr` targets, a server that fails is not retried until restart); kept current when servers or config keys change (add a line to CLAUDE.md's conventions)
- ✅ **Scroll bars in the editors** — [details](docs/plan-done.md)
- ✅ **Revert hunks in the diff view** — [details](docs/plan-done.md)
- **Kitty integration for recent workspaces** (deferred from M13; see [Recent workspaces](#workspace)): `jed_workspace` user var on start / exit / panic; `kitten @ ls` marks rows already open and focuses that window; `kitten @ launch --type=os-window --cwd <path> jed <path>` for one not open; off the UI thread with a timeout, message fallback; tests through a fake `kitten` on `PATH` (canned `ls` JSON, records its arguments); `docs/terminals/kitty.md` (`allow_remote_control`). Kitty isn't installed on the Linux test box: real-kitty behaviour is a hand check
- **Bug reporter**: "Help: Report Bug" (`help.reportBug`, palette, unbound) asks for a brief description of what happened (an `Input` prompt, multi-line not needed), then writes one self-contained Markdown file, `<state dir>/bug-reports/<UTC time>.md`, and shows its path in a message (also copied to the clipboard). Contents, each a section: the description; jed's version and git commit (built in at compile time), OS, terminal (`$TERM`, `$TERM_PROGRAM`, kitty keyboard protocol on or not, size, truecolor); the workspace (root, trust, `.workspace` settings, the layered settings, `keybindings.toml` overrides and load errors); the layout (groups and split tree, each group's tabs — path relative to the root, language, size, line count, dirty / deleted / read-only / disk conflict, EOL, encoding, selection count and primary's position, scroll — the active group and tab, focus, side / bottom panel and their views, open prompt / palette / find widget); language servers (per server: command, state, encoding, pending requests, diagnostics counts), git (branch label, entry counts), tasks (name, status), background work in flight (saves, parses, searches, backups); the last messages shown and the last ~50 commands run (a ring buffer of `Command::name()`s with times; no typed text); and the tail of the current log file (`newest_log`, last ~500 lines). Never file contents or typed text (privacy: the file is meant to be attached to a report); paths outside the workspace shown with `~`. Gathered on the UI thread as a cheap snapshot (O(groups + tabs + buffers), no rope scans), written in `spawn_blocking`. The panic hook could later write the same report (description = the panic message)
- ✅ **Scroll bars in the side bar's lists** — [details](docs/plan-done.md)
- ✅ **First reveal in a never-drawn list** — [details](docs/plan-done.md)
- **Tools view and tool logs**: now the [Tools view and logs](#named-milestones) milestone
- **PTY smoke tests**: a few end-to-end tests that run the real `jed` binary (`env!("CARGO_BIN_EXE_jed")`) in a pseudo-terminal (`portable-pty`, already a dependency) and parse what it writes with `vt100` (as task runs do), so the bytes crossterm emits and the bytes a terminal sends are tested, not just `TestBackend`: (1) after start the vt100 screen equals the `TestBackend` frame of the same state (text and colours, cell by cell; also after an edit and a scroll, to catch stale cells from ratatui's diff flush); (2) real input sequences reach the right commands — legacy and kitty-protocol (`CSI u`) keys incl. `Cmd+!` / Ctrl+@, bracketed paste, SGR mouse clicks and wheel; (3) start / quit leaves the terminal as it was (raw mode off, alt screen left, mouse / bracketed paste / kitty flags popped: check the mode-setting sequences in the output and the PTY's termios), and so does a forced panic (a test-only `JED_TEST_PANIC` hook or similar), which also leaves its crash backup; (4) wide graphemes (CJK, emoji, combining marks) land in the columns `pos::display_grapheme` says. Isolated `XDG_STATE_HOME` / `XDG_CONFIG_HOME` per test (they're child processes, so setting them is fine), a fixed size (e.g. 100×30), waits on screen contents with a timeout (no sleeps), `TERM=xterm-256color` and `COLORTERM=truecolor`; skip with a note where PTYs aren't available. Not a cross-emulator matrix: vt100 is one emulator; termless (TypeScript; xterm.js / Ghostty backends) or esctest could be added later if wide-character or protocol differences between terminals become a real problem
- **File History section in the Explorer** ([File history](#file-history), after the [Git history](#named-milestones) milestone): the active file's saved snapshots as a third Explorer section, built like "Git History" (VS Code's Timeline shows both sources; jed keeps them apart): the tree, then "FILE HISTORY", then "GIT HISTORY", each section with its header row (click toggles collapse), a draggable divider above it, and the tree keeping its minimum height (`git_history::split` generalised to N sections: each section's height and collapsed state; a collapsed one is its header only)
  - Rows: the entries newest first, as the picker shows them — relative time (`history::ago`), size (`size_label`), the UTC time dimmed (theme keys, e.g. `fileHistory.date` / `.size`); "No history" for a file with none, a file outside the workspace, or a non-file tab (log, commit); a diff tab counts as its file (a history diff as its file too)
  - Follows the active file like Git History; entries read off-thread (`history::entries` in `spawn_blocking`, generation-checked); re-read after `HistoryDone` for that file (a save added an entry, a rename moved them) — no polling, and nothing read while the section is collapsed
  - Selection, scrolling and the scroll bar as the other side lists (`Scrollable`, a `ScrollTarget` of its own); keys in a new `Context::FileHistory` (Up / Down / PageUp / PageDown / Home / End, Left / Esc back to the tree); Enter or a click opens Compare (`Pair::history(snapshot)` diff tab, as the picker's Compare); "File History: Restore Entry" (palette, and a key in the section, e.g. Cmd+Enter / Ctrl+Enter — decide) restores the selected entry into the buffer (`restore` as today: one undo step, nothing written)
  - "File History: Show" focuses the section (expanding it) instead of opening the picker; the picker stays reachable for Restore and from the explorer's / tab bar's right-click until context menus exist — decide in the item whether the picker goes
  - Sessions keep its collapsed state and height (`SideState`, serde defaults, `VERSION` unchanged)
  - Tests: entries listed newest first after saves (and a new row after one more save), switching tabs switches the list, Enter / click opens the history diff, Restore through the section, collapse / divider drag / scroll bar, session round trip; its reveal defers to the first draw like the other lists' (a pending flag run by `set_height`)
- **Indentation detection and commands** ([Editor](#editor)): VS Code's `editor.detectIndentation` — a `detect_indentation` setting (default true): on load (and after a reload), guess spaces vs tabs and the indent size from the first ~10k lines (VS Code's `guessIndentation`: line-to-line indentation deltas, ties to the setting), stored per buffer and overriding the language's / settings' `insert_spaces` and tab width for edits (`Settings::for_language` gains the buffer's), Enter, Tab, indent / outdent and the LSP `FormattingOptions`. The status bar shows `Spaces: 4` / `Tab Size: 4` (a click opens the picker below). Commands (palette, unbound): "Indent Using Spaces" / "Indent Using Tabs" (a picker of sizes 1–8, the current one marked; the buffer's choice, kept until it's closed), "Change Tab Display Size", "Detect Indentation from Content", "Convert Indentation to Spaces" / "… to Tabs" (every line's leading whitespace rewritten, one `EditKind::Other` step via `LineWalker`). Session-only per buffer (not in sessions); a Makefile still always uses tabs
- **Change End of Line Sequence** ([Files](#files)): "Change End of Line Sequence" (palette, unbound; a click on the status bar's `LF` / `CRLF`) picks LF or CRLF for the buffer: every line break rewritten as one `EditKind::Other` step through `Buffer::edit` (undo brings the old ones back), new lines then use it, saved as is; a `files_eol` setting (`auto` (detect, default) / `lf` / `crlf`) for new files and files without a line break
- **Block comment and bracket jumps** ([Editor](#editor)): "Toggle Block Comment" (`edit.blockComment`, Shift+Alt+A on both platforms, VS Code's) with each language's block tokens in `LanguageConfig` (`/* */` C / C++ / Rust / JSONC / CSS-like, `<!-- -->` XML / Markdown, `"""` Python as VS Code; none: nothing): per selection, remove when the selection (trimmed of surrounding whitespace) starts and ends with the tokens, else wrap it (`/* ` … ` */`); an empty cursor inserts `/*  */` with the cursor in the middle; selections keep their text; one `EditKind::Other` step, every selection. "Go to Bracket" (`editor.jumpToBracket`, Cmd+Shift+\\ on macOS, Ctrl+Shift+\\ on Linux): each cursor to its bracket's partner (`brackets::match_at`), else to the closing bracket of the innermost pair around it (`brackets::Openers`); "Select to Bracket" (unbound): the selection grows to cover the enclosing pair, brackets included. Both at every cursor (the 61k-selection budget). Cmd bindings update `docs/terminals/`
- **Explorer path commands** ([Explorer](#explorer-file-tree), palette entries, no context menus needed): "File: Copy Path of Active File" / "File: Copy Relative Path of Active File" (VS Code's, unbound; a diff / commit / log tab: its file or nothing), "Explorer: Copy Path" / "Explorer: Copy Relative Path" (the selected row, file or folder; `/`-separated, relative to the root), "File: Reveal in Finder" (Linux: "Open Containing Folder" — `open -R <path>` / `xdg-open <parent>` off the UI thread, failure reported; for the active file, and "Explorer: Reveal in Finder" for the selected row), "Explorer: Reveal Active File" (expand to and select the active file's row, focusing the explorer), "Explorer: Collapse Folders" (every folder closed, the selection on its top-level row); all through `Clipboard::write` (OSC 52 over SSH), each with a short status message
- **Open File by path** ([Files](#files), palette entry): "File: Open File…" (`file.open`, unbound; Cmd+O stays the terminal's) opens an `Input` prompt for a path — absolute, `~/…`, or relative to the root, with `:line[:col]` as the command line takes it — Tab completes the path segment (a listing off the UI thread, common prefix, a second Tab lists the choices in the prompt's hint), Enter opens it as quick open does (`open_file`, back / forward records it); a folder → a message with `jed <path>` (jed never switches workspace); a missing file → a message (creating it is the Untitled / Save As milestone's). Files outside the root work as today (no watch, no history)
- **Compare any two files** ([Git](#git-view-and-stage-only) diff view): "File: Compare Active File With…" (palette, unbound) picks another file with quick open's file list and opens a diff tab of that file ↔ the active buffer (`diff_view::Text::File(path)` — a file's text, its open buffer's when open, else the disk — on the left; the right side `Buffer` as File History's Compare); "Explorer: Select for Compare" then "Explorer: Compare with Selected" (selected rows, or the active file for the second); titled `a.rs ↔ b.rs`, no staging / revert, follows edits on either side (`Later` reloads as now), next / previous change; sessions keep it (`Side::File`); binary files say "binary" as now
- **Discard changes in Source Control** ([Git](#git-view-and-stage-only)): a `↶` button on each Changes row (beside `+`) and "Git: Discard Changes" (palette, `git.clean` — the active file or the selected row) / "Git: Discard All Changes" (the Changes section's header button): after a confirm prompt naming the file(s) ("Discard changes in a.rs?"; for a closed file it can't be undone, though file history keeps its last save from jed), the work tree file gets the index's text back (= `git checkout -- path`; an open buffer through `Buffer::edit`, one undo step, left dirty, as Revert hunk (so the confirm says the buffer needs saving); a closed file written off-thread, keeping permissions); an untracked file is moved to the Trash (VS Code deletes it), its tab marked deleted; a staged-only change is left alone (the Staged section isn't touched); conflicted files refused (merge conflicts milestone). Refused while the file has a save in flight, like staging
- **Stage / unstage / revert selected lines** ([Git](#git-view-and-stage-only)): VS Code's "Git: Stage Selected Ranges" / "Unstage Selected Ranges" / "Revert Selected Ranges" (palette, unbound): from a text tab, the lines its selections touch (every selection; an empty cursor = its line); from an unstaged / staged diff tab, a row range selected with Shift + Up / Down (a new selection on `DiffTab`, drawn with `editor.selection`). Stage applies only those lines of each context-0 index → disk hunk to the index blob (an added line inside = added, a removed line inside = removed, the rest of the hunk left as it was — `git add -p`'s `e`dit result), checked against `Stale` like `stage_hunk`; unstage likewise HEAD → index; revert the same on the work tree (the buffer through `Buffer::edit`, one undo step). Saves a dirty buffer first as hunk staging does. A `git::stage::stage_lines` / `unstage_lines` / `revert_lines` in the model with tests against `git apply --cached` of the equivalent hand-edited patch

## Open questions
- _Add your own_
