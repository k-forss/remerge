//! Phase 4 — Server API tests (in-process HTTP).
//!
//! These tests start the remerge-server in-process and exercise
//! the HTTP API endpoints. Requires Docker to be available.
//!
//! Gated behind the `integration` feature.

mod common;

#[cfg(feature = "integration")]
mod server_api {
    use super::common;

    #[tokio::test]
    async fn server_requires_docker() {
        // This test documents the Docker requirement.
        // If Docker is not available, TestServer::start() returns None.
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping server API tests");
            return;
        }

        let server = common::server::TestServer::start().await;
        assert!(server.is_some(), "Server should start when Docker is available");
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        let Some(server) = common::server::TestServer::start().await else {
            eprintln!("Server failed to start — skipping");
            return;
        };

        let resp = reqwest::get(format!("{}/api/v1/health", server.base_url))
            .await
            .expect("health request");
        assert!(resp.status().is_success());
    }

    #[tokio::test]
    async fn info_endpoint_returns_json() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        let Some(server) = common::server::TestServer::start().await else {
            eprintln!("Server failed to start — skipping");
            return;
        };

        let resp = reqwest::get(format!("{}/api/v1/info", server.base_url))
            .await
            .expect("info request");
        assert!(resp.status().is_success());

        let body = resp.text().await.expect("body");
        let _json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    }
}
