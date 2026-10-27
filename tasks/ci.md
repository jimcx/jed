---
id: ci
name: CI
summary: 'CI on GitHub Actions (Linux and macOS): fmt, clippy, build, test, and release tarballs'
status: done
priority: 0
dependencies: []
---

  - Done: `.github/workflows/build.yml` on push, pull request and by hand: Ubuntu 26.04 and macOS 26, toolchain from `rust-toolchain.toml` (`rustup toolchain install`), `Swatinem/rust-cache`; `cargo fmt --check`, clippy with `-D warnings`, `cargo build --release`, `cargo test` (`scripts/timeout 900`); each platform's stripped binary uploaded as `jed-<os>-<arch>.tar.gz`; a `v*` tag publishes a GitHub Release with both tarballs
  - Both keymaps (Cmd on macOS, Ctrl on Linux) and both watchers (FSEvents, inotify) now run in CI
  - Deferred: the release-only timing budgets (`--release milestone` / `search`), since shared runners are too noisy for them. Run those locally
