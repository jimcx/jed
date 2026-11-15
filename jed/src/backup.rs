//! Crash backups: dirty buffers' text kept under the state dir so a killed or crashed jed loses
//! at most one backup interval of edits (no app state; `app/backup.rs` schedules the writes,
//! the panic hook writes the same files synchronously, best effort).
//!
//! One folder per workspace, `<state dir>/backup/<32 hex of the root's SHA-256>/`, one file per
//! buffer, `<32 hex of the file path's SHA-256>.bak`, rewritten in place (atomically: a temp
//! file beside it, fsynced, renamed). An untitled buffer has no path: its file is
//! `untitled-<id>.bak` (`Key::Untitled`; the id is the buffer's own, `Buffer::untitled_id`, kept
//! when it is restored) and its header has `"untitled":"<id>"` and an empty path. A file is
//! three parts:
//!
//! ```text
//! jed-backup 1\n
//! {"root":…,"path":…,"bom":false,"disk":"12:9f…"|null,"time":1760000000000,"len":12}\n
//! <len bytes: the buffer's text, UTF-8, BOM left out>
//! ```
//!
//! `disk` is the file's `Stamp` the buffer's edits are based on (`Buffer::disk`), so a restore
//! can tell the file changed since. Reading checks the magic line, the version, the root, the
//! path's hash against the file name and the text's length and UTF-8: anything else is corrupt
//! (skipped with a log line, never offered). Every function here is blocking.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ropey::Rope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::buffer::file::{self, Stamp};

/// Under the state dir.
pub const DIR: &str = "backup";
/// How often changed dirty buffers are backed up: what killing the terminal can lose at most.
pub const INTERVAL: Duration = Duration::from_secs(30);
const MAGIC: &str = "jed-backup";
const VERSION: u32 = 1;
const EXT: &str = "bak";

/// What a backup is of: a file, or an untitled buffer (by its id).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    File(PathBuf),
    Untitled(String),
}

impl Key {
    /// A file name (`Untitled-2`'s backups are named by id; the app shows `Untitled-N` itself).
    pub fn describe(&self) -> String {
        match self {
            Key::File(p) => p.display().to_string(),
            Key::Untitled(id) => format!("untitled text {id}"),
        }
    }
}

/// A dirty buffer's text to back up (rope clones are cheap).
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Absolute; empty for an untitled buffer.
    pub path: PathBuf,
    /// `Some(id)`: an untitled buffer's (`path` is empty).
    pub untitled: Option<String>,
    pub rope: Rope,
    pub bom: bool,
    /// The file as the buffer's edits know it (`Buffer::disk`; `None`: no file).
    pub disk: Option<Stamp>,
}

/// A backup read back.
#[derive(Debug, Clone)]
pub struct Backup {
    /// Empty for an untitled buffer's.
    pub path: PathBuf,
    pub untitled: Option<String>,
    pub text: Rope,
    pub bom: bool,
    /// The file the text's edits were based on.
    pub disk: Option<Stamp>,
    /// When it was written (Unix ms).
    pub time: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    root: PathBuf,
    #[serde(default)]
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    untitled: Option<String>,
    bom: bool,
    disk: Option<String>,
    time: u64,
    len: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn name_hash(path: &Path) -> String {
    hex(&Sha256::digest(path.as_os_str().as_encoded_bytes())[..16])
}

impl Snapshot {
    pub fn key(&self) -> Key {
        key_of(&self.path, &self.untitled)
    }
}

impl Backup {
    /// For the offer prompt: an untitled buffer's is "untitled text" and its first line.
    pub fn label(&self) -> Option<String> {
        self.untitled.as_ref()?;
        let start = self.text.chunks().next().unwrap_or("");
        let line: String = start
            .trim_start()
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(30)
            .collect();
        Some(if line.is_empty() {
            "untitled text".to_string()
        } else {
            format!("untitled text \"{line}\"")
        })
    }

