#![cfg(feature = "integration")]

mod common;

use remerge_types::api::{ListWorkordersResponse, ServerInfoResponse, SubmitWorkorderRequest};
use reqwest::StatusCode;
use tokio::task::JoinSet;

fn require_docker() {
    assert!(
        common::server::docker_available(),
        "Docker must be available for load_test; run with a Docker daemon"
    );
}

fn make_request() -> SubmitWorkorderRequest {
    SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
    }
}

async fn submit_batch(server: &common::server::TestServer, total: usize) -> Vec<StatusCode> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/workorders", server.base_url);
    let mut tasks = JoinSet::new();

    for _ in 0..total {
        let request = make_request();
        let client = client.clone();
        let url = url.clone();
        tasks.spawn(async move {
            client
                .post(url)
                .json(&request)
                .send()
                .await
                .expect("submit request")
                .status()
        });
    }

    let mut statuses = Vec::with_capacity(total);
    while let Some(result) = tasks.join_next().await {
        statuses.push(result.expect("join submit task"));
    }
    statuses
}

async fn fetch_counts(server: &common::server::TestServer) -> (usize, usize) {
    let list: ListWorkordersResponse =
        reqwest::get(format!("{}/api/v1/workorders", server.base_url))
            .await
            .expect("list workorders")
            .json()
            .await
            .expect("parse workorder list");

    let info: ServerInfoResponse = reqwest::get(format!("{}/api/v1/info", server.base_url))
        .await
        .expect("get server info")
        .json()
        .await
        .expect("parse server info");

    (list.workorders.len(), info.queued_workorders)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_submissions_fill_queue_within_capacity() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        max_active_workorders: 12,
        ..Default::default()
    };
    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let statuses = submit_batch(&server, 12).await;
    assert!(
        statuses.iter().all(|status| *status == StatusCode::OK),
        "all submissions within configured capacity should be accepted, got {statuses:?}"
    );

    let (listed, queued) = fetch_counts(&server).await;
    assert_eq!(listed, 12, "all accepted workorders should remain listed");
    assert_eq!(queued, 12, "queued count should match accepted submissions");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_submissions_reject_over_capacity() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        max_active_workorders: 8,
        ..Default::default()
    };
    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let statuses = submit_batch(&server, 24).await;
    let accepted = statuses
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    let rejected = statuses
        .iter()
        .filter(|status| **status == StatusCode::SERVICE_UNAVAILABLE)
        .count();

    assert_eq!(accepted, 8, "accepts should stop exactly at queue capacity");
    assert_eq!(rejected, 16, "all excess submissions should be rejected");
    assert!(
        statuses
            .iter()
            .all(|status| matches!(*status, StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE)),
        "load test should only observe 200 or 503 responses, got {statuses:?}"
    );

    let (listed, queued) = fetch_counts(&server).await;
    assert_eq!(
        listed, 8,
        "rejected submissions must not leak retained state"
    );
    assert_eq!(
        queued, 8,
        "queued count should stay capped at configured capacity"
    );
}

#[ignore = "stress harness; run explicitly when validating higher concurrency envelopes"]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_submission_stress_harness() {
    require_docker();

    let config = remerge_server::config::ServerConfig {
        max_active_workorders: 32,
        ..Default::default()
    };
    let server = common::server::TestServer::start_with_config(Some(config)).await;

    let statuses = submit_batch(&server, 96).await;
    let accepted = statuses
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    let rejected = statuses
        .iter()
        .filter(|status| **status == StatusCode::SERVICE_UNAVAILABLE)
        .count();

    assert_eq!(
        accepted, 32,
        "stress harness should still honor queue capacity"
    );
    assert_eq!(
        rejected, 64,
        "overflow submissions should be rejected under stress"
    );

    let (listed, queued) = fetch_counts(&server).await;
    assert_eq!(
        listed, 32,
        "stress run should retain only accepted workorders"
    );
    assert_eq!(queued, 32, "stress run queue depth should remain capped");
}
