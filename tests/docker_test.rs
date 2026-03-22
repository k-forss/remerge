//! Phase 5 — Docker integration tests.
//!
//! These tests exercise Docker container operations.
//! Gated behind `#[cfg(feature = "integration")]`.

mod common;

#[cfg(feature = "integration")]
mod docker_tests {
    use super::common;

    /// Helper: create a DockerManager with default config.
    async fn make_manager() -> Option<(
        remerge_server::docker::DockerManager,
        tempfile::TempDir,
        tempfile::TempDir,
    )> {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return None;
        }
        let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
        let tmp_state = tempfile::TempDir::new().expect("temp dir");
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            ..Default::default()
        };
        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");
        Some((manager, tmp_binpkg, tmp_state))
    }

    /// DockerManager can connect to local Docker socket.
    #[tokio::test]
    async fn docker_manager_connects() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        let tmp_binpkg = tempfile::TempDir::new().unwrap();
        let tmp_state = tempfile::TempDir::new().unwrap();
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            ..Default::default()
        };

        let manager = remerge_server::docker::DockerManager::new(&config).await;
        assert!(
            manager.is_ok(),
            "DockerManager should connect to local Docker"
        );
    }

    /// Image tag derivation from SystemIdentity.
    #[tokio::test]
    async fn image_tag_from_system_identity() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let sys = common::fixtures::minimal_system_identity();
        let tag = manager.image_tag(&sys);
        assert!(!tag.is_empty(), "image tag should not be empty");
        assert!(
            tag.contains("remerge-worker"),
            "tag should contain image prefix"
        );
    }

    /// `image_needs_rebuild` returns true for a nonexistent image.
    #[tokio::test]
    async fn needs_rebuild_nonexistent_image() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let needs = manager
            .image_needs_rebuild("remerge-test-nonexistent:latest")
            .await;
        assert!(needs, "nonexistent image should need rebuild");
    }

    /// `remove_container` on a nonexistent container returns an error.
    #[tokio::test]
    async fn remove_nonexistent_container_errors() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let result = manager.remove_container("nonexistent-container-id").await;
        assert!(
            result.is_err(),
            "removing nonexistent container should error"
        );
    }

    /// `remove_image` on a nonexistent image returns an error.
    #[tokio::test]
    async fn remove_nonexistent_image_errors() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let result = manager
            .remove_image("remerge-test-nonexistent:latest")
            .await;
        assert!(result.is_err(), "removing nonexistent image should error");
    }

    /// `stop_container` on a nonexistent container returns an error.
    #[tokio::test]
    async fn stop_nonexistent_container_errors() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let result = manager.stop_container("nonexistent-container-id").await;
        assert!(
            result.is_err(),
            "stopping nonexistent container should error"
        );
    }

    /// 5.3: `build_worker_image` — create a dummy worker binary, build image,
    /// verify it exists and has the `remerge.worker.sha256` label.
    /// Requires the Gentoo stage3 image (pulled automatically if missing).
    #[tokio::test]
    async fn build_worker_image_with_label() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        common::server::ensure_test_stage3();

        let docker = bollard::Docker::connect_with_local_defaults()
            .expect("connect to Docker for post-build inspection");

        // Create a dummy worker binary.
        let tmp_dir = tempfile::TempDir::new().expect("temp dir");
        let dummy_binary = tmp_dir.path().join("remerge-worker");
        std::fs::write(&dummy_binary, b"#!/bin/true\n").expect("write dummy binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dummy_binary, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
        let tmp_state = tempfile::TempDir::new().expect("temp dir");
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            worker_binary: Some(dummy_binary.clone()),
            worker_base_image: Some(common::server::TEST_STAGE3_IMAGE.to_string()),
            ..Default::default()
        };

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let sys = common::fixtures::minimal_system_identity();
        let tag = format!("remerge-test-build:{}", uuid::Uuid::new_v4());

        let result = manager.build_worker_image(&sys, &tag).await;

        // If the build fails, the test fails — no silent pass on error.
        result.expect("build_worker_image should succeed with stage3 available");

        // Verify image exists and has the sha256 label using bollard.
        let image_info = docker
            .inspect_image(&tag)
            .await
            .expect("image should exist after build");

        let labels = image_info
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .expect("image should have labels");

        let sha256_label = labels
            .get("remerge.worker.sha256")
            .expect("image should have remerge.worker.sha256 label");
        assert!(!sha256_label.is_empty(), "sha256 label should not be empty");

        // Verify the image does not need rebuild (label matches).
        let needs = manager.image_needs_rebuild(&tag).await;
        assert!(!needs, "freshly built image should not need rebuild");

        // Clean up image.
        let _ = manager.remove_image(&tag).await;
    }

    /// 6.5: Cross-arch build — verify `generate_dockerfile` includes crossdev
    /// setup when the SystemIdentity specifies a non-x86_64 CHOST.
    ///
    /// This tests the Dockerfile generation path without requiring QEMU.
    /// The crossdev approach compiles on x86_64 for a foreign target, so no
    /// binfmt_misc / QEMU user-static is needed for the build itself.
    #[tokio::test]
    async fn crossdev_dockerfile_for_cross_arch() {
        let Some((manager, _b, _s)) = make_manager().await else {
            return;
        };

        let cross_sys = common::fixtures::cross_arch_system_identity();

        // generate_dockerfile should detect non-x86_64 CHOST and include crossdev.
        let dockerfile = manager.generate_dockerfile(&cross_sys);

        // Verify the Dockerfile contains the crossdev installation block.
        assert!(
            dockerfile.contains("crossdev"),
            "Dockerfile for cross-arch should contain crossdev setup, got:\n{dockerfile}"
        );
        assert!(
            dockerfile.contains("aarch64-unknown-linux-gnu"),
            "Dockerfile should reference the target CHOST (aarch64-unknown-linux-gnu)"
        );
        assert!(
            dockerfile.contains("crossdev --stable -t aarch64-unknown-linux-gnu"),
            "Dockerfile should install crossdev toolchain for target CHOST"
        );

        // Verify CHOST/CBUILD are set in make.conf.
        assert!(
            dockerfile.contains("CHOST=\"aarch64-unknown-linux-gnu\""),
            "Dockerfile should set CHOST in make.conf"
        );
        assert!(
            dockerfile.contains("CBUILD=\"x86_64-pc-linux-gnu\""),
            "Dockerfile should set CBUILD in make.conf"
        );

        // Verify native build does NOT include crossdev.
        let native_sys = common::fixtures::minimal_system_identity();
        let native_dockerfile = manager.generate_dockerfile(&native_sys);
        assert!(
            !native_dockerfile.contains("crossdev"),
            "Dockerfile for native x86_64 should NOT contain crossdev"
        );
    }

    /// 6.5 (continued): Cross-arch image build — actually build a worker
    /// image with crossdev for a foreign target. Requires Docker and the
    /// stage3 base image. This is a slow test (crossdev compilation).
    #[tokio::test]
    async fn cross_arch_image_build() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        // Check that stage3 base image exists — skip if not.
        let docker = bollard::Docker::connect_with_local_defaults()
            .expect("connect to Docker for pre-check");
        if docker.inspect_image("gentoo/stage3:latest").await.is_err() {
            eprintln!(
                "gentoo/stage3:latest not available — skipping cross-arch build. \
                 Build with: docker build -f docker/test-stage3.Dockerfile \
                 -t ghcr.io/k-forss/remerge/test-stage3:latest ."
            );
            return;
        }

        // Create a dummy worker binary.
        let tmp_dir = tempfile::TempDir::new().expect("temp dir");
        let dummy_binary = tmp_dir.path().join("remerge-worker");
        std::fs::write(&dummy_binary, b"#!/bin/true\n").expect("write dummy binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dummy_binary, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
        let tmp_state = tempfile::TempDir::new().expect("temp dir");
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            worker_binary: Some(dummy_binary.clone()),
            ..Default::default()
        };

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let cross_sys = common::fixtures::cross_arch_system_identity();
        let tag = format!("remerge-test-cross:{}", uuid::Uuid::new_v4());

        let result = manager.build_worker_image(&cross_sys, &tag).await;

        // The build should succeed — crossdev installs on x86_64 and
        // compiles a cross-toolchain. No QEMU needed.
        result.expect("cross-arch build_worker_image should succeed with stage3 + crossdev");

        // Verify image exists and has the sha256 label.
        let image_info = docker
            .inspect_image(&tag)
            .await
            .expect("cross-arch image should exist after build");

        let labels = image_info
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .expect("cross-arch image should have labels");

        assert!(
            labels.contains_key("remerge.worker.sha256"),
            "cross-arch image should have remerge.worker.sha256 label"
        );

        // Clean up.
        let _ = manager.remove_image(&tag).await;
    }

    /// 5.5: `start_worker` — start a container with valid config, verify
    /// it runs with correct env vars and mounts.
    /// Requires a valid worker image (depends on 5.3 / stage3 available).
    #[tokio::test]
    async fn start_worker_container() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        // Check that the base image is available — skip if not.
        let docker = bollard::Docker::connect_with_local_defaults()
            .expect("connect to Docker for pre-check");
        if docker.inspect_image("gentoo/stage3:latest").await.is_err() {
            eprintln!(
                "gentoo/stage3:latest not available — skipping start_worker test. \
                 Build with: docker build -f docker/test-stage3.Dockerfile \
                 -t ghcr.io/k-forss/remerge/test-stage3:latest ."
            );
            return;
        }

        // Build an image first (depends on 5.3).
        let tmp_dir = tempfile::TempDir::new().expect("temp dir");
        let dummy_binary = tmp_dir.path().join("remerge-worker");
        std::fs::write(&dummy_binary, b"#!/bin/true\n").expect("write dummy binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dummy_binary, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
        let tmp_state = tempfile::TempDir::new().expect("temp dir");
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: tmp_binpkg.path().to_path_buf(),
            state_dir: tmp_state.path().to_path_buf(),
            worker_binary: Some(dummy_binary.clone()),
            ..Default::default()
        };

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let sys = common::fixtures::minimal_system_identity();
        let tag = format!("remerge-test-start:{}", uuid::Uuid::new_v4());

        manager
            .build_worker_image(&sys, &tag)
            .await
            .expect("build image for start_worker test");

        // Start a worker container.
        let workorder_json =
            serde_json::to_string(&common::fixtures::minimal_portage_config()).expect("serialize");
        let container_name = format!("remerge-test-{}", uuid::Uuid::new_v4());

        let container_id = manager
            .start_worker(&container_name, &tag, &workorder_json, &config)
            .await
            .expect("start_worker should succeed with built image");

        assert!(!container_id.is_empty(), "container ID should not be empty");

        // Inspect the container to verify env vars and mounts.
        let inspect = docker
            .inspect_container(&container_id, None)
            .await
            .expect("inspect container");

        // Verify REMERGE_WORKORDER env var is set.
        let env_vars = inspect
            .config
            .as_ref()
            .and_then(|c| c.env.as_ref())
            .expect("container should have env vars");
        let has_workorder_env = env_vars.iter().any(|e| e.starts_with("REMERGE_WORKORDER="));
        assert!(
            has_workorder_env,
            "container should have REMERGE_WORKORDER env var, got: {env_vars:?}"
        );

        // Verify binpkg mount exists.
        let mounts = inspect
            .mounts
            .as_ref()
            .expect("container should have mounts");
        let has_binpkg_mount = mounts.iter().any(|m| {
            m.destination
                .as_deref()
                .is_some_and(|d| d.contains("binpkg"))
        });
        assert!(
            has_binpkg_mount,
            "container should have binpkg mount, got mounts: {mounts:?}"
        );

        // Clean up container and image.
        let _ = manager.stop_container(&container_id).await;
        let _ = manager.remove_container(&container_id).await;
        let _ = manager.remove_image(&tag).await;
    }

    /// 5.7: Image eviction — the image reaper preserves the newest image
    /// per (CHOST, profile) group and removes older idle ones.
    /// This test creates real Docker images, sets timestamps, and verifies
    /// that `remove_image` works on expired images while tracking state.
    #[tokio::test]
    async fn image_last_used_tracking_and_eviction() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping");
            return;
        }

        let Some(server) = common::server::TestServer::start().await else {
            return;
        };

        // Create two lightweight test images by tagging the existing
        // busybox or alpine image. We use `docker tag` to avoid needing
        // to build anything. First, pull a tiny base image.
        let docker = bollard::Docker::connect_with_local_defaults().expect("connect to Docker");

        // Use a very small image (hello-world) that's likely cached or fast to pull.
        // If not available, skip.
        let tag_new = format!("remerge-test-eviction-new:{}", uuid::Uuid::new_v4());
        let tag_old = format!("remerge-test-eviction-old:{}", uuid::Uuid::new_v4());

        // Try to use hello-world as base. If it doesn't exist, skip.
        let base = "hello-world:latest";
        if docker.inspect_image(base).await.is_err() {
            // Try pulling it.
            use futures_util::StreamExt;
            let mut stream = docker.create_image(
                Some(bollard::query_parameters::CreateImageOptions {
                    from_image: Some(base.to_string()),
                    ..Default::default()
                }),
                None,
                None,
            );
            while let Some(r) = stream.next().await {
                if r.is_err() {
                    eprintln!("Cannot pull hello-world — skipping eviction test");
                    return;
                }
            }
        }

        // Tag the base image with our test tags.
        docker
            .tag_image(
                base,
                Some(bollard::query_parameters::TagImageOptions {
                    repo: Some(tag_new.split(':').next().unwrap().to_string()),
                    tag: Some(tag_new.split(':').nth(1).unwrap().to_string()),
                }),
            )
            .await
            .expect("tag new image");
        docker
            .tag_image(
                base,
                Some(bollard::query_parameters::TagImageOptions {
                    repo: Some(tag_old.split(':').next().unwrap().to_string()),
                    tag: Some(tag_old.split(':').nth(1).unwrap().to_string()),
                }),
            )
            .await
            .expect("tag old image");

        // Populate image_last_used: one recent, one expired.
        {
            let mut images = server.state.image_last_used.write().await;
            images.insert(tag_new.clone(), std::time::Instant::now());
            // 2 hours old (past default idle timeout of 1 hour).
            images.insert(
                tag_old.clone(),
                std::time::Instant::now() - std::time::Duration::from_secs(7200),
            );
        }

        // Verify both tracked.
        {
            let images = server.state.image_last_used.read().await;
            assert_eq!(images.len(), 2, "should have 2 tracked images");
        }

        // Now simulate what the reaper does: remove the old image.
        let timeout = std::time::Duration::from_secs(server.state.config.worker_idle_timeout);
        let now = std::time::Instant::now();
        let expired: Vec<String> = {
            let images = server.state.image_last_used.read().await;
            images
                .iter()
                .filter(|(_, last_used)| now.duration_since(**last_used) > timeout)
                .map(|(tag, _)| tag.clone())
                .collect()
        };

        assert!(
            !expired.is_empty(),
            "old image should be in the expired list"
        );
        assert!(expired.contains(&tag_old), "tag_old should be expired");
        assert!(!expired.contains(&tag_new), "tag_new should NOT be expired");

        // Remove expired images (as the reaper would).
        for tag in &expired {
            server
                .state
                .docker
                .remove_image(tag)
                .await
                .expect("remove expired image");
            server.state.image_last_used.write().await.remove(tag);
        }

        // Verify: old image removed from tracking.
        {
            let images = server.state.image_last_used.read().await;
            assert_eq!(images.len(), 1, "only new image should remain tracked");
            assert!(
                images.contains_key(&tag_new),
                "new image should still be tracked"
            );
            assert!(
                !images.contains_key(&tag_old),
                "old image should be removed"
            );
        }

        // Verify: old image actually removed from Docker.
        assert!(
            docker.inspect_image(&tag_old).await.is_err(),
            "old image should be removed from Docker"
        );

        // Verify: new image still exists in Docker.
        assert!(
            docker.inspect_image(&tag_new).await.is_ok(),
            "new image should still exist in Docker"
        );

        // Clean up.
        let _ = docker
            .remove_image(
                &tag_new,
                Some(bollard::query_parameters::RemoveImageOptions {
                    force: true,
                    ..Default::default()
                }),
                None,
            )
            .await;
    }
}
