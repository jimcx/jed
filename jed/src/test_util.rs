//! Test helpers: where the repository (the workspace root) is, copying a test repository, and a
//! clock for frame budgets.

use std::path::Path;

/// The repository root, the parent of jed's package directory (`CARGO_MANIFEST_DIR` is `jed/`):
/// where `.venv`, `docs/`, `rust-toolchain.toml` and `claude-plugin/` live. Jed's own files
/// (`src/`, `examples/`) stay under `CARGO_MANIFEST_DIR`.
pub(crate) fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("jed/ has a parent")
}

/// A copy (`cp -a`) of the test repository at `root`, beside it as `name`. The app's background
/// git reads can make and drop files (`index.lock`) in `.git` while it is copied, failing `cp`
/// ("No such file"): it tries again on a fresh copy.
pub(crate) fn copy_repo(root: &Path, name: &str) -> std::path::PathBuf {
    let to = root.with_file_name(name);
    for tries in 1.. {
        let out = std::process::Command::new("cp")
            .arg("-a")
            .arg(root)
            .arg(&to)
            .output()
            .unwrap();
        if out.status.success() {
            break;
        }
        assert!(tries < 10, "cp -a: {}", String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::remove_dir_all(&to);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    to
}

/// A clock for frame budget tests: wall time in release builds (what a budget is about); in
/// debug, this thread's CPU time. Debug bounds only catch whole-file work done on the UI thread,
/// and a loaded machine (parallel test runs) stretches wall time past them. Measure only code
/// with no `.await` in it (a multi-thread runtime may move the task to another thread).
pub(crate) fn frame_clock() -> std::time::Duration {
    if cfg!(debug_assertions) {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime fills `ts`.
        let r = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        assert_eq!(r, 0, "clock_gettime");
        std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
    } else {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::repo_root;

    #[test]
    fn repo_root_is_the_workspace_root_not_jeds_package() {
        let root = repo_root();
        assert!(root.join("rust-toolchain.toml").is_file(), "{}", root.display());
        assert!(root.join("jed/Cargo.toml").is_file());
        assert!(root.join("docs/forges.md").is_file());
    }
}
