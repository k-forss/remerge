//! Phase 1 — Pure logic tests for remerge-types.
//!
//! These tests verify serialization round-trips, validation rules,
//! and default values without touching the filesystem or network.

mod common;

use remerge_types::portage::*;

// ── Serialization round-trips ───────────────────────────────────────

#[test]
fn portage_config_serde_roundtrip() {
    let config = common::fixtures::full_portage_config();
    let json = serde_json::to_string_pretty(&config).expect("serialize");
    let deserialized: PortageConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(config, deserialized);
}

#[test]
fn make_conf_default_values() {
    let mc = MakeConf::default();
    assert_eq!(mc.chost, "x86_64-pc-linux-gnu");
    assert!(!mc.use_flags_resolved);
}

#[test]
fn system_identity_serde_roundtrip() {
    let id = common::fixtures::minimal_system_identity();
    let json = serde_json::to_string(&id).expect("serialize");
    let deserialized: SystemIdentity = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, deserialized);
}

#[test]
fn minimal_portage_config_has_empty_collections() {
    let config = common::fixtures::minimal_portage_config();
    assert!(config.package_use.is_empty());
    assert!(config.package_mask.is_empty());
    assert!(config.world.is_empty());
    assert!(config.patches.is_empty());
}

#[test]
fn full_portage_config_has_populated_fields() {
    let config = common::fixtures::full_portage_config();
    assert!(!config.package_use.is_empty());
    assert!(!config.package_mask.is_empty());
    assert!(!config.world.is_empty());
    assert!(!config.repos_conf.is_empty());
    assert!(!config.patches.is_empty());
    assert!(!config.env_files.is_empty());
    assert!(config.make_conf.use_flags_resolved);
}
