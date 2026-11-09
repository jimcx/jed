//! Streaming byte search over a `Source` (PLAN.md hex_more): `stream_next` / `stream_prev` /
//! `stream_count` give exactly what `find_next` / `find_prev` / `count` give over the same bytes in
//! memory (they are the reference the tests compare with), but read the file in chunks of
//! `chunk` start positions (each read `query.len() - 1` bytes longer, so a match split by a chunk's
//! end is found in the chunk it starts in, once), so a multi-GB file needs one chunk of memory.
//!
//! Every read is blocking: run these from `spawn_blocking`. A `Cancel` is checked before each chunk:
//! a newer query or keystroke bumps the shared generation and the old job ends with
//! `StreamError::Cancelled` instead of a wrong answer. A failed read is `StreamError::Io`.

// Unused until the hex tab streams its finds (hex_more-b).
#![allow(dead_code)]

use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::page::Source;
use super::{Count, Query, Searcher};

/// Start positions searched per read for a paged file.
pub const STREAM_CHUNK: usize = 1 << 20;

/// Cancels a streaming search when the shared generation moves off the one the job started with.
#[derive(Debug, Clone)]
pub struct Cancel {
    current: Option<(Arc<AtomicU64>, u64)>,
}

impl Cancel {
    /// Never cancelled.
    pub fn never() -> Self {
        Self { current: None }
    }

    /// Cancelled once `generation` is no longer `mine` (the job takes `mine` from it when queued;
    /// the next query stores a newer one).
    pub fn new(generation: Arc<AtomicU64>, mine: u64) -> Self {
        Self {
            current: Some((generation, mine)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(g, mine)| g.load(Ordering::Relaxed) != *mine)
    }
}

#[derive(Debug)]
pub enum StreamError {
    Cancelled,
    Io(io::Error),
}

impl From<io::Error> for StreamError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

type Found<T> = Result<T, StreamError>;

/// Reads `len` bytes at `at` into `buf` (resized), which must all be there.
fn fill(src: &Source, at: u64, len: usize, buf: &mut Vec<u8>) -> io::Result<()> {
    buf.resize(len, 0);
    src.read_exact_at(at, buf)
}

/// Bytes to read for the start positions `lo..hi`: through the last match's end, within the file.
fn span(src: &Source, s: &Searcher, lo: u64, hi: u64) -> usize {
    (hi - lo + s.len as u64 - 1).min(src.len() - lo) as usize
}

/// Calls `f` with every match start in `starts` in order until it returns false (`Ok(false)`).
fn each(
    src: &Source,
    s: &Searcher,
    starts: Range<u64>,
    chunk: usize,
    cancel: &Cancel,
    mut f: impl FnMut(u64) -> bool,
) -> Found<bool> {
    let end = starts.end.min(src.len());
    let chunk = chunk.max(1) as u64;
    let mut buf = Vec::new();
    let mut lo = starts.start;
    while lo < end {
        if cancel.is_cancelled() {
            return Err(StreamError::Cancelled);
        }
        let hi = (lo + chunk).min(end);
        fill(src, lo, span(src, s, lo, hi), &mut buf)?;
        let mut more = true;
        s.each(&buf, 0..(hi - lo) as usize, |m| {
            more = f(lo + m as u64);
            more
        });
        if !more {
            return Ok(false);
        }
        lo = hi;
    }
    Ok(true)
}

/// The first match start in `starts`.
fn first(src: &Source, s: &Searcher, starts: Range<u64>, chunk: usize, cancel: &Cancel) -> Found<Option<u64>> {
    let mut found = None;
    each(src, s, starts, chunk, cancel, |m| {
        found = Some(m);
        false
    })?;
    Ok(found)
}

/// The last match start in `starts`, searching from its end.
fn last(src: &Source, s: &Searcher, starts: Range<u64>, chunk: usize, cancel: &Cancel) -> Found<Option<u64>> {
    let end = starts.end.min(src.len());
    let chunk = chunk.max(1) as u64;
    let mut buf = Vec::new();
    let mut hi = end;
    while hi > starts.start {
        if cancel.is_cancelled() {
            return Err(StreamError::Cancelled);
        }
        let lo = hi.saturating_sub(chunk).max(starts.start);
        fill(src, lo, span(src, s, lo, hi), &mut buf)?;
        if let Some(i) = s.last(&buf, 0..(hi - lo) as usize) {
            return Ok(Some(lo + i as u64));
        }
        hi = lo;
    }
    Ok(None)
}

/// As `find_next`: the first match starting at or after `from`, else (wrapping) the first one
/// before it.
/// Blocking: call from spawn_blocking.
pub fn stream_next(src: &Source, q: &Query, from: u64, chunk: usize, cancel: &Cancel) -> Found<Option<Range<u64>>> {
    let Some(s) = q.searcher() else { return Ok(None) };
    let len = src.len();
    let from = from.min(len);
    let start = match first(src, &s, from..len, chunk, cancel)? {
        Some(m) => Some(m),
        None => first(src, &s, 0..from, chunk, cancel)?,
    };
    Ok(start.map(|m| m..m + q.len() as u64))
}

/// As `find_prev`: the last match starting before `before`, else (wrapping) the last one at or
/// after it.
/// Blocking: call from spawn_blocking.
pub fn stream_prev(src: &Source, q: &Query, before: u64, chunk: usize, cancel: &Cancel) -> Found<Option<Range<u64>>> {
    let Some(s) = q.searcher() else { return Ok(None) };
    let len = src.len();
    let before = before.min(len);
    let start = match last(src, &s, 0..before, chunk, cancel)? {
        Some(m) => Some(m),
        None => last(src, &s, before..len, chunk, cancel)?,
    };
    Ok(start.map(|m| m..m + q.len() as u64))
}

/// As `count`: every match's start up to `limit`.
/// Blocking: call from spawn_blocking (a whole-file scan).
pub fn stream_count(src: &Source, q: &Query, limit: usize, chunk: usize, cancel: &Cancel) -> Found<Count> {
    let mut c = Count::default();
    let Some(s) = q.searcher() else { return Ok(c) };
    if limit == 0 {
        c.capped = first(src, &s, 0..src.len(), chunk, cancel)?.is_some();
        return Ok(c);
    }
    each(src, &s, 0..src.len(), chunk, cancel, |m| {
        c.starts.push(m as usize);
        if c.starts.len() == limit {
            c.capped = true;
            return false;
        }
        true
    })?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::super::{count, find_next, find_prev};
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn never() -> Cancel {
        Cancel::never()
    }

    fn ascii(text: &str, case_sensitive: bool) -> Query {
        Query::Ascii {
            text: text.to_string(),
            case_sensitive,
        }
    }

    fn range(r: Option<Range<usize>>) -> Option<Range<u64>> {
        r.map(|r| r.start as u64..r.end as u64)
    }

    #[test]
    fn streaming_matches_the_in_memory_functions() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        let alphabet = b"abAB ";
        for _ in 0..400 {
            let n = rng.below(120);
            let k = 1 + rng.below(alphabet.len());
            let data: Vec<u8> = (0..n).map(|_| alphabet[rng.below(k)]).collect();
            let needle: Vec<u8> = (0..rng.below(5)).map(|_| alphabet[rng.below(k)]).collect();
            let q = match rng.below(3) {
                0 => Query::Hex(needle),
                1 => ascii(std::str::from_utf8(&needle).unwrap(), true),
                _ => ascii(std::str::from_utf8(&needle).unwrap(), false),
            };
            let src = Source::resident(data.clone());
            let chunk = 1 + rng.below(10);
            let limit = rng.below(6);
            let at = rng.below(n + 3);
            let msg = format!("{data:?} {q:?} chunk {chunk} at {at} limit {limit}");
            assert_eq!(
                stream_next(&src, &q, at as u64, chunk, &never()).unwrap(),
                range(find_next(&data, &q, at)),
                "next {msg}"
            );
            assert_eq!(
                stream_prev(&src, &q, at as u64, chunk, &never()).unwrap(),
                range(find_prev(&data, &q, at)),
                "prev {msg}"
            );
            assert_eq!(
                stream_count(&src, &q, limit, chunk, &never()).unwrap(),
                count(&data, &q, limit),
                "count {msg}"
            );
        }
    }

