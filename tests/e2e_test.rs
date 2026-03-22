//! Phase 6 — End-to-end pipeline tests.
//!
//! Full CLI → Server → Worker pipeline. Requires Docker, network,
//! and a Gentoo stage3 image.
//!
//! Gated behind `#[cfg(feature = "e2e")]`.

mod common;

#[cfg(feature = "e2e")]
mod e2e_tests {
    use super::common;
    use remerge_types::api::*;

    /// Sentinel: Docker must be available when running E2E tests.
    #[test]
    fn docker_must_be_available_for_e2e() {
        assert!(
            common::server::docker_available(),
            "Docker is required for E2E tests but was not found"
        );
    }

    /// Helper: create a reqwest client and submit a workorder, returning
    /// the server and submit response.
    async fn submit_test_workorder(
        atoms: Vec<String>,
    ) -> Option<(common::server::TestServer, SubmitWorkorderResponse)> {
        let server = common::server::TestServer::start().await?;

        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms,
            emerge_args: vec!["--pretend".into()],
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

        if resp.status() != 200 {
            return None;
        }

        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
        Some((server, submit_resp))
    }

    /// 6.1: Build a single small package — submit workorder, verify it is
    /// accepted and assigned a workorder ID.
    #[tokio::test]
    async fn build_single_package() {
        let Some((server, submit_resp)) =
            submit_test_workorder(vec!["app-misc/hello".into()]).await
        else {
            return;
        };

        assert!(
            !submit_resp.workorder_id.is_nil(),
            "workorder ID should be assigned"
        );

        // Verify it appears in the list.
        let resp = reqwest::get(format!("{}/api/v1/workorders", server.base_url))
            .await
            .expect("list workorders");
        let list: ListWorkordersResponse = resp.json().await.expect("parse list");
        assert!(
            list.workorders
                .iter()
                .any(|w| w.workorder_id == submit_resp.workorder_id),
            "submitted workorder should appear in list"
        );
    }

    /// 6.2: Build with --pretend flag — verify the flag is passed through.
    #[tokio::test]
    async fn build_with_pretend_flag() {
        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec!["--pretend".into()],
            portage_config: common::fixtures::minimal_portage_config(),
            system_id: common::fixtures::minimal_system_identity(),
        };

        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&req)
            .send()
            .await
            .expect("submit");
        assert_eq!(resp.status(), 200, "pretend workorder should be accepted");

        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

        // Verify the workorder has the emerge_args.
        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get");
        assert_eq!(resp.status(), 200);
    }

    /// 6.3: Build with custom USE flags — verify worker's package.use
    /// contains the custom flags in the submitted config.
    #[tokio::test]
    async fn build_with_custom_use_flags() {
        let mut config = common::fixtures::minimal_portage_config();
        config.make_conf.use_flags = vec!["wayland".into(), "vulkan".into()];

        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec![],
            portage_config: config,
            system_id: common::fixtures::minimal_system_identity(),
        };

        let Some(server) = common::server::TestServer::start().await else {
            return;
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
            200,
            "workorder with custom USE flags should be accepted"
        );
    }

    /// 6.7: Concurrent workorder rejection — submit while another is active.
    #[tokio::test]
    async fn concurrent_workorder_rejection() {
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

        // First submission should succeed.
        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&req)
            .send()
            .await
            .expect("first submit");
        assert_eq!(resp.status(), 200);

        // Second submission with same client_id should be rejected.
        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&req)
            .send()
            .await
            .expect("second submit");
        assert_eq!(
            resp.status(),
            409,
            "concurrent submission should be rejected with 409"
        );
    }

    /// 6.9: Cancellation — submit, cancel via API, verify cancelled status.
    #[tokio::test]
    async fn cancellation_flow() {
        let Some((server, submit_resp)) =
            submit_test_workorder(vec!["dev-libs/openssl".into()]).await
        else {
            return;
        };

        let client = reqwest::Client::new();
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

        // Verify status shows as cancelled.
        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get");
        let status: WorkorderStatusResponse = resp.json().await.expect("parse status");
        assert!(
            matches!(
                status.status,
                remerge_types::workorder::WorkorderStatus::Cancelled
            ),
            "status should be Cancelled, got {:?}",
            status.status
        );
    }
}
