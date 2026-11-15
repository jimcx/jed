//! What is compiled into the binary, zstd-packed by `build.rs` (`bundle.rs`): the bundled
//! dictionary (`assets/dictionaries/en_AU/`, with its licence) and `NOTICES.md`. They are
//! unpacked on first use ([`dictionary_files`], [`notices`]) and written out to
//! `<data dir>/` by [`sync`] when the stamp file there doesn't name this build's bundle (so the
//! licence travels with the dictionary on disk).

use crate::buffer::file::{Stamp, atomic_write};
use crate::bundle::unpack;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

static DICTIONARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/en_AU.zst"));
static NOTICES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/notices.zst"));

/// The language the bundled dictionary is for (the folder it is written to).
pub const LANGUAGE: &str = "en_AU";

/// The bundled dictionary's files (`en_AU.aff`, `en_AU.dic`, `README_en_AU.txt`), unpacked once.
pub fn dictionary_files() -> &'static [(String, Vec<u8>)] {
    static FILES: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();
    FILES.get_or_init(|| unpack(DICTIONARY).expect("bundled dictionary unpacks"))
}

/// The text of one bundled dictionary file.
pub fn dictionary_text(name: &str) -> &'static str {
    dictionary_files()
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, data)| std::str::from_utf8(data).ok())
        .unwrap_or_else(|| panic!("bundled dictionary has {name}"))
}

/// `NOTICES.md`.
pub fn notices() -> String {
    let files = unpack(NOTICES).expect("bundled notices unpack");
    String::from_utf8_lossy(&files[0].1).into_owned()
}

/// The About page: the version, then the notices.
pub fn about() -> String {
    format!("# jed {}\n\n{}", env!("JED_VERSION"), notices())
}

/// Names the packed bundle that was last written out (`Stamp::encode`), next to the files.
const STAMP_FILE: &str = ".bundle-stamp";

/// Writes the bundled files under `dir` (`dictionaries/en_AU/*` and `NOTICES.md`) unless the
/// stamp file there already names this build's bundle (one small read, no unpacking: the check
/// costs startup nothing); then writes the stamp last, so an interrupted write is redone.
/// Returns how many files were written.
/// Blocking: call from spawn_blocking.
pub fn sync(dir: &Path) -> io::Result<usize> {
    let stamp = Stamp::of(&[DICTIONARY, NOTICES].concat());
    let stamp_path = dir.join(STAMP_FILE);
    if std::fs::read_to_string(&stamp_path).is_ok_and(|t| Stamp::parse(t.trim()) == Some(stamp)) {
        return Ok(0);
    }
    let notices = notices();
    let dict_dir = dir.join("dictionaries").join(LANGUAGE);
    let targets = dictionary_files()
        .iter()
        .map(|(name, data)| (dict_dir.join(name), data.as_slice()))
        .chain([(dir.join("NOTICES.md"), notices.as_bytes())]);
    let mut written = 0;
    for (path, data) in targets {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&path, data)?;
        written += 1;
    }
    atomic_write(&stamp_path, stamp.encode().as_bytes())?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_and_its_licence_are_bundled() {
        let names: Vec<_> = dictionary_files().iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["README_en_AU.txt", "en_AU.aff", "en_AU.dic"]);
        assert!(dictionary_text("en_AU.dic").len() > 100_000);
        assert!(notices().contains("Third-party notices"));
        assert!(about().starts_with("# jed "));
    }

    #[test]
    fn sync_writes_once_per_bundle_and_then_only_reads_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let files = dictionary_files().len() + 1;
        assert_eq!(sync(dir.path()).unwrap(), files);
        assert!(dir.path().join(STAMP_FILE).exists());
        assert_eq!(sync(dir.path()).unwrap(), 0);
        // The stamp is trusted: files are not compared.
        let aff = dir.path().join("dictionaries/en_AU/en_AU.aff");
        std::fs::write(&aff, "edited").unwrap();
        assert_eq!(sync(dir.path()).unwrap(), 0);
        assert_eq!(std::fs::read_to_string(&aff).unwrap(), "edited");
        // A stamp of another bundle (an older jed) or a damaged one: written again.
        std::fs::write(dir.path().join(STAMP_FILE), Stamp::of(b"old").encode()).unwrap();
        assert_eq!(sync(dir.path()).unwrap(), files);
        assert_eq!(std::fs::read_to_string(&aff).unwrap(), dictionary_text("en_AU.aff"));
        std::fs::write(dir.path().join(STAMP_FILE), "junk").unwrap();
        assert_eq!(sync(dir.path()).unwrap(), files);
    }
}
