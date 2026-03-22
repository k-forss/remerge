//! Phase 2 — CLI Portage Reader tests (filesystem).
//!
//! Tests version comparison functions, atom parsing, and VDB lookups
//! using synthetic portage trees in temp directories.

mod common;

use remerge::portage;

// ── split_name_version ──────────────────────────────────────────────

/// Split a package name with version.
#[test]
fn split_name_version_basic() {
    let (name, version) = portage::split_name_version("openssl-3.1.4");
    assert_eq!(name, "openssl");
    assert_eq!(version, Some("3.1.4"));
}

/// Split a package name with revision.
#[test]
fn split_name_version_with_revision() {
    let (name, version) = portage::split_name_version("openssl-3.1.4-r1");
    assert_eq!(name, "openssl");
    assert_eq!(version, Some("3.1.4-r1"));
}

/// Package name without version.
#[test]
fn split_name_version_no_version() {
    let (name, version) = portage::split_name_version("openssl");
    assert_eq!(name, "openssl");
    assert_eq!(version, None);
}

/// Package name with leading digit in name.
#[test]
fn split_name_version_numeric_name() {
    let (name, version) = portage::split_name_version("lib3ds-1.2");
    assert_eq!(name, "lib3ds");
    assert_eq!(version, Some("1.2"));
}

/// Package with plus sign in name.
#[test]
fn split_name_version_plus_sign() {
    let (name, version) = portage::split_name_version("gtk+-2.24.33");
    assert_eq!(name, "gtk+");
    assert_eq!(version, Some("2.24.33"));
}

// ── split_revision ──────────────────────────────────────────────────

/// Split revision from version.
#[test]
fn split_revision_with_revision() {
    let (base, rev) = portage::split_revision("3.1.4-r1");
    assert_eq!(base, "3.1.4");
    assert_eq!(rev, Some("r1"));
}

/// No revision.
#[test]
fn split_revision_no_revision() {
    let (base, rev) = portage::split_revision("3.1.4");
    assert_eq!(base, "3.1.4");
    assert_eq!(rev, None);
}

/// Revision zero.
#[test]
fn split_revision_zero() {
    let (base, rev) = portage::split_revision("1.0-r0");
    assert_eq!(base, "1.0");
    assert_eq!(rev, Some("r0"));
}

// ── compare_versions ────────────────────────────────────────────────

/// Basic version comparison.
#[test]
fn compare_versions_basic() {
    use std::cmp::Ordering;
    assert_eq!(portage::compare_versions("1.0", "1.0"), Ordering::Equal);
    assert_eq!(portage::compare_versions("1.0", "2.0"), Ordering::Less);
    assert_eq!(portage::compare_versions("2.0", "1.0"), Ordering::Greater);
}

/// Numeric vs lexicographic: 1.9 < 1.10.
#[test]
fn compare_versions_numeric() {
    use std::cmp::Ordering;
    assert_eq!(portage::compare_versions("1.9", "1.10"), Ordering::Less);
    assert_eq!(portage::compare_versions("1.10", "1.9"), Ordering::Greater);
}

/// Trailing letter comparison: 1.1.1a < 1.1.1z.
#[test]
fn compare_versions_trailing_letter() {
    use std::cmp::Ordering;
    assert_eq!(
        portage::compare_versions("1.1.1a", "1.1.1z"),
        Ordering::Less
    );
    assert_eq!(
        portage::compare_versions("1.1.1z", "1.1.1a"),
        Ordering::Greater
    );
}

/// PMS suffix ordering: _alpha < _beta < _pre < _rc < (none) < _p.
#[test]
fn compare_versions_suffixes() {
    use std::cmp::Ordering;
    assert_eq!(
        portage::compare_versions("1.0_alpha", "1.0_beta"),
        Ordering::Less
    );
    assert_eq!(
        portage::compare_versions("1.0_beta", "1.0_pre"),
        Ordering::Less
    );
    assert_eq!(
        portage::compare_versions("1.0_pre", "1.0_rc"),
        Ordering::Less
    );
    assert_eq!(portage::compare_versions("1.0_rc", "1.0"), Ordering::Less);
    assert_eq!(portage::compare_versions("1.0", "1.0_p"), Ordering::Less);
    assert_eq!(portage::compare_versions("1.0_p", "1.0_p1"), Ordering::Less);
}

/// Revision comparison.
#[test]
fn compare_versions_revisions() {
    use std::cmp::Ordering;
    assert_eq!(portage::compare_versions("1.0", "1.0-r1"), Ordering::Less);
    assert_eq!(
        portage::compare_versions("1.0-r0", "1.0-r1"),
        Ordering::Less
    );
    assert_eq!(
        portage::compare_versions("1.0-r1", "1.0-r2"),
        Ordering::Less
    );
    assert_eq!(
        portage::compare_versions("1.0-r1", "1.0-r1"),
        Ordering::Equal
    );
}

