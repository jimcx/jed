//! Workspace search: every file under the root that the explorer would show (`.gitignore`,
//! `.workspace` excludes, never `.git/` or `.jed/`), searched with the `search::Matcher` find in
//! file uses, off the UI thread. No drawing here: the Search view (`ui::search`) draws `Results`.
//!
//! - The walk is `explorer::walker`, run in parallel; each file is read whole and searched with
//!   `grep-searcher`. Binary files are skipped by the buffer's own rule (`file::looks_binary`: a
//!   NUL in the first 8000 bytes), so what can't be opened isn't searched and a file that can is.
//!   A UTF-8 BOM is dropped and invalid UTF-8 decoded lossily, as loading does, so line text and
//!   columns are the buffer's.
//! - Lines are the buffer's: LF, CRLF and lone CR end a line, and the empty "line" after a final
//!   line break isn't one (as `rg`). `grep-searcher` only knows LF, so a file with a lone CR is
//!   searched as a rope instead.
//! - Open buffers with unsaved changes are searched from their text (a rope snapshot) instead of
//!   the disk; one whose file doesn't exist (new, or deleted) is searched when it lies in the root
//!   outside `.git/`, `.jed/` and the excludes. Both paths give the same matches for the same text.
//! - Matches stay within one line: the matcher is `Matcher::new_single_line`, so a pattern naming
//!   a line break (`\n`, `\W`, …) is searched line by line like any other.
//! - Results go back in `Batch`es of whole files (at most every `BATCH_INTERVAL`), in no
//!   particular order; `Results` keeps files sorted by path. The search stops at `MAX_MATCHES`
//!   (VS Code's cap) with `truncated` set, or when its `Handle` is cancelled or dropped.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkState;
use ignore::overrides::Override;
use ropey::Rope;

use crate::buffer::file;
use crate::buffer::pos::{self, ByteIdx};
use crate::explorer;
use crate::search::{self, Matcher, SearchError};

/// The most matches one search reports (VS Code's `search.maxResults`).
pub const MAX_MATCHES: usize = 20_000;
/// A line longer than this (bytes) keeps only this much text, around its first match.
pub const PREVIEW_LEN: usize = 300;
/// How much of a long line is kept before its first match.
const PREVIEW_BEFORE: usize = 60;
/// Results found are sent at most this often (plus once at the end).
const BATCH_INTERVAL: Duration = Duration::from_millis(50);

/// What to search for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub options: search::Options,
}

/// The matches on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// 0-based, counted as the buffer counts lines.
    pub line: usize,
    /// Each match as byte offsets into the whole line (without its line break), in order. Never
    /// empty; a match is empty only for regexes like `^` or `a*`.
    pub ranges: Vec<Range<usize>>,
    /// The line's text, or for a line longer than `PREVIEW_LEN` a piece of it around the first
    /// match (cut at char boundaries; later matches may lie partly or wholly outside it).
    pub text: String,
    /// Where `text` starts in the line: 0 unless the line was cut.
    pub text_start: usize,
}

impl LineMatch {
    /// The matches as offsets into `text`, clipped to it (what the Search view highlights).
    pub fn text_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        let (from, to) = (self.text_start, self.text_start + self.text.len());
        self.ranges
            .iter()
            .filter(move |r| r.start >= from && r.start <= to)
            .map(move |r| r.start - from..r.end.min(to) - from)
    }
}

/// One file's matches, lines in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    /// Relative to the workspace root.
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

impl FileMatches {
    pub fn count(&self) -> usize {
        self.lines.iter().map(|l| l.ranges.len()).sum()
    }
}

/// Results streamed back from a running search.
#[derive(Debug)]
pub struct Batch {
    pub generation: u64,
    pub files: Vec<FileMatches>,
    /// The last batch: the search finished (not sent for a cancelled search).
    pub done: bool,
    /// The search stopped at `MAX_MATCHES`; set on the last batch.
    pub truncated: bool,
}

/// One search's results so far, as the app keeps them.
#[derive(Debug, Default)]
pub struct Results {
    /// Which search these belong to; batches of any other are dropped.
    pub generation: u64,
    pub query: Query,
    /// Sorted by path.
    pub files: Vec<FileMatches>,
    /// Matches in `files`.
    pub total: usize,
    pub truncated: bool,
    pub done: bool,
}

impl Results {
    pub fn new(generation: u64, query: Query) -> Self {
        Self {
            generation,
            query,
            ..Self::default()
        }
    }

    /// Takes in a batch of this search; false (nothing changes) for another search's.
    pub fn add(&mut self, batch: Batch) -> bool {
        if batch.generation != self.generation || self.done {
            return false;
        }
        for f in batch.files {
            self.total += f.count();
            match self.files.binary_search_by(|g| g.path.cmp(&f.path)) {
                Ok(i) => {
                    self.total -= self.files[i].count();
                    self.files[i] = f;
                }
                Err(i) => self.files.insert(i, f),
            }
        }
        self.truncated |= batch.truncated;
        self.done |= batch.done;
        true
    }
}

