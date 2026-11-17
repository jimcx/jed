# jmux

A small terminal multiplexer in the style of jed's terminal tabs: sessions that keep running when you
detach, tabs and splits of shells, scrollback, find, and a side bar of the commands you ran. The terminal
handling is jed's (shared through `crates/jterm`), so the keys and the feel match.

## Install

```sh
scripts/install                  # builds jed and jmux, copies both into ~/.local/bin (or $1 / $PREFIX_BIN)
scripts/install --only jmux      # just jmux (--only jed: just jed)
cargo build --release -p jmux    # just jmux, uninstalled: target/release/jmux
```

Release tags publish `jmux-<os>-<arch>.tar.gz` beside `jed-<os>-<arch>.tar.gz` (macOS and Linux; one `jmux`
binary in each).

## Sessions

A session is a server process (started by the first command that needs it) that owns the shells; `jmux` attaches
a client to it. Closing the host terminal or detaching leaves everything running.

| Command | Does |
|---|---|
| `jmux` | Attach to the most recently used session, or start one |
| `jmux new [-s name] [-c dir]` | Start a session (shell in `dir`) and attach |
| `jmux attach [-t name]` | Attach (the most recently used by default); takes the session over from another client |
| `jmux --touch`, `jmux new --touch`, `jmux attach --touch` | Attach with the touch layout: tab bars three rows tall, tabs padded, wider `×` and scroll targets (3 cells), for a touch screen. It lasts until this client detaches or another takes the session over; the shells lose two rows per group |
| `jmux ls` | List the sessions |
| `jmux kill-session -t name` | End a session and its shells |
| `jmux rename-session [-t name] new` | Rename a session (the current one inside jmux) |
| `jmux install-shell-integration` | Add jmux's shell integration to your shell's init file |

Session names are `A-Z a-z 0-9 _ . -`, up to 64 characters. Detach with **Ctrl+Shift+D**. jmux inside jmux is
refused (`$JMUX` is set in every shell it starts).

## Keys

