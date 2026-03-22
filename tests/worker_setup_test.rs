//! Phase 3 — Worker portage setup tests (filesystem).
//!
//! Tests the worker's portage configuration writing functions using
//! temp directories to avoid needing root access.

mod common;

use remerge_types::portage::*;
use remerge_worker::portage_setup;

// ── write_make_conf ─────────────────────────────────────────────────

/// Full golden-path test: write a make.conf with every field populated
/// and verify each expected line is present.
#[tokio::test]
async fn write_make_conf_golden_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("CHOST=\"x86_64-pc-linux-gnu\""),
        "must have CHOST"
    );
    assert!(content.contains("CFLAGS=\""), "must have CFLAGS");
    assert!(content.contains("CXXFLAGS=\""), "must have CXXFLAGS");
    assert!(content.contains("LDFLAGS=\""), "must have LDFLAGS");
    assert!(content.contains("MAKEOPTS=\""), "must have MAKEOPTS");
    assert!(content.contains("USE=\""), "must have USE");
    assert!(
        content.contains("ACCEPT_LICENSE=\""),
        "must have ACCEPT_LICENSE"
    );
    assert!(
        content.contains("ACCEPT_KEYWORDS=\""),
        "must have ACCEPT_KEYWORDS"
    );
    assert!(content.contains("FEATURES=\""), "must have FEATURES");
    assert!(
        content.contains("EMERGE_DEFAULT_OPTS=\""),
        "must have EMERGE_DEFAULT_OPTS"
    );
    assert!(content.contains("CPU_FLAGS_X86=\""), "must have CPU_FLAGS");
    assert!(
        content.contains("VIDEO_CARDS=\""),
        "must have USE_EXPAND VIDEO_CARDS"
    );
    assert!(
        content.contains("INPUT_DEVICES=\""),
        "must have USE_EXPAND INPUT_DEVICES"
    );
    assert!(content.contains("GENTOO_MIRRORS=\""), "must have extra var");
    assert!(
        content.contains("PKGDIR=\"/var/cache/binpkgs\""),
        "must have PKGDIR"
    );
}

/// use_flags_resolved = true → USE line must start with `-*`.
#[tokio::test]
async fn write_make_conf_use_flags_resolved() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.use_flags = vec!["X".into(), "wayland".into()];
    config.make_conf.use_flags_resolved = true;

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("USE=\"-* X wayland\""),
        "resolved USE flags must start with -*, got:\n{content}"
    );
}

/// use_flags_resolved = false → USE line must NOT have `-*` prefix.
#[tokio::test]
async fn write_make_conf_use_flags_unresolved() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.use_flags = vec!["X".into(), "wayland".into()];
    config.make_conf.use_flags_resolved = false;

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("USE=\"X wayland\""),
        "unresolved USE flags must not have -* prefix"
    );
    // Verify that the USE line itself doesn't have the -* prefix
    // (other lines like ACCEPT_LICENSE may legitimately contain -*)
    for line in content.lines() {
        if line.starts_with("USE=") {
            assert!(
                !line.contains("-*"),
                "USE line must not contain -* when unresolved, got: {line}"
            );
        }
    }
}

/// USE_EXPAND flags appear as separate variables, not inside USE.
#[tokio::test]
async fn write_make_conf_use_expand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .make_conf
        .use_expand
        .insert("VIDEO_CARDS".into(), vec!["intel".into()]);
    config
        .make_conf
        .use_expand
        .insert("INPUT_DEVICES".into(), vec!["libinput".into()]);

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("INPUT_DEVICES=\"libinput\""),
        "must have INPUT_DEVICES as separate line"
    );
    assert!(
        content.contains("VIDEO_CARDS=\"intel\""),
        "must have VIDEO_CARDS as separate line"
    );
}

/// Cross-compilation sets CBUILD when worker arch differs from target.
#[tokio::test]
async fn write_make_conf_cross_compile() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.chost = "aarch64-unknown-linux-gnu".into();

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("CHOST=\"aarch64-unknown-linux-gnu\""),
        "must have target CHOST"
    );
    assert!(
        content.contains("CBUILD=\"x86_64-pc-linux-gnu\""),
        "must have CBUILD for cross-compilation"
    );
}

