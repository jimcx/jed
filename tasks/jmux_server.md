---
id: jmux_server
name: "jmux: sessions, server and client"
summary: a `jmux` binary with named sessions — a detached server per session owning one full-screen shell, a thin client that attaches, detaches and re-attaches
status: in-progress
priority: 0
dependencies: [jterm]
---

See [jmux's plan](../jmux/PLAN.md): [Architecture](../jmux/PLAN.md#architecture), [Sessions](../jmux/PLAN.md#sessions), [Tests](../jmux/PLAN.md#tests).

  - Needs: `jtui` (host terminal setup, socket dirs, theme, keymap), `jterm` (`Run`, `ShellTab`, drawing); jed's `ipc.rs` and `app/mod.rs` event loop as the models to copy from
  - A new workspace member `jmux/` (binary `jmux`), in the checks and CI as the crates are
  - A stream backend in `jtui`: a ratatui `Backend` that writes crossterm's commands into a byte buffer and takes its size from the caller instead of the tty; the server draws through it
  - The protocol, socket naming, stale-socket handling, attach / detach / take-over and session end as the plan's Architecture section says
  - Starting a server: `jmux new` re-executes itself as `jmux server --session <name>` detached (new session id, stdio closed, cwd kept for the first shell) and waits for the socket before attaching; the server forks nothing before its runtime starts except this
  - The server `App` in this task: one shell filling the screen above a one-line status bar (session name); keys and paste go to the shell, the detach key detaches; the client's size is the shell's size
  - The client restores the terminal on every exit path (detach, session end, server death, a panic, SIGHUP / SIGTERM) and prints one line saying why it left
  - Logging: `tracing` to `<state dir>/jmux/logs`, one file per server; nothing printed while the UI is up
  - Done when: `jmux new -s work` shows a shell; detaching returns to the outer shell with `[detached from work]`; `jmux ls` lists `work`; `jmux attach -t work` shows the same screen, scrollback included, at the new window size; a second attach takes the session over and the first client says so; `exit` in the shell ends the session, removes the socket and the client says `[session work ended]`; `kill -9` of the server makes the client restore the terminal and say the server went away; `jmux` inside a jmux shell is refused
  - Subtasks (one agent and one commit each):
    - ✅ jmux_server-a: the crate, the stream backend in `jtui`, the protocol module (frames, messages, encode / decode with tests) and the session directory (names, socket paths, listing, stale sockets)
      - `jtui::stream_backend::StreamBackend::new(cols, rows)`: `set_size`, `take_output` (drains the bytes), never reads the tty
      - `jmux/src/protocol.rs`: u32 BE length, kind byte (0 JSON, 1 raw output), max 16 MiB; `ClientMsg`, `ServerMsg`, `ByeReason` (+ `ServerError`) with `message(session)` for the client's last line; `FrameReader<M>`, async `read_msg` / `write_all_msg`, blocking `read_msg_blocking` (`Ok(None)` only on EOF at a frame boundary); `PROTOCOL_VERSION`
      - `jmux/src/session.rs`: `valid_name` (≤ 64 bytes), `socket_path`, `list` / `remove_stale` (stale = ECONNREFUSED / NotFound only), `next_free_name`, `most_recent` (newest socket mtime: touch it on attach)
      - `main.rs` is a stub; `protocol` / `session` carry `#[allow(dead_code)]` until b / c use them
    - ✅ jmux_server-b: the server — detached start, the listener, the `App` with one `ShellTab`, drawing to the attached client, input and resize from it, the session ending when the shell exits
      - `server.rs`: `spawn_server` (re-exec `jmux server --session <name> [--socket-dir <dir>]`, `setsid`, null stdio, waits ≤ 5 s for the socket; call it before any runtime or in `spawn_blocking`), `bind` (live socket refused with `AddrInUse`, stale replaced, a guard removes it), `run` (the event loop)
      - `app.rs`: one shell (24×80 until the first `Hello`), one attached client; a new `Hello` sends the old one `Bye TakenOver`; Ctrl+Shift+D / `Detach` / the client closing detach; per-connection output queue bounded (`OUT_QUEUE` 256: a client that stops reading is dropped); the shell ending sends `Bye SessionEnded` and `shutdown` awaits the writers
      - `ui.rs` / `theme.rs` / `logging.rs` (`<state dir>/jmux/logs/<name>-<pid>.log`, `JMUX_LOG`); `main.rs` parses only `server`; other subcommands still say "not yet implemented"
      - No `Kill` message or terminal count yet (c adds what `kill-session` / `ls` need); no SIGTERM handling in the server (a killed server leaves a stale socket, which `remove_stale` / `bind` handle)
    - jmux_server-c: the client and CLI — `new`, `attach`, `ls`, `kill-session`, `rename-session`, bare `jmux`; raw mode and restore; detach, take-over and server-death handling; the done-when tests (`jmux/src/jmux_server_tests.rs`), hand checks and marks the task ✅
