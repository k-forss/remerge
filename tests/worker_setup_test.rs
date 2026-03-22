//! Phase 3 — Worker portage setup tests (filesystem).
//!
//! Tests the worker's portage configuration writing functions using
//! temp directories to avoid needing root access.

mod common;

use remerge_worker::portage_setup;

// ── write_profile_overlay ───────────────────────────────────────────

/// Write profile overlay files to a temp directory and verify content.
#[tokio::test]
async fn write_profile_overlay_creates_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .profile_overlay
        .insert("use.mask".into(), "custom-flag\n".into());
    config
        .profile_overlay
        .insert("package.provided".into(), "sys-libs/glibc-2.38\n".into());

    portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("write profile overlay");

    assert_eq!(
        std::fs::read_to_string(base.join("use.mask")).unwrap(),
        "custom-flag\n",
        "use.mask content mismatch"
    );
    assert_eq!(
        std::fs::read_to_string(base.join("package.provided")).unwrap(),
        "sys-libs/glibc-2.38\n",
        "package.provided content mismatch"
    );
}

/// Profile overlay with nested subdirectory.
#[tokio::test]
async fn write_profile_overlay_nested() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .profile_overlay
        .insert("subdir/nested.mask".into(), "nested content\n".into());

    portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("write profile overlay");

    assert_eq!(
        std::fs::read_to_string(base.join("subdir/nested.mask")).unwrap(),
        "nested content\n"
    );
}

/// Profile overlay path traversal is rejected (keys with `..`).
#[tokio::test]
async fn write_profile_overlay_rejects_path_traversal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .profile_overlay
        .insert("../etc/passwd".into(), "evil\n".into());
    config
        .profile_overlay
        .insert("/etc/passwd".into(), "evil\n".into());

    // These should be silently skipped (not written), not error.
    portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("should not error on path traversal — just skip");

    // Verify the traversal files were NOT written.
    assert!(
        !tmp.path().join("etc/passwd").exists(),
        "path traversal file should not be written"
    );
}

/// Empty profile overlay is a no-op.
#[tokio::test]
async fn write_profile_overlay_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("profile");
    std::fs::create_dir_all(&base).unwrap();

    let config = common::fixtures::minimal_portage_config();

    portage_setup::write_profile_overlay_inner(&base, &config)
        .await
        .expect("empty overlay should succeed");
}

// ── write_patches ───────────────────────────────────────────────────

/// Write patches to a temp directory and verify structure.
#[tokio::test]
async fn write_patches_creates_structure() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("patches");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config.patches.insert(
        "dev-libs/openssl/fix.patch".into(),
        "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n".into(),
    );

    portage_setup::write_patches_inner(&base, &config)
        .await
        .expect("write patches");

    let written = std::fs::read_to_string(base.join("dev-libs/openssl/fix.patch")).unwrap();
    assert!(written.contains("+new"), "patch content should be written");
}

/// Patches path traversal is rejected.
#[tokio::test]
async fn write_patches_rejects_path_traversal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("patches");
    std::fs::create_dir_all(&base).unwrap();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .patches
        .insert("../../etc/shadow".into(), "evil".into());
    config.patches.insert("/etc/shadow".into(), "evil".into());

    portage_setup::write_patches_inner(&base, &config)
        .await
        .expect("should not error on path traversal — just skip");

    assert!(!tmp.path().join("etc/shadow").exists());
}

// ── set_profile ─────────────────────────────────────────────────────

/// Set profile symlink to a valid profile directory.
#[tokio::test]
async fn set_profile_creates_symlink() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo_dir = tmp.path().join("repo");
    let profile_dir = repo_dir.join("profiles/default/linux/amd64/23.0");
    std::fs::create_dir_all(&profile_dir).unwrap();

    let link_path = tmp.path().join("make.profile");
    let repo_locations = vec![repo_dir.to_string_lossy().to_string()];

    portage_setup::set_profile_inner("default/linux/amd64/23.0", &repo_locations, &link_path)
        .await
        .expect("set profile");

    assert!(link_path.is_symlink(), "make.profile should be a symlink");
    let target = std::fs::read_link(&link_path).unwrap();
    assert!(
        target
            .to_string_lossy()
            .contains("profiles/default/linux/amd64/23.0"),
        "symlink should point to profile dir"
    );
}

/// Set profile with empty profile name is a no-op.
#[tokio::test]
async fn set_profile_empty_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let link_path = tmp.path().join("make.profile");

    portage_setup::set_profile_inner("", &[], &link_path)
        .await
        .expect("empty profile should succeed");

    assert!(
        !link_path.exists(),
        "no symlink should be created for empty profile"
    );
}

/// Set profile with "unknown" profile is a no-op.
#[tokio::test]
async fn set_profile_unknown_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let link_path = tmp.path().join("make.profile");

    portage_setup::set_profile_inner("unknown", &[], &link_path)
        .await
        .expect("unknown profile should succeed");

    assert!(
        !link_path.exists(),
        "no symlink should be created for unknown profile"
    );
}

// ── build_makeopts ──────────────────────────────────────────────────

/// No server override — client MAKEOPTS preserved.
#[test]
fn build_makeopts_no_override() {
    let result = portage_setup::build_makeopts_inner("-j4 -l4.0", None, None);
    assert_eq!(result, "-j4 -l4.0");
}

/// Server overrides replace client -j/-l flags.
#[test]
fn build_makeopts_server_override() {
    let result = portage_setup::build_makeopts_inner("-j4 -l4.0 --quiet", Some("16"), Some("12.0"));
    assert!(result.contains("-j16"), "should contain server's -j flag");
    assert!(result.contains("-l12.0"), "should contain server's -l flag");
    assert!(
        result.contains("--quiet"),
        "should preserve non -j/-l flags"
    );
    assert!(
        !result.contains("-j4"),
        "should not contain client's -j flag"
    );
}

/// Server provides only -j, client's -l is stripped.
#[test]
fn build_makeopts_partial_override() {
    let result = portage_setup::build_makeopts_inner("-j4 -l4.0", Some("8"), None);
    assert!(result.contains("-j8"), "should contain server's -j flag");
    assert!(
        !result.contains("-l"),
        "client -l should be stripped when server provides -j"
    );
}

// ── parse_repo_sections ─────────────────────────────────────────────

/// Parse a single-section repos.conf.
#[test]
fn parse_repo_sections_single() {
    let content = "[gentoo]\nlocation = /var/db/repos/gentoo\nsync-type = rsync\n";
    let repos = portage_setup::parse_repo_sections(content);
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].0, "gentoo");
    assert_eq!(repos[0].1, "/var/db/repos/gentoo");
}

/// Parse multi-section repos.conf.
#[test]
fn parse_repo_sections_multiple() {
    let content =
        "[gentoo]\nlocation = /var/db/repos/gentoo\n\n[custom]\nlocation = /var/db/repos/custom\n";
    let repos = portage_setup::parse_repo_sections(content);
    assert_eq!(repos.len(), 2);
}

/// DEFAULT section is skipped.
#[test]
fn parse_repo_sections_skips_default() {
    let content = "[DEFAULT]\nmain-repo = gentoo\n\n[gentoo]\nlocation = /var/db/repos/gentoo\n";
    let repos = portage_setup::parse_repo_sections(content);
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].0, "gentoo");
}
