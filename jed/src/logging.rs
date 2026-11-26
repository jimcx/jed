//! File logging. Nothing may write to stdout/stderr while the UI is up.
//!
//! Every jed instance appends to the same daily file: each line carries the writer's pid after
//! its timestamp, and each event reaches the file in one `write_all` on a file opened with
//! `O_APPEND` (tracing-appender's), so concurrent instances' lines never interleave mid-line.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::paths;

/// Changes the log filter while running ("Set Log Level").
pub type LevelHandle = reload::Handle<EnvFilter, Registry>;

pub fn log_dir() -> PathBuf {
    paths::state_dir().join("logs")
}

/// `filter` comes from `--log-level`; otherwise `JED_LOG`, otherwise `info`.
/// Both use `EnvFilter` syntax, e.g. `jed::buffer=debug,input=trace`. What passes the filter
/// also goes to `tools`' "jed" ring (`logs::layer`).
pub fn init(filter: Option<&str>, tools: crate::logs::Logs) -> anyhow::Result<LevelHandle> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("jed")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)?;
    let filter = match filter {
        Some(f) => EnvFilter::try_new(f)?,
        None => EnvFilter::try_from_env("JED_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
    };
    // Blocking writer: the panic hook exits the process, which would lose a non-blocking writer's queue.
    let (subscriber, handle) = subscriber(filter, appender, tools);
    subscriber.init();
    Ok(handle)
}

/// jed's `tracing` stack: the reloadable `filter`, then the log file's lines (time, pid, level,
/// target, message) to `writer`, one write per event, and the same events to `tools`' "jed" ring.
pub fn subscriber<W>(
    filter: EnvFilter,
    writer: W,
    tools: crate::logs::Logs,
) -> (impl tracing::Subscriber + Send + Sync, LevelHandle)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let (filter, handle) = reload::Layer::new(filter);
    let file = tracing_subscriber::fmt::layer()
        .with_writer(WholeEvents(writer))
        .with_timer(WithPid(std::process::id()))
        .with_ansi(false);
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(file)
        .with(crate::logs::layer::layer(tools));
    (subscriber, handle)
}

/// The default timestamp, then the process id: tells apart the lines of jed instances sharing
/// a day's file.
struct WithPid(u32);

impl FormatTime for WithPid {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        SystemTime.format_time(w)?;
        write!(w, " {}", self.0)
    }
}

/// Hands each event to the inner writer in a single `write_all` (on an `O_APPEND` file one
/// `write`, which the kernel appends whole), however the formatter writes it.
struct WholeEvents<M>(M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for WholeEvents<M> {
    type Writer = Buffered<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        Buffered {
            buf: Vec::new(),
            inner: self.0.make_writer(),
        }
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        Buffered {
            buf: Vec::new(),
            inner: self.0.make_writer_for(meta),
        }
    }
}

/// One event's bytes, written out when dropped (the fmt layer drops it after each event).
struct Buffered<W: Write> {
    buf: Vec<u8>,
    inner: W,
}

impl<W: Write> Write for Buffered<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Drop for Buffered<W> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let _ = self.inner.write_all(&self.buf);
            let _ = self.inner.flush();
        }
    }
}

/// The filter in use, in `EnvFilter` syntax.
pub fn current_filter(handle: &LevelHandle) -> Option<String> {
    handle.with_current(|f| f.to_string()).ok()
}

/// Replaces the filter; `filter` uses `EnvFilter` syntax (`debug`, `jed::lsp=trace,info`, …).
pub fn set_filter(handle: &LevelHandle, filter: &str) -> Result<(), String> {
    let new = EnvFilter::try_new(filter).map_err(|e| e.to_string())?;
    handle.reload(new).map_err(|e| e.to_string())?;
    tracing::info!(filter, "log filter changed");
    Ok(())
}

/// Blocking. The newest `jed.*.log` in `dir` (daily files sort by date).
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("jed.") && n.ends_with(".log"))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_log_by_date() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(newest_log(dir.path()), None);
        for name in [
            "jed.2026-10-12.log",
            "jed.2026-10-14.log",
            "other.log",
            "jed.2026-10-13.log",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert_eq!(newest_log(dir.path()), Some(dir.path().join("jed.2026-10-14.log")));
    }

    /// Records each `write` call whole.
    #[derive(Clone, Default)]
    struct Writes(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().push(String::from_utf8_lossy(bytes).into_owned());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn each_line_carries_the_pid_and_is_one_write() {
        let writes = Writes::default();
        let w = writes.clone();
        let (subscriber, _handle) = subscriber(EnvFilter::new("info"), move || w.clone(), crate::logs::Logs::default());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(path = "a.rs", "first");
            tracing::warn!("second");
        });
        let writes = writes.0.lock().unwrap().clone();
        assert_eq!(writes.len(), 2, "one write per event: {writes:?}");
        let pid = std::process::id().to_string();
        for (line, level) in writes.iter().zip(["INFO", "WARN"]) {
            // `<time> <pid>  INFO jed::logging::tests: …\n`
            let mut words = line.split_whitespace();
            assert!(words.next().is_some_and(|t| t.ends_with('Z')), "{line}");
            assert_eq!(words.next(), Some(pid.as_str()), "{line}");
            assert_eq!(words.next(), Some(level), "{line}");
            assert_eq!(line.matches('\n').count(), 1, "{line}");
        }
        assert!(writes[0].ends_with("first path=\"a.rs\"\n"), "{}", writes[0]);
    }

    #[test]
    fn an_event_is_written_when_it_ends() {
        let writes = Writes::default();
        let w = writes.clone();
        let make = WholeEvents(move || w.clone());
        {
            let mut out = make.make_writer();
            out.write_all(b"2026-11-26T00:00:00Z 1 ").unwrap();
            out.write_all(b" INFO jed: hello\n").unwrap();
            out.flush().unwrap();
            assert!(
                writes.0.lock().unwrap().is_empty(),
                "nothing written before the event ends"
            );
        }
        assert_eq!(*writes.0.lock().unwrap(), ["2026-11-26T00:00:00Z 1  INFO jed: hello\n"]);
    }

    #[test]
    fn filter_reloads() {
        let (layer, handle) = reload::Layer::new(EnvFilter::new("info"));
        let _subscriber = tracing_subscriber::registry().with(layer);
        assert_eq!(current_filter(&handle).as_deref(), Some("info"));
        set_filter(&handle, "jed::app=trace").unwrap();
        assert_eq!(current_filter(&handle).as_deref(), Some("jed::app=trace"));
        assert!(set_filter(&handle, "jed::app=loud").is_err());
        assert_eq!(current_filter(&handle).as_deref(), Some("jed::app=trace"));
    }
}
