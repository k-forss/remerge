//! Phase 5 — Docker integration tests.
//!
//! These tests exercise Docker container operations.
//! Gated behind `#[cfg(feature = "integration")]`.

mod common;

#[cfg(feature = "integration")]
mod docker_tests {
    use super::common;

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

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

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

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let needs = manager
            .image_needs_rebuild("remerge-test-nonexistent:latest")
            .await;
        assert!(needs, "nonexistent image should need rebuild");
    }

    /// `remove_container` on a nonexistent container returns an error.
    #[tokio::test]
    async fn remove_nonexistent_container_errors() {
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

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let result = manager.remove_container("nonexistent-container-id").await;
        assert!(
            result.is_err(),
            "removing nonexistent container should error"
        );
    }

    /// `remove_image` on a nonexistent image returns an error.
    #[tokio::test]
    async fn remove_nonexistent_image_errors() {
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

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let result = manager
            .remove_image("remerge-test-nonexistent:latest")
            .await;
        assert!(result.is_err(), "removing nonexistent image should error");
    }

    /// `stop_container` on a nonexistent container returns an error.
    #[tokio::test]
    async fn stop_nonexistent_container_errors() {
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

        let manager = remerge_server::docker::DockerManager::new(&config)
            .await
            .expect("connect");

        let result = manager.stop_container("nonexistent-container-id").await;
        assert!(
            result.is_err(),
            "stopping nonexistent container should error"
        );
    }
}
