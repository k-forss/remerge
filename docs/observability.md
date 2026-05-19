# Observability

Remerge has two complementary telemetry channels: **OTLP tracing** (for
distributed trace data sent to an OpenTelemetry collector) and **WebSocket
log streaming** (for forwarding worker log events to the CLI in real time
over the same progress WebSocket).

---

## OTLP tracing

All three processes — CLI, server, and worker — initialise a
`SdkTracerProvider` via `remerge_observability::init_tracing`.  If the
`OTEL_EXPORTER_OTLP_ENDPOINT` environment variable is set the provider
installs a gRPC batch exporter; otherwise it is a no-op provider that
incurs no overhead.

Span context is propagated from the CLI through the workorder submission
payload (`trace_context.traceparent`) and injected into the worker
container as `REMERGE_TRACEPARENT`.

---

## WebSocket log streaming

While OTLP is the preferred channel for structured trace data, the WebSocket
progress stream also forwards worker tracing log events so operators can
see build-time log output without a full observability stack.

### Architecture

```
Worker process
  tracing subscriber
  └── WsLogLayer (crates/observability/src/ws_log.rs)
        │ SyncSender<LogEvent>
        └── background thread
              │ REMERGE_EVENT:{"type":"log",...}  ← stdout
              ▼
Server (queue.rs — REMERGE_EVENT reader)
  push_log_event(workorder_id, event)
  ├── log_ring_buf[id].push_back(event)   ← replay buffer (256 events)
  └── log_event_tx[id].send(event)        ← broadcast to live WS clients

CLI (client.rs — stream_progress)
  WebSocket text frames
  ├── BuildProgress JSON  → print_event()
  └── LogEvent JSON       → print_log_event()
```

### Log level negotiation

The CLI appends `?log_level=<level>` to the WebSocket URL.  The server
parses this query parameter in the `ws_progress` handler and uses the
resulting `LogLevel` as a ceiling filter when forwarding events:

| CLI verbosity | `?log_level=` | Events forwarded                  |
|---------------|---------------|-----------------------------------|
| `-q` (quiet)  | `error`       | Error only                        |
| (normal)      | `warn`        | Up to Warn (Error, Warn)          |
| `-v`          | `info`        | Up to Info (Error, Warn, Info)    |
| `-vv`         | `debug`       | Up to Debug (Error…Debug)         |
| `-vvv`        | `trace`       | All events (Error…Trace)          |

The worker always emits up to the level configured by
`REMERGE_WORKER_LOG_LEVEL` (default `info`).  The server stores ALL
forwarded events in the ring buffer; each WS connection independently
applies its own ceiling.

### Scope filter invariant

The server only stores events originating from the worker process for a
specific workorder.  The `WsLogLayer` is initialised with the
`REMERGE_WORKORDER_ID` environment variable so every `LogEvent` it emits
carries the correct workorder ID.  The server routes events by that ID
before storing them — there is no cross-workorder leakage.

### Ring buffer replay

When a CLI connects mid-build (or reconnects), the server replays up to
256 buffered log events before switching to live forwarding.  The
snapshot is taken **after** subscribing to the broadcast channel to avoid
a race: any event emitted between the subscribe and snapshot is covered
by the live channel.  Clients applying deduplication on timestamp can
discard any duplicates in the overlap window.

### Data types

```rust
// crates/types/src/api.rs
pub struct LogEvent {
    pub workorder_id: WorkorderId,
    pub level: LogLevel,
    pub target: String,   // tracing span/target name
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(PartialOrd, Ord)]  // Error=0 < Warn=1 < Info=2 < Debug=3 < Trace=4
pub enum LogLevel { Error, Warn, Info, Debug, Trace }
```

---

## Environment variables

| Variable | Used by | Description |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | all | OTLP gRPC endpoint; absent = no-op |
| `REMERGE_TRACEPARENT` | worker | W3C traceparent header from the CLI |
| `REMERGE_WORKORDER_ID` | worker | UUID set by server for log event tagging |
| `REMERGE_WORKER_LOG_LEVEL` | worker | Maximum level emitted by WsLogLayer (default `info`) |
| `REMERGE_LOG_JSON` | CLI | Set to `1` to emit events as NDJSON |
