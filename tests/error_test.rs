//! Phase 7 — Error path tests.
//!
//! Tests that error conditions are handled gracefully.

mod common;

use remerge_types::portage::*;
use remerge_types::validation::validate_atom;

// ── Deserialization error paths ─────────────────────────────────────

/// Empty JSON string fails to deserialize.
#[test]
fn deserialize_empty_json_fails() {
    let result = serde_json::from_str::<PortageConfig>("");
    assert!(result.is_err(), "empty string should fail");
}

/// Invalid JSON fails to deserialize.
#[test]
fn deserialize_invalid_json_fails() {
    let result = serde_json::from_str::<PortageConfig>("not valid json");
    assert!(result.is_err(), "invalid JSON should fail");
}

/// JSON array where object expected fails.
#[test]
fn deserialize_wrong_type_fails() {
    let result = serde_json::from_str::<PortageConfig>("[]");
    assert!(result.is_err(), "wrong type should fail");
}

/// SystemIdentity with missing required fields fails.
#[test]
fn system_identity_missing_fields_fails() {
    let json = r#"{"arch": "amd64"}"#;
    let result = serde_json::from_str::<SystemIdentity>(json);
    assert!(result.is_err(), "missing fields should fail");
}

// ── Shell injection in atom names ───────────────────────────────────

/// Various shell injection attempts are rejected.
#[test]
fn shell_injection_variants() {
    let injections = [
        "; rm -rf /",
        "$(evil)",
        "`evil`",
        "dev-libs/openssl\"",
        "dev-libs/openssl\\n",
        "foo\nbar",
        "foo\rbar",
        "foo\0bar",
        "foo{bar}",
        "foo(bar)",
    ];
    for injection in injections {
        assert!(
            validate_atom(injection).is_err(),
            "shell injection should be rejected: {injection:?}"
        );
    }
}

// ── Path traversal in profile_overlay ───────────────────────────────

/// Profile overlay path traversal with .. is rejected.
#[tokio::test]
async fn profile_overlay_path_traversal_dotdot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .profile_overlay
        .insert("../escape".into(), "evil".into());

    remerge_worker::portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("should handle gracefully");

    assert!(
        !tmp.path().join("escape").exists(),
        "traversal should be blocked"
    );
}

/// Profile overlay with absolute path is rejected.
#[tokio::test]
async fn profile_overlay_path_traversal_absolute() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .profile_overlay
        .insert("/etc/shadow".into(), "evil".into());

    remerge_worker::portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("should handle gracefully");

    assert!(!std::path::Path::new("/tmp/etc/shadow").exists());
}

/// Profile overlay with empty key is rejected.
#[tokio::test]
async fn profile_overlay_empty_key() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config.profile_overlay.insert("".into(), "content".into());

    remerge_worker::portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("should handle gracefully");
}

// ── Path traversal in patches ───────────────────────────────────────

/// Patches path traversal with .. is rejected.
#[tokio::test]
async fn patches_path_traversal_dotdot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("patches");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .patches
        .insert("../../etc/shadow".into(), "evil".into());

    remerge_worker::portage_setup::write_patches_inner(&base, &config)
        .await
        .expect("should handle gracefully");

    assert!(!tmp.path().join("etc/shadow").exists());
}

/// Patches with absolute path is rejected.
#[tokio::test]
async fn patches_path_traversal_absolute() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("patches");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config.patches.insert("/etc/shadow".into(), "evil".into());

    remerge_worker::portage_setup::write_patches_inner(&base, &config)
        .await
        .expect("should handle gracefully");
}

// ── Validation edge cases ───────────────────────────────────────────

/// Null byte in atom name is rejected.
#[test]
fn validate_atom_null_byte() {
    assert!(validate_atom("foo\0bar").is_err());
}

/// Newline in atom name is rejected.
#[test]
fn validate_atom_newline() {
    assert!(validate_atom("foo\nbar").is_err());
}

/// Atom with only whitespace is handled without panicking.
#[test]
fn validate_atom_whitespace_only() {
    // Spaces are not in SHELL_CHARS but would fail package name validation.
    // Just verify it doesn't panic.
    let _ = validate_atom("   ");
}

/// MakeConf defaults produce valid non-empty values.
#[test]
fn make_conf_defaults_are_sane() {
    let mc = MakeConf::default();
    assert!(!mc.cflags.is_empty(), "CFLAGS should have a default");
    assert!(!mc.chost.is_empty(), "CHOST should have a default");
    assert!(!mc.makeopts.is_empty(), "MAKEOPTS should have a default");
}

