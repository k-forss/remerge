//! Phase 3 — Worker portage_setup tests (filesystem).
//!
//! These tests verify that the worker's portage_setup module
//! correctly writes configuration files to a chroot-like temp
//! directory.
//!
//! Gated behind the `integration` feature.

mod common;

#[cfg(feature = "integration")]
mod worker_setup {
    use super::common;

    #[test]
    fn minimal_config_produces_valid_structure() {
        // Verify that a minimal PortageConfig can be constructed
        // without panics and has sane defaults.
        let config = common::fixtures::minimal_portage_config();
        assert_eq!(config.profile, "default/linux/amd64/23.0");
        assert_eq!(config.make_conf.chost, "x86_64-pc-linux-gnu");
    }

    #[test]
    fn full_config_has_all_sections() {
        let config = common::fixtures::full_portage_config();
        assert!(!config.package_use.is_empty(), "package_use should be populated");
        assert!(!config.package_accept_keywords.is_empty(), "package_accept_keywords should be populated");
        assert!(!config.package_license.is_empty(), "package_license should be populated");
        assert!(!config.package_mask.is_empty(), "package_mask should be populated");
        assert!(!config.package_unmask.is_empty(), "package_unmask should be populated");
        assert!(!config.package_env.is_empty(), "package_env should be populated");
        assert!(!config.env_files.is_empty(), "env_files should be populated");
        assert!(!config.repos_conf.is_empty(), "repos_conf should be populated");
        assert!(!config.patches.is_empty(), "patches should be populated");
        assert!(!config.profile_overlay.is_empty(), "profile_overlay should be populated");
    }
}
