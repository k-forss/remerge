//! Phase 4 — Server API tests (in-process HTTP).
//!
//! Tests the axum HTTP API. Requires Docker to be available.
//! Tests are gated behind the `integration` feature flag to prevent
//! accidental omission in default CI. Docker and gpg are hard requirements
//! for this suite.

mod common;

#[cfg(feature = "integration")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "integration")]
use remerge::client::RemergeClient;
#[cfg(feature = "integration")]
use remerge_types::api::*;
#[cfg(feature = "integration")]
use sha2::{Digest, Sha256};
#[cfg(feature = "integration")]
use std::collections::BTreeMap;
#[cfg(feature = "integration")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "integration")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "integration")]
async fn submit_workorder_for_test(
    server: &common::server::TestServer,
    req: &SubmitWorkorderRequest,
) -> SubmitWorkorderResponse {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(req)
        .send()
        .await
        .expect("submit workorder request");
    assert_eq!(resp.status(), 200, "workorder submission should succeed");
    resp.json::<SubmitWorkorderResponse>()
        .await
        .expect("parse submit workorder response")
}

#[cfg(feature = "integration")]
fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(feature = "integration")]
fn blob_stream_ws_url(base_url: &str) -> String {
    if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{rest}/api/v1/snapshots/blobs/stream")
    } else {
        format!(
            "ws://{}/api/v1/snapshots/blobs/stream",
            base_url.trim_start_matches("http://")
        )
    }
}

/// Sentinel: Docker must be available when running integration tests.
#[cfg(feature = "integration")]
#[test]
fn docker_must_be_available_for_integration() {
    assert!(
        common::server::docker_available(),
        "Docker is required for integration tests but was not found"
    );
}

/// Helper to enforce Docker availability for integration tests.
#[cfg(feature = "integration")]
fn require_docker() {
    assert!(
        common::server::docker_available(),
        "Docker is required for integration tests but was not found"
    );
}

#[cfg(feature = "integration")]
fn gpg_available() -> bool {
    std::process::Command::new("gpg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(feature = "integration")]
fn require_gpg() {
    assert!(
        gpg_available(),
        "gpg is required for signing integration tests but was not found"
    );
}

#[cfg(feature = "integration")]
fn generate_signing_test_key() -> (tempfile::TempDir, String) {
    let gpg_home = tempfile::TempDir::new().expect("temp gpg home");
    let gpg_home_str = gpg_home.path().to_string_lossy().to_string();

    let status = std::process::Command::new("gpg")
        .args([
            "--homedir",
            &gpg_home_str,
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-gen-key",
            "remerge integration signing <integration@example.invalid>",
            "ed25519",
            "sign",
            "0",
        ])
        .status()
        .expect("run gpg --quick-gen-key");
    assert!(status.success(), "gpg test key generation should succeed");

    let output = std::process::Command::new("gpg")
        .args([
            "--homedir",
            &gpg_home_str,
            "--batch",
            "--list-secret-keys",
            "--with-colons",
        ])
        .output()
        .expect("list test signing key");
    assert!(
        output.status.success(),
        "gpg should list the generated signing key"
    );

    let fingerprint = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.starts_with("fpr:"))
        .and_then(|line| line.split(':').nth(9))
        .map(str::to_string)
        .expect("generated signing key fingerprint");

    (gpg_home, fingerprint)
}

#[cfg(feature = "integration")]
const TEST_CERT_FINGERPRINT: &str = "sha256:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";

#[cfg(feature = "integration")]
fn auth_config(mode: remerge_types::auth::AuthMode) -> remerge_server::config::ServerConfig {
    remerge_server::config::ServerConfig {
        auth: remerge_server::auth::AuthConfig {
            mode,
            clients: vec![remerge_server::auth::CertEntry {
                fingerprint: TEST_CERT_FINGERPRINT.into(),
                client_id: uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
                    .expect("valid uuid"),
                role: remerge_types::client::ClientRole::Main,
                label: Some("integration-client".into()),
            }],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(feature = "integration")]
fn cert_headers(fingerprint: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-Client-Cert-Fingerprint",
        fingerprint.parse().expect("valid fingerprint header"),
    );
    headers
}

#[cfg(feature = "integration")]
fn write_binpkg_fixture(server: &common::server::TestServer, name: &str, body: &str) {
    std::fs::write(server.state.config.binpkg_dir.join(name), body).expect("write binpkg fixture");
}

#[cfg(feature = "integration")]
async fn metrics_body(server: &common::server::TestServer) -> String {
    let resp = reqwest::get(format!("{}/metrics", server.base_url))
        .await
        .expect("metrics request");
    assert_eq!(resp.status(), 200);
    resp.text().await.expect("metrics body")
}

#[cfg(feature = "integration")]
fn metric_value(body: &str, name: &str) -> Option<u64> {
    body.lines()
        .find_map(|line| {
            let (metric, value) = line.split_once(' ')?;
            (metric == name).then_some(value)
        })
        .and_then(|value| value.parse().ok())
}

/// GET /api/v1/health returns 200 with status "ok".
#[cfg(feature = "integration")]
#[tokio::test]
async fn health_endpoint() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let resp = reqwest::get(format!("{}/api/v1/health", server.base_url))
        .await
        .expect("health request");
    assert_eq!(resp.status(), 200);

    let health: HealthResponse = resp.json().await.expect("parse health");
    assert_eq!(health.status, "ok");
    assert!(!health.version.is_empty());
}

/// GET /api/v1/info returns server info with version and auth_mode.
#[cfg(feature = "integration")]
#[tokio::test]
async fn info_endpoint() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let resp = reqwest::get(format!("{}/api/v1/info", server.base_url))
        .await
        .expect("info request");
    assert_eq!(resp.status(), 200);

    let info: ServerInfoResponse = resp.json().await.expect("parse info");
    assert!(!info.version.is_empty());
    assert!(!info.binhost_base_url.is_empty());
    assert_eq!(info.auth_mode, remerge_types::auth::AuthMode::None);
    assert!(!info.binpkg_signing);
    assert_eq!(info.signing_key_fingerprint, None);
    assert_eq!(info.signing_key_endpoint, None);
}

/// GET /api/v1/signing-key returns 404 when signing is disabled.
#[cfg(feature = "integration")]
#[tokio::test]
async fn signing_key_endpoint_disabled() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let resp = reqwest::get(format!("{}/api/v1/signing-key", server.base_url))
        .await
        .expect("signing-key request");
    assert_eq!(resp.status(), 404);
}

/// GET /api/v1/signing-key returns the public key when signing is enabled.
#[cfg(feature = "integration")]
#[tokio::test]
async fn signing_key_endpoint_enabled() {
    require_docker();
    require_gpg();

    let (gpg_home, fingerprint) = generate_signing_test_key();
    let config = remerge_server::config::ServerConfig {
        signing: remerge_server::config::SigningConfig {
            gpg_key: Some(fingerprint.clone()),
            gpg_home: Some(gpg_home.path().to_string_lossy().to_string()),
        },
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let info = reqwest::get(format!("{}/api/v1/info", server.base_url))
        .await
        .expect("info request")
        .json::<ServerInfoResponse>()
        .await
        .expect("parse info response");
    assert!(info.binpkg_signing, "signing should be reported as enabled");
    assert_eq!(
        info.signing_key_fingerprint,
        Some(fingerprint.clone()),
        "info endpoint should publish the signing-key fingerprint"
    );
    assert_eq!(
        info.signing_key_endpoint.as_deref(),
        Some("/api/v1/signing-key")
    );

    let resp = reqwest::get(format!("{}/api/v1/signing-key", server.base_url))
        .await
        .expect("signing-key request");
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/pgp-keys")),
        "signing-key endpoint should return application/pgp-keys"
    );

    let body = resp.text().await.expect("signing-key body");
    assert!(
        body.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "signing-key endpoint should return an ASCII-armored public key"
    );
}

/// GET /metrics returns Prometheus-formatted text with remerge_ prefix.
#[cfg(feature = "integration")]
#[tokio::test]
async fn metrics_endpoint() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let resp = reqwest::get(format!("{}/metrics", server.base_url))
        .await
        .expect("metrics request");
    assert_eq!(resp.status(), 200);

    let body = resp.text().await.expect("body");
    assert!(
        body.contains("remerge_"),
        "metrics should have remerge_ prefix"
    );
    assert!(
        body.contains("remerge_workorders_submitted_total"),
        "should have workorders metric"
    );
    assert!(
        body.contains("remerge_worker_image_build_duration_seconds_total"),
        "should expose worker image build duration metrics"
    );
    assert!(
        body.contains("remerge_worker_container_startup_duration_seconds_total"),
        "should expose worker startup duration metrics"
    );
    assert!(
        body.contains("remerge_package_build_duration_seconds_by_atom_total"),
        "should expose best-effort per-package timing metrics"
    );
    assert!(
        body.contains("remerge_cleanup_success_total"),
        "should expose cleanup outcome metrics"
    );
}

