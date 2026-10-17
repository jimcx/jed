//! Tool logs: an in-memory ring of lines per tool jed runs (each language server, and "jed"
//! itself), shared by the server tasks, the `tracing` layer for "jed" ([`layer`]) and, later, the
//! log tabs and the Tools view. No drawing, no app state.
//!
//! - [`Ring`]: the last [`MAX_LINES`] lines / [`MAX_BYTES`] bytes of one tool, the oldest dropped.
//!   Every line gets a sequence number; a clear bumps the ring's generation. A reader keeps a
//!   [`Cursor`] and asks [`Ring::read`] for the lines since it, learning when lines it never saw
//!   were dropped (`missed`) or the ring was cleared (`reset`).
//! - [`Logs`]: the store (a cheap `Clone`, one `Arc`): `push` takes the lock for a few
//!   `VecDeque` operations only, so callers on the UI thread (the `tracing` layer) pay little.
//!   Once [`Logs::set_disk`] gave it a folder (`App::start`: `<state dir>/logs/tools`; not in tests
//!   unless one sets `App::state_dir`), every tool but "jed" (already the daily `jed.<date>.log`)
//!   also goes to `<tool>.log` there ([`disk`]): lines are queued and appended in batches by one
//!   `spawn_blocking` job at a time; while the disk lags, the queue stops at [`MAX_PENDING`]
//!   (those lines stay in memory, the file gets a note). A write error is kept for the app to report once
//!   ([`Logs::take_disk_error`]); logging carries on in memory.

pub mod disk;
pub mod layer;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// A ring keeps at most this many lines…
pub const MAX_LINES: usize = 10_000;
/// …and this many bytes of text.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// A longer line is cut (a protocol trace's `didOpen` of a big file).
pub const MAX_LINE: usize = 64 * 1024;
/// How long a disk job waits before writing, so lines arriving together go out together.
const DISK_BATCH: Duration = Duration::from_millis(50);
/// Queued bytes beyond which new lines aren't queued for disk (a disk slower than a server's
/// output, a hung network file system): they stay in memory, the file gets a note.
const MAX_PENDING: usize = 8 * 1024 * 1024;

/// A tool with a log.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    /// jed's own `tracing` events.
    Jed,
    /// A language server, by `ServerConfig::name` (shared by its restarts).
    Server(String),
}

impl Tool {
    #[cfg_attr(not(test), allow(dead_code))] // log tabs, the Tools view
    pub fn name(&self) -> &str {
        match self {
            Tool::Jed => "jed",
            Tool::Server(name) => name,
        }
    }

    /// The file stem its log goes to on disk; `None`: not written (jed's own log is the daily
    /// `jed.<date>.log` already).
    pub fn file_stem(&self) -> Option<String> {
        match self {
            Tool::Jed => None,
            Tool::Server(name) => Some(disk::file_stem(name)),
        }
    }
}

/// A line's level: `tracing`'s for "jed", a `window/logMessage`'s type for a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }

    pub fn from_tracing(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Level::Error,
            tracing::Level::WARN => Level::Warn,
            tracing::Level::INFO => Level::Info,
            tracing::Level::DEBUG => Level::Debug,
            tracing::Level::TRACE => Level::Trace,
        }
    }

    /// An LSP `MessageType` (`window/logMessage`, `window/showMessage`): 1 error, 2 warning,
    /// 3 info, 4 log, 5 debug.
    pub fn from_lsp(kind: Option<i64>) -> Self {
        match kind {
            Some(1) => Level::Error,
            Some(2) => Level::Warn,
            Some(3) => Level::Info,
            _ => Level::Debug,
        }
    }
}

/// One line of a log (no line break in `text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Unix ms.
    pub time: u64,
    pub level: Level,
    pub text: String,
}

/// Where a reader is in a ring: the generation it read and the sequence number of the next
/// line it hasn't. The default reads everything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub generation: u64,
    pub next: u64,
}

/// What [`Ring::read`] gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The ring was cleared since the cursor's read: start over with `lines`.
    pub reset: bool,
    /// Lines dropped (the ring wrapped) before the reader got them.
    pub missed: u64,
    /// The sequence number of the ring's oldest line: a reader mirroring the ring drops its own
    /// lines before it.
    pub first: u64,
    /// The new lines, oldest first.
    pub lines: Vec<Line>,
    /// For the next read.
    pub cursor: Cursor,
}

