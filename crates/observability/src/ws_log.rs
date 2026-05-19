//! Tracing subscriber layer for forwarding worker log events.
//!
//! The [`WsLogLayer`] captures tracing events and sends them to a
//! [`std::sync::mpsc::SyncSender<LogEvent>`].  The worker process drains the
//! channel and writes events to stdout as `REMERGE_EVENT:{json}` lines, which
//! the server intercepts from the Docker attach stream and routes into the
//! per-workorder log ring buffer.

use std::sync::mpsc::SyncSender;

use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use remerge_types::api::{LogEvent, LogLevel};
use remerge_types::workorder::WorkorderId;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn to_log_level(level: &tracing::Level) -> LogLevel {
    match *level {
        tracing::Level::ERROR => LogLevel::Error,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::TRACE => LogLevel::Trace,
    }
}

/// Visitor that extracts the `message` field from a tracing event.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

// ─── WsLogLayer ──────────────────────────────────────────────────────────────

/// A [`tracing_subscriber::Layer`] that forwards worker log events to a channel.
///
/// Events whose level exceeds `max_level` (more verbose than requested) are
/// dropped before being enqueued.  All forwarded events are tagged with the
/// provided `workorder_id` so the server can route them into the correct
/// per-workorder ring buffer.
///
/// # Usage (worker)
///
/// ```rust,ignore
/// let (tx, rx) = std::sync::mpsc::sync_channel::<LogEvent>(512);
/// let layer = WsLogLayer::new(tx, workorder_id, LogLevel::Info);
///
/// // Register alongside the existing subscriber…
/// tracing_subscriber::registry().with(layer).with(fmt_layer).init();
///
/// // Drain the channel and emit REMERGE_EVENT: lines to stdout.
/// std::thread::spawn(move || {
///     while let Ok(event) = rx.recv() {
///         let json = serde_json::to_string(&WorkerEventEnvelope::Log(event)).unwrap();
///         println!("REMERGE_EVENT:{json}");
///     }
/// });
/// ```
pub struct WsLogLayer {
    tx: SyncSender<LogEvent>,
    workorder_id: WorkorderId,
    max_level: LogLevel,
}

impl WsLogLayer {
    /// Create a new layer.
    ///
    /// - `tx`: destination channel for captured [`LogEvent`]s.
    /// - `workorder_id`: stamped onto every emitted event for server routing.
    /// - `max_level`: events *more verbose* than this level are silently dropped.
    pub fn new(tx: SyncSender<LogEvent>, workorder_id: WorkorderId, max_level: LogLevel) -> Self {
        Self {
            tx,
            workorder_id,
            max_level,
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for WsLogLayer {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let level = to_log_level(event.metadata().level());

        // Drop events that are more verbose than the requested ceiling.
        if level > self.max_level {
            return;
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);

        let span_name = ctx
            .lookup_current()
            .map(|span_ref| span_ref.name().to_string());

        let log_event = LogEvent {
            level,
            target: event.metadata().target().to_string(),
            message: visitor.0,
            workorder_id: self.workorder_id,
            span: span_name,
            timestamp: chrono::Utc::now(),
        };

        // Best-effort: drop silently if the channel is full or disconnected.
        let _ = self.tx.try_send(log_event);
    }
}