The defaults follow jed's terminal tab: Cmd on macOS where Linux uses Ctrl or Ctrl+Shift (plain Ctrl keys belong to
the shell). Unbound keys go to the shell. Ctrl+Shift combinations need a terminal that speaks the kitty keyboard
protocol; see [Terminal setup](#terminal-setup).

| Action | Linux | macOS |
|---|---|---|
| New terminal | Ctrl+Shift+T, Ctrl+\` | Cmd+T, Ctrl+\` |
| Close terminal (asks if something runs) | Ctrl+Shift+W | Cmd+W |
| Next / previous tab | Ctrl+PageDown / PageUp | same |
| Go to tab 1-9 | Alt+1-9 | Ctrl+1-9 |
| Split right / down | Ctrl+\\ / Ctrl+Alt+\\ | Cmd+\\ / Cmd+Alt+\\ |
| Focus group 1-8 | Ctrl+1-8 | Cmd+1-8 |
| Focus the group left / right / above / below | Alt+Shift+arrows | Cmd+Alt+arrows |
| Next / previous group | Ctrl+Shift+. / Ctrl+Shift+, | same |
| Scroll a page up / down | Shift+PageUp / PageDown | same |
| Scroll lines up / down | Ctrl+Shift+Up / Down | same |
| Scroll to top / bottom | Ctrl+Home / End | same |
| Copy selection | Ctrl+Shift+C | Cmd+C |
| Find in the terminal | Ctrl+Shift+F | Cmd+F |
| Search bar: next / previous | Enter or F3 / Shift+Enter or Shift+F3 | same |
| Search bar: match case | Alt+C | Cmd+Alt+C |
| Search bar: close | Esc | Esc |
| Show / hide the commands side bar | Ctrl+Shift+B | Cmd+B |
| Give the side bar the keys | Ctrl+Shift+E | Cmd+Shift+E |
| Install shell integration | Ctrl+Shift+I | same |
| Reload settings and key bindings | Ctrl+Shift+R | same |
| Detach | Ctrl+Shift+D | same |

The mouse works too: click a tab or group to focus it, drag a tab to move it, wheel to scroll, drag to select
(Shift keeps the selection for jmux when a program uses the mouse). Paste is the host terminal's own.

### keybindings.toml

`~/.config/jmux/keybindings.toml` rebinds commands, in jed's format (`[[binding]]` with `key`, `command` and an
optional `context`):

```toml
[[binding]]
key = "ctrl+shift+v"            # modifiers in the order cmd, ctrl, alt, shift; lowercase keys (left, f2, backslash)
command = "terminal.splitRight"
context = "terminal"            # optional; terminal (the default), search, side or confirm
```

A command named in the file is bound only to the file's keys (its default keys go), and a file key takes over
whatever default was on it in that context. The `search` and `side` contexts fall back to `terminal`. A bad entry (an
unknown command, context or key) is skipped and shown as a notice, e.g. `keybindings.toml: unknown command "x"`; the
other entries apply. A file that is not valid TOML is one error and the keys stay as they were. Reload with
Ctrl+Shift+R.

Command names: `terminal.new`, `terminal.close`, `terminal.nextTab`, `terminal.previousTab`, `terminal.goToTab1` ..
`terminal.goToTab9`, `terminal.splitRight`, `terminal.splitDown`, `terminal.focusLeftGroup` / `Right` / `Above` /
`Below`, `terminal.focusNextGroup`, `terminal.focusPreviousGroup`, `terminal.focusGroup1` .. `8`,
`terminal.copySelection`, `terminal.scrollUp`, `terminal.scrollDown`, `terminal.scrollPageUp`,
`terminal.scrollPageDown`, `terminal.scrollToTop`, `terminal.scrollToBottom`, `terminal.find`, `search.next`,
`search.previous`, `search.toggleCase`, `search.close`, `sideBar.toggle`, `sideBar.focus`, `session.detach`,
`session.installShellIntegration`, `session.reloadSettings`, `confirm.yes`, `confirm.no`. The side bar's list keys
(`sideBar.up`, `.down`, `.pageUp`, `.pageDown`, `.first`, `.last`, `.open`, `.back`) and the single-line field
editing in the search bar and the side bar's filter (`search.field.*`) have names too (see `jmux/src/command.rs`,
`Command::name`).

## settings.toml

`~/.config/jmux/settings.toml` (a missing file means all defaults):

| Key | Type | Default | Meaning |
|---|---|---|---|
| `shell` | string | `$SHELL`, else `/bin/sh` | The program a new terminal runs |
| `scrollback` | whole number, 0-10000000 | 5000 | Lines kept above a terminal's screen |
| `scrollback_compression` | true / false | false | Keep old scrollback lines zstd-compressed (less memory, some CPU) |
| `command_history` | whole number, 0-100000 | 500 | The most commands the side bar keeps |
| `side_bar_width` | whole number, 1-500 | 32 | The side bar's width in columns |
| `cursor_style` | `"block"`, `"line"`, `"underline"` | `"block"` | The host terminal's cursor while attached |
| `tabs_at_bottom` | `true`, `false` | `false` | Each group's tab bar on its last row, the terminal above it (applies at once on Reload Settings) |

A key that is unknown or badly typed is reported as a notice (`settings.toml: line 3: ...`, the first problem and
how many more) and skipped; the rest applies. A TOML syntax error makes the whole file unusable: one error,
defaults.

The file is read when a session's server starts and on **Reload Settings** (Ctrl+Shift+R, which also re-reads
`keybindings.toml`). `shell`, `scrollback` and `scrollback_compression` apply to the *next* terminal, not the
running ones; `cursor_style` is read by the client when it attaches (re-attach to change it).

## Shell integration

Optional, for zsh, bash and fish: a small hook in your shell's init file that tells jmux when a command starts and
ends, its exit code and the directory. With it a tab is named after the running program and shows a glyph for the
last result, and the commands side bar lists what you ran (running first, then finished, with duration and age;
Enter jumps to the terminal that ran it). Set it up with `jmux install-shell-integration` or Ctrl+Shift+I; the hook
does nothing outside jmux (it is guarded on `$JMUX`).

## Status bar

` work  2 terminals · 1 running  <title> ... Ctrl+Shift+D detach`: the session name, the number of terminals and
how many have a command running, the active terminal's title (dropped first when the window is narrow) and the detach
key. A notice (settings or key binding problems, for example) replaces it until the next key.

## Terminal setup

On macOS the terminal must pass Cmd keys on to the program, and everywhere it needs the kitty keyboard protocol.
Config snippets for kitty, Ghostty, WezTerm, Alacritty and iTerm2 are in
[../docs/terminals/](../docs/terminals/README.md). Every Cmd key jmux binds is one jed binds too, so a terminal set
up for jed works for jmux (kitty scopes its mappings by a user variable: jmux sets `jmux`, so those lines need a
`var:jmux` twin; see kitty.md there).