/// One tool's lines.
#[derive(Debug)]
pub struct Ring {
    lines: VecDeque<Line>,
    bytes: usize,
    /// Sequence number of `lines[0]`.
    first: u64,
    generation: u64,
    /// Error lines since the last clear (the Tools view's badge).
    errors: u64,
    max_lines: usize,
    max_bytes: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Self::with_caps(MAX_LINES, MAX_BYTES)
    }
}

impl Ring {
    pub fn with_caps(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            first: 0,
            generation: 0,
            errors: 0,
            max_lines: max_lines.max(1),
            max_bytes,
        }
    }

    pub fn push(&mut self, line: Line) {
        if line.level == Level::Error {
            self.errors += 1;
        }
        self.bytes += line.text.len();
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines || (self.bytes > self.max_bytes && self.lines.len() > 1) {
            let old = self.lines.pop_front().expect("not empty");
            self.bytes -= old.text.len();
            self.first += 1;
        }
    }

    /// Drops every line; readers get `reset` on their next read. Sequence numbers go on.
    pub fn clear(&mut self) {
        self.first = self.next();
        self.lines.clear();
        self.bytes = 0;
        self.errors = 0;
        self.generation += 1;
    }

    /// The lines since `cursor`.
    pub fn read(&self, cursor: Cursor) -> Read {
        let (reset, missed, skip) = if cursor.generation != self.generation {
            (true, 0, 0)
        } else if cursor.next < self.first {
            (false, self.first - cursor.next, 0)
        } else {
            (false, 0, (cursor.next - self.first) as usize)
        };
        Read {
            reset,
            missed,
            first: self.first,
            lines: self.lines.iter().skip(skip).cloned().collect(),
            cursor: self.cursor(),
        }
    }

    /// A cursor past the last line.
    pub fn cursor(&self) -> Cursor {
        Cursor {
            generation: self.generation,
            next: self.next(),
        }
    }

    /// The sequence number the next line gets.
    pub fn next(&self) -> u64 {
        self.first + self.lines.len() as u64
    }

    #[cfg_attr(not(test), allow(dead_code))] // log tabs, the Tools view
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    #[cfg_attr(not(test), allow(dead_code))] // clippy's len_without_is_empty; log tabs later
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))] // log tabs, the Tools view
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    #[cfg_attr(not(test), allow(dead_code))] // log tabs, the Tools view
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }
}

/// `text` split at line breaks (a trailing one dropped, CRLF as one), each part cut to
/// [`MAX_LINE`] bytes.
fn split_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    text.split('\n').map(|l| {
        let l = l.strip_suffix('\r').unwrap_or(l);
        if l.len() <= MAX_LINE {
            l.to_string()
        } else {
            let cut = l.floor_char_boundary(MAX_LINE);
            format!("{}… ({} bytes)", &l[..cut], l.len())
        }
    })
}

/// The disk side of the store.
#[derive(Default)]
struct DiskState {
    config: Option<DiskConfig>,
    /// Lines waiting for the next job: file stem and the formatted line (with its `\n`).
    pending: Vec<(String, String)>,
    /// Their bytes (at most [`MAX_PENDING`]).
    pending_bytes: usize,
    /// Lines not queued since the last job took the queue, per file stem.
    dropped: BTreeMap<String, u64>,
    /// A job is running (it drains `pending` until empty).
    busy: bool,
    /// The first write error, kept until the app takes it.
    error: Option<String>,
    /// An error was already reported: later ones only go to `tracing`.
    reported: bool,
}

#[derive(Clone)]
struct DiskConfig {
    dir: PathBuf,
    root: PathBuf,
    runtime: tokio::runtime::Handle,
    max_file: u64,
    max_pending: usize,
}

#[derive(Default)]
struct Inner {
    rings: Mutex<BTreeMap<Tool, Ring>>,
    disk: Mutex<DiskState>,
    /// `disk.error` is set: lets the app's per-turn check skip the lock.
    has_error: AtomicBool,
}

