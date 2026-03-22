//! Phase 5 — Docker integration tests.
//!
//! These tests exercise Docker container operations.
//! Gated behind `#[cfg(feature = "integration")]`.

mod common;

#[cfg(feature = "integration")]
mod docker_tests {
    use super::common;

    /// Verify Docker availability check works.
    #[test]
    fn docker_availability_check() {
        let available = common::server::docker_available();
        if !available {
            eprintln!(
                "Docker is not available. Phase 5 tests will be skipped. \
                 Install and start Docker to run these tests."
            );
        }
        // This test always passes — it just reports Docker status.
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
}
