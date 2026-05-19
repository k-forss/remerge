# remerge Roadmap — CLI UX & Developer Experience

This file is the active task tracker for the CLI verbosity, feedback, and
operator-experience work on the `f-cli-ux` branch.

The previous roadmap (transport and state-convergence, Phases 0–7) is archived in
`docs/archive/ROADMAP-transport-convergence.md`.

## Goal

The CLI must clearly communicate what it is doing at every stage without
spamming the operator when everything is working normally. Operators must be
able to tune verbosity to match their needs: quiet for unattended automation,
verbose for debugging, and a sensible default in between.

## Decisions

- [✓] Verbosity flags follow portage convention: `-q` for quiet, `-v` for
  verbose, `-vv`/`-vvv` for deeper traces.
- [✓] `-v` flags couple to `RUST_LOG` elevation unless `RUST_LOG` is
  explicitly set by the caller.
- [✓] Verbosity flags are forwarded to the remote emerge invocation so emerge
  itself decides how much output to produce; the CLI does not suppress PTY bytes.
- [✓] A dynamic status bar on stderr provides live phase and elapsed-time
  feedback without cluttering stdout.
- [✓] The status bar is hidden during PTY streaming so raw emerge output
  reaches the terminal unobstructed.
- [✓] `StatusChanged` events update the status bar silently at normal
  verbosity and print a human-readable message at verbose level.
- [✓] Watchdog heartbeats update the status bar phase rather than emitting
  timed `eprintln!` spam; non-TTY environments receive log-line fallback.
- [✓] The status bar uses a 100 ms background redraw loop and cleans up on
  drop to avoid terminal corruption.
- [✓] `crossterm` is the only new dependency; sub-50 KB, no runtime
  overhead outside TTY check calls.
- [✓] Portage `-q` ("reduced or condensed output") maps to: no status bar,
  no phase messages, no sync progress bar, only errors and final result.
  `--quiet-build` is NOT injected — emerge itself respects `--quiet` on its
  PTY output.
- [✓] Portage `-v` ("verbose metadata: USE flags, GNU info, repo sources")
  maps to: status bar + verbose `StatusChanged` prints + trace ID + structured
  server-side log events forwarded back over WS at `--verbose-events` level.
- [✓] `RUST_LOG` at `Normal` should default to `"warn"` (not `"error"`) so
  internal warnings surface without adding debug noise.
- [✓] `emerge --sync` runs with `--quiet --ask=n` hardcoded in the worker;
  at `Verbose+` the `--quiet` flag should be omitted so the operator can see
  sync progress in the PTY stream.
- [✓] At `-vv`, worker internal `tracing::info!` events should be forwarded
  back to the CLI as structured log frames over the progress WebSocket,
  distinct from raw PTY binary frames.
- [✓] The status bar must detect non-TTY stderr and become a no-op (write
  log-line progress instead) to avoid noise in CI pipelines and pipes.
- [✓] Multi-level verbose (`-vv`, `-vvv`) stops injecting repeated
  `--verbose` flags into emerge args; emerge only receives `--verbose` once
  at most — additional levels only elevate the CLI's own `RUST_LOG`.
- [✓] `LogEvent` frames are buffered per-workorder (bounded ring buffer,
  256 events) and replayed to a connecting CLI before switching to live
  forwarding, so late connections (e.g. `remerge watch <id>`) see the full
  log.
- [✓] Server-side events are forwarded only when scoped to the requesting
  client's workorder. Server-wide events (auth failures for other clients,
  pool state, scheduler internals) are never forwarded — the scope filter
  is enforced server-side, not client-side, to prevent information leakage.
- [✓] Verbosity is negotiated at WS upgrade time via a `?log_level=` query
  parameter; the server applies a per-connection ceiling filter before sending
  any `LogEvent` frame, so the client cannot receive log volume it did not
  request and debug events from other workorders never reach the wire.

## Invariants

- [✓] Quiet mode (`-q`) suppresses non-error CLI output.
- [✓] Default mode shows phase transitions and final results only.
- [✓] Verbose mode (`-v`) adds `StatusChanged` transitions, trace IDs, and
  tool-level messages.
- [✓] Deeper verbose levels (`-vv`, `-vvv`) elevate `RUST_LOG` to `debug`
  and `trace`.