/// Different depth versions.
#[test]
fn compare_versions_different_depth() {
    use std::cmp::Ordering;
    assert_eq!(portage::compare_versions("1.0", "1.0.1"), Ordering::Less);
    assert_eq!(portage::compare_versions("1.0.0", "1.0"), Ordering::Greater);
}

// ── parse_atom_operator ─────────────────────────────────────────────

/// Parse various atom operators.
#[test]
fn parse_atom_operators() {
    use remerge::portage::AtomOp;
    let (op, rest) = portage::parse_atom_operator(">=dev-libs/openssl-3.0");
    assert_eq!(op, AtomOp::Ge);
    assert_eq!(rest, "dev-libs/openssl-3.0");

    let (op, rest) = portage::parse_atom_operator("=dev-libs/openssl-3.1*");
    assert_eq!(op, AtomOp::EqGlob);
    assert_eq!(rest, "dev-libs/openssl-3.1*");

    let (op, rest) = portage::parse_atom_operator("~dev-libs/openssl-3.1.0");
    assert_eq!(op, AtomOp::Tilde);
    assert_eq!(rest, "dev-libs/openssl-3.1.0");

    let (op, rest) = portage::parse_atom_operator("dev-libs/openssl");
    assert_eq!(op, AtomOp::None);
    assert_eq!(rest, "dev-libs/openssl");

    let (op, _) = portage::parse_atom_operator("<=dev-libs/openssl-3.0");
    assert_eq!(op, AtomOp::Le);

    let (op, _) = portage::parse_atom_operator(">dev-libs/openssl-3.0");
    assert_eq!(op, AtomOp::Gt);

    let (op, _) = portage::parse_atom_operator("<dev-libs/openssl-3.0");
    assert_eq!(op, AtomOp::Lt);

    let (op, _) = portage::parse_atom_operator("=dev-libs/openssl-3.1.0");
    assert_eq!(op, AtomOp::Eq);
}

// ── is_installed (VDB lookup) ───────────────────────────────────────

/// is_installed: any version matches with bare atom.
#[test]
fn is_installed_any_version() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[
        ("dev-libs", "openssl-3.1.0-r2"),
        ("dev-libs", "openssl-1.1.1w"),
    ]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed("dev-libs/openssl"),
        "any version should match"
    );
}

/// is_installed: exact version match.
#[test]
fn is_installed_exact_match() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[
        ("dev-libs", "openssl-3.1.0-r2"),
        ("dev-libs", "openssl-1.1.1w"),
    ]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed("=dev-libs/openssl-3.1.0-r2"),
        "exact match should succeed"
    );
    assert!(
        !reader.is_installed("=dev-libs/openssl-3.1.0"),
        "different revision should fail"
    );
}

/// is_installed: >= operator.
#[test]
fn is_installed_ge() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[
        ("dev-libs", "openssl-3.1.0-r2"),
        ("dev-libs", "openssl-1.1.1w"),
    ]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed(">=dev-libs/openssl-3.0"),
        "3.1.0 >= 3.0 should satisfy"
    );
    assert!(
        !reader.is_installed(">dev-libs/openssl-4.0"),
        "nothing > 4.0"
    );
}

/// is_installed: < and <= operators.
#[test]
fn is_installed_lt_le() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[
        ("dev-libs", "openssl-3.1.0-r2"),
        ("dev-libs", "openssl-1.1.1w"),
    ]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed("<dev-libs/openssl-2.0"),
        "1.1.1w < 2.0 should satisfy"
    );
    assert!(
        reader.is_installed("<=dev-libs/openssl-3.1.0-r2"),
        "exact match satisfies <="
    );
}

/// is_installed: ~ (tilde) operator — any revision of same version.
#[test]
fn is_installed_tilde() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0-r2")]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed("~dev-libs/openssl-3.1.0"),
        "any revision of 3.1.0 should match"
    );
}

/// is_installed: glob operator.
#[test]
fn is_installed_glob() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0-r2")]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        reader.is_installed("=dev-libs/openssl-3.1*"),
        "3.1.0 matches 3.1*"
    );
    assert!(
        !reader.is_installed("=dev-libs/openssl-3.2*"),
        "3.1.0 does not match 3.2*"
    );
}

/// is_installed: sets always return false.
#[test]
fn is_installed_set() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0")]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(!reader.is_installed("@world"), "sets are never installed");
}