    pub fn key(&self) -> Key {
        key_of(&self.path, &self.untitled)
    }
}

fn key_of(path: &Path, untitled: &Option<String>) -> Key {
    match untitled {
        Some(id) => Key::Untitled(id.clone()),
        None => Key::File(path.to_path_buf()),
    }
}

/// The folder holding the workspace at `root`'s backups.
pub fn dir(state_dir: &Path, root: &Path) -> PathBuf {
    state_dir.join(DIR).join(name_hash(root))
}

/// The backup file of `path` in the workspace at `root`.
pub fn file(state_dir: &Path, root: &Path, path: &Path) -> PathBuf {
    dir(state_dir, root).join(format!("{}.{EXT}", name_hash(path)))
}

/// The backup file of `key`.
pub fn key_file(state_dir: &Path, root: &Path, key: &Key) -> PathBuf {
    match key {
        Key::File(path) => file(state_dir, root, path),
        Key::Untitled(id) => dir(state_dir, root).join(format!("untitled-{id}.{EXT}")),
    }
}

/// Writes (replaces) `snap`'s backup, atomically; returns the file written.
/// Blocking: call from spawn_blocking (the panic hook calls it directly).
pub fn write(state_dir: &Path, root: &Path, snap: &Snapshot, time: u64) -> std::io::Result<PathBuf> {
    let dest = key_file(state_dir, root, &snap.key());
    let folder = dest.parent().expect("backups are in a folder");
    std::fs::create_dir_all(folder)?;
    let header = Header {
        root: root.to_path_buf(),
        path: snap.path.clone(),
        untitled: snap.untitled.clone(),
        bom: snap.bom,
        disk: snap.disk.map(|s| s.encode()),
        time,
        len: snap.rope.len() as u64,
    };
    let json = serde_json::to_string(&header).map_err(std::io::Error::other)?;
    // Unique per write: the panic hook may write while a periodic write of the same file runs.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = dest.with_extension(format!("{EXT}.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        writeln!(f, "{MAGIC} {VERSION}")?;
        writeln!(f, "{json}")?;
        for chunk in snap.rope.chunks() {
            f.write_all(chunk.as_bytes())?;
        }
        let f = f.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| dest)
}