- [✓] PTY bytes from the remote build always reach stdout unmodified
  regardless of verbosity level.
- [✓] The status bar never interleaves with stdout output.
- [✓] Non-TTY environments (CI, pipes) receive log-line progress instead of
  status bar redraws in all phases.
- [✓] `EMERGE_DEFAULT_OPTS` containing `-q` or `--verbose` is respected and
  treated as the user's preferred verbosity when no explicit CLI flag overrides
  it; both the CLI's own output level and the injected emerge arg are derived
  from this fallback.
- [✓] Worker internal log events are never silently discarded at `-vv` —
  they are forwarded back to the CLI as structured WS text frames alongside
  PTY binary frames so the operator has full visibility without needing to
  connect to a separate log aggregator.

## Phase A: Verbosity Infrastructure

- [✓] Add `-q`/`-v` CLI flags with portage-compatible semantics.
  - [✓] `-q` and `-v` are mutually exclusive (`conflicts_with`).
  - [✓] `-v` uses `clap::ArgAction::Count` for graduated levels.
- [✓] Add `Verbosity` enum: `Quiet / Normal / Verbose / VerboseDebug / VerboseTrace`.
  - [✓] `from_flags(quiet, verbose_count, emerge_default_opts)` — derives
    from CLI flags, falls back to `EMERGE_DEFAULT_OPTS` portage env.
  - [✓] `early_detect()` — pre-clap argv scan used in `main()` before
    tracing init.
  - [✓] `rust_log_level()` — maps enum to `"error"/"warn"/"info"/"debug"/"trace"`.
  - [✓] `emerge_flag()` — returns `Some("--quiet")`, `None`, or
    `Some("--verbose")` to inject into emerge args.
  - [✓] `is_verbose()` / `is_quiet()` predicate helpers.
- [✓] Early-detect verbosity from `argv` before clap parsing so `RUST_LOG`
  is set before `init_tracing()`.
- [✓] Set `RUST_LOG` via `std::env::set_var` in `main()` only when the
  caller has not already set it.
- [✓] `workorder_emerge_args()` helper injects verbosity flag into the
  emerge arg list without duplicating an already-present `--quiet`/`--verbose`.
- [✓] Add `crossterm = "0.28"` to `crates/cli/Cargo.toml`.

## Phase B: Dynamic Status Bar

- [✓] Add `StatusBar` struct with `Arc<Mutex<State>>` internal state.
- [✓] `OnceLock<Arc<StatusBar>>` global singleton; `init()` sets it,
  `global()` returns `Option<Arc<StatusBar>>` for call sites that tolerate
  absence (tests, non-TTY).
- [✓] Background 100 ms `tokio::spawn` redraw task held via
  `Weak<Mutex<State>>` so it does not keep the bar alive past `finish()`.
- [✓] `crossterm::terminal::size()` for width-aware phase text truncation.
- [✓] Elapsed-time suffix appended to phase label on redraw (e.g.
  `Checking snapshot blobs… 3s`).
- [✓] ANSI dim styling (`\x1b[2m…\x1b[0m`) for visual hierarchy.
- [✓] `\r\x1b[2K` overwrite strategy — no scrollback pollution.
- [✓] `hide()` / `show()` pair for suppressing the bar during PTY relay.
- [✓] `finish()` — clears bar and sets `finished` flag to stop redraws.
- [✓] `println(msg)` — atomically clears bar, prints message, redraws bar
  so concurrent output does not interleave.
- [✓] `Drop` impl calls `clear_line()` to clean up on panic or early exit.
- [✓] Bar initialized in `main()` after `init_tracing()`.
- [✓] Error path in `main()` calls `bar.finish()` before printing the error.

## Phase C: Phase Messages and Output Quality

- [✓] Phase messages added throughout `run()`:
  - [✓] `"Expanding package sets…"` before atom set reader.
  - [✓] `"Checking installed packages…"` before VDB scan.
  - [✓] `"Reading portage configuration…"` before portage config read.
  - [✓] `"Checking snapshot blobs…"` before blob negotiation.
  - [✓] `"Submitting workorder…"` before submit call.
  - [✓] `"Waiting for build to start…"` before `stream_progress`.
  - [✓] `"Syncing binary packages…"` before `complete_local_followup`.