/// is_installed: nonexistent package.
#[test]
fn is_installed_nonexistent() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0")]);
    let _env = common::set_root_env(&root);
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(
        !reader.is_installed("dev-libs/nonexistent"),
        "nonexistent package should return false"
    );
}

/// Fixture portage tree creates expected structure.
#[test]
fn fixture_portage_tree_structure() {
    let (_tmp, root) = common::fixtures::portage_tree();
    assert!(root.join("etc/portage/make.conf").exists());
    assert!(root.join("etc/portage/package.use/custom").exists());
    assert!(root.join("etc/portage/repos.conf/gentoo.conf").exists());
    assert!(root.join("etc/portage/profile/use.mask").exists());
    assert!(
        root.join("etc/portage/patches/dev-libs/openssl/fix.patch")
            .exists()
    );
    assert!(root.join("var/lib/portage/world").exists());
}

/// Fixture VDB tree creates package directories.
#[test]
fn fixture_vdb_tree() {
    let (_tmp, root) =
        common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0"), ("sys-apps", "systemd-254")]);
    assert!(root.join("var/db/pkg/dev-libs/openssl-3.1.0").is_dir());
    assert!(root.join("var/db/pkg/sys-apps/systemd-254").is_dir());
}

// ── expand_set ──────────────────────────────────────────────────────

/// @world expansion reads the world file — no portageq needed.
#[test]
fn expand_set_world() {
    let (_tmp, root) = common::fixtures::portage_tree();
    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let atoms = reader.expand_set("@world");
    assert!(!atoms.is_empty(), "world set should expand");
    // The portage_tree fixture writes dev-libs/openssl, sys-apps/systemd, app-misc/screen
    assert!(
        atoms.contains(&"dev-libs/openssl".to_string()),
        "world should contain dev-libs/openssl, got: {atoms:?}"
    );
    assert!(
        atoms.contains(&"sys-apps/systemd".to_string()),
        "world should contain sys-apps/systemd"
    );
}

/// Unknown set names are passed through unchanged.
#[test]
fn expand_set_unknown_passthrough() {
    let (_tmp, root) = common::fixtures::portage_tree();
    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let atoms = reader.expand_set("@custom-set");
    assert_eq!(
        atoms,
        vec!["@custom-set"],
        "unknown set should pass through"
    );
}

// ── PortageReader round-trip tests (Tasks 2.1-2.6) ──────────────────

/// 2.1: read_config golden path — populate a full temp portage tree,
/// read it back, verify all fields are populated.
#[test]
fn read_config_golden_path() {
    let (_tmp, root) = common::fixtures::portage_tree();
    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader.read_config().expect("read_config should succeed");

    // Verify package.use was read.
    assert!(
        !config.package_use.is_empty(),
        "package_use should not be empty"
    );
    assert!(
        config
            .package_use
            .iter()
            .any(|e| e.atom == "dev-libs/openssl"),
        "package_use should contain openssl entry"
    );

    // Verify package.accept_keywords was read.
    assert!(
        !config.package_accept_keywords.is_empty(),
        "package_accept_keywords should not be empty"
    );

    // Verify package.license was read.
    assert!(
        !config.package_license.is_empty(),
        "package_license should not be empty"
    );

    // Verify package.mask was read.
    assert!(
        !config.package_mask.is_empty(),
        "package_mask should not be empty"
    );

    // Verify package.unmask was read.
    assert!(
        !config.package_unmask.is_empty(),
        "package_unmask should not be empty"
    );

    // Verify package.env was read.
    assert!(
        !config.package_env.is_empty(),
        "package_env should not be empty"
    );

    // Verify env files were read.
    assert!(
        !config.env_files.is_empty(),
        "env_files should not be empty"
    );
    assert!(
        config.env_files.contains_key("no-lto.conf"),
        "env_files should contain no-lto.conf"
    );

    // Verify repos.conf was read.
    assert!(
        !config.repos_conf.is_empty(),
        "repos_conf should not be empty"
    );

    // Verify profile overlay was read.
    assert!(
        !config.profile_overlay.is_empty(),
        "profile_overlay should not be empty"
    );
    assert!(
        config.profile_overlay.contains_key("use.mask"),
        "profile_overlay should contain use.mask"
    );

    // Verify patches were read.
    assert!(!config.patches.is_empty(), "patches should not be empty");

    // Verify world was read.
    assert!(!config.world.is_empty(), "world should not be empty");

    // Verify make.conf fields.
    assert!(!config.make_conf.cflags.is_empty(), "CFLAGS should be read");
    assert!(
        !config.make_conf.chost.is_empty(),
        "CHOST should be detected or read"
    );
}