/// Queue depth metric increases for pending workorders and returns to zero when claimed.
#[cfg(feature = "integration")]
#[tokio::test]
async fn queue_depth_metric_tracks_pending_workorders() {
    require_docker();
    let server =
        common::server::TestServer::start_with_config(Some(remerge_server::config::ServerConfig {
            max_workers: 0,
            ..Default::default()
        }))
        .await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 200, "submit should return 200");

    let body = metrics_body(&server).await;
    assert_eq!(
        metric_value(&body, "remerge_queue_depth"),
        Some(1),
        "queue depth should reflect the pending workorder"
    );

    let queue_state = server.state.clone();
    let queue_handle = tokio::spawn(async move {
        remerge_server::queue::process_queue(queue_state).await;
    });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let body = metrics_body(&server).await;
        if metric_value(&body, "remerge_queue_depth") == Some(0) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "queue depth did not return to zero after the queue processor claimed the workorder"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    queue_handle.abort();
}

/// POST /api/v1/workorders with valid atoms returns 200 and workorder ID.
#[cfg(feature = "integration")]
#[tokio::test]
async fn submit_workorder_valid() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 200, "submit should return 200");

    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
    assert!(
        !submit_resp.workorder_id.is_nil(),
        "workorder ID should be set"
    );
    assert!(
        !submit_resp.progress_ws_url.is_empty(),
        "WebSocket URL should be set"
    );
}

/// POST /api/v1/snapshots/missing-blobs reports which digests are absent.
#[cfg(feature = "integration")]
#[tokio::test]
async fn missing_blobs_endpoint_reports_only_absent_digests() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let present_bytes = b"present-blob";
    let present_digest = sha256_hex(present_bytes);
    let missing_digest = sha256_hex(b"missing-blob");
    remerge_server::blob_store::store_blob(server.state.config.state_dir.as_path(), present_bytes)
        .await
        .expect("store present blob");

    let req = FindMissingBlobsRequest {
        digests: vec![present_digest.clone(), missing_digest.clone()],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "{}/api/v1/snapshots/missing-blobs",
            server.base_url
        ))
        .json(&req)
        .send()
        .await
        .expect("missing-blobs request");
    assert_eq!(resp.status(), 200);

    let body: FindMissingBlobsResponse = resp.json().await.expect("parse response");
    assert_eq!(body.missing_digests, vec![missing_digest]);
}

/// PUT /api/v1/snapshots/blobs/{digest} stores a verified blob and is idempotent.
#[cfg(feature = "integration")]
#[tokio::test]
async fn upload_blob_endpoint_stores_verified_blob() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let bytes = b"upload-me";
    let digest = sha256_hex(bytes);
    let client = reqwest::Client::new();

    let resp = client
        .put(format!(
            "{}/api/v1/snapshots/blobs/{digest}",
            server.base_url
        ))
        .body(bytes.to_vec())
        .send()
        .await
        .expect("upload blob request");
    assert_eq!(resp.status(), 200);
    let uploaded: UploadBlobResponse = resp.json().await.expect("parse upload response");
    assert_eq!(uploaded.digest, digest);
    assert!(uploaded.uploaded);

    let resp = client
        .put(format!(
            "{}/api/v1/snapshots/blobs/{}",
            server.base_url, uploaded.digest
        ))
        .body(bytes.to_vec())
        .send()
        .await
        .expect("second upload blob request");
    assert_eq!(resp.status(), 200);
    let second: UploadBlobResponse = resp.json().await.expect("parse second upload response");
    assert!(!second.uploaded);
}

/// PUT /api/v1/snapshots/blobs/{digest} rejects mismatched content.
#[cfg(feature = "integration")]
#[tokio::test]
async fn upload_blob_endpoint_rejects_digest_mismatch() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let digest = sha256_hex(b"expected-bytes");
    let client = reqwest::Client::new();
    let resp = client
        .put(format!(
            "{}/api/v1/snapshots/blobs/{digest}",
            server.base_url
        ))
        .body(b"wrong-bytes".to_vec())
        .send()
        .await
        .expect("upload mismatch request");
    assert_eq!(resp.status(), 400);
}

/// GET /api/v1/snapshots/blobs/{digest} serves a zstd sidecar when the client accepts it.
#[cfg(feature = "integration")]
#[tokio::test]
async fn download_blob_endpoint_serves_zstd_variant_when_requested() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let payload = vec![b'a'; 256 * 1024];
    let digest = sha256_hex(&payload);
    remerge_server::blob_store::store_blob(server.state.config.state_dir.as_path(), &payload)
        .await
        .expect("store compressible blob");
    let metadata = remerge_server::blob_store::load_blob_metadata(
        server.state.config.state_dir.as_path(),
        &digest,
    )
    .await
    .expect("load blob metadata");
    assert!(
        metadata
            .encoded_variants
            .contains_key(&remerge_server::blob_store::BlobEncoding::Zstd)
    );

    let client = reqwest::Client::builder()
        .no_zstd()
        .build()
        .expect("build reqwest client");
    let resp = client
        .get(format!(
            "{}/api/v1/snapshots/blobs/{digest}",
            server.base_url
        ))
        .header(reqwest::header::ACCEPT_ENCODING, "zstd")
        .send()
        .await
        .expect("download blob request");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("zstd")
    );

    let encoded_bytes = resp.bytes().await.expect("read encoded blob body");
    let decoded = zstd::stream::decode_all(std::io::Cursor::new(encoded_bytes))
        .expect("decode zstd blob response");
    assert_eq!(decoded, payload);
}

/// GET /api/v1/snapshots/blobs/{digest} falls back to raw bytes when no zstd variant exists.
#[cfg(feature = "integration")]
#[tokio::test]
async fn download_blob_endpoint_falls_back_to_raw_payload() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let payload = b"small-raw-blob";
    let digest = sha256_hex(payload);
    remerge_server::blob_store::store_blob(server.state.config.state_dir.as_path(), payload)
        .await
        .expect("store small blob");

    let client = reqwest::Client::builder()
        .no_zstd()
        .build()
        .expect("build reqwest client");
    let resp = client
        .get(format!(
            "{}/api/v1/snapshots/blobs/{digest}",
            server.base_url
        ))
        .header(reqwest::header::ACCEPT_ENCODING, "zstd")
        .send()
        .await
        .expect("download blob request");
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .is_none()
    );
    assert_eq!(
        resp.bytes().await.expect("read raw blob body").as_ref(),
        payload
    );
}

