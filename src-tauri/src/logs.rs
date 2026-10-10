//! In-app log panel feed: a `tracing` layer that keeps the last lines in a
//! ring buffer and forwards each new one to a sink (the UI's `log-line` event).
//! Captures INFO+ from this crate and WARN+ from dependencies.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const CAPACITY: usize = 300;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Unix epoch milliseconds.
    pub ts: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

type Sink = Box<dyn Fn(&LogLine) + Send + Sync>;

#[derive(Default)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<LogLine>>,
    sink: OnceLock<Sink>,
}

impl LogBuffer {
    pub fn recent(&self) -> Vec<LogLine> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Install the live forwarder (once the app handle exists).
    pub fn set_sink(&self, sink: Sink) {
        let _ = self.sink.set(sink);
    }

    fn push(&self, line: LogLine) {
        {
            let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
            if lines.len() == CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        if let Some(sink) = self.sink.get() {
            sink(&line);
        }
    }
}

pub struct LogLayer(pub Arc<LogBuffer>);

thread_local! {
    /// Forwarding a line can itself log (e.g. inside the IPC layer); don't
    /// capture those, or we'd recurse.
    static IN_LAYER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Default)]
struct Fields {
    message: String,
    extra: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.extra, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.extra, " {}={value}", field.name());
        }
    }
}

fn wanted(level: &Level, target: &str) -> bool {
    *level <= Level::WARN || (*level <= Level::INFO && target.starts_with("idm_replica"))
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if !wanted(meta.level(), meta.target()) || IN_LAYER.with(|f| f.get()) {
            return;
        }
        IN_LAYER.with(|f| f.set(true));
        let mut fields = Fields::default();
        event.record(&mut fields);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.0.push(LogLine {
            ts,
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: format!("{}{}", fields.message, fields.extra),
        });
        IN_LAYER.with(|f| f.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    #[test]
    fn captures_filtered_lines_with_fields_and_forwards_them() {
        let buf = Arc::new(LogBuffer::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        buf.set_sink(Box::new(move |l| {
            s2.lock().unwrap().push(l.message.clone())
        }));
        let sub = tracing_subscriber::registry().with(LogLayer(buf.clone()));
        tracing::subscriber::with_default(sub, || {
            tracing::info!(target: "idm_replica::downloader", total = 5, "probe complete");
            tracing::info!(target: "hyper::client", "noisy dependency info");
            tracing::warn!(target: "hyper::client", "dependency warning");
            tracing::debug!(target: "idm_replica", "too chatty");
        });
        let lines = buf.recent();
        let msgs: Vec<&str> = lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(msgs, ["probe complete total=5", "dependency warning"]);
        assert_eq!(lines[1].level, "WARN");
        assert_eq!(*seen.lock().unwrap(), msgs, "each line is forwarded live");
    }

    #[test]
    fn ring_buffer_is_bounded() {
        let buf = LogBuffer::default();
        for i in 0..CAPACITY + 10 {
            buf.push(LogLine {
                ts: 0,
                level: "INFO".into(),
                target: "t".into(),
                message: i.to_string(),
            });
        }
        let r = buf.recent();
        assert_eq!(r.len(), CAPACITY);
        assert_eq!(r[0].message, "10");
    }
}