/// 2.2: read_config with missing optional dirs (no package.use/, etc.).
#[test]
fn read_config_missing_optional_dirs() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let root = tmp.path().to_path_buf();
    let portage = root.join("etc/portage");
    std::fs::create_dir_all(&portage).expect("create portage dir");

    // Write only make.conf — all optional dirs are missing.
    std::fs::write(
        portage.join("make.conf"),
        "CFLAGS=\"-O2\"\nCHOST=\"x86_64-pc-linux-gnu\"\n",
    )
    .expect("write make.conf");

    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader.read_config().expect("should handle missing dirs");

    assert!(
        config.package_use.is_empty(),
        "package_use should be empty with no dir"
    );
    assert!(
        config.repos_conf.is_empty(),
        "repos_conf should be empty with no dir"
    );
    assert!(
        config.patches.is_empty(),
        "patches should be empty with no dir"
    );
    assert!(
        config.profile_overlay.is_empty(),
        "profile_overlay should be empty with no dir"
    );
}

/// 2.3: read_config with package.use as a single file vs. a directory.
#[test]
fn read_config_package_use_single_file() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let root = tmp.path().to_path_buf();
    let portage = root.join("etc/portage");
    std::fs::create_dir_all(&portage).expect("create portage dir");

    std::fs::write(
        portage.join("make.conf"),
        "CFLAGS=\"-O2\"\nCHOST=\"x86_64-pc-linux-gnu\"\n",
    )
    .expect("write make.conf");

    // package.use as a single file (not a directory).
    std::fs::write(
        portage.join("package.use"),
        "dev-libs/openssl -bindist\nsys-apps/systemd cryptsetup\n",
    )
    .expect("write package.use");

    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader
        .read_config()
        .expect("should handle file-mode package.use");

    assert!(
        !config.package_use.is_empty(),
        "package_use should be read from single file"
    );
    assert!(
        config
            .package_use
            .iter()
            .any(|e| e.atom == "dev-libs/openssl"),
        "should contain openssl entry"
    );
}

/// 2.4: read_profile_overlay round-trip — write files, read back, compare.
#[test]
fn read_profile_overlay_round_trip() {
    let (_tmp, root) = common::fixtures::portage_tree();
    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader.read_config().expect("read config");

    assert!(
        config.profile_overlay.contains_key("use.mask"),
        "should contain use.mask"
    );
    let content = config.profile_overlay.get("use.mask").unwrap();
    assert!(
        content.contains("custom-flag"),
        "use.mask should contain custom-flag"
    );
}

/// 2.5: read_patches_recursive with nested category/package/*.patch structure.
#[test]
fn read_patches_recursive_nested() {
    let (_tmp, root) = common::fixtures::portage_tree();
    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader.read_config().expect("read config");

    assert!(!config.patches.is_empty(), "patches should contain entries");
    // The fixture creates patches/dev-libs/openssl/fix.patch.
    assert!(
        config.patches.keys().any(|k| k.contains("openssl")),
        "patches should contain openssl patch, got keys: {:?}",
        config.patches.keys().collect::<Vec<_>>()
    );
}

/// 2.6: read_repos_conf with multiple section blocks.
#[test]
fn read_repos_conf_multiple_sections() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let root = tmp.path().to_path_buf();
    let portage = root.join("etc/portage");
    std::fs::create_dir_all(portage.join("repos.conf")).expect("create repos.conf dir");
    std::fs::create_dir_all(&portage).expect("create portage dir");

    std::fs::write(
        portage.join("make.conf"),
        "CFLAGS=\"-O2\"\nCHOST=\"x86_64-pc-linux-gnu\"\n",
    )
    .expect("write make.conf");

    // Write repos.conf with multiple files.
    std::fs::write(
        portage.join("repos.conf/gentoo.conf"),
        "[gentoo]\nlocation = /var/db/repos/gentoo\nsync-type = rsync\n",
    )
    .expect("write gentoo.conf");

    std::fs::write(
        portage.join("repos.conf/overlay.conf"),
        "[guru]\nlocation = /var/db/repos/guru\nsync-type = git\nsync-uri = https://github.com/gentoo/guru.git\n",
    )
    .expect("write overlay.conf");

    let _env = common::set_root_env(&root);
    let reader = portage::PortageReader::new().unwrap();
    let config = reader.read_config().expect("read config");

    assert!(
        config.repos_conf.len() >= 2,
        "repos_conf should have at least 2 entries, got {}",
        config.repos_conf.len()
    );

    // Check that sections are parsed correctly.
    let all_content: String = config.repos_conf.values().cloned().collect();
    assert!(
        all_content.contains("[gentoo]"),
        "should contain gentoo section"
    );
    assert!(
        all_content.contains("[guru]"),
        "should contain guru section"
    );
}
