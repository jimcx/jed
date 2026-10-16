//! File history: a snapshot of every save under `<root>/.red/history/`, independent of git.
//!
//! One folder per file, named by `folder_name` (a hash of its workspace-relative path, `/`
//! separated), holding `entries.toml` (the relative path and the entries, oldest first: id,
//! time, size, content hash) and one snapshot file per entry, named by its id, with the exact
//! bytes saved (BOM and line endings kept). `.red/.gitignore` (`*`) keeps it out of git.
//!
//! Every function here touching the disk is blocking, and they all take one process-wide lock,
//! so concurrent saves (and renames) never lose an entry. `entries.toml` is written to a temp
//! file and renamed over; a snapshot is written before the entry naming it, and a trimmed one
//! removed after, so a reader never sees an entry without its bytes (short of another process).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workspace::config::HistoryConfig;

/// red's per-workspace folder (history; hidden from the explorer, quick open and search, its
/// watcher events ignored).
pub const RED_DIR: &str = ".red";
const INDEX: &str = "entries.toml";

/// Serializes every history read and write in this process.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// One snapshot of a file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Unique within the file's history; also the snapshot's file name.
    pub id: String,
    /// When it was recorded: milliseconds since the Unix epoch.
    pub time: u64,
    /// The snapshot's length in bytes.
    pub size: u64,
    /// SHA-256 of the bytes, lowercase hex.
    pub hash: String,
}

impl Entry {
    /// What names this entry where a `Copy` value is needed (a diff tab's side).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            time: self.time,
            hash: hash_prefix(&self.hash),
        }
    }
}

/// An entry as a diff tab's side names it: its time and the first 8 bytes of its hash (an id is
/// a `String`, and `diff_view::Text` is `Copy`). Entries of one file never share both (`record`
/// skips a duplicate of the newest; a rename's merge keeps ids apart, not contents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Snapshot {
    pub time: u64,
    pub hash: u64,
}

fn hash_prefix(hash: &str) -> u64 {
    hash.get(..16)
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Index {
    /// The file's workspace-relative path, `/` separated.
    path: String,
    /// Oldest first.
    #[serde(default, rename = "entry")]
    entries: Vec<Entry>,
}

/// What `record` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Added(Entry),
    /// The bytes are the newest snapshot's.
    Duplicate,
    /// Over `max_file_size`.
    TooLarge,
    /// `enabled = false` or `max_entries = 0`.
    Disabled,
}

/// `<root>/.red/history`.
pub fn history_dir(root: &Path) -> PathBuf {
    root.join(RED_DIR).join("history")
}

/// `path` relative to `root` when it has a history: inside the workspace, not in `.red/`, and
/// valid UTF-8 (the key is a string).
pub fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    match parts.next()? {
        Component::Normal(first) if first != RED_DIR => {}
        _ => return None,
    }
    if parts.any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    key(rel)?;
    Some(rel.to_path_buf())
}