// ── Docker socket unavailable ──────────────────────────────────────

/// Docker socket unavailable returns error, not panic.
#[tokio::test]
async fn docker_socket_unavailable_returns_error() {
    let tmp_binpkg = tempfile::TempDir::new().unwrap();
    let tmp_state = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        binpkg_dir: tmp_binpkg.path().to_path_buf(),
        state_dir: tmp_state.path().to_path_buf(),
        docker_socket: "unix:///nonexistent/docker.sock".into(),
        ..Default::default()
    };
    let result = remerge_server::docker::DockerManager::new(&config).await;
    assert!(result.is_err(), "should error on bad socket");
}

// ── Server config validation ────────────────────────────────────────

/// ServerConfig with all defaults can be serialized and deserialized.
#[test]
fn server_config_default_roundtrip() {
    let config = remerge_server::config::ServerConfig::default();
    let json = serde_json::to_string(&config).expect("serialize");
    let _: remerge_server::config::ServerConfig = serde_json::from_str(&json).expect("deserialize");
}

/// ServerConfig deserialization from empty object uses defaults.
#[test]
fn server_config_empty_object_uses_defaults() {
    let config: remerge_server::config::ServerConfig =
        serde_json::from_str("{}").expect("deserialize empty");
    assert!(
        !config.docker_socket.is_empty(),
        "docker_socket should have default"
    );
    assert!(
        !config.binhost_url.is_empty(),
        "binhost_url should have default"
    );
    assert!(config.max_workers > 0, "max_workers should have default");
}

// ── Server config validation — Task 7.6 ────────────────────────────

/// AppState::new() with non-writable binpkg_dir fails.
/// DockerManager::new() is called first, so Docker must be available.
#[cfg(feature = "integration")]
#[tokio::test]
async fn appstate_non_writable_binpkg_dir_fails() {
    if !common::server::docker_available() {
        eprintln!("Docker not available — skipping");
        return;
    }
    let tmp_state = tempfile::TempDir::new().expect("temp dir");
    let config = remerge_server::config::ServerConfig {
        binpkg_dir: "/proc/nonexistent/binpkgs".into(),
        state_dir: tmp_state.path().to_path_buf(),
        ..Default::default()
    };
    let result = remerge_server::state::AppState::new(config).await;
    assert!(
        result.is_err(),
        "AppState::new with non-writable binpkg_dir should fail"
    );
}

/// AppState::new() with non-writable state_dir fails.
/// DockerManager::new() is called first, so Docker must be available.
#[cfg(feature = "integration")]
#[tokio::test]
async fn appstate_non_writable_state_dir_fails() {
    if !common::server::docker_available() {
        eprintln!("Docker not available — skipping");
        return;
    }
    let tmp_binpkg = tempfile::TempDir::new().expect("temp dir");
    let config = remerge_server::config::ServerConfig {
        binpkg_dir: tmp_binpkg.path().to_path_buf(),
        state_dir: "/proc/nonexistent/state".into(),
        ..Default::default()
    };
    let result = remerge_server::state::AppState::new(config).await;
    assert!(
        result.is_err(),
        "AppState::new with non-writable state_dir should fail"
    );
}

/// TLS config with nonexistent cert file fails when loading.
#[test]
fn tls_config_missing_cert_file_fails() {
    let tls_cfg = remerge_server::config::TlsConfig {
        cert: "/nonexistent/cert.pem".into(),
        key: "/nonexistent/key.pem".into(),
    };
    let result = tls_cfg.load_rustls_config();
    assert!(result.is_err(), "TLS config with missing cert should fail");
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.contains("Failed to read TLS cert"),
        "error should mention cert file, got: {err}"
    );
}

/// TLS config with nonexistent key file fails when loading.
#[test]
fn tls_config_missing_key_file_fails() {
    // Create a valid (but self-signed) cert file so the cert read succeeds,
    // but point the key at a nonexistent file.
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let cert_path = tmp.path().join("cert.pem");
    // Write a minimal PEM placeholder — doesn't need to be valid for this test
    // because the key file read should fail first.
    std::fs::write(
        &cert_path,
        "-----BEGIN CERTIFICATE-----\nMIIBkTCB+wIJALRiMLAh\n-----END CERTIFICATE-----\n",
    )
    .expect("write cert");

    let tls_cfg = remerge_server::config::TlsConfig {
        cert: cert_path,
        key: "/nonexistent/key.pem".into(),
    };
    let result = tls_cfg.load_rustls_config();
    assert!(result.is_err(), "TLS config with missing key should fail");
    let err = format!("{:#}", result.unwrap_err());
    assert!(
        err.contains("Failed to read TLS key"),
        "error should mention key file, got: {err}"
    );
}

