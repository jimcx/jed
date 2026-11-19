---
id: kitty_keys_passthrough
name: Kitty keyboard protocol in the hosted terminal
summary: programs in jmux panes and jed's shell / task tabs can turn on the kitty keyboard protocol and get its key encoding, so jed inside jmux (or neovim, helix, fish) gets Ctrl+Shift and the other disambiguated keys
status: open
priority: 0
dependencies: [fake_shell]
---

jed and jmux already speak the kitty keyboard protocol to the terminal they run in (`jtui::terminal::start` pushes `DISAMBIGUATE | REPORT_ALTERNATE_KEYS`; crossterm decodes the replies), but not to the programs they host. `jterm::keys::encode` only produces xterm's encoding; the `vt100` parser drops `CSI … u`; and `jterm` answers no query at all: no reply ever goes from the parser back to the PTY. So jed run inside jmux (or in another jed's shell tab) finds no protocol: crossterm's `supports_keyboard_enhancement` sends `CSI ? u` + DA1 and gets neither answer, so jed decides there is no protocol, shows its "Terminal lacks the kitty keyboard protocol" message and loses Ctrl+Shift and the other keys that need the protocol. It may also wait out crossterm's timeout before starting (check at the start of -a).

One encoder in `jterm` serves both hosts: jmux (`App::key`, `jmux/src/app.rs`) and jed (`app/tasks.rs::send_key`, shell and task tabs) already share `keys::encode`.

  - Needs: `jterm::keys` (`encode`, `paste`), `jterm::run` (`TitleCallback`, the vt100 `Callbacks` impl, `read_output`, the writer thread's `Input` channel and `merge`), the `vt100` fork's `Callbacks::unhandled_csi`, `jtui::terminal` (`start`, `kitty_supported`, `KITTY_PUSHED`), jmux's `App::key` (drops `KeyEventKind::Release` first), jed's event loop (`app/mod.rs`, also drops releases), the `fake_shell` task's `fakesh` (end-to-end tests)
  - Spec: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>. Flags: 1 disambiguate, 2 report event types, 4 report alternate keys, 8 report all keys as escape codes, 16 report associated text
  - The program's flags (`jterm`):
    - `unhandled_csi` with `c == 'u'`: `CSI > f u` pushes `f`, `CSI < n u` pops `n` (default 1; popping past the bottom empties the stack), `CSI = f ; m u` sets the top (`m` 1 set, 2 or, 3 and-not; default 1), `CSI ? u` queries. A stack per screen (main and alternate, as the spec says), each capped (kitty's limit, so a program pushing in a loop can't grow it without bound) and switched with the alternate screen. Cleared on RIS (`ESC c`) and when the run ends
    - `Run` exposes the active screen's current flags (`kitty_flags() -> u8`, 0 = legacy) next to `application_cursor` / `bracketed_paste`
  - Replies to queries (`jterm`): the parser runs on the reader thread, so a reply goes into a queue the reader thread drains into the writer thread's `Input` channel (as `Input::Bytes`, or a new `Input::Reply` that `merge` never drops) after each `process`. Answer `CSI ? u` with `CSI ? <flags> u`, and DA1 (`CSI c` / `CSI 0 c`) with a fixed VT220-ish answer (`CSI ? 62 ; 22 c`). Programs send `CSI ? u` then DA1 and take the first answer as the verdict, so DA1 has to be answered or every probing program waits out its timeout. Nothing else is answered in this task (DSR / DA2 are Deferred)
  - The encoder (`jterm::keys`): `encode(key, KeyModes { application_cursor, kitty_flags })` (or a second argument), xterm's encoding unchanged at flags 0. With flags:
    - 1: Esc, Alt+ / Ctrl+ combinations and keypad keys as `CSI code ; mods u` (mods = 1 + Shift 1, Alt 2, Ctrl 4, Super 8, Hyper 16, Meta 32, CapsLock 64, NumLock 128); Enter, Tab and Backspace stay legacy unless modified; plain text stays text. Super / Hyper / Meta keys that aren't the host's own reach the program here (today `encode` drops them)
    - 2: `:2` repeat and `:3` release on the mods field (press is the default `:1`), so releases must reach `encode` (see Wiring); with flag 2 alone Enter, Tab and Backspace releases aren't sent (the spec's exception, unless 8 is on)
    - 4: the shifted key as the alternate code (`CSI 97:65 ; 2 u`) from crossterm's `Char`; crossterm doesn't give the base-layout key, so that field is never sent
    - 8: every key as an escape code, text too, and the modifier keys themselves (`KeyCode::Modifier` → 57441… `u`)
    - 16 (only with 8): the text field from the `Char` (crossterm doesn't carry the terminal's own associated text)
    - the functional keys' numbers: F1–F12 keep their legacy forms (`CSI 1;m P` …, `CSI n ~`), F13–F35, keypad, media, lock and modifier keys from the spec's table; arrows / Home / End / PageUp … keep their legacy letters and tildes, with `;mods:event` when flag 2 needs it
  - Wiring (jmux and jed): pass `run.kitty_flags()` into `encode` in jmux's `App::key` and jed's `send_key`. Release events reach `encode` when the focused run's flags include 2 (jmux's `App::key` and jed's event loop drop them before any lookup today; a release still never runs a binding). What jed / jmux push to their own terminal stays as it is (1 | 4): without 2 and 8 from the outer terminal, a pane gets no releases and no plain-text-as-escape keys, which jed, neovim, helix and fish don't need. Pushing more, or re-pushing per focused pane (in jmux that's a server→client message, since the client owns the terminal), is Deferred
  - Tests:
    - `jterm::keys` table-driven unit tests per flag: Esc, Ctrl+Shift+letter, Alt+letter, Ctrl+Enter, keypad, F13, a modifier key, alternate keys, repeat / release forms, and flags 0 unchanged (the existing tests pass as they are)
    - `jterm::run` unit tests on a parser: push / pop / set (all three modes), separate main and alternate stacks, the cap, RIS clearing, and the replies to `CSI ? u` and DA1 reaching the writer
    - end to end (`jed/src/kitty_keys_passthrough_tests.rs`, `jmux/src/kitty_keys_passthrough_tests.rs`), on `fakesh`: a `fakesh` command (one match arm, e.g. `keyecho [flags]`) that puts its tty in raw mode, pushes the flags, queries them, prints the reply and then every byte it reads as hex until `q`, then pops and restores. The tests: the query is answered with the pushed flags; Ctrl+Shift+A, Esc and Alt+X arrive as their `CSI … u` bytes; with flag 2 a release arrives as `:3`; after the pop the same keys arrive in xterm's encoding; in jmux and in a jed shell tab
  - Docs: `jterm::keys` and `jterm::run` `//!` docs (the protocol, what is answered), `docs/dev/crates.md`'s jterm part, jmux's README (Ctrl+Shift keys reach programs that ask for the protocol; jed inside jmux keeps them), the root README / docs where they say jed needs the protocol from its terminal
  - Done when:
    - jed started in a jmux pane, and in a jed shell tab, finds the protocol without delay (no "Terminal lacks …" message) and its Ctrl+Shift bindings work, checked by hand in kitty or Ghostty
    - the `keyecho` end-to-end tests above pass in jmux and in a jed shell tab
    - a program that never asks for the protocol gets exactly today's bytes (the existing key tests pass unchanged)
    - `scripts/check` passes
  - Subtasks (one agent and one commit each):
    - kitty_keys_passthrough-a: `jterm`: the flag stacks, the reply queue (`CSI ? u`, DA1), the encoder for flags 1, 2, 4, 8 and 16, `Run::kitty_flags`, their unit tests and `jterm`'s docs
    - kitty_keys_passthrough-b: wiring in jmux and jed (flags into `encode`, releases through when asked), `fakesh`'s `keyecho`, the end-to-end tests in both apps, the READMEs, the hand check of jed inside jmux, mark the task ✅
  - Deferred:
    - pushing 2 / 8 to the outer terminal, or per focused pane (a jmux server→client message), so panes can get releases and plain keys as escape codes without the host asking for them for itself
    - the base-layout key (flag 4's third field) and the terminal's own associated text: crossterm's `KeyEvent` doesn't carry them (would need raw input bytes)
    - other queries (DSR 6n cursor position, DA2, XTVERSION) if programs turn out to wait on them