    #[test]
    fn matches_split_by_chunks_and_edges() {
        let data = b"xxabcabcabxx".to_vec();
        let src = Source::resident(data.clone());
        let q = ascii("abcab", true);
        for chunk in 1..=14 {
            assert_eq!(
                stream_count(&src, &q, 10, chunk, &never()).unwrap().starts,
                vec![2, 5],
                "overlapping matches, chunk {chunk}"
            );
            for at in [0, 2, 3, 5, 6, 12, 99] {
                assert_eq!(
                    stream_next(&src, &q, at, chunk, &never()).unwrap(),
                    range(find_next(&data, &q, at as usize))
                );
                assert_eq!(
                    stream_prev(&src, &q, at, chunk, &never()).unwrap(),
                    range(find_prev(&data, &q, at as usize))
                );
            }
        }
        // Empty query, query longer than the file, empty file.
        let empty = Query::Hex(vec![]);
        assert_eq!(stream_next(&src, &empty, 0, 4, &never()).unwrap(), None);
        assert_eq!(stream_count(&src, &empty, 5, 4, &never()).unwrap(), Count::default());
        let long = ascii(&"a".repeat(40), true);
        assert_eq!(stream_next(&src, &long, 0, 4, &never()).unwrap(), None);
        assert_eq!(stream_prev(&src, &long, 5, 4, &never()).unwrap(), None);
        let none = Source::resident(Vec::new());
        assert_eq!(stream_next(&none, &q, 0, 4, &never()).unwrap(), None);
        assert_eq!(stream_count(&none, &q, 0, 4, &never()).unwrap(), Count::default());
    }

