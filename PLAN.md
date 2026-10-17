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
- **Activity bar** (left edge): icons for Explorer, Search, Source Control
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
- Crash recovery: dirty buffers backed up periodically to `~/.local/state/jed/backup/`; offer restore on next start
- Untitled buffers, Save As and Save All (the [Untitled files and Save As](#named-milestones) milestone):
  - "File: New Untitled File" (Cmd+N) opens `Untitled-1` (the lowest free number): no path, Plain Text until saved or a language is picked ("Change Language Mode", a picker of `Language`s, also for files: overrides detection for that buffer)
  - Everything keyed by a path skips an untitled buffer: disk checks, the watcher, file history, git markers / blame, language servers (no `didOpen` until it has a path)
  - Save on an untitled buffer, and "File: Save As…" (Cmd+Shift+S) on any buffer, prompt for a path (relative to the root, prefilled with the current one; an existing file asks to overwrite); the buffer then takes the path: language re-detected (unless picked), every tab's title, LSP `didClose` / `didOpen`, the disk stamp, history and git markers follow; Save As leaves the old file as it was
  - "File: Save All" (Cmd+Alt+S): every dirty buffer, untitled ones prompting one at a time
  - Close / quit prompts include untitled buffers ("Untitled-1"); an empty untitled buffer closes without asking
  - Crash backups cover untitled buffers (keyed by an id instead of the path's hash, restored as untitled)
  - Sessions: _decide in the first task_ — leaning: empty untitled tabs aren't saved; non-empty ones are restored from their backup (VS Code's hot exit, minus the prompt), which makes the backup survive a clean quit

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
  - Code actions / quick fixes: Cmd+. (Ctrl+. on Linux) lists `textDocument/codeAction` for the cursor / selection with the diagnostics there (quick fixes first, preferred marked) in a popup list at the cursor; applying one runs its `WorkspaceEdit` (`rename::parse_workspace_edit` / `App::apply_*` like rename, one undo step per file) and/or its `command` (`workspace/executeCommand`; server-sent `workspace/applyEdit` answered); `codeAction/resolve` when the server needs it; a lightbulb `💡` in the gutter on the cursor line when actions exist (requested after a short rest, cached by revision)
  - Breadcrumbs and sticky scroll, a setting each (`breadcrumbs`, `sticky_scroll`), off by default: a row above the editor with the path and the symbols enclosing the cursor (the tree-sitter tree, else `documentSymbol`), a click on a crumb opening a picker of its siblings; sticky scroll pins the enclosing blocks' first lines at the top while scrolling (VS Code's)
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
  - A commit tab (a new `Tab` kind, read-only, one per commit, reused): the commit as `git show --stat <commit>` prints it, with git's default colours (theme keys): `commit <full id>` (yellow) with `Merge: a b` for a merge, `Author: Name <email>`, `Date:` (git's default date format, the commit's own offset), the message indented four spaces, then the stat — ` path | 12 ++++++--` per file (insertions green, deletions red, the graph scaled to the tab's width as git scales to the terminal's; `Bin 0 -> 1234 bytes` for binary; renames `old => new` with git's default rename detection), then ` 3 files changed, 10 insertions(+), 2 deletions(-)`
    - Diffed against the first parent; a root commit against the empty tree. A merge: `git show` prints no stat for one by default — _decide in git_history-a_, leaning: the stat against the first parent, labelled so
    - Scrolls, selects and copies like a read-only editor (the text is plain; colours come from the line kinds); the file's own row in the stat is highlighted
    - Enter or a click on a stat row opens that file's change in a diff tab in the group to the right of the commit tab's (split right when there's none; reused for the next file clicked): the parent's text ↔ the commit's (`diff_view::Text::Commit`, a new side: a blob read from a commit's tree by path), titled `name (abc1234)`; no staging or revert buttons
  - Sessions keep commit tabs and commit diff tabs by commit id (dropped when the commit is gone), the section's collapsed state and divider position
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
- Later: replace across files, whole-word toggle

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
| Toggle search details (files to include / exclude) | Cmd+Shift+J | Ctrl+Shift+J |
| Go to line | Ctrl+G | Ctrl+G |
| Open recent workspace | Ctrl+R | Ctrl+R |
| Comment line | Cmd+/ | Ctrl+/ |
| Move line | Alt+Up/Down | Alt+Up/Down |
| Duplicate line | Shift+Alt+Up/Down | Shift+Alt+Up/Down |
| Undo / redo | Cmd+Z / Cmd+Shift+Z | Ctrl+Z / Ctrl+Shift+Z |
| Cut / copy / paste | Cmd+X / Cmd+C / Cmd+V | Ctrl+X / Ctrl+C / Ctrl+V |
| Add next match / all matches | Cmd+D, Cmd+Shift+L | Ctrl+D, Ctrl+Shift+L |
| Toggle panel | Cmd+J | Ctrl+J |
| Go back / go forward | Ctrl+- / Ctrl+Shift+- | Ctrl+Alt+- / Ctrl+Shift+- |

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
   - Deferred: moving tabs between groups, drag-and-drop tabs, keyboard resize of splits (TODO)

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
   - Done when: copy in jed → paste in another app → copy there → paste back into jed, both locally and over SSH; empty-selection copy/cut/paste matches VS Code
   - Implemented; automated tests cover the editing rules with an in-memory clipboard. Still to check by hand: the round trip with another app locally (`arboard`) and over SSH (OSC 52), incl. inside tmux