/// GPG signing config adds signing-related FEATURES and variables.
#[tokio::test]
async fn write_make_conf_gpg_signing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::minimal_portage_config();
    portage_setup::write_make_conf_inner(
        &base,
        &config,
        "x86_64-pc-linux-gnu",
        Some("0xABCD1234"),
        Some("/var/gnupg"),
    )
    .await
    .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(
        content.contains("BINPKG_FORMAT=\"gpkg\""),
        "must have BINPKG_FORMAT"
    );
    assert!(
        content.contains("BINPKG_GPG_SIGNING_KEY=\"0xABCD1234\""),
        "must have signing key"
    );
    assert!(
        content.contains("BINPKG_GPG_SIGNING_GPG_HOME=\"/var/gnupg\""),
        "must have GPG home"
    );
    assert!(
        content.contains("binpkg-signing"),
        "FEATURES must include binpkg-signing"
    );
}

/// ccache and distcc FEATURES are stripped.
#[tokio::test]
async fn write_make_conf_strips_ccache_distcc() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.features = vec![
        "buildpkg".into(),
        "ccache".into(),
        "distcc".into(),
        "noclean".into(),
    ];

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(!content.contains("ccache"), "ccache should be stripped");
    assert!(!content.contains("distcc"), "distcc should be stripped");
    assert!(content.contains("buildpkg"), "buildpkg should remain");
    assert!(content.contains("noclean"), "noclean should remain");
}

/// Empty features list gets defaults (buildpkg + noclean).
#[tokio::test]
async fn write_make_conf_empty_features_defaults() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.make_conf.features = vec![];

    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(content.contains("buildpkg"), "must have buildpkg default");
    assert!(content.contains("noclean"), "must have noclean default");
}

// ── write_package_use ───────────────────────────────────────────────

/// Write package.use entries and verify content.
#[tokio::test]
async fn write_package_use_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_use_inner(&base, &config)
        .await
        .expect("write_package_use_inner");

    let content = std::fs::read_to_string(base.join("package.use/remerge")).unwrap();
    assert!(
        content.contains("dev-libs/openssl -bindist"),
        "must have openssl entry"
    );
    assert!(
        content.contains("sys-apps/systemd cryptsetup"),
        "must have systemd entry"
    );
}

/// Empty package_use is a no-op (no file created).
#[tokio::test]
async fn write_package_use_empty_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::minimal_portage_config();
    portage_setup::write_package_use_inner(&base, &config)
        .await
        .expect("empty should succeed");

    assert!(
        !base.join("package.use/remerge").exists(),
        "no file for empty config"
    );
}

// ── write_package_accept_keywords ───────────────────────────────────

/// Write package.accept_keywords entries and verify content.
#[tokio::test]
async fn write_package_accept_keywords_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_accept_keywords_inner(&base, &config)
        .await
        .expect("write_package_accept_keywords_inner");

    let content = std::fs::read_to_string(base.join("package.accept_keywords/remerge")).unwrap();
    assert!(
        content.contains("sys-kernel/gentoo-sources ~amd64"),
        "must have keywords entry"
    );
}

/// Empty package_accept_keywords is a no-op.
#[tokio::test]
async fn write_package_accept_keywords_empty_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::minimal_portage_config();
    portage_setup::write_package_accept_keywords_inner(&base, &config)
        .await
        .expect("empty should succeed");

    assert!(!base.join("package.accept_keywords/remerge").exists());
}

// ── write_package_license ───────────────────────────────────────────

/// Write package.license entries and verify content.
#[tokio::test]
async fn write_package_license_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_license_inner(&base, &config)
        .await
        .expect("write_package_license_inner");

    let content = std::fs::read_to_string(base.join("package.license/remerge")).unwrap();
    assert!(
        content.contains("sys-kernel/linux-firmware linux-fw-redistributable"),
        "must have license entry"
    );
}