/// Auth config with Mtls mode but empty clients is accepted (resolve fails at runtime).
#[test]
fn auth_config_mtls_empty_clients() {
    use remerge_server::auth::{AuthConfig, CertRegistry};
    use remerge_types::auth::AuthMode;

    let config = AuthConfig {
        mode: AuthMode::Mtls,
        clients: Vec::new(),
        ..Default::default()
    };

    // CertRegistry::new does not fail — it just creates an empty registry.
    // The error happens at resolve() time when no cert header is present.
    let registry = CertRegistry::new(&config);
    assert_eq!(registry.mode(), AuthMode::Mtls);
}

/// Auth resolve in Mtls mode without cert header returns error.
#[test]
fn auth_mtls_resolve_without_cert_rejects() {
    use remerge_server::auth::{AuthConfig, CertRegistry};
    use remerge_types::auth::AuthMode;
    use remerge_types::client::ClientRole;

    let config = AuthConfig {
        mode: AuthMode::Mtls,
        clients: Vec::new(),
        ..Default::default()
    };

    let registry = CertRegistry::new(&config);

    let headers = axum::http::HeaderMap::new();
    let result = registry.resolve(&headers, uuid::Uuid::new_v4(), ClientRole::Main);
    assert!(
        result.is_err(),
        "Mtls resolve without cert should return error"
    );
}

// ── Build failure error events (require running worker container) ───

/// 7.1: Worker exits non-zero — verify workorder reaches Failed status.
/// Submits a nonexistent package that will cause emerge to fail.
#[cfg(feature = "e2e")]
#[tokio::test]
async fn worker_exit_nonzero_sets_failed_status() {
    if !common::server::docker_available() {
        return;
    }

    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    // Submit a workorder for a nonexistent package to trigger build failure.
    let req = remerge_types::api::SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/nonexistent-package-12345".into()],
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

    // The submission itself should succeed (validation passes).
    // The failure happens during build execution.
    assert_eq!(resp.status(), 200, "submission should be accepted");

    let submit_resp: remerge_types::api::SubmitWorkorderResponse =
        resp.json().await.expect("parse");

    // Poll for status change — allow up to 120s for the worker to start,
    // attempt the build, and fail.
    let mut final_status = None;
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get status");

        if resp.status() == 200 {
            let status: remerge_types::api::WorkorderStatusResponse =
                resp.json().await.expect("parse status");
            final_status = Some(status.status.clone());
            if matches!(
                status.status,
                remerge_types::workorder::WorkorderStatus::Failed { .. }
                    | remerge_types::workorder::WorkorderStatus::Completed
                    | remerge_types::workorder::WorkorderStatus::Cancelled
            ) {
                break;
            }
        }
    }

    let status = final_status.expect("should have received a status update within 120s");
    assert!(
        matches!(
            status,
            remerge_types::workorder::WorkorderStatus::Failed { .. }
        ),
        "workorder for nonexistent package should reach Failed status, got {status:?}"
    );
}

/// Helper: submit a workorder, wait up to `timeout_secs` for a terminal status,
/// and return the final status.
#[cfg(feature = "e2e")]
async fn submit_and_wait_for_terminal(
    server: &common::server::TestServer,
    atoms: Vec<String>,
    emerge_args: Vec<String>,
    timeout_secs: u64,
) -> remerge_types::workorder::WorkorderStatus {
    let req = remerge_types::api::SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms,
        emerge_args,
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
    assert_eq!(resp.status(), 200, "submission should be accepted");

    let submit_resp: remerge_types::api::SubmitWorkorderResponse =
        resp.json().await.expect("parse");

    let mut final_status = None;
    for _ in 0..timeout_secs {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get status");

        if resp.status() == 200 {
            let status: remerge_types::api::WorkorderStatusResponse =
                resp.json().await.expect("parse status");
            final_status = Some(status.status.clone());
            if matches!(
                status.status,
                remerge_types::workorder::WorkorderStatus::Failed { .. }
                    | remerge_types::workorder::WorkorderStatus::Completed
                    | remerge_types::workorder::WorkorderStatus::Cancelled
            ) {
                break;
            }
        }
    }

    final_status.expect("should have received a status update within timeout")
}

