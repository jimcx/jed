//! Loading and saving files: encoding checks, line endings, atomic writes.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ropey::Rope;
use sha2::{Digest, Sha256};

pub const BOM: &[u8] = b"\xEF\xBB\xBF";
/// Same heuristic as git: a NUL in the first 8000 bytes means binary.
const BINARY_SNIFF_LEN: usize = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }
}

/// What a file held, to tell whether it changed since red last read or wrote it: its length and
/// a SHA-256 of its bytes (BOM included). Compared, never ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    hash: [u8; 32],
}

impl Stamp {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            hash: Sha256::digest(bytes).into(),
        }
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub rope: Rope,
    pub bom: bool,
    /// The file's main line ending; Enter inserts this.
    pub eol: Eol,
    /// Set when the file can't be saved safely, with the reason.
    pub read_only: Option<String>,
    /// The bytes decoded; `None` when the file was missing (loaded empty).
    pub stamp: Option<Stamp>,
}

impl Loaded {
    /// What a reload takes from the file besides its text (`None` for a missing file).
    pub fn meta(&self) -> Option<DiskMeta> {
        Some(DiskMeta {
            stamp: self.stamp?,
            bom: self.bom,
            eol: self.eol,
            read_only: self.read_only.clone(),
        })
    }
}

/// A file's state besides its text, as a reload from disk sets it on the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskMeta {
    pub stamp: Stamp,
    pub bom: bool,
    pub eol: Eol,
    pub read_only: Option<String>,
}

#[derive(Debug)]
pub enum LoadError {
    Binary,
    Io(io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Binary => write!(f, "binary file, not opened"),
            LoadError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Reads a file. A missing file loads as empty and is created on save.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    match read(path).map_err(LoadError::Io)? {
        Some(bytes) => decode(bytes),
        None => {
            let mut loaded = decode(Vec::new())?;
            loaded.stamp = None;
            Ok(loaded)
        }
    }
}

/// A file's bytes, `None` when it doesn't exist. Blocking.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether a file's contents are binary (not opened, not searched): a NUL in the first 8000 bytes.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_LEN)].contains(&0)
}

pub fn decode(mut bytes: Vec<u8>) -> Result<Loaded, LoadError> {
    if looks_binary(&bytes) {
        return Err(LoadError::Binary);
    }
    let stamp = Some(Stamp::of(&bytes));
    let bom = bytes.starts_with(BOM);
    if bom {
        bytes.drain(..BOM.len());
    }
    let (text, read_only) = match String::from_utf8(bytes) {
        Ok(t) => (t, None),
        Err(e) => (
            String::from_utf8_lossy(e.as_bytes()).into_owned(),
            Some("not valid UTF-8".to_string()),
        ),
    };
    let eol = detect_eol(&text);
    Ok(Loaded {
        rope: Rope::from_str(&text),
        bom,
        eol,
        read_only,
        stamp,
    })
}

fn detect_eol(text: &str) -> Eol {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf { Eol::Crlf } else { Eol::Lf }
}

/// The bytes to write for a buffer: exactly its text, plus the BOM if it had one.
pub fn encode(rope: &Rope, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(rope.len() + BOM.len());
    if bom {
        out.extend_from_slice(BOM);
    }
    for chunk in rope.chunks() {
        out.extend_from_slice(chunk.as_bytes());
    }
    out
}

/// Writes to a temp file in the same directory, fsyncs, then renames over the target.
/// Keeps the target's permissions and writes through symlinks.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_target(path)?;
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other("path has no file name"))?;
    let tmp = dir.join(format!(".{}.red-{}.tmp", name.to_string_lossy(), std::process::id()));

    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(&target) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    // Make the rename itself durable; not all filesystems support this.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_binary_bom_and_eol() {
        assert!(matches!(decode(b"ab\0c".to_vec()), Err(LoadError::Binary)));

        let l = decode(b"\xEF\xBB\xBFa\r\nb\r\nc\n".to_vec()).unwrap();
        assert!(l.bom);
        assert_eq!(l.eol, Eol::Crlf);
        assert_eq!(l.rope.to_string(), "a\r\nb\r\nc\n");
        assert!(l.read_only.is_none());

        let l = decode(b"a\xFFb".to_vec()).unwrap();
        assert!(l.read_only.is_some());
    }

    #[test]
    fn stamps_tell_contents_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        assert_eq!(load(&path).unwrap().stamp, None, "missing");
        fs::write(&path, "\u{FEFF}one\n").unwrap();
        let l = load(&path).unwrap();
        assert_eq!(
            l.stamp,
            Some(Stamp::of("\u{FEFF}one\n".as_bytes())),
            "the bytes, BOM included"
        );
        assert_ne!(l.stamp, Some(Stamp::of(b"one\n")));
        assert_eq!(
            l.stamp,
            Some(Stamp::of(&encode(&l.rope, l.bom))),
            "what a save writes back"
        );
    }

    #[test]
    fn round_trip_is_byte_identical() {
        let input = b"\xEF\xBB\xBFone\r\ntwo\nthree\r".to_vec();
        let l = decode(input.clone()).unwrap();
        assert_eq!(encode(&l.rope, l.bom), input);
    }

    #[test]
    fn atomic_write_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        atomic_write(&link, b"new").unwrap();

        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp file left");
    }
}
