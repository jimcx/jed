//! Recent workspaces (no app state; `app/recent.rs` records the root at start and drives
//! "Open Recent Workspace"): the list on disk, the picker's entries, and how picking one opens it.
//!
//! `<state dir>/recent.toml` holds workspace roots, most recent first, no duplicates, at most
//! [`MAX`]: `[[workspace]]` tables with a `path`. Every change is a read-modify-write under an
//! advisory lock on `recent.toml.lock` (so two jed processes starting at once both get in), written
//! atomically (a temp file beside it, fsynced, renamed): a lost race loses an entry at worst,
//! never the file. A file that doesn't parse counts as empty and is rewritten by the next change.
//! Every disk function is blocking.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::termctl::WindowRef;

/// In the state dir.
pub const FILE_NAME: &str = "recent.toml";
/// The list keeps this many workspaces.
pub const MAX: usize = 50;

/// The list's file in `state_dir`.
pub fn file(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE_NAME)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentFile {
    #[serde(default)]
    workspace: Vec<Item>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    path: String,
}

/// The list in `text`, most recent first (duplicates and entries beyond [`MAX`] dropped).
/// `Err`: not a list jed wrote.
pub fn parse(text: &str) -> Result<Vec<PathBuf>, String> {
    let file: RecentFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let mut out: Vec<PathBuf> = Vec::new();
    for item in file.workspace.into_iter().rev() {
        push(&mut out, Path::new(&item.path));
    }
    Ok(out)
}

/// `paths` as the file stores them (a path that isn't UTF-8 is left out: TOML can't hold it).
pub fn render(paths: &[PathBuf]) -> String {
    let file = RecentFile {
        workspace: paths
            .iter()
            .filter_map(|p| p.to_str())
            .map(|p| Item { path: p.to_string() })
            .collect(),
    };
    let body = toml::to_string(&file).unwrap_or_default();
    format!("# Recently opened jed workspaces, most recent first (written by jed).\n\n{body}")
}

/// Puts `root` first in `list`, dropping an earlier copy and anything beyond [`MAX`].
pub fn push(list: &mut Vec<PathBuf>, root: &Path) {
    list.retain(|p| p != root);
    list.insert(0, root.to_path_buf());
    list.truncate(MAX);
}

/// Records `root` as the most recent workspace.
/// Blocking: call from spawn_blocking.
pub fn record(state_dir: &Path, root: &Path) -> std::io::Result<()> {
    if root.to_str().is_none() {
        return Err(std::io::Error::other(format!("{} isn't UTF-8", root.display())));
    }
    update(state_dir, |list| push(list, root)).map(drop)
}

/// The recent workspaces that still exist (folders), most recent first, `current` left out.
/// Folders gone since are pruned from the file.
/// Blocking: call from spawn_blocking.
pub fn list(state_dir: &Path, current: &Path) -> std::io::Result<Vec<PathBuf>> {
    let path = file(state_dir);
    // Nothing to read, nothing to prune: don't create the state dir for that.
    if !path.exists() {
        return Ok(Vec::new());
    }
    let list = update(state_dir, |list| {
        list.retain(|p| {
            let keep = p.is_dir();
            if !keep {
                tracing::info!(path = %p.display(), "recent workspace gone: dropped");
            }
            keep
        })
    })?;
    Ok(list.into_iter().filter(|p| p != current).collect())
}

/// Reads the list, lets `change` edit it and writes it back if it changed (or the file was
/// corrupt), all under the lock. Returns the list as written.
fn update(state_dir: &Path, change: impl FnOnce(&mut Vec<PathBuf>)) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(state_dir)?;
    let path = file(state_dir);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join(format!("{FILE_NAME}.lock")))?;
    // Some file systems can't lock: carry on, the rename still keeps the file whole.
    if let Err(e) = lock.lock() {
        tracing::warn!(error = %e, "can't lock the recent workspaces list");
    }
    let (mut list, rewrite) = match std::fs::read_to_string(&path) {
        Ok(text) => match parse(&text) {
            Ok(list) => (list, false),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "recent workspaces list unreadable: starting afresh");
                (Vec::new(), true)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), true),
        // Not UTF-8 and the like: as corrupt.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => (Vec::new(), true),
        Err(e) => return Err(e),
    };
    let before = list.clone();
    change(&mut list);
    if rewrite || list != before {
        write(&path, &render(&list))?;
    }
    Ok(list)
}

