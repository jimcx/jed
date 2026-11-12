//! Where a process's unix socket lives: `$XDG_RUNTIME_DIR/<app>/<pid>.sock` in a `0700`
//! directory, with the fallbacks under `$TMPDIR`. The app's name is passed in.

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// The socket directory from the environment's values: `$XDG_RUNTIME_DIR/<app>` (absolute only),
/// else `$TMPDIR/<app>-<uid>`, else `/tmp/<app>-<uid>`.
pub fn socket_dir(app: &str, xdg_runtime_dir: Option<&OsStr>, tmpdir: Option<&OsStr>, uid: u32) -> PathBuf {
    let absolute = |v: Option<&OsStr>| {
        v.filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(xdg) = absolute(xdg_runtime_dir) {
        return xdg.join(app);
    }
    let base = absolute(tmpdir).unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("{app}-{uid}"))
}

/// [`socket_dir`] for this process's environment and user.
pub fn default_socket_dir(app: &str) -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    socket_dir(
        app,
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("TMPDIR").as_deref(),
        uid,
    )
}

/// The socket for process `pid` in `dir`.
pub fn socket_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

/// Create `dir` with mode `0700`, or check an existing one is ours and closed to others.
///
/// Blocking: call from spawn_blocking
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() {
        return Err(io::Error::other(format!("{} is not a directory", dir.display())));
    }
    if meta.uid() != uid {
        return Err(io::Error::other(format!("{} is owned by another user", dir.display())));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!("{} is accessible by others", dir.display())));
    }
    Ok(())
}

/// Delete the socket file; never panics, ignores errors (also for the panic hook).
pub fn remove_socket(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn socket_dir_choices() {
        let s = |v: &'static str| Some(OsStr::new(v));
        assert_eq!(
            socket_dir("jed", s("/run/user/1"), s("/t"), 5),
            PathBuf::from("/run/user/1/jed")
        );
        assert_eq!(
            socket_dir("jed", s("rel"), s("/var/t"), 5),
            PathBuf::from("/var/t/jed-5")
        );
        assert_eq!(socket_dir("jed", s(""), s("/var/t"), 5), PathBuf::from("/var/t/jed-5"));
        assert_eq!(socket_dir("jed", None, None, 5), PathBuf::from("/tmp/jed-5"));
        assert_eq!(socket_dir("jed", None, s("rel"), 5), PathBuf::from("/tmp/jed-5"));
        // The name is the caller's, not jed's.
        assert_eq!(
            socket_dir("alice", s("/run/user/1"), None, 5),
            PathBuf::from("/run/user/1/alice")
        );
        assert_eq!(socket_dir("alice", None, None, 5), PathBuf::from("/tmp/alice-5"));
        assert!(default_socket_dir("alice").to_string_lossy().contains("alice"));
        assert_eq!(socket_path(Path::new("/d"), 42), PathBuf::from("/d/42.sock"));
    }

    #[test]
    fn prepare_dir_creates_private_and_refuses_open() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("jed");
        prepare_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        prepare_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_dir(&dir).is_err());
        let file = tmp.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(prepare_dir(&file).is_err());
        // A symlink to a private directory of ours is still refused (someone could repoint it).
        let private = tmp.path().join("private");
        prepare_dir(&private).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let err = prepare_dir(&link).unwrap_err();
        assert!(err.to_string().contains("not a directory"), "{err}");
    }
}
