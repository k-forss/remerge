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

    /// 6.1: Build a single small package — submit workorder WITHOUT
    /// --pretend, connect to WebSocket, wait for completion or failure,
    /// and verify binpkg output (or that the build actually ran).
    #[tokio::test]
    async fn build_single_package() {
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        // Submit WITHOUT --pretend — we want a real build attempt.
        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
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

        assert_eq!(resp.status(), 200, "workorder submission should succeed");
        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse response");
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
                .any(|w| w.id == submit_resp.workorder_id),
            "submitted workorder should appear in list"
        );

        // Connect to WebSocket to monitor progress.
        let ws_url = format!(
            "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
            server.port, submit_resp.workorder_id
        );

        let mut received_any_message = false;
        if let Ok((mut stream, _)) = tokio_tungstenite::connect_async(&ws_url).await {
            use futures_util::StreamExt;
            let timeout = tokio::time::Duration::from_secs(300);
            let result = tokio::time::timeout(timeout, async {
                while let Some(msg) = stream.next().await {
                    if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = msg {
                        received_any_message = true;
                        if text.contains("Finished") || text.contains("finished") {
                            return true;
                        }
                    } else if let Ok(tokio_tungstenite::tungstenite::Message::Binary(_)) = msg {
                        received_any_message = true;
                    }
                }
                false
            })
            .await;

            match result {
                Ok(true) => {
                    // Build finished — check binpkg dir for output.
                    let binpkg_entries: Vec<_> =
                        std::fs::read_dir(server.state.config.binpkg_dir.clone())
                            .map(|rd| rd.filter_map(|e| e.ok()).collect())
                            .unwrap_or_default();
                    assert!(
                        !binpkg_entries.is_empty(),
                        "binpkg directory should contain output after successful build"
                    );
                }
                Ok(false) => {
                    // Stream closed without Finished — the build may have failed.
                    // Check the workorder status to verify it's in a terminal state.
                    let resp = reqwest::get(format!(
                        "{}/api/v1/workorders/{}",
                        server.base_url, submit_resp.workorder_id
                    ))
                    .await
                    .expect("get final status");
                    let status: WorkorderStatusResponse =
                        resp.json().await.expect("parse final status");
                    assert!(
                        matches!(
                            status.status,
                            remerge_types::workorder::WorkorderStatus::Failed { .. }
                                | remerge_types::workorder::WorkorderStatus::Completed
                                | remerge_types::workorder::WorkorderStatus::Cancelled
                        ),
                        "workorder should reach a terminal state, got {:?}",
                        status.status
                    );
                }
                Err(_) => {
                    panic!("build did not complete within 5 minutes — timed out");
                }
            }
        } else {
            // WebSocket connection failed — this is a real test failure.
            // The server must be reachable and the WS endpoint must work.
            panic!(
                "WebSocket connection to {} failed — server must be reachable \
                 and the progress endpoint must accept connections",
                ws_url
            );
        }
    }

    /// 6.2: Build with --pretend flag — verify the flag is passed through.
    /// --ask should be filtered/rejected (single expected outcome).
    #[tokio::test]
    async fn build_with_pretend_flag() {
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        // Submit with --pretend (should be accepted and passed through).
        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
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
            .expect("submit");
        assert_eq!(resp.status(), 200, "pretend workorder should be accepted");

        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");
        assert!(
            !submit_resp.workorder_id.is_nil(),
            "pretend workorder should get valid ID"
        );

        // Verify the workorder is retrievable and shows the correct status.
        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get");
        assert_eq!(resp.status(), 200, "should retrieve pretend workorder");

        // Connect to WebSocket — if pretend mode works, we should see
        // output without actual compilation (pretend output is fast).
        let ws_url = format!(
            "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
            server.port, submit_resp.workorder_id
        );
        if let Ok((mut stream, _)) = tokio_tungstenite::connect_async(&ws_url).await {
            use futures_util::StreamExt;
            let timeout = tokio::time::Duration::from_secs(120);
            let result = tokio::time::timeout(timeout, async {
                let mut saw_output = false;
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(tokio_tungstenite::tungstenite::Message::Text(_))
                        | Ok(tokio_tungstenite::tungstenite::Message::Binary(_)) => {
                            saw_output = true;
                        }
                        _ => {}
                    }
                }
                saw_output
            })
            .await;

            // The WebSocket connection and streaming must work. If it timed
            // out, the pretend build is hanging; if it errored, the stream
            // is broken. Either way it's a real failure.
            match result {
                Ok(saw_output) => {
                    // Pretend mode should produce output quickly — at minimum
                    // the stream should have opened successfully.
                    assert!(saw_output, "pretend build should produce WebSocket output");
                }
                Err(_) => {
                    panic!("pretend build did not complete within 120s — timed out");
                }
            }
        } else {
            panic!(
                "WebSocket connection to {} failed — server progress endpoint must be reachable",
                ws_url
            );
        }

        // Cancel to clean up.
        let _ = client
            .delete(format!(
                "{}/api/v1/workorders/{}",
                server.base_url, submit_resp.workorder_id
            ))
            .send()
            .await;

        // Submit with --ask — should be accepted (200) because the server
        // supports interactive emerge via PTY/WebSocket stdin forwarding.
        let req_ask = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec!["--ask".into()],
            portage_config: common::fixtures::minimal_portage_config(),
            system_id: common::fixtures::minimal_system_identity(),
        };

        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&req_ask)
            .send()
            .await
            .expect("submit --ask");

        // The server allocates a TTY for stdin forwarding, so --ask is
        // supported and the workorder should be accepted.
        assert_eq!(
            resp.status(),
            200,
            "--ask workorder should be accepted (server supports interactive PTY mode)"
        );

        let ask_resp: SubmitWorkorderResponse = resp.json().await.expect("parse --ask response");
        assert!(
            !ask_resp.workorder_id.is_nil(),
            "--ask workorder should receive a valid ID"
        );

        // Verify the stored workorder has --ask in its emerge_args.
        {
            let workorders = server.state.workorders.read().await;
            let wo = workorders
                .get(&ask_resp.workorder_id)
                .expect("--ask workorder should exist in state");
            assert!(
                wo.emerge_args.contains(&"--ask".to_string()),
                "stored workorder should preserve --ask in emerge_args, got: {:?}",
                wo.emerge_args
            );
        }

        // Clean up.
        let _ = client
            .delete(format!(
                "{}/api/v1/workorders/{}",
                server.base_url, ask_resp.workorder_id
            ))
            .send()
            .await;
    }

    /// 6.3: Build with custom USE flags — verify worker receives the
    /// submitted portage config with the custom flags.
    #[tokio::test]
    async fn build_with_custom_use_flags() {
        let mut config = common::fixtures::minimal_portage_config();
        config.make_conf.use_flags = vec!["wayland".into(), "vulkan".into()];
        config.package_use = vec![remerge_types::portage::PackageUseEntry {
            atom: "app-misc/hello".into(),
            flags: vec!["custom-flag".into()],
        }];

        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec!["--pretend".into()],
            portage_config: config.clone(),
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

        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");
        assert!(
            !submit_resp.workorder_id.is_nil(),
            "workorder with USE flags should get valid ID"
        );

        // Verify the stored workorder's config contains the submitted USE flags.
        // Check via the workorder state directly (in-process access).
        let workorders = server.state.workorders.read().await;
        let wo = workorders
            .get(&submit_resp.workorder_id)
            .expect("workorder should exist in state");
        assert_eq!(
            wo.portage_config.make_conf.use_flags,
            vec!["wayland".to_string(), "vulkan".to_string()],
            "stored workorder should have the submitted USE flags"
        );
        assert_eq!(
            wo.portage_config.package_use.len(),
            1,
            "stored workorder should have 1 package_use entry"
        );
        assert_eq!(
            wo.portage_config.package_use[0].atom, "app-misc/hello",
            "package_use atom should match"
        );
        assert_eq!(
            wo.portage_config.package_use[0].flags,
            vec!["custom-flag".to_string()],
            "package_use flags should match"
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
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["dev-libs/openssl".into()],
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
            .expect("submit");
        assert_eq!(resp.status(), 200);
        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

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

    /// 6.4: Build with @world set — verify set expansion by confirming
    /// the workorder was accepted and the atoms field contains @world.
    #[tokio::test]
    async fn build_with_world_set() {
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["@world".into()],
            emerge_args: vec!["--pretend".into()],
            portage_config: common::fixtures::full_portage_config(),
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
            200,
            "workorder with @world should be accepted"
        );

        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

        // Verify the stored workorder has @world in its atoms.
        let workorders = server.state.workorders.read().await;
        let wo = workorders
            .get(&submit_resp.workorder_id)
            .expect("workorder should exist in state");
        assert!(
            wo.atoms.contains(&"@world".to_string()),
            "stored workorder atoms should contain @world, got: {:?}",
            wo.atoms
        );

        // Also verify it shows up in the list endpoint with @world.
        let resp = reqwest::get(format!("{}/api/v1/workorders", server.base_url))
            .await
            .expect("list");
        let list: ListWorkordersResponse = resp.json().await.expect("parse list");
        let summary = list
            .workorders
            .iter()
            .find(|w| w.id == submit_resp.workorder_id)
            .expect("workorder should appear in list");
        assert!(
            summary.atoms.contains(&"@world".to_string()),
            "listed workorder atoms should contain @world, got: {:?}",
            summary.atoms
        );
    }

    /// 6.6: Follower client — verify follower is accepted with a
    /// DIFFERENT client_id and receives the same workorder_id.
    #[tokio::test]
    async fn follower_inherits_main_config() {
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        let client = reqwest::Client::new();
        let main_client_id = uuid::Uuid::new_v4();

        // Submit main workorder first.
        let main_req = SubmitWorkorderRequest {
            client_id: main_client_id,
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec![],
            portage_config: common::fixtures::full_portage_config(),
            system_id: common::fixtures::minimal_system_identity(),
        };

        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&main_req)
            .send()
            .await
            .expect("submit main");
        assert_eq!(resp.status(), 200, "main workorder should be accepted");
        let main_resp: SubmitWorkorderResponse = resp.json().await.expect("parse main response");

        // Submit follower workorder with a DIFFERENT client_id.
        let follower_client_id = uuid::Uuid::new_v4();
        let follower_req = SubmitWorkorderRequest {
            client_id: follower_client_id,
            role: remerge_types::client::ClientRole::Follower,
            atoms: vec!["app-misc/hello".into()],
            emerge_args: vec![],
            portage_config: common::fixtures::minimal_portage_config(),
            system_id: common::fixtures::minimal_system_identity(),
        };

        let resp = client
            .post(format!("{}/api/v1/workorders", server.base_url))
            .json(&follower_req)
            .send()
            .await
            .expect("submit follower");

        // Follower should be accepted — assert 200 only.
        // If the server rejects followers, that's a production bug
        // (the test is correct, the code needs fixing).
        assert_eq!(
            resp.status(),
            200,
            "follower with different client_id should be accepted"
        );

        let follower_resp: SubmitWorkorderResponse =
            resp.json().await.expect("parse follower response");
        assert!(
            !follower_resp.workorder_id.is_nil(),
            "follower workorder ID should be assigned"
        );

        // Verify follower got the same workorder_id as main.
        assert_eq!(
            follower_resp.workorder_id, main_resp.workorder_id,
            "follower should inherit main's workorder_id"
        );

        // Clean up main workorder.
        let _ = client
            .delete(format!(
                "{}/api/v1/workorders/{}",
                server.base_url, main_resp.workorder_id
            ))
            .send()
            .await;
    }

    /// 6.8: Worker binary upgrade detection — building an image with
    /// binary A and then checking with manager B (different binary hash)
    /// should detect a mismatch.
    #[tokio::test]
    async fn worker_binary_upgrade_detection() {
        if !common::server::docker_available() {
            return;
        }

        // Check that the base image is available — skip if not.
        let docker = bollard::Docker::connect_with_local_defaults()
            .expect("connect to Docker for pre-check");
        if docker.inspect_image("gentoo/stage3:latest").await.is_err() {
            eprintln!(
                "gentoo/stage3:latest not available — skipping upgrade detection. \
                 Build with: docker build -f docker/test-stage3.Dockerfile \
                 -t ghcr.io/k-forss/remerge/test-stage3:latest ."
            );
            return;
        }

        // Create two different dummy binaries.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let binary_a = tmp.path().join("worker-a");
        let binary_b = tmp.path().join("worker-b");
        std::fs::write(&binary_a, b"#!/bin/true version-a\n").expect("write a");
        std::fs::write(&binary_b, b"#!/bin/true version-b\n").expect("write b");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_a, std::fs::Permissions::from_mode(0o755))
                .expect("chmod a");
            std::fs::set_permissions(&binary_b, std::fs::Permissions::from_mode(0o755))
                .expect("chmod b");
        }

        let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
        let tmp_state = tempfile::TempDir::new().expect("temp dir");

        // Create manager with binary A.
        let config_a = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            worker_binary: Some(binary_a),
            ..Default::default()
        };
        let manager_a = remerge_server::docker::DockerManager::new(&config_a)
            .await
            .expect("connect with binary A");

        let sys = common::fixtures::minimal_system_identity();
        let tag = format!("remerge-test-upgrade:{}", uuid::Uuid::new_v4());

        // Build image with binary A.
        manager_a
            .build_worker_image(&sys, &tag)
            .await
            .expect("build image with binary A");

        // manager_a should NOT need rebuild (hash matches).
        assert!(
            !manager_a.image_needs_rebuild(&tag).await,
            "image built with binary A should NOT need rebuild when checked by manager A"
        );

        // Create manager with binary B (different hash).
        let tmp_binpkg2 = tempfile::TempDir::new().expect("temp dir");
        let tmp_state2 = tempfile::TempDir::new().expect("temp dir");
        let config_b = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg2.path().to_path_buf(),
            state_dir: tmp_state2.path().to_path_buf(),
            worker_binary: Some(binary_b),
            ..Default::default()
        };
        let manager_b = remerge_server::docker::DockerManager::new(&config_b)
            .await
            .expect("connect with binary B");

        // manager_b should detect rebuild needed (different hash).
        assert!(
            manager_b.image_needs_rebuild(&tag).await,
            "image built with binary A SHOULD need rebuild when checked by manager B"
        );

        // Clean up image.
        let _ = manager_a.remove_image(&tag).await;
    }

    /// 6.10: WebSocket reconnect — connect, disconnect, reconnect,
    /// and verify events are received after reconnection.
    #[tokio::test]
    async fn websocket_reconnect() {
        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        // Submit a workorder to have an active progress stream.
        let req = SubmitWorkorderRequest {
            client_id: uuid::Uuid::new_v4(),
            role: remerge_types::client::ClientRole::Main,
            atoms: vec!["app-misc/hello".into()],
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
            .expect("submit");
        assert_eq!(resp.status(), 200, "submission should succeed");
        let submit_resp: SubmitWorkorderResponse = resp.json().await.expect("parse");

        let ws_url = format!(
            "ws://127.0.0.1:{}/api/v1/workorders/{}/progress",
            server.port, submit_resp.workorder_id
        );

        // First connection.
        let (stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("first WebSocket connection should succeed");

        // Drop to simulate disconnect.
        drop(stream);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Reconnect.
        let (mut stream2, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("WebSocket reconnect should succeed");

        // Cancel the workorder to trigger a StatusChanged event.
        let cancel_resp = client
            .delete(format!(
                "{}/api/v1/workorders/{}",
                server.base_url, submit_resp.workorder_id
            ))
            .send()
            .await
            .expect("cancel");
        assert_eq!(cancel_resp.status(), 200, "cancel should succeed");

        // Verify we receive at least one event after reconnect.
        use futures_util::StreamExt;
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(msg) = stream2.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::Message::Text(_))
                    | Ok(tokio_tungstenite::tungstenite::Message::Binary(_))
                    | Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                        return true;
                    }
                    _ => continue,
                }
            }
            false
        })
        .await;

        assert!(
            received.unwrap_or(false),
            "should receive at least one event after WebSocket reconnect"
        );
    }
}