fn write(path: &Path, text: &str) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A recent workspace as the picker lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// The terminal window already showing jed on this workspace, when the terminal backend
    /// (`termctl`) knows one: its row is marked and picking it focuses that window.
    pub window: Option<WindowRef>,
}

impl Entry {
    pub fn new(path: PathBuf) -> Self {
        Self { path, window: None }
    }
}

/// What picking a recent workspace does. jed never switches its own workspace: it opens the
/// folder in another jed: raising its terminal window, or launching one, through a
/// `termctl::Backend`; without one, a message with the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// Raise the terminal window already running jed on it.
    Focus(WindowRef),
    /// Open a new terminal window running `jed <path>`.
    Launch,
    /// Say how to open it: the path and the `jed <path>` command.
    Message,
}

/// How to open `entry`, given whether a terminal backend is present.
pub fn how_to_open(entry: &Entry, has_backend: bool) -> Open {
    match (&entry.window, has_backend) {
        (Some(w), _) => Open::Focus(w.clone()),
        (None, true) => Open::Launch,
        (None, false) => Open::Message,
    }
}

/// The status-bar message for opening `path` by hand.
pub fn message(path: &Path, home: &Path) -> String {
    format!("To open {}, run: {}", display(path, home), command(path))
}

/// The command line that opens `path` in jed: `jed <path>`, shell-quoted when needed.
pub fn command(path: &Path) -> String {
    format!("jed {}", shell_quote(&path.to_string_lossy()))
}

/// `s` as one POSIX shell word: as is when it only holds characters no shell treats specially,
/// else in single quotes (a `'` inside as `'\''`).
pub fn shell_quote(s: &str) -> Cow<'_, str> {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./,:@%+=".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(format!("'{}'", s.replace('\'', r"'\''")))
    }
}

