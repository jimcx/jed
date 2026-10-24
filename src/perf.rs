//! Reusable perf logging. `--perf` turns on timing logs (target `perf`, info level) around
//! file opens and git reads, so slow operations can be found without raising the log level.
//!
//! `time` wraps a block and logs `label` with the elapsed milliseconds, but only when the flag
//! is on — otherwise it just runs the block (one atomic load per call). Labels are static and
//! dotted, so they group in the log (e.g. `file.open`, `file.open.indentation`, `git.statuses`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns perf logging on (`--perf`). Called in `main` before the runtime starts.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether perf logging is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Times `f` and, when perf logging is on, logs `label` with the elapsed milliseconds. Returns
/// `f`'s value, so it wraps an existing expression or closure.
pub fn time<T>(label: &'static str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let out = f();
    tracing::info!(
        target: "perf",
        label,
        elapsed_ms = start.elapsed().as_millis() as u64,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_returns_the_value_without_logging_when_disabled() {
        assert!(!enabled(), "perf is off unless --perf enables it");
        assert_eq!(time("x", || 42), 42);
    }
}
