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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
    let reader = remerge::portage::PortageReader::new().unwrap();

    assert!(!reader.is_installed("@world"), "sets are never installed");
}

/// is_installed: nonexistent package.
#[test]
fn is_installed_nonexistent() {
    let (_tmp, root) = common::fixtures::vdb_tree(&[("dev-libs", "openssl-3.1.0")]);
    unsafe {
        std::env::set_var("ROOT", root.to_str().unwrap());
    }
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
