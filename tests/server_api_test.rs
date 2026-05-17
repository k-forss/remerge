//! Phase 4 — Server API tests (in-process HTTP).
//!
//! Tests the axum HTTP API. Requires Docker to be available.
//! Tests are gated behind the `integration` feature flag to prevent
//! accidental omission in default CI. Docker and gpg are hard requirements
//! for this suite.

mod common;

#[cfg(feature = "integration")]
use futures_util::StreamExt;
#[cfg(feature = "integration")]
use remerge_types::api::*;

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