/// `path` with `home` shown as `~` (`~` itself, `~/rest`); other paths as they are.
pub fn display(path: &Path, home: &Path) -> String {
    if home.as_os_str().is_empty() || home == Path::new("/") {
        return path.display().to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The folder's own name (the whole path for `/`).
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn push_puts_first_dedupes_and_caps() {
        let mut list = paths(&["/a", "/b", "/c"]);
        push(&mut list, Path::new("/c"));
        assert_eq!(list, paths(&["/c", "/a", "/b"]));
        push(&mut list, Path::new("/d"));
        assert_eq!(list, paths(&["/d", "/c", "/a", "/b"]));
        push(&mut list, Path::new("/d"));
        assert_eq!(list, paths(&["/d", "/c", "/a", "/b"]));
        for i in 0..60 {
            push(&mut list, &PathBuf::from(format!("/w{i}")));
        }
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0], PathBuf::from("/w59"));
        assert_eq!(list[MAX - 1], PathBuf::from("/w10"));
    }

    #[test]
    fn records_read_back_most_recent_first() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        record(&state, Path::new("/x/one")).unwrap();
        record(&state, Path::new("/x/two")).unwrap();
        record(&state, Path::new("/x/one")).unwrap();
        let text = std::fs::read_to_string(file(&state)).unwrap();
        assert_eq!(parse(&text).unwrap(), paths(&["/x/one", "/x/two"]));
        assert!(text.contains("[[workspace]]\npath = \"/x/one\""), "{text}");
        // A file with duplicates (edited by hand) keeps the first of each.
        let text = "[[workspace]]\npath = \"/a\"\n[[workspace]]\npath = \"/b\"\n[[workspace]]\npath = \"/a\"\n";
        assert_eq!(parse(text).unwrap(), paths(&["/a", "/b"]));
        assert_eq!(parse("").unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_corrupt_file_counts_as_empty_and_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_path_buf();
        for junk in [&b"not [toml"[..], b"[[workspace]]\nname = 3\n", b"\xff\xfe\x00"] {
            std::fs::write(file(&state), junk).unwrap();
            assert!(list(&state, Path::new("/none")).unwrap().is_empty());
            let text = std::fs::read_to_string(file(&state)).unwrap();
            assert_eq!(parse(&text).unwrap(), Vec::<PathBuf>::new(), "rewritten empty");
            std::fs::write(file(&state), junk).unwrap();
            record(&state, Path::new("/p")).unwrap();
            let text = std::fs::read_to_string(file(&state)).unwrap();
            assert_eq!(parse(&text).unwrap(), paths(&["/p"]));
        }
    }

    #[test]
    fn list_drops_missing_folders_and_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let state = base.join("state");
        let (a, b, gone, file_not_dir) = (base.join("a"), base.join("b"), base.join("gone"), base.join("f"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&gone).unwrap();
        std::fs::write(&file_not_dir, "").unwrap();
        for p in [&file_not_dir, &gone, &b, &a] {
            record(&state, p).unwrap();
        }
        std::fs::remove_dir(&gone).unwrap();
        assert_eq!(list(&state, &a).unwrap(), vec![b.clone()]);
        // Pruned from the file; the current workspace stays in it.
        let text = std::fs::read_to_string(file(&state)).unwrap();
        assert_eq!(parse(&text).unwrap(), vec![a, b]);
        // No file: nothing, and no state dir made.
        let none = base.join("no-state");
        assert!(list(&none, base).unwrap().is_empty());
        assert!(!none.exists());
    }

    #[test]
    fn a_record_waits_for_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_path_buf();
        record(&state, Path::new("/first")).unwrap();
        // Another jed holds the lock: the record waits for it (without the lock it would read
        // and write at once).
        let lock = std::fs::File::create(state.join(format!("{FILE_NAME}.lock"))).unwrap();
        lock.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = {
            let state = state.clone();
            std::thread::spawn(move || {
                record(&state, Path::new("/second")).unwrap();
                tx.send(()).unwrap();
            })
        };
        let waited = rx.recv_timeout(std::time::Duration::from_millis(300));
        assert!(waited.is_err(), "recorded while the lock was held");
        assert_eq!(
            parse(&std::fs::read_to_string(file(&state)).unwrap()).unwrap(),
            paths(&["/first"])
        );
        drop(lock);
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("recorded after the lock went");
        thread.join().unwrap();
        assert_eq!(
            parse(&std::fs::read_to_string(file(&state)).unwrap()).unwrap(),
            paths(&["/second", "/first"])
        );
    }

    #[test]
    fn writes_replace_the_file_never_overwrite_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_path_buf();
        record(&state, Path::new("/a")).unwrap();
        // A hard link to the old file keeps the old text when the new one is renamed over it; an
        // in-place write (which a crash could leave half done) would change both.
        let old = state.join("old.toml");
        std::fs::hard_link(file(&state), &old).unwrap();
        record(&state, Path::new("/b")).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&old).unwrap()).unwrap(), paths(&["/a"]));
        assert_eq!(
            parse(&std::fs::read_to_string(file(&state)).unwrap()).unwrap(),
            paths(&["/b", "/a"])
        );
    }

    #[test]
    fn concurrent_records_keep_the_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let state = state.clone();
                std::thread::spawn(move || {
                    for j in 0..5 {
                        record(&state, &PathBuf::from(format!("/t{i}/{j}"))).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let list = parse(&std::fs::read_to_string(file(&state)).unwrap()).unwrap();
        assert_eq!(list.len(), 40, "the lock serialises the writers");
        let leftovers: Vec<_> = std::fs::read_dir(&state)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn home_shows_as_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(display(Path::new("/home/me/src/jed"), home), "~/src/jed");
        assert_eq!(display(Path::new("/home/me"), home), "~");
        assert_eq!(display(Path::new("/home/meg/x"), home), "/home/meg/x");
        assert_eq!(display(Path::new("/srv/x"), home), "/srv/x");
        assert_eq!(display(Path::new("/srv/x"), Path::new("/")), "/srv/x");
        assert_eq!(folder_name(Path::new("/home/me/src/jed")), "jed");
        assert_eq!(folder_name(Path::new("/")), "/");
    }

    #[test]
    fn commands_are_shell_quoted_when_needed() {
        assert_eq!(command(Path::new("/home/me/src/jed")), "jed /home/me/src/jed");
        assert_eq!(command(Path::new("/tmp/my proj")), "jed '/tmp/my proj'");
        assert_eq!(command(Path::new("/tmp/it's")), r"jed '/tmp/it'\''s'");
        assert_eq!(command(Path::new("/tmp/$x")), "jed '/tmp/$x'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(
            message(Path::new("/home/me/a b"), Path::new("/home/me")),
            "To open ~/a b, run: jed '/home/me/a b'"
        );
        let e = Entry::new("/p".into());
        assert_eq!((how_to_open(&e, false), e.window.clone()), (Open::Message, None));
        assert_eq!(how_to_open(&e, true), Open::Launch);
        let w = Entry {
            window: Some(WindowRef("3".into())),
            ..e
        };
        assert_eq!(how_to_open(&w, true), Open::Focus(WindowRef("3".into())));
        assert_eq!(how_to_open(&w, false), Open::Focus(WindowRef("3".into())));
    }
}