/// Removes `key`'s backup (none is fine).
/// Blocking: call from spawn_blocking.
pub fn remove(state_dir: &Path, root: &Path, key: &Key) -> std::io::Result<()> {
    match std::fs::remove_file(key_file(state_dir, root, key)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes every backup of the workspace (corrupt ones and leftover temp files too) except
/// those of the keys in `keep`; the folder too when it's left empty. Returns how many went.
/// Blocking: call from spawn_blocking.
pub fn remove_all(state_dir: &Path, root: &Path, keep: &HashSet<Key>) -> std::io::Result<usize> {
    let folder = dir(state_dir, root);
    let entries = match std::fs::read_dir(&folder) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let kept: HashSet<PathBuf> = keep.iter().map(|k| key_file(state_dir, root, k)).collect();
    let mut removed = 0;
    for entry in entries {
        let path = entry?.path();
        if kept.contains(&path) {
            continue;
        }
        std::fs::remove_file(&path)?;
        removed += 1;
    }
    // Fails (harmlessly) when something is left in it.
    let _ = std::fs::remove_dir(&folder);
    Ok(removed)
}

/// Reads one backup file of the workspace at `root`.
/// Blocking: call from spawn_blocking.
pub fn read(state_dir: &Path, root: &Path, at: &Path) -> Result<Backup, String> {
    let bytes = std::fs::read(at).map_err(|e| e.to_string())?;
    parse(&bytes, state_dir, root, at)
}

fn parse(bytes: &[u8], state_dir: &Path, root: &Path, at: &Path) -> Result<Backup, String> {
    let mut parts = bytes.splitn(3, |&b| b == b'\n');
    let (Some(magic), Some(header), Some(text)) = (parts.next(), parts.next(), parts.next()) else {
        return Err("truncated".into());
    };
    if magic != format!("{MAGIC} {VERSION}").as_bytes() {
        return Err("not a jed backup of this version".into());
    }
    let header: Header = serde_json::from_slice(header).map_err(|e| format!("bad header: {e}"))?;
    if header.root != root {
        return Err(format!("a backup of {}", header.root.display()));
    }
    if key_file(state_dir, root, &key_of(&header.path, &header.untitled)) != at {
        return Err(format!("named for another file than {}", header.path.display()));
    }
    if text.len() as u64 != header.len {
        return Err(format!("{} bytes of text, not {}", text.len(), header.len));
    }
    let text = std::str::from_utf8(text).map_err(|_| "text isn't UTF-8".to_string())?;
    let disk = match &header.disk {
        Some(s) => Some(Stamp::parse(s).ok_or_else(|| format!("bad disk stamp {s:?}"))?),
        None => None,
    };
    Ok(Backup {
        path: header.path,
        untitled: header.untitled,
        text: Rope::from_str(text),
        bom: header.bom,
        disk,
        time: header.time,
    })
}

/// The workspace's backups worth offering, sorted by path: corrupt ones are skipped with a log
/// line (left for a clean quit to remove), ones whose file on disk already holds their text are
/// removed (nothing to restore).
/// Blocking: call from spawn_blocking.
pub fn list(state_dir: &Path, root: &Path) -> Vec<Backup> {
    let folder = dir(state_dir, root);
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let at = entry.path();
        if at.extension().is_none_or(|e| e != EXT) {
            continue;
        }
        let backup = match read(state_dir, root, &at) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("backup {} skipped: {e}", at.display());
                continue;
            }
        };
        let on_disk = if backup.untitled.is_some() {
            None
        } else {
            file::read(&backup.path).ok().flatten()
        };
        if on_disk.is_some_and(|bytes| bytes == file::encode(&backup.text, backup.bom)) {
            tracing::info!("backup of {} dropped: the file holds its text", backup.path.display());
            let _ = std::fs::remove_file(&at);
            continue;
        }
        found.push(backup);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.untitled.cmp(&b.untitled)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(path: &Path, text: &str) -> Snapshot {
        Snapshot {
            path: path.to_path_buf(),
            untitled: None,
            rope: Rope::from_str(text),
            bom: true,
            disk: Some(Stamp::of(b"base")),
        }
    }

    #[test]
    fn backups_read_back_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        let path = root.join("a.txt");
        let at = write(&state, &root, &snap(&path, "héllo\nworld"), 42).unwrap();
        assert_eq!(at, file(&state, &root, &path));
        assert!(at.starts_with(state.join(DIR)));
        let b = read(&state, &root, &at).unwrap();
        assert_eq!(b.path, path);
        assert_eq!(b.text.to_string(), "héllo\nworld");
        assert!(b.bom);
        assert_eq!(b.disk, Some(Stamp::of(b"base")));
        assert_eq!(b.time, 42);
        // Rewritten in place.
        write(&state, &root, &snap(&path, "two"), 43).unwrap();
        let listed = list(&state, &root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].text.to_string(), "two");
        // Another workspace sees none of them, even copied under its own folder.
        let other = dir.path().join("other");
        assert!(list(&state, &other).is_empty());
        let copied = file(&state, &other, &path);
        std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
        std::fs::copy(&at, &copied).unwrap();
        assert!(read(&state, &other, &copied).unwrap_err().contains("a backup of"));
        assert!(list(&state, &other).is_empty());
        remove(&state, &root, &Key::File(path.clone())).unwrap();
        remove(&state, &root, &Key::File(path.clone())).unwrap();
        assert!(list(&state, &root).is_empty());
    }

    #[test]
    fn corrupt_backups_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        let good = write(&state, &root, &snap(&root.join("good"), "ok"), 1).unwrap();
        let bytes = std::fs::read(&good).unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("truncated text", bytes[..bytes.len() - 1].to_vec()),
            ("no header", b"jed-backup 1\n".to_vec()),
            ("bad magic", [b"jed-backup 9".as_slice(), &bytes[12..]].concat()),
            ("garbage", b"\x00\x01\x02".to_vec()),
        ];
        for (i, (what, bytes)) in cases.into_iter().enumerate() {
            let path = root.join(format!("bad{i}"));
            let at = file(&state, &root, &path);
            let text =
                String::from_utf8_lossy(&bytes).replace(&*root.join("good").to_string_lossy(), &path.to_string_lossy());
            let bytes = if what == "garbage" { bytes } else { text.into_bytes() };
            std::fs::write(&at, bytes).unwrap();
            assert!(read(&state, &root, &at).is_err(), "{what}");
        }
        // A good file under another file's name.
        std::fs::copy(&good, file(&state, &root, &root.join("renamed"))).unwrap();
        let listed = list(&state, &root);
        assert_eq!(listed.len(), 1, "only the good one");
        assert_eq!(listed[0].path, root.join("good"));
        // A clean quit removes them all, except what it's told to keep.
        let keep = HashSet::from([Key::File(root.join("good"))]);
        assert_eq!(remove_all(&state, &root, &keep).unwrap(), 5);
        assert_eq!(list(&state, &root).len(), 1);
        assert_eq!(remove_all(&state, &root, &HashSet::new()).unwrap(), 1);
        assert!(!super::dir(&state, &root).exists());
    }

    #[test]
    fn untitled_backups_are_keyed_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        let mut s = snap(Path::new(""), "scratch");
        s.untitled = Some("ab12".into());
        let at = write(&state, &root, &s, 5).unwrap();
        assert_eq!(at.file_name().unwrap(), "untitled-ab12.bak");
        let b = read(&state, &root, &at).unwrap();
        assert_eq!(b.key(), Key::Untitled("ab12".into()));
        assert_eq!(b.text.to_string(), "scratch");
        assert_eq!(list(&state, &root).len(), 1, "no file on disk to compare with");
        // Under another id's name it is corrupt.
        let wrong = key_file(&state, &root, &Key::Untitled("cd34".into()));
        std::fs::copy(&at, &wrong).unwrap();
        assert!(read(&state, &root, &wrong).is_err());
        std::fs::remove_file(&wrong).unwrap();
        // Kept or removed by key.
        let keep = HashSet::from([Key::Untitled("ab12".into())]);
        assert_eq!(remove_all(&state, &root, &keep).unwrap(), 0);
        remove(&state, &root, &Key::Untitled("ab12".into())).unwrap();
        assert!(list(&state, &root).is_empty());
    }

    #[test]
    fn a_backup_the_file_already_holds_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a.txt");
        std::fs::write(&path, b"\xEF\xBB\xBFsame").unwrap();
        let at = write(&state, &root, &snap(&path, "same"), 1).unwrap();
        assert!(list(&state, &root).is_empty());
        assert!(!at.exists());
    }
}