/// The CLI blob streamer does not advance send state speculatively when an ack is lost.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_client_retries_from_confirmed_resume_point_after_missing_ack() {
    use axum::{
        Router,
        extract::{State, WebSocketUpgrade, ws},
        response::IntoResponse,
        routing::get,
    };

    #[derive(Clone)]
    struct TestState {
        attempts: Arc<AtomicUsize>,
        seen_frames: Arc<Mutex<Vec<(u64, u64)>>>,
    }

    async fn test_ws_handler(
        State(state): State<TestState>,
        ws: WebSocketUpgrade,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| async move {
            let (mut write, mut read) = socket.split();

            let init = read
                .next()
                .await
                .expect("init frame")
                .expect("init message");
            let init = match init {
                ws::Message::Text(text) => {
                    serde_json::from_str::<SnapshotBlobClientControlMessage>(&text)
                        .expect("parse upload_init")
                }
                other => panic!("expected upload_init text frame, got {other:?}"),
            };
            let SnapshotBlobClientControlMessage::UploadInit {
                version,
                workorder_id,
                digest,
                total_size_bytes,
                chunk_size_bytes,
                ..
            } = init;

            let attempt = state.attempts.fetch_add(1, Ordering::SeqCst);
            write
                .send(ws::Message::Text(
                    serde_json::to_string(&SnapshotBlobServerControlMessage::UploadResume {
                        version,
                        workorder_id,
                        digest: digest.clone(),
                        next_offset_bytes: 0,
                        next_sequence: 0,
                        selected_encoding: None,
                        expected_size_bytes: total_size_bytes,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .expect("send upload_resume");

            let chunk = read
                .next()
                .await
                .expect("chunk frame")
                .expect("chunk message");
            let chunk = match chunk {
                ws::Message::Binary(frame) => frame,
                other => panic!("expected chunk binary frame, got {other:?}"),
            };
            let (header, payload) = SnapshotBlobChunkHeader::decode(&chunk).expect("decode chunk");
            assert_eq!(payload.len() as u64, total_size_bytes);
            assert_eq!(header.payload_size_bytes, total_size_bytes);
            assert_eq!(chunk_size_bytes, SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES);
            state
                .seen_frames
                .lock()
                .unwrap()
                .push((header.sequence, header.offset_bytes));

            if attempt == 0 {
                let _ = write.send(ws::Message::Close(None)).await;
                return;
            }

            write
                .send(ws::Message::Text(
                    serde_json::to_string(&SnapshotBlobServerControlMessage::UploadAck {
                        version,
                        workorder_id,
                        digest: digest.clone(),
                        sequence: header.sequence,
                        offset_bytes: header.offset_bytes,
                        size_bytes: header.payload_size_bytes,
                        received_bytes: header.payload_size_bytes,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .expect("send upload_ack");
            write
                .send(ws::Message::Text(
                    serde_json::to_string(&SnapshotBlobServerControlMessage::UploadComplete {
                        version,
                        workorder_id,
                        digest,
                        uploaded: true,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .expect("send upload_complete");
            let _ = write.send(ws::Message::Close(None)).await;
        })
    }

    let state = TestState {
        attempts: Arc::new(AtomicUsize::new(0)),
        seen_frames: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route("/api/v1/snapshots/blobs/stream", get(test_ws_handler))
        .with_state(state.clone());
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = RemergeClient::new(&format!("http://127.0.0.1:{port}")).expect("client");
    let payload = b"single-chunk-payload";
    let digest = sha256_hex(payload);
    let uploaded = client
        .stream_upload_blob(&digest, payload)
        .await
        .expect("stream upload after reconnect");
    assert!(uploaded);

    let seen_frames = state.seen_frames.lock().unwrap().clone();
    assert_eq!(seen_frames, vec![(0, 0), (0, 0)]);
    assert_eq!(state.attempts.load(Ordering::SeqCst), 2);

    server_handle.abort();
}

/// The CLI blob streamer adapts chunk size: starts at 10 MiB, shrinks after a slow ack,
/// and only grows again after a stable run of healthy acknowledgements.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_client_adapts_chunk_size_after_slow_ack() {
    use axum::{
        Router,
        extract::{State, WebSocketUpgrade, ws},
        response::IntoResponse,
        routing::get,
    };
    use tokio::time::Duration;

    #[derive(Clone)]
    struct TestState {
        chunk_sizes: Arc<Mutex<Vec<u64>>>,
    }

    async fn test_ws_handler(
        State(state): State<TestState>,
        ws: WebSocketUpgrade,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| async move {
            let (mut write, mut read) = socket.split();

            let init = read
                .next()
                .await
                .expect("init frame")
                .expect("init message");
            let init = match init {
                ws::Message::Text(text) => {
                    serde_json::from_str::<SnapshotBlobClientControlMessage>(&text)
                        .expect("parse upload_init")
                }
                other => panic!("expected upload_init text frame, got {other:?}"),
            };
            let SnapshotBlobClientControlMessage::UploadInit {
                version,
                workorder_id,
                digest,
                total_size_bytes,
                ..
            } = init;

            write
                .send(ws::Message::Text(
                    serde_json::to_string(&SnapshotBlobServerControlMessage::UploadResume {
                        version,
                        workorder_id,
                        digest: digest.clone(),
                        next_offset_bytes: 0,
                        next_sequence: 0,
                        selected_encoding: None,
                        expected_size_bytes: total_size_bytes,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .expect("send upload_resume");

            let mut received_bytes = 0u64;
            let mut chunk_index = 0u64;
            while received_bytes < total_size_bytes {
                let frame = read
                    .next()
                    .await
                    .expect("chunk frame")
                    .expect("chunk message");
                let frame = match frame {
                    ws::Message::Binary(frame) => frame,
                    other => panic!("expected chunk binary frame, got {other:?}"),
                };
                let (header, payload) =
                    SnapshotBlobChunkHeader::decode(&frame).expect("decode chunk");
                assert_eq!(header.sequence, chunk_index);
                assert_eq!(header.offset_bytes, received_bytes);
                received_bytes += payload.len() as u64;
                state.chunk_sizes.lock().unwrap().push(payload.len() as u64);

                if chunk_index == 0 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }

                write
                    .send(ws::Message::Text(
                        serde_json::to_string(&SnapshotBlobServerControlMessage::UploadAck {
                            version,
                            workorder_id,
                            digest: digest.clone(),
                            sequence: header.sequence,
                            offset_bytes: header.offset_bytes,
                            size_bytes: payload.len() as u64,
                            received_bytes,
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .expect("send upload_ack");
                chunk_index += 1;
            }

            write
                .send(ws::Message::Text(
                    serde_json::to_string(&SnapshotBlobServerControlMessage::UploadComplete {
                        version,
                        workorder_id,
                        digest,
                        uploaded: true,
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .expect("send upload_complete");
            let _ = write.send(ws::Message::Close(None)).await;
        })
    }

    let state = TestState {
        chunk_sizes: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route("/api/v1/snapshots/blobs/stream", get(test_ws_handler))
        .with_state(state.clone());
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = RemergeClient::new(&format!("http://127.0.0.1:{port}")).expect("client");
    let ten_mib = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize;
    let payload_len = ten_mib * 3;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 233) as u8).collect();
    let digest = sha256_hex(&payload);
    let uploaded = client
        .stream_upload_blob(&digest, &payload)
        .await
        .expect("adaptive stream upload");
    assert!(uploaded);

    let observed = state.chunk_sizes.lock().unwrap().clone();
    assert_eq!(
        observed,
        vec![
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 2,
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 2,
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        ]
    );

    server_handle.abort();
}

/// Websocket blob streaming uploads multi-chunk blobs and remains idempotent.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_uploads_verified_blob_in_chunks() {
    require_docker();
    let server = common::server::TestServer::start().await;
    let client = RemergeClient::new(&server.base_url).expect("client");

    let payload_len = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize + 8192;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 251) as u8).collect();
    let digest = sha256_hex(&payload);

    let uploaded = client
        .stream_upload_blob(&digest, &payload)
        .await
        .expect("stream upload blob");
    assert!(uploaded, "the first stream upload should store the blob");

    let stored_path =
        remerge_server::blob_store::blob_path(server.state.config.state_dir.as_path(), &digest)
            .expect("blob path");
    assert_eq!(tokio::fs::read(&stored_path).await.unwrap(), payload);

    let uploaded_again = client
        .stream_upload_blob(&digest, &payload)
        .await
        .expect("second stream upload blob");
    assert!(
        !uploaded_again,
        "stream upload should be idempotent when the blob already exists"
    );
}

/// Websocket blob streaming may negotiate zstd transport while keeping the raw digest canonical.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_negotiates_zstd_upload_and_preserves_raw_digest() {
    require_docker();
    let server = common::server::TestServer::start().await;
    let client = RemergeClient::new(&server.base_url).expect("client");

    let payload = vec![b'a'; 256 * 1024];
    let digest = sha256_hex(&payload);
    let uploaded = client
        .stream_upload_blob(&digest, &payload)
        .await
        .expect("stream upload with zstd negotiation");
    assert!(uploaded);

    let stored_path =
        remerge_server::blob_store::blob_path(server.state.config.state_dir.as_path(), &digest)
            .expect("blob path");
    let stored_metadata = remerge_server::blob_store::load_blob_metadata(
        server.state.config.state_dir.as_path(),
        &digest,
    )
    .await
    .expect("blob metadata");

    assert_eq!(tokio::fs::read(&stored_path).await.unwrap(), payload);
    assert!(
        stored_metadata
            .encoded_variants
            .contains_key(&remerge_server::blob_store::BlobEncoding::Zstd)
    );
}

/// Websocket blob streaming rejects corrupted chunk payloads.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_rejects_invalid_chunk_checksum() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let payload = b"expected-payload";
    let digest = sha256_hex(payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect blob stream websocket");

    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&init).unwrap().into(),
    ))
    .await
    .expect("send upload init");

    let resume = ws
        .next()
        .await
        .expect("resume frame")
        .expect("resume websocket message");
    let resume = match resume {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resume frame, got {other:?}"),
    };
    assert!(matches!(
        resume,
        SnapshotBlobServerControlMessage::UploadResume {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            next_offset_bytes: 0,
            next_sequence: 0,
            selected_encoding: None,
            expected_size_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && expected_size_bytes == payload.len() as u64
    ));

    let header = SnapshotBlobChunkHeader::from_payload(0, 0, payload);
    let mut frame = header
        .encode_with_payload(payload)
        .expect("encode payload frame");
    let last = frame.len() - 1;
    frame[last] ^= 0x7f;

    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        frame.into(),
    ))
    .await
    .expect("send corrupted chunk");

    let error_frame = ws
        .next()
        .await
        .expect("error frame")
        .expect("error websocket message");
    let error = match error_frame {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text error frame, got {other:?}"),
    };

    match error {
        SnapshotBlobServerControlMessage::UploadError {
            version,
            workorder_id: Some(id),
            digest: Some(response_digest),
            code,
            message,
        } => {
            assert_eq!(version, SNAPSHOT_BLOB_PROTOCOL_VERSION);
            assert_eq!(id, workorder_id);
            assert_eq!(response_digest, digest);
            assert_eq!(code, "invalid_chunk_frame");
            assert!(
                message.contains("checksum mismatch"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected upload_error, got {other:?}"),
    }
}

/// Websocket blob streaming resumes from the last acknowledged offset after reconnect.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_resumes_after_disconnect() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let chunk_size = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize;
    let payload_len = chunk_size + 4096;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 241) as u8).collect();
    let digest = sha256_hex(&payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect blob stream websocket");
    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&init).unwrap().into(),
    ))
    .await
    .expect("send upload init");

    let first_resume = ws
        .next()
        .await
        .expect("resume frame")
        .expect("resume message");
    let first_resume = match first_resume {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resume frame, got {other:?}"),
    };
    assert!(matches!(
        first_resume,
        SnapshotBlobServerControlMessage::UploadResume {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            next_offset_bytes: 0,
            next_sequence: 0,
            selected_encoding: None,
            expected_size_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && expected_size_bytes == payload.len() as u64
    ));

    let first_chunk = &payload[..chunk_size];
    let frame = SnapshotBlobChunkHeader::from_payload(0, 0, first_chunk)
        .encode_with_payload(first_chunk)
        .expect("encode first chunk");
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        frame.into(),
    ))
    .await
    .expect("send first chunk");

    let first_ack = ws.next().await.expect("ack frame").expect("ack message");
    let first_ack = match first_ack {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text ack frame, got {other:?}"),
    };
    assert!(matches!(
        first_ack,
        SnapshotBlobServerControlMessage::UploadAck {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            sequence: 0,
            offset_bytes: 0,
            size_bytes,
            received_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && size_bytes == chunk_size as u64
            && received_bytes == chunk_size as u64
    ));
    ws.close(None).await.expect("close first websocket");

    let (mut resumed_ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("reconnect blob stream websocket");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&init).unwrap().into(),
        ))
        .await
        .expect("send resumed upload init");

    let resumed = resumed_ws
        .next()
        .await
        .expect("resumed frame")
        .expect("resumed message");
    let resumed = match resumed {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resumed frame, got {other:?}"),
    };
    assert!(matches!(
        resumed,
        SnapshotBlobServerControlMessage::UploadResume {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            next_offset_bytes,
            next_sequence,
            selected_encoding: None,
            expected_size_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && next_offset_bytes == chunk_size as u64
            && next_sequence == 1
            && expected_size_bytes == payload.len() as u64
    ));

    let remaining = &payload[chunk_size..];
    let resumed_frame = SnapshotBlobChunkHeader::from_payload(1, chunk_size as u64, remaining)
        .encode_with_payload(remaining)
        .expect("encode resumed chunk");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            resumed_frame.into(),
        ))
        .await
        .expect("send resumed chunk");

    let resumed_ack = resumed_ws
        .next()
        .await
        .expect("resumed ack frame")
        .expect("resumed ack message");
    let resumed_ack = match resumed_ack {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resumed ack frame, got {other:?}"),
    };
    assert!(matches!(
        resumed_ack,
        SnapshotBlobServerControlMessage::UploadAck {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            sequence: 1,
            offset_bytes,
            size_bytes,
            received_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && offset_bytes == chunk_size as u64
            && size_bytes == remaining.len() as u64
            && received_bytes == payload.len() as u64
    ));

    let complete = resumed_ws
        .next()
        .await
        .expect("complete frame")
        .expect("complete message");
    let complete = match complete {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text complete frame, got {other:?}"),
    };
    assert!(matches!(
        complete,
        SnapshotBlobServerControlMessage::UploadComplete {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            uploaded: true,
        } if id == workorder_id && response_digest == &digest
    ));

    let stored_path =
        remerge_server::blob_store::blob_path(server.state.config.state_dir.as_path(), &digest)
            .expect("blob path");
    assert_eq!(tokio::fs::read(&stored_path).await.unwrap(), payload);
}

/// Websocket blob streaming resumes with the exact next sequence after smaller
/// chunks have already advanced the stream past the initial 10 MiB stride.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_resumes_after_shrunk_chunks() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let default_chunk = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize;
    let shrunk_chunk = default_chunk / 4;
    let payload_len = default_chunk + shrunk_chunk + shrunk_chunk + 4096;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 239) as u8).collect();
    let digest = sha256_hex(&payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);

    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };

    let (mut first_ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect first blob stream websocket");
    first_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&init).unwrap().into(),
        ))
        .await
        .expect("send first upload init");

    let first_resume = first_ws
        .next()
        .await
        .expect("first resume frame")
        .expect("first resume message");
    let first_resume = match first_resume {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text first resume frame, got {other:?}"),
    };
    assert!(matches!(
        first_resume,
        SnapshotBlobServerControlMessage::UploadResume {
            next_offset_bytes: 0,
            next_sequence: 0,
            ..
        }
    ));

    for (sequence, offset, end) in [
        (0u64, 0usize, default_chunk),
        (1u64, default_chunk, default_chunk + shrunk_chunk),
        (
            2u64,
            default_chunk + shrunk_chunk,
            default_chunk + shrunk_chunk + shrunk_chunk,
        ),
    ] {
        let chunk = &payload[offset..end];
        let frame = SnapshotBlobChunkHeader::from_payload(sequence, offset as u64, chunk)
            .encode_with_payload(chunk)
            .expect("encode upload chunk");
        first_ws
            .send(tokio_tungstenite::tungstenite::Message::Binary(
                frame.into(),
            ))
            .await
            .expect("send upload chunk");

        let ack = first_ws
            .next()
            .await
            .expect("ack frame")
            .expect("ack message");
        let ack = match ack {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
            }
            other => panic!("expected text ack frame, got {other:?}"),
        };
        assert!(matches!(
            ack,
            SnapshotBlobServerControlMessage::UploadAck {
                sequence: ack_sequence,
                offset_bytes,
                received_bytes,
                ..
            } if ack_sequence == sequence
                && offset_bytes == offset as u64
                && received_bytes == end as u64
        ));
    }
    first_ws.close(None).await.expect("close first websocket");

    let resumed_offset = (default_chunk + shrunk_chunk + shrunk_chunk) as u64;
    let (mut resumed_ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("reconnect blob stream websocket");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&init).unwrap().into(),
        ))
        .await
        .expect("send resumed upload init");

    let resumed = resumed_ws
        .next()
        .await
        .expect("resumed frame")
        .expect("resumed message");
    let resumed = match resumed {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resumed frame, got {other:?}"),
    };
    assert!(matches!(
        resumed,
        SnapshotBlobServerControlMessage::UploadResume {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            next_offset_bytes,
            next_sequence,
            selected_encoding: None,
            expected_size_bytes,
        } if id == workorder_id
            && response_digest == &digest
            && next_offset_bytes == resumed_offset
            && next_sequence == 3
            && expected_size_bytes == payload.len() as u64
    ));

    let final_chunk = &payload[resumed_offset as usize..];
    let final_frame = SnapshotBlobChunkHeader::from_payload(3, resumed_offset, final_chunk)
        .encode_with_payload(final_chunk)
        .expect("encode final upload chunk");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            final_frame.into(),
        ))
        .await
        .expect("send final upload chunk");

    let final_ack = resumed_ws
        .next()
        .await
        .expect("final ack frame")
        .expect("final ack message");
    let final_ack = match final_ack {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text final ack frame, got {other:?}"),
    };
    assert!(matches!(
        final_ack,
        SnapshotBlobServerControlMessage::UploadAck {
            sequence: 3,
            offset_bytes,
            received_bytes,
            ..
        } if offset_bytes == resumed_offset && received_bytes == payload.len() as u64
    ));

    let complete = resumed_ws
        .next()
        .await
        .expect("complete frame")
        .expect("complete message");
    let complete = match complete {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text complete frame, got {other:?}"),
    };
    assert!(matches!(
        complete,
        SnapshotBlobServerControlMessage::UploadComplete {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id: id,
            digest: ref response_digest,
            uploaded: true,
        } if id == workorder_id && response_digest == &digest
    ));

    let stored_path =
        remerge_server::blob_store::blob_path(server.state.config.state_dir.as_path(), &digest)
            .expect("blob path");
    assert_eq!(tokio::fs::read(&stored_path).await.unwrap(), payload);
}