- [✓] Verbosity derived from portage config `EMERGE_DEFAULT_OPTS` after
  the config is read, so portage-set flags are respected.
- [✓] Dry-run path calls `bar.finish()` before printing result lines.
- [✓] Trace ID print gated on `verbosity.is_verbose()`.
- [✓] Already-installed package messages routed through `bar.println()`.
- [✓] Per-blob upload progress in `prepare_manifest_submission()`:
  `"Checking snapshot blobs ({N} repo snapshots)…"` and
  `"Uploading snapshot blob {i}/{total}…"`.
- [✓] `complete_local_followup()` accepts `bar: Option<&StatusBar>` param;
  uses bar for phase and completion messages; hides bar before local emerge
  starts; calls `bar.finish()` on all exit paths.
- [✓] `run_with_watchdog()` heartbeat tick updates bar phase with elapsed
  time (`"{stage} ({N}s)…"`); falls back to `eprintln!` on non-TTY.
- [✓] `stream_progress()` accepts `verbosity: Verbosity` param.
- [✓] Status bar hidden on first PTY binary frame in `stream_progress()`
  via a `bar_hidden` one-shot flag.
- [✓] `print_event()` accepts `verbosity: Verbosity`:
  - [✓] At normal verbosity: `StatusChanged` silently updates bar phase
    with human-readable string (e.g. `"Remote build: building packages…"`).
  - [✓] At verbose: also prints the friendly string to stderr.
  - [✓] Raw `{:?}` debug format replaced with mapped human-readable strings.
- [✓] `test_cli()` fixture updated with `quiet: false, verbose: 0` fields.
- [✓] Full workspace `cargo check` passes clean (`EXIT:0`).

---

## Phase D — Portage Verbosity Alignment

*Goal: ensure remerge's verbosity model is a faithful extension of portage's
own conventions so operators with portage muscle memory get exactly what they
expect.*

### Background

From the portage man page:

| Flag | portage behaviour |
|------|-------------------|
| `--quiet` / `-q` | "Results may vary, but the general outcome is a **reduced or condensed output**." Does NOT silence build output — only condenses it. Separate from `--quiet-build`. |
| `--verbose` / `-v` | "Currently this flag causes emerge to print out GNU info errors, if any, and to show the **USE flags** that will be used for each package when pretending." Primarily a metadata-richness flag, not a debug-dump flag. |
| `--quiet-build` | Redirects all build output to logs. Distinct from `--quiet`. We do NOT inject this at any level — raw emerge PTY output is always passed through to the operator. |
| `EMERGE_DEFAULT_OPTS` | User's persistent defaults in `make.conf`. We already parse this for verbosity fallback. |

portage "default" output is everything a user needs to monitor a build: package
names, fetch progress, compilation lines, and a final summary. That is what our
PTY relay already provides. Our CLI layers phase messages and a status bar on top.

### Identified gaps

1. **`rust_log_level()` Normal → `"error"`** — Should be `"warn"` so internal
   warnings surface (e.g. "HMAC key missing", "pool exhausted") without adding
   debug noise at default verbosity.
2. **`emerge --sync --quiet`** — The worker hard-codes `--quiet --ask=n` for
   the portage sync step in `crates/worker/src/builder.rs`. At `Verbose+` the
   `--quiet` should be stripped so the operator can see rsync/sync progress in
   the PTY stream, matching what `emerge --sync` shows locally.
3. **crossdev `--quiet`** — Similarly hard-coded in `crates/worker/src/crossdev.rs`
   for cross-compilation setup. Should be conditional.
4. **`-vv`/`-vvv` don't inject repeated `--verbose`** — emerge only respects
   one `--verbose` flag; extra levels must only elevate `RUST_LOG`.
5. **Quiet mode and `SyncProgressReporter`** — At `-q` the sync progress bar
   should be suppressed entirely; only a one-line "syncing portage tree…" and
   final status should print.
6. **Non-TTY detection** — `StatusBar::init()` must detect non-TTY stderr and
   return a no-op path. CI environments, `tee` pipes, and `--quiet` combined
   must not receive raw ANSI escape codes.
7. **Watchdog heartbeats** — Should use `tracing::info!` not raw `eprintln!`
   so they participate in the structured log pipeline and can be silenced via
   `RUST_LOG`.

