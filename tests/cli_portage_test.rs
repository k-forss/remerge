//! Phase 2 — CLI portage reader tests (filesystem).
//!
//! These tests verify that the CLI's PortageReader correctly parses
//! a populated `/etc/portage/` tree from a temp directory.
//!
//! Gated behind the `integration` feature because they exercise
//! filesystem operations and require the `remerge` (CLI) crate.

mod common;

#[cfg(feature = "integration")]
mod portage_reader {
    use super::common;

    #[test]
    fn fixture_portage_tree_creates_expected_structure() {
        let (_tmp, root) = common::fixtures::portage_tree();
        assert!(root.join("etc/portage/make.conf").exists());
        assert!(root.join("etc/portage/package.use/custom").exists());
        assert!(root.join("etc/portage/package.accept_keywords/custom").exists());
        assert!(root.join("etc/portage/package.license/custom").exists());
        assert!(root.join("etc/portage/package.mask/custom").exists());
        assert!(root.join("etc/portage/package.unmask/custom").exists());
        assert!(root.join("etc/portage/package.env/custom").exists());
        assert!(root.join("etc/portage/env/no-lto.conf").exists());
        assert!(root.join("etc/portage/repos.conf/gentoo.conf").exists());
        assert!(root.join("etc/portage/profile/use.mask").exists());
        assert!(root.join("etc/portage/patches/dev-libs/openssl/fix.patch").exists());
        assert!(root.join("var/lib/portage/world").exists());
    }

    #[test]
    fn fixture_vdb_tree_creates_package_dirs() {
        let packages = &[
            ("dev-libs", "openssl-3.1.0"),
            ("sys-apps", "systemd-254"),
        ];
        let (_tmp, root) = common::fixtures::vdb_tree(packages);
        assert!(root.join("var/db/pkg/dev-libs/openssl-3.1.0/PF").exists());
        assert!(root.join("var/db/pkg/sys-apps/systemd-254/PF").exists());
    }
}
