---
id: update
name: Self-update from GitHub releases
summary: "\"jed: Check for Updates\" (`app.checkForUpdates`, palette, unbound) and `jed --update` (run before raw mode, progress printed to stdout) fetch the newest release of `jimcx/jed` and replace the running binary with it. The releases are the ones CI's `release` job publishes on a `v*` tag (`.github/workflows/build.yml`: `jed-<linux|macos>-<uname -m>.tar.gz`, the stripped `jed` inside)"
status: open
priority: 0
dependencies: []
---

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