    #[test]
    fn a_cancelled_search_is_cancelled_not_wrong() {
        let src = Source::resident(b"abcabc".to_vec());
        let q = ascii("abc", true);
        let generation = Arc::new(AtomicU64::new(1));
        let live = Cancel::new(generation.clone(), 1);
        assert!(stream_next(&src, &q, 0, 2, &live).unwrap().is_some());
        generation.store(2, Ordering::Relaxed);
        assert!(live.is_cancelled());
        assert!(matches!(
            stream_next(&src, &q, 0, 2, &live),
            Err(StreamError::Cancelled)
        ));
        assert!(matches!(
            stream_prev(&src, &q, 0, 2, &live),
            Err(StreamError::Cancelled)
        ));
        assert!(matches!(
            stream_count(&src, &q, 5, 2, &live),
            Err(StreamError::Cancelled)
        ));
        assert!(stream_count(&src, &q, 5, 2, &Cancel::new(generation, 2)).is_ok());
    }

    #[test]
    fn a_file_shrunk_under_the_search_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, vec![b'a'; 100]).unwrap();
        let src = super::super::open_with(&p, 0).unwrap().unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(10)
            .unwrap();
        let q = ascii("zz", true);
        assert!(matches!(
            stream_next(&src, &q, 0, 16, &never()),
            Err(StreamError::Io(_))
        ));
    }

    #[test]
    fn streams_a_paged_file_past_4_gib() {
        use std::os::unix::fs::FileExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        let len: u64 = (4 << 30) + 1000;
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(len).unwrap();
        // One match split by a chunk boundary, one at the very end.
        let split = (4u64 << 30) - 2;
        f.write_all_at(b"NEEDLE", split).unwrap();
        f.write_all_at(b"NEEDLE", len - 6).unwrap();
        let src = super::super::open(&p).unwrap().unwrap();
        assert!(src.resident_bytes().is_none());
        let q = ascii("needle", false);
        // Searches start near the matches (a scan of all 4 GiB would be slow in a debug build); the
        // small chunk's boundary falls inside the first match.
        let chunk = 4;
        assert_eq!(
            stream_next(&src, &q, split - 5, chunk, &never()).unwrap(),
            Some(split..split + 6)
        );
        assert_eq!(stream_prev(&src, &q, len, chunk, &never()).unwrap(), Some(len - 6..len));
        assert_eq!(
            stream_next(&src, &q, split + 1, chunk, &never()).unwrap(),
            Some(len - 6..len)
        );
    }

    #[test]
    fn a_search_cancelled_while_running_stops_early() {
        use std::os::unix::fs::FileExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        // A sparse 1 GiB file with the only match at the very end: a full scan reads all of it.
        let len: u64 = 1 << 30;
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(len).unwrap();
        f.write_all_at(b"needle", len - 6).unwrap();
        let src = super::super::open(&p).unwrap().unwrap();
        let q = ascii("needle", true);
        let generation = Arc::new(AtomicU64::new(1));
        let bump = {
            let generation = generation.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(5));
                generation.store(2, Ordering::Relaxed);
            })
        };
        // Small chunks: thousands of checks, so the bump lands mid-scan.
        let r = stream_next(&src, &q, 0, 64 << 10, &Cancel::new(generation.clone(), 1));
        bump.join().unwrap();
        assert!(matches!(r, Err(StreamError::Cancelled)), "{r:?}");
        // The same scan under the current generation finds the match.
        let r = stream_next(&src, &q, 0, 64 << 20, &Cancel::new(generation, 2)).unwrap();
        assert_eq!(r, Some(len - 6..len));
    }

    #[test]
    fn paged_files_stream_like_memory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let mut rng = Rng(0x2545f4914f6cdd1d);
        for _ in 0..40 {
            let n = 1 + rng.below(200);
            let data: Vec<u8> = (0..n).map(|_| b"abAB"[rng.below(3)]).collect();
            std::fs::write(&p, &data).unwrap();
            let src = super::super::open_with(&p, 0).unwrap().unwrap();
            assert!(src.resident_bytes().is_none());
            let needle: Vec<u8> = (0..1 + rng.below(3)).map(|_| b"abAB"[rng.below(3)]).collect();
            let q = ascii(std::str::from_utf8(&needle).unwrap(), rng.below(2) == 0);
            let chunk = 1 + rng.below(9);
            let at = rng.below(n + 2);
            let msg = format!("{data:?} {q:?} chunk {chunk} at {at}");
            assert_eq!(
                stream_next(&src, &q, at as u64, chunk, &never()).unwrap(),
                range(find_next(&data, &q, at)),
                "{msg}"
            );
            assert_eq!(
                stream_prev(&src, &q, at as u64, chunk, &never()).unwrap(),
                range(find_prev(&data, &q, at)),
                "{msg}"
            );
            assert_eq!(
                stream_count(&src, &q, 7, chunk, &never()).unwrap(),
                count(&data, &q, 7),
                "{msg}"
            );
        }
    }
}
