---
id: crates_workspace
name: Cargo workspace for shared crates
summary: the repository becomes a cargo workspace (jed at the root, empty `crates/jtui` and `crates/jterm`) with the checks, CI and docs covering every member
status: open
priority: 0
dependencies: []
---

See [Shared crates and jmux](../PLAN.md#shared-crates-and-jmux), [jmux's plan](../jmux/PLAN.md#crates).

  - Root `Cargo.toml` gains `[workspace]` (`members = ["crates/*"]`, jed the root package and default member) and `[workspace.package]` / `[workspace.dependencies]` for what the crates will share (edition, ratatui, crossterm, vt100, tokio, serde, unicode-*, tracing); the `[patch.crates-io]` forks and `[profile.*]` stay at the root, where they cover every member
  - `crates/jtui` and `crates/jterm` exist as libraries with a `//!` doc saying what they will hold and nothing else; jed depends on both by path; `jterm` depends on `jtui`
  - `scripts/check` runs fmt, clippy and tests for the whole workspace (`--workspace`); `scripts/check test <filter>` still finds a test in any member; `.github/workflows/build.yml` likewise
  - `rustfmt.toml` and `rust-toolchain.toml` apply to every member (they are found from the root); `build.rs` and `JED_VERSION` stay jed's
  - Docs: a new `docs/dev/crates.md` (what goes in which crate, the rule that shared code names no application, how a jed module becomes a re-export, where each crate's tests go); one line in the root `CLAUDE.md` pointing at it; `.claude/commands/task.md` and `.opencode/commands/task.md` say where a crate's or jmux's tests live
  - Done when: `scripts/check` passes and reports the two new crates' (empty) test runs; `cargo build --release` still produces `target/release/jed`; `cargo metadata` lists three members; nothing under `src/` changed
