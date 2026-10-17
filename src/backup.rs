//! Crash backups: dirty buffers' text kept under the state dir so a killed or crashed jed loses
//! at most one backup interval of edits (no app state; `app/backup.rs` schedules the writes,
//! the panic hook writes the same files synchronously, best effort).
//!
//! One folder per workspace, `<state dir>/backup/<32 hex of the root's SHA-256>/`, one file per
//! buffer, `<32 hex of the file path's SHA-256>.bak`, rewritten in place (atomically: a temp
//! file beside it, fsynced, renamed). A file is three parts:
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

/// A dirty buffer's text to back up (rope clones are cheap).
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Absolute.
    pub path: PathBuf,
    pub rope: Rope,
    pub bom: bool,
    /// The file as the buffer's edits know it (`Buffer::disk`; `None`: no file).
    pub disk: Option<Stamp>,
}

/// A backup read back.
#[derive(Debug, Clone)]
pub struct Backup {
    pub path: PathBuf,
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
    path: PathBuf,
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

/// The folder holding the workspace at `root`'s backups.
pub fn dir(state_dir: &Path, root: &Path) -> PathBuf {
    state_dir.join(DIR).join(name_hash(root))
}

/// The backup file of `path` in the workspace at `root`.
pub fn file(state_dir: &Path, root: &Path, path: &Path) -> PathBuf {
    dir(state_dir, root).join(format!("{}.{EXT}", name_hash(path)))
}

/// Writes (replaces) `snap`'s backup, atomically; returns the file written.
/// Blocking: call from spawn_blocking (the panic hook calls it directly).
pub fn write(state_dir: &Path, root: &Path, snap: &Snapshot, time: u64) -> std::io::Result<PathBuf> {
    let dest = file(state_dir, root, &snap.path);
    let folder = dest.parent().expect("backups are in a folder");
    std::fs::create_dir_all(folder)?;
    let header = Header {
        root: root.to_path_buf(),
        path: snap.path.clone(),
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

/// Removes `path`'s backup (none is fine).
/// Blocking: call from spawn_blocking.
pub fn remove(state_dir: &Path, root: &Path, path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file(state_dir, root, path)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes every backup of the workspace (corrupt ones and leftover temp files too) except
/// those of the files in `keep`; the folder too when it's left empty. Returns how many went.
/// Blocking: call from spawn_blocking.
pub fn remove_all(state_dir: &Path, root: &Path, keep: &HashSet<PathBuf>) -> std::io::Result<usize> {
    let folder = dir(state_dir, root);
    let entries = match std::fs::read_dir(&folder) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let kept: HashSet<PathBuf> = keep.iter().map(|p| file(state_dir, root, p)).collect();
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
    if file(state_dir, root, &header.path) != at {
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
        let on_disk = file::read(&backup.path).ok().flatten();
        if on_disk.is_some_and(|bytes| bytes == file::encode(&backup.text, backup.bom)) {
            tracing::info!("backup of {} dropped: the file holds its text", backup.path.display());
            let _ = std::fs::remove_file(&at);
            continue;
        }
        found.push(backup);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(path: &Path, text: &str) -> Snapshot {
        Snapshot {
            path: path.to_path_buf(),
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
        remove(&state, &root, &path).unwrap();
        remove(&state, &root, &path).unwrap();
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
        let keep = HashSet::from([root.join("good")]);
        assert_eq!(remove_all(&state, &root, &keep).unwrap(), 5);
        assert_eq!(list(&state, &root).len(), 1);
        assert_eq!(remove_all(&state, &root, &HashSet::new()).unwrap(), 1);
        assert!(!super::dir(&state, &root).exists());
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