/// Websocket blob streaming rejects duplicate replay outside the confirmed resume point.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_rejects_duplicate_replay_outside_resume_point() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let chunk_size = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize;
    let payload_len = chunk_size + 2048;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 239) as u8).collect();
    let digest = sha256_hex(&payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect blob stream websocket");
    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&init).unwrap().into(),
    ))
    .await
    .expect("send upload init");

    let _ = ws
        .next()
        .await
        .expect("resume frame")
        .expect("resume message");
    let first_chunk = &payload[..chunk_size];
    let frame = SnapshotBlobChunkHeader::from_payload(0, 0, first_chunk)
        .encode_with_payload(first_chunk)
        .expect("encode first chunk");
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        frame.into(),
    ))
    .await
    .expect("send first chunk");
    let _ = ws.next().await.expect("ack frame").expect("ack message");
    ws.close(None).await.expect("close first websocket");

    let (mut resumed_ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("reconnect blob stream websocket");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&init).unwrap().into(),
        ))
        .await
        .expect("send resumed upload init");

    let resumed = resumed_ws
        .next()
        .await
        .expect("resumed frame")
        .expect("resumed message");
    let resumed = match resumed {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text resumed frame, got {other:?}"),
    };
    assert!(matches!(
        resumed,
        SnapshotBlobServerControlMessage::UploadResume {
            next_offset_bytes,
            next_sequence,
            selected_encoding: None,
            expected_size_bytes,
            ..
        } if next_offset_bytes == chunk_size as u64
            && next_sequence == 1
            && expected_size_bytes == payload.len() as u64
    ));

    let duplicate_frame = SnapshotBlobChunkHeader::from_payload(0, 0, first_chunk)
        .encode_with_payload(first_chunk)
        .expect("encode duplicate chunk");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            duplicate_frame.into(),
        ))
        .await
        .expect("send duplicate replay chunk");

    let error_frame = resumed_ws
        .next()
        .await
        .expect("error frame")
        .expect("error message");
    let error = match error_frame {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text error frame, got {other:?}"),
    };
    match error {
        SnapshotBlobServerControlMessage::UploadError {
            version,
            workorder_id: Some(id),
            digest: Some(response_digest),
            code,
            message,
        } => {
            assert_eq!(version, SNAPSHOT_BLOB_PROTOCOL_VERSION);
            assert_eq!(id, workorder_id);
            assert_eq!(response_digest, digest);
            assert_eq!(code, "unexpected_sequence");
            assert!(message.contains("Expected chunk sequence 1, got 0"));
        }
        other => panic!("expected upload_error, got {other:?}"),
    }
}

