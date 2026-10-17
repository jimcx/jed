//! The "jed" tool's log: a `tracing` layer that pushes every event it sees to the store, after
//! the same filter as the log file (`logging::init` puts it behind the reload filter), so the ring
//! holds what `jed.<date>.log` does: `target: message field=value …`.

use std::fmt::Write;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::{Level, Logs, Tool};

/// Feeds jed's own events into `logs`' "jed" ring.
pub struct JedLayer {
    logs: Logs,
}

pub fn layer(logs: Logs) -> JedLayer {
    JedLayer { logs }
}

impl<S: Subscriber> Layer<S> for JedLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut text = Text::default();
        event.record(&mut text);
        let line = format!("{}: {}{}", meta.target(), text.message, text.fields);
        self.logs.push(&Tool::Jed, Level::from_tracing(*meta.level()), &line);
    }
}

/// An event's message and its other fields as ` name=value`.
#[derive(Default)]
struct Text {
    message: String,
    fields: String,
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn events_at_the_filter_level_land_in_jed() {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("info"))
            .with(layer(logs.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "jed::app", path = "a.rs", n = 3, "saved");
            tracing::debug!(target: "jed::app", "too quiet");
            tracing::error!(target: "jed::git", "broken {}", 42);
        });
        let lines = logs.lines(&Tool::Jed);
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["jed::app: saved path=a.rs n=3", "jed::git: broken 42"]);
        assert_eq!(lines[1].level, Level::Error);
        assert_eq!(logs.errors(&Tool::Jed), 1);
    }
}
