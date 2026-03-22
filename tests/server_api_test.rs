//! Phase 4 — Server API tests (in-process HTTP).
//!
//! Tests the axum HTTP API. Requires Docker to be available.
//! Tests skip gracefully when Docker is not present.

mod common;

use remerge_types::api::*;

/// Helper to skip tests when Docker is not available.
fn require_docker() -> bool {
    if !common::server::docker_available() {
        eprintln!("Docker not available — skipping server API test");
        false
    } else {
        true
    }
}

/// GET /api/v1/health returns 200 with status "ok".
#[tokio::test]
async fn health_endpoint() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    let resp = reqwest::get(format!("{}/api/v1/health", server.base_url))
        .await
        .expect("health request");
    assert_eq!(resp.status(), 200);

    let health: HealthResponse = resp.json().await.expect("parse health");
    assert_eq!(health.status, "ok");
    assert!(!health.version.is_empty());
}

/// GET /api/v1/info returns server info with version and auth_mode.
#[tokio::test]
async fn info_endpoint() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    let resp = reqwest::get(format!("{}/api/v1/info", server.base_url))
        .await
        .expect("info request");
    assert_eq!(resp.status(), 200);

    let info: ServerInfoResponse = resp.json().await.expect("parse info");
    assert!(!info.version.is_empty());
    assert!(!info.binhost_base_url.is_empty());
    assert_eq!(info.auth_mode, remerge_types::auth::AuthMode::None);
}

/// GET /metrics returns Prometheus-formatted text with remerge_ prefix.
#[tokio::test]
async fn metrics_endpoint() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
}

/// POST /api/v1/workorders with valid atoms returns 200 and workorder ID.
#[tokio::test]
async fn submit_workorder_valid() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn submit_workorder_invalid_atoms() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn submit_workorder_duplicate_active() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn get_workorder() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn list_workorders() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn cancel_workorder() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn get_nonexistent_workorder() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
#[tokio::test]
async fn follower_without_main_rejected() {
    if !require_docker() {
        return;
    }
    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

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
