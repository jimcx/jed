//! The session directory: one `<name>.sock` per session in the socket directory
//! (`jtui::socket::default_socket_dir(APP)`, created with `jtui::socket::prepare_dir`).
//!
//! Every function takes the directory, so tests pass a temp dir. A socket nothing listens on is
//! stale (its server died): [`list`] reports it as not live and [`remove_stale`] deletes it.
//! Names are `[A-Za-z0-9_.-]{1,64}`; an unnamed session gets the lowest free number.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const SUFFIX: &str = ".sock";
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    /// A server accepted a connection.
    pub live: bool,
    pub modified: SystemTime,
}

/// Whether `name` is usable as a session name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

pub fn socket_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{SUFFIX}"))
}

/// Is a server listening on `path`? A refusal or a missing file is dead; anything else (a permission
/// error, say) counts as live so that nothing is deleted on a guess.
fn is_live(path: &Path) -> bool {
    match UnixStream::connect(path) {
        Ok(_) => true,
        Err(e) => !matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound),
    }
}

/// The sessions in `dir` sorted by name (empty when the directory is missing).
///
/// Blocking: call from spawn_blocking
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = read
        .flatten()
        .filter_map(|e| {
            let file = e.file_name();
            let name = file.to_str()?.strip_suffix(SUFFIX)?.to_string();
            if !valid_name(&name) {
                return None;
            }
            let path = e.path();
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let live = is_live(&path);
            Some(Entry {
                name,
                path,
                live,
                modified,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Delete the stale sockets in `dir`; returns their names.
///
/// Blocking: call from spawn_blocking
pub fn remove_stale(dir: &Path) -> Vec<String> {
    list(dir)
        .into_iter()
        .filter(|e| !e.live)
        .filter(|e| std::fs::remove_file(&e.path).is_ok())
        .map(|e| e.name)
        .collect()
}

/// The lowest number ("0", "1", ...) not used by a live session; stale sockets count as free.
///
/// Blocking: call from spawn_blocking
pub fn next_free_name(dir: &Path) -> String {
    let live: Vec<String> = list(dir).into_iter().filter(|e| e.live).map(|e| e.name).collect();
    (0u32..)
        .map(|n| n.to_string())
        .find(|n| !live.contains(n))
        .unwrap_or_default()
}

/// Sets the socket's modification time to now (a client attached), so [`most_recent`] finds the
/// session used last. A failure is only logged: the session still works.
pub fn touch(path: &Path) {
    let Some(c) = path.to_str().and_then(|p| std::ffi::CString::new(p).ok()) else {
        return;
    };
    // SAFETY: c is a valid NUL-terminated path; null times mean "now".
    if unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), std::ptr::null(), 0) } != 0 {
        tracing::debug!(path = %path.display(), "cannot touch socket");
    }
}

/// The live session touched last (bare `jmux` attaches to it).
///
/// Blocking: call from spawn_blocking
pub fn most_recent(dir: &Path) -> Option<Entry> {
    list(dir)
        .into_iter()
        .filter(|e| e.live)
        .max_by(|a, b| a.modified.cmp(&b.modified).then_with(|| b.name.cmp(&a.name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn bind(dir: &Path, name: &str) -> UnixListener {
        UnixListener::bind(socket_path(dir, name)).unwrap()
    }

    #[test]
    fn names() {
        for ok in ["work", "a.b-c_d", "0", "A9", &"x".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "a b", "a/b", "é", "a:b", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn socket_path_in_dir() {
        assert_eq!(
            socket_path(Path::new("/r/jmux"), "work"),
            PathBuf::from("/r/jmux/work.sock")
        );
    }

    #[test]
    fn lists_live_and_stale_sorted() {
        let d = tempfile::tempdir().unwrap();
        let _b = bind(d.path(), "b");
        drop(bind(d.path(), "a"));
        let _c = bind(d.path(), "c");
        std::fs::write(d.path().join("notes.txt"), "").unwrap();
        std::fs::write(d.path().join("bad name.sock"), "").unwrap();
        let l = list(d.path());
        let got: Vec<_> = l.iter().map(|e| (e.name.as_str(), e.live)).collect();
        assert_eq!(got, [("a", false), ("b", true), ("c", true)]);
        assert_eq!(l[1].path, d.path().join("b.sock"));
        assert!(list(&d.path().join("missing")).is_empty());
    }

    #[test]
    fn removes_only_stale() {
        let d = tempfile::tempdir().unwrap();
        let _live = bind(d.path(), "live");
        drop(bind(d.path(), "dead"));
        assert_eq!(remove_stale(d.path()), ["dead"]);
        assert!(!d.path().join("dead.sock").exists());
        assert!(d.path().join("live.sock").exists());
        assert!(remove_stale(d.path()).is_empty());
    }

    #[test]
    fn lowest_free_number() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(next_free_name(d.path()), "0");
        let _zero = bind(d.path(), "0");
        let _two = bind(d.path(), "2");
        let _w = bind(d.path(), "work");
        assert_eq!(next_free_name(d.path()), "1");
        drop(bind(d.path(), "1"));
        assert_eq!(next_free_name(d.path()), "1");
    }

    #[test]
    fn most_recent_is_live_and_newest() {
        let d = tempfile::tempdir().unwrap();
        assert!(most_recent(d.path()).is_none());
        let _old = bind(d.path(), "old");
        let _new = bind(d.path(), "new");
        drop(bind(d.path(), "dead"));
        // A socket cannot be opened to set its time, so set it through the path.
        let set = |name: &str, secs: i64| {
            let c = std::ffi::CString::new(socket_path(d.path(), name).to_str().unwrap()).unwrap();
            let t = libc::timespec {
                tv_sec: secs as libc::time_t,
                tv_nsec: 0,
            };
            let times = [t, t];
            // SAFETY: c is a valid NUL-terminated path and times has two entries.
            assert_eq!(
                unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) },
                0
            );
        };
        set("old", 1000);
        set("new", 2000);
        set("dead", 3000);
        assert_eq!(most_recent(d.path()).unwrap().name, "new");
    }

    #[test]
    fn touch_makes_a_session_the_most_recent() {
        let d = tempfile::tempdir().unwrap();
        let _a = bind(d.path(), "a");
        let _b = bind(d.path(), "b");
        for (name, secs) in [("a", 1000), ("b", 2000)] {
            let c = std::ffi::CString::new(socket_path(d.path(), name).to_str().unwrap()).unwrap();
            let t = libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            };
            let times = [t, t];
            // SAFETY: c is a valid NUL-terminated path and times has two entries.
            assert_eq!(
                unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) },
                0
            );
        }
        assert_eq!(most_recent(d.path()).unwrap().name, "b");
        touch(&socket_path(d.path(), "a"));
        assert_eq!(most_recent(d.path()).unwrap().name, "a");
    }

    #[test]
    fn next_free_name_ignores_non_numeric_and_stale_and_non_sock_files() {
        let d = tempfile::tempdir().unwrap();
        drop(bind(d.path(), "0"));
        let _one = bind(d.path(), "1");
        std::fs::write(d.path().join("2"), "").unwrap();
        std::fs::write(d.path().join("2.txt"), "").unwrap();
        // "0" is stale, so free; a bare file "2" is not a session.
        assert_eq!(next_free_name(d.path()), "0");
        let _zero = bind_over(d.path(), "0");
        assert_eq!(next_free_name(d.path()), "2");
    }

    fn bind_over(dir: &Path, name: &str) -> UnixListener {
        let _ = std::fs::remove_file(socket_path(dir, name));
        bind(dir, name)
    }

    #[test]
    fn more_invalid_names() {
        for bad in ["a\nb", "a\0b", "a\\b", "名前", "a*", "~", "a/../b"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        for ok in [".hidden", "-x", "a.sock", "..a", "a.."] {
            assert!(valid_name(ok), "{ok}");
        }
    }

    #[test]
    fn remove_stale_keeps_non_sock_files_and_lists_dotted_names() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("keep.txt"), "").unwrap();
        std::fs::write(d.path().join(".sock"), "").unwrap();
        drop(bind(d.path(), "a.b"));
        assert_eq!(remove_stale(d.path()), ["a.b"]);
        assert!(d.path().join("keep.txt").exists());
        assert!(d.path().join(".sock").exists());
    }
}