/// Websocket blob streaming rejects chunks that would exceed the negotiated blob length.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_rejects_payload_beyond_declared_length() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let payload = b"declared";
    let digest = sha256_hex(payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect blob stream websocket");

    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&init).unwrap().into(),
    ))
    .await
    .expect("send upload init");

    let _ = ws
        .next()
        .await
        .expect("resume frame")
        .expect("resume message");

    let oversized_payload = b"declared-and-extra";
    let oversized_frame = SnapshotBlobChunkHeader::from_payload(0, 0, oversized_payload)
        .encode_with_payload(oversized_payload)
        .expect("encode oversized chunk");
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        oversized_frame.into(),
    ))
    .await
    .expect("send oversized chunk");

    let error_frame = ws
        .next()
        .await
        .expect("error frame")
        .expect("error message");
    let error = match error_frame {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text error frame, got {other:?}"),
    };

    match error {
        SnapshotBlobServerControlMessage::UploadError {
            version,
            workorder_id: Some(id),
            digest: Some(response_digest),
            code,
            message,
        } => {
            assert_eq!(version, SNAPSHOT_BLOB_PROTOCOL_VERSION);
            assert_eq!(id, workorder_id);
            assert_eq!(response_digest, digest);
            assert_eq!(code, "blob_too_large");
            assert!(message.contains("exceeds the declared blob length"));
        }
        other => panic!("expected upload_error, got {other:?}"),
    }
}

/// Websocket blob streaming rejects overlapping byte ranges outside the confirmed resume offset.
#[cfg(feature = "integration")]
#[tokio::test]
async fn blob_stream_endpoint_rejects_overlapping_offset_outside_resume_point() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let chunk_size = SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES as usize;
    let payload_len = chunk_size + 2048;
    let payload: Vec<u8> = (0..payload_len).map(|idx| (idx % 227) as u8).collect();
    let digest = sha256_hex(&payload);
    let workorder_id = uuid::Uuid::new_v4();
    let ws_url = blob_stream_ws_url(&server.base_url);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect blob stream websocket");
    let init = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: digest.clone(),
        total_size_bytes: payload.len() as u64,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: Vec::new(),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        serde_json::to_string(&init).unwrap().into(),
    ))
    .await
    .expect("send upload init");

    let _ = ws
        .next()
        .await
        .expect("resume frame")
        .expect("resume message");
    let first_chunk = &payload[..chunk_size];
    let first_frame = SnapshotBlobChunkHeader::from_payload(0, 0, first_chunk)
        .encode_with_payload(first_chunk)
        .expect("encode first chunk");
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        first_frame.into(),
    ))
    .await
    .expect("send first chunk");
    let _ = ws.next().await.expect("ack frame").expect("ack message");
    ws.close(None).await.expect("close first websocket");

    let (mut resumed_ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("reconnect blob stream websocket");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&init).unwrap().into(),
        ))
        .await
        .expect("send resumed upload init");
    let _ = resumed_ws
        .next()
        .await
        .expect("resumed frame")
        .expect("resumed message");

    let overlap_offset = (chunk_size / 2) as u64;
    let overlapping_chunk = &payload[chunk_size / 2..chunk_size];
    let overlapping_frame =
        SnapshotBlobChunkHeader::from_payload(1, overlap_offset, overlapping_chunk)
            .encode_with_payload(overlapping_chunk)
            .expect("encode overlapping chunk");
    resumed_ws
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            overlapping_frame.into(),
        ))
        .await
        .expect("send overlapping chunk");

    let error_frame = resumed_ws
        .next()
        .await
        .expect("error frame")
        .expect("error message");
    let error = match error_frame {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            serde_json::from_str::<SnapshotBlobServerControlMessage>(&text).unwrap()
        }
        other => panic!("expected text error frame, got {other:?}"),
    };

    match error {
        SnapshotBlobServerControlMessage::UploadError {
            version,
            workorder_id: Some(id),
            digest: Some(response_digest),
            code,
            message,
        } => {
            assert_eq!(version, SNAPSHOT_BLOB_PROTOCOL_VERSION);
            assert_eq!(id, workorder_id);
            assert_eq!(response_digest, digest);
            assert_eq!(code, "unexpected_offset");
            assert!(message.contains(&format!(
                "Expected chunk offset {}, got {}",
                chunk_size, overlap_offset
            )));
        }
        other => panic!("expected upload_error, got {other:?}"),
    }
}

/// Client-side manifest negotiation only uploads missing blobs and refs-only
/// submission is enough for the server to materialize runtime snapshots.
#[cfg(feature = "integration")]
#[tokio::test]
async fn refs_only_submission_materializes_uploaded_and_preexisting_blobs() {
    require_docker();
    let server = common::server::TestServer::start().await;
    let client = RemergeClient::new(&server.base_url).expect("client");

    let repo_bytes = b"EAPI=8\nDESCRIPTION=\"demo\"\n";
    let distfile_bytes = b"demo-distfile";
    let repo_digest = sha256_hex(repo_bytes);
    let distfile_digest = sha256_hex(distfile_bytes);

    remerge_server::blob_store::store_blob(server.state.config.state_dir.as_path(), repo_bytes)
        .await
        .expect("store preexisting repo blob");

    let missing = client
        .find_missing_blobs(&[repo_digest.clone(), distfile_digest.clone()])
        .await
        .expect("find missing blobs");
    assert_eq!(missing, vec![distfile_digest.clone()]);

    let uploaded = client
        .upload_blob(&distfile_digest, distfile_bytes)
        .await
        .expect("upload missing distfile blob");
    assert!(uploaded, "the missing distfile blob should be uploaded");

    let repo_refs = BTreeMap::from([(
        "dev-libs/demo/demo-1.0.ebuild".to_string(),
        repo_digest.clone(),
    )]);
    let repo_tree =
        remerge_server::tree_store::store_tree(server.state.config.state_dir.as_path(), &repo_refs)
            .await
            .expect("store repo tree manifest");

    let mut portage_config = common::fixtures::minimal_portage_config();
    portage_config.repos_conf.insert(
        "local-overlay.conf".into(),
        "[local-overlay]\nlocation = /var/db/repos/local-overlay\nauto-sync = no\n".into(),
    );
    portage_config
        .repo_snapshot_refs
        .insert("local-overlay".into(), repo_refs);
    portage_config
        .repo_snapshot_trees
        .insert("local-overlay".into(), repo_tree.digest.clone());
    portage_config
        .distfile_snapshot_refs
        .insert("demo-1.0.tar.xz".into(), distfile_digest.clone());

    let atoms = vec!["dev-libs/demo".to_string()];
    let emerge_args = atoms.clone();
    let resp = client
        .submit_workorder(
            uuid::Uuid::new_v4(),
            remerge_types::client::ClientRole::Main,
            &atoms,
            &emerge_args,
            &portage_config,
            &common::fixtures::minimal_system_identity(),
        )
        .await
        .expect("submit refs-only workorder");

    let stored_workorder = server
        .state
        .workorders
        .read()
        .await
        .get(&resp.workorder_id)
        .cloned()
        .expect("stored workorder");
    assert!(stored_workorder.portage_config.repo_snapshots.is_empty());
    assert!(
        stored_workorder
            .portage_config
            .distfile_snapshots
            .is_empty()
    );

    let staged = remerge_server::runtime::stage_workorder_runtime(
        server.state.config.state_dir.as_path(),
        &stored_workorder,
    )
    .await
    .expect("stage refs-only workorder");

    assert_eq!(
        tokio::fs::read(
            staged
                .runtime_dir
                .join("snapshots/repos/local-overlay/dev-libs/demo/demo-1.0.ebuild"),
        )
        .await
        .unwrap(),
        repo_bytes
    );
    assert_eq!(
        tokio::fs::read(
            staged
                .runtime_dir
                .join("snapshots/distfiles/demo-1.0.tar.xz")
        )
        .await
        .unwrap(),
        distfile_bytes
    );

    let missing_after = client
        .find_missing_blobs(&[repo_digest, distfile_digest])
        .await
        .expect("find missing blobs after upload");
    assert!(
        missing_after.is_empty(),
        "all referenced blobs should now be present"
    );
}

