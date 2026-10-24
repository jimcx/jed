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
- A general plugin system (scripted commands, UI, keymaps or themes as code): the one kind of plugin is a syntax plugin — a tree-sitter grammar built and loaded from `~/.config/jed/languages/` (see [Syntax plugins](#syntax-plugins)), which is native code through tree-sitter's C ABI, never Rust types across a library boundary
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
| Syntax plugins | `libloading` + the system C compiler | grammars from `~/.config/jed/languages/` built to a shared library and `dlopen`ed (the [Syntax plugins](#tasks) task) |
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
  - `jed <git message file>` (`.git/COMMIT_EDITMSG`, …) or `jed --wait <file>`: editor mode, one file, no session — see [Git editor](#git-editor-commit-messages)
  - `jed -s <file>` / `--single`: single file mode — one file, no side panel (its activity icons greyed), nothing per-workspace — see the [Single file mode](#tasks) task
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
  - jed marks its window by setting a kitty user var `jed_workspace` (OSC 1337 `SetUserVar`, base64 path) at startup, and clears it on exit and in the panic hook (done: `terminal::Identity`, with a `jed` var and the window title)
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
- Untitled buffers, Save As and Save All (the [Untitled files and Save As](#tasks) task):
  - "File: New Untitled File" (Cmd+N) opens `Untitled-1` (the lowest free number): no path, Plain Text until saved or a language is picked ("Change Language Mode", a picker of `Language`s, also for files: overrides detection for that buffer)
  - Everything keyed by a path skips an untitled buffer: disk checks, the watcher, file history, git markers / blame, language servers (no `didOpen` until it has a path)
  - Save on an untitled buffer, and "File: Save As…" (Cmd+Shift+S) on any buffer, prompt for a path (relative to the root, prefilled with the current one; an existing file asks to overwrite); the buffer then takes the path: language re-detected (unless picked), every tab's title, LSP `didClose` / `didOpen`, the disk stamp, history and git markers follow; Save As leaves the old file as it was
  - "File: Save All" (Cmd+Alt+S): every dirty buffer, untitled ones prompting one at a time
  - Close / quit prompts include untitled buffers ("Untitled-1"); an empty untitled buffer closes without asking
  - Crash backups cover untitled buffers (keyed by an id instead of the path's hash, restored as untitled)
  - Sessions: empty untitled tabs aren't saved; non-empty ones are restored from their crash backup (VS Code's hot exit, minus the prompt), saved as `TabState::Untitled { id, … }` and read from the backup in the session's own read. A clean quit has asked about every untitled buffer, so it writes none and removes their backups: the case this covers is the terminal going away (session and every dirty buffer's backup written); a kill or crash is offered by the restore prompt instead

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
- Soft wrap (the [Soft wrap](#tasks) task), VS Code's `editor.wordWrap`:
  - A `word_wrap` setting (`off` / `on`, default `off`) and "View: Toggle Word Wrap" (`view.toggleWordWrap`, Alt+Z as VS Code; session-only like the render whitespace toggle)
  - Lines wrap at the editor's width (left of the scroll bar), breaking after spaces, else between graphemes (wide chars and tabs never split); continuation rows keep the line's indentation (VS Code's `wrappingIndent: same`); the line number only on a line's first row; no horizontal scroll while wrapping
  - Up / Down, PageUp / PageDown and the goal column work on screen rows; Home / End go to the row's start / end first, then the line's (VS Code); add cursor above / below, box select and line commands stay per line
  - Scrolling, the scroll bar, mouse hits (click, drag, double click, Alt+Click), the find widget and popups (completion, hover) use screen rows; the view's top is a (line, row) pair, kept in sessions
  - Cost: rows per line computed lazily for what's drawn and cached per view by (revision, width), mapped through edits; the scroll bar's total from a per-line row count kept current incrementally; a 1 MB single-line file stays within the frame budget
  - Diff tabs and task tabs don't wrap
- Back / forward navigation (VS Code's Go Back / Go Forward): return the cursor to where it was before a jump, across files and tabs
  - A location is recorded on jumps: opening or switching files, go to line / definition / references, search and problem results, F4 / F8, Cmd+Home / End, and any click or move of 10+ lines in one step; edits record nothing
  - One history per window: entries are (group, file, primary selection, scroll) that follow edits, at most 50 (VS Code's); going back first records where you are; closed tabs reopen (a closed group's in the active one), files gone from disk are skipped, a diff tab counts as its file; going back then jumping elsewhere drops the forward entries
  - Keys: Ctrl+- / Ctrl+Shift+- (Linux Ctrl+Alt+- / Ctrl+Shift+-); mouse back / forward buttons (buttons 8 / 9) later (see [Tasks](#tasks))
- Indentation per buffer (the [Indentation](#tasks) task), VS Code's `editor.detectIndentation`:
  - A `detect_indentation` setting (default true): on load and after a reload, spaces vs tabs and the indent size are guessed from the first ~10k lines (VS Code's `guessIndentation`: line-to-line indentation deltas; ties and files with too little indentation keep the settings); a Makefile (`LanguageConfig::insert_spaces: Some(false)`) always uses tabs
  - The result is stored per buffer (every split of it follows) and overrides the language's / settings' `insert_spaces` and `tab_width` everywhere they are read: typing Tab, Enter and auto-indent, indent / outdent, comment line, completion re-indenting, the drawn width of a tab, column maths (Up / Down, box select) and the LSP `FormattingOptions`
  - The status bar shows `Spaces: 4` / `Tab Size: 4`; a click opens "Indent Using…"
  - Commands (palette, unbound): "Indent Using Spaces" / "Indent Using Tabs" (a picker of sizes 1–8, the current one marked), "Change Tab Display Size" (tabs' drawn width only), "Detect Indentation from Content", "Convert Indentation to Spaces" / "Convert Indentation to Tabs" (every line's leading whitespace rewritten at the buffer's size, one `EditKind::Other` step)
  - Session-only: a buffer's choice lasts until it's closed or re-detected on a reload (not kept in sessions)
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
- Drag tabs with the mouse: along the bar to reorder, onto another group's bar or editor to move it there (an empty group closes), onto an editor within a quarter of its width or height of an edge to split that group on that side and move it into the new split
  - The drop target is shaded while dragging (a theme key); a press that never leaves its tab is a click; Esc or a release outside any target cancels
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

### Syntax plugins
More languages without rebuilding jed (the [Syntax plugins](#tasks) task). A plugin is a tree-sitter grammar's sources plus a manifest in `~/.config/jed/languages/<id>/`; jed compiles it with the system C compiler and loads the result as a shared library.
- **Why this is possible:** Rust has no stable ABI, so a Rust `dylib` exposing jed's own types would have to be built with the exact compiler and crate versions jed was, and plugins in that style are out. A tree-sitter grammar isn't one: its whole interface is one C function, `const TSLanguage *tree_sitter_<name>(void)`, and the `TSLanguage` tables it returns carry an ABI version that the `tree-sitter` crate checks (0.27 accepts 13–15). Neovim and Helix load grammars the same way. jed takes the symbol through `libloading`, wraps it with `tree_sitter_language::LanguageFn::from_raw` and checks `Language::abi_version()` before any parse. Queries (`highlights.scm`) are plain text, compiled at runtime as the built-in ones are
- **Layout** (one directory per language, as `~/.config/jed/themes/` holds themes):
  - `language.toml`: `id` (the VS Code / LSP language id, e.g. `lua`), `name` (status bar: `Lua`), `symbol` (default `tree_sitter_<id>`), `[files]` with `extensions`, `filenames` and `shebangs` (program names, as the built-in shebang table), and the `LanguageConfig` fields under their own names: `line_comment`, `brackets` (`["()", "[]", "{}"]`), `pairs` (`[{ open = "'", close = "'", not_in = ["string", "comment"] }]`), `surround`, `insert_spaces`, `tree_tokens`; plus `comment_kinds` / `string_kinds` / `code_kinds` for node kinds the name rule in `Syntax::scope_of` misreads. Missing fields take the plain `config(…)` defaults. Read leniently, like `settings.toml`: unknown keys and bad values are reported, the rest still applies
  - `src/parser.c`, optional `src/scanner.c` or `src/scanner.cc`, `src/tree_sitter/*.h`: as a grammar's repository ships them (no tree-sitter CLI or `grammar.js` step: grammars commit the generated parser)
  - `queries/highlights.scm` (the grammar's own), then an optional `queries/jed.scm` appended after it, as `queries/<lang>.scm` follows the built-in ones (later patterns win)
  - Or, instead of `src/`, a prebuilt `parser.so` / `parser.dylib`, loaded as is
- **Building:** jed writes `parser.so` (Linux) / `parser.dylib` (macOS) next to `src/` with `$CC` (else `cc`; `$CXX` / `c++` when there's a `scanner.cc`): `-shared -fPIC -O2 -I src` (`-dynamiclib` on macOS). `build.toml` beside it records a SHA-256 of the sources, the compiler and the platform, so a build is redone only when one changes (stamps, never file times, as for files on disk). Builds run off the UI thread, one at a time, after startup; until a language's grammar is there its files open as that language but uncoloured, then get coloured. Each build's command line and compiler output go to a tool log (`Tool::Grammar(id)`, a row in the Tools view); a failure is reported once with `App::notify` and the language stays uncoloured. "Languages: Rebuild Syntax Plugins" (palette, unbound) rebuilds all of them
- **Loading:** a library is never unloaded (trees, queries and parsers point into it), so it is leaked for the life of the process; a bad file — missing symbol, ABI outside 13–15, a query that fails to compile (reported with its row and column) — is reported and that language falls back to no grammar. A plugin's grammar runs in jed's process: a crashing grammar crashes jed (the panic hook doesn't see a segfault: crash backups are what protect the text)
- **Languages:** `Language` stays `Copy`: plugins are `Language::Plugin(PluginId)`, an index into a registry filled once in `main` from the manifests (before raw mode, as `settings.toml`), with the grammar and query installed later; `id()`, `name()` and `config()` return the plugin's (leaked, `'static`); `from_id` finds plugin ids, so `.workspace` `[files] associations` and `[lsp.<id>]` work for them unchanged. Captures map through the same `highlight_for` / `KEYS` / `ALIASES` (no new theme keys; unknown captures are plain, logged once at debug). A plugin can't reuse a built-in id (reported; replacing a built-in grammar is deferred). Detection: associations, built-in exact names, then plugin filenames, then plugin extensions, then built-in extensions (a plugin claiming `.h` wins), then shebangs, built-in first
- **Making one is meant to be easy** — copy the template, swap the grammar, check it:
  - **The template:** `examples/languages/cmake/` in the repo is jed's built-in CMake redone as a plugin: a `language.toml` with every key set and commented (what each one does, which VS Code `language-configuration.json` field it matches), tree-sitter-cmake's `src/` (its `LICENSE` beside it) and jed's own `queries/cmake.scm` as `queries/highlights.scm`. Tests keep it honest: its manifest must give exactly the built-in CMake's `LanguageConfig` and detection (`CMakeLists.txt`, `*.cmake`), and once built its highlight spans on the CMake test snippet must equal the built-in's, so the template can't drift from what jed really does
  - **`jed --new-language <id>`** (prints and exits, before raw mode) copies the template into `~/.config/jed/languages/<id>/` with `id`, `name`, `symbol` and the file lists filled in and `src/` left empty for the grammar's files, and prints the steps: copy the grammar repository's `src/` and `queries/highlights.scm` in, edit `language.toml`, run the check
  - **`jed --check-language <id>`** (prints and exits) reads the manifest (every error with its line), builds the grammar (compiler output shown), loads it, compiles the query (errors with row and column), and lists captures that map to no theme key (with the keys they could alias to), the comment / string node kinds `scope_of` will see, and a sample file's highlighting as coloured text when given `--sample <file>`; exit status 0 only when nothing failed
  - **`docs/syntax-plugins.md`**: the walk-through (Lua from scratch with the two commands), every `language.toml` key, how captures become colours (`KEYS` / `ALIASES`, the table of theme keys), the `jed.scm` override rule, ABI versions, and troubleshooting by error message
- **Trust:** only the user's own `~/.config/jed/languages/` is read. A `.workspace` can associate files with a plugin's id but never add, build or point at a plugin: a cloned repository must not be able to make jed compile and load its code
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
- Server per language set in config / `.workspace`: `[lsp.<id>]` (`command`, `args`, `init_options`) in `.workspace` (when trusted) or in the user's `settings.toml` (always trusted: the user's own file); a trusted `.workspace` entry wins, then `settings.toml`'s, then the built-ins (`clangd`, `ty`, `rust-analyzer`). An id that is neither built-in nor a syntax plugin's is reported
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
- Language features (the [Language features](#tasks) task):
  - Go to symbol in file: Cmd+Shift+O (`@` in quick open, VS Code): `textDocument/documentSymbol`, flattened with containers; rows: kind icon (like completion's), name, container dimmed; moving the selection previews the place (scrolls the editor, Esc restores), Enter goes there (recorded for back / forward); no server: a message (tree-sitter `tags` queries later)
  - Go to symbol in workspace: Cmd+T (`#` in quick open): `workspace/symbol`, re-requested as the query changes (debounced, older requests cancelled); same rows plus the file
  - Signature help: `textDocument/signatureHelp` on the server's trigger characters (`(`, `,`) and Cmd+Shift+Space: a popup above the cursor with the signature, the active parameter highlighted (`editorHoverWidget` colours, `popup::place`), its doc below; follows typing, closes on `)` / Esc / leaving the call; Up / Down cycles overloads while shown; coexists with the completion list (completion below, signature above)
  - Breadcrumbs and sticky scroll, a setting each (`breadcrumbs`, `sticky_scroll`), off by default: a row above the editor with the path and the symbols enclosing the cursor (the tree-sitter tree, else `documentSymbol`), a click on a crumb opening a picker of its siblings; sticky scroll pins the enclosing blocks' first lines at the top while scrolling (VS Code's)
- Diagnostics and quick fixes (the [Diagnostics and quick fixes](#tasks) task):
  - Diagnostics in hover (VS Code's): resting the mouse on a squiggle shows its diagnostics in the hover popup — today the message is only in the status bar (cursor line) and Problems. Every diagnostic of the buffer's `Marks` under the pointer (`overlapping(at, at + 1)`, plus empty marks at `at`, or at the line end for the char before, where they're drawn), most severe first: the severity icon in its colour (`Severity::icon` / `icon_key`), the message (plain text, multi-line kept, wrapped as hover lines are), then `source(code)` dimmed (`ty(unresolved-reference)`, a theme key such as `editorHoverWidget.source`); several separated by a rule; then a rule and the server's hover below when it gives one
    - The diagnostics show when the rest ends, without waiting for the server; the hover request still goes out and its answer is appended to that popup (same group / buffer / revision / pointer run). No server, no `hoverProvider`, or a null / failed answer leaves the diagnostics-only popup (no "No hover information"). The popup's range is the union of the marks' ranges and the word, so moving along a long squiggle keeps it open; edits, scrolling and tab switches close it as now
    - `app/hover.rs`'s `hover_target` returns `None` on whitespace and past a line's end: a cell carrying a squiggle counts (trailing-whitespace warnings, a squiggle on a space, an empty range at a line end), other whitespace still doesn't
    - "Show Hover" (`editor.showHover`, Cmd+Alt+I) does the same at the cursor; "No hover information" only when neither exists
    - The popup's lines gain kinds (e.g. `Kind::Diagnostic(Severity)` / `Kind::Source`, built by `hover::diagnostic_lines`), so `ui::hover` keeps wrapping, scrolling and sizing one list of lines
  - Code actions / quick fixes: Cmd+. (Ctrl+. on Linux) lists `textDocument/codeAction` for the cursor / selection with the diagnostics there (quick fixes first, preferred marked, disabled ones greyed with their reason) in a popup list at the cursor; applying one runs its `WorkspaceEdit` (`rename::parse_workspace_edit` / `App::apply_*` like rename, one undo step per file) and/or its `command` (`workspace/executeCommand`; server-sent `workspace/applyEdit` answered by applying it, where today `server.rs` refuses it); `codeAction/resolve` when the server needs it (`resolveSupport` for `edit`); a lightbulb `💡` in the gutter on the cursor line when actions exist (requested after a short rest, cached by revision)
    - The request's `context.diagnostics` are the server's own diagnostics as it published them (range, `code`, `data`: rust-analyzer and clangd find their fixes by them), so each published diagnostic keeps its raw JSON alongside `diagnostics::Diagnostic` (in `lsp::diagnostics::Item` / the `Arc`); only that server's diagnostics go to it
  - The hover's diagnostics section ends with a "Quick Fix…" row when the server offers fixes there (asked with the hover): clicking it opens the Cmd+. list for that diagnostic, as VS Code
- Snippets (the [Snippets](#tasks) task): completions whose `insertTextFormat` is a snippet keep their tab stops instead of being flattened by `completion::strip_snippet`
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
- Git history (the [Git history](#tasks) task), read-only:
  - A "Git History" section in the Explorer view, under the tree (the side panel split vertically: a header row with the section's title, collapsible, and a draggable divider between the two; VS Code's Timeline): the commits that touched the active editor's file (a diff tab counts as its file), newest first — summary, author, relative date (`history::ago`), short id dimmed; nothing for a file outside a repository or untracked ("No history")
    - `git log -- <path>` on HEAD: a `git2` revwalk with the path filter (commits whose tree entry for the path differs from every parent's, as git's default history simplification), off the UI thread, read in pages of 200 as the list scrolls; refreshed when HEAD moves; renames not followed (`--follow`): the list stops at the commit that added the path
    - Selection, scrolling and the scroll bar like the other side lists (`Scrollable`); Enter or a click opens the commit
  - A commit tab (a read-only text tab, as a log tab: one buffer per commit, reused; the editor's cursor, selection, find, copy and splits): the commit as `git show --stat <commit>` prints it, with git's default colours (theme keys): `commit <full id>` (yellow) with `Merge: a b` for a merge, `Author: Name <email>`, `Date:` (git's default date format, the commit's own offset), the message indented four spaces, then the stat — ` path | 12 ++++++--` per file (insertions green, deletions red, the graph scaled to the tab's width as git scales to the terminal's; `Bin 0 -> 1234 bytes` for binary; renames `old => new` with git's default rename detection), then ` 3 files changed, 10 insertions(+), 2 deletions(-)`
    - Diffed against the first parent (what `git show --stat` prints for a merge too, git 2.53); a root commit against the empty tree. The `Merge:` header labels a merge's stat
    - Scrolls, selects and copies like a read-only editor (the text is plain; colours come from the line kinds); the file's own row in the stat is highlighted
    - Enter or a click on a stat row opens that file's change in a diff tab in the group to the right of the commit tab's (split right when there's none; reused for the next file clicked): the parent's text ↔ the commit's (`diff_view::Text::Commit`, a new side: a blob read from a commit's tree by path), titled `name (abc1234)`; no staging or revert buttons
  - Sessions keep commit tabs and commit diff tabs by commit id (dropped when the commit is gone), the section's collapsed state and divider position
- Merge conflicts (the [Merge conflicts](#tasks) task): editing a conflicted file, not merging (`git merge` / `rebase` stay on the command line)
  - A conflict is `<<<<<<<` … (`|||||||` base …, diff3 / zdiff3) `=======` … `>>>>>>>` at line starts, found by a line scan of the buffer (not the tree), cached by revision; nested or unterminated markers aren't a conflict
  - Drawn: the current block, the incoming block and the base block in VS Code's merge colours (`merge.currentHeaderBackground` / `.currentContentBackground`, `merge.incomingHeader…` / `…Content…`, `merge.commonHeader…` / `…Content…`), the marker lines labelled after their text (`(Current Change)` / `(Incoming Change)`, dim, as blame is drawn); an action row of buttons after the `<<<<<<<` line's text: Accept Current · Accept Incoming · Accept Both · Compare (clickable, `HitMap`), dropped from the right when they don't fit
  - Commands (palette, unbound unless noted): "Merge Conflict: Accept Current / Incoming / Both / None" for the conflict at each cursor (or every conflict a selection touches), "… Accept All Current / All Incoming / All Both" for the file, "Next / Previous Conflict" (Alt+F8 / Shift+Alt+F8 — check against F8's problems first), "Compare Current Conflict" (a diff tab of current ↔ incoming, read-only, no staging); each accept is one `EditKind::Other` step through `Buffer::edit`
  - Gutter / scroll bar: none in the milestone (git markers already show the region as changed); the status bar shows "2 conflicts" for a file with markers
  - Source Control: conflicted files in their own "Merge Changes" section above Staged (VS Code), `!` and `gitDecoration.conflictingResourceForeground`; opening one opens the file (not a diff); Stage File on a conflicted file = `git add` (marks it resolved), asking first when the buffer still has markers
- Discard and line-level staging (see [Tasks](#tasks)): "Git: Discard Changes" for a whole file and staging / unstaging / reverting only the selected lines — see [Tasks](#tasks)
- Current branch shown in the status bar (read-only)

### Git editor (commit messages)
jed as git's editor (the [Git editor](#tasks) task): `git config --global core.editor "jed --wait"`. git still does the commit; jed only edits the message file, so this stays inside "view and stage only".
- **Languages:** "Git Commit Message" (`git-commit`, VS Code's id) on tree-sitter-gitcommit: `COMMIT_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG`, `TAG_EDITMSG`, `EDIT_DESCRIPTION`, `NOTES_EDITMSG` by name. Coloured: the subject, its overflow past 50 columns (`overflow`, drawn as a warning), the blank line rule (text on line 2 flagged), comments, the status comment's branch names, change kinds and paths, trailers (`Signed-off-by:` keys), and below the scissors line (`# --- >8 ---`, `git commit -v`) the diff by line kinds (`diff --git`, `@@`, `+`, `-`: the diff view's theme keys) since jed has no injections. `LanguageConfig`: `#` line comment (Cmd+/), no auto-closed `'` (apostrophes in prose), `[` `(` brackets
  - "Git Rebase Message" (`git-rebase`, `git-rebase-todo`) opens as that language uncoloured but with `#` comments; its grammar (tree-sitter-git-rebase isn't on crates.io) is deferred — a syntax plugin can supply it. `addp-hunk-edit.diff` (`git add -p`, `e`) is plain text until there's a diff language
  - `core.commentChar` other than `#` (or `auto`): deferred; the grammar assumes `#`
- **Rulers:** a `rulers` setting (column list, default `[]`, VS Code's `editor.rulers`) drawn as a thin vertical line (`editorRuler.foreground`) behind the text; `git-commit`'s `LanguageConfig` defaults it to `[72]` (the body's wrap column; the subject's 50 is the overflow colour); a `rulers` the user set (even `[]`) wins over that default
- **Editor mode:** `jed <file>` where the file is one of git's message files inside a git directory (`.git/`, `.git/worktrees/<name>/`, a bare repo, `$GIT_DIR`: the parent holds `HEAD`), or `--wait` on any file, starts jed lean, for one file:
  - The root is the repository's work tree (so `.workspace` settings and associations apply), but nothing per-workspace happens: no session restored or written, no recent-workspace entry, no trust dialog, no language servers or tasks, no file history (paths under `.git/` never get any); the side and bottom panels stay closed (this is [single file mode](#tasks) with the git bits below)
  - The cursor starts at line 1, column 1 (git leaves the message empty above its comments); the status bar hint: "Save and close to commit · an empty message aborts"
  - Closing the file's last tab quits jed (Cmd+W), as does Cmd+Q; unsaved changes prompt as usual; the exit status is 0 — git aborts on an empty message itself, so "abort" is: delete the text, save, close (or quit without saving a message git left empty)
  - Crash backups still run (the message is the one thing worth keeping)
- **From a task terminal** (the [Tasks](#tasks-throwaway-terminals) PTYs): `git commit` run in a task must not start a second jed inside the first. jed listens on a Unix socket and tells its task processes where it is, as VS Code (`VSCODE_IPC_HOOK_CLI` in its integrated terminal), Neovim (`$NVIM` in `:terminal`) and Emacs (`with-editor`'s `emacsclient`) do:
  - The socket: `$XDG_RUNTIME_DIR/jed/<pid>.sock` (else `$TMPDIR/jed-<uid>/`, macOS's `TMPDIR` included), the directory `0700`, removed on quit and in the panic hook; listened on with tokio from `App::start` (not in tests unless a dir is set, as sessions). Only task PTYs get `JED_IPC=<path>` in their environment: nothing is advertised globally, so `jed` in an ordinary shell runs on its own
  - A `jed [--wait] <file>[:line[:col]]` started with `JED_IPC` set connects before doing anything else and sends one JSON line (`{"open": "<abs path>", "line": …, "col": …, "wait": true}`); the running jed opens the file in the active group as quick open does (the git message rules above: language, cursor at 1:1), focuses the editor, and with `wait` answers `{"closed": true}` when the buffer's last tab closes (any group) — the client prints nothing and exits 0; without `--wait` the answer comes at once. jed quitting first drops the connection: the client exits 1 (git then aborts)
  - A stale or refused socket (jed gone, another user's) falls back to starting jed normally, logged
  - Deferred: a global registry (`jed <file>` from any shell opening in an already running jed of that workspace, as `code` does): terminal editors open where they're started, and Recent Workspaces' kitty integration already finds a running jed

### Tasks (throwaway terminals)
- A terminal is spawned for one task, then thrown away when it finishes
- Templates: name, command, cwd, env — in `.workspace` and/or `~/.config/jed/tasks.toml` (see [Workspace](#workspace))
- Run from the command palette: "Run Task" (_TBD: shortcut_)
- Each run gets its own tab in the bottom panel
- Runs in a PTY, so tools keep their colours and progress bars
- On exit: show the exit code and keep the output until the tab is closed
- Kill a running task
- Out of scope for tasks: a general-purpose interactive shell — the [Integrated shell](#tasks) task adds one as its own editor tab
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
- The [Themes](#tasks) task:
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
- Replace across files, the whole-word toggle and collapsing / dismissing results (the [Search and replace](#tasks) task):
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
| Save As / Save All | Cmd+Shift+S / Cmd+Alt+S | Ctrl+Shift+S / Ctrl+Alt+S |
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
| Emacs-style keys in the editor (macOS's text keys): line start / end, left / right, up / down (in the completion list: its selection), delete left / right | Ctrl+A / E, Ctrl+B / F, Ctrl+P / N (+ Shift to select), Ctrl+H / D | — (Ctrl+D, Ctrl+H, … are VS Code's Linux commands) |

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

Milestones 1–13 are done and keep their numbers; everything after them is a [task](#tasks).

## Tasks
Everything still to build, and the finished work after milestone 13, in **no order**: build whichever is wanted next with `/task <name>`. A task is either:
- **single**: one agent, one commit (most of them), or
- **with subtasks**: a feature too big for one commit. It has a short id (`tools`, `soft_wrap`, …) for its subtask ids (`tools-a`, `tools-b`, …), commit subjects (`Tools view and logs (tools-a): …`), test file (`src/<id>_tests.rs`) and test names (`<id>_*`), plus "Needs", "Done when" and a `Subtasks (one agent and one commit each):` list, each subtask leaving the checks green. A single task that turns out too big is split this way (or into several single tasks).

A task that adds a Cmd binding also updates `docs/terminals/` (the `terminal_docs_list_every_cmd_binding` test fails otherwise) — see [Terminals](#terminals). A finished task stays here as one line, marked ✅; its full entry moves to [docs/plan-done.md](docs/plan-done.md).

- ✅ **Tools view and logs** (`tools`): a side panel view of every tool jed runs, each with its log in a read-only editor tab — [details](docs/plan-done.md)
- **Soft wrap** (`soft_wrap`): wrap long lines at the editor's width, with movement, scrolling and the mouse on screen rows — see [Editor](#editor)
  - Needs: the editor view and drawing (`ui::editor`), its scroll bar, mouse hits, sessions (the view's top)
  - The view stops assuming one screen row per line: a wrap layout per view (rows per line, byte ↔ (row, column)), used by drawing, scrolling, the scroll bar, mouse hits, popups and the find widget
  - `word_wrap` setting and "View: Toggle Word Wrap" (Alt+Z)
  - Done when: with wrap on, a Markdown file of long paragraphs and a 1 MB minified JSON file show every character without horizontal scrolling; Up / Down / Home / End / PageDown, click, drag and the scroll bar work on wrapped rows; resizing a split rewraps it with the cursor kept in view; typing in the 1 MB line stays within the frame budget; toggling off returns to the unwrapped view at the same cursor
  - Subtasks (one agent and one commit each):
    - soft_wrap-a: the wrap model (`wrap.rs`: a line's break points for a width — after spaces, else grapheme boundaries, wide chars and tabs whole, continuation indentation; byte ↔ (row, display column); a per-view cache by (revision, width) mapped through edits; the total row count kept incrementally); the `word_wrap` setting and the toggle command, not drawn yet; unit tests incl. a reference check against a slow per-line wrap
    - soft_wrap-b: drawing the text on screen rows: the editor draws wrapped rows (gutter number on a line's first row; text, whitespace, selections and cursors on the right rows), the view's top as (line, row), no horizontal scroll while wrapping
    - soft_wrap-c: the overlays on wrapped rows: find matches, brackets, squiggles, git markers, blame, the lightbulb and anything else drawn per line
    - soft_wrap-d: scrolling over rows: reveal, the wheel, PageUp / PageDown's scroll, the scroll bar (`Scrollable` on rows), resizing a split rewraps with the cursor kept in view
    - soft_wrap-e: movement: Up / Down / PageUp / PageDown with the goal column on rows, Home / End row first, at every selection (the 61k-selection budget)
    - soft_wrap-f: the mouse and popups: hits (click, drag, double / triple click, Alt+Click) to positions in wrapped rows; the find widget and completion / hover / code action popups placed at the wrapped cursor
    - soft_wrap-g: sessions keep the (line, row) top and the toggle; the 1 MB release budget; the done-when tests, gaps and hand checks; marks the task ✅
- ✅ **Untitled files and Save As** (`untitled`): new untitled buffers, Save As and Save All — [details](docs/plan-done.md)
- **Language features** (`lang`): go to symbol, signature help and breadcrumbs / sticky scroll (code actions are in [Diagnostics and quick fixes](#tasks)) — see [LSP](#lsp)
  - Needs: language servers (M9), completion's popup, kind icons and request handling (`App::send_request`), quick open (M5), back / forward navigation (see [Tasks](#tasks); jumps recorded when it's there)
  - Go to symbol in file (Cmd+Shift+O, `@`) and workspace (Cmd+T, `#`); signature help (trigger characters, Cmd+Shift+Space); breadcrumbs and sticky scroll (settings, off by default); Cmd bindings update `docs/terminals/`
  - Done when: with rust-analyzer on a small cargo project: Cmd+Shift+O lists the file's items with their containers and Enter jumps (Esc after moving the selection goes back); Cmd+T finds a struct in another file; typing `foo(` shows its signature with the active parameter moving on `,` and closing on `)`; with breadcrumbs on, the row shows path › impl › fn following the cursor; clangd does the same on a C file
  - Subtasks (one agent and one commit each):
    - lang-a: go to symbol in file: `documentSymbol` (both shapes: `DocumentSymbol[]` tree, `SymbolInformation[]`) → a symbols model, the `@` quick open mode and Cmd+Shift+O, preview while moving, Esc restores
    - lang-b: go to symbol in workspace: `workspace/symbol`, `#` mode and Cmd+T, debounced re-requests with older ones cancelled, rows with the file
    - lang-c: signature help: the popup, trigger / retrigger characters, the active parameter, overloads, closing rules, next to the completion list
    - lang-d: breadcrumbs (setting, off by default): the row above the editor from `documentSymbol` (the tree-sitter tree without a server), following the cursor
    - lang-e: sticky scroll (setting, off by default): the enclosing scopes' first lines pinned at the top while scrolling, from the same model
    - lang-f: the done-when tests (real rust-analyzer and clangd, skipped with a note when absent), gaps and hand checks; marks the task ✅
- ✅ **Diagnostics and quick fixes** (`fixes`): a squiggle's diagnostics in the hover popup, and code actions / quick fixes (Cmd+., Ctrl+. on Linux), as VS Code — [details](docs/plan-done.md)
- **Themes** (`themes`): a built-in Light+, a `theme` setting, a picker with live preview and theme files — see [Themes](#themes)
  - Needs: `Theme` and its keys (M8), settings layering (M8), the palette's pickers (M5)
  - Light+ with the same keys as Dark+; `theme` setting; "Preferences: Color Theme" with live preview, saved to `settings.toml`; theme files from `~/.config/jed/themes/`
  - Done when: picking Light+ recolours everything — editor, syntax, side bar, panel, popups, diff view, task terminals — with no Dark+ colour left (a test draws every view under both and finds no colour of the other theme); Esc in the picker restores the old theme; the choice survives a restart; a theme file dropped into the themes folder is listed and applies, and a broken one is reported and falls back
  - Subtasks (one agent and one commit each):
    - themes-a: Light+ (`Theme::light_plus`, the same-keys test, 256-colour fallback) and the `theme` setting (`dark` / `light`), live on a `.workspace` reload
    - themes-b: "Preferences: Color Theme": the picker with live preview, Esc restores, Enter writes `theme` to the user's `settings.toml` keeping comments
    - themes-c: theme files: decide the format first (see [Themes](#themes)), load from `~/.config/jed/themes/`, list in the picker, fallbacks and error reports
    - themes-d: the done-when tests (every view drawn under both themes, no colour of the other left), gaps and hand checks; marks the task ✅
- ✅ **Git history** (`git_history`): a file's commits in the Explorer view, each commit shown as `git show --stat` in its own tab, and a file's change in a commit as a diff — [details](docs/plan-done.md)
- ✅ **Hex view** (`hex`): binary files open read-only as a `hexdump -C`, rows a power of two bytes wide, with byte find in ASCII or hex — [details](docs/plan-done.md)
- ✅ **Indentation** (`indentation`): detect spaces vs tabs and the indent size per buffer, switch between them, and convert a file's indentation — [details](docs/plan-done.md)
- ✅ **Syntax plugins** (`syntax_plugins`): tree-sitter grammars from `~/.config/jed/languages/<id>/`, built with the system C compiler and loaded at runtime — [details](docs/plan-done.md)
- **Snippets** (`snippets`): LSP snippet completions with tab stops, mirrors and choices — see [LSP](#lsp)
  - Needs: completion (M9: `completion::insert`, the popup), multi-cursor selections, `ChangeSet` mapping (as `diagnostics::Marks`)
  - A snippet parser and a per-view snippet session (tab stops as mapped ranges, Tab / Shift+Tab, `$0`, ending rules), `snippetSupport` advertised, placeholders drawn
  - Done when: with rust-analyzer on a small cargo project, completing a function call inserts `name(arg1, arg2)` with `arg1` selected, Tab moves to `arg2`, Tab again leaves the cursor after `)`; a `match` / `for` postfix snippet with a mirrored name edits both places as one types; Esc ends the session and Tab indents again; the same completion at three cursors gives three sessions' worth of stops moving together; undo removes the insertion in one step; clangd's function-argument placeholders behave the same
  - Subtasks (one agent and one commit each):
    - snippets-a: the model, no UI: `snippet.rs` — the parser (grammar above, a reference test against VS Code's documented examples), resolving variables, laying the text out at each cursor (indentation, EOL) and the stops' ranges; `Session` (stops → ranges per cursor, `map(&ChangeSet)`, next / previous, `contains(selections)` for the ending rule); unit tests
    - snippets-b: in the app: `snippetSupport: true`, `completion::insert` keeps the snippet and starts the session, `Context::Snippet` keys (Tab / Shift+Tab), ending rules (Esc, outside edits / moves, click, undo)
    - snippets-c: choices' list, placeholders drawn and their theme keys
    - snippets-d: the done-when tests (real rust-analyzer and clangd, skipped with a note when absent; the fake server's scripted snippet completions for the rest), the 61k-selection budget unaffected when no session runs, gaps and hand checks; marks the task ✅
- ✅ **Search and replace** (`replace`): replace across files from the Search view, plus its whole-word toggle and collapsing / dismissing results — [details](docs/plan-done.md)
- ✅ **Merge conflicts** (`conflicts`): conflict markers coloured with Accept Current / Incoming / Both in the editor, and a "Merge Changes" section in Source Control — [details](docs/plan-done.md)
- ✅ **Git editor** (`git_editor`): jed as `core.editor` — commit messages coloured, a lean one-file editor mode, and `git commit` from a task terminal opening in the running jed — [details](docs/plan-done.md)
- ✅ **CI** — [details](docs/plan-done.md)
- ✅ **Terminal config snippets** — [details](docs/plan-done.md)
- **Live reload of settings**: `settings.toml`, `.workspace` overrides and `keybindings.toml` reloaded when they change (the files themselves load at startup since M8, 8d)
- ✅ **Move tabs** — [details](docs/plan-done.md)
- ✅ **Drag-and-drop tabs** (`drag_tabs`): mouse-dragging a tab along its bar reorders it, onto another group's bar or editor moves it there, and onto an editor's edge (a quarter-wide snap band) splits that group on that side, the drop zone shaded — [details](docs/plan-done.md)
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
- **Feature-based end-to-end test files** (four steps, in order, each done as its own `/task` by its bold name below): the `src/milestone*_tests.rs` files (and `src/todo_tests.rs`) are named after when the work was done, not what they test (`milestone2_tests.rs` = workspace, trust, explorer, tabs, file ops); move them to `src/tests/<feature>.rs` (e.g. `editing`, `multicursor`, `find`, `workspace_search`, `explorer`, `splits`, `palette`, `highlighting`, `lsp`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …), splitting a milestone's file where it spans features and merging tests of one feature from several milestones. Rules for every step:
  - Pure move and rename: no test logic changes; the test count stays the same (compare `cargo test -- --list` before / after)
  - Tests renamed from `milestone_N_*` / `<id>_*` to the behaviour; `done_when` tests keep a recognisable name (`done_when_*`), their file's `//!` doc saying which milestone's "Done when" they cover; the release budget tests become `budget_*` (CLAUDE.md's `cargo test --release milestone` commands change to `budget` in the step that renames the last of them)
  - Each step updates the CLAUDE.md references it moves (the Tests section's helper paths, the module map entry, nested CLAUDE.md files) and PLAN.md's references to test names
  - **Test support module**: `src/tests/mod.rs` and `src/tests/support.rs` with the shared helpers (`open_app`, `cmd`, `settle_git`, `draw_sized`, …, now duplicated or reached across files as `milestone10_tests::…`); callers updated, duplicates removed, no test moved yet
  - **Feature test files: editing** — M1, M4 and M6 (`milestone_tests.rs`, `milestone4_tests.rs`, `milestone6_tests.rs`) into `editing`, `clipboard`, `multicursor`, `find`, plus `todo_tests`' modules of those features
  - **Feature test files: workbench** — M2, M3, M5, M7 and M8 into `workspace`, `explorer`, `tabs`, `splits`, `palette`, `workspace_search`, `highlighting`, `settings`, plus `todo_tests`' modules of those features
  - **Feature test files: tools** — M9–M13 and the rest of `todo_tests` into `lsp`, `problems`, `completion`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …; then CLAUDE.md, `.claude/commands/task.md` say new tests go in the feature's file (a task with subtasks too, instead of `src/<id>_tests.rs`)
- ✅ **Rust language server** — [details](docs/plan-done.md)
- ✅ **Inline git blame** — [details](docs/plan-done.md)
- ✅ **Cursor style setting** — [details](docs/plan-done.md)
- ✅ **Syntax highlighting in the diff view** — [details](docs/plan-done.md)
- ✅ **Indent / outdent lines** — [details](docs/plan-done.md)
- **Reopen closed tab**: Cmd+Shift+T; a stack of closed tabs per window with their view state, files reopened from disk; in the palette; Cmd bindings update `docs/terminals/`
- **Close tabs and go to the Nth tab**: "Close All", "Close Others", "Close Saved" (in a group; prompts for unsaved like closing one), go to the Nth tab in the group (Ctrl+1–9 on macOS, Alt+1–9 on Linux, as VS Code; 9 = last); in the palette, and on the tab bar's right-click once there are context menus
- **More line commands**: delete line (Cmd+Shift+K), select line (Cmd+L, repeated: the next line too), join lines (Ctrl+J on macOS; unbound on Linux, where Ctrl+J is the panel), insert line below / above (Cmd+Enter / Cmd+Shift+Enter, with auto-indent); at every selection (`LineWalker`, one undo step each, the line-command budget); Cmd bindings update `docs/terminals/`
- **Transform, sort and dedupe lines** (palette, unbound): transform to upper / lower / title case, sort lines ascending / descending, delete duplicate lines; at every selection (`LineWalker`, one undo step each, the line-command budget)
- **Expand / shrink selection** (Shift+Alt+Right / Left on Linux, Ctrl+Shift+Cmd+Right / Left on macOS, as VS Code): grow each selection to the enclosing tree-sitter node (word first, then string contents, the string, the bracket contents, the brackets, the statement, the block, …), shrink back through the same history (kept per view, cleared on other selection changes); without a tree: word, line, all; at every selection
- **Keyboard Shortcuts view** (`keys`): "Preferences: Keyboard Shortcuts" opens a full-screen list of every command with its key(s) and context (fuzzy on name or key text, e.g. `ctrl+shift`; a "record keys" mode: the next key combo pressed filters to its bindings, VS Code's); shows where a binding came from (default / `keybindings.toml`) and user overrides; Enter on a row opens `keybindings.toml` with a `[[binding]]` stub for that command (creating the file like settings)
  - Needs: the `Keymap` (track where each binding came from), the palette's command list and `nucleo` fuzzy filter (`palette::command_rows`), the cheat sheet overlay's `Scrollable` / `ScrollTarget` pattern, settings' `create_if_missing` / `TEMPLATE` (the "create like settings" pattern)
  - Done when: "Preferences: Keyboard Shortcuts" (palette, unbound) opens the view; every command shows its title, key(s), context and source; the filter matches name or key text (`ctrl+shift`); record keys filters to the pressed combo's bindings; Enter writes a `[[binding]]` stub for that command, creating `keybindings.toml` if missing (like settings); a user `keybindings.toml` binding shows as an override over the default
  - Subtasks (one agent and one commit each):
    - ✅ keys-a: the `Keymap` records each binding's source (default / `keybindings.toml`) and lists every binding with source and context; the `Command::KeyboardShortcuts` command (`preferences.keyboardShortcuts`, palette, unbound) opens and closes a new (empty) view end-to-end (for keys-b: `KeysView` is an empty unit struct, `source_of` returns `Default` for unbound commands, `bindings_with_source` / `source_of` carry a `dead_code` allow until the view uses them)
    - ✅ keys-b: the view: a full-screen scrollable list of every command (title, key, context, source), fuzzy search on name or key text (the `nucleo` path as `palette::command_rows`), selection, scrolling and a scroll bar (the cheat sheet's `Scrollable` / `ScrollTarget` pattern); Esc closes (for keys-c: `KeysView` has `query` / `selected` / `selected_row` (with a `dead_code` allow until used) ready for record-keys and the Enter→stub)
    - keys-c: record-keys mode (the next combo filters to its bindings) and Enter on a row writes the `[[binding]]` stub (creating the file like settings); the done-when tests, gaps and hand checks; marks the task ✅
- ✅ **Keyboard cheat sheet overlay** — [details](docs/plan-done.md)
- **Word-based completion**: without a language server (plain text, YAML, TOML, Markdown, … or a server returning nothing), Ctrl+Space and quick suggestions offer the words in the open buffers (current buffer first, then others; VS Code's `editor.wordBasedSuggestions: matchingDocuments`), filtered with the same `completion::Filter`; collected off the UI thread per buffer and cached by revision; capped (e.g. 2 MB of text, 20k words); a setting to turn it off
- **Language server setup docs**: human-facing `docs/language-servers.md` (folding in `docs/clangd.md` and `docs/ty.md`, which exist: ty's install, `.venv`, `ty.toml` and SCons are in the latter) (linked from a README / `docs/` index): which servers jed starts by itself (`clangd` for C / C++, `ty server` for Python, `rust-analyzer`) and how to install each on macOS (Homebrew, `uv tool install ty`, `rustup component add rust-analyzer`) and Linux (apt / dnf, uv, rustup); making them findable (`PATH`, a project `.venv`); clangd's `compile_commands.json` (CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, `bear -- make`, SCons) and `.clangd`; ty's project config (`pyproject.toml` / `ty.toml`, the venv) and the SCons support jed adds; adding any other server through `.workspace` `[lsp.<language id>]` (command, args, `initialization_options`, with worked examples, e.g. a YAML or TOML server) and why workspace trust asks first; what works (the v1 features) and the keys for each; troubleshooting (the status bar / Problems, `--log-level` and "Log: Open" with the `lsp::wire` / `lsp::stderr` targets, a server that fails is not retried until restart); kept current when servers or config keys change (add a line to CLAUDE.md's conventions)
- ✅ **Scroll bars in the editors** — [details](docs/plan-done.md)
- ✅ **Revert hunks in the diff view** — [details](docs/plan-done.md)
- **Kitty integration for recent workspaces** (deferred from M13; see [Recent workspaces](#workspace)): `jed_workspace` user var (already set: Terminal window identity); `kitten @ ls` marks rows already open and focuses that window; `kitten @ launch --type=os-window --cwd <path> jed <path>` for one not open; off the UI thread with a timeout, message fallback; tests through a fake `kitten` on `PATH` (canned `ls` JSON, records its arguments); `docs/terminals/kitty.md` (`allow_remote_control`). Kitty isn't installed on the Linux test box: real-kitty behaviour is a hand check
- **Bug reporter** (`bug_report`): "Help: Report Bug" (`help.reportBug`, palette, unbound) asks for a brief description of what happened (an `Input` prompt, multi-line not needed), then writes one self-contained Markdown file, `<state dir>/bug-reports/<UTC time>.md`, and shows its path in a message (also copied to the clipboard). Contents, each a section: the description; jed's version and git commit (built in at compile time), OS, terminal (`$TERM`, `$TERM_PROGRAM`, kitty keyboard protocol on or not, size, truecolor); the workspace (root, trust, `.workspace` settings, the layered settings, `keybindings.toml` overrides and load errors); the layout (groups and split tree, each group's tabs — path relative to the root, language, size, line count, dirty / deleted / read-only / disk conflict, EOL, encoding, selection count and primary's position, scroll — the active group and tab, focus, side / bottom panel and their views, open prompt / palette / find widget); language servers (per server: command, state, encoding, pending requests, diagnostics counts), git (branch label, entry counts), tasks (name, status), background work in flight (saves, parses, searches, backups); the last messages shown and the last ~50 commands run (a ring buffer of `Command::name()`s with times; no typed text); and the tail of the current log file (`newest_log`, last ~500 lines). Never file contents or typed text (privacy: the file is meant to be attached to a report); paths outside the workspace shown with `~`. Gathered on the UI thread as a cheap snapshot (O(groups + tabs + buffers), no rope scans), written in `spawn_blocking`. The panic hook could later write the same report (description = the panic message)
  - Needs: prompts, `App::notify`, the clipboard, `logging::newest_log`, the state dir, `build.rs`'s `JED_VERSION`
  - Done when: Report Bug with a description writes the file and shows / copies its path; every section above is there for a workspace with two groups, a running fake server, a git repo and a task; no file contents or typed text appear in it (a test types a marker string and searches the report for it)
  - Subtasks (one agent and one commit each):
    - bug_report-a: the command, the prompt, the file written off-thread and its path shown and copied; the description, version / commit (`build.rs`), OS, terminal, workspace and settings sections, and the log tail
    - bug_report-b: the app state sections: layout, language servers, git, tasks, background work, the last messages and the command ring buffer; the privacy test, gaps and hand checks; marks the task ✅
- ✅ **Scroll bars in the side bar's lists** — [details](docs/plan-done.md)
- ✅ **First reveal in a never-drawn list** — [details](docs/plan-done.md)
- ✅ **Horizontal scrolling and its scroll bar** (hscroll): the wheel scrolls text tabs sideways, and text and diff tabs get a horizontal scroll bar — [details](docs/plan-done.md)
- **PTY smoke tests** (`pty`): a few end-to-end tests that run the real `jed` binary (`env!("CARGO_BIN_EXE_jed")`) in a pseudo-terminal (`portable-pty`, already a dependency) and parse what it writes with `vt100` (as task runs do), so the bytes crossterm emits and the bytes a terminal sends are tested, not just `TestBackend`: (1) after start the vt100 screen equals the `TestBackend` frame of the same state (text and colours, cell by cell; also after an edit and a scroll, to catch stale cells from ratatui's diff flush); (2) real input sequences reach the right commands — legacy and kitty-protocol (`CSI u`) keys incl. `Cmd+!` / Ctrl+@, bracketed paste, SGR mouse clicks and wheel; (3) start / quit leaves the terminal as it was (raw mode off, alt screen left, mouse / bracketed paste / kitty flags popped: check the mode-setting sequences in the output and the PTY's termios), and so does a forced panic (a test-only `JED_TEST_PANIC` hook or similar), which also leaves its crash backup; (4) wide graphemes (CJK, emoji, combining marks) land in the columns `pos::display_grapheme` says. Isolated `XDG_STATE_HOME` / `XDG_CONFIG_HOME` per test (they're child processes, so setting them is fine), a fixed size (e.g. 100×30), waits on screen contents with a timeout (no sleeps), `TERM=xterm-256color` and `COLORTERM=truecolor`; skip with a note where PTYs aren't available. Not a cross-emulator matrix: vt100 is one emulator; termless (TypeScript; xterm.js / Ghostty backends) or esctest could be added later if wide-character or protocol differences between terminals become a real problem
  - Needs: `portable-pty` and `vt100` (task runs), the panic hook and crash backups (M13), `terminal.rs`'s mode sequences
  - Done when: the four kinds of test above pass on Linux and macOS and are skipped with a note where PTYs aren't available
  - Subtasks (one agent and one commit each):
    - pty-a: the harness (spawn the binary in a PTY with isolated dirs, a vt100 parser, wait-for-screen with a timeout, the skip) and test (1), screen equals the `TestBackend` frame, plus (4), wide graphemes
    - pty-b: test (2), real input sequences: legacy and kitty keys, bracketed paste, SGR mouse
    - pty-c: test (3), the terminal restored on quit and on a forced panic (the test-only panic hook, the crash backup); gaps and hand checks; marks the task ✅
- **File History section in the Explorer** (`history_view`; [File history](#file-history), after the [Git history](#tasks) task): the active file's saved snapshots as a third Explorer section, built like "Git History" (VS Code's Timeline shows both sources; jed keeps them apart): the tree, then "FILE HISTORY", then "GIT HISTORY", each section with its header row (click toggles collapse), a draggable divider above it, and the tree keeping its minimum height (`git_history::split` generalised to N sections: each section's height and collapsed state; a collapsed one is its header only)
  - Rows: the entries newest first, as the picker shows them — relative time (`history::ago`), size (`size_label`), the UTC time dimmed (theme keys, e.g. `fileHistory.date` / `.size`); "No history" for a file with none, a file outside the workspace, or a non-file tab (log, commit); a diff tab counts as its file (a history diff as its file too)
  - Follows the active file like Git History; entries read off-thread (`history::entries` in `spawn_blocking`, generation-checked); re-read after `HistoryDone` for that file (a save added an entry, a rename moved them) — no polling, and nothing read while the section is collapsed
  - Selection, scrolling and the scroll bar as the other side lists (`Scrollable`, a `ScrollTarget` of its own); keys in a new `Context::FileHistory` (Up / Down / PageUp / PageDown / Home / End, Left / Esc back to the tree); Enter or a click opens Compare (`Pair::history(snapshot)` diff tab, as the picker's Compare); "File History: Restore Entry" (palette, and a key in the section, e.g. Cmd+Enter / Ctrl+Enter — decide) restores the selected entry into the buffer (`restore` as today: one undo step, nothing written)
  - "File History: Show" focuses the section (expanding it) instead of opening the picker; the picker stays reachable for Restore and from the explorer's / tab bar's right-click until context menus exist — decide in the item whether the picker goes
  - Sessions keep its collapsed state and height (`SideState`, serde defaults, `VERSION` unchanged)
  - Tests: entries listed newest first after saves (and a new row after one more save), switching tabs switches the list, Enter / click opens the history diff, Restore through the section, collapse / divider drag / scroll bar, session round trip; its reveal defers to the first draw like the other lists' (a pending flag run by `set_height`)
  - Needs: the Explorer's Git History section (`git_history.rs`, `git_history::split`), `history.rs`, the diff view's `Pair::history`, `Scrollable`, sessions' `SideState`
  - Done when: the tests above pass
  - Subtasks (one agent and one commit each):
    - history_view-a: `git_history::split` generalised to N sections (heights, collapsed states, dividers, the tree's minimum) with an empty "FILE HISTORY" section drawn above "GIT HISTORY"; Git History unchanged (its tests still pass)
    - history_view-b: the rows: following the active file, entries read off-thread and re-read after `HistoryDone`, "No history" cases, drawing and theme keys, selection / scrolling / the scroll bar
    - history_view-c: `Context::FileHistory` keys, Enter / click opens Compare, Restore Entry, "File History: Show" and the picker decision, sessions; the remaining tests, gaps and hand checks; marks the task ✅
- **Change End of Line Sequence** ([Files](#files)): "Change End of Line Sequence" (palette, unbound; a click on the status bar's `LF` / `CRLF`) picks LF or CRLF for the buffer: every line break rewritten as one `EditKind::Other` step through `Buffer::edit` (undo brings the old ones back), new lines then use it, saved as is; a `files_eol` setting (`auto` (detect, default) / `lf` / `crlf`) for new files and files without a line break
- **Block comment and bracket jumps** ([Editor](#editor)): "Toggle Block Comment" (`edit.blockComment`, Shift+Alt+A on both platforms, VS Code's) with each language's block tokens in `LanguageConfig` (`/* */` C / C++ / Rust / JSONC / CSS-like, `<!-- -->` XML / Markdown, `"""` Python as VS Code; none: nothing): per selection, remove when the selection (trimmed of surrounding whitespace) starts and ends with the tokens, else wrap it (`/* ` … ` */`); an empty cursor inserts `/*  */` with the cursor in the middle; selections keep their text; one `EditKind::Other` step, every selection. "Go to Bracket" (`editor.jumpToBracket`, Cmd+Shift+\\ on macOS, Ctrl+Shift+\\ on Linux): each cursor to its bracket's partner (`brackets::match_at`), else to the closing bracket of the innermost pair around it (`brackets::Openers`); "Select to Bracket" (unbound): the selection grows to cover the enclosing pair, brackets included. Both at every cursor (the 61k-selection budget). Cmd bindings update `docs/terminals/`
- **Explorer path commands** ([Explorer](#explorer-file-tree), palette entries, no context menus needed): "File: Copy Path of Active File" / "File: Copy Relative Path of Active File" (VS Code's, unbound; a diff / commit / log tab: its file or nothing), "Explorer: Copy Path" / "Explorer: Copy Relative Path" (the selected row, file or folder; `/`-separated, relative to the root), "File: Reveal in Finder" (Linux: "Open Containing Folder" — `open -R <path>` / `xdg-open <parent>` off the UI thread, failure reported; for the active file, and "Explorer: Reveal in Finder" for the selected row), "Explorer: Reveal Active File" (expand to and select the active file's row, focusing the explorer), "Explorer: Collapse Folders" (every folder closed, the selection on its top-level row); all through `Clipboard::write` (OSC 52 over SSH), each with a short status message
- **Open File by path** ([Files](#files), palette entry): "File: Open File…" (`file.open`, unbound; Cmd+O stays the terminal's) opens an `Input` prompt for a path — absolute, `~/…`, or relative to the root, with `:line[:col]` as the command line takes it — Tab completes the path segment (a listing off the UI thread, common prefix, a second Tab lists the choices in the prompt's hint), Enter opens it as quick open does (`open_file`, back / forward records it); a folder → a message with `jed <path>` (jed never switches workspace); a missing file → a message (creating it is the Untitled / Save As milestone's). Files outside the root work as today (no watch, no history)
- **Compare any two files** ([Git](#git-view-and-stage-only) diff view): "File: Compare Active File With…" (palette, unbound) picks another file with quick open's file list and opens a diff tab of that file ↔ the active buffer (`diff_view::Text::File(path)` — a file's text, its open buffer's when open, else the disk — on the left; the right side `Buffer` as File History's Compare); "Explorer: Select for Compare" then "Explorer: Compare with Selected" (selected rows, or the active file for the second); titled `a.rs ↔ b.rs`, no staging / revert, follows edits on either side (`Later` reloads as now), next / previous change; sessions keep it (`Side::File`); binary files say "binary" as now
- **Discard changes in Source Control** ([Git](#git-view-and-stage-only)): a `↶` button on each Changes row (beside `+`) and "Git: Discard Changes" (palette, `git.clean` — the active file or the selected row) / "Git: Discard All Changes" (the Changes section's header button): after a confirm prompt naming the file(s) ("Discard changes in a.rs?"; for a closed file it can't be undone, though file history keeps its last save from jed), the work tree file gets the index's text back (= `git checkout -- path`; an open buffer through `Buffer::edit`, one undo step, left dirty, as Revert hunk (so the confirm says the buffer needs saving); a closed file written off-thread, keeping permissions); an untracked file is moved to the Trash (VS Code deletes it), its tab marked deleted; a staged-only change is left alone (the Staged section isn't touched); conflicted files refused (merge conflicts milestone). Refused while the file has a save in flight, like staging
- **Stage / unstage / revert selected lines** ([Git](#git-view-and-stage-only)): VS Code's "Git: Stage Selected Ranges" / "Unstage Selected Ranges" / "Revert Selected Ranges" (palette, unbound): from a text tab, the lines its selections touch (every selection; an empty cursor = its line). Stage applies only those lines of each context-0 index → disk hunk to the index blob (an added line inside = added, a removed line inside = removed, the rest of the hunk left as it was — `git add -p`'s `e`dit result), checked against `Stale` like `stage_hunk`; unstage likewise HEAD → index; revert the same on the work tree (the buffer through `Buffer::edit`, one undo step). Saves a dirty buffer first as hunk staging does. A `git::stage::stage_lines` / `unstage_lines` / `revert_lines` in the model with tests against `git apply --cached` of the equivalent hand-edited patch
- **Selected lines in diff tabs** (after the item above): from an unstaged / staged diff tab, a row range selected with Shift + Up / Down (a new selection on `DiffTab`, drawn with `editor.selection`) is what Stage / Unstage / Revert Selected Ranges act on
- **SCons: a real SCons for `site_scons` in the docs** ([LSP](#lsp), `docs/ty.md` section 3): helpers in `site_scons/` that `from SCons.Script import …` / `import SCons.Util` only resolve when ty sees the `SCons` package. Document both ways, checked against real ty and SCons in a scratch project as the rest of the section was: `scons` installed into the `.venv` ty uses (`.venv/bin/python -m pip install scons`), or SCons's install folder on `extra-paths` (`/usr/lib/python3/dist-packages` on Debian / Ubuntu, `/usr/lib/scons` on some distros, `$(python3 -c 'import SCons, os; print(os.path.dirname(os.path.dirname(SCons.__file__)))')` to find it). Say why the stub isn't replaced by `from SCons.Script import *` (SCons's run-time globals `Glob` / `Default` / `Export` / `Import` are missing and `env.Program` comes out as `Never`, as tried with ty 0.0.90). A row in the troubleshooting table for `Cannot resolve imported module 'SCons…'`. Docs only
- **SCons settings in `.workspace`** ([LSP](#lsp), `src/lsp/scons.rs`, after the docs item above): a `[scons]` table, read with the rest of `.workspace` and live on its reload (ty restarted when it changes, as for an `[lsp.*]` change):
  - `sconstruct = "build/SConstruct"` (relative to the root, or absolute): the build file when it isn't at the root. `scons::prepare` then treats the workspace as an SCons one from that path (today only the root's `SConstruct` / `Sconstruct` / `sconstruct` / `….py`), and `site_scons/` beside it goes on ty's `extra-paths` automatically (SCons puts it on `sys.path`), as does the root's `site_scons/` without the setting
  - `scons_path = "/usr/lib/python3/dist-packages"` (a folder containing the `SCons` package; `~` expanded): added to ty's `extra-paths` beside the stub folder (`scons::with_extra_path`), so `site_scons` helpers importing `SCons` resolve; a folder without `SCons/__init__.py` logs a warning and is still passed. jed's stub stays (it covers the run-time globals the real package can't)
  - `enabled = false` turns the stub off (for a workspace that isn't an SCons one but has a file named `SConstruct`)
  - No trust prompt: these name no commands (as `[settings]`); `docs/ty.md` section 3 updated to use them in place of the "open the subfolder" note; `docs/dev/conventions.md` if a rule comes of it
  - Tests: `prepare` / `with_extra_path` unit tests for each key (a subfolder `SConstruct`, its `site_scons`, `scons_path` merged after the project's own `extra-paths` and the stub's, `enabled = false` → nothing written); the `initialize` params through the fake server with a `.workspace` reload changing them; one real-ty test (skipped when absent, the `.venv/bin/ty` lookup) on a subfolder `SConstruct` with a `site_scons` helper importing `SCons.Script` and `scons_path` pointing at a scratch install
- **Live diagnostics while editing** ([LSP](#lsp)): squiggles follow the unsaved text as the user types, for every server. Today each edit already goes out as an incremental `didChange` each loop turn and versioned publishes are mapped through later edits (`lsp/diagnostics.rs`), so clangd and ty re-check unsaved text by themselves; the gaps:
  - Pull diagnostics: advertise `textDocument.diagnostic` and, for a server with `diagnosticProvider`, ask `textDocument/diagnostic` (with `previousResultId`; an `unchanged` report keeps the marks) after a short rest in typing (~300 ms, one timer like the lightbulb's; older requests cancelled through `App::send_request`), on open, and on `workspace/diagnostic/refresh` (answered, then every open document of that server re-asked). `relatedDocuments` and `interFileDependencies` re-ask the other open files of that server. Pull results go into the same store as published ones (`Marks`, Problems, the raw JSON for code actions); a server that does both keeps whichever came for the newer version
  - Never block typing: requests and answers are async as now; a burst of edits sends one request after the rest, not one per key (checked at 61k selections: the timer, not the edit, does the work)
  - Tests: the fake server gains `diagnosticProvider` and scripted pull reports (`full` / `unchanged`, a `resultId`), and `workspace/diagnostic/refresh`; typing then resting asks once with the current version and draws the answer; an `unchanged` keeps the marks; a stale answer (older version) is dropped
- **Stale cargo check diagnostics** ([LSP](#lsp), after the item above): rust-analyzer's own diagnostics are live already; rustc's (`cargo check`, type and borrow errors) only run on save, because cargo reads the files on disk. Keep save as the trigger (VS Code's default), but make it visible: after an edit, the check's marks of that file are drawn dimmed (a theme key, e.g. `editorDiagnostic.stale`) and the status bar says "cargo check: on save" until the next check publishes; a `.workspace` example in the docs for `rust-analyzer.diagnostics.experimental.enable` (more live diagnostics from rust-analyzer itself). No auto-save. Tests: rust-analyzer's real check marks dimmed after an edit and back to normal after save (real-server test, skipped when absent)
- **Self-update from GitHub releases** (`update`): "jed: Check for Updates" (`app.checkForUpdates`, palette, unbound) and `jed --update` (run before raw mode, progress printed to stdout) fetch the newest release of `jimcx/jed` and replace the running binary with it. The releases are the ones CI's `release` job publishes on a `v*` tag (`.github/workflows/build.yml`: `jed-<linux|macos>-<uname -m>.tar.gz`, the stripped `jed` inside)
  - Check: `GET https://api.github.com/repos/jimcx/jed/releases/latest` off the UI thread (`spawn_blocking`, a blocking HTTP client with rustls — e.g. `ureq` — and a `User-Agent`; a timeout of a few seconds), reported as an `AppEvent`. Compared with `JED_VERSION` (`build.rs`): a tag build (`v1.2.0`) updates only to a newer semver tag ("jed is up to date (v1.2.0)" otherwise); a branch / commit build (`main (8d91187)`) is a development build and is always offered the latest release with a note that it replaces a source build. No network error on the UI thread blocks or panics: failures (offline, rate-limited, no asset for this platform) are one `notify` line
  - Offer: a prompt "jed v1.3.0 is available (you have v1.2.0). Update / Release Notes / Not Now" (Release Notes opens the release's `html_url` in the browser or copies it when there's no opener)
  - Install: download the asset for `std::env::consts::OS` / the machine (`x86_64`, `aarch64`; macOS's `uname -m` is `arm64`, so match both names), verify it against a `SHA256SUMS` file the release job also uploads (CI change in the same commit; a release without it is refused), extract `jed` (`flate2` + `tar`), write it beside `std::env::current_exe()` (symlinks resolved) as a temp file with the old file's permissions, then `rename` over it (atomic; the running process keeps its old inode). An unwritable location (e.g. root-owned `/usr/local/bin`) refuses with the path and the `jed --update` command to run with the right rights; never `sudo` itself. Then "Updated to v1.3.0 — restart jed to use it"; jed doesn't restart itself
  - A `check_for_updates` setting (default false; the setting rules in CLAUDE.md): when on, one check at most a day at start (the last check's time in the state dir), a notice only when a newer release exists
  - Tests: version comparison and asset choice (unit); the whole flow against a local `std::net::TcpListener` serving a fake API answer, a tarball and its `SHA256SUMS` (the API base URL an `App` field, set by tests), installing over a fake "current exe" in a tempdir; a bad checksum, a missing asset, an unwritable directory and a newer-or-equal current version each leave the file untouched
  - Needs: `build.rs`'s `JED_VERSION`, CI's `release` job, prompts, the settings rules, the state dir
  - Done when: the tests above pass, and a hand check updates a real older release build on Linux and macOS
  - Subtasks (one agent and one commit each):
    - update-a: the check: the HTTP client, the API call off-thread against the local test server, version comparison and asset choice, the palette command reporting "up to date" / "available" / failures
    - update-b: install: download, `SHA256SUMS` check (and the CI release job uploading it), extract, write and `rename` over the current exe, the unwritable-location refusal; `jed --update` in `main`
    - update-c: the offer prompt (Update / Release Notes / Not Now), the `check_for_updates` setting and the once-a-day check at start; gaps and hand checks; marks the task ✅
- ✅ **Text field selection and keys** — [details](docs/plan-done.md)
- **Text field mouse** (after the item above; the find widget and the Search view's inputs, the palette's input too since it's the same code): a click in an input places the cursor at that column (the horizontal scroll taken into account: `draw_field` reports the scroll with the input's rect into `FindHit` / `SearchHit` / `PaletteHit` — the find widget's hit gains the query / replace input rects it lacks today), a drag selects (a `Drag::Field(FieldTarget)` in `app/mouse.rs`, extending past either end scrolls), Shift+click extends, a double click selects a word (same classifier), a triple click all. Focus moves to the input as clicks do now. Tests: click / drag / double click in the find widget and the Search view's query on a `TestBackend`, including a scrolled long query
- **VHDL: tree-sitter and VHDL-LS** (`vhdl`; [Syntax highlighting](#syntax-highlighting), [LSP](#lsp)): VHDL files get syntax colours, comment / pair / indent behaviour and a language server (`vhdl_ls` from [rust_hdl](https://github.com/VHDL-LS/rust_hdl)), as C++ / Python / Rust do
  - Language: `Language::Vhdl` (id `vhdl`, name "VHDL", VS Code's VHDL extensions' id); detected by `.vhd` / `.vhdl` (case-insensitive, as the other extensions; `.vho` / `.vhf` too if VS Code's VHDL extension lists them — check); `.workspace` associations work as for any language
  - Grammar: the `tree-sitter-vhdl` crate (1.4.0, jpt13653903/tree-sitter-vhdl, MIT, on `tree-sitter-language` like the others; its `HIGHLIGHTS_QUERY` is the Neovim query) — check its captures map to `highlight::KEYS` / `ALIASES` (the every-capture test), add `queries/vhdl.scm` for Dark+'s colours (control keywords `if` / `case` / `loop` / `process` …, built-in types `std_logic` / `integer` …, `--` comments). VHDL is case-insensitive: keyword / type predicates must match any case (`(?i)`). Its `parser.c` is 8.4 MB: note the clean-build time it adds; `scope_of` must know its string / comment / character-literal node kinds (`scopes_in_every_language`)
  - Language config (`LanguageConfig`, VS Code's VHDL extension's `language-configuration.json`): line comment `--`; brackets `()` (VHDL has no `{}` / `[]` blocks in practice — check the extension); quotes `"` (and `'` not auto-closed: it's attributes and character literals, `clk'event`); block comments `/* */` (VHDL-2008) left to the grammar's scopes
  - Server: `vhdl_ls` as a built-in (`lsp::config` `Builtins`, like `clangd` / `ty` / `rust-analyzer`; `.workspace` `[lsp.vhdl]` still overrides), root = the workspace root. It reads its libraries from `vhdl_ls.toml` at the root (then `$VHDL_LS_CONFIG`, then `~/.vhdl_ls.toml`): check what it does without one (only open files? a warning?) and say so in the server docs; its standard libraries (`ieee`, `std`) ship with the release binary (`vhdl_libraries/` beside it) — check a `cargo install vhdl_ls` binary finds them. Diagnostics (pushed), hover, definition, references, completion, rename; check whether it formats (if not, Format Document says the server can't)
  - Install: not on this machine; `cargo install vhdl_ls` (or the GitHub release zip) — ask the user before installing. A missing binary gives the usual "couldn't start vhdl_ls" message with its stderr line
  - Tests: unit tests for detection, config and highlight spans for a VHDL snippet (entity / architecture / process, a `--` comment, a string, a `'event` attribute); a fake-server test for the built-in being picked for a `.vhd` file; a real-server test (like rust-analyzer's) on a two-file temp project with a `vhdl_ls.toml` — a diagnostic for an undeclared signal, hover and definition across files — skipped with a note when `vhdl_ls` isn't runnable
  - Needs: `syntax::Language` and its queries, `lsp::config` `Builtins`, the real-server test helpers (`REAL_SERVERS`, `children_named`)
  - Done when: a `.vhd` file opens coloured with `--` comment toggling and auto-closed `(` / `"`, and with `vhdl_ls` installed shows diagnostics, hover and go to definition across files in a project with a `vhdl_ls.toml`; the tests above pass
  - Subtasks (one agent and one commit each):
    - vhdl-a: the language: `Language::Vhdl`, detection, the grammar crate, highlight queries (+ `queries/vhdl.scm`), `scope_of` kinds, `LanguageConfig`; unit tests; docs (`src/syntax/CLAUDE.md`'s language lists)
    - vhdl-b: the server: the `vhdl_ls` built-in, the fake-server test, the real-server test (skipped without the binary), a `vhdl_ls.toml` note in the server docs; hand checks; marks the task ✅
- ✅ **Single file mode**: a lean one-file editor (`jed -s <file>`) — [details](docs/plan-done.md)
- ✅ **Terminal window identity**: the window title, kitty user vars and OSC 7 — [details](docs/plan-done.md)
- ✅ **Integrated shell** (`shell`): a new kind of editor tab hosting the user's own interactive shell, with jed's socket exported in it and the user's regular init run — [details](docs/plan-done.md)

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

## Open questions
- _Add your own_
