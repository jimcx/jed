//! Tool logs on disk: `<state dir>/logs/tools/<tool>.log`, shared by every jed process (each
//! line says which: time, pid, workspace root), rotated by size: a batch that would take the file
//! past [`MAX_FILE`] first moves it to `<tool>.log.1` (`.1` → `.2` → `.3`, the old `.3`
//! removed). Each append (and its rotation) runs under an advisory lock on `<tool>.log.lock`, as
//! `recent.rs` does, so two windows never rotate the same file at once or lose a batch to a
//! rename. Every function here is blocking.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::Line;

/// A log file is rotated before it grows past this.
pub const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Rotated files kept: `.1` (newest) to `.3`.
pub const KEEP: usize = 3;

/// The file stem for a tool named `name`: ASCII letters, digits, `.`, `_` and `-` kept, anything
/// else `_`, no leading dot (no hidden files, no `..`); `tool` when nothing is left.
pub fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_start_matches('.');
    if stem.is_empty() { "tool".into() } else { stem.into() }
}

/// `<dir>/<stem>.log`.
pub fn file(dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{stem}.log"))
}

/// `<dir>/<stem>.log.<n>` (`n` ≥ 1).
pub fn rotated(dir: &Path, stem: &str, n: usize) -> PathBuf {
    dir.join(format!("{stem}.log.{n}"))
}

/// `line` as the file holds it: `2026-10-18 14:03:09.123 UTC [4242 /home/me/proj] INFO  text\n`.
pub fn format_line(line: &Line, root: &str) -> String {
    let label = crate::history::utc_label(line.time);
    let (date, _) = label.split_once(" UTC").unwrap_or((&label, ""));
    format!(
        "{date}.{:03} UTC [{} {root}] {:<5} {}\n",
        line.time % 1000,
        std::process::id(),
        line.level.label(),
        line.text
    )
}

/// Appends `bytes` (whole lines) to `<dir>/<stem>.log`, rotating it first when it would grow
/// past `max` (unless it's empty: one batch bigger than `max` is written whole).
/// Blocking: call from spawn_blocking.
pub fn append(dir: &Path, stem: &str, bytes: &[u8], max: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{stem}.log.lock")))?;
    // Some file systems can't lock: carry on (a race then may lose a batch to a rotation).
    let locked = lock.lock().is_ok();
    let path = file(dir, stem);
    let len = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e),
    };
    if len > 0 && len + bytes.len() as u64 > max {
        rotate(dir, stem)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    f.write_all(bytes)?;
    drop(f);
    if locked {
        let _ = lock.unlock();
    }
    Ok(())
}

