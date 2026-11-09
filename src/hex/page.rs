//! The hex view's byte source and page cache (PLAN.md hex_more): a file's bytes read either whole
//! (`Source::Resident`, files up to `RESIDENT_MAX`, as the hex tab always did) or through
//! positioned reads (`Source::Paged`, `FileExt::read_at` on one open `File`; no `mmap`: a file
//! truncated under it would crash the process instead of failing a read).
//!
//! A frame never reads a paged file: it asks the `PageCache` (plain data, the UI thread's) for its
//! visible bytes (`copy_range`), draws what is missing as a placeholder and queues the missing
//! pages; `read_pages` (blocking, from `spawn_blocking`) reads them and the UI thread `insert`s
//! the results. Pages are `PAGE` bytes (the last one shorter), evicted least recently used first
//! once the cache holds more than its byte cap.
//!
//! What cached pages are keyed by is a `Version`: a resident file's `Stamp` (a SHA-256 of its
//! bytes, free once they are read), a paged one's metadata (length, mtime, inode, device):
//! hashing gigabytes at open and at every disk check would take far too long, and the cost of a
//! missed same-length same-mtime rewrite is a stale page until the file is next touched. A page
//! read for one version is never shown for another: `PageCache::set_version` drops them all.

// Unused until the hex tab pages its file (hex_more-b).
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::ops::Range;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;
use std::sync::Arc;

use crate::buffer::file::{self, Stamp};

/// Bytes per page.
pub const PAGE: u64 = 64 << 10;
/// The cache's byte cap (the tab's splits share one).
pub const CACHE_MAX: usize = 64 << 20;
/// A file this size or smaller is read whole at open.
pub const RESIDENT_MAX: u64 = 8 << 20;

/// What a file's bytes were read from, to tell later that they changed (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    /// A resident file's content hash.
    Hash(Stamp),
    /// A paged file's metadata.
    Meta {
        len: u64,
        mtime: i64,
        mtime_nsec: i64,
        ino: u64,
        dev: u64,
    },
}

impl Version {
    fn of_meta(m: &std::fs::Metadata) -> Self {
        Self::Meta {
            len: m.len(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
            ino: m.ino(),
            dev: m.dev(),
        }
    }
}

/// One open file read by position.
#[derive(Debug)]
pub struct PagedFile {
    file: File,
    version: Version,
    len: u64,
}

/// A hex tab's bytes: shared (cloning is an `Arc` clone: a tab and its splits share one).
#[derive(Debug, Clone)]
pub enum Source {
    Resident { bytes: Arc<[u8]>, version: Version },
    Paged(Arc<PagedFile>),
}

impl Source {
    /// A resident source over `bytes`; hashes them for the version (blocking for big ones).
    pub fn resident(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        let version = Version::Hash(Stamp::of(&bytes));
        Self::Resident { bytes, version }
    }

    pub fn len(&self) -> u64 {
        match self {
            Self::Resident { bytes, .. } => bytes.len() as u64,
            Self::Paged(p) => p.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn version(&self) -> Version {
        match self {
            Self::Resident { version, .. } => *version,
            Self::Paged(p) => p.version,
        }
    }

    /// The whole file when it is resident.
    pub fn resident_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Resident { bytes, .. } => Some(bytes),
            Self::Paged(_) => None,
        }
    }

    /// Reads up to `buf.len()` bytes at `offset` (fewer only at the end of the file; 0 at or past
    /// it). Blocking for a paged source: call from spawn_blocking.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Resident { bytes, .. } => {
                let start = offset.min(bytes.len() as u64) as usize;
                let n = buf.len().min(bytes.len() - start);
                buf[..n].copy_from_slice(&bytes[start..start + n]);
                Ok(n)
            }
            Self::Paged(p) => {
                let mut done = 0;
                while done < buf.len() {
                    match p.file.read_at(&mut buf[done..], offset + done as u64) {
                        Ok(0) => break,
                        Ok(n) => done += n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(done)
            }
        }
    }