/// Inline snapshot submission deduplicates blobs and trees across distinct client IDs.
#[cfg(feature = "integration")]
#[tokio::test]
async fn inline_snapshot_submission_deduplicates_across_distinct_clients() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let client_one_id = uuid::Uuid::new_v4();
    let client_two_id = uuid::Uuid::new_v4();
    let portage_config = common::fixtures::full_portage_config();
    let system_id = common::fixtures::minimal_system_identity();
    let atoms = vec!["app-misc/hello".to_string()];

    let first_req = SubmitWorkorderRequest {
        client_id: client_one_id,
        role: remerge_types::client::ClientRole::Main,
        atoms: atoms.clone(),
        emerge_args: vec!["--pretend".to_string()],
        portage_config: portage_config.clone(),
        system_id: system_id.clone(),
    };
    let first_resp = submit_workorder_for_test(&server, &first_req).await;

    let first_workorder = server
        .state
        .workorders
        .read()
        .await
        .get(&first_resp.workorder_id)
        .cloned()
        .expect("first stored workorder");
    let first_blob_count =
        remerge_server::blob_store::list_blob_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list blob metadata after first submit")
            .len();
    let first_tree_count =
        remerge_server::tree_store::list_tree_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list tree metadata after first submit")
            .len();
    assert!(
        first_blob_count > 0,
        "first submit should populate snapshot blobs"
    );
    assert!(
        first_tree_count > 0,
        "first submit should populate snapshot trees"
    );

    let second_req = SubmitWorkorderRequest {
        client_id: client_two_id,
        role: remerge_types::client::ClientRole::Main,
        atoms,
        emerge_args: vec!["--pretend".to_string()],
        portage_config,
        system_id,
    };
    let second_resp = submit_workorder_for_test(&server, &second_req).await;

    let second_workorder = server
        .state
        .workorders
        .read()
        .await
        .get(&second_resp.workorder_id)
        .cloned()
        .expect("second stored workorder");
    let second_blob_count =
        remerge_server::blob_store::list_blob_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list blob metadata after second submit")
            .len();
    let second_tree_count =
        remerge_server::tree_store::list_tree_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list tree metadata after second submit")
            .len();

    assert_eq!(
        second_blob_count, first_blob_count,
        "second distinct client should reuse existing snapshot blobs"
    );
    assert_eq!(
        second_tree_count, first_tree_count,
        "second distinct client should reuse existing tree manifests"
    );
    assert_eq!(
        first_workorder.portage_config.repo_snapshot_refs,
        second_workorder.portage_config.repo_snapshot_refs,
        "distinct clients should reference the same deduplicated repo blobs"
    );
    assert_eq!(
        first_workorder.portage_config.repo_snapshot_trees,
        second_workorder.portage_config.repo_snapshot_trees,
        "distinct clients should reference the same deduplicated repo trees"
    );
    assert_eq!(
        first_workorder.portage_config.distfile_snapshot_refs,
        second_workorder.portage_config.distfile_snapshot_refs,
        "distinct clients should reference the same deduplicated distfile blobs"
    );

    let first_staged = remerge_server::runtime::stage_workorder_runtime(
        server.state.config.state_dir.as_path(),
        &first_workorder,
    )
    .await
    .expect("stage first workorder runtime");
    let second_staged = remerge_server::runtime::stage_workorder_runtime(
        server.state.config.state_dir.as_path(),
        &second_workorder,
    )
    .await
    .expect("stage second workorder runtime");

    assert_eq!(
        first_staged.snapshot_references.blob_digests,
        second_staged.snapshot_references.blob_digests,
        "both clients should stage the same deduplicated blob set"
    );
    assert_eq!(
        first_staged.snapshot_references.tree_digests,
        second_staged.snapshot_references.tree_digests,
        "both clients should stage the same deduplicated tree set"
    );
}

/// Snapshot reuse survives a same-client config change because cleanup honors the grace window.
#[cfg(feature = "integration")]
#[tokio::test]
async fn snapshot_reuse_survives_client_config_change_within_grace_window() {
    require_docker();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 0,
        ..Default::default()
    };
    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let shared_client_id = uuid::Uuid::new_v4();
    let mut initial_config = common::fixtures::full_portage_config();
    let system_id = common::fixtures::minimal_system_identity();
    let first_req = SubmitWorkorderRequest {
        client_id: shared_client_id,
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["app-misc/hello".to_string()],
        emerge_args: vec!["--pretend".to_string()],
        portage_config: initial_config.clone(),
        system_id: system_id.clone(),
    };
    let first_resp = submit_workorder_for_test(&server, &first_req).await;

    let first_workorder = server
        .state
        .workorders
        .read()
        .await
        .get(&first_resp.workorder_id)
        .cloned()
        .expect("first stored workorder");
    let first_client_state = server
        .state
        .clients
        .get(&shared_client_id)
        .await
        .expect("first client registry state");
    let first_staged = remerge_server::runtime::stage_workorder_runtime(
        server.state.config.state_dir.as_path(),
        &first_workorder,
    )
    .await
    .expect("stage first workorder runtime");
    let first_blob_count =
        remerge_server::blob_store::list_blob_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list blob metadata after first submit")
            .len();

    let cancel_client = reqwest::Client::new();
    let cancel_resp = cancel_client
        .delete(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, first_resp.workorder_id
        ))
        .send()
        .await
        .expect("cancel first workorder");
    assert_eq!(
        cancel_resp.status(),
        200,
        "first workorder cancellation should succeed"
    );

    let cleanup_time = chrono::Utc::now() + chrono::Duration::hours(12);
    let cleanup_summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        server.state.config.state_dir.as_path(),
        &server.state.config,
        &[],
        cleanup_time,
    )
    .await
    .expect("cleanup within grace window");
    assert_eq!(
        cleanup_summary.deleted_blobs, 0,
        "recently unreferenced snapshot blobs should survive the grace window"
    );
    assert_eq!(
        cleanup_summary.deleted_trees, 0,
        "recently unreferenced snapshot trees should survive the grace window"
    );

    initial_config
        .make_conf
        .use_flags
        .push("new-config-flag".to_string());
    let second_req = SubmitWorkorderRequest {
        client_id: shared_client_id,
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["app-misc/hello".to_string()],
        emerge_args: vec!["--pretend".to_string()],
        portage_config: initial_config,
        system_id,
    };
    let second_resp = submit_workorder_for_test(&server, &second_req).await;

    let second_workorder = server
        .state
        .workorders
        .read()
        .await
        .get(&second_resp.workorder_id)
        .cloned()
        .expect("second stored workorder");
    let second_client_state = server
        .state
        .clients
        .get(&shared_client_id)
        .await
        .expect("second client registry state");
    let second_staged = remerge_server::runtime::stage_workorder_runtime(
        server.state.config.state_dir.as_path(),
        &second_workorder,
    )
    .await
    .expect("stage second workorder runtime");
    let second_blob_count =
        remerge_server::blob_store::list_blob_metadata(server.state.config.state_dir.as_path())
            .await
            .expect("list blob metadata after config change")
            .len();

    assert_ne!(
        first_client_state.config_hash, second_client_state.config_hash,
        "same client should register a new config hash after the config change"
    );
    assert_eq!(
        first_blob_count, second_blob_count,
        "config changes should reuse warm snapshot blobs instead of creating duplicates"
    );
    assert_eq!(
        first_workorder.portage_config.repo_snapshot_refs,
        second_workorder.portage_config.repo_snapshot_refs,
        "config changes should preserve deduplicated repo blob references"
    );
    assert_eq!(
        first_workorder.portage_config.repo_snapshot_trees,
        second_workorder.portage_config.repo_snapshot_trees,
        "config changes should preserve deduplicated tree references"
    );
    assert_eq!(
        first_workorder.portage_config.distfile_snapshot_refs,
        second_workorder.portage_config.distfile_snapshot_refs,
        "config changes should preserve deduplicated distfile references"
    );
    assert_eq!(
        first_staged.snapshot_references.blob_digests,
        second_staged.snapshot_references.blob_digests,
        "the replacement workorder should reuse the same warm blob set"
    );
    assert_eq!(
        first_staged.snapshot_references.tree_digests,
        second_staged.snapshot_references.tree_digests,
        "the replacement workorder should reuse the same warm tree set"
    );
}