// ── write_package_mask ──────────────────────────────────────────────

/// Write package.mask entries and verify content.
#[tokio::test]
async fn write_package_mask_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_mask_inner(&base, &config)
        .await
        .expect("write_package_mask_inner");

    let content = std::fs::read_to_string(base.join("package.mask/remerge")).unwrap();
    assert!(
        content.contains(">=dev-libs/foo-2.0"),
        "must have mask entry"
    );
}

// ── write_package_unmask ────────────────────────────────────────────

/// Write package.unmask entries and verify content.
#[tokio::test]
async fn write_package_unmask_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_unmask_inner(&base, &config)
        .await
        .expect("write_package_unmask_inner");

    let content = std::fs::read_to_string(base.join("package.unmask/remerge")).unwrap();
    assert!(
        content.contains("=dev-libs/bar-1.5"),
        "must have unmask entry"
    );
}

// ── write_package_env ───────────────────────────────────────────────

/// Write package.env entries and verify content.
#[tokio::test]
async fn write_package_env_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_package_env_inner(&base, &config)
        .await
        .expect("write_package_env_inner");

    let content = std::fs::read_to_string(base.join("package.env/remerge")).unwrap();
    assert!(
        content.contains("dev-qt/qtwebengine no-lto.conf"),
        "must have env entry"
    );
}

/// Invalid env_file entries are filtered out.
#[tokio::test]
async fn write_package_env_filters_invalid() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.package_env = vec![
        PackageEnvEntry {
            atom: "dev-libs/valid".into(),
            env_file: "valid.conf".into(),
        },
        PackageEnvEntry {
            atom: "dev-libs/bad-slash".into(),
            env_file: "../escape.conf".into(),
        },
        PackageEnvEntry {
            atom: "dev-libs/bad-empty".into(),
            env_file: "".into(),
        },
    ];

    portage_setup::write_package_env_inner(&base, &config)
        .await
        .expect("write_package_env_inner");

    let content = std::fs::read_to_string(base.join("package.env/remerge")).unwrap();
    assert!(
        content.contains("dev-libs/valid valid.conf"),
        "valid entry present"
    );
    assert!(!content.contains("escape"), "invalid entry filtered");
    assert!(!content.contains("bad-empty"), "empty env_file filtered");
}

// ── write_env_files ─────────────────────────────────────────────────

/// Write env files and verify content.
#[tokio::test]
async fn write_env_files_creates_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_env_files_inner(&base, &config)
        .await
        .expect("write_env_files_inner");

    let content = std::fs::read_to_string(base.join("env/no-lto.conf")).unwrap();
    assert!(content.contains("-fno-lto"), "must have env file content");
}

/// Invalid env filenames are skipped.
#[tokio::test]
async fn write_env_files_skips_invalid() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .env_files
        .insert("valid.conf".into(), "content".into());
    config
        .env_files
        .insert("../escape.conf".into(), "evil".into());
    config
        .env_files
        .insert("sub/dir.conf".into(), "evil".into());
    config.env_files.insert("".into(), "evil".into());

    portage_setup::write_env_files_inner(&base, &config)
        .await
        .expect("write_env_files_inner");

    assert!(
        base.join("env/valid.conf").exists(),
        "valid file should exist"
    );
    assert!(
        !base.join("env/../escape.conf").exists(),
        "traversal should be skipped"
    );
    assert!(
        !tmp.path().join("escape.conf").exists(),
        "traversal should not escape"
    );
}

// ── write_repos_conf ────────────────────────────────────────────────

/// Write repos.conf files and verify content.
#[tokio::test]
async fn write_repos_conf_creates_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_repos_conf_inner(&base, &config)
        .await
        .expect("write_repos_conf_inner");

    let content = std::fs::read_to_string(base.join("repos.conf/gentoo.conf")).unwrap();
    assert!(content.contains("[gentoo]"), "must have gentoo section");
    assert!(
        content.contains("location = /var/db/repos/gentoo"),
        "must have location"
    );
}