### Tasks

- [✓] D1 · Change `Verbosity::Normal` `rust_log_level()` return value from
  `"error"` to `"warn"`.
- [✓] D2 · In `crates/worker/src/builder.rs`, make sync step `--quiet` flag
  conditional: omit it when `emerge_args` contains `--verbose` or `-v`.
- [✓] D3 · Same treatment for `crates/worker/src/crossdev.rs`.
- [✓] D4 · Guard repeated `--verbose` injection: `workorder_emerge_args()`
  already deduplicates; add comment + test asserting at most one verbosity flag.
- [✓] D5 · In `complete_local_followup()` and any code using
  `SyncProgressReporter`, gate writes on `!verbosity.is_quiet()`.
- [✓] D6 · `StatusBar::init()` returns a `StatusBar` whose methods are no-ops
  when either `is_quiet()` or not a TTY; distinguish the two cases so
  CI-friendly one-line progress messages can be printed instead.
- [✓] D7 · Replace `eprintln!` watchdog fallback with `tracing::info!`.
- [✓] D8 · Integration tests: assert key lines present/absent for each of the
  four verbosity levels against a recorded WS session fixture.

---

## Phase E — Server / Worker Tracing → CLI Visibility

*Goal: give the CLI operator meaningful structured feedback from inside the
remote worker at appropriate verbosity levels, without requiring a separate
OTLP stack.*

### Architecture

The WebSocket progress stream already carries two frame types:

- **Binary frames** — raw PTY bytes from the emerge process.
- **Text frames** — `BuildEvent` JSON (currently: `StatusChanged`,
  `Heartbeat`, `Complete`).

A third class of text frame, **`LogEvent`**, can be added to forward
`tracing::warn!` / `tracing::info!` records from the server and worker back to
the CLI. At normal verbosity these are discarded; at `-v` they are printed as
annotated lines; at `-vv` they are streamed live; at `-vvv` debug records
follow.

This avoids adding OpenTelemetry infrastructure requirements for local
development while making OTLP side-by-side export still work for production.

### Trace context propagation (already in place)

- CLI attaches W3C `traceparent` header when submitting a workorder.
- Server parses the header and stores a `TraceContext` on the `Workorder`.
- Worker reads `REMERGE_TRACEPARENT` env var and calls
  `set_span_parent()` to link its spans into the client's trace.
- The CLI displays the trace ID at verbose level so operators can correlate
  with an OTLP backend.

### New work

**E1 · `LogEvent` WS frame type**