/// The matcher for `query`: `Err` for an invalid regex (shown under the input; nothing runs).
pub fn matcher(query: &Query) -> Result<Matcher, SearchError> {
    Matcher::new_single_line(&query.pattern, query.options)
}

/// Everything a search needs, owned so it can move to another thread.
pub struct Job {
    pub root: PathBuf,
    pub excludes: Override,
    pub matcher: Matcher,
    /// Snapshots of open buffers with unsaved changes: path relative to the root, and text.
    pub dirty: Vec<(PathBuf, Rope)>,
    pub generation: u64,
}

/// A running search: cancels it when dropped.
pub struct Handle {
    cancel: Arc<AtomicBool>,
}

impl Handle {
    #[allow(dead_code)] // no caller yet: starting another search drops (and so cancels) the handle
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Runs `job` in `spawn_blocking`; `send` gets the batches (from the walk's threads).
pub fn start(job: Job, send: impl Fn(Batch) + Send + Sync + 'static) -> Handle {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    tokio::task::spawn_blocking(move || run(job, &flag, &send));
    Handle { cancel }
}

/// What one search did, for logging and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub searched: usize,
    pub matched_files: usize,
    pub matches: usize,
    pub truncated: bool,
    pub cancelled: bool,
}

/// Shared between the walk's threads.
struct Shared<'a> {
    generation: u64,
    cancel: &'a AtomicBool,
    send: &'a (dyn Fn(Batch) + Sync),
    /// Matches reserved so far (may overshoot `MAX_MATCHES`; what's over is dropped).
    total: AtomicUsize,
    truncated: AtomicBool,
    searched: AtomicUsize,
    matched_files: AtomicUsize,
    pending: Mutex<(Vec<FileMatches>, Instant)>,
}

impl Shared<'_> {
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.truncated.load(Ordering::Relaxed)
    }

    /// Matches a file may still collect: one past the cap tells "exactly full" from "truncated".
    fn room(&self) -> usize {
        (MAX_MATCHES + 1)
            .saturating_sub(self.total.load(Ordering::Relaxed))
            .max(1)
    }

    /// Records a searched file and queues its matches, trimmed to the cap.
    fn add(&self, path: PathBuf, mut lines: Vec<LineMatch>) {
        self.searched.fetch_add(1, Ordering::Relaxed);
        let n: usize = lines.iter().map(|l| l.ranges.len()).sum();
        if n == 0 || self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let before = self.total.fetch_add(n, Ordering::Relaxed);
        if before + n > MAX_MATCHES {
            self.truncated.store(true, Ordering::Relaxed);
            let mut keep = MAX_MATCHES.saturating_sub(before);
            if keep == 0 {
                return;
            }
            lines.retain_mut(|l| {
                l.ranges.truncate(keep);
                keep -= l.ranges.len();
                !l.ranges.is_empty()
            });
        }
        self.matched_files.fetch_add(1, Ordering::Relaxed);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.0.push(FileMatches { path, lines });
        if pending.1.elapsed() >= BATCH_INTERVAL {
            pending.1 = Instant::now();
            // Sent under the lock, so batches go out in the order they were taken.
            (self.send)(Batch {
                generation: self.generation,
                files: std::mem::take(&mut pending.0),
                done: false,
                truncated: false,
            });
        }
    }
}