/// POST /api/v1/workorders with shell injection in atoms returns 400.
#[cfg(feature = "integration")]
#[tokio::test]
async fn submit_workorder_invalid_atoms() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["; rm -rf /".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 400, "invalid atoms should return 400");
}

/// POST /api/v1/workorders twice with same client returns 409 (duplicate active).
#[cfg(feature = "integration")]
#[tokio::test]
async fn submit_workorder_duplicate_active() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let client_id = uuid::Uuid::new_v4();
    let req = SubmitWorkorderRequest {
        client_id,
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp1 = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("first submit");
    assert_eq!(resp1.status(), 200);

    // Second submission with same client_id should be rejected.
    let resp2 = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("second submit");
    assert_eq!(
        resp2.status(),
        409,
        "duplicate active workorder should return 409"
    );
}

/// GET /api/v1/workorders/{id} returns workorder details.
#[cfg(feature = "integration")]
#[tokio::test]
async fn get_workorder() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");
    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

    // Fetch the workorder.
    let resp = reqwest::get(format!(
        "{}/api/v1/workorders/{}",
        server.base_url, submit_resp.workorder_id
    ))
    .await
    .expect("get workorder");
    assert_eq!(resp.status(), 200);

    let status_resp: WorkorderStatusResponse = resp.json().await.expect("parse status");
    assert_eq!(status_resp.workorder_id, submit_resp.workorder_id);
}

/// GET /api/v1/workorders lists submitted workorders.
#[cfg(feature = "integration")]
#[tokio::test]
async fn list_workorders() {
    require_docker();
    let server = common::server::TestServer::start().await;

    // Submit a workorder first.
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let _ = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");

    let resp = reqwest::get(format!("{}/api/v1/workorders", server.base_url))
        .await
        .expect("list");
    assert_eq!(resp.status(), 200);

    let list_resp: ListWorkordersResponse = resp.json().await.expect("parse list");
    assert!(
        !list_resp.workorders.is_empty(),
        "should have at least one workorder"
    );
}

/// DELETE /api/v1/workorders/{id} cancels a workorder.
#[cfg(feature = "integration")]
#[tokio::test]
async fn cancel_workorder() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");
    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

    // Cancel it.
    let resp = client
        .delete(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .send()
        .await
        .expect("cancel");
    assert_eq!(resp.status(), 200);

    let cancel_resp: CancelWorkorderResponse = resp.json().await.expect("parse cancel");
    assert!(cancel_resp.cancelled, "workorder should be cancelled");
}

/// GET /api/v1/workorders/{nonexistent} returns 404.
#[cfg(feature = "integration")]
#[tokio::test]
async fn get_nonexistent_workorder() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let resp = reqwest::get(format!(
        "{}/api/v1/workorders/{}",
        server.base_url,
        uuid::Uuid::new_v4()
    ))
    .await
    .expect("get nonexistent");
    assert_eq!(resp.status(), 404);
}

/// Follower without main client is rejected.
#[cfg(feature = "integration")]
#[tokio::test]
async fn follower_without_main_rejected() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Follower,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");
    assert_eq!(
        resp.status(),
        409,
        "follower without main should be rejected"
    );
}

/// WebSocket /api/v1/workorders/:id/progress — connects and receives events.
#[cfg(feature = "integration")]
#[tokio::test]
async fn websocket_progress_stream() {
    require_docker();
    let server = common::server::TestServer::start().await;

    // 1. Submit a workorder.
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 200);

    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
    let workorder_id = submit_resp.workorder_id;
    assert!(
        submit_resp.trace_id.is_some(),
        "submit response should include a trace ID"
    );

    // 2. Connect to WebSocket.
    let ws_url = format!(
        "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
        server.port, workorder_id
    );

    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("WebSocket connection should succeed");

    let (_, mut read) = ws_stream.split();

    // 3. Cancel the workorder to trigger a StatusChanged event.
    let resp = client
        .delete(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, workorder_id
        ))
        .send()
        .await
        .expect("cancel request");
    assert_eq!(resp.status(), 200);

    // 4. Read frames with timeout — we should get at least one text event.
    let mut received_text = false;
    for _ in 0..10 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), read.next()).await {
            Ok(Some(Ok(msg))) => {
                if msg.is_text() {
                    received_text = true;
                    let text = msg.into_text().expect("text frame");
                    let progress =
                        serde_json::from_str::<remerge_types::workorder::BuildProgress>(&text)
                            .expect("valid build progress event");
                    assert_eq!(progress.workorder_id, workorder_id);
                    assert_eq!(progress.trace_id, submit_resp.trace_id);
                    match progress.event {
                        remerge_types::workorder::BuildEvent::StatusChanged { from, to } => {
                            assert_eq!(from, remerge_types::workorder::WorkorderStatus::Pending);
                            assert_eq!(to, remerge_types::workorder::WorkorderStatus::Cancelled);
                        }
                        other => panic!("expected status change event, got {other:?}"),
                    }
                    break;
                }
            }
            Ok(Some(Err(e))) => {
                eprintln!("WebSocket error: {e}");
                break;
            }
            Ok(None) => break, // Stream closed.
            Err(_) => break,   // Timeout.
        }
    }

    assert!(
        received_text,
        "should have received at least one text frame with status event"
    );

    let status_resp = client
        .get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, workorder_id
        ))
        .send()
        .await
        .expect("status request");
    assert_eq!(status_resp.status(), 200);
    let status: WorkorderStatusResponse = status_resp.json().await.expect("parse status");
    assert_eq!(status.trace_id, submit_resp.trace_id);
}