Add to `crates/types/src/api.rs`:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct LogEvent {
    pub level: LogLevel,       // Error, Warn, Info, Debug, Trace
    pub target: String,        // Rust module path (e.g. "remerge_worker::builder")
    pub message: String,
    pub workorder_id: Uuid,
    pub span: Option<String>,  // current span name if any
    pub timestamp: DateTime<Utc>,
}
```

**E2 · Worker `tracing` subscriber layer for WS forwarding**

Add an optional `WsLogLayer` in `crates/observability/src/ws_log.rs` that holds a
`tokio::sync::mpsc::Sender<LogEvent>`. Worker init passes it a channel
connected to the existing progress-event sender. The layer is registered in
`init_tracing` only when `WS_LOG_LEVEL` is `Some`.

**E3 · Server bridges worker log events into progress stream — with scope filtering**

The server maintains a bounded ring buffer (capacity: N events, e.g. 256) per
workorder. Two sources feed it:

1. **Worker-scoped events**: forwarded verbatim from `WsLogLayer` over the
   Docker attach channel or a side-channel IPC socket. All targets beginning
   with `remerge_worker::` are eligible.
2. **Server-scoped events**: only events whose `workorder_id` field matches the
   requesting client's workorder are forwarded. Server-wide events (connection
   pool exhaustion, auth failures for other clients, scheduler state) are never
   forwarded regardless of log level — they may leak other clients' identities
   or server internals.

The filter is enforced server-side before the frame leaves the per-workorder
WebSocket handler, not client-side. The client receives only what it owns.

**Buffer and replay**: when a CLI connects (or reconnects) to the progress
WebSocket, the server replays the buffered `LogEvent` frames for that workorder
before switching to live forwarding. This ensures late-connecting CLIs (e.g.
`remerge watch <id>`) see the full log for the build, not just what was
produced after they connected. Buffer is per-workorder and discarded on
`Complete` + a configurable TTL (default 10 minutes).

**Verbosity negotiation**: the CLI includes its current verbosity level in the
WebSocket handshake (as a query parameter or header, e.g.
`?log_level=info`). The server uses this to gate which buffered and live
`LogEvent` frames it actually sends — it does not rely on the client to
discard. This prevents leaking debug-level log volume to operators who did not
ask for it.

**E4 · CLI `print_log_event()`**

In `client.rs`, add a match arm for `LogEvent` frames:

| Verbosity | Behaviour |
|-----------|-----------|
| Quiet | discard |
| Normal | `Warn`/`Error` only → print prefixed with `▲ warn:` / `✗ error:` |
| Verbose | `Info`+above → print prefixed with module path |
| VerboseDebug | `Debug`+above |
| VerboseTrace | `Trace`+above |

The client still applies local filtering as a defence-in-depth layer, but the
server's per-connection filter (E3) is the authoritative gate.

**E5 · Trace ID display**

At `Verbose+`, print the trace ID as a footnote after the final outcome line:

```
trace: 4bf92f3577b34da6a3ce929d0e0e4736
```

so the operator can paste it into Jaeger/Tempo if OTLP is configured.

**E6 · `--log-json` / `REMERGE_LOG_JSON` flag (CLI)**

Expose the server's existing `log_json` flag to CLI operator mode: when set,
all structured frames (both `BuildEvent` and `LogEvent`) are emitted to stdout
as newline-delimited JSON instead of human-readable text. Status bar is
suppressed. This makes `remerge` suitable for CI log-capture tooling.

### Tasks

- [✓] E1 · Add `LogEvent` type + `LogLevel` enum to `crates/types/src/api.rs`.
- [✓] E2 · Implement `WsLogLayer` tracing subscriber layer in
  `crates/observability`.
- [✓] E3a · Add per-workorder `LogEvent` ring buffer (bounded, 256 events) in
  the server's workorder state.
- [✓] E3b · Implement scope filter: only forward events with target prefix
  `remerge_worker::` or events explicitly tagged with the matching
  `workorder_id`; discard all server-internal events.
- [✓] E3c · Implement verbosity negotiation: read `?log_level=` from WS
  upgrade request and apply a per-connection `LogLevel` ceiling filter before
  forwarding any frame.
- [✓] E3d · Implement buffer replay: on WS connect, drain the ring buffer
  (filtered by log level ceiling) before switching to live forwarding.
- [✓] E4 · Add `print_log_event()` to `client.rs` with the verbosity dispatch
  table above (client-side defence-in-depth filter).
- [✓] E5 · Print trace ID footnote in `stream_progress()` at `Verbose+`.
- [✓] E6 · Pass verbosity as `?log_level=` query param in the WS upgrade URL
  built by `stream_progress()`.
- [✓] E7 · Add `--log-json` flag and JSON output mode.
- [✓] E8 · Document OTLP vs WS-log duality in `docs/observability.md`: OTLP
  for production, WS log forwarding for development / `-vv`. Document the
  scope filter invariant (client only sees its own workorder's events).

## Immediate Next Slice

- [✓] D4 — assert at most one verbosity flag in `workorder_emerge_args()` (+ unit test).
- [✓] D8 — add integration test fixtures for verbosity levels.
- [✓] E2 — implement `WsLogLayer` tracing subscriber in `crates/observability`.
- [✓] E3a — per-workorder `LogEvent` ring buffer in server workorder state.
- [✓] E3b — scope filter (target prefix `remerge_worker::` or matching `workorder_id`).
- [✓] E3c — verbosity negotiation: read `?log_level=` at WS upgrade.
- [✓] E3d — buffer replay on WS connect.
- [✓] E4 — `print_log_event()` in `client.rs` with verbosity dispatch table.
- [✓] E5 — trace ID footnote in `stream_progress()` at `Verbose+`.
- [✓] E6 — pass verbosity as `?log_level=` query param in WS upgrade URL.
- [✓] E7 — add `--log-json` flag and JSON output mode.
- [✓] E8 — `docs/observability.md` documenting OTLP vs WS-log duality.