    /// Fills `buf` from `offset`; `UnexpectedEof` when the file ends first (it shrank since open).
    /// Blocking for a paged source: call from spawn_blocking.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        match self {
            Self::Paged(p) => p.file.read_exact_at(buf, offset),
            Self::Resident { .. } if self.read_at(offset, buf)? == buf.len() => Ok(()),
            Self::Resident { .. } => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "file shorter than when opened",
            )),
        }
    }

    /// The version `path` has now, of the same kind as this source's (a hash of its bytes for a
    /// resident source, its metadata for a paged one): `Ok(None)` when it's missing; differs from
    /// `version()` when the file changed.
    /// Blocking: call from spawn_blocking.
    pub fn current_version(&self, path: &Path) -> io::Result<Option<Version>> {
        match self {
            Self::Resident { .. } => Ok(file::read(path)?.map(|b| Version::Hash(Stamp::of(&b)))),
            Self::Paged(_) => match std::fs::metadata(path) {
                Ok(m) => Ok(Some(Version::of_meta(&m))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
        }
    }
}

/// Opens `path` for a hex tab: resident when it is at most `RESIDENT_MAX` bytes, else paged (just
/// opened, nothing read); `Ok(None)` when it's missing.
/// Blocking: call from spawn_blocking.
pub fn open(path: &Path) -> io::Result<Option<Source>> {
    open_with(path, RESIDENT_MAX)
}

/// `open` with the resident limit a parameter (tests use small ones).
/// Blocking: call from spawn_blocking.
pub fn open_with(path: &Path, resident_max: u64) -> io::Result<Option<Source>> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.len() <= resident_max {
        return Ok(file::read(path)?.map(Source::resident));
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // The open file's own metadata: the version is of the file the reads will see.
    let meta = file.metadata()?;
    Ok(Some(Source::Paged(Arc::new(PagedFile {
        file,
        version: Version::of_meta(&meta),
        len: meta.len(),
    }))))
}

/// The pages holding the bytes in `range`.
pub fn pages_for(range: Range<u64>) -> Range<u64> {
    if range.is_empty() {
        return 0..0;
    }
    range.start / PAGE..range.end.div_ceil(PAGE)
}

/// The byte offsets page `page` covers in a `len`-byte file (empty past the end).
pub fn page_bytes(page: u64, len: u64) -> Range<u64> {
    let start = page.saturating_mul(PAGE).min(len);
    start..start.saturating_add(PAGE).min(len)
}

/// Reads `pages` of `src`, each as its own result (a page past the end, or cut short because the
/// file shrank under us, is an error: the caller retries after the next disk check).
/// Blocking: call from spawn_blocking.
pub fn read_pages(src: &Source, pages: impl IntoIterator<Item = u64>) -> Vec<(u64, io::Result<Arc<[u8]>>)> {
    pages
        .into_iter()
        .map(|page| {
            let r = page_bytes(page, src.len());
            let result = if r.is_empty() {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "page past the end of the file",
                ))
            } else {
                let mut buf = vec![0u8; (r.end - r.start) as usize];
                src.read_exact_at(r.start, &mut buf).map(|()| Arc::from(buf))
            };
            (page, result)
        })
        .collect()
}

/// Pages read so far, byte-capped, least recently used out first. Holds one version's pages.
#[derive(Debug)]
pub struct PageCache {
    version: Version,
    pages: HashMap<u64, (Arc<[u8]>, u64)>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl PageCache {
    pub fn new(version: Version) -> Self {
        Self::with_cap(version, CACHE_MAX)
    }

    pub fn with_cap(version: Version, cap: usize) -> Self {
        Self {
            version,
            pages: HashMap::new(),
            bytes: 0,
            cap,
            tick: 0,
        }
    }

    pub fn version(&self) -> Version {
        self.version
    }

    /// Bytes held now (never more than the cap, unless one page alone is bigger).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.bytes = 0;
    }