/// Queue claim emits the real Pending -> Provisioning transition before work starts.
#[cfg(feature = "integration")]
#[tokio::test]
async fn queue_claim_emits_pending_to_provisioning_status_event() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        max_workers: 0,
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 200);

    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
    let ws_url = format!(
        "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
        server.port, submit_resp.workorder_id
    );
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("WebSocket connection should succeed");
    let (_, mut read) = ws_stream.split();

    let queue_state = server.state.clone();
    let queue_handle = tokio::spawn(async move {
        remerge_server::queue::process_queue(queue_state).await;
    });

    let mut saw_transition = false;
    for _ in 0..10 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), read.next()).await {
            Ok(Some(Ok(msg))) if msg.is_text() => {
                let text = msg.into_text().expect("text frame");
                let progress =
                    serde_json::from_str::<remerge_types::workorder::BuildProgress>(&text)
                        .expect("valid build progress event");
                if let remerge_types::workorder::BuildEvent::StatusChanged { from, to } =
                    progress.event
                {
                    assert_eq!(progress.workorder_id, submit_resp.workorder_id);
                    assert_eq!(progress.trace_id, submit_resp.trace_id);
                    assert_eq!(from, remerge_types::workorder::WorkorderStatus::Pending);
                    assert_eq!(to, remerge_types::workorder::WorkorderStatus::Provisioning);
                    saw_transition = true;
                    break;
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => panic!("WebSocket error: {e}"),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    queue_handle.abort();

    assert!(
        saw_transition,
        "queue claim should emit a Pending -> Provisioning status event"
    );
}

/// Explicit traceparent headers are accepted and persisted on the workorder.
#[cfg(feature = "integration")]
#[tokio::test]
async fn submit_workorder_accepts_traceparent_header() {
    require_docker();
    let server = common::server::TestServer::start().await;

    let traceparent = "00-11111111111111111111111111111111-2222222222222222-01";
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .header("traceparent", traceparent)
        .json(&req)
        .send()
        .await
        .expect("submit request");
    assert_eq!(resp.status(), 200);

    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
    assert_eq!(
        submit_resp.trace_id.as_deref(),
        Some("11111111111111111111111111111111")
    );

    let status_resp = client
        .get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .send()
        .await
        .expect("status request");
    assert_eq!(status_resp.status(), 200);

    let status: WorkorderStatusResponse = status_resp.json().await.expect("parse status");
    assert_eq!(
        status.trace_id.as_deref(),
        Some("11111111111111111111111111111111")
    );
}

/// Auth enforcement: None mode allows all (implicitly tested above),
/// Mtls mode rejects requests without cert header.
#[cfg(feature = "integration")]
#[tokio::test]
async fn auth_mtls_rejects_without_cert() {
    require_docker();

    let port = common::free_port();
    let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
    let state_dir = tempfile::TempDir::new().expect("temp dir");

    let config = remerge_server::config::ServerConfig {
        binpkg_dir: binpkg_dir.path().to_path_buf(),
        binhost_url: format!("http://127.0.0.1:{port}/binpkgs"),
        state_dir: state_dir.path().to_path_buf(),
        auth: remerge_server::auth::AuthConfig {
            mode: remerge_types::auth::AuthMode::Mtls,
            ..Default::default()
        },
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    // Submit without cert header — should be rejected.
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");

    // mTLS mode with no client certificate triggers AuthError::CertificateRequired,
    // which maps to 401 Unauthorized.
    assert_eq!(
        resp.status(),
        401,
        "mTLS without cert should return 401 Unauthorized, got {}",
        resp.status()
    );
}

/// In mtls mode, /metrics requires a trusted client certificate header.
#[cfg(feature = "integration")]
#[tokio::test]
async fn metrics_requires_cert_in_mtls_mode() {
    require_docker();

    let server = common::server::TestServer::start_with_config(Some(auth_config(
        remerge_types::auth::AuthMode::Mtls,
    )))
    .await;

    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await
        .expect("metrics request without cert");
    assert_eq!(resp.status(), 401, "mtls metrics should require a cert");

    let resp = client
        .get(format!("{}/metrics", server.base_url))
        .headers(cert_headers("sha256:00:11:22:33"))
        .send()
        .await
        .expect("metrics request with unknown cert");
    assert_eq!(
        resp.status(),
        401,
        "unknown client certificate should be rejected"
    );

    let resp = client
        .get(format!("{}/metrics", server.base_url))
        .headers(cert_headers(TEST_CERT_FINGERPRINT))
        .send()
        .await
        .expect("metrics request with known cert");
    assert_eq!(resp.status(), 200, "known client certificate should pass");
}

/// In mixed mode, /metrics stays protected but /binpkgs stays public.
#[cfg(feature = "integration")]
#[tokio::test]
async fn mixed_mode_keeps_metrics_private_and_binpkgs_public() {
    require_docker();

    let server = common::server::TestServer::start_with_config(Some(auth_config(
        remerge_types::auth::AuthMode::Mixed,
    )))
    .await;

    write_binpkg_fixture(&server, "public-test.pkg", "ok");
    let client = reqwest::Client::new();

    let metrics = client
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await
        .expect("mixed metrics request");
    assert_eq!(
        metrics.status(),
        401,
        "mixed mode should keep /metrics behind mTLS"
    );

    let binpkg = client
        .get(format!("{}/binpkgs/public-test.pkg", server.base_url))
        .send()
        .await
        .expect("mixed binpkg request");
    assert_eq!(
        binpkg.status(),
        200,
        "mixed mode should keep /binpkgs public"
    );
    assert_eq!(binpkg.text().await.expect("binpkg body"), "ok");
}

/// In mtls mode, /binpkgs also requires a trusted client certificate header.
#[cfg(feature = "integration")]
#[tokio::test]
async fn binpkgs_requires_cert_in_mtls_mode() {
    require_docker();

    let server = common::server::TestServer::start_with_config(Some(auth_config(
        remerge_types::auth::AuthMode::Mtls,
    )))
    .await;

    write_binpkg_fixture(&server, "private-test.pkg", "ok");
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/binpkgs/private-test.pkg", server.base_url))
        .send()
        .await
        .expect("binpkg request without cert");
    assert_eq!(resp.status(), 401, "mtls binpkgs should require a cert");

    let resp = client
        .get(format!("{}/binpkgs/private-test.pkg", server.base_url))
        .headers(cert_headers(TEST_CERT_FINGERPRINT))
        .send()
        .await
        .expect("binpkg request with cert");
    assert_eq!(resp.status(), 200, "mtls binpkgs should allow a known cert");
    assert_eq!(resp.text().await.expect("binpkg body"), "ok");
}

/// Workorder TTL expiry — stale completed workorders are evicted.
#[cfg(feature = "integration")]
#[tokio::test]
async fn workorder_ttl_eviction() {
    require_docker();

    // Create server with retention_hours = 0 (cutoff = now).
    let port = common::free_port();
    let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
    let state_dir = tempfile::TempDir::new().expect("temp dir");

    let config = remerge_server::config::ServerConfig {
        binpkg_dir: binpkg_dir.path().to_path_buf(),
        binhost_url: format!("http://127.0.0.1:{port}/binpkgs"),
        state_dir: state_dir.path().to_path_buf(),
        retention_hours: 0,
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    // Submit and cancel a workorder (making it terminal).
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");
    let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");
    let wo_id = submit_resp.workorder_id;

    // Cancel it to make it terminal.
    let _ = client
        .delete(format!("{}/api/v1/workorders/{}", server.base_url, wo_id))
        .send()
        .await
        .expect("cancel");

    // Back-date the workorder's updated_at to guarantee it's past the cutoff.
    {
        let mut workorders = server.state.workorders.write().await;
        if let Some(wo) = workorders.get_mut(&wo_id) {
            wo.updated_at = chrono::Utc::now() - chrono::Duration::hours(2);
        }
    }

    // Trigger eviction.
    let evicted = server.state.evict_workorders().await;
    assert!(evicted > 0, "at least one workorder should be evicted");

    // Verify the workorder is gone (404).
    let resp = reqwest::get(format!("{}/api/v1/workorders/{}", server.base_url, wo_id))
        .await
        .expect("get");
    assert_eq!(
        resp.status(),
        404,
        "evicted workorder should return 404, got {}",
        resp.status()
    );
}

/// Max retained workorders cap — eviction removes oldest terminal entries.
#[cfg(feature = "integration")]
#[tokio::test]
async fn max_retained_workorders_enforced() {
    require_docker();

    // Create a server with very low max_retained_workorders cap.
    let port = common::free_port();
    let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
    let state_dir = tempfile::TempDir::new().expect("temp dir");

    let config = remerge_server::config::ServerConfig {
        binpkg_dir: binpkg_dir.path().to_path_buf(),
        binhost_url: format!("http://127.0.0.1:{port}/binpkgs"),
        state_dir: state_dir.path().to_path_buf(),
        max_retained_workorders: 2,
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let client = reqwest::Client::new();
    let mut wo_ids = Vec::new();

    // Submit and cancel 3 workorders to exceed the cap.
    for i in 0..3 {
        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec![format!("dev-libs/test-{i}")],
            emerge_args: vec![],
            portage_config: common::fixtures::minimal_portage_config(),
            system_id: common::fixtures::minimal_system_identity(),
        };

        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&req)
            .send()
            .await
            .expect("submit");
        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");
        wo_ids.push(submit_resp.workorder_id);

        // Cancel to make terminal.
        let _ = client
            .delete(format!(
                "{}/api/v1/workorders/{}",
                server.base_url, submit_resp.workorder_id
            ))
            .send()
            .await
            .expect("cancel");
    }

    // All 3 workorders should exist before eviction.
    let resp = reqwest::get(format!("{}/api/v1/workorders", server.base_url))
        .await
        .expect("list");
    let list: ListWorkordersResponse = resp.json().await.expect("parse list");
    assert_eq!(
        list.workorders.len(),
        3,
        "all 3 workorders should exist before eviction"
    );

    // Trigger eviction — cap is 2, so 1 should be removed.
    let evicted = server.state.evict_workorders().await;
    assert!(
        evicted >= 1,
        "at least 1 workorder should be evicted (cap=2, had 3)"
    );

    // Verify at most 2 workorders remain.
    let resp = reqwest::get(format!("{}/api/v1/workorders", server.base_url))
        .await
        .expect("list");
    let list: ListWorkordersResponse = resp.json().await.expect("parse list");
    assert!(
        list.workorders.len() <= 2,
        "at most 2 workorders should remain after eviction (cap=2), got {}",
        list.workorders.len()
    );
}

/// Oversized workorder body is rejected by axum's default body limit.
#[cfg(feature = "integration")]
#[tokio::test]
async fn oversized_workorder_rejected() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        request_body_size_bytes: 1024,
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let large_atom = "a".repeat(10_000);
    let req = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec![large_atom],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit request");

    assert_eq!(
        resp.status(),
        413,
        "oversized body should be rejected with 413 Payload Too Large when the configured limit is exceeded, got {}",
        resp.status()
    );
}

/// Submission is rejected when the configured non-terminal workorder capacity is full.
#[cfg(feature = "integration")]
#[tokio::test]
async fn queue_capacity_limit_rejects_new_workorders() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        max_active_workorders: 1,
        ..Default::default()
    };

    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let client = reqwest::Client::new();
    let first = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };
    let second = SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["sys-apps/portage".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    };

    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&first)
        .send()
        .await
        .expect("first submit");
    assert_eq!(resp.status(), 200, "first workorder should be accepted");

    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&second)
        .send()
        .await
        .expect("second submit");
    assert_eq!(
        resp.status(),
        503,
        "server should reject new workorders once max_active_workorders is reached"
    );
}