/// Blocking: call from spawn_blocking. Searches the workspace, sending batches to `send`
/// (the last one with `done`, unless cancelled).
pub fn run(job: Job, cancel: &AtomicBool, send: &(dyn Fn(Batch) + Sync)) -> Stats {
    let t = Instant::now();
    let Job {
        root,
        excludes,
        matcher,
        dirty,
        generation,
    } = job;
    tracing::debug!(
        generation,
        dirty = dirty.len(),
        root = %root.display(),
        "workspace search started"
    );
    let shared = Shared {
        generation,
        cancel,
        send,
        total: AtomicUsize::new(0),
        truncated: AtomicBool::new(false),
        searched: AtomicUsize::new(0),
        matched_files: AtomicUsize::new(0),
        pending: Mutex::new((Vec::new(), Instant::now())),
    };
    if matcher.is_empty() {
        send(Batch {
            generation,
            files: Vec::new(),
            done: true,
            truncated: false,
        });
        return Stats {
            searched: 0,
            matched_files: 0,
            matches: 0,
            truncated: false,
            cancelled: false,
        };
    }
    let dirty: HashMap<PathBuf, Rope> = dirty.into_iter().collect();
    // Unsaved buffers the walk won't reach: their file doesn't exist.
    let mut unsaved: Vec<_> = dirty
        .iter()
        .filter(|(rel, _)| !root.join(rel).is_file() && in_scope(&root, rel, &excludes))
        .collect();
    unsaved.sort_by(|a, b| a.0.cmp(b.0));
    for (rel, rope) in unsaved {
        if shared.stopped() {
            break;
        }
        shared.add(rel.clone(), search_rope(&matcher, rope, shared.room()));
    }
    if !shared.stopped() {
        // As many threads as cores (`ignore`'s default, capped at 12).
        explorer::walker(&root, &excludes).build_parallel().run(|| {
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::none())
                .bom_sniffing(false)
                .build();
            let (root, matcher, dirty, shared) = (&root, &matcher, &dirty, &shared);
            Box::new(move |entry| {
                if shared.stopped() {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let Ok(rel) = entry.path().strip_prefix(root) else {
                    return WalkState::Continue;
                };
                let lines = match dirty.get(rel) {
                    Some(rope) => search_rope(matcher, rope, shared.room()),
                    None => match search_file(&mut searcher, matcher, entry.path(), shared) {
                        Ok(Some(lines)) => lines,
                        Ok(None) => return WalkState::Continue, // binary
                        Err(e) => {
                            tracing::debug!(path = %entry.path().display(), "not searched: {e}");
                            return WalkState::Continue;
                        }
                    },
                };
                shared.add(rel.to_path_buf(), lines);
                WalkState::Continue
            })
        });
    }
    let truncated = shared.truncated.load(Ordering::Relaxed);
    let cancelled = cancel.load(Ordering::Relaxed);
    let stats = Stats {
        searched: shared.searched.load(Ordering::Relaxed),
        matched_files: shared.matched_files.load(Ordering::Relaxed),
        matches: shared.total.load(Ordering::Relaxed).min(MAX_MATCHES),
        truncated,
        cancelled,
    };
    tracing::debug!(
        generation,
        searched = stats.searched,
        files = stats.matched_files,
        matches = stats.matches,
        truncated,
        cancelled,
        elapsed_ms = t.elapsed().as_millis() as u64,
        "workspace search finished"
    );
    if !cancelled {
        let files = std::mem::take(&mut shared.pending.lock().unwrap_or_else(|e| e.into_inner()).0);
        send(Batch {
            generation,
            files,
            done: true,
            truncated,
        });
    }
    stats
}

/// Whether a path the walk doesn't reach would be searched: not under `.git/` or `.jed/`, and
/// neither it nor a folder above it excluded. (`.gitignore` isn't checked.)
fn in_scope(root: &Path, rel: &Path, excludes: &Override) -> bool {
    let hidden = rel
        .components()
        .any(|c| matches!(c, Component::Normal(n) if explorer::HIDDEN_DIRS.iter().any(|h| n == *h)));
    let excluded = rel
        .ancestors()
        .filter(|a| !a.as_os_str().is_empty())
        .any(|a| excludes.matched(root.join(a), a != rel).is_ignore());
    !hidden && !excluded
}

/// One file from disk: `None` when it's binary.
fn search_file(
    searcher: &mut Searcher,
    matcher: &Matcher,
    path: &Path,
    shared: &Shared,
) -> io::Result<Option<Vec<LineMatch>>> {
    let bytes = std::fs::read(path)?;
    if file::looks_binary(&bytes) {
        return Ok(None);
    }
    let body = bytes.strip_prefix(file::BOM).unwrap_or(&bytes);
    let text = String::from_utf8_lossy(body);
    let limit = shared.room();
    if has_lone_cr(&text) {
        return Ok(Some(search_rope(matcher, &Rope::from_str(&text), limit)));
    }
    let mut sink = LineSink {
        matcher,
        cancel: shared.cancel,
        limit,
        count: 0,
        out: Vec::new(),
    };
    searcher.search_slice(matcher, text.as_bytes(), &mut sink)?;
    Ok(Some(sink.out))
}

/// A CR that isn't half of a CRLF: a line break `grep-searcher` doesn't know.
fn has_lone_cr(text: &str) -> bool {
    text.match_indices('\r')
        .any(|(i, _)| text.as_bytes().get(i + 1) != Some(&b'\n'))
}

/// Collects `grep-searcher`'s matching lines (LF-terminated; the file has no lone CR).
struct LineSink<'a> {
    matcher: &'a Matcher,
    cancel: &'a AtomicBool,
    /// Stop after this many matches.
    limit: usize,
    count: usize,
    out: Vec<LineMatch>,
}