    /// Switches to `version`, dropping every page when it differs.
    pub fn set_version(&mut self, version: Version) {
        if version != self.version {
            self.clear();
            self.version = version;
        }
    }

    /// Page `page` of `version`, marked as just used.
    pub fn get(&mut self, version: Version, page: u64) -> Option<Arc<[u8]>> {
        if version != self.version {
            return None;
        }
        self.tick += 1;
        let tick = self.tick;
        let (bytes, used) = self.pages.get_mut(&page)?;
        *used = tick;
        Some(bytes.clone())
    }

    pub fn contains(&self, version: Version, page: u64) -> bool {
        version == self.version && self.pages.contains_key(&page)
    }

    /// Adds a page read for `version`; dropped when that isn't the cache's version (a stale
    /// read). Evicts least recently used pages while over the cap (never the one just added).
    pub fn insert(&mut self, version: Version, page: u64, bytes: Arc<[u8]>) {
        if version != self.version {
            return;
        }
        self.tick += 1;
        self.bytes += bytes.len();
        if let Some((old, _)) = self.pages.insert(page, (bytes, self.tick)) {
            self.bytes -= old.len();
        }
        while self.bytes > self.cap && self.pages.len() > 1 {
            let oldest = self.pages.iter().min_by_key(|(_, (_, used))| *used).map(|(&p, _)| p);
            let Some(oldest) = oldest.filter(|&p| p != page) else {
                break;
            };
            if let Some((b, _)) = self.pages.remove(&oldest) {
                self.bytes -= b.len();
            }
        }
    }

    /// Copies the cached bytes from `offset` into `out` (the caller clips to the file's length):
    /// returns the pages that were missing (ascending), whose bytes of `out` are left untouched.
    pub fn copy_range(&mut self, version: Version, offset: u64, out: &mut [u8]) -> Vec<u64> {
        let end = offset + out.len() as u64;
        let mut missing = Vec::new();
        for page in pages_for(offset..end) {
            let Some(bytes) = self.get(version, page) else {
                missing.push(page);
                continue;
            };
            let base = page * PAGE;
            let from = offset.max(base);
            let to = end.min(base + bytes.len() as u64);
            if from < to {
                out[(from - offset) as usize..(to - offset) as usize]
                    .copy_from_slice(&bytes[(from - base) as usize..(to - base) as usize]);
            }
        }
        missing
    }