/// The log store: every tool's ring, and the queue to disk. Cloning shares it.
#[derive(Clone, Default)]
pub struct Logs {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Logs {
    /// From now on every tool but "jed" is also appended to `<dir>/<tool>.log`, each line with
    /// its time, jed's pid and `root`. Must run inside the tokio runtime (`App::start`): the
    /// writes go through its `spawn_blocking`.
    pub fn set_disk(&self, dir: PathBuf, root: PathBuf) {
        self.set_disk_with(dir, root, disk::MAX_FILE, MAX_PENDING);
    }

    /// `set_disk` with another rotation size and queue limit (tests).
    pub fn set_disk_with(&self, dir: PathBuf, root: PathBuf, max_file: u64, max_pending: usize) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("tool logs: no runtime, not written to disk");
            return;
        };
        lock(&self.inner.disk).config = Some(DiskConfig {
            dir,
            root,
            runtime,
            max_file,
            max_pending,
        });
    }

    /// A handle that pushes to `tool`'s log.
    pub fn tool(&self, tool: Tool) -> ToolLog {
        ToolLog {
            logs: self.clone(),
            tool,
        }
    }

    /// Appends `text` (split at line breaks) to `tool`'s ring, and queues it for its file.
    /// Cheap: no I/O, a short lock. Never logs through `tracing` (the "jed" layer calls it).
    pub fn push(&self, tool: &Tool, level: Level, text: &str) {
        let time = crate::history::now_ms();
        let lines: Vec<Line> = split_lines(text).map(|text| Line { time, level, text }).collect();
        let stem = tool.file_stem();
        let mut formatted = Vec::new();
        if let Some(stem) = &stem {
            let disk = lock(&self.inner.disk);
            if let Some(config) = &disk.config {
                let root = config.root.display().to_string();
                formatted = lines
                    .iter()
                    .map(|l| (stem.clone(), disk::format_line(l, &root)))
                    .collect();
            }
        }
        {
            let mut rings = lock(&self.inner.rings);
            let ring = match rings.get_mut(tool) {
                Some(r) => r,
                None => rings.entry(tool.clone()).or_default(),
            };
            for line in lines {
                ring.push(line);
            }
        }
        if !formatted.is_empty() {
            self.queue(formatted);
        }
    }

    fn queue(&self, lines: Vec<(String, String)>) {
        let mut disk = lock(&self.inner.disk);
        let Some(config) = disk.config.clone() else {
            return;
        };
        for (stem, line) in lines {
            if disk.pending_bytes + line.len() > config.max_pending {
                *disk.dropped.entry(stem).or_default() += 1;
            } else {
                disk.pending_bytes += line.len();
                disk.pending.push((stem, line));
            }
        }
        if disk.busy {
            return;
        }
        disk.busy = true;
        drop(disk);
        let logs = self.clone();
        let runtime = config.runtime.clone();
        runtime.spawn_blocking(move || logs.write_pending(&config));
    }

    /// The disk job: writes what's queued, batched per file, until nothing is.
    /// Blocking: call from spawn_blocking.
    fn write_pending(&self, config: &DiskConfig) {
        std::thread::sleep(DISK_BATCH);
        loop {
            let (pending, dropped) = {
                let mut disk = lock(&self.inner.disk);
                if disk.pending.is_empty() && disk.dropped.is_empty() {
                    disk.busy = false;
                    return;
                }
                disk.pending_bytes = 0;
                (std::mem::take(&mut disk.pending), std::mem::take(&mut disk.dropped))
            };
            let mut by_file: BTreeMap<String, String> = BTreeMap::new();
            for (stem, line) in pending {
                by_file.entry(stem).or_default().push_str(&line);
            }
            let root = config.root.display().to_string();
            for (stem, n) in dropped {
                let note = Line {
                    time: crate::history::now_ms(),
                    level: Level::Warn,
                    text: format!("… {n} lines not written: the disk was too slow"),
                };
                by_file
                    .entry(stem)
                    .or_default()
                    .push_str(&disk::format_line(&note, &root));
            }
            for (stem, text) in by_file {
                if let Err(e) = disk::append(&config.dir, &stem, text.as_bytes(), config.max_file) {
                    let message = format!(
                        "Can't write the {stem} log to {}: {e} (kept in memory only)",
                        config.dir.display()
                    );
                    let first = {
                        let mut disk = lock(&self.inner.disk);
                        let first = !disk.reported;
                        if first {
                            disk.reported = true;
                            disk.error = Some(message.clone());
                            self.inner.has_error.store(true, Ordering::Release);
                        }
                        first
                    };
                    if first {
                        tracing::warn!("{message}");
                    } else {
                        tracing::debug!("{message}");
                    }
                }
            }
        }
    }

    /// The first disk write error, once (the app reports it); `None` after that.
    pub fn take_disk_error(&self) -> Option<String> {
        if !self.inner.has_error.swap(false, Ordering::AcqRel) {
            return None;
        }
        lock(&self.inner.disk).error.take()
    }

    /// Waits (≤ `limit`) until every queued line is written. True when it was.
    /// Blocking: call from spawn_blocking.
    pub fn flush(&self, limit: Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        loop {
            {
                let disk = lock(&self.inner.disk);
                if !disk.busy && disk.pending.is_empty() && disk.dropped.is_empty() {
                    return true;
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The lines of `tool` since `cursor` (an empty read for a tool with no log yet).
    #[cfg_attr(not(test), allow(dead_code))] // log tabs (tools-b)
    pub fn read(&self, tool: &Tool, cursor: Cursor) -> Read {
        match lock(&self.inner.rings).get(tool) {
            Some(ring) => ring.read(cursor),
            None => Ring::default().read(cursor),
        }
    }

    /// Empties `tool`'s ring ("Tools: Clear Log"); its file is kept.
    #[cfg_attr(not(test), allow(dead_code))] // tools-b
    pub fn clear(&self, tool: &Tool) {
        if let Some(ring) = lock(&self.inner.rings).get_mut(tool) {
            ring.clear();
        }
    }

    /// The tools with a log, sorted ("jed" first).
    #[cfg_attr(not(test), allow(dead_code))] // tools-b / tools-c
    pub fn tools(&self) -> Vec<Tool> {
        lock(&self.inner.rings).keys().cloned().collect()
    }

    /// Error lines in `tool`'s log since it was last cleared.
    #[cfg_attr(not(test), allow(dead_code))] // the Tools view's badge (tools-c)
    pub fn errors(&self, tool: &Tool) -> u64 {
        lock(&self.inner.rings).get(tool).map_or(0, Ring::errors)
    }

    /// `tool`'s lines' texts, oldest first.
    #[cfg(test)]
    pub fn texts(&self, tool: &Tool) -> Vec<String> {
        lock(&self.inner.rings)
            .get(tool)
            .map(|r| r.lines().map(|l| l.text.clone()).collect())
            .unwrap_or_default()
    }

    /// `tool`'s lines, oldest first.
    #[cfg(test)]
    pub fn lines(&self, tool: &Tool) -> Vec<Line> {
        lock(&self.inner.rings)
            .get(tool)
            .map(|r| r.lines().cloned().collect())
            .unwrap_or_default()
    }
}

/// Pushes to one tool's log (given to each language server's task).
#[derive(Clone)]
pub struct ToolLog {
    logs: Logs,
    tool: Tool,
}

impl ToolLog {
    pub fn push(&self, level: Level, text: &str) {
        self.logs.push(&self.tool, level, text);
    }

    pub fn info(&self, text: &str) {
        self.push(Level::Info, text);
    }

    pub fn error(&self, text: &str) {
        self.push(Level::Error, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line {
            time: 0,
            level: Level::Info,
            text: text.into(),
        }
    }

    fn texts(read: &Read) -> Vec<&str> {
        read.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn ring_caps_lines() {
        let mut ring = Ring::with_caps(3, 1000);
        for i in 0..5 {
            ring.push(line(&i.to_string()));
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(texts(&ring.read(Cursor::default())), ["2", "3", "4"]);
        assert_eq!(ring.next(), 5);
    }

    #[test]
    fn ring_caps_bytes() {
        let mut ring = Ring::with_caps(100, 10);
        ring.push(line("aaaa"));
        ring.push(line("bbbb"));
        assert_eq!(ring.bytes(), 8);
        ring.push(line("cccc"));
        assert_eq!(ring.bytes(), 8);
        assert_eq!(texts(&ring.read(Cursor::default())), ["bbbb", "cccc"]);
        // One line over the cap is still kept, alone.
        ring.push(line("dddddddddddddd"));
        assert_eq!(texts(&ring.read(Cursor::default())), ["dddddddddddddd"]);
    }

    #[test]
    fn default_caps() {
        let mut ring = Ring::default();
        for _ in 0..MAX_LINES + 5 {
            ring.push(line("x"));
        }
        assert_eq!(ring.len(), MAX_LINES);
        let mut ring = Ring::default();
        let big = "y".repeat(1024);
        for _ in 0..3000 {
            ring.push(line(&big));
        }
        assert_eq!(ring.len(), MAX_BYTES / 1024);
        assert!(ring.bytes() <= MAX_BYTES);
    }

    #[test]
    fn reads_since_a_cursor() {
        let mut ring = Ring::with_caps(10, 1000);
        ring.push(line("a"));
        ring.push(line("b"));
        let first = ring.read(Cursor::default());
        assert_eq!(texts(&first), ["a", "b"]);
        assert!(!first.reset);
        assert_eq!(first.missed, 0);
        let none = ring.read(first.cursor);
        assert!(none.lines.is_empty());
        ring.push(line("c"));
        let more = ring.read(first.cursor);
        assert_eq!(texts(&more), ["c"]);
        assert_eq!(more.cursor.next, 3);
    }

    #[test]
    fn a_wrapped_ring_reports_missed_lines() {
        let mut ring = Ring::with_caps(3, 1000);
        ring.push(line("0"));
        let cursor = ring.read(Cursor::default()).cursor;
        for i in 1..6 {
            ring.push(line(&i.to_string()));
        }
        // Read "0"; "1" and "2" dropped unread.
        let read = ring.read(cursor);
        assert_eq!(read.missed, 2);
        assert_eq!(read.first, 3);
        assert_eq!(texts(&read), ["3", "4", "5"]);
        assert!(!read.reset);
    }

    #[test]
    fn a_cleared_ring_resets_readers() {
        let mut ring = Ring::with_caps(10, 1000);
        ring.push(Line {
            level: Level::Error,
            ..line("bad")
        });
        assert_eq!(ring.errors(), 1);
        let cursor = ring.read(Cursor::default()).cursor;
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.errors(), 0);
        ring.push(line("new"));
        let read = ring.read(cursor);
        assert!(read.reset);
        assert_eq!(texts(&read), ["new"]);
        // Sequence numbers go on.
        assert_eq!(read.first, 1);
        assert!(!ring.read(read.cursor).reset);
    }

    #[test]
    fn pushes_split_and_cut_lines() {
        let logs = Logs::default();
        let tool = Tool::Server("srv".into());
        logs.push(&tool, Level::Info, "one\r\ntwo\nthree\n");
        let long = "é".repeat(MAX_LINE);
        logs.push(&tool, Level::Warn, &long);
        let texts = logs.texts(&tool);
        assert_eq!(&texts[..3], ["one", "two", "three"]);
        assert!(texts[3].len() < MAX_LINE + 40);
        assert!(texts[3].ends_with(&format!("… ({} bytes)", long.len())));
        assert_eq!(logs.tools(), std::slice::from_ref(&tool));
        let read = logs.read(&tool, Cursor::default());
        assert_eq!(read.lines.len(), 4);
        assert!(logs.read(&tool, read.cursor).lines.is_empty());
        assert!(logs.read(&Tool::Jed, Cursor::default()).lines.is_empty());
        assert_eq!(logs.errors(&tool), 0);
        logs.clear(&tool);
        assert!(logs.texts(&tool).is_empty());
    }

    #[test]
    fn tool_names() {
        assert_eq!(Tool::Jed.name(), "jed");
        assert_eq!(Tool::Jed.file_stem(), None);
        let ra = Tool::Server("rust-analyzer".into());
        assert_eq!(ra.file_stem().as_deref(), Some("rust-analyzer"));
        assert!(Tool::Jed < ra);
        assert_eq!(Level::from_lsp(Some(1)), Level::Error);
        assert_eq!(Level::from_lsp(Some(4)), Level::Debug);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn servers_go_to_disk_but_not_jed() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::default();
        logs.set_disk(dir.path().to_path_buf(), "/ws".into());
        logs.push(&Tool::Jed, Level::Info, "jed line");
        let srv = logs.tool(Tool::Server("my server".into()));
        srv.info("hello");
        srv.error("oops");
        let flushed = {
            let logs = logs.clone();
            tokio::task::spawn_blocking(move || logs.flush(Duration::from_secs(5)))
                .await
                .unwrap()
        };
        assert!(flushed);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| !n.ends_with(".lock"))
            .collect();
        assert_eq!(names, ["my_server.log"]);
        let text = std::fs::read_to_string(dir.path().join("my_server.log")).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let pid = format!("[{} /ws]", std::process::id());
        assert!(
            lines[0].contains(&pid) && lines[0].ends_with("INFO  hello"),
            "{}",
            lines[0]
        );
        assert!(lines[1].ends_with("ERROR oops"), "{}", lines[1]);
        assert_eq!(logs.take_disk_error(), None);
    }

    async fn flushed(logs: &Logs) {
        let logs = logs.clone();
        let done = tokio::task::spawn_blocking(move || logs.flush(Duration::from_secs(5)));
        assert!(done.await.unwrap());
    }

    /// The store rotates its files at the size it was given.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_store_rotates_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::default();
        // Lines of about 50 bytes, files of at most 60: one line each.
        logs.set_disk_with(dir.path().to_path_buf(), "/ws".into(), 60, MAX_PENDING);
        let srv = logs.tool(Tool::Server("srv".into()));
        for i in 0..3 {
            srv.info(&format!("line {i}"));
            flushed(&logs).await;
        }
        let file = std::fs::read_to_string(disk::file(dir.path(), "srv")).unwrap();
        assert!(file.ends_with("INFO  line 2\n") && file.lines().count() == 1, "{file}");
        for (n, i) in [(1, 1), (2, 0)] {
            let rotated = std::fs::read_to_string(disk::rotated(dir.path(), "srv", n)).unwrap();
            assert!(
                rotated.ends_with(&format!("INFO  line {i}\n")) && rotated.lines().count() == 1,
                "{rotated}"
            );
        }
    }

    /// While the disk is stuck, the queue stops growing at its limit: the rest stays in memory
    /// and the file says how many lines it lacks.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stuck_disk_doesnt_queue_without_bound() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::default();
        // Room for about three lines in the queue.
        logs.set_disk_with(dir.path().to_path_buf(), "/ws".into(), disk::MAX_FILE, 3 * 80);
        // Another window holding the file's lock: the job blocks in its first append.
        let held = std::fs::File::create(dir.path().join("srv.log.lock")).unwrap();
        held.lock().unwrap();
        let srv = logs.tool(Tool::Server("srv".into()));
        srv.info("first");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !lock(&logs.inner.disk).pending.is_empty() {
            assert!(std::time::Instant::now() < deadline, "the job took the queue");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for i in 0..10 {
            srv.info(&format!("line {i}"));
        }
        assert!(lock(&logs.inner.disk).pending_bytes <= 3 * 80);
        held.unlock().unwrap();
        flushed(&logs).await;
        assert_eq!(logs.texts(&Tool::Server("srv".into())).len(), 11, "all in memory");
        let file = std::fs::read_to_string(disk::file(dir.path(), "srv")).unwrap();
        let lines: Vec<_> = file.lines().collect();
        let written = lines.iter().filter(|l| l.contains("INFO  line ")).count();
        assert!(lines[0].ends_with("INFO  first"), "{file}");
        assert!((1..10).contains(&written), "{file}");
        let note = format!("WARN  … {} lines not written: the disk was too slow", 10 - written);
        assert!(lines.last().unwrap().ends_with(&note), "{file}");
        assert_eq!(lines.len(), written + 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_write_error_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be.
        let blocked = dir.path().join("tools");
        std::fs::write(&blocked, "").unwrap();
        let logs = Logs::default();
        logs.set_disk(blocked, "/ws".into());
        let srv = logs.tool(Tool::Server("srv".into()));
        srv.info("one");
        let wait = |logs: Logs| tokio::task::spawn_blocking(move || logs.flush(Duration::from_secs(5)));
        assert!(wait(logs.clone()).await.unwrap());
        let error = logs.take_disk_error().expect("reported");
        assert!(error.contains("srv log"), "{error}");
        assert_eq!(logs.take_disk_error(), None);
        srv.info("two");
        assert!(wait(logs.clone()).await.unwrap());
        assert_eq!(logs.take_disk_error(), None, "once");
        // In memory all the same.
        assert_eq!(logs.texts(&Tool::Server("srv".into())), ["one", "two"]);
    }
}