/// The relative path as stored: components joined with `/`.
fn key(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// The folder holding the history of the file with this key.
pub fn folder_name(key: &str) -> String {
    hex(&Sha256::digest(key.as_bytes())[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Now, in milliseconds since the Unix epoch (entries' `time`).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn folder(root: &Path, key: &str) -> PathBuf {
    history_dir(root).join(folder_name(key))
}

fn context(what: &str, path: &Path) -> impl FnOnce(io::Error) -> String {
    let path = path.display().to_string();
    let what = what.to_string();
    move |e| format!("{what} {path}: {e}")
}

fn key_of(rel: &Path) -> Result<String, String> {
    key(rel).ok_or_else(|| format!("not a UTF-8 path: {}", rel.display()))
}

/// The index in `dir`, `None` when there is none.
fn read_index(dir: &Path) -> Result<Option<Index>, String> {
    let path = dir.join(INDEX);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(context("can't read", &path)(e)),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The index for `key` in `dir`: an empty one when missing; an error when the folder belongs to
/// another path (a hash collision, or a hand-edited file).
fn index_for(dir: &Path, key: &str) -> Result<Index, String> {
    match read_index(dir)? {
        Some(index) if index.path != key => Err(format!(
            "{} holds the history of {}, not {key}",
            dir.display(),
            index.path
        )),
        Some(index) => Ok(index),
        None => Ok(Index {
            path: key.to_string(),
            entries: Vec::new(),
        }),
    }
}

fn write_index(dir: &Path, index: &Index) -> Result<(), String> {
    let text = toml::to_string(index).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{INDEX}.tmp"));
    fs::write(&tmp, text).map_err(context("can't write", &tmp))?;
    let path = dir.join(INDEX);
    fs::rename(&tmp, &path).map_err(context("can't write", &path))
}

/// Creates `.red/` with its `.gitignore` (`*`) when missing.
fn ensure_red_dir(root: &Path) -> Result<(), String> {
    let red = root.join(RED_DIR);
    fs::create_dir_all(&red).map_err(context("can't create", &red))?;
    let ignore = red.join(".gitignore");
    if !ignore.exists() {
        fs::write(&ignore, "*\n").map_err(context("can't write", &ignore))?;
    }
    Ok(())
}

/// Records `bytes` (what was just saved to `rel`) as the file's newest snapshot, unless they
/// equal the newest one, then drops the oldest entries beyond `max_entries`.
///
/// Blocking: call from spawn_blocking.
pub fn record(root: &Path, rel: &Path, bytes: &[u8], limits: &HistoryConfig) -> Result<Recorded, String> {
    if !limits.enabled || limits.max_entries == 0 {
        return Ok(Recorded::Disabled);
    }
    if bytes.len() as u64 > limits.max_file_size {
        return Ok(Recorded::TooLarge);
    }
    let key = key_of(rel)?;
    let hash = hex(&Sha256::digest(bytes));
    let _guard = lock();
    let dir = folder(root, &key);
    let mut index = index_for(&dir, &key)?;
    if index.entries.last().is_some_and(|e| e.hash == hash) {
        return Ok(Recorded::Duplicate);
    }
    ensure_red_dir(root)?;
    fs::create_dir_all(&dir).map_err(context("can't create", &dir))?;

    let time = now_ms();
    let mut id = format!("{time}-{}", &hash[..8]);
    let mut n = 1;
    while index.entries.iter().any(|e| e.id == id) || dir.join(&id).exists() {
        n += 1;
        id = format!("{time}-{}-{n}", &hash[..8]);
    }
    let snapshot = dir.join(&id);
    fs::write(&snapshot, bytes).map_err(context("can't write", &snapshot))?;
    let entry = Entry {
        id,
        time,
        size: bytes.len() as u64,
        hash,
    };
    index.entries.push(entry.clone());
    let excess = index.entries.len().saturating_sub(limits.max_entries);
    let dropped: Vec<Entry> = index.entries.drain(..excess).collect();
    if let Err(e) = write_index(&dir, &index) {
        let _ = fs::remove_file(&snapshot);
        return Err(e);
    }
    for old in dropped {
        if let Err(e) = fs::remove_file(dir.join(&old.id)) {
            tracing::warn!(target: "red::history", id = old.id, "can't remove a trimmed snapshot: {e}");
        }
    }
    Ok(Recorded::Added(entry))
}

/// The file's entries, newest first (none without a history).
///
/// Blocking: call from spawn_blocking.
pub fn entries(root: &Path, rel: &Path) -> Result<Vec<Entry>, String> {
    let key = key_of(rel)?;
    let _guard = lock();
    let mut entries = index_for(&folder(root, &key), &key)?.entries;
    entries.reverse();
    Ok(entries)
}

/// The bytes of the entry `id` of the file.
///
/// Blocking: call from spawn_blocking.
#[cfg_attr(not(test), allow(dead_code))] // tests (the app reads by `Snapshot`)
pub fn read(root: &Path, rel: &Path, id: &str) -> Result<Vec<u8>, String> {
    let key = key_of(rel)?;
    let _guard = lock();
    let dir = folder(root, &key);
    if !index_for(&dir, &key)?.entries.iter().any(|e| e.id == id) {
        return Err(format!("no history entry {id} for {key}"));
    }
    let path = dir.join(id);
    fs::read(&path).map_err(context("can't read", &path))
}

/// The bytes of the entry named by `snapshot` (the newest, should several match).
///
/// Blocking: call from spawn_blocking.
pub fn read_snapshot(root: &Path, rel: &Path, snapshot: Snapshot) -> Result<Vec<u8>, String> {
    let key = key_of(rel)?;
    let _guard = lock();
    let dir = folder(root, &key);
    let index = index_for(&dir, &key)?;
    let Some(entry) = index.entries.iter().rev().find(|e| e.snapshot() == snapshot) else {
        return Err(format!("that history entry of {key} is gone"));
    };
    let path = dir.join(&entry.id);
    fs::read(&path).map_err(context("can't read", &path))
}

/// How long before `now` (ms) `time` (ms) was, as the history list shows it: "just now",
/// "42 sec ago", "3 min ago", "5 hr ago", "2 days ago", "3 mo ago", "1 yr ago".
pub fn ago(time: u64, now: u64) -> String {
    let secs = now.saturating_sub(time) / 1000;
    let (mins, hours, days) = (secs / 60, secs / 3600, secs / 86_400);
    match () {
        _ if secs < 5 => "just now".into(),
        _ if secs < 60 => format!("{secs} sec ago"),
        _ if mins < 60 => format!("{mins} min ago"),
        _ if hours < 24 => format!("{hours} hr ago"),
        _ if days == 1 => "1 day ago".into(),
        _ if days < 30 => format!("{days} days ago"),
        _ if days < 365 => format!("{} mo ago", days / 30),
        _ => format!("{} yr ago", days / 365),
    }
}

/// A size as the history list shows it: "512 B", "1.5 KB", "12 KB", "3.2 MB".
pub fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    let scaled = |v: f64, unit: &str| {
        if v < 10.0 {
            format!("{v:.1} {unit}")
        } else {
            format!("{} {unit}", v.round() as u64)
        }
    };
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < K * K {
        scaled(b / K, "KB")
    } else {
        scaled(b / (K * K), "MB")
    }
}

/// `time` (ms since the epoch) as `2026-10-17 14:03:09 UTC` (no time zone database: UTC).
pub fn utc_label(time: u64) -> String {
    let secs = time / 1000;
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Moves the history of `from` (a file, or every file under a folder) to `to`, after the file
/// or folder was renamed. A history already at the new path (a file deleted there earlier) is
/// merged, entries in time order. Returns how many files' histories moved.
///
/// Blocking: call from spawn_blocking.
pub fn rename(root: &Path, from: &Path, to: &Path) -> Result<usize, String> {
    let (from, to) = (key_of(from)?, key_of(to)?);
    let _guard = lock();
    let base = history_dir(root);
    let dirs = match fs::read_dir(&base) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(context("can't read", &base)(e)),
    };
    let mut moved = 0;
    let mut errors = Vec::new();
    for dir in dirs.flatten() {
        let dir = dir.path();
        let index = match read_index(&dir) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let new_key = if index.path == from {
            to.clone()
        } else if let Some(rest) = index.path.strip_prefix(&from)
            && rest.starts_with('/')
        {
            format!("{to}{rest}")
        } else {
            continue;
        };
        match move_history(&dir, index, &folder(root, &new_key), new_key) {
            Ok(()) => moved += 1,
            Err(e) => errors.push(e),
        }
    }
    match errors.first() {
        None => Ok(moved),
        Some(e) => Err(e.clone()),
    }
}

fn move_history(dir: &Path, mut index: Index, target: &Path, key: String) -> Result<(), String> {
    if target == dir {
        return Ok(());
    }
    index.path = key;
    if !target.exists() {
        // The index first names the new path, then the folder moves: either way it's whole.
        write_index(dir, &index)?;
        return fs::rename(dir, target).map_err(context("can't move", dir));
    }
    let mut merged = index_for(target, &index.path)?;
    for entry in index.entries {
        let mut entry = entry;
        let src = dir.join(&entry.id);
        if merged.entries.iter().any(|e| e.id == entry.id) {
            entry.id = format!("{}-moved", entry.id);
        }
        let dst = target.join(&entry.id);
        fs::rename(&src, &dst).map_err(context("can't move", &src))?;
        merged.entries.push(entry);
    }
    merged.entries.sort_by_key(|e| e.time);
    write_index(target, &merged)?;
    fs::remove_dir_all(dir).map_err(context("can't remove", dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> HistoryConfig {
        HistoryConfig::default()
    }

    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    fn contents(root: &Path, rel: &str) -> Vec<Vec<u8>> {
        let rel = Path::new(rel);
        entries(root, rel)
            .unwrap()
            .iter()
            .map(|e| read(root, rel, &e.id).unwrap())
            .collect()
    }

    #[test]
    fn relative_paths() {
        let root = Path::new("/w");
        assert_eq!(
            relative(root, Path::new("/w/src/a.rs")),
            Some(PathBuf::from("src/a.rs"))
        );
        assert_eq!(relative(root, Path::new("/x/a.rs")), None, "outside the workspace");
        assert_eq!(relative(root, Path::new("/w/.red/history/x")), None);
        assert_eq!(relative(root, Path::new("/w")), None);
        assert_eq!(relative(root, Path::new("/w/a/../b")), None);
        assert_eq!(folder_name("src/a.rs").len(), 32);
        assert_ne!(folder_name("src/a.rs"), folder_name("src/b.rs"));
        // Stable: the folder names on disk must not change between versions.
        assert_eq!(folder_name("a"), "ca978112ca1bbdcafac231b39a23dc4d");
    }

    #[test]
    fn records_newest_first_skipping_duplicates() {
        let (_d, root) = root();
        let rel = Path::new("src/a.txt");
        assert!(entries(&root, rel).unwrap().is_empty());
        assert!(matches!(
            record(&root, rel, b"one\r\n", &limits()),
            Ok(Recorded::Added(_))
        ));
        assert_eq!(record(&root, rel, b"one\r\n", &limits()), Ok(Recorded::Duplicate));
        let Ok(Recorded::Added(two)) = record(&root, rel, b"\xEF\xBB\xBFtwo", &limits()) else {
            panic!()
        };
        assert_eq!(two.size, 6);
        assert_eq!(two.hash, hex(&Sha256::digest(b"\xEF\xBB\xBFtwo")));
        // Back to an older text: not the newest, so recorded again.
        assert!(matches!(
            record(&root, rel, b"one\r\n", &limits()),
            Ok(Recorded::Added(_))
        ));
        assert_eq!(
            contents(&root, "src/a.txt"),
            vec![b"one\r\n".to_vec(), b"\xEF\xBB\xBFtwo".to_vec(), b"one\r\n".to_vec()]
        );
        assert_eq!(entries(&root, rel).unwrap()[1], two);
        assert_eq!(fs::read_to_string(root.join(".red/.gitignore")).unwrap(), "*\n");
        let index = fs::read_to_string(history_dir(&root).join(folder_name("src/a.txt")).join(INDEX)).unwrap();
        assert!(index.starts_with("path = \"src/a.txt\""), "{index}");
        assert!(read(&root, rel, "nope").is_err());
    }

    #[test]
    fn limits_apply() {
        let (_d, root) = root();
        let rel = Path::new("a");
        let small = HistoryConfig {
            max_entries: 2,
            max_file_size: 4,
            ..limits()
        };
        for text in ["1", "2", "3"] {
            record(&root, rel, text.as_bytes(), &small).unwrap();
        }
        assert_eq!(contents(&root, "a"), vec![b"3".to_vec(), b"2".to_vec()]);
        let dir = history_dir(&root).join(folder_name("a"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3, "two snapshots and the index");
        assert_eq!(record(&root, rel, b"12345", &small), Ok(Recorded::TooLarge));
        assert_eq!(
            record(&root, rel, b"1234", &small).map(|r| matches!(r, Recorded::Added(_))),
            Ok(true)
        );
        let off = HistoryConfig {
            enabled: false,
            ..limits()
        };
        assert_eq!(record(&root, rel, b"x", &off), Ok(Recorded::Disabled));
        assert_eq!(entries(&root, rel).unwrap().len(), 2);
    }

    #[test]
    fn a_folder_of_another_path_is_an_error() {
        let (_d, root) = root();
        record(&root, Path::new("a"), b"x", &limits()).unwrap();
        let dir = history_dir(&root).join(folder_name("a"));
        fs::write(dir.join(INDEX), "path = \"b\"\n").unwrap();
        assert!(record(&root, Path::new("a"), b"y", &limits()).is_err());
    }

    #[test]
    fn rename_moves_files_and_folders() {
        let (_d, root) = root();
        let l = limits();
        record(&root, Path::new("dir/a"), b"a", &l).unwrap();
        record(&root, Path::new("dir/sub/b"), b"b", &l).unwrap();
        record(&root, Path::new("dirt"), b"t", &l).unwrap();
        assert_eq!(rename(&root, Path::new("dir"), Path::new("new")), Ok(2));
        assert!(entries(&root, Path::new("dir/a")).unwrap().is_empty());
        assert_eq!(contents(&root, "new/a"), vec![b"a".to_vec()]);
        assert_eq!(contents(&root, "new/sub/b"), vec![b"b".to_vec()]);
        assert_eq!(
            contents(&root, "dirt"),
            vec![b"t".to_vec()],
            "a prefix that's not a folder"
        );

        // Onto a path with a history of its own: merged in time order.
        std::thread::sleep(std::time::Duration::from_millis(2));
        record(&root, Path::new("c"), b"c1", &l).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record(&root, Path::new("new/a"), b"a2", &l).unwrap();
        assert_eq!(rename(&root, Path::new("new/a"), Path::new("c")), Ok(1));
        assert_eq!(
            contents(&root, "c"),
            vec![b"a2".to_vec(), b"c1".to_vec(), b"a".to_vec()]
        );
        assert!(!history_dir(&root).join(folder_name("new/a")).exists());
        // No history: nothing to do.
        assert_eq!(rename(&root, Path::new("x"), Path::new("y")), Ok(0));
    }

    #[test]
    fn concurrent_records_keep_every_entry() {
        let (_d, root) = root();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || record(&root, Path::new("a"), format!("{i}").as_bytes(), &limits()))
            })
            .collect();
        for t in threads {
            t.join().unwrap().unwrap();
        }
        assert_eq!(entries(&root, Path::new("a")).unwrap().len(), 8);
    }

    #[test]
    fn read_snapshot_finds_the_entry_by_time_and_hash() {
        let (_d, root) = root();
        let rel = Path::new("a");
        record(&root, rel, b"one", &limits()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record(&root, rel, b"two", &limits()).unwrap();
        let all = entries(&root, rel).unwrap();
        assert_eq!(read_snapshot(&root, rel, all[0].snapshot()), Ok(b"two".to_vec()));
        assert_eq!(read_snapshot(&root, rel, all[1].snapshot()), Ok(b"one".to_vec()));
        assert_ne!(all[0].snapshot(), all[1].snapshot());
        let gone = Snapshot { time: 1, hash: 2 };
        assert!(read_snapshot(&root, rel, gone).unwrap_err().contains("gone"));
    }

    #[test]
    fn labels_for_the_list() {
        let now = 1_000_000_000_000;
        let ago_s = |s: u64| ago(now - s * 1000, now);
        assert_eq!(ago_s(0), "just now");
        assert_eq!(ago(now + 5000, now), "just now", "a clock that went back");
        assert_eq!(ago_s(42), "42 sec ago");
        assert_eq!(ago_s(180), "3 min ago");
        assert_eq!(ago_s(5 * 3600 + 59), "5 hr ago");
        assert_eq!(ago_s(86_400), "1 day ago");
        assert_eq!(ago_s(3 * 86_400), "3 days ago");
        assert_eq!(ago_s(95 * 86_400), "3 mo ago");
        assert_eq!(ago_s(400 * 86_400), "1 yr ago");

        assert_eq!(size_label(0), "0 B");
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(1536), "1.5 KB");
        assert_eq!(size_label(12 * 1024 + 100), "12 KB");
        assert_eq!(size_label(3_355_443), "3.2 MB");

        assert_eq!(utc_label(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_label(1_792_245_789_000), "2026-10-17 14:03:09 UTC");
        assert_eq!(utc_label(951_782_400_000), "2000-02-29 00:00:00 UTC");
    }
}