    /// The byte at `offset` when its page is cached.
    pub fn byte(&mut self, version: Version, offset: u64) -> Option<u8> {
        let page = self.get(version, offset / PAGE)?;
        page.get((offset % PAGE) as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn v(n: u64) -> Version {
        Version::Meta {
            len: n,
            mtime: 0,
            mtime_nsec: 0,
            ino: 1,
            dev: 1,
        }
    }

    fn page(n: usize, fill: u8) -> Arc<[u8]> {
        vec![fill; n].into()
    }

    #[test]
    fn pages_cover_ranges() {
        assert_eq!(pages_for(0..0), 0..0);
        assert_eq!(pages_for(0..1), 0..1);
        assert_eq!(pages_for(0..PAGE), 0..1);
        assert_eq!(pages_for(PAGE - 1..PAGE + 1), 0..2);
        assert_eq!(pages_for(5 * PAGE..6 * PAGE + 1), 5..7);
        assert_eq!(page_bytes(1, PAGE + 10), PAGE..PAGE + 10, "short last page");
        assert!(page_bytes(2, PAGE + 10).is_empty());
    }

    #[test]
    fn cache_evicts_least_recently_used_over_the_cap() {
        let mut c = PageCache::with_cap(v(1), 300);
        for p in 0..3 {
            c.insert(v(1), p, page(100, p as u8));
        }
        assert_eq!((c.len(), c.bytes()), (3, 300));
        assert!(c.get(v(1), 0).is_some(), "touch 0: now 1 is the oldest");
        c.insert(v(1), 3, page(100, 3));
        assert_eq!((c.len(), c.bytes()), (3, 300));
        assert!(!c.contains(v(1), 1));
        assert!(c.contains(v(1), 0) && c.contains(v(1), 2) && c.contains(v(1), 3));
        // A big page evicts several; replacing a page doesn't double count it.
        c.insert(v(1), 9, page(250, 9));
        assert!(c.contains(v(1), 9));
        assert!(c.bytes() <= 300);
        c.insert(v(1), 9, page(10, 9));
        assert!(c.bytes() <= 110);
        // One page over the cap alone stays.
        let mut d = PageCache::with_cap(v(1), 10);
        d.insert(v(1), 0, page(50, 0));
        assert_eq!((d.len(), d.bytes()), (1, 50));
    }

    #[test]
    fn cache_versions_never_mix() {
        let mut c = PageCache::with_cap(v(1), 1000);
        c.insert(v(1), 0, page(10, 1));
        c.insert(v(2), 1, page(10, 2));
        assert_eq!(c.len(), 1, "a stale read is dropped");
        assert!(c.get(v(2), 0).is_none(), "and asking for another version finds nothing");
        c.set_version(v(1));
        assert_eq!(c.len(), 1);
        c.set_version(v(2));
        assert_eq!((c.len(), c.bytes(), c.version()), (0, 0, v(2)));
        c.insert(v(2), 1, page(10, 2));
        c.clear();
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn copy_range_reports_missing_pages_and_short_last_page() {
        let mut c = PageCache::with_cap(v(1), 1 << 20);
        let len = 2 * PAGE + 100;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        for p in [0, 2] {
            let r = page_bytes(p, len);
            c.insert(v(1), p, data[r.start as usize..r.end as usize].into());
        }
        let mut out = vec![0xee; 300];
        let missing = c.copy_range(v(1), PAGE - 100, &mut out);
        assert_eq!(missing, vec![1]);
        assert_eq!(&out[..100], &data[PAGE as usize - 100..PAGE as usize]);
        assert!(out[100..].iter().all(|&b| b == 0xee), "missing bytes untouched");
        let mut out = vec![0; 150];
        assert!(c.copy_range(v(1), 2 * PAGE - 50, &mut out).contains(&1));
        assert_eq!(&out[50..], &data[2 * PAGE as usize..]);
        assert_eq!(c.byte(v(1), 2 * PAGE + 99), Some(data[len as usize - 1]));
        assert_eq!(c.byte(v(1), 2 * PAGE + 100), None);
        assert_eq!(c.byte(v(1), PAGE), None);
    }

    #[test]
    fn open_picks_resident_or_paged_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, vec![7u8; 1000]).unwrap();
        let s = open_with(&p, 1000).unwrap().unwrap();
        assert_eq!(s.resident_bytes().map(<[u8]>::len), Some(1000));
        assert!(matches!(s.version(), Version::Hash(_)));
        let s = open_with(&p, 999).unwrap().unwrap();
        assert!(s.resident_bytes().is_none());
        assert_eq!(s.len(), 1000);
        assert!(matches!(s.version(), Version::Meta { len: 1000, .. }));
        assert_eq!(s.current_version(&p).unwrap(), Some(s.version()));
        std::fs::write(&p, vec![8u8; 1001]).unwrap();
        assert_ne!(s.current_version(&p).unwrap(), Some(s.version()));
        std::fs::remove_file(&p).unwrap();
        assert_eq!(s.current_version(&p).unwrap(), None);
        assert!(open(&p).unwrap().is_none());
        // The default limit: 8 MiB is resident, one byte more is paged (sparse: no disk cost).
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(RESIDENT_MAX).unwrap();
        assert!(open(&p).unwrap().unwrap().resident_bytes().is_some());
        f.set_len(RESIDENT_MAX + 1).unwrap();
        assert!(open(&p).unwrap().unwrap().resident_bytes().is_none());
    }

    #[test]
    fn resident_and_paged_read_alike() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let data: Vec<u8> = (0..3 * PAGE as usize + 17).map(|i| (i * 7 % 256) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let paged = open_with(&p, 0).unwrap().unwrap();
        let res = Source::resident(data.clone());
        for (off, n) in [
            (0u64, 10usize),
            (PAGE - 3, 9),
            (data.len() as u64 - 5, 20),
            (data.len() as u64, 4),
        ] {
            let (mut a, mut b) = (vec![0u8; n], vec![0u8; n]);
            let (x, y) = (paged.read_at(off, &mut a).unwrap(), res.read_at(off, &mut b).unwrap());
            assert_eq!((x, &a[..x]), (y, &b[..y]), "at {off}");
        }
        let read = read_pages(&paged, [0, 3, 4]);
        assert_eq!(&**read[0].1.as_ref().unwrap(), &data[..PAGE as usize]);
        assert_eq!(&**read[1].1.as_ref().unwrap(), &data[3 * PAGE as usize..]);
        assert!(read[2].1.is_err(), "past the end");
        // Truncated under us: the page is an error, not zeros.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(10)
            .unwrap();
        assert!(read_pages(&paged, [1])[0].1.is_err());
    }

    #[test]
    fn sparse_file_over_4_gib_reads_through_pages() {
        use std::os::unix::fs::FileExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        let len: u64 = (6 << 30) + 123;
        let mut f = std::fs::File::create(&p).unwrap();
        f.set_len(len).unwrap();
        let marks = [(5u64, b'a'), ((4 << 30) + PAGE + 7, b'b'), (len - 1, b'c')];
        for (off, b) in marks {
            f.write_all_at(&[b], off).unwrap();
        }
        f.flush().unwrap();
        let s = open(&p).unwrap().unwrap();
        assert_eq!(s.len(), len);
        let mut cache = PageCache::new(s.version());
        let wanted: Vec<u64> = marks.iter().map(|&(o, _)| o / PAGE).collect();
        for (page, r) in read_pages(&s, wanted) {
            cache.insert(s.version(), page, r.unwrap());
        }
        assert!(cache.bytes() <= 3 * PAGE as usize);
        for (off, b) in marks {
            assert_eq!(cache.byte(s.version(), off), Some(b), "at {off}");
        }
        let mut one = [0u8; 1];
        assert_eq!(s.read_at(len - 1, &mut one).unwrap(), 1);
        assert_eq!(one[0], b'c');
        assert_eq!(s.read_at(len, &mut one).unwrap(), 0);
    }

    #[test]
    fn copy_range_at_page_edges() {
        let mut c = PageCache::with_cap(v(1), 1 << 20);
        let len = 2 * PAGE;
        let data: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
        for p in 0..2 {
            let r = page_bytes(p, len);
            c.insert(v(1), p, data[r.start as usize..r.end as usize].into());
        }
        // Exactly one page, exactly the file, one byte either side of the seam.
        for (off, n) in [
            (0u64, PAGE),
            (PAGE, PAGE),
            (0, len),
            (PAGE - 1, 2),
            (PAGE, 1),
            (len - 1, 1),
        ] {
            let mut out = vec![0xee; n as usize];
            assert!(c.copy_range(v(1), off, &mut out).is_empty(), "{off}+{n}");
            assert_eq!(&out[..], &data[off as usize..(off + n) as usize], "{off}+{n}");
        }
        let mut out = [0u8; 0];
        assert!(c.copy_range(v(1), 5, &mut out).is_empty());
        let mut out = [0u8; 4];
        assert_eq!(c.copy_range(v(2), 0, &mut out), vec![0], "another version: all missing");
    }
}