/// Empty repos_conf is a no-op (no directory created).
#[tokio::test]
async fn write_repos_conf_empty_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let config = common::fixtures::minimal_portage_config();
    portage_setup::write_repos_conf_inner(&base, &config)
        .await
        .expect("empty should succeed");

    assert!(!base.join("repos.conf").exists(), "no dir for empty config");
}

/// Invalid repos.conf filenames are skipped.
#[tokio::test]
async fn write_repos_conf_skips_invalid_filenames() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config
        .repos_conf
        .insert("valid.conf".into(), "[test]\nlocation = /tmp\n".into());
    config
        .repos_conf
        .insert("../escape.conf".into(), "[evil]\nlocation = /tmp\n".into());

    portage_setup::write_repos_conf_inner(&base, &config)
        .await
        .expect("write_repos_conf_inner");

    assert!(
        base.join("repos.conf/valid.conf").exists(),
        "valid file created"
    );
    assert!(
        !tmp.path().join("escape.conf").exists(),
        "traversal skipped"
    );
}

/// Multiple repos.conf files are all written.
#[tokio::test]
async fn write_repos_conf_multiple_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    let mut config = common::fixtures::minimal_portage_config();
    config.repos_conf.insert(
        "gentoo.conf".into(),
        "[gentoo]\nlocation = /var/db/repos/gentoo\n".into(),
    );
    config.repos_conf.insert(
        "custom.conf".into(),
        "[custom]\nlocation = /var/db/repos/custom\n".into(),
    );

    portage_setup::write_repos_conf_inner(&base, &config)
        .await
        .expect("write_repos_conf_inner");

    assert!(base.join("repos.conf/gentoo.conf").exists());
    assert!(base.join("repos.conf/custom.conf").exists());
}

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

// ── ensure_repo_locations_inner ─────────────────────────────────────

/// Repo found in bind-mount → symlink is created pointing to bind-mount path.
#[tokio::test]
async fn ensure_repo_locations_bind_mount_symlink() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let base = tmp.path();

    // Set up a repos_base with a "gentoo" repo directory (simulating bind-mount).
    let repos_base = base.join("repos");
    let gentoo_bind = repos_base.join("gentoo");
    std::fs::create_dir_all(&gentoo_bind).expect("create bind dir");

    // Target location for the repo.
    let target_location = base.join("target_repos/gentoo");

    // repos.conf content referencing the target location.
    let repos_conf_base = base.join("repos_conf");
    std::fs::create_dir_all(&repos_conf_base).expect("create repos_conf dir");
    let remap_base = base.join("remap");

    let mut repos_conf = std::collections::BTreeMap::new();
    repos_conf.insert(
        "gentoo.conf".to_string(),
        format!(
            "[gentoo]\nlocation = {}\nsync-type = rsync\n",
            target_location.display()
        ),
    );

    let config = PortageConfig {
        repos_conf,
        ..common::fixtures::minimal_portage_config()
    };

    portage_setup::ensure_repo_locations_inner(&config, &repos_base, &repos_conf_base, &remap_base)
        .await
        .expect("ensure_repo_locations_inner should succeed");

    // The target location should now be a symlink to the bind-mount path.
    assert!(
        target_location.is_symlink(),
        "target location should be a symlink"
    );
    let link_target = std::fs::read_link(&target_location).expect("read symlink");
    assert_eq!(
        link_target, gentoo_bind,
        "symlink should point to bind-mount path"
    );
}