impl Sink for LineSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let first = mat.line_number().unwrap_or(1) as usize - 1;
        for (i, line) in mat.lines().enumerate() {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let mut ranges = Vec::new();
            self.matcher.each_in_line(line, |s, e| {
                ranges.push(s..e);
                self.count + ranges.len() < self.limit
            });
            if ranges.is_empty() {
                continue;
            }
            self.count += ranges.len();
            // The searched text is a `str` and matches lie on char boundaries.
            let text = std::str::from_utf8(line).map_err(io::Error::other)?;
            self.out.push(line_match(first + i, text, ranges));
            if self.count >= self.limit {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The matches in a rope (an unsaved buffer, or a file with lone CRs), at most `limit`.
fn search_rope(matcher: &Matcher, rope: &Rope, limit: usize) -> Vec<LineMatch> {
    // The empty line after a final line break (or of an empty text) isn't searched.
    let last = pos::line_count(rope) - 1;
    let skip_last = pos::line_start(rope, last).0 == rope.len();
    let mut out: Vec<LineMatch> = Vec::new();
    let mut current: Option<(usize, ByteIdx, Vec<Range<usize>>)> = None;
    let flush = |out: &mut Vec<LineMatch>, current: Option<(usize, ByteIdx, Vec<Range<usize>>)>| {
        if let Some((line, _, ranges)) = current {
            out.push(line_match(line, &pos::line_text(rope, line), ranges));
        }
    };
    for m in matcher.find_all(rope).into_iter().take(limit) {
        let line = pos::line_of(rope, m.start);
        if skip_last && line == last {
            break;
        }
        if current.as_ref().is_none_or(|c| c.0 != line) {
            flush(&mut out, current.take());
            current = Some((line, pos::line_start(rope, line), Vec::new()));
        }
        let (_, start, ranges) = current.as_mut().expect("set above");
        ranges.push(m.start.0 - start.0..m.end.0 - start.0);
    }
    flush(&mut out, current);
    out
}

/// A `LineMatch`, cutting a long line's text down to a piece around the first match.
pub fn line_match(line: usize, text: &str, ranges: Vec<Range<usize>>) -> LineMatch {
    if text.len() <= PREVIEW_LEN {
        return LineMatch {
            line,
            ranges,
            text: text.to_string(),
            text_start: 0,
        };
    }
    let start = text.floor_char_boundary(ranges[0].start.saturating_sub(PREVIEW_BEFORE));
    let end = text.ceil_char_boundary((start + PREVIEW_LEN).min(text.len()));
    LineMatch {
        line,
        ranges,
        text: text[start..end].to_string(),
        text_start: start,
    }
}

/// The independent reference the search is checked against (here and in M7's end-to-end tests).
#[cfg(test)]
pub mod reference {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use ignore::overrides::Override;

    use super::Results;
    use crate::{explorer, search};

    /// `path:line:start-end` per match, line 1-based as an editor shows it.
    pub fn hits(r: &Results) -> Vec<String> {
        let mut out = Vec::new();
        for f in &r.files {
            for l in &f.lines {
                for m in &l.ranges {
                    out.push(format!("{}:{}:{}-{}", f.path.display(), l.line + 1, m.start, m.end));
                }
            }
        }
        out
    }

    /// What `rg` would report, the slow obvious way: the same file list, each file read and split
    /// into lines by hand, each line searched with `str` methods or a `regex::Regex` of its own.
    pub fn reference(root: &Path, pattern: &str, o: search::Options) -> Vec<String> {
        let mut paths = Vec::new();
        explorer::walk_files(root, &Override::empty(), &AtomicBool::new(false), |p| paths.push(p));
        paths.sort();
        let re = (o.regex || !o.case_sensitive).then(|| {
            let source = if o.regex {
                pattern.to_string()
            } else {
                regex::escape(pattern)
            };
            regex::RegexBuilder::new(&source)
                .case_insensitive(!o.case_sensitive)
                .build()
                .unwrap()
        });
        let mut out = Vec::new();
        for rel in paths {
            let bytes = fs::read(root.join(&rel)).unwrap();
            if bytes.iter().take(8000).any(|&b| b == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned();
            let mut lines: Vec<&str> = text.split('\n').collect();
            if lines.last() == Some(&"") {
                lines.pop();
            }
            let lines = lines.iter().flat_map(|l| {
                let l = l.strip_suffix('\r').unwrap_or(l);
                l.split('\r')
            });
            for (n, line) in lines.enumerate() {
                let found: Vec<(usize, usize)> = match &re {
                    Some(re) => re.find_iter(line).map(|m| (m.start(), m.end())).collect(),
                    None => line.match_indices(pattern).map(|(i, s)| (i, i + s.len())).collect(),
                };
                for (s, e) in found {
                    out.push(format!("{}:{}:{s}-{e}", rel.display(), n + 1));
                }
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // one-match lines
mod tests {
    use std::fs;
    use std::sync::mpsc;

    use super::reference::{hits, reference};
    use super::*;
    use crate::explorer::excludes;

    fn opts(regex: bool, case_sensitive: bool) -> search::Options {
        search::Options {
            regex,
            case_sensitive,
            whole_word: false,
        }
    }

    const PLAIN: search::Options = search::Options {
        regex: false,
        case_sensitive: false,
        whole_word: false,
    };

    fn root_of(dir: &tempfile::TempDir) -> PathBuf {
        crate::workspace::normalize(dir.path())
    }

    fn write(root: &Path, rel: &str, bytes: impl AsRef<[u8]>) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Runs a search to the end on this thread; results as `Results` would hold them.
    fn search_with(
        root: &Path,
        pattern: &str,
        options: search::Options,
        exclude: &[&str],
        dirty: Vec<(PathBuf, Rope)>,
    ) -> (Results, Stats) {
        let query = Query {
            pattern: pattern.to_string(),
            options,
        };
        let job = Job {
            root: root.to_path_buf(),
            excludes: excludes(root, &exclude.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
            matcher: matcher(&query).unwrap(),
            dirty,
            generation: 7,
        };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let stats = run(job, &AtomicBool::new(false), &|b| tx.lock().unwrap().send(b).unwrap());
        let mut results = Results::new(7, query);
        for batch in rx.try_iter() {
            assert!(results.add(batch));
        }
        assert!(results.done);
        assert_eq!(results.total, stats.matches);
        (results, stats)
    }

    fn search(root: &Path, pattern: &str, options: search::Options) -> Results {
        search_with(root, pattern, options, &[], Vec::new()).0
    }

    fn files(r: &Results) -> Vec<String> {
        r.files.iter().map(|f| f.path.display().to_string()).collect()
    }

    #[test]
    fn three_modes() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        write(&root, "a.txt", "Foo foo\nbar FOO.\nf.o\n");
        write(&root, "src/b.rs", "fn foo() {}\nfn Foo_bar() {}\n");
        assert_eq!(
            hits(&search(&root, "foo", PLAIN)),
            vec![
                "a.txt:1:0-3",
                "a.txt:1:4-7",
                "a.txt:2:4-7",
                "src/b.rs:1:3-6",
                "src/b.rs:2:3-6"
            ]
        );
        assert_eq!(
            hits(&search(&root, "Foo", opts(false, true))),
            vec!["a.txt:1:0-3", "src/b.rs:2:3-6"]
        );
        // Plain text is literal: `.` is a dot.
        assert_eq!(hits(&search(&root, "f.o", PLAIN)), vec!["a.txt:3:0-3"]);
        assert_eq!(
            hits(&search(&root, r"f.o\b", opts(true, false))),
            vec![
                "a.txt:1:0-3",
                "a.txt:1:4-7",
                "a.txt:2:4-7",
                "a.txt:3:0-3",
                "src/b.rs:1:3-6"
            ]
        );
        assert_eq!(
            hits(&search(&root, r"fn [A-Z]\w+", opts(true, true))),
            vec!["src/b.rs:2:0-10"]
        );
        let r = search(&root, "Foo", opts(false, true));
        assert_eq!(r.files[0].lines[0].text, "Foo foo");
        assert_eq!(r.files[1].lines[0].text, "fn Foo_bar() {}");
        assert_eq!(r.total, 2);
        assert!(!r.truncated);
        // Whole word works too (no toggle yet, but the option is the find widget's).
        let ww = search::Options {
            whole_word: true,
            ..PLAIN
        };
        assert_eq!(hits(&search(&root, "foo", ww)).len(), 4);
    }

    #[test]
    fn empty_and_invalid_patterns() {
        let q = |pattern: &str, regex| Query {
            pattern: pattern.into(),
            options: opts(regex, false),
        };
        assert!(matcher(&q("", false)).unwrap().is_empty());
        assert_eq!(matcher(&q("(a", true)).unwrap_err().to_string(), "unclosed group");
        assert!(matcher(&q("(a", false)).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        write(&root, "a.txt", "abc\n");
        let (r, stats) = search_with(&root, "", PLAIN, &[], Vec::new());
        assert!(r.files.is_empty());
        assert_eq!(stats.matches, 0);
    }

    #[test]
    fn skips_what_the_explorer_skips_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        fs::create_dir_all(root.join(".git")).unwrap();
        write(&root, ".git/config", "needle");
        write(&root, ".jed/history/x", "needle");
        write(&root, ".gitignore", "target/\n*.log\n");
        write(&root, "target/out.txt", "needle");
        write(&root, "app.log", "needle");
        write(&root, "build/gen.c", "needle");
        write(&root, "x.o", "needle");
        write(&root, ".hidden", "needle");
        write(&root, "keep.txt", "needle");
        write(&root, "sub/keep.txt", "a needle");
        let mut bin = b"needle\0".to_vec();
        bin.extend(b"x".repeat(100));
        write(&root, "bin.dat", &bin);
        // A NUL past the first 8000 bytes: the buffer opens it, so it's searched.
        let mut late = b"x".repeat(9000);
        late.extend(b"\0\nneedle\n");
        write(&root, "late.txt", &late);
        let (r, stats) = search_with(&root, "needle", PLAIN, &["build/", "*.o"], Vec::new());
        assert_eq!(files(&r), vec![".hidden", "keep.txt", "late.txt", "sub/keep.txt"]);
        assert_eq!(hits(&r)[2], "late.txt:2:0-6");
        // .gitignore, .hidden, keep.txt, sub/keep.txt, late.txt; bin.dat is read but binary.
        assert_eq!(stats.searched, 5);
    }

    #[test]
    fn line_endings_bom_and_multibyte() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        write(&root, "crlf.txt", "x\r\nab ab\r\nab\r\n");
        write(&root, "bom.txt", b"\xEF\xBB\xBFab\nxab");
        write(&root, "cr.txt", "ab\rx\rab\r\nab");
        write(&root, "utf8.txt", "héllo 界 ab AB\n😀ab\n");
        write(&root, "bad.txt", b"\xFFab\n");
        let r = search(&root, "ab", PLAIN);
        assert_eq!(
            hits(&r),
            vec![
                "bad.txt:1:3-5", // U+FFFD is 3 bytes, as the buffer decodes it
                "bom.txt:1:0-2",
                "bom.txt:2:1-3",
                "cr.txt:1:0-2",
                "cr.txt:3:0-2",
                "cr.txt:4:0-2",
                "crlf.txt:2:0-2",
                "crlf.txt:2:3-5",
                "crlf.txt:3:0-2",
                "utf8.txt:1:11-13",
                "utf8.txt:1:14-16",
                "utf8.txt:2:4-6",
            ]
        );
        let text = |path: &str, i: usize| {
            let f = r.files.iter().find(|f| f.path == Path::new(path)).unwrap();
            f.lines[i].text.clone()
        };
        assert_eq!(text("crlf.txt", 0), "ab ab");
        assert_eq!(text("bom.txt", 0), "ab");
        assert_eq!(text("cr.txt", 1), "ab");
        assert_eq!(text("utf8.txt", 0), "héllo 界 ab AB");
        // `$` sits before the CR; nothing matches the empty line after the last line break.
        let r = search(&root, "b$", opts(true, true));
        assert_eq!(files(&r), vec!["bad.txt", "bom.txt", "cr.txt", "crlf.txt", "utf8.txt"]);
        assert_eq!(search(&root, "^$", opts(true, true)).total, 0);
        // A pattern naming a line break is searched line by line too.
        assert_eq!(search(&root, r"x\nab", opts(true, false)).total, 0);
        // `ab ` in crlf.txt and utf8.txt; never `ab` + a line break.
        assert_eq!(search(&root, r"ab\W", opts(true, false)).total, 2);
    }

    #[test]
    fn long_lines_keep_a_piece_around_the_first_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        let line = format!("{}needle{}needle", "é".repeat(500), "x".repeat(1000));
        write(&root, "long.txt", &line);
        let r = search(&root, "needle", PLAIN);
        let l = &r.files[0].lines[0];
        assert_eq!(l.ranges, vec![1000..1006, 2006..2012]);
        assert!(l.text_start > 0 && l.text.len() <= PREVIEW_LEN + 3);
        assert_eq!(&line[l.text_start..l.text_start + l.text.len()], l.text);
        assert!(l.text.contains("needle"));
        assert_eq!(
            l.text_ranges().collect::<Vec<_>>(),
            vec![1000 - l.text_start..1006 - l.text_start]
        );
        // The piece is cut at char boundaries: 60 bytes before the match falls inside an `é`,
        // 300 bytes on inside a `界`.
        let line = format!("{}éneedle{}needle", "界".repeat(333), "界".repeat(200));
        write(&root, "long.txt", &line);
        let l = &search(&root, "needle", PLAIN).files[0].lines[0];
        assert_eq!(l.ranges, vec![1001..1007, 1607..1613]);
        assert_eq!((l.text_start, l.text.len()), (939, 302));
        assert_eq!(&line[939..1241], l.text);
        assert_eq!(l.text_ranges().collect::<Vec<_>>(), vec![62..68]);
        // A short line is kept whole.
        write(&root, "long.txt", "a needle");
        let l = &search(&root, "needle", PLAIN).files[0].lines[0];
        assert_eq!((l.text.as_str(), l.text_start), ("a needle", 0));
        assert_eq!(l.text_ranges().collect::<Vec<_>>(), vec![2..8]);
    }

    #[test]
    fn dirty_buffers_are_searched_instead_of_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        write(&root, "a.txt", "needle on disk\n");
        write(&root, "b.txt", "nothing\n");
        write(&root, "c.txt", "needle\n");
        write(&root, "build/x.txt", "x\n");
        let dirty = vec![
            (PathBuf::from("a.txt"), Rope::from_str("no match any more\n")),
            (PathBuf::from("b.txt"), Rope::from_str("x\r\nneedle\rand needle\n")),
            // Not on disk: searched when in scope.
            (PathBuf::from("new/d.txt"), Rope::from_str("needle")),
            (PathBuf::from("build/e.txt"), Rope::from_str("needle")),
            (PathBuf::from(".jed/f.txt"), Rope::from_str("needle")),
        ];
        let (r, _) = search_with(&root, "needle", PLAIN, &["build/"], dirty);
        assert_eq!(
            hits(&r),
            vec!["b.txt:2:0-6", "b.txt:3:4-10", "c.txt:1:0-6", "new/d.txt:1:0-6"]
        );
    }

    #[test]
    fn rope_and_file_paths_agree() {
        let texts = [
            "foo\nfoo foo\n\nfoo",
            "foo\r\nbar\r\nFOO\r\n",
            "a\rfoo\r\rfoo\n",
            "",
            "\n",
            "héllo 界 foo😀foo\nfoo",
            "x foo_bar foo.bar\n",
        ];
        let cases = [
            ("foo", PLAIN),
            ("FOO", opts(false, true)),
            (r"\bfoo\b", opts(true, false)),
            ("^", opts(true, true)),
            ("$", opts(true, true)),
            ("o*", opts(true, true)),
            (r"\W", opts(true, true)),
            (".", opts(true, true)),
        ];
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        for (i, text) in texts.iter().enumerate() {
            write(&root, &format!("{i}.txt"), text);
        }
        let dirty: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| (PathBuf::from(format!("{i}.txt")), Rope::from_str(t)))
            .collect();
        for (pattern, o) in cases {
            let disk = search(&root, pattern, o);
            let (memory, _) = search_with(&root, pattern, o, &[], dirty.clone());
            assert_eq!(disk.files, memory.files, "{pattern:?} {o:?}");
        }
    }

    #[test]
    fn cap_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        // 200 files of 1000 matches each.
        for i in 0..200 {
            write(&root, &format!("f{i:03}.txt"), "ab ab\n".repeat(500));
        }
        let (r, stats) = search_with(&root, "ab", PLAIN, &[], Vec::new());
        assert_eq!(r.total, MAX_MATCHES);
        assert!(r.truncated && stats.truncated);
        // The walk stops too: 21 files fill the cap, plus at most one in flight per thread.
        assert!(stats.searched < 50, "searched {}", stats.searched);
        // Exactly the cap: not truncated.
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        for i in 0..20 {
            write(&root, &format!("f{i:02}.txt"), "ab ab\n".repeat(500));
        }
        let r = search(&root, "ab", PLAIN);
        assert_eq!((r.total, r.truncated), (MAX_MATCHES, false));
        // One huge file stops early too.
        write(&root, "f00.txt", "ab".repeat(50_000));
        let r = search(&root, "ab", PLAIN);
        assert_eq!((r.total, r.truncated), (MAX_MATCHES, true));
        // One file alone, from disk and from memory: exactly the cap isn't truncated, one more is.
        for n in [MAX_MATCHES, MAX_MATCHES + 1] {
            let dir = tempfile::tempdir().unwrap();
            let root = root_of(&dir);
            write(&root, "one.txt", "ab\n".repeat(n));
            let r = search(&root, "ab", PLAIN);
            assert_eq!((r.total, r.truncated), (MAX_MATCHES, n > MAX_MATCHES), "{n} on disk");
            let dirty = vec![(PathBuf::from("one.txt"), Rope::from_str(&"ab ".repeat(n)))];
            let (r, _) = search_with(&root, "ab", PLAIN, &[], dirty);
            assert_eq!((r.total, r.truncated), (MAX_MATCHES, n > MAX_MATCHES), "{n} in memory");
        }
    }

    /// Both ways of searching a file stop at their limit (what's left of the cap), mid-line too,
    /// so a huge file doesn't collect matches only to drop them.
    #[test]
    fn file_searches_stop_at_their_limit() {
        let m = matcher(&Query {
            pattern: "ab".into(),
            options: PLAIN,
        })
        .unwrap();
        let text = "ab ab ab\nab ab\n";
        let count = |lines: &[LineMatch]| lines.iter().map(|l| l.ranges.len()).sum::<usize>();
        for limit in 1..=6 {
            let lines = search_rope(&m, &Rope::from_str(text), limit);
            assert_eq!(count(&lines), limit.min(5), "rope, limit {limit}");
            let mut sink = LineSink {
                matcher: &m,
                cancel: &AtomicBool::new(false),
                limit,
                count: 0,
                out: Vec::new(),
            };
            SearcherBuilder::new()
                .line_number(true)
                .build()
                .search_slice(&m, text.as_bytes(), &mut sink)
                .unwrap();
            assert_eq!(count(&sink.out), limit.min(5), "grep-searcher, limit {limit}");
            assert_eq!(sink.out, lines, "limit {limit}");
        }
    }

    #[test]
    fn cancel_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        for i in 0..1000 {
            write(&root, &format!("d{}/f{i:03}.txt", i % 10), "needle\n");
        }
        let query = Query {
            pattern: "needle".into(),
            options: PLAIN,
        };
        let job = || Job {
            root: root.clone(),
            excludes: Override::empty(),
            matcher: matcher(&query).unwrap(),
            dirty: Vec::new(),
            generation: 1,
        };
        // Cancelled before it starts: nothing searched, nothing sent.
        let sent = AtomicUsize::new(0);
        let stats = run(job(), &AtomicBool::new(true), &|_| {
            sent.fetch_add(1, Ordering::Relaxed);
        });
        assert!(stats.cancelled);
        assert_eq!((stats.searched, sent.load(Ordering::Relaxed)), (0, 0));
        // Cancelled from another thread, likely before the walk gets far: no last batch.
        // (That the walk itself stops is checked at the cap, which stops it the same way.)
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let thread = {
            let (cancel, done, job) = (cancel.clone(), done.clone(), job());
            std::thread::spawn(move || {
                run(job, &cancel, &|b| {
                    if b.done {
                        done.store(true, Ordering::Relaxed);
                    }
                })
            })
        };
        cancel.store(true, Ordering::Relaxed);
        let stats = thread.join().unwrap();
        assert!(stats.cancelled && !done.load(Ordering::Relaxed));
        assert!(stats.searched < 1000, "searched {}", stats.searched);
        assert_eq!(run(job(), &AtomicBool::new(false), &|_| {}).searched, 1000);
        // A `Handle` cancels when told to, and when dropped.
        let flag = Arc::new(AtomicBool::new(false));
        Handle { cancel: flag.clone() }.cancel();
        assert!(flag.load(Ordering::Relaxed));
        let flag = Arc::new(AtomicBool::new(false));
        drop(Handle { cancel: flag.clone() });
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn stale_batches_are_ignored() {
        let mut r = Results::new(2, Query::default());
        let batch = |generation, path: &str, done| Batch {
            generation,
            files: vec![FileMatches {
                path: path.into(),
                lines: vec![LineMatch {
                    line: 0,
                    ranges: vec![0..1],
                    text: "x".into(),
                    text_start: 0,
                }],
            }],
            done,
            truncated: false,
        };
        assert!(!r.add(batch(1, "old.txt", true)));
        assert!(r.add(batch(2, "b.txt", false)));
        assert!(r.add(batch(2, "a.txt", true)));
        assert!(!r.add(batch(2, "c.txt", false)), "nothing after done");
        assert_eq!(files(&r), vec!["a.txt", "b.txt"]);
        assert_eq!((r.total, r.done), (2, true));
    }

    #[test]
    fn agrees_with_a_reference_on_this_repo() {
        let root = crate::workspace::normalize(Path::new(env!("CARGO_MANIFEST_DIR")));
        let cases = [
            ("fn", PLAIN),
            ("Matcher", opts(false, true)),
            ("matcher", opts(false, true)),
            ("é", PLAIN),
            ("self.", PLAIN),
            (r"fn \w+", opts(true, false)),
            (r"\bSelf\b", opts(true, true)),
            (r"^\s*//!", opts(true, true)),
            (r"[A-Z][a-z]+Error", opts(true, true)),
            (r"\d+\s*$", opts(true, false)),
        ];
        for (pattern, o) in cases {
            let want = reference(&root, pattern, o);
            let got = hits(&search(&root, pattern, o));
            assert!(
                !want.is_empty() && want.len() <= MAX_MATCHES,
                "{pattern:?}: {}",
                want.len()
            );
            assert_eq!(got.len(), want.len(), "{pattern:?} {o:?}");
            assert_eq!(got, want, "{pattern:?} {o:?}");
        }
    }

    /// Searching a generated ~10 MB tree (~10 ms per search on Linux x86, 8 threads, files
    /// cached). Timings are asserted in release builds only (`cargo test --release search`);
    /// debug builds search 1 MB, to keep `cargo test` quick.
    #[test]
    fn ten_mb_tree_is_fast() {
        let dir = tempfile::tempdir().unwrap();
        let root = root_of(&dir);
        let mut size = 0;
        let mut files = 0;
        let target = if cfg!(debug_assertions) { 1_000_000 } else { 10_000_000 };
        while size < target {
            let mut text = String::new();
            for i in 0..1000 {
                text.push_str(&format!(
                    "    let value_{i} = compute(héllo, {files}, \"界\"); // note {i}\n"
                ));
            }
            size += text.len();
            write(&root, &format!("d{}/f{files}.rs", files % 20), &text);
            files += 1;
        }
        for (pattern, o, want) in [
            ("compute", PLAIN, (files * 1000).min(MAX_MATCHES)),
            ("not there", PLAIN, 0),
            (r"value_\d+9 =", opts(true, true), files * 99),
            ("note 999\n", PLAIN, 0),
            ("note 999", opts(false, true), files),
        ] {
            let t = Instant::now();
            let (r, stats) = search_with(&root, pattern, o, &[], Vec::new());
            let took = t.elapsed();
            println!(
                "workspace search {pattern:?} over {files} files, {size} bytes: {} matches, {} files searched, {took:?}",
                r.total, stats.searched
            );
            assert_eq!(r.total, want, "{pattern:?}");
            if !cfg!(debug_assertions) {
                assert!(took < Duration::from_secs(2), "{pattern:?} too slow: {took:?}");
            }
        }
    }
}
