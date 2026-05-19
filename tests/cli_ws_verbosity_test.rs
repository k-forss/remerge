//! D8 — Integration tests for CLI verbosity levels against a recorded WS
//! session fixture.
//!
//! These tests do NOT require Docker; they spin up a tiny in-process axum
//! server that serves a pre-recorded sequence of WebSocket frames alongside
//! the REST endpoint that `stream_progress` uses to fetch the final result.
//!
//! # What is tested
//!
//! 1. **`?log_level=` query parameter**: `stream_progress` must append the
//!    correct ceiling for each verbosity level so the server knows which events
//!    to forward.
//! 2. **End-to-end WS session completion**: The mock sends a `Finished` event;
//!    the CLI must parse it, fetch the result, and return without error.
//! 3. **`--log-json` mode**: When `log_json = true` the status bar is
//!    suppressed and text frames are emitted as NDJSON — the WS session still
//!    completes successfully.
//!
//! # Fixture
//!
//! The fixture sequence sent by the mock server:
//! 1. `StatusChanged` (WaitingForWorker → Building)
//! 2. `LogEvent` at Error
//! 3. `LogEvent` at Warn
//! 4. `LogEvent` at Info
//! 5. `LogEvent` at Debug
//! 6. `LogEvent` at Trace
//! 7. `Finished` (one built package, none failed)
//!
//! After the `Finished` event the client calls `GET /api/v1/workorders/:id`
//! and the mock returns a complete `WorkorderStatusResponse`.

mod common;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use remerge::client::RemergeClient;
use remerge_types::api::{LogEvent, LogLevel, WorkorderStatusResponse};
use remerge_types::workorder::{
    BuildEvent, BuildProgress, WorkorderId, WorkorderResult, WorkorderStatus,
};
use uuid::Uuid;

// ─── Fixture helpers ─────────────────────────────────────────────────────────

fn fixture_build_progress(workorder_id: WorkorderId, event: BuildEvent) -> String {
    serde_json::to_string(&BuildProgress {
        workorder_id,
        trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string()),
        event,
        timestamp: Utc::now(),
    })
    .unwrap()
}

fn fixture_log_event(workorder_id: WorkorderId, level: LogLevel) -> String {
    serde_json::to_string(&LogEvent {
        level,
        target: "remerge_worker::builder".to_string(),
        message: format!("{level:?} message from worker"),
        workorder_id,
        span: Some("build_packages".to_string()),
        timestamp: Utc::now(),
    })
    .unwrap()
}

fn fixture_result(workorder_id: WorkorderId) -> WorkorderResult {
    WorkorderResult {
        workorder_id,
        built_packages: vec![remerge_types::workorder::BuiltPackage {
            atom: "dev-libs/fixture-1.0".to_string(),
            binpkg_path: "dev-libs/fixture-1.0.gpkg.tar".to_string(),
            sha256: "aabbcc".to_string(),
            size: 1234,
        }],
        failed_packages: vec![],
        binhost_uri: "http://127.0.0.1/binpkgs".to_string(),
        fetched_distfiles: Default::default(),
        parity_manifest: Default::default(),
    }
}

// ─── Mock server state ───────────────────────────────────────────────────────

/// Shared state between the mock axum routes.
#[derive(Clone)]
struct MockState {
    workorder_id: WorkorderId,
    /// The `?log_level=` value sent by the client during the WS upgrade.
    received_log_level: Arc<Mutex<Option<String>>>,
}

/// Query params extracted from the WS upgrade URL.
#[derive(serde::Deserialize)]
struct ProgressParams {
    log_level: Option<String>,
}

/// WS upgrade handler — serves the pre-recorded fixture sequence.
async fn ws_progress_handler(
    State(state): State<MockState>,
    Path(_id): Path<Uuid>,
    Query(params): Query<ProgressParams>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Record the log_level param so tests can assert on it.
    if let Some(ref level) = params.log_level {
        *state.received_log_level.lock().unwrap() = Some(level.clone());
    }

    ws.on_upgrade(move |socket| async move {
        let id = state.workorder_id;
        let (mut send, _recv) = socket.split();

        let frames: Vec<String> = vec![
            fixture_build_progress(
                id,
                BuildEvent::StatusChanged {
                    from: WorkorderStatus::Pending,
                    to: WorkorderStatus::Building,
                },
            ),
            fixture_log_event(id, LogLevel::Error),
            fixture_log_event(id, LogLevel::Warn),
            fixture_log_event(id, LogLevel::Info),
            fixture_log_event(id, LogLevel::Debug),
            fixture_log_event(id, LogLevel::Trace),
            fixture_build_progress(
                id,
                BuildEvent::Finished {
                    built: vec!["dev-libs/fixture-1.0".to_string()],
                    failed: vec![],
                },
            ),
        ];

        for frame in frames {
            let _ = send
                .send(axum::extract::ws::Message::Text(frame.into()))
                .await;
        }
        let _ = send.send(axum::extract::ws::Message::Close(None)).await;
    })
}

/// REST handler — returns a completed workorder status with a result.
async fn workorder_status_handler(
    State(state): State<MockState>,
    Path(_id): Path<Uuid>,
) -> axum::Json<WorkorderStatusResponse> {
    axum::Json(WorkorderStatusResponse {
        workorder_id: state.workorder_id,
        status: WorkorderStatus::Completed,
        result: Some(fixture_result(state.workorder_id)),
        trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string()),
    })
}

// ─── Mock server lifecycle ───────────────────────────────────────────────────

