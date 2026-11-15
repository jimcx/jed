---
id: jed_dir
name: jed in its own directory
summary: "move jed's package (`src/`, `build.rs`, `assets/`, `examples/`, `NOTICES.md`, its `[package]`) into `jed/` beside `jmux/`; the root `Cargo.toml` becomes a virtual workspace"
status: in-progress
priority: 0
dependencies: []
---

The repository root is jed's package and the workspace at once, while jmux has its own `jmux/` directory. Move jed into `jed/` so both apps sit side by side over `crates/`, and the root holds only what the whole workspace shares.

Layout after the move:

| Moves into `jed/` | Stays at the root (shared by the workspace) |
|---|---|
| `src/` (with its nested `CLAUDE.md` files and `src/testdata/`) | `Cargo.toml` (now `[workspace]` only: members, `[workspace.dependencies]`, `[profile.*]`, `[patch.crates-io]`), `Cargo.lock`, `target/` |
| `build.rs` | `rust-toolchain.toml`, `rustfmt.toml` |
| jed's `[package]`, `[dependencies]`, `[build-dependencies]`, `[dev-dependencies]` → `jed/Cargo.toml` | `CLAUDE.md`, `PLAN.md`, `README.md`, `tasks/`, `docs/` (incl. `docs/terminals/`, shared with jmux), `scripts/`, `.github/` |
| `assets/` (compiled in by `build.rs`) | `crates/`, `jmux/` |
| `examples/` (the syntax plugin template, read by jed's tests) | `.workspace`, `.venv`, `.claude/`, `.opencode/`, `opencode.jsonc`, `claude-plugin/`, `.claude-plugin/` |
| `NOTICES.md` (jed's bundled dictionary's notices; `build.rs` packs it) | |

  - Needs: nothing; land it when no other task branch is open (every open branch touching `src/` has to rebase across the rename: git follows the renames on rebase, but expect conflicts in files both changed)
  - Decided: `PLAN.md`, `tasks/` and `docs/` stay at the root. `PLAN.md` is the repository's plan and the index of every task (jmux's included); `jmux/PLAN.md` stays jmux's design. Moving jed's design out of `PLAN.md` into a `jed/PLAN.md` is a separate, later question
  - Renames with `git mv` only in the commit that moves files, so `git log --follow` / blame keep the history; path fixes inside moved files are small enough for rename detection
  - Workspace: root `Cargo.toml` `members = ["crates/*", "jed", "jmux"]`, `default-members = ["jed"]` (plain `cargo build` / `cargo run` / `cargo test` at the root still mean jed, as today); `jed/Cargo.toml` uses `crates/` by `../crates/jtui` / `../crates/jterm` and `edition.workspace = true`, like `jmux/Cargo.toml`. `-p jed`, `target/release/jed` and the binary name are unchanged
  - Paths that break, found by grep (`CARGO_MANIFEST_DIR`, `include_str!` / `include_bytes!`, `#[path]`, relative `../`):
    - `include_str!("../../docs/terminals/README.md")` (`config/keymap.rs`) gains a `../`; the `assets/` and `examples/` includes and `build.rs`'s `#[path = "src/bundle.rs"]` / `assets/…` / `NOTICES.md` move with their files and stay as they are
    - `env!("CARGO_MANIFEST_DIR")` used as the repository root now points at `jed/`: `.venv/bin/ty` (milestone 9, fixes, todo tests), `rust-toolchain.toml` (claude_lsp, fixes, todo tests), `docs/forges.md` (`forge/config.rs`), `claude-plugin` / `.` (claude_ide tests), the copied `src` + `docs` (milestone 7), `workspace_search.rs`'s test root. Add one test helper (e.g. `test_util::repo_root()` = the manifest dir's parent) and use it for each of these, rather than sprinkling `..`; uses meaning jed's own files (`examples/…`, `src/…`) keep `CARGO_MANIFEST_DIR`
    - Test expectations with `src/…` paths in them (search results, quick open, file trees over the repository) — run the whole suite and fix what moves
  - Scripts and CI: `scripts/fetch-dictionary --bundled` writes into `jed/assets/dictionaries/`; `scripts/check`, `scripts/install`, `build.yml` already use `--workspace` / `-p jed` / the root `target/` and should need nothing — check each, and the release job's tarballs
  - `.workspace`, `docs/tasks.md` examples (`cargo build`, `cargo run -- …` at the root) still work through `default-members`; check
  - Docs and agent instructions: every `src/…` that means jed's source becomes `jed/src/…` (not `jmux/src/…` or `crates/*/src/…`); likewise `build.rs`, `assets/`, `examples/`, `NOTICES.md`. Files: root `CLAUDE.md` (Where things are, Tests, the `src/<id>_tests.rs` rules), `docs/dev/*.md` (`crates.md`'s "jed is the root package" paragraph rewritten: jed is `jed/`, the root a virtual workspace), the nested `CLAUDE.md` files, `PLAN.md`, `README.md` (build-from-source section), `NOTICES.md`'s links, `docs/*.md`, `jmux/PLAN.md` / `jmux/README.md` mentions of jed's files, `.claude/commands/` and `.opencode/commands/` (`task.md`, `task-review.md`: end-to-end test locations, `src/app/mod.rs`), and `src/` comments and `//!` docs naming jed's paths
  - `tasks/*.md`: rewrite jed paths in every file, open and done, with one reviewed script (done tasks are grepped when later work refers back to them, so stale paths would mislead); links like `../jmux/PLAN.md` are unchanged
  - Done when: `scripts/check` passes from the root; `cargo build`, `cargo run`, `cargo build --release -p jed -p jmux` and `scripts/install` work from the root; the release budget runs (`cargo test --release milestone …`, `search`) pass; `git log --follow jed/src/main.rs` reaches back past the move; a grep for root-relative `src/`, `build.rs`, `assets/`, `examples/` in docs, agent commands and tasks finds only `jed/…`, `jmux/…` or `crates/…` paths; the bundled dictionary and "Help: About" still show (the `assets/` and `NOTICES.md` packing); `scripts/tasks check` is clean
  - Subtasks (one agent and one commit each):
    - ✅ jed_dir-a: the move — `git mv` of `src/`, `build.rs`, `assets/`, `examples/`, `NOTICES.md` into `jed/`; `jed/Cargo.toml` split out, the root `Cargo.toml` a virtual workspace (`default-members = ["jed"]`); the `repo_root()` test helper and the include / manifest-dir fixes; `scripts/fetch-dictionary`; checks, scripts, CI and the release budgets pass. Only the docs a broken build or test needs change here
      - Done: 367 pure renames; root `Cargo.toml` workspace-only, `jed/Cargo.toml` holds the package; `jed/src/test_util.rs` `repo_root()` (with a test that it is the workspace root) replaces `CARGO_MANIFEST_DIR` for `.venv`, `rust-toolchain.toml`, `docs/forges.md`, `claude-plugin`, milestone 7's `docs` copy and workspace_search's reference test; keymap's terminals README include gained a `../`; `scripts/fetch-dictionary --bundled` → `jed/assets/dictionaries/`. Scripts, CI and `.workspace` needed nothing. Only `docs/dev/modules.md` / `testing.md` got the helper's lines; stale doc paths (incl. `task.md` in `.claude/commands/` and `.opencode/commands/`) are for jed_dir-b
      - The release budget `milestone_conflict_scan_budget_typing_in_a_1mb_file` fails on this Linux machine (~155µs vs 100µs) identically before the move; not caused by it
    - jed_dir-b: docs and agent instructions; task done — every jed path in `CLAUDE.md`, `docs/`, the nested `CLAUDE.md` files, `PLAN.md`, `README.md`, `NOTICES.md`, `jmux/` docs, `.claude/commands/` and `.opencode/commands/` (kept in step), source comments and `tasks/*.md` (one reviewed rewrite script, its rule in the commit body); the leftover-path grep; mark the task ✅
