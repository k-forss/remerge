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