struct MockFixtureServer {
    port: u16,
    base_url: String,
    workorder_id: WorkorderId,
    received_log_level: Arc<Mutex<Option<String>>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl Drop for MockFixtureServer {
    fn drop(&mut self) {
        // Abort the axum serve task instead of leaking it when the test ends.
        self._handle.abort();
    }
}

impl MockFixtureServer {
    async fn start() -> Self {
        let workorder_id = Uuid::new_v4();
        let received_log_level = Arc::new(Mutex::new(None::<String>));

        let state = MockState {
            workorder_id,
            received_log_level: received_log_level.clone(),
        };

        let app = Router::new()
            .route("/api/v1/workorders/{id}/progress", get(ws_progress_handler))
            .route("/api/v1/workorders/{id}", get(workorder_status_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");

        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // Wait until the server is actually accepting connections instead of
        // using a fixed sleep, which is flaky on slow CI runners.
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "mock server did not become ready within 5 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        MockFixtureServer {
            port,
            base_url,
            workorder_id,
            received_log_level,
            _handle: handle,
        }
    }

    fn ws_url(&self) -> String {
        format!(
            "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
            self.port, self.workorder_id
        )
    }

    fn received_log_level(&self) -> Option<String> {
        self.received_log_level.lock().unwrap().clone()
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

fn client(base_url: &str) -> RemergeClient {
    RemergeClient::new(base_url).expect("build test client")
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// `stream_progress` must send `?log_level=error` for Quiet verbosity.
#[tokio::test]
async fn verbosity_quiet_sends_error_log_level() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    let result = c
        .stream_progress(&server.ws_url(), Verbosity::Quiet, false)
        .await
        .expect("stream_progress with Quiet should complete");

    assert_eq!(result.built_packages.len(), 1);
    assert_eq!(
        server.received_log_level().as_deref(),
        Some("error"),
        "Quiet verbosity must request log_level=error ceiling"
    );
}

/// `stream_progress` must send `?log_level=warn` for Normal verbosity.
#[tokio::test]
async fn verbosity_normal_sends_warn_log_level() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    c.stream_progress(&server.ws_url(), Verbosity::Normal, false)
        .await
        .expect("stream_progress with Normal should complete");

    assert_eq!(
        server.received_log_level().as_deref(),
        Some("warn"),
        "Normal verbosity must request log_level=warn ceiling"
    );
}

/// `stream_progress` must send `?log_level=info` for Verbose verbosity.
#[tokio::test]
async fn verbosity_verbose_sends_info_log_level() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    c.stream_progress(&server.ws_url(), Verbosity::Verbose, false)
        .await
        .expect("stream_progress with Verbose should complete");

    assert_eq!(
        server.received_log_level().as_deref(),
        Some("info"),
        "Verbose verbosity must request log_level=info ceiling"
    );
}

/// `stream_progress` must send `?log_level=debug` for VerboseDebug.
#[tokio::test]
async fn verbosity_verbose_debug_sends_debug_log_level() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    c.stream_progress(&server.ws_url(), Verbosity::VerboseDebug, false)
        .await
        .expect("stream_progress with VerboseDebug should complete");

    assert_eq!(
        server.received_log_level().as_deref(),
        Some("debug"),
        "VerboseDebug verbosity must request log_level=debug ceiling"
    );
}

/// `stream_progress` must send `?log_level=trace` for VerboseTrace.
#[tokio::test]
async fn verbosity_verbose_trace_sends_trace_log_level() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    c.stream_progress(&server.ws_url(), Verbosity::VerboseTrace, false)
        .await
        .expect("stream_progress with VerboseTrace should complete");

    assert_eq!(
        server.received_log_level().as_deref(),
        Some("trace"),
        "VerboseTrace verbosity must request log_level=trace ceiling"
    );
}

/// `log_json = true`: session completes successfully regardless of verbosity.
///
/// In JSON mode all text frames are written as NDJSON and binary PTY frames
/// are suppressed.  The WS session must still run to completion.
#[tokio::test]
async fn log_json_mode_session_completes() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    let result = c
        .stream_progress(&server.ws_url(), Verbosity::Normal, true)
        .await
        .expect("stream_progress with log_json=true should complete");

    assert_eq!(
        result.built_packages.len(),
        1,
        "log_json mode must still return the build result"
    );
}

/// Connecting to the progress URL with a log_level that has a query string
/// must not break `workorder_id_from_progress_url` URL parsing — the UUID
/// must be extracted correctly.
///
/// Regression guard for the query-string stripping fix.
#[tokio::test]
async fn progress_url_with_query_string_completes() {
    use remerge::verbosity::Verbosity;

    let server = MockFixtureServer::start().await;
    let c = client(&server.base_url);

    // Use a URL that already has a query string appended; stream_progress will
    // re-append its own log_level param — workorder_id must be found despite
    // the query string on the base URL.
    // Pass a URL that already carries a query string; stream_progress must
    // append `&log_level=` (not `?log_level=` again) so the URL stays valid.
    let base_ws = format!("{}?foo=bar", server.ws_url());
    let result = c
        .stream_progress(&base_ws, Verbosity::Verbose, false)
        .await
        .expect("URL with query string must be handled correctly");

    assert_eq!(result.built_packages.len(), 1);
    // Confirm the server still received the log_level param correctly.
    assert_eq!(
        server.received_log_level().as_deref(),
        Some("info"),
        "log_level param must be forwarded even when base URL has a query string"
    );
}
