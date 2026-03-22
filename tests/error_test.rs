//! Phase 7 — Error path tests.
//!
//! These tests verify that error conditions are handled gracefully:
//! invalid configs, missing files, malformed JSON, etc.

mod common;

use remerge_types::portage::*;

#[test]
fn deserialize_empty_json_fails() {
    let result = serde_json::from_str::<PortageConfig>("");
    assert!(result.is_err());
}

#[test]
fn deserialize_invalid_json_fails() {
    let result = serde_json::from_str::<PortageConfig>("not valid json");
    assert!(result.is_err());
}

#[test]
fn deserialize_wrong_type_fails() {
    // Pass a JSON array where an object is expected.
    let result = serde_json::from_str::<PortageConfig>("[]");
    assert!(result.is_err());
}

#[test]
fn system_identity_missing_fields_fails() {
    let json = r#"{"arch": "amd64"}"#;
    let result = serde_json::from_str::<SystemIdentity>(json);
    assert!(result.is_err());
}

#[test]
fn make_conf_default_is_valid() {
    let mc = MakeConf::default();
    // Defaults should produce a non-empty chost
    assert!(!mc.chost.is_empty());
}

#[cfg(feature = "integration")]
mod error_integration {
    use super::common;

    #[test]
    fn vdb_tree_with_no_packages_is_empty() {
        let (_tmp, root) = common::fixtures::vdb_tree(&[]);
        let pkg_dir = root.join("var/db/pkg");
        if pkg_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&pkg_dir).unwrap().collect();
            assert!(entries.is_empty(), "var/db/pkg should contain no packages");
        }
        // If the directory doesn't exist at all, that's also valid.
    }
}