/// `.log` → `.log.1`, older ones shifted up to `.KEEP`, the oldest removed.
fn rotate(dir: &Path, stem: &str) -> std::io::Result<()> {
    let ignore_missing = |r: std::io::Result<()>| match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    };
    ignore_missing(std::fs::remove_file(rotated(dir, stem, KEEP)))?;
    for n in (1..KEEP).rev() {
        ignore_missing(std::fs::rename(rotated(dir, stem, n), rotated(dir, stem, n + 1)))?;
    }
    std::fs::rename(file(dir, stem), rotated(dir, stem, 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::Level;

    #[test]
    fn file_stems() {
        assert_eq!(file_stem("rust-analyzer"), "rust-analyzer");
        assert_eq!(file_stem("clangd"), "clangd");
        assert_eq!(file_stem("my server"), "my_server");
        assert_eq!(file_stem("../etc/passwd"), "_etc_passwd");
        assert_eq!(file_stem("a/b\\c:d"), "a_b_c_d");
        assert_eq!(file_stem("..."), "tool");
        assert_eq!(file_stem(""), "tool");
        assert_eq!(file_stem("ty.exe"), "ty.exe");
        assert_eq!(file_stem("é"), "_");
        let dir = Path::new("/s/logs/tools");
        assert_eq!(file(dir, "clangd"), Path::new("/s/logs/tools/clangd.log"));
        assert_eq!(rotated(dir, "clangd", 2), Path::new("/s/logs/tools/clangd.log.2"));
    }

    #[test]
    fn line_format() {
        // 2026-10-18 14:03:09.123 UTC
        let time = 1_792_332_189_123;
        let line = Line {
            time,
            level: Level::Warn,
            text: "careful".into(),
        };
        let pid = std::process::id();
        assert_eq!(
            format_line(&line, "/home/me/proj"),
            format!("2026-10-18 14:03:09.123 UTC [{pid} /home/me/proj] WARN  careful\n")
        );
    }

    fn read(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    #[test]
    fn rotation_shifts_and_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // 10-byte batches, 25-byte files: the third batch rotates.
        for i in 0..12 {
            append(d, "srv", format!("batch {i:02}\n").as_bytes(), 25).unwrap();
        }
        assert_eq!(read(&file(d, "srv")).unwrap(), "batch 10\nbatch 11\n");
        assert_eq!(read(&rotated(d, "srv", 1)).unwrap(), "batch 08\nbatch 09\n");
        assert_eq!(read(&rotated(d, "srv", 2)).unwrap(), "batch 06\nbatch 07\n");
        assert_eq!(read(&rotated(d, "srv", 3)).unwrap(), "batch 04\nbatch 05\n");
        assert_eq!(read(&rotated(d, "srv", 4)), None, "the oldest removed");
    }

    #[test]
    fn rotation_with_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(file(d, "srv"), "old\n").unwrap();
        std::fs::write(rotated(d, "srv", 2), "older\n").unwrap();
        append(d, "srv", b"new\n", 5).unwrap();
        assert_eq!(read(&file(d, "srv")).unwrap(), "new\n");
        assert_eq!(read(&rotated(d, "srv", 1)).unwrap(), "old\n");
        assert_eq!(read(&rotated(d, "srv", 2)), None);
        assert_eq!(read(&rotated(d, "srv", 3)).unwrap(), "older\n");
        // A batch bigger than the cap into an empty file: written whole, nothing rotated.
        let big = "x".repeat(20) + "\n";
        append(d, "big", big.as_bytes(), 5).unwrap();
        assert_eq!(read(&file(d, "big")).unwrap(), big);
        assert_eq!(read(&rotated(d, "big", 1)), None);
    }

    /// Several writers (as several jed windows) appending and rotating one file: no line is
    /// lost or torn while nothing is old enough to be removed.
    #[test]
    fn concurrent_writers_share_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        let (writers, batches) = (4, 50);
        // Each batch is 3 lines of 16 bytes; 200 batches = 9600 bytes over files of ≤ 3000: the
        // file rotates about 3 times, so `.1`–`.3` and the file hold everything.
        let handles: Vec<_> = (0..writers)
            .map(|w| {
                let d = d.clone();
                std::thread::spawn(move || {
                    for b in 0..batches {
                        let text: String = (0..3).map(|l| format!("w{w} b{b:03} l{l} ....\n")).collect();
                        append(&d, "shared", text.as_bytes(), 3000).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let mut lines = Vec::new();
        for path in [
            rotated(&d, "shared", 3),
            rotated(&d, "shared", 2),
            rotated(&d, "shared", 1),
            file(&d, "shared"),
        ] {
            let text = read(&path).unwrap_or_default();
            assert!(std::fs::metadata(&path).map_or(0, |m| m.len()) <= 3000);
            lines.extend(text.lines().map(String::from));
        }
        assert_eq!(lines.len(), writers * batches * 3);
        for l in &lines {
            assert_eq!(l.len(), 15, "torn line {l:?}");
        }
        // Each writer's lines in order across the files (oldest file first).
        for w in 0..writers {
            let mine: Vec<_> = lines.iter().filter(|l| l.starts_with(&format!("w{w} "))).collect();
            let expected: Vec<String> = (0..batches)
                .flat_map(|b| (0..3).map(move |l| format!("w{w} b{b:03} l{l} ....")))
                .collect();
            assert_eq!(mine, expected.iter().collect::<Vec<_>>());
        }
    }
}