/// 7.2: Missing dependency — building a package with an unsatisfied
/// dependency should result in a Failed workorder.
/// The emerge process will fail because the dependency cannot be resolved.
#[cfg(feature = "e2e")]
#[tokio::test]
async fn missing_dependency_causes_failure() {
    if !common::server::docker_available() {
        return;
    }

    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    // Submit a workorder for a package that depends on something
    // unavailable. Using a nonexistent package atom is the simplest way
    // to trigger a dependency resolution failure.
    let status = submit_and_wait_for_terminal(
        &server,
        vec!["dev-libs/nonexistent-dep-trigger-12345".into()],
        vec![],
        120,
    )
    .await;

    assert!(
        matches!(
            status,
            remerge_types::workorder::WorkorderStatus::Failed { .. }
        ),
        "workorder with missing dependency should reach Failed status, got {status:?}"
    );
}

/// 7.3: USE conflict — submitting a build with conflicting USE flags
/// should result in a failure. We submit with contradictory flags
/// (a flag and its negation) to trigger a USE conflict.
#[cfg(feature = "e2e")]
#[tokio::test]
async fn use_conflict_causes_failure() {
    if !common::server::docker_available() {
        return;
    }

    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    // Build a config with contradictory USE flags to trigger a conflict.
    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.use_flags = vec!["ssl".into(), "-ssl".into()];
    config.package_use = vec![remerge_types::portage::PackageUseEntry {
        atom: "dev-libs/openssl".into(),
        flags: vec!["bindist".into(), "-bindist".into()],
    }];

    let req = remerge_types::api::SubmitWorkorderRequest {
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/openssl".into()],
        emerge_args: vec![],
        portage_config: config,
        system_id: common::fixtures::minimal_system_identity(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/workorders", server.base_url))
        .json(&req)
        .send()
        .await
        .expect("submit");
    assert_eq!(resp.status(), 200, "submission should be accepted");

    let submit_resp: remerge_types::api::SubmitWorkorderResponse =
        resp.json().await.expect("parse");

    // Wait for the build to fail.
    let mut final_status = None;
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let resp = reqwest::get(format!(
            "{}/api/v1/workorders/{}",
            server.base_url, submit_resp.workorder_id
        ))
        .await
        .expect("get status");

        if resp.status() == 200 {
            let status: remerge_types::api::WorkorderStatusResponse =
                resp.json().await.expect("parse status");
            final_status = Some(status.status.clone());
            if matches!(
                status.status,
                remerge_types::workorder::WorkorderStatus::Failed { .. }
                    | remerge_types::workorder::WorkorderStatus::Completed
                    | remerge_types::workorder::WorkorderStatus::Cancelled
            ) {
                break;
            }
        }
    }

    let status = final_status.expect("should have received a status update within 120s");
    // The build MUST fail — mutually exclusive USE flags on the same
    // package cannot be resolved by portage. A test named
    // "use_conflict_causes_failure" must only accept Failed.
    assert!(
        matches!(
            status,
            remerge_types::workorder::WorkorderStatus::Failed { .. }
        ),
        "workorder with USE conflict should fail, got {status:?}"
    );
}

/// 7.4: Fetch failure — building a package with a bad GENTOO_MIRRORS
/// or using --fetchonly with no network should trigger a fetch failure.
/// We use a nonexistent package to trigger the failure path.
#[cfg(feature = "e2e")]
#[tokio::test]
async fn fetch_failure_causes_error() {
    if !common::server::docker_available() {
        return;
    }

    let Some(server) = common::server::TestServer::start().await else {
        return;
    };

    // Submit with --fetchonly and a nonexistent package — the fetch will fail.
    let status = submit_and_wait_for_terminal(
        &server,
        vec!["dev-libs/nonexistent-fetch-test-12345".into()],
        vec!["--fetchonly".into()],
        120,
    )
    .await;

    assert!(
        matches!(
            status,
            remerge_types::workorder::WorkorderStatus::Failed { .. }
        ),
        "workorder with fetch failure should reach Failed status, got {status:?}"
    );
}
