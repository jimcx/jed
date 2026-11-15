# lsp

Language server clients. No drawing; the app (`app/lsp.rs`) owns a `Manager` and gets `Event`s as `AppEvent::Lsp`. Summary only: [docs/dev/lsp.md](../../../docs/dev/lsp.md) has every type, message and rule — read the entry for the file you touch, and add detail there, not here. Requests from the app: [conventions](../../../docs/dev/conventions.md).

- `mod.rs` — `Manager`: `new(launcher)` (`None` = servers off, the test default), `start(sink)`, `sync` (which server each language resolves to; equal `ServerConfig`s share one), `restart`, `publish` (the diagnostics store)
- `server.rs` — `Launcher` / `ProcessLauncher`, the per-server `Task` (launch, `initialize`, one `select!` over stdout / stderr / app commands, shutdown), its tool log lines
- `config.rs` — `ServerConfig`, `Builtins`, `resolve` (workspace → user → built-in; trust)
- `documents.rs` — document sync (`didOpen` / `didChange` / `didSave` / `didClose`) from buffers' tracked changes
- `diagnostics.rs` — parsing `publishDiagnostics` into `Published`, placing them as marks
- `progress.rs` — `$/progress` tokens per server
- `rpc.rs` — `Message`, `Id`, `RpcError` (`is_routine`), framing
- `uri.rs` — `from_path` / `to_path`
- `scons.rs`, `typescript.rs` — per-server preparation (SCons globals for ty; locating `typescript-language-server` and its tsserver)
- `fake.rs` (`cfg(test)`) — `FakeLauncher` / `FakeOptions`: an in-process scripted server

Rules:

- Nothing is spawned before `Manager::start` (called from `App::start`).
- A server in `State::Failed` stays and is never restarted by `sync`; only `restart(name)` replaces it.
- A request that depends on the turn's edits must follow its `didChange`: call `sync_documents` first.
- Positions cross the wire as `LspPos` in the server's `PosEncoding`; convert only through `buffer::pos`.
- Logging: `lsp::wire` at trace (every message), `lsp::stderr` at debug, the rest under `jed::lsp`.