5. ✅ **Palette**: command palette, quick open, keybinding config with contexts — see [Command palette and quick open](#command-palette-and-quick-open), [Keybindings](#keybindings-vs-code-defaults)
   - Needs: named `Command`s (M1), workspace excludes and the explorer's `ignore` walk (M2)
   - Needed by: the find widget and Search view (M6, M7 key contexts), F2 meaning rename-file in the explorer and rename-symbol in the editor (M9), palette commands in M9–M11
   - Command palette over every `Command`, fuzzy via `nucleo`, showing each one's binding
   - Quick open: walk off the UI thread, fuzzy, `file:line[:col]`; skips `.jed/`
   - Go to line (Ctrl+G) in the same input widget
   - `keybindings.toml`: user bindings over the defaults, each limited to a context (editor, find widget, palette, explorer, search, …)
   - Keymaps stay data, chosen at startup (keeps [Basic terminals](#basic-terminals) open)
   - Commands owed from earlier: "Manage Workspace Trust" (M2), "Open Log File", "Set Log Level" ([Logging](#logging))
   - Done when: every command can be run from the palette; a binding in `keybindings.toml` replaces the default and shows in the palette; Esc closes the palette and does nothing else
   - Also: F1 opens the palette (VS Code default); the explorer hides `.jed/` too (shared walker with quick open)
   - Deferred: live preview for go to line; unbinding in `keybindings.toml`; live reload of `keybindings.toml` (TODO); find-widget and search contexts arrive with M6 / M7
   - Still to check by hand: Cmd+P / Cmd+Shift+P reach jed in each terminal (Ghostty and WezTerm palettes, iTerm2 Print menu); quick open speed on a large repo
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
       - Still to check by hand: Cmd+D / Cmd+Shift+L / Cmd+Alt+D / Cmd+U reach jed in each terminal (Ghostty and iTerm2 split on Cmd+D; iTerm2's Cmd+U menu item is unverified)
     - ✅ 6c: add cursor above / below, Shift+Alt+I, Alt+Click, box select (mouse and keyboard), paste spread across cursors, move / duplicate line at every cursor
       - `column.rs` (add cursor above / below, cursors at line ends, box select as VS Code's ColumnSelection) and `lines.rs` (move / copy lines as blocks, one undo step, no-op adds none); `clipboard::Copied { text, whole_line, pieces }`; `pos::Line` / `LineWalker` / `line_bounds` and an ASCII fast path for 60k-cursor speed
       - Paste spread as VS Code's `multiCursorPaste: spread`; copying from several empty cursors is no longer whole-line (VS Code marks only one cursor), changing M4's behaviour
       - Also (separate commit before 6c): typing at 63k cursors sped up for slower machines (one-string `ChangeSet`, `Arc` change sets, copy-on-write `Selections`)
       - Not done: move lines doesn't re-indent (M8); middle-button box select; a mix of empty and non-empty selections copies `""` for the empty ones (VS Code copies their lines) and cut leaves those lines
       - Still to check by hand: Shift+Alt+drag and Alt+Click reach jed (terminals keep these for their own selection); Cmd+Alt+Up/Down vs Ghostty / iTerm2 pane keys; Cmd+Shift+Alt+arrows / PageUp / PageDown arrive with Super; Shift+Alt+I needs Option-as-Alt; on Linux GNOME takes Ctrl+Alt+Up/Down and Ctrl+Shift+Alt+arrows
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
8. ✅ **Highlighting**: grammars and the built-in dark theme — see [Syntax highlighting](#syntax-highlighting), [Themes](#themes)
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
       - jed's own query additions in `syntax/queries/` (control keywords purple, builtin types / constants, `this` / `self`, …); 20 Dark+ token keys in the theme
       - While a background reparse runs, the last spans are mapped through each edit (text typed at a span's edge stays plain until the new tree), so colours never flash off
       - 1 MB C++ (release, i7-2600K): open draws plain text in 1 ms and stays under 3 ms a frame during the 620 ms parse; typing avg 5.3 ms, worst ~11 ms; paging worst 2.2 ms; a 1 MB single line ~10 ms
       - Not done: an unclosed `/*` isn't coloured as a comment (tree-sitter error recovery); C macro calls and goto labels; semantic highlighting
       - Still to check by hand: colours against VS Code Dark+; 256-colour mode with `COLORTERM` unset; a big C++ file opening plain then coloured without a stall
     - ✅ 8c: comment line (Cmd+/) and auto-close pairs (brackets and quotes, type-over, delete the pair, wrap a selection) at every cursor, from a per-language config (comment tokens, pairs)
       - `syntax/config.rs` `LanguageConfig` (VS Code's C/C++, Python and plaintext language configurations: comment token, auto-close pairs with `notIn`, surround pairs, `autoCloseBefore`, Python string prefixes); `comment.rs` (toggle per selection as VS Code's LineCommentCommand); `pairs.rs` (type-over → auto-close → wrap; `delete_pair`; `View::auto_closed` tracks jed-inserted closers so only those are typed over / deleted, dropped on any other change or a line break inside)
       - `[settings] auto_close_pairs` (default true) turns off auto-close, type-over, pair delete and wrap; comment line unaffected
       - 63k cursors (release): `(` 9 ms, type-over 10 ms, pair Backspace 11 ms; Cmd+/ on 84k lines ≤ 46 ms
       - Not done (VS Code differences): no auto-close before an unmatched closing bracket (needs bracket matching, 8e); no stand-in-character quote check; without a current tree the "not in string / comment" rule is skipped; comment indentation isn't rounded to the indent size
       - Still to check by hand: Cmd+/ reaches jed (iTerm2's Find Cursor menu item; layouts where `/` is shifted may send Cmd+Shift+7; on Linux without the kitty protocol Ctrl+/ arrives as Ctrl+7)
     - ✅ 8d: user settings file: `~/.config/jed/settings.toml` (`paths::config_dir()`, the same `[settings]` keys as `.workspace`) loaded at startup like `keybindings.toml`, errors reported via notify; `.workspace` `[settings]` override it per key; `auto_close_pairs` (added in 8c) and the other settings work from either. Live reload is in the TODO list
       - `config/settings.rs`: `SettingsOverrides` (shared with `.workspace`), `layer(user, workspace)` is the one precedence function (defaults ← `settings.toml` ← `.workspace`, per key); `TEMPLATE` documents every key (a compile-time check keeps it complete)
       - `settings.toml` is lenient (bad keys / types reported and skipped, a syntax error keeps the defaults); `.workspace` stays strict; load errors from settings and keybindings show together after start (`App::report_errors`)
       - "Preferences: Open User Settings" (palette) creates the file from the template if missing (never overwrites) and opens it
     - ✅ 8e: auto-indent (Enter keeps the indent, indents after an opening bracket or Python `:`, outdents on a closing bracket) and bracket matching (highlight the pair at the cursor) at every cursor, each with an on / off setting
       - `indent.rs` (Enter: carry the indent up to the cursor, +1 after a real-token opener, closer on its own line between a pair, C/C++ `indentationRules` (increase / decrease patterns), Python block `:` and dedent after `return` / `pass` / …; outdent a typed closer to its opener's indent; Python `else` / `elif` / `except` / `finally` dedent); `brackets.rs` (bracket after the cursor first, then before; tree tokens with a ±16 KB text scan fallback; up to 64 on-screen cursors; cached in `View::brackets`; `editor.bracketMatch`); `syntax/tokens.rs` forward-only token walk
       - Settings `auto_indent` and `match_brackets` (default true), from `settings.toml` or `.workspace`
       - 65k cursors (release): Enter ≤ 53 ms, `}` outdent 17 ms; bracket-matching frame 3.4 ms
       - Not done (VS Code differences): C++ `onEnterRules` continuing `// ` comments and outdenting after a brace-less `if` / `for` / `while`; no indentation normalisation or `trimAutoWhitespace`; outdent applies per cursor; without a tree brackets in strings count; Go to Bracket; no auto-close before an unmatched closer
     - ✅ 8f: more languages: Rust, Markdown, YAML, JSON (and JSONC), XML, TOML, Bash / shell, CMake, Makefile, Dockerfile — each a tree-sitter grammar crate compatible with the pinned tree-sitter version, detection (extensions, file names like `Makefile` / `CMakeLists.txt` / `Cargo.lock` / `Dockerfile` / `Containerfile` / `*.dockerfile`, shebangs like `sh` / `bash`), a `highlights.scm` mapped onto the existing Dark+ keys (adding keys only where needed, e.g. Markdown headings / emphasis / links, XML tags / attributes), and a `LanguageConfig` (comment tokens, pairs) so comment line and auto-close work; a language with no usable grammar crate is dropped and noted; marks M8 ✅
       - Grammars: tree-sitter-rust, -md (block grammar), -yaml, -json (JSON and JSONC), -xml, -toml-ng, -bash, -cmake, -make, -containerfile (for Dockerfile: tree-sitter-dockerfile needs tree-sitter 0.20); none dropped
       - Detection (VS Code ids and lists): exact names (`Makefile`, `CMakeLists.txt`, `Dockerfile`, `Cargo.lock`, `tsconfig.json` (JSONC), shell dotfiles, …), name patterns (`Dockerfile.*`, `tsconfig.*.json`, `.vscode/*.json`), multi-dot suffixes, extensions, shell / `make` shebangs; associations still first; `.workspace` is TOML
       - Highlighting: each grammar's `highlights.scm` + jed's `queries/<lang>.scm` (CMake and TOML: jed's own only); `ALIASES` maps grammar captures onto existing keys; new keys `markup.heading` / `list` / `quote` / `raw` / `link`, `tag`, `tag.attribute`, `punctuation.tag` (YAML keys use `tag`, as Dark+)
       - `LanguageConfig` gained explicit `brackets`, `tree_tokens` (false for Markdown: brackets matched from the text) and `insert_spaces` (Makefiles: tabs, via `edit::Settings::for_language`); `Syntax::scope_of(kind)` is one string / comment / code table across grammars; `Tokens::node_at` finds comments whose text is in no leaf (Rust); Rust uses C's `indentationRules`, YAML indents after `:` / `-`
       - 1 MB files (release) in Rust, JSON, YAML, Markdown, XML, TOML, shell: open frames ≤ 4.5 ms, a key + re-parse + frame 9–11 ms, background parses 120–690 ms
       - Not done (VS Code differences): Markdown inline formatting (emphasis, code spans, links in paragraphs: needs injections) and embedded languages in fenced code; block-comment toggling (Markdown, XML have no Cmd+/); JSON has no `//`; no general per-language settings (only Makefile `insert_spaces`); `fish` shebang; `.xsl` / `.xslt`
       - Known: with the cursor on the root `{` of a 1 MB flat JSON object, tree bracket matching walks all its siblings (~6 ms, a ~14.5 ms frame); cap the partner search like the text scan's `WINDOW` (TODO)
       - Still to check by hand: colours against VS Code Dark+ per language (YAML keys, XML tag brackets, Markdown headings and link underline); a real Makefile's Tab and status bar; brackets in a Markdown paragraph
9. ✅ **LSP**: clangd and ty — see [LSP](#lsp), [Text positions](#text-positions)
   - Needs: language detection (M8), `LspPos` in `buffer::pos` (M1), workspace trust (M2: servers named in `.workspace` only run when trusted), key contexts (M5), multi-cursor (M6), the results list (M7)
   - First: check that ty analyzes files without a `.py` extension when opened with `languageId: python`, and honours a `__builtins__.pyi` (best effort, see [LSP](#lsp))
   - One tokio task per server; `positionEncoding: utf-8` with a UTF-16 fallback; incremental `didChange`
   - Bottom panel built here: Problems list; Cmd+J toggles it (Tasks, M12, adds its own tabs)
   - Diagnostics as squiggles, in Problems and as counts in the status bar
   - Completion (inserted at every cursor), hover, go to definition (F12), find references (Shift+F12), rename symbol (F2), format document
   - SCons: ship the `__builtins__.pyi` (if ty supports it)
   - Logging: `lsp::wire` and `lsp::stderr`
   - Done when: on a C++ project with `compile_commands.json` and on an SCons project, diagnostics follow edits without full-text syncs; all seven features work in both
   - Servers for development and tests: `clangd` on `PATH`; `ty` on `PATH` or in the repo's `.venv/bin` (gitignored; `python3 -m venv .venv && .venv/bin/pip install ty`). Tests against real servers find them there and skip with a printed note when absent; protocol tests use an in-process fake server
   - Tasks (one agent and one commit each):
     - ✅ 9a: `lsp` module core: JSON-RPC framing over stdio, one tokio task per server, `initialize` (`positionEncoding` utf-8, UTF-16 fallback) / `shutdown` / exit, server config (built-in `clangd` for C/C++ and `ty server` for Python; `[lsp.*]` in `.workspace` only when trusted), started on first open of a matching file, crash / missing binary reported via notify; `lsp::wire` / `lsp::stderr` logging; the ty checks from "First"
       - `lsp/` (`rpc` framer, `server` task per server, `Manager`, `config`, `uri`, `fake` in-process test server over `tokio::io::duplex`); `app/lsp.rs` `sync_servers` + `AppEvent::Lsp`; `buffer::pos` `PosEncoding` / `LspPos` / `lsp_pos` / `from_lsp_pos`; crates `lsp-types` 0.97, `serde_json`
       - One server shared by C and C++; a missing binary, crash or `initialize` error is reported once and never restarted; untrusted / undecided workspaces run no `[lsp.*]` server (restricted: the built-in); quit sends `shutdown` / `exit` and kills after ~1.5 s
       - Servers are off in tests unless `app.lsp.set_launcher(...)`; real clangd / ty tests skip when absent
       - ty 0.0.88 findings: analyzes an extensionless `SConstruct` opened as `python`; honours a root `__builtins__.pyi`; picks UTF-8; pushes `publishDiagnostics` when the client doesn't declare pull support (clangd 21 also picks UTF-8)
       - For 9e on: a server that dies drops its pending requests without responses; callers drop theirs on `Failed` / `Exited`
       - Not done: no "restart language server" command; servers keep running when their files close; unknown ids in `[lsp.*]` aren't reported; quit can hang up to ~2.5 s on a stuck server with no message
     - ✅ 9b: document sync: `didOpen` / `didClose` / `didSave`, incremental `didChange` from each applied `ChangeSet` (ranges in the negotiated encoding via `buffer::pos`), never full text; a fake-server test that applies the changes and compares with the buffer after random edits, undo / redo, multi-cursor
       - `Buffer::apply` records change sets (+ the pre-edit rope) while `track_changes(true)` (a document open on a Running server); `lsp/documents.rs` `Manager::sync_documents` each loop turn: didOpen / didClose (also on rename and language change: close + reopen), one didChange per document per turn, changes last-first within a set, touching changes merged (a lone CR could otherwise become CRLF on the server), >256 changes coalesced into one range; positions by `pos::LspWalker` (one forward pass per set)
       - Server `textDocumentSync` honoured (none / full / incremental, openClose, save + includeText); didSave sends pending changes first
       - For 9c on: requests about a document call `lsp.sync_documents` first; `lsp.version(buffer)` is the last version sent
       - 63k cursors with a server (release): a key 7.6 ms avg / 9.1 ms worst; undo 17 ms
       - clangd keeps indexing after `exit`: killed after `EXIT_GRACE` (300 ms)
       - Not done: buffers without a file path (jed has none yet); reload-from-disk (M10) must go through `Buffer::edit` so servers see it
     - ✅ 9c: diagnostics: `publishDiagnostics` → squiggles in the editor (mapped through edits until the next publish), error / warning counts in the status bar, the diagnostic message on hover of the cursor line (status bar)
       - Parsed in the server task (`Event::Diagnostics`); `lsp::diagnostics::Store` (`app.lsp.diagnostics`): per path, per server, LSP positions, sorted by path, incremental `counts()` — for 9d: `files()`, or `buf.diagnostics.all()` (current byte ranges) for open buffers, both sharing `Arc<Diagnostic>`
       - `Buffer::diagnostics` (`diagnostics::Marks`: sorted, running max end, `overlapping`), mapped through every change set in `Buffer::apply`; a stale-version publish is read in that version's text (`Documents` keeps the last 32 sent versions) and mapped forward; no / unknown version: current text (VS Code)
       - Squiggles: coloured underlines (`editorError/Warning/Info/Hint.foreground`, SGR 58 underline colour); status bar `⊗ n  ⚠ m` for the workspace and the cursor line's message (after a notification)
       - A stopped / crashed server's diagnostics are cleared; closed files keep theirs
       - 31.6k diagnostics on a 1 MB file (release): placed in ≤ 19 ms, typing at 63k cursors ~8 ms
       - Not done: undercurl (ratatui has none), hint dots, `Unnecessary` / `Deprecated` tags, source / code in the message; marks of a renamed buffer stay until the server publishes for the new URI
       - Still to check by hand: underline colour in real terminals and its fallback; the width of `⊗` / `⚠`
     - ✅ 9d: bottom panel with the Problems list (Cmd+J toggles, grouped by file, Enter / click opens at the position, next / previous problem)
       - Panel under the editor groups (border row, tab strip with `PanelTab::ALL`, list), drag the border to resize (`PANEL_HEIGHT` 10, editor ≥ 5, panel ≥ 4); `Focus::Panel` → `Context::Problems`; Cmd+J `view.togglePanel`, Cmd+Shift+M `view.toggleProblems`, F8 / Shift+F8 `problems.next` / `previous` (status bar message "(k of n)" instead of VS Code's peek); status bar counts click toggles Problems
       - `grouped.rs` (`Group`, `RowKey`, `ListState`): grouped lists with scroll / selection kept by key, shared by the Search view and Problems — 9g's references list should use it, and `Goto::Problem { place: Place::Lsp, … }` opens LSP locations in unopened files
       - For M12: add a `PanelTab` variant, its context in `App::context()` and its drawing in `ui/panel.rs`
       - Not done (VS Code differences): problems ordered by position, not severity first; no collapsing, filter or copy; unopened files show the LSP character + 1 as the column
       - Still to check by hand: Ctrl+J on Linux needs the kitty protocol (else it's Enter); Cmd+J / Cmd+Shift+M reach jed in each terminal (iTerm2 menu items); panel border drag; icon widths
     - ✅ 9e: hover (a popup at the cursor, a command and mouse rest) and go to definition (F12; several locations → the results list)
       - Requests: `Manager::request_at` (syncs documents first) / `cancel` (`$/cancelRequest`); the app's `requests` map; a new request cancels a pending one of the same kind; `check_requests` after every input cancels requests whose place moved on (edit, cursor, pointer); stale / error answers dropped; Failed / Exited drop a server's pending requests
       - Hover: `editor.showHover` on Cmd+Alt+I (VS Code's chord Cmd+K Cmd+I: jed has no chords), mouse rest 300 ms (`next_timer`); popup (`ui/popup.rs`: `cell_of`, `place`, `frame` — 9f reuses it) above else below the word, ≤ 72×14, wrapped, simple markdown, wheel-scrollable; closes on any key, click, edit, scroll, tab change, pointer leaving the word
       - Definition: F12 and Ctrl/Cmd+Click; Location / Location[] / LocationLink (`targetSelectionRange`); one → jump (Cmd+U goes back); several → the panel's LOCATIONS tab (`PanelTab::Locations`, `Context::Locations`, previews of unopened files read off-thread) — 9g: `App::show_locations(title, locations, encoding)`
       - Not done (VS Code differences): no syntax colours in hover code blocks; no navigation history across files; no F4 through Locations; hover under the find widget still asks
       - Still to check by hand: mouse-motion hover per terminal; Cmd+Alt+I and F12 reaching jed on macOS; Ctrl+Click on Linux, Cmd+Click on macOS
     - ✅ 9f: completion: popup on trigger characters and Ctrl+Space, filtered as you type, accepted text (and `textEdit`s) inserted at every cursor
       - Asking: Ctrl+Space `editor.triggerSuggest` (Ctrl on macOS too; legacy Ctrl+@ works), server trigger characters (`triggerKind` 2), quick suggestions 10 ms after a word char, not in strings / comments (`[settings] quick_suggestions`, default true); "No suggestions." only for Ctrl+Space
       - List (`Context::Suggest`): below the word (else above), kind letter in `symbolIcon.*` colours, matched chars highlighted, the selected item's detail right-aligned, ≤ 12 rows, wheel scrolls; nucleo fuzzy filter (first char at a word start), score → `sortText` → label; incomplete lists re-ask (`triggerKind` 3); Up / Down wrap, PageUp / PageDown, Enter / Tab accept, Esc closes, click inserts, commit characters; closes on leaving the word, clicks / wheel elsewhere, focus / tab change
       - Inserting: at every cursor (same chars before / after each head as at the primary), `textEdit` (InsertReplaceEdit's insert range) else `insertText` else label, `additionalTextEdits`, one undo step; snippets stripped (first placeholder selected, else `$0`); multi-line text indented, buffer EOL; selected item resolved for its detail, an item accepted before resolving gets its additional edits as their own step if the buffer is unchanged
       - `completion.rs` (`Anchor` follows edits via `Buffer::sets_applied`), `app/completion.rs`, `ui/completion.rs`; requests `Completion` / `Resolve`; `send_request_with`, `send_server_request`, `Manager::server_of` / `request`
       - 63k cursors (release): a key with the list open 8.4 ms, inserting 10.6 ms
       - Not done (VS Code differences): no snippet session (later tab stops), no documentation side panel, `labelDetails`, deprecated strikethrough or `replace` insert mode; additional edits overlapping the insertions dropped; the list closes after 512 change sets; `allCommitCharacters` untested
       - Still to check by hand: Ctrl+Space per terminal (kitty vs NUL; macOS input-source shortcut); list colours; real clangd / ty snippets and auto-import edits; quick-suggestion timing in a large file
     - ✅ 9g: find references (Shift+F12, the results list) and rename symbol (F2 in the editor: a prompt, `WorkspaceEdit` across open and unopened files, one undo step per buffer)
       - References: `editor.goToReferences` (Shift+F12), `includeDeclaration: true`, the Locations list titled "References to 'w'" (one result still opens the list: it's usually the cursor's own place); none / error / no server → "No references found"
       - Rename: `editor.rename` (F2 in the editor; the explorer's F2 still renames files); `prepareRename` when supported (placeholder, else the range's text, else the word for `defaultBehavior`; `null` → "The element can't be renamed."), prompt "Rename 'old' to" with the name selected; empty / same name / buffer changed meanwhile sends nothing
       - `WorkspaceEdit`: `documentChanges` preferred over `changes`; the whole edit refused on file operations, a read-only open file, a version mismatch or overlapping edits; open buffers get one undo step each, all views follow (`follow_edits`); unopened files load off-thread into background tabs (VS Code), edited there and left unsaved; "Renamed 'a' to 'b': n edits in m files"
       - `rename.rs` (prepare / `WorkspaceEdit` parsing, `changes` → one `ChangeSet`: 9h's format document reuses it), `app/rename.rs`; `group.rs` `open_background`; capabilities: references, rename `prepareSupport`, `workspaceEdit.documentChanges` with no resource operations
       - Not done (VS Code differences): no peek view, rename preview or change-annotation confirmations; refactored files never auto-saved (no Save All yet); file operations refused; `workspace/applyEdit` still answered "not applied"; two renames racing on one still-loading file, or a server URI through a different symlink path, aren't handled
       - Still to check by hand: F2 / Shift+F12 per terminal; the prompt's selected prefill; clangd cross-file rename once indexed on a large project
     - ✅ 9h: format document (Shift+Alt+F), the SCons `__builtins__.pyi` (if supported), the done-when tests on a C++ project with `compile_commands.json` and an SCons project; marks M9 ✅
       - Format document: `editor.formatDocument` (Shift+Alt+F on macOS, Ctrl+Shift+I on Linux), `textDocument/formatting` with the buffer language's `tabSize` / `insertSpaces`; edits trimmed to what changes (`format::minimize`), one undo step via rename's `apply_changes` / `rename::follow`; dropped if text or cursor changed meanwhile; read-only refused; no provider → "There is no formatter for 'x' files installed."
       - SCons: `lsp/scons_builtins.pyi` embedded; when ty starts in a workspace with a root `SConstruct` and no `__builtins__.pyi` of its own, jed writes it under `state_dir()/lsp/scons/` (off-thread, only when changed) and adds that dir to ty's `environment.extra-paths` via `initializationOptions.configuration` (merged with the project's) — nothing is written into the workspace
       - Done-when tests with real clangd (C++ with `compile_commands.json`) and ty (SConstruct + helper): diagnostics follow edits, every `didChange` incremental (a `Tee` launcher records the wire), completion, hover, definition, references, rename, format (clangd)
       - `format.rs`, `app/format.rs`, `lsp/scons.rs`; `Manager::request_doc` / `App::send_doc_request` for position-less requests
   - Deferred: Python formatting (ty 0.0.88 has no formatter; VS Code uses a separate ruff extension: needs a second, formatting-only server per language); format selection / on save / on type; SCons `Import("env")` names are still flagged undefined; only a root `SConstruct` is detected; the 9a–9g "Not done" items above
   - Still to check by hand: Shift+Alt+F on macOS (Option as Alt) and Ctrl+Shift+I on Linux (kitty protocol; legacy terminals send Tab); formatting a larger real C++ file (cursor and scroll); SCons on a real project (`Environment` methods, `site_scons`); plus the 9c–9g hand checks above
10. ✅ **Git**: SCM panel, gutter markers, side-by-side diff view, stage/unstage hunks — see [Git](#git-view-and-stage-only)
    - Needs: the `notify` watcher (M2), off-thread saves with "save in flight" (M1), editor tabs that split (M3), the side panel (M2)
    - Needed by: file history (M11 reuses the diff view)
    - `git2` work on a single git thread or a fresh `Repository` per blocking task ([Async](#async-and-blocking-work))
    - Source Control panel; gutter markers; diff view as an editor tab with next/previous change
    - Stage / unstage file or hunk, always from disk after any pending save
    - Owed from M2: git colouring in the explorer; branch in the status bar
    - Files changed on disk ([Files](#files)), owed from M2: ignore our own saves, reload clean buffers silently, banner for dirty ones (reload / keep mine / show diff, using the diff view), deleted-file tabs
    - Done when: staging a single hunk from the gutter gives the same index as `git add -p`; changing a file outside jed updates the gutter, the panel and the open buffer
    - Tasks (one agent and one commit each):
      - ✅ 10a: `git` module (`git2`, a fresh `Repository` per blocking task): repo discovery, status (staged / unstaged / untracked per path), hunks between two texts via `git2::Patch::from_buffers` (the one diff engine for gutter, staging and diff view), stage / unstage a whole file or one hunk (new index blob = index text with that hunk applied / reverted); refreshed off-thread on `notify` events (debounced); branch in the status bar; git colouring in the explorer
        - `git/{mod,diff,stage}.rs`: `Repo::discover`, `Status::read` (`Head::{Branch, Unborn, Detached}`, `Entry { staged, unstaged }`, `decoration`, `label` `main*+!`), `refresh` → `Refreshed`; `diff::diff(old, new, context)` (`GIT_CONTEXT` 3 = `git add -p`'s hunks, 0 for the gutter), `Hunk { old, new }` 0-based half-open line ranges; `stage::file_diff(repo, path, Side, context)`, `stage_file` / `unstage_file`, `stage_hunk(repo, path, &shown, k)` / `unstage_hunk` take the shown `FileDiff` and refuse with `Error::Stale` if index / disk / HEAD moved on (10c: save, `file_diff` from disk, pick the hunk, stage); binary and conflicted refused; index mode kept
        - `app/git.rs`: `GitState` (`app.git`), one refresh at a time off-thread (`AppEvent::GitRefreshed`), later requests merged into one follow-up; reuses the 100 ms fs-event debounce; a git dir outside the root is watched separately (a workspace in a repo subdirectory sees other work-tree edits only at the next refresh)
        - Not done: clean filters (`core.autocrlf`, `.gitattributes`) aren't applied when staging hunks; no rename detection
      - ✅ 10b: files changed on disk: ignore our own saves, reload clean buffers silently (cursors and scroll kept), banner for dirty ones (reload / keep mine; "show diff" wired in 10d), deleted-file tabs (save recreates)
        - `disk.rs` / `app/disk.rs` / `ui/banner.rs`: each `Buffer` keeps a `Stamp` (len + SHA-256) of its file as last loaded / saved / reloaded; fs events start one off-thread `disk::check` per buffer (never during a save; `save_epoch` redoes a check that overlapped one), so own saves are ignored by content, not timing; saves read the file first and refuse (`SaveError::Conflict`) if it holds neither the stamp nor the bytes to write
        - Clean reload: one undoable `Buffer::edit` from git's line diff (context 0, trimmed to differing bytes), every view mapped (cursors and scroll kept); binary / non-UTF-8 → message + banner, a forced reload opens non-UTF-8 read-only
        - Banner: Reload / Keep Mine buttons = `disk::BannerAction` (10d adds `ShowDiff` there), commands `file.reloadFromDisk` / `file.keepMine` (unbound); deleted tabs "name (deleted)" struck through, cleared by save or reappearance; a moved folder marks the open files inside
        - Not done: files outside the workspace root aren't watched (the save conflict check still protects them); a write between a save's read and its rename is overwritten
      - ✅ 10c: gutter markers (added / modified / deleted lines: buffer vs index, recomputed off-thread after edits and index changes); stage / unstage the hunk at the cursor or clicked in the gutter — from disk, after prompting to save a dirty buffer and waiting for a save in flight
        - `quick_diff.rs` / `app/quick_diff.rs` / `ui/quick_diff.rs`: markers per buffer (buffer vs index blob, `git::diff` context 0, off-thread; at once on open / index change, 150 ms throttle after edits, mapped through edits meanwhile), drawn in the blank column before the text (`editorGutter.{added,modified,deleted}Background`); none for files not in the index
        - Clicking a marker opens a peek (removed / added lines, ` Stage Change `, Enter / Esc); `git.stageHunk` / `git.unstageHunk` (palette, unbound) at the cursor; `editor.nextChange` / `previousChange` on Alt+F5 / Shift+Alt+F5 (VS Code's keys)
        - Staging: context 0 from the disk file (`git::stage::stage_line`: = `git add -p`'s yes, or split + yes when merged with a neighbour); dirty buffer → "Save … before staging" prompt, a save in flight waited for, the line followed through edits meanwhile, a failed save stages nothing, stale → re-diff once; unstage maps the buffer line to the index's
        - Not done: staging a selection or several changes; revert change; no markers for staged changes (unstage is cursor / palette only); no size limit on markers
      - ✅ 10d: diff view as an editor tab (side by side; working tree vs index for unstaged, index vs HEAD for staged; splittable), next / previous change, stage / unstage hunk from it, "Open Changes" for the current file; the dirty-file banner's "show diff"
        - `group::Tab` is an enum: `Text(TextTab { buffer, view })` / `Diff(Box<DiffTab>)`; `active_tab()` / `active_buffer()` are `None` on a diff tab (editor commands do nothing there), `App::active_file_buffer()` for save / reload / keep mine; "the group has tabs" is `!group().tabs.is_empty()`
        - `diff_view.rs` / `app/diff_view.rs` / `ui/diff.rs`: sides `Text::{Head, Index, WorkTree, Disk, Buffer}` (the work tree is the open buffer's live text), `Pair::{UNSTAGED, STAGED, SAVED}`; loaded and aligned off-thread (context 0), side by side from 90 columns, inline below; Dark+ diff backgrounds, changed-part highlight, `╱` filler, a ` Stage ` / ` Unstage ` button per change; reloads on index / disk changes, buffer open / close, 150 ms after edits; both sides read-only
        - Keys: Up / Down / Page / Cmd+Home / End, Left / Right sideways; F7 / Shift+F7 (`Context::Diff`, parent `editor`) and Alt+F5 / Shift+Alt+F5; `git.stageHunk` / `unstageHunk` / `stageFile` / `unstageFile` (an open file goes through 10c's save-first path), `git.openChange` (unstaged, else staged, else a message), banner Show Diff = `file.compareWithSaved` (disk ↔ yours); `App::open_diff(path, Pair)` for 10e
        - Not done: syntax colours, editing / copying in the view, word-level diff, Ln/Col in the status bar on a diff tab; a diff tab doesn't follow an explorer rename
      - ✅ 10e: Source Control panel in the side panel (staged, unstaged, untracked; live through `notify`; Enter / click opens the diff view; stage / unstage file); the done-when tests; marks M10 ✅
        - `scm.rs` / `app/scm.rs` / `ui/scm.rs`: "Staged Changes" (only when non-empty) and "Changes" (unstaged, untracked `U`, conflicted `!`: VS Code's default, no merge section), counts, collapsible, branch line, no-repo / reading / no-changes states; live from 10a's refresh, the selection kept on its file (its own list: `grouped::ListState` keys by offset in a group, not by file)
        - Ctrl+Shift+G (`view.focusSourceControl`, both platforms) and the activity bar; `Context::Scm` (`scm`): arrows / Page / Home / End, Left / Right collapse, Enter opens the diff (Staged → `Pair::STAGED`, else `UNSTAGED`) and focuses the editor, Space keeps the keys, Esc; click opens keeping focus, double click focuses the editor; `+` / `−` on hovered / selected rows and headers; `git.stageFile` / `unstageFile` on the selected row (a header = its section), `git.stageAll` / `unstageAll`
        - Dirty buffers go through the save-first path (`stage_path`; Stage All asks once, `Prompt::SaveBeforeStageAll`, and waits for every save); a deleted file's open tab doesn't count as dirty (its deletion is staged); `git::stage::stage_files` / `unstage_files` write the index once
        - Done-when tests: `milestone_10_done_when_*` (gutter marker + peek Stage clicked as drawn = `git add -p`; an outside write updates the buffer, gutter and panel, then a CLI `git add` moves it to Staged)
    - Deferred: commit input, discard / revert changes, open-file buttons, merge-conflict section, rename detection; staging a selection or several changes; markers for staged changes; editing / syntax colours in the diff view; clean filters (`core.autocrlf`, `.gitattributes`) when staging hunks; files outside the workspace root aren't watched; a diff tab doesn't follow an explorer rename
    - Still to check by hand: Ctrl+Shift+G, Alt+F5 and F7 per terminal (macOS and Linux); hover `+` / `−` buttons (mouse-move reporting); the `▎ ▁ ▔ ╱ ⎇ −` glyphs and diff backgrounds in 256 colours; the panel, gutter and branch following `git add` / `checkout` / `commit` in another terminal (also with the workspace in a repo subdirectory); gutter peek staging on a real project vs `git add -p`; diff view side by side and inline, typing in the file while it's open; the changed-on-disk banner with an external editor (Reload, Keep Mine, Show Diff); Stage All with several dirty buffers; a repository with thousands of untracked files (the panel rebuilds its rows per frame)
11. ✅ **File history**: snapshot on save, history list, compare (diff view), restore — see [File history](#file-history)
    - Needs: the atomic save path (M1), the palette (M5), the diff view (M10), explorer rename (M2)
    - The diff view's sides are `diff_view::Text` variants: add one for a history entry (`Pair` of entry ↔ `Text::Buffer`)
    - `history` module: `.jed/history/` with its `.gitignore`, snapshot after the save's rename, skip duplicates, `[history]` limits
    - Ignore watcher events under `.jed/` (the explorer and quick open already hide it, M5; search skips it, M7)
    - "File History: Show" in the palette, explorer and tab bar; compare; restore as one undo step
    - Done when: a history write failure never fails a save; restore and then undo gives back the exact buffer
    - Tasks (one agent and one commit each):
      - ✅ 11a: `history` module and recording: `.jed/history/` storage (per-file folder by path hash, `entries.toml`, snapshot files, `.jed/.gitignore`), snapshot off-thread after each successful save's rename, skip duplicates, `[history]` limits (`enabled`, `max_entries`, `max_file_size`), failures logged + status message (never fail the save); watcher events under `.jed/` ignored; explorer rename moves a file's history (delete keeps it)
        - `history.rs` (blocking, one process-wide lock): folder = 32 hex of SHA-256 of the `/`-separated relative path; `entries.toml` (path + `[[entry]]` id, time ms, size, hash; oldest first, temp + rename); `relative(root, path)` (`None` outside the root / under `.jed/`), `record` → Added / Duplicate / TooLarge / Disabled, `entries(root, rel)` newest first and `read(root, rel, id)` for 11b, `rename` (file or folder; merges by time into an existing history)
        - `app/history.rs`: `SaveJob::write` returns the written bytes; after `SaveDone` a second blocking task records, `AppEvent::HistoryDone` (warning "File history of <rel> not saved: …" on error); explorer rename moves history in its own blocking task (only when the rename succeeded); `App::fs_changed` drops paths under `.jed/`
        - Not done: two jed processes writing one history at once; delete-keeps-history has no test (explorer delete uses the real Trash)
      - ✅ 11b: "File History: Show" for the active file: a palette picker of entries (newest first, "3 min ago · 12 KB"), Compare (a `diff_view::Text` variant for an entry, `Pair` entry ↔ `Text::Buffer`) and Restore (buffer text replaced as one undoable edit, not saved)
        - `fileHistory.show` / `fileHistory.restore` (palette, unbound): entries read off-thread, then the palette opens on them (`Picker::History { restore }`; no history → a message, never an empty picker); Enter = Compare (a diff tab `name (History: 3 min ago)`, entry ↔ yours) or, for Restore, restores; Restore on a history diff tab restores its entry
        - `diff_view::Text::History(history::Snapshot)` (Copy: time + 8 hash bytes, read back with `history::read_snapshot`); restore decodes like a load, refuses binary / non-UTF-8 / read-only, diffs like `disk::changes` (re-diffed if the buffer changed meanwhile) and applies one undo step via `apply_changes`; keeps the buffer's BOM / EOL setting
        - Not done: absolute times are UTC (no time-zone database); no key inside the picker to restore instead of compare
      - ✅ 11c: entry points from the explorer (on a file) and the tab bar; the done-when tests (a history write failure never fails a save; restore then undo gives the exact buffer); marks M11 ✅
        - `explorer.showFileHistory` (palette, unbound; a folder gets a message) and right-click on an explorer row; right-click on a tab activates it and runs File History: Show for its file (also a diff tab's file). jed has no context menus: right-click runs the command directly
        - Files not open: Compare reads the disk (`Text::Buffer` falls back to the file, switching to the buffer once it opens); Restore opens the file first (`OpenRequest.restore`), nothing written
        - Done-when tests: `milestone_11_done_when_a_history_write_failure_never_fails_a_save`, `milestone_11_done_when_restore_then_undo_gives_back_the_exact_buffer` (text, selections, primary, BOM, dirty state)
    - Deferred: absolute times in UTC; no key in the picker to restore instead of compare; no context menus (right-click runs the command); two jed processes writing one history at once; delete keeping history is untested (explorer delete uses the real Trash)
    - Still to check by hand: right-click on explorer rows and tabs in real terminals (kitty, iTerm2, tmux may take right-click); Compare / Restore of a file not open from the explorer; restore then undo on a real file with multiple cursors; a broken `.jed/history` (e.g. read-only) still lets saves through with the warning; `git status` never shows `.jed/`; the `·` in the picker rows
12. ✅ **Tasks**: templates, PTY runner, task tabs in the bottom panel — see [Tasks](#tasks-throwaway-terminals)
    - Needs: workspace trust (M2), the palette (M5), the bottom panel (M9)
    - Templates from `.workspace` `[tasks]` and `~/.config/jed/tasks.toml`, workspace wins on a clash
    - "Run Task" from the palette; one tab per run; PTY via `portable-pty` + `vt100` + `tui-term`
    - Exit code shown, output kept until the tab closes; kill a running task
    - Done when: a colourful build (e.g. `cargo build`) renders as in a real terminal; restricted mode runs no `.workspace` task
    - Tasks (one agent and one commit each):
      - ✅ 12a: `tasks` module, no UI: templates from `.workspace` `[tasks]` and `~/.config/jed/tasks.toml` (workspace wins on a clash; restricted / undecided trust: no `.workspace` tasks), cwd / env resolution; the PTY runner (`portable-pty` + `vt100`): spawn off the UI thread, output parsed into a screen with scrollback, resize, exit code, kill; results reported through `AppEvent`s
        - `tasks/templates.rs`: `tasks.toml` uses `.workspace`'s `[tasks.<name>]` tables, read once at startup in `main` (lenient like `settings.toml`); `merge` (by name, workspace wins, `Source`), cwd (root / relative / `~`), env (`TERM=xterm-256color`, `COLORTERM=truecolor` unless set); `App::task_templates()` takes `.workspace` tasks only when `Trusted`
        - `tasks/run.rs`: `Run::start` (openpty + spawn in `spawn_blocking`, `$SHELL -c`, falling back to `/bin/sh`), a reader thread into a `vt100::Parser` (5000 lines of scrollback) behind a mutex, a `waitid(WNOWAIT)` waiter, `Event::{Started, Output (≤ one per 20 ms), Ended}`; `Status::{Starting, Running, Exited(Exit { code, signal, killed }), Failed}`; `resize`, `kill` (SIGHUP + SIGCONT to the group, SIGKILL after 1 s, also to leftovers holding the PTY), `force_kill`; dropping a `Run` kills it
        - `app.tasks` (`Tasks`: `run`, `get`, `close`, `size` 24×80 until 12b, `shutdown`); `AppEvent::Task`; `App::shutdown` stops servers and tasks together on quit
        - Not done (12b): input to the PTY, answers to terminal queries, `size` from the panel; daemons that `setsid` and keep the PTY keep its reader alive
      - ✅ 12b: "Run Task" in the palette (a picker of templates); one bottom-panel tab per run (`PanelTab` for task runs, drawn with `tui-term`, Dark+ colours), PTY resized to the panel, exit code shown when it ends, output kept until the tab closes; kill a running task; close a run's tab (killing it if running); keys while the panel has focus
        - `task.run` (palette, unbound: name, command, source; no templates / restricted mode → a message naming `.workspace` and `tasks.toml`), `task.kill` (the shown tab's run, else the newest running), `task.close` (Cmd+W in a task tab, the tab's `×`, middle click; kills if running)
        - `PanelTab::Task(id)` after Problems / Locations, marks `●` / `✓` / `✗ 2` / `✗ killed`; drawn with `tui-term` through `Theme::terminal_cell` (`terminal.*` keys, Dark+'s ANSI palette, bold → bright, RGB → 256 without truecolor); PTYs resized to the panel at draw time (only on change); an end note under the output and in the status bar
        - `Context::Task` (`task`): Shift+PageUp / Down, Ctrl+Shift+Up / Down, Ctrl+Home / End scroll the scrollback, Esc back to the editor; every other key and pastes go to the PTY as xterm encodes them (application cursor, bracketed paste), through a writer thread (never blocks the UI); global keys stay jed's
        - Not done: answers to terminal queries; keys typed while a run is still starting are dropped; Esc never reaches a task; no spinner; the tab strip doesn't scroll; ended runs aren't resized; no mouse selection / copy in a task
      - ✅ 12c: the done-when tests (a colourful build renders its colours and progress as in a real terminal; restricted mode runs no `.workspace` task), remaining gaps and hand checks; marks M12 ✅
        - Input written while a run is still starting is queued (`State::queued`) and sent first once it spawns; dropped if the start fails
        - Done-when tests: a real `cargo build --color=always` of a temp crate (own `CARGO_HOME` / target dir, cargo's env pinned) run from the picker, its drawn panel compared row by row (text and colours) with an independent terminal model fed a piped build, plus named colour checks (bold bright green Compiling / Finished, bright yellow warning) and the progress bar seen while running; a `\r` / `ESC[K` / cursor-movement progress script; restricted (dialog and stored) and undecided trust run no `.workspace` task (its marker never appears, `run_task` refuses, the user's same-named task runs instead), then trusting runs it
    - Deferred: answers to terminal queries (interactive tools like `htop` / `less` may misbehave); Esc never reaches a task; no spinner; the task tab strip doesn't scroll; ended runs aren't resized; no mouse selection / copy in a task tab; daemons that `setsid` and keep the PTY keep its reader alive; no Run Task key (still TBD); compiler errors into Problems (later)
    - Still to check by hand: `cargo build` of a real project with dependencies in kitty, iTerm2 and tmux (colours, progress bar, wide output wrapping); truecolor vs 256 colours; typing into a task right after picking it; Shift+PageUp / Down and Ctrl+Shift+Up / Down not grabbed by the terminal; Ctrl+C and a y/n prompt in a task; kill / close of a long build; quitting with a build running leaves no processes; the restricted-mode hint and running after Manage Workspace Trust; macOS `waitid` and process-group kills; a fish `$SHELL`
13. ✅ **Sessions and recent workspaces**: restore open tabs, splits and layout per workspace; jump between workspaces — see [Workspace](#workspace) (incl. Recent workspaces), [Files](#files)
    - Needs: the split tree (M3), state paths (M2), the palette (M5), the panic hook (M1)
    - Session state in `~/.local/state/jed/`, keyed by workspace path
    - Task tabs (M12) are throwaway: not restored; history diff tabs (M11) and git diff tabs (M10) may be restored by path and pair
    - Crash recovery: back up dirty buffers periodically to `~/.local/state/jed/backup/` (the panic hook already writes there); offer restore on next start
    - Recent workspaces: `recent.toml`; "Open Recent Workspace" (Ctrl+R) in the palette
    - Kitty (focus / launch from Open Recent Workspace): deferred to the TODO list; picking a recent workspace shows the `jed <path>` message everywhere
    - Done when: quit and reopen gives back the same tabs, splits, cursors and scroll; killing the terminal loses at most one backup interval of edits (the kitty clause — focus a workspace open in another kitty window, start one not open in a new OS window — moved to the TODO list with the kitty work)
    - Tasks (one agent and one commit each):
      - ✅ 13a: sessions: per-workspace session file in the state dir (keyed by workspace path); saved on quit (off the UI thread, atomic), restored on start: groups and the split tree, tabs per group (text tabs by path; git and history diff tabs by path and pair; task tabs never), active tab and group, selections and scroll, side / bottom panel layout; missing files and bad session files skipped with a log line
        - `session.rs` (model, blocking load / save: `<state>/sessions/<sha256 of root>.json`, root and version checked), `app/session.rs`: one `spawn_blocking` reads the session and every tab's file → `AppEvent::SessionRead`, applied in one step (no half-restored layout); the save runs in `App::shutdown` with servers and tasks, skipped while the restore is still loading
        - `App::state_dir` (`None` in tests: sessions off unless a test sets it); `jed <file>` restores, then opens the file on top in the active group; `opened` drops a buffer for a path loaded meanwhile (restore / quick open race)
        - Not done: diff tabs' own cursor / scroll, expanded explorer folders, the Search query, the shown panel tab, focus; a first frame may draw before the session arrives
      - ✅ 13b: crash recovery: dirty buffers backed up periodically to `~/.local/state/jed/backup/` (one format shared with the panic hook), backups removed when a buffer is saved / discarded / jed quits cleanly; on the next start, an offer to restore them into their buffers (left dirty, nothing written)
        - `backup.rs` (format: `jed-backup 1` + a JSON header (root, path, bom, base disk stamp, time, length) + the text, `<state>/backup/<root hash>/<path hash>.bak`, atomic), `app/backup.rs`: every 30 s (`backup::INTERVAL`, deadline set by the first change, not pushed back by typing) only changed dirty buffers, one blocking job at a time; removed when a buffer goes clean, closes discarded or is renamed, and on a clean quit; the terminal going away without Quit writes them; the panic hook writes the same files
        - Offer after the session restore and the trust dialog: Restore (Enter) / Discard / Later (Esc: kept and offered next start, survives a clean quit); restore = one `EditKind::Other` step via `apply_changes`, dirty, nothing written; a file changed since the backup gets the backup's base stamp (`Buffer::set_disk_base`), so the banner shows and a save conflicts
        - Not done: no untitled buffers to back up; old-format panic backups (`backup/%path.pid.bak`) ignored; the panic hook doesn't spare a not-yet-offered backup; "Later" then saving still offers the stale backup (restoring shows the banner)
      - ✅ 13c: recent workspaces: `recent.toml` recorded on every start (most recent first, no duplicates, cap 50); "Open Recent Workspace" (Ctrl+R) as a palette picker (folder name, `~` path, current workspace left out, missing folders dropped); picking one outside kitty: status-bar message with `jed <path>`
        - `recent.rs` (`[[workspace]]` `path` tables, locked read-modify-write via `recent.toml.lock`, atomic), `app/recent.rs`: recorded at start off-thread; `workspace.openRecent` (Ctrl+R, global, literal Ctrl on macOS too: a focused task tab never gets it) lists off-thread (missing folders pruned from the file, current one left out) then opens `Picker::Recent`; a pick shows `To open ~/x, run: jed <quoted path>`
        - Seam for the deferred kitty work: `recent::Entry::window` (drawn "open"; always `None`) and `recent::how_to_open` → `Open::Message` only
      - ✅ 13d: the done-when tests (quit and reopen gives back the same tabs, splits, cursors and scroll; killing the terminal loses at most one backup interval of edits), remaining gaps and hand checks; marks M13 ✅. Kitty integration is deferred (see the TODO list)
        - Done-when tests: `milestone_13_done_when_quit_and_reopen_gives_back_the_same_screen` (a git project driven by keys: three groups with dragged dividers, multi-cursors with a non-first primary, scroll, a diff tab, Source Control and the bottom panel; the reopened frame equals the old one cell for cell, twice), `milestone_13_done_when_a_kill_loses_at_most_one_backup_interval` (Restore gives exactly the buffers at the last backup tick, nothing written, one undo back to disk)
        - Fix: a diff tab swallowed every mouse event over it (split / panel border drags, Source Control hover); now only wheel and left click
    - Deferred: kitty integration (TODO list); diff tabs' own cursor / scroll, expanded explorer folders, the Search query, the shown panel tab, focus and the Source Control selection aren't in the session; no untitled buffers to back up; old-format panic backups ignored; Ctrl+R is global, so a task tab never gets it (no reverse-i-search)
    - Still to check by hand: quit / reopen in a real terminal (groups, ratios, tabs, cursors, scroll, panels, diff tabs reload); `jed <file>:<line>` on a workspace with a session; edit, wait > 30 s, edit, `kill -9`, reopen and Restore (then Cmd+Z, save removes the backup); closing the terminal window with dirty buffers; a forced panic's backups offered next start; restore after another program changed the file (banner, save conflicts until Keep Mine); Later then a clean quit offers again, Discard removes; Ctrl+R in two workspaces (each lists the other, deleted folders vanish, spaced paths quoted); files land in `~/.local/state/jed/sessions/`, `backup/`, `recent.toml`, and a corrupt session file is ignored and replaced

### Named milestones
Milestones 1–13 above are done and keep their numbers. Milestones planned after them have names, not numbers, and **no order**: build whichever is wanted next. Each has a short id (`tools`, `soft_wrap`, …) for its task ids (`tools-a`, `tools-b`, …), commit subjects (`Tools view and logs (tools-a): …`), test file (`src/<id>_tests.rs`) and test names (`<id>_*`). A finished one stays here, marked ✅.

- **Tools view and logs** (`tools`): a side panel view of every tool jed runs, each with its log in a read-only editor tab — see [Tools and logs](#tools-and-logs)
  - Needs: language servers (M9), tasks (M12), side panel lists and their scroll bars
  - Per-tool log rings filled off the UI thread, also written to `<state dir>/logs/tools/` with size rotation: language servers' stderr, lifecycle, `window/logMessage` / `showMessage`, optional protocol trace; jed's own `tracing` events
  - Read-only log tabs (a buffer appended to without undo or dirtiness, following the tail), one per tool
  - `SidePanel::Tools` with an activity bar icon: servers, task runs, "jed"; open a log, focus a task's tab
  - Commands: Show, Open Log, Clear Log, Toggle Protocol Trace, Restart Server
  - Done when: with rust-analyzer and clangd running, the Tools view lists both with their states; opening rust-analyzer's log shows its stderr and lifecycle lines and keeps up while it indexes (following the tail, not while scrolled up); a server that crashes shows as failed with its last lines in its log, and Restart Server brings it back; a protocol trace shows the JSON-RPC messages
  - Tasks (one agent and one commit each):
    - tools-a: the log store: `logs` module (a ring per tool: lines with time and level, capped by lines and bytes, a generation for readers), fed by language servers (stderr, lifecycle, `window/logMessage` / `showMessage`) and a `tracing` layer for "jed"; each tool's log also written to `<state dir>/logs/tools/<tool>.log`, rotated past 4 MB (`.1`–`.3`, under a lock: several jed windows share it); no UI yet beyond tests
    - tools-b: read-only log tabs: a buffer kind appended through one `ChangeSet` per frame without undo or dirtiness, never typed into; "Tools: Open Log" (picker) and "Tools: Clear Log"; following the tail; reused per tool; splits; never in sessions (a log tab isn't restored)
    - tools-c: the Tools view: activity bar icon, `SidePanel::Tools` rows (servers with state / progress / error badge, task runs, "jed"), `Scrollable`, click / Enter opens the log or focuses the task tab, "Tools: Show"
    - tools-d: "Tools: Toggle Protocol Trace" (the `lsp::wire` messages into that server's log) and "Tools: Restart Server" (stops it and lets `sync` start it again, also after a failure); the done-when tests, gaps and hand checks; marks the milestone ✅
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
- **Language features** (`lang`): go to symbol, signature help, code actions and breadcrumbs / sticky scroll — see [LSP](#lsp)
  - Needs: language servers (M9), completion's popup, kind icons and request handling (`App::send_request`), quick open (M5), back / forward navigation (TODO list; jumps recorded when it's there)
  - Go to symbol in file (Cmd+Shift+O, `@`) and workspace (Cmd+T, `#`); signature help (trigger characters, Cmd+Shift+Space); code actions / quick fixes (Cmd+., the lightbulb); breadcrumbs and sticky scroll (settings, off by default); Cmd bindings update `docs/terminals/`
  - Done when: with rust-analyzer on a small cargo project: Cmd+Shift+O lists the file's items with their containers and Enter jumps (Esc after moving the selection goes back); Cmd+T finds a struct in another file; typing `foo(` shows its signature with the active parameter moving on `,` and closing on `)`; Cmd+. on an unused import offers the fix and applying it is one undo step; with breadcrumbs on, the row shows path › impl › fn following the cursor; clangd does the same on a C file
  - Tasks (one agent and one commit each):
    - lang-a: go to symbol in file: `documentSymbol` (both shapes: `DocumentSymbol[]` tree, `SymbolInformation[]`) → a symbols model, the `@` quick open mode and Cmd+Shift+O, preview while moving, Esc restores
    - lang-b: go to symbol in workspace: `workspace/symbol`, `#` mode and Cmd+T, debounced re-requests with older ones cancelled, rows with the file
    - lang-c: signature help: the popup, trigger / retrigger characters, the active parameter, overloads, closing rules, next to the completion list
    - lang-d: code actions: Cmd+., the list popup, applying edits and commands (`workspace/executeCommand`, answering `workspace/applyEdit`), `codeAction/resolve`, the lightbulb after a rest
    - lang-e: breadcrumbs and sticky scroll (settings, off by default) from the tree-sitter tree / `documentSymbol`; the done-when tests (real rust-analyzer and clangd, skipped with a note when absent), gaps and hand checks; marks the milestone ✅
- **Themes** (`themes`): a built-in Light+, a `theme` setting, a picker with live preview and theme files — see [Themes](#themes)
  - Needs: `Theme` and its keys (M8), settings layering (M8), the palette's pickers (M5)
  - Light+ with the same keys as Dark+; `theme` setting; "Preferences: Color Theme" with live preview, saved to `settings.toml`; theme files from `~/.config/jed/themes/`
  - Done when: picking Light+ recolours everything — editor, syntax, side bar, panel, popups, diff view, task terminals — with no Dark+ colour left (a test draws every view under both and finds no colour of the other theme); Esc in the picker restores the old theme; the choice survives a restart; a theme file dropped into the themes folder is listed and applies, and a broken one is reported and falls back
  - Tasks (one agent and one commit each):
    - themes-a: Light+ (`Theme::light_plus`, the same-keys test, 256-colour fallback) and the `theme` setting (`dark` / `light`), live on a `.workspace` reload
    - themes-b: "Preferences: Color Theme": the picker with live preview, Esc restores, Enter writes `theme` to the user's `settings.toml` keeping comments
    - themes-c: theme files: decide the format first (see [Themes](#themes)), load from `~/.config/jed/themes/`, list in the picker, fallbacks and error reports; the done-when tests, gaps and hand checks; marks the milestone ✅
- **Git history** (`git_history`): a file's commits in the Explorer view, each commit shown as `git show --stat` in its own tab, and a file's change in a commit as a diff — see [Git](#git-view-and-stage-only)
  - Needs: `git2` and the diff view (M10), side panel lists and their scroll bars, sessions (M13)
  - The Explorer view split vertically: the tree above, "Git History" for the active file below (header, collapsible, draggable divider); commits read off-thread in pages, refreshed when HEAD moves
  - Commit tabs: a new read-only tab kind drawn like `git show --stat`, with git's colours; clicking a file opens its diff (parent ↔ commit) in the group to the right
  - Done when: in a repository with a few commits touching a file, opening that file lists its commits newest first in the Explorer view's lower section (and switching tabs switches the list); clicking a commit opens a tab whose text matches `git show --stat --format=medium <commit>` at the tab's width (a test compares them line by line, colours by line kind); clicking a file row there opens a side-by-side diff of that file's change to the right, matching `git diff <commit>^ <commit> -- <file>`; a new commit made outside jed appears at the top; quit and reopen restores the tabs
  - Tasks (one agent and one commit each):
    - git_history-a: the model, no UI: `git::log` (a path's commits from HEAD, git's default simplification, paged, off-thread job), `git::show` (a commit's header, message and per-file stat: insertions / deletions, binary sizes, renames with git's default detection, first parent / root) and `git_show::render(show, width)` → lines with kinds (git's `--stat` graph scaling); `diff_view::Text::Commit` (a blob from a commit's tree); unit tests against the `git` CLI's own output on test repositories
    - git_history-b: the Explorer view split: the "Git History" section (header, collapse, draggable divider, `Scrollable` list, the active file followed, pages read as it scrolls, refreshed when HEAD moves, "No history"), theme keys; no commit tab yet (Enter does nothing)
    - git_history-c: commit tabs (`Tab::Commit`: drawing with git's colours, scroll / select / copy, reuse per commit, splits) opened from the section, and stat rows opening the file's diff in the group to the right (split when none, reused); sessions keep both and the section's layout
    - git_history-d: the done-when tests (against the `git` CLI), gaps and hand checks; marks the milestone ✅

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

- **CI: Gitea Actions pipeline on Linux** (no macOS runner available)
  - `.gitea/workflows/ci.yml`, on push and pull request
  - Steps: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release`
  - Toolchain from `rust-toolchain.toml`; cache `~/.cargo` and `target/`
  - Linux exercises the Ctrl keymap and inotify watcher; macOS paths (Cmd keymap, FSEvents, Trash) stay untested in CI
    - Keep running `cargo test` locally on macOS before pushing
- ✅ **Terminal config snippets** (do soon: terminals grab Cmd keys, Cmd+W can close the terminal tab with unsaved work)
  - `docs/terminals/<terminal>.md` for Kitty, Ghostty, WezTerm, iTerm2, Alacritty
  - Each: pass through every Cmd binding in the default keymap; turn on Option-as-Alt
  - Update the snippets whenever a Cmd binding is added (keymap change → snippet change)
  - Per-terminal check: press every binding and confirm it arrives (`JED_LOG=input=trace`)
  - Done: `docs/terminals/` (README + kitty, ghostty, wezterm, iterm2, alacritty); unit test `terminal_docs_list_every_cmd_binding` fails if the README table and the macOS Cmd keymap differ
  - Still to do: hand-check each terminal with its checklist (iTerm2 least certain)

- **Live reload of settings**: `settings.toml`, `.workspace` overrides and `keybindings.toml` reloaded when they change (the files themselves load at startup since M8, 8d)
- **Soft wrap**: now the [Soft wrap](#named-milestones) milestone
- **Move tabs** (owed from M3; the first of three items replacing "Tab and split management"): "View: Move Editor into Next / Previous Group" (VS Code: Ctrl+Cmd+Right / Left on macOS, Ctrl+Alt+Right / Left on Linux; into a new split to the right when there's no next group, as VS Code) and "View: Move Editor Left / Right" within its group (Cmd+Shift+PageUp / PageDown on macOS, Ctrl+Shift+PageUp / PageDown on Linux); the tab keeps its view (selections, scroll), a group left empty closes; diff tabs move too; Cmd bindings update `docs/terminals/`
- **Drag-and-drop tabs** (after Move tabs): drag a tab along its bar to reorder, onto another group's tab bar to move it there, or onto an editor's edge / centre to split / move into it (the drop zone shaded, a theme key); a drag that never leaves its tab is a click; Esc or a release outside cancels
- **Keyboard resize of splits** (owed from M3): "View: Increase / Decrease Editor Width" and "… Height" (VS Code's, unbound; in the palette) move the active group's nearest divider in that direction by a step (a few cells, clamped like a mouse drag); the ratios are the session's as after a drag
- **Side panel resizing, finished**: the side panel already resizes by dragging its border (its last column: `Hit::SideBorder` → `Drag::SideBorder`, width in `ui.side_width`, kept in sessions), but it's undiscoverable and rough. Make it VS Code's sash:
  - The border column is drawn as a visible divider (a theme key, e.g. `sideBar.border`), lit on hover and while dragging (`sash.hoverBorder`); the hit area doesn't take the list's last column (scroll bars sit there since the side-bar scroll bars item) — the divider gets its own column, or the hit moves to the editor's first column, decide
  - A drag stores the clamped width (`layout::compute`'s max: the editor keeps `MIN_EDITOR_WIDTH`), so dragging back after overshooting the max moves the border at once (today the stored width can exceed the max and the border stays put until the pointer comes back); dragging below the minimum by a few more cells hides the panel (VS Code snaps it closed), dragging out from the activity bar edge reopens it
  - Double-click on the border resets the default width; the width survives toggling the panel off and on and a terminal resize (clamped when drawn, the stored width kept)
  - "View: Increase / Decrease Side Bar Width" (unbound, palette; a step of a few cells), sharing the step with Keyboard resize of splits
  - Tests: drag through `app.mouse` (hover colour, overshoot and back, snap closed, reopen, double-click reset), the commands, the session keeping the width
- ✅ **Back / forward navigation** ([Editor](#editor)): `nav.back` / `nav.forward` on Ctrl+- / Ctrl+Shift+- (macOS) and Ctrl+Alt+- / Ctrl+Shift+- (Linux), as VS Code's Go Back / Go Forward
  - One history for the window (VS Code's default `navigationScope`): entries are (group, file, primary selection, scroll), up to 50; recorded before jumps — go to definition / references / the Locations list, quick open (files and `:line`), go to line, opening a Search view or Problems result, F8 / Shift+F8, Cmd+Home / End, opening or switching to another file's tab — and when the primary cursor moves 10+ lines from the last entry in one step; going back first records where you are so forward returns; a new jump after going back drops the forward entries
  - Back to a closed tab reopens its file (off the UI thread, through `opening`); a file gone from disk is skipped; positions map through edits (kept as byte positions mapped like other views', or re-clamped when used); a diff tab counts as its file
  - `navigation.rs` (model: `History` of `Entry { group, path, selection, scroll }`, ≤ 50, consecutive duplicates collapsed, forward entries dropped by a new jump, mapped through edits) and `app/navigation.rs`: recorded in one place — `nav_begin` / `nav_end` around every input event, background result and `test_execute` — when the active group or file changed, a jump command moved the primary without an edit (`open_file`: quick open, Search / Problems / Locations results, F8, definition; go to line; Cmd+Home / End), or (input only) the primary moved 10+ lines in one step without an edit; session restore records nothing. Going to an entry prefers a text tab of its file in its group (else the active group), else a new view of the loaded buffer, else reopens it through `opening` (`Goto::Place`); a file gone from disk or failing to load is skipped and navigation goes on. Keys: global `nav.back` / `nav.forward`; `KeyCombo::from_event` turns `_` with modifiers into `-` + Shift (kitty's Ctrl+Shift+-)
  - The 10-line rule is per step (from where the cursor was before the event), not from the last entry
  - Deferred: an explorer rename doesn't update entries' paths (the renamed file's entries are skipped as gone); entries recorded in the same event as a background edit (rename, format) are only clamped; legacy (non-kitty) terminals send Ctrl+- as Ctrl+7, so Go Back doesn't fire there; mouse buttons (next item)
  - Still to check by hand: Ctrl+- / Ctrl+Shift+- (macOS) and Ctrl+Alt+- / Ctrl+Shift+- (Linux) arriving in kitty, Ghostty, WezTerm, iTerm2, Alacritty and tmux; going back across splits and to a closed tab's file in a real session
- **Mouse back / forward buttons** (after back / forward navigation): the mouse's back / forward buttons run `nav.back` / `nav.forward`. crossterm 0.29 drops SGR mouse buttons 8 / 9 (`parse_cb`: "We do not support other buttons"): needs a patched crossterm (a vendored fork via `[patch.crates-io]`) or jed's own parse of those reports before crossterm sees them — decide first; not every terminal sends them (check kitty, Ghostty, WezTerm, iTerm2, Alacritty)
- **Performance pass** against the [Goals](#goals): startup time, memory, 1 MB files; check the slow-frame warnings in the log; tree bracket matching on a 1 MB flat JSON root (see M8)
  - Done when: no warnings over 16 ms in a day of normal use
- ✅ **Render whitespace**: VS Code's `editor.renderWhitespace` as a setting (`render_whitespace`: `none` / `boundary` / `selection` / `trailing` / `all`, default `all`): spaces as `·`, tabs as `→`, drawn in a dim theme colour (`editorWhitespace.foreground`) without changing columns; plus a "View: Toggle Render Whitespace" command
  - `edit::RenderWhitespace` (`Settings::render_whitespace`, both settings files); drawn in `ui/editor.rs` (`shows_whitespace`, `Overlay::whitespace`): only U+0020 and tab (NBSP draws as itself), a tab's `→` in its first cell, `editorWhitespace.foreground` over token colours, under selection / find backgrounds, not at secondary cursors or the focused primary; `view.toggleRenderWhitespace` flips `UiState::whitespace_toggled` (session-only, survives `.workspace` reloads), read via `App::render_whitespace()`
  - Tests: `todo_tests::render_whitespace`; older end-to-end tests that read drawn text call `App::test_plain_whitespace()`; the M1 / M6 1 MB frame-budget tests assert they draw with `all`
  - Still to check by hand: the dim colour on the current line (256 colours: 237 on 235) and a block cursor over `·` in a real terminal; the release budget run on an idle machine (it missed by < 1 ms under a load of ~9 from a kernel build, the same with whitespace off)
- ✅ **Folder filter in the Search view** ([Workspace search](#workspace-search)): VS Code's "files to include" / "files to exclude" fields under the query (toggled by a `…` button, as VS Code's "Toggle Search Details"); comma-separated globs (`src/**`, `*.rs`, `!tests`), a bare folder name or path meaning everything under it (`src` = `src/**`), relative to the root; applied on top of `.gitignore` and `.workspace` `[files] exclude` (same `ignore` walker overrides), dirty buffers filtered the same way; re-searches with the debounce like the query; "Find in Folder…" on an explorer folder (right-click / a command) opens the Search view with that folder filled in; the fields are Search view state (kept with the query)
  - `path_filter.rs` (VS Code's globs: comma-split, `{…}` / `\,` aware; no `/` = any depth (`src` = every folder named src, VS Code — a deviation from "relative to the root"), a `/` or `./` = rooted; each pattern takes everything under it; `!` within a field; exclude beats include; an only-`!` include = everything else; `folder_pattern` escapes a folder for Find in Folder), one matcher for the walk (`explorer::walker_with` prunes only excluded folders), files and dirty buffers (also deleted files); `search_view.rs` `include` / `exclude` fields, `details`, `input`, `cycle`; `…` on the query row and `search.toggleDetails` (Cmd+Shift+J, `docs/terminals` updated); Tab / Shift+Tab cycle query → include → exclude → list; the 150 ms debounce for every field; hidden fields still apply (`…` lit); `explorer.findInFolder` (palette; right-click on a folder row — a file row still shows its history) fills include with `./path`, shows details and searches; not in sessions
  - Deferred: nothing of the item
  - Still to check by hand: Cmd+Shift+J reaching jed in each terminal with the updated snippets (iTerm2's default Jump to Mark is on it); the `…` button and the inputs in a narrow side bar; right-clicking a folder
- **Feature-based end-to-end test files** (four steps, in order, each done as its own `/todo` by its bold name below): the `src/milestone*_tests.rs` files (and `src/todo_tests.rs`) are named after when the work was done, not what they test (`milestone2_tests.rs` = workspace, trust, explorer, tabs, file ops); move them to `src/tests/<feature>.rs` (e.g. `editing`, `multicursor`, `find`, `workspace_search`, `explorer`, `splits`, `palette`, `highlighting`, `lsp`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …), splitting a milestone's file where it spans features and merging tests of one feature from several milestones. Rules for every step:
  - Pure move and rename: no test logic changes; the test count stays the same (compare `cargo test -- --list` before / after)
  - Tests renamed from `milestone_N_*` / `<id>_*` to the behaviour; `done_when` tests keep a recognisable name (`done_when_*`), their file's `//!` doc saying which milestone's "Done when" they cover; the release budget tests become `budget_*` (CLAUDE.md's `cargo test --release milestone` commands change to `budget` in the step that renames the last of them)
  - Each step updates the CLAUDE.md references it moves (the Tests section's helper paths, the module map entry, nested CLAUDE.md files) and PLAN.md's references to test names
  - **Test support module**: `src/tests/mod.rs` and `src/tests/support.rs` with the shared helpers (`open_app`, `cmd`, `settle_git`, `draw_sized`, …, now duplicated or reached across files as `milestone10_tests::…`); callers updated, duplicates removed, no test moved yet
  - **Feature test files: editing** — M1, M4 and M6 (`milestone_tests.rs`, `milestone4_tests.rs`, `milestone6_tests.rs`) into `editing`, `clipboard`, `multicursor`, `find`, plus `todo_tests`' modules of those features
  - **Feature test files: workbench** — M2, M3, M5, M7 and M8 into `workspace`, `explorer`, `tabs`, `splits`, `palette`, `workspace_search`, `highlighting`, `settings`, plus `todo_tests`' modules of those features
  - **Feature test files: tools** — M9–M13 and the rest of `todo_tests` into `lsp`, `problems`, `completion`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …; then CLAUDE.md, `.claude/commands/milestones.md` and `todo.md` say new tests go in the feature's file (a named milestone's too, instead of `src/<id>_tests.rs`)
- ✅ **Rust language server** ([LSP](#lsp)): `rust-analyzer` as a built-in server for Rust (`Builtins`, like `clangd` / `ty`; `.workspace` `[lsp.rust]` still overrides), so the v1 features (diagnostics, completion, hover, definition, references, rename, format via rustfmt) work in Rust files without configuration
  - Root: the workspace root (rust-analyzer finds `Cargo.toml` / the cargo workspace under it itself); no `initializationOptions` by default (`check.command` stays rust-analyzer's `check`)
  - It's slow to start on big projects (indexing, `cargo check`): `$/progress` / `window/workDoneProgress` notifications are answered, and ideally shown (e.g. "rust-analyzer: indexing 40%" in the status bar); requests before it's ready may return nothing or `ContentModified` (-32801), which must be dropped quietly, not reported as errors
  - Diagnostics come from `cargo check` on save (pull vs push: rust-analyzer pushes `publishDiagnostics`; check it doesn't need `textDocument/diagnostic` pull support)
  - Tests: a fake-server test for the built-in being picked; a real-server test (like ty's / clangd's) on a tiny temp crate, skipped with a note when `rust-analyzer` isn't runnable. Here it's only the rustup proxy: `rustup component add rust-analyzer` for the pinned 1.99 toolchain would install it (ask first: it changes the toolchain setup); on macOS the same, or Homebrew's
  - Built-in `rust-analyzer` (`lsp::config` `Builtins`, no args or `initializationOptions`, the workspace root; `.workspace` `[lsp.rust]` wins only when trusted); `lsp/progress.rs` (per-server work-done progress by token — JSON values, `1` ≠ `"1"` — fed by `$/progress`, cleared when the server fails or crashes); the status bar shows the first running server's latest unfinished work after the problem counts ("rust-analyzer: Indexing 40% 3/25 (core)", any server); answers with -32800 / -32801 / -32802 (`RpcError::is_routine`) dropped without a message (a pending completion cleared); a server dying during startup quotes its last stderr line ("Unknown binary 'rust-analyzer'"); diagnostics are pushed (no pull support needed); the fake's `test/progress` and `fail(method, code, message)`; `REAL_SERVERS` / `children_named` shared from milestone9_tests
  - `rustup component add rust-analyzer` installed for 1.99 on the Linux box; `real_rust_analyzer_on_a_cargo_crate` runs hover, definition, references, completion, `cargo check` errors on save, rename and format on a temp crate (its own `cargo.targetDir` via the test's config), retrying requests while it loads
  - Deferred: nothing of the item
  - Still to check by hand: a large cargo project (indexing progress in the status bar, `cargo check` diagnostics after a save); the error message without the component installed; macOS (rustup or Homebrew's rust-analyzer)
- ✅ **Inline git blame** ([Git](#git-view-and-stage-only)): "Git: Toggle Inline Blame" (`git.toggleBlame`, unbound; global, on by default or off — decide) shows the cursor line's blame after its text, VS Code's `git.blame.editorDecoration` / GitLens style: `<committer> <when> <commit summary>`, e.g. `Ann Smith, 3 days ago • Fix crash opening a file`, dimmed (a theme key), a few cells after the line's end
  - Only the primary cursor's line; never moves text or columns, cut with "…" at the editor's edge, not drawn when the line fills the width; hidden while a selection spans lines; "You, 2 min ago • Uncommitted changes" (or nothing) for a line changed since the last commit, incl. unsaved edits
  - `when` is relative (`history::ago`-style: "3 days ago"); summary = the commit message's first line, cut to fit
  - Blame from `git2` (`blame_file`, then `blame_buffer` with the buffer's text so unsaved edits count) off the UI thread (`spawn_blocking` → an `AppEvent`), cached per buffer by (HEAD, index stamp, revision) — one blame per file, so moving the cursor costs nothing; mapped through edits while a recompute runs like quick diff's marks, recomputed after a short delay; untracked files and files outside a repo: nothing
  - `blame.rs` (model: hunks of line-start bytes + commit or uncommitted, on every `Buffer` as `buf.blame`, mapped in `Buffer::apply`; blocking `compute`: `git2` `blame_file` at HEAD, cached as a `Base` per (HEAD oid, path), then a context-0 diff of the buffer's encoded bytes against the HEAD text marks added / changed lines uncommitted — instead of `blame_buffer`, so an edit doesn't redo the history walk), `app/blame.rs` (quick diff's scheduling: only while on and for shown buffers; at once when never blamed or after each git status read, after edits `DELAY` 300 ms via `Timer::Blame` → `AppEvent::Blame`; off resets); drawn by `ui::editor` (`Overlay::blame`) 3 cells after the primary line's end in `git.blame.editorDecorationForeground`
  - Off by default, session state (VS Code's default); `Author, 3 days ago • summary` (author, as VS Code; "You" when the author's email is `user.email`); uncommitted lines (staged too: compared with HEAD) say "Uncommitted changes" without a name / time; cut with "…" before the scroll bar, nothing with < 6 cells or when the primary spans lines
  - Deferred: a hover / details for the blame; blame in diff tabs; following renames; `autocrlf` work trees marked all uncommitted (as quick diff); a horizontally scrolled view hides it rather than showing part of it
  - Still to check by hand: a big file with long history (blame time; "slow git blame" logged over 500 ms); typing switches to "Uncommitted changes" after ~300 ms; a commit from another terminal updates it
- **Cursor style setting**: `cursor_style` (`line` / `block` / `underline`, default `line` as VS Code) and `cursor_blinking` (bool, default true) settings (CLAUDE.md's "Settings layer per key"); the terminal cursor's shape set with DECSCUSR (crossterm `SetCursorStyle`: `BlinkingBar` / `SteadyBar`, …) when entering raw mode in `terminal.rs`, reset to the terminal's default (`CSI 0 q`, `SetCursorStyle::DefaultUserShape`) in `restore` (exit) and the panic hook, and re-sent when the setting changes (a `.workspace` reload)
  - Secondary cursors (drawn by jed as reversed cells, `editor.cursor.secondary`) follow the style: `line` / `underline` as an underlined cell (the char stays readable; a theme key with an underline colour), `block` as now
  - Task tabs: a task's own cursor shape requests (DECSCUSR in its output) aren't forwarded; jed's style stays
  - Tests: the bytes written on enter / restore (a test writer), each style's secondary cursor cells; hand check in kitty, Ghostty, WezTerm, iTerm2, Alacritty, tmux (needs `terminal-overrides` `Ss` / `Se` on older tmux)
- ✅ **Syntax highlighting in the diff view** ([Syntax highlighting](#syntax-highlighting), [Git](#git-view-and-stage-only)): diff tabs (git, history, Show Diff) draw both sides with the file's syntax colours instead of plain text, side by side and inline
  - Language from the tab's path (`syntax::detect`, `.workspace` associations); each side parsed with tree-sitter off the UI thread in `diff_view::load` (the side's whole text, not its lines on their own, so multi-line strings / comments colour right), highlight spans kept per side in the `Doc` by line; above `syntax_max_bytes` (or binary): plain text as now
  - A live right side (the open buffer) could reuse the buffer's own `Syntax` spans instead of a second parse — decide; either way a reload after an edit re-colours (the diff already reloads after `DELAY`)
  - Token colours are foreground only: the removed / inserted line backgrounds and the changed-parts text backgrounds stay on top (VS Code); the `╱` filler and line numbers unchanged
  - Drawing stays O(visible rows): spans looked up for the visible rows only, walked with one index like the editor's
  - Decided: no reuse of the live buffer's spans; both sides parsed in `diff_view::load` (`syntax::highlight_text(language, text)`, blocking), the text exactly as drawn (BOM dropped, lossy UTF-8, line breaks kept); spans flat per side in `Lines` (`spans`, `start`, `first_span`), `Doc::highlight`; `Job` carries `language` and `syntax_max_bytes` (from `sync_diffs`): the open buffer's language, else the path (`syntax::detect_path`, `.workspace` associations), else either side's shebang (`detect_shebang`); over the limit, binary or no grammar: plain
  - `ui/diff.rs` walks each side's spans with one index from a binary search at the first visible line; token styles are foreground / modifiers only, under the changed-part backgrounds; line numbers and `╱` filler unchanged
  - Also: `DiffTab::set_layout` keeps a cursor on a change's first row on that change's first row in the other layout (the first load's landing no longer depends on whether the tab was drawn inline first)
  - Deferred: the inline layout still has no changed-part highlight; a `.workspace` reload changing associations / `syntax_max_bytes` re-colours only at the next reload; a superseded load's parse isn't cancelled; a side near the 8 MB default limit can take seconds to colour (off-thread)
  - Still to check by hand: Open Changes on a Rust / Python file with a block comment or docstring changed in the middle, at both widths; File History Compare and the banner's Show Diff; 256-colour terminals
- **Indent / outdent lines**: Tab with a selection spanning lines (or any non-empty multi-line selection) indents every line it touches by one level instead of replacing it (VS Code); Shift+Tab outdents the touched lines (also with an empty cursor); Cmd+] / Cmd+[ (Linux Ctrl+] / Ctrl+[) indent / outdent regardless of selection; one undo step (`EditKind::Other`), selections keep their text (grow / shrink with the indentation), at every selection via `LineWalker` (61k-selection budget like the line commands); respects the language's `insert_spaces` / tab width; blank lines aren't indented (VS Code); Cmd bindings update `docs/terminals/`
- **New untitled file, Save As, Save All**: now the [Untitled files and Save As](#named-milestones) milestone
- **Tab management commands**: reopen closed tab (Cmd+Shift+T; a stack of closed tabs per window with their view state, files reopened from disk), "Close All", "Close Others", "Close Saved" (in a group; prompts for unsaved like closing one), go to the Nth tab in the group (Ctrl+1–9 on macOS, Alt+1–9 on Linux, as VS Code; 9 = last); in the palette, and on the tab bar's right-click once there are context menus
- **More line commands**: delete line (Cmd+Shift+K), select line (Cmd+L, repeated: the next line too), join lines (Ctrl+J on macOS; unbound on Linux, where Ctrl+J is the panel), insert line below / above (Cmd+Enter / Cmd+Shift+Enter, with auto-indent), transform to upper / lower / title case, sort lines ascending / descending, delete duplicate lines; at every selection (`LineWalker`, one undo step each, the line-command budget); Cmd bindings update `docs/terminals/`
- **Expand / shrink selection** (Shift+Alt+Right / Left on Linux, Ctrl+Shift+Cmd+Right / Left on macOS, as VS Code): grow each selection to the enclosing tree-sitter node (word first, then string contents, the string, the bracket contents, the brackets, the statement, the block, …), shrink back through the same history (kept per view, cleared on other selection changes); without a tree: word, line, all; at every selection
- **Go to symbol, signature help, code actions, breadcrumbs / sticky scroll**: now the [Language features](#named-milestones) milestone
- **Keyboard Shortcuts view**: "Preferences: Keyboard Shortcuts" lists every command with its key(s) and context in the palette (fuzzy on name or key text, e.g. `ctrl+shift`; a "record keys" mode: the next key combo pressed filters to its bindings, VS Code's); shows where a binding came from (default / `keybindings.toml`) and user overrides; Enter on a row opens `keybindings.toml` with a `[[binding]]` stub for that command (creating the file like settings)
- **Light theme / theme files**: now the [Themes](#named-milestones) milestone
- **Word-based completion**: without a language server (plain text, YAML, TOML, Markdown, … or a server returning nothing), Ctrl+Space and quick suggestions offer the words in the open buffers (current buffer first, then others; VS Code's `editor.wordBasedSuggestions: matchingDocuments`), filtered with the same `completion::Filter`; collected off the UI thread per buffer and cached by revision; capped (e.g. 2 MB of text, 20k words); a setting to turn it off
- **Language server setup docs**: human-facing `docs/language-servers.md` (linked from a README / `docs/` index): which servers jed starts by itself (`clangd` for C / C++, `ty server` for Python, `rust-analyzer`) and how to install each on macOS (Homebrew, `uv tool install ty`, `rustup component add rust-analyzer`) and Linux (apt / dnf, uv, rustup); making them findable (`PATH`, a project `.venv`); clangd's `compile_commands.json` (CMake `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, `bear -- make`, SCons) and `.clangd`; ty's project config (`pyproject.toml` / `ty.toml`, the venv) and the SCons support jed adds; adding any other server through `.workspace` `[lsp.<language id>]` (command, args, `initialization_options`, with worked examples, e.g. a YAML or TOML server) and why workspace trust asks first; what works (the v1 features) and the keys for each; troubleshooting (the status bar / Problems, `--log-level` and "Log: Open" with the `lsp::wire` / `lsp::stderr` targets, a server that fails is not retried until restart); kept current when servers or config keys change (add a line to CLAUDE.md's conventions)
- ✅ **Scroll bars in the editors**: a vertical scroll bar per editor group (position and viewport size over the whole text; click and drag to scroll), as VS Code; also marks for selections / find matches / diagnostics / git changes later (VS Code's overview ruler)
  - Text and diff tabs always reserve the editor's last column (text never reflows; the find widget, popups, `view.width` and `GroupHit::editor` use the narrower area; the splash keeps the full width); a thumb only when the text doesn't fit or is scrolled (VS Code's `auto`). `scrollbar.rs` (model: `Scroll::text` — `max_top = lines - 1`, as jed scrolls the last line to the top — / `Scroll::rows` for diff tabs, `thumb`, `top_at`), `ui/scrollbar.rs` (drawing, `scrollbarSlider.{background,hoverBackground,activeBackground}`), `app/scrollbar.rs` (`Hit::ScrollBar(group)`, `Drag::ScrollBar { group, grab }`): a track click centres the thumb there and grabs it, a drag keeps the grab and continues off the bar until release; never focuses the group or moves cursors; the wheel over it scrolls the editor
  - Deferred: the overview-ruler marks; a setting to hide it; paging on track clicks; sub-cell thumb precision; a horizontal bar; a tab switch mid-drag keeps scrolling the new tab; the hover colour can stay after a wheel scroll until the pointer moves
  - Still to check by hand: the slider greys' visibility (also the 256-colour fallback); hover (needs any-motion mouse reporting); smooth drags on a huge file; a drag continuing outside the window; the diff tab's header cell above the bar; the find widget and popups clear of the bar
- ✅ **Revert hunks in the diff view**: a red Revert button on the index (left) side of an unstaged diff (Index ↔ Working Tree), opposite each hunk's Stage button; reverts that hunk in the working tree (the open buffer through `Buffer::edit`, one undo step, or the file on disk when not open) back to the index's text; a new theme key for the button
  - Only on `Pair::UNSTAGED` diffs: a ` Revert ` button (`diffEditor.revertAction`, white on Dark+'s error red) on each hunk's first row — side by side at the right end of the left (index) column, inline left of Stage, dropped when it doesn't fit (`DiffHit::reverts`); `git.revertHunk` (unbound, palette: the hunk at the diff cursor). Open buffer: `diff_view::revert_edit` (git's line split: CRLF / lone CR / BOM) applied with `apply_changes` (one `EditKind::Other` undo step, left dirty) only while the buffer still holds the shown text; read-only refused. Not open: blocking `git::stage::revert_hunk` (index and disk checked against what was shown, else `Stale`; untracked, conflicted, binary and symlinks refused; bytes as stored, mode kept, a deleted file back with the index's mode; the index untouched), then the file's text from before the revert is recorded in its file history (a failed record only warns) so File History: Restore can undo it; git status and the diff re-read
  - Deferred: revert from the gutter / its peek in a text tab; a confirmation before writing to disk (VS Code doesn't ask); a deleted file's revert records an empty "before" snapshot; a BOM only in the index's first line stays a difference on the buffer path
  - Still to check by hand: Revert in side-by-side and inline layouts; Cmd+Z in the file's tab; revert a file that isn't open, then File History: Restore
- **Kitty integration for recent workspaces** (deferred from M13; see [Recent workspaces](#workspace)): `jed_workspace` user var on start / exit / panic; `kitten @ ls` marks rows already open and focuses that window; `kitten @ launch --type=os-window --cwd <path> jed <path>` for one not open; off the UI thread with a timeout, message fallback; tests through a fake `kitten` on `PATH` (canned `ls` JSON, records its arguments); `docs/terminals/kitty.md` (`allow_remote_control`). Kitty isn't installed on the Linux test box: real-kitty behaviour is a hand check
- **Bug reporter**: "Help: Report Bug" (`help.reportBug`, palette, unbound) asks for a brief description of what happened (an `Input` prompt, multi-line not needed), then writes one self-contained Markdown file, `<state dir>/bug-reports/<UTC time>.md`, and shows its path in a message (also copied to the clipboard). Contents, each a section: the description; jed's version and git commit (built in at compile time), OS, terminal (`$TERM`, `$TERM_PROGRAM`, kitty keyboard protocol on or not, size, truecolor); the workspace (root, trust, `.workspace` settings, the layered settings, `keybindings.toml` overrides and load errors); the layout (groups and split tree, each group's tabs — path relative to the root, language, size, line count, dirty / deleted / read-only / disk conflict, EOL, encoding, selection count and primary's position, scroll — the active group and tab, focus, side / bottom panel and their views, open prompt / palette / find widget); language servers (per server: command, state, encoding, pending requests, diagnostics counts), git (branch label, entry counts), tasks (name, status), background work in flight (saves, parses, searches, backups); the last messages shown and the last ~50 commands run (a ring buffer of `Command::name()`s with times; no typed text); and the tail of the current log file (`newest_log`, last ~500 lines). Never file contents or typed text (privacy: the file is meant to be attached to a report); paths outside the workspace shown with `~`. Gathered on the UI thread as a cheap snapshot (O(groups + tabs + buffers), no rope scans), written in `spawn_blocking`. The panic hook could later write the same report (description = the panic message)
- ✅ **Scroll bars in the side bar's lists** (after the editor scroll bars): the same one-column bar (`scrollbar::Scroll::rows`, `ui::scrollbar::render`, the `scrollbarSlider.*` keys; VS Code's `auto`: a thumb only when the rows don't fit) at the right edge of the explorer tree, the Search view's results list and the Source Control list, with the same click (centre on the click and grab) / drag (keep the grab, continue off the bar until release) / hover / wheel behaviour, never moving the list's selection or taking focus. Made reusable for anything the side bar shows (and the bottom panel's lists: Problems, Locations — cheap to include once it's generic): the editor-only hit and drag (`Hit::ScrollBar(group)`, `Drag::ScrollBar { group, grab }`, `ui.scrollbar_hover`) become one keyed by a target (e.g. `ScrollTarget::{Group(GroupId), Side(SidePanel), Panel(PanelTab)}`) with one trait or pair of functions per list — rows, height, top as a row index, and set top — so a new list (a future outline / symbols view) gets a bar by implementing it; lists that keep their scroll as a key (`grouped::ListState`, `ScmView`) convert through `index_of` / `key_at` (binary search: no per-frame work proportional to the list). The bar takes the list's last column (right-aligned counts, git letters and the Source Control `+` / `−` buttons move left of it); the Search view's query and toggles keep the full width. Tests: drawn thumb size / position and a drag per list through `app.mouse`, the selection unchanged
  - `scrollbar::Scrollable` (`scroll()`, `set_top`) implemented by `view::TextScroll`, `DiffTab`, `Explorer` (`row_count`), `grouped::GroupedScroll` (`ListState::scrollable`: Search results, Problems, Locations) and `scm::ScmScroll`; `layout::ScrollTarget::{Group, Side(SidePanel), Panel(PanelTab)}` keys `Hit::ScrollBar`, `Drag::ScrollBar { target, grab }`, `ui.scrollbar_hover` and `HitMap::scrollbar(target)`; `App::with_scrollable(target, f)` (`None` when that list isn't shown: a drag whose list goes away scrolls nothing); drawing via `ui::scrollbar::take_column` / `render`. A new list gets a bar by implementing `Scrollable`, drawing with `take_column`, reporting under a `ScrollTarget` and adding an arm in `with_scrollable` (CLAUDE.md's Scroll bars convention)
  - The list column is always reserved; counts, git letters and the `+` / `−` buttons sit left of it; the Search query / toggles, the branch line and the panel's tab strip keep the full width; no bar on empty-state messages or task tabs; the wheel over a bar scrolls its list
  - Deferred: the hover colour can stay on after switching side views until the pointer moves
  - Still to check by hand: the slider colours over `sideBar` and `panel`; the blank column between counts / letters and the bar; dragging the explorer thumb in a large tree
- **First reveal in a never-drawn list**: a list's first selection reveal runs while its `height` is still 0 (never drawn), so the selected row is scrolled to the top — e.g. the first Ctrl+Shift+G hides the Source Control "Changes" header; skip the reveal (or defer it to the first draw) while `height == 0`, for every `ListState` / `ScmView` / explorer list, with a test per list
- **Tools view and tool logs**: now the [Tools view and logs](#named-milestones) milestone
- **PTY smoke tests**: a few end-to-end tests that run the real `jed` binary (`env!("CARGO_BIN_EXE_jed")`) in a pseudo-terminal (`portable-pty`, already a dependency) and parse what it writes with `vt100` (as task runs do), so the bytes crossterm emits and the bytes a terminal sends are tested, not just `TestBackend`: (1) after start the vt100 screen equals the `TestBackend` frame of the same state (text and colours, cell by cell; also after an edit and a scroll, to catch stale cells from ratatui's diff flush); (2) real input sequences reach the right commands — legacy and kitty-protocol (`CSI u`) keys incl. `Cmd+!` / Ctrl+@, bracketed paste, SGR mouse clicks and wheel; (3) start / quit leaves the terminal as it was (raw mode off, alt screen left, mouse / bracketed paste / kitty flags popped: check the mode-setting sequences in the output and the PTY's termios), and so does a forced panic (a test-only `JED_TEST_PANIC` hook or similar), which also leaves its crash backup; (4) wide graphemes (CJK, emoji, combining marks) land in the columns `pos::display_grapheme` says. Isolated `XDG_STATE_HOME` / `XDG_CONFIG_HOME` per test (they're child processes, so setting them is fine), a fixed size (e.g. 100×30), waits on screen contents with a timeout (no sleeps), `TERM=xterm-256color` and `COLORTERM=truecolor`; skip with a note where PTYs aren't available. Not a cross-emulator matrix: vt100 is one emulator; termless (TypeScript; xterm.js / Ghostty backends) or esctest could be added later if wide-character or protocol differences between terminals become a real problem

## Open questions
- _Add your own_