/// Repo NOT in bind-mount + REMERGE_SKIP_SYNC=1 → remapped and location line rewritten.
#[tokio::test]
async fn ensure_repo_locations_remap_when_skip_sync() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let base = tmp.path();

    // repos_base with NO "custom" repo (not in bind-mount).
    let repos_base = base.join("repos");
    std::fs::create_dir_all(&repos_base).expect("create repos dir");

    // Target location under repos_base prefix (will trigger remap).
    let target_location = format!("{}/custom", repos_base.display());

    let repos_conf_base = base.join("repos_conf");
    std::fs::create_dir_all(&repos_conf_base).expect("create repos_conf dir");
    let remap_base = base.join("remap");

    // Write the repos.conf file that will be rewritten.
    let conf_content = format!("[custom]\nlocation = {target_location}\nsync-type = rsync\n");
    std::fs::write(repos_conf_base.join("custom.conf"), &conf_content)
        .expect("write initial repos.conf");

    let mut repos_conf = std::collections::BTreeMap::new();
    repos_conf.insert("custom.conf".to_string(), conf_content);

    let config = PortageConfig {
        repos_conf,
        ..common::fixtures::minimal_portage_config()
    };

    // Set REMERGE_SKIP_SYNC to trigger remapping.
    unsafe { std::env::set_var("REMERGE_SKIP_SYNC", "1") };

    let result = portage_setup::ensure_repo_locations_inner(
        &config,
        &repos_base,
        &repos_conf_base,
        &remap_base,
    )
    .await;

    unsafe { std::env::remove_var("REMERGE_SKIP_SYNC") };

    result.expect("ensure_repo_locations_inner should succeed");

    // The remap directory should have been created.
    assert!(
        remap_base.join("custom").is_dir(),
        "remapped directory should exist"
    );

    // The repos.conf file should have been rewritten.
    let rewritten = std::fs::read_to_string(repos_conf_base.join("custom.conf"))
        .expect("read rewritten conf");
    let expected_alt = format!("{}", remap_base.join("custom").display());
    assert!(
        rewritten.contains(&expected_alt),
        "repos.conf should contain remapped path, got: {rewritten}"
    );
}

/// Repo NOT in bind-mount, no skip-sync → empty directory created at location.
#[tokio::test]
async fn ensure_repo_locations_create_empty_dir() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let base = tmp.path();

    let repos_base = base.join("repos");
    std::fs::create_dir_all(&repos_base).expect("create repos dir");

    // Target location NOT under repos_base prefix.
    let target_location = base.join("external_repos/overlay");

    let repos_conf_base = base.join("repos_conf");
    std::fs::create_dir_all(&repos_conf_base).expect("create repos_conf dir");
    let remap_base = base.join("remap");

    let mut repos_conf = std::collections::BTreeMap::new();
    repos_conf.insert(
        "overlay.conf".to_string(),
        format!(
            "[overlay]\nlocation = {}\nsync-type = rsync\n",
            target_location.display()
        ),
    );

    let config = PortageConfig {
        repos_conf,
        ..common::fixtures::minimal_portage_config()
    };

    // Make sure REMERGE_SKIP_SYNC is not set.
    unsafe { std::env::remove_var("REMERGE_SKIP_SYNC") };

    portage_setup::ensure_repo_locations_inner(&config, &repos_base, &repos_conf_base, &remap_base)
        .await
        .expect("ensure_repo_locations_inner should succeed");

    // The target location should be an empty directory.
    assert!(
        target_location.is_dir(),
        "target location should be a directory"
    );
}

/// Invalid location (path traversal with `..`) → error returned.
#[tokio::test]
async fn ensure_repo_locations_rejects_traversal() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let base = tmp.path();

    let repos_base = base.join("repos");
    std::fs::create_dir_all(&repos_base).expect("create repos dir");
    let repos_conf_base = base.join("repos_conf");
    std::fs::create_dir_all(&repos_conf_base).expect("create repos_conf dir");
    let remap_base = base.join("remap");

    let mut repos_conf = std::collections::BTreeMap::new();
    repos_conf.insert(
        "evil.conf".to_string(),
        "[evil]\nlocation = /var/db/repos/../../../etc/shadow\nsync-type = rsync\n".to_string(),
    );

    let config = PortageConfig {
        repos_conf,
        ..common::fixtures::minimal_portage_config()
    };

    let result = portage_setup::ensure_repo_locations_inner(
        &config,
        &repos_base,
        &repos_conf_base,
        &remap_base,
    )
    .await;

    assert!(result.is_err(), "path traversal should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("invalid location") || err_msg.contains("traversal"),
        "error should mention invalid location: {err_msg}"
    );
}
