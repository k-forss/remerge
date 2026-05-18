//! Phase 1 — Types & Validation (no I/O).
//!
//! Verifies all shared types serialize, deserialize, validate, and
//! display correctly.

mod common;

use remerge_types::api::{
    SNAPSHOT_BLOB_CHUNK_HEADER_LEN, SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
    SNAPSHOT_BLOB_ENCODING_ZSTD, SNAPSHOT_BLOB_PROTOCOL_VERSION, SnapshotBlobChunkFrameError,
    SnapshotBlobChunkHeader, SnapshotBlobClientControlMessage, SnapshotBlobEncodingOffer,
    SnapshotBlobServerControlMessage,
};
use remerge_types::auth::AuthMode;
use remerge_types::client::ClientRole;
use remerge_types::portage::*;
use remerge_types::validation::{AtomValidationError, validate_atom};
use remerge_types::workorder::*;

// ── PortageConfig round-trips ───────────────────────────────────────

/// Construct a fully-populated PortageConfig, serialize to JSON, deserialize,
/// and assert field-by-field equality.
#[test]
fn portage_config_full_roundtrip() {
    let config = common::fixtures::full_portage_config();
    let json = serde_json::to_string_pretty(&config).expect("serialize");
    let deserialized: PortageConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(config, deserialized, "full PortageConfig round-trip failed");
}

/// Deserialize a minimal JSON object into PortageConfig and verify defaults.
#[test]
fn portage_config_minimal_defaults() {
    let json = r#"{
        "make_conf": {
            "cflags": "-O2",
            "cxxflags": "",
            "ldflags": "",
            "makeopts": "-j1",
            "use_flags": [],
            "features": [],
            "accept_license": "",
            "accept_keywords": "",
            "emerge_default_opts": "",
            "use_expand": {},
            "extra": {}
        },
        "package_use": [],
        "package_accept_keywords": [],
        "package_license": [],
        "profile": "default/linux/amd64/23.0",
        "world": []
    }"#;
    let config: PortageConfig = serde_json::from_str(json).expect("deserialize");
    assert!(
        config.profile_overlay.is_empty(),
        "profile_overlay should default to empty BTreeMap"
    );
    assert!(
        !config.make_conf.use_flags_resolved,
        "use_flags_resolved should default to false"
    );
    assert!(config.patches.is_empty(), "patches should default to empty");
    assert!(
        config.package_mask.is_empty(),
        "package_mask should default to empty"
    );
    assert!(
        config.package_unmask.is_empty(),
        "package_unmask should default to empty"
    );
    assert!(
        config.package_env.is_empty(),
        "package_env should default to empty"
    );
    assert!(
        config.env_files.is_empty(),
        "env_files should default to empty"
    );
    assert!(
        config.repos_conf.is_empty(),
        "repos_conf should default to empty"
    );
    assert!(
        config.repo_snapshots.is_empty(),
        "repo_snapshots should default to empty"
    );
    assert!(
        config.repo_snapshot_refs.is_empty(),
        "repo_snapshot_refs should default to empty"
    );
    assert!(
        config.repo_snapshot_trees.is_empty(),
        "repo_snapshot_trees should default to empty"
    );
    assert!(
        config.distfile_snapshots.is_empty(),
        "distfile_snapshots should default to empty"
    );
    assert!(
        config.distfile_snapshot_refs.is_empty(),
        "distfile_snapshot_refs should default to empty"
    );
}

#[test]
fn portage_config_snapshot_payload_roundtrip() {
    let config = common::fixtures::full_portage_config();
    let json = serde_json::to_string(&config).expect("serialize");
    let deserialized: PortageConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(config.repo_snapshots, deserialized.repo_snapshots);
    assert_eq!(config.repo_snapshot_refs, deserialized.repo_snapshot_refs);
    assert_eq!(config.repo_snapshot_trees, deserialized.repo_snapshot_trees);
    assert_eq!(config.distfile_snapshots, deserialized.distfile_snapshots);
    assert_eq!(
        config.distfile_snapshot_refs,
        deserialized.distfile_snapshot_refs
    );
}

/// MakeConf defaults are sane.
#[test]
fn make_conf_defaults() {
    let mc = MakeConf::default();
    assert_eq!(mc.cflags, "-O2 -pipe");
    assert_eq!(mc.chost, "x86_64-pc-linux-gnu");
    assert!(!mc.use_flags_resolved);
    assert!(mc.cpu_flags.is_none());
    assert!(mc.original_cflags.is_none());
    assert!(mc.use_expand.is_empty());
    assert!(mc.extra.is_empty());
}

/// SystemIdentity round-trip through JSON.
#[test]
fn system_identity_roundtrip() {
    let id = common::fixtures::minimal_system_identity();
    let json = serde_json::to_string(&id).expect("serialize");
    let deserialized: SystemIdentity = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, deserialized);
}

// ── Workorder status transitions ────────────────────────────────────

/// Verify WorkorderStatus serializes with snake_case.
#[test]
fn workorder_status_serde() {
    let status = WorkorderStatus::Pending;
    let json = serde_json::to_string(&status).expect("serialize");
    assert_eq!(json, "\"pending\"");

    let status = WorkorderStatus::Failed {
        reason: "build error".into(),
    };
    let json = serde_json::to_string(&status).expect("serialize");
    assert!(
        json.contains("\"failed\""),
        "Failed should serialize as 'failed'"
    );
    assert!(json.contains("build error"));
}

/// Verify all status variants round-trip.
#[test]
fn workorder_status_all_variants_roundtrip() {
    let statuses = vec![
        WorkorderStatus::Pending,
        WorkorderStatus::Provisioning,
        WorkorderStatus::Building,
        WorkorderStatus::Completed,
        WorkorderStatus::Failed {
            reason: "dependency error".into(),
        },
        WorkorderStatus::Cancelled,
    ];
    for status in statuses {
        let json = serde_json::to_string(&status).expect("serialize");
        let deserialized: WorkorderStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, deserialized, "status round-trip failed for {json}");
    }
}

/// WorkorderResult with mixed built/failed packages.
#[test]
fn workorder_result_roundtrip() {
    let result = WorkorderResult {
        workorder_id: uuid::Uuid::new_v4(),
        built_packages: vec![BuiltPackage {
            atom: "dev-libs/openssl-3.1.4".into(),
            binpkg_path: "dev-libs/openssl-3.1.4.gpkg.tar".into(),
            sha256: "abcdef1234567890".into(),
            size: 1024,
        }],
        failed_packages: vec![FailedPackage {
            atom: "dev-libs/foo-1.0".into(),
            reason: "USE conflict".into(),
            build_log: Some("error log here".into()),
        }],
        binhost_uri: "http://localhost:7654/binpkgs".into(),
        fetched_distfiles: std::collections::BTreeMap::new(),
        parity_manifest: ParityManifest {
            files: std::collections::BTreeMap::from([(
                "var/db/repos/gentoo/metadata/timestamp.chk".into(),
                ParityFileEntry {
                    digest: "ab".repeat(32),
                    size: 42,
                    mtime_secs: 1_700_000_000,
                },
            )]),
            directories: std::collections::BTreeMap::from([(
                "var/lib/portage".into(),
                ParityDirectoryEntry {
                    mtime_secs: 1_700_000_001,
                },
            )]),
            symlinks: std::collections::BTreeMap::from([(
                "var/lib/portage/make.profile".into(),
                ParitySymlinkEntry {
                    digest: "cd".repeat(32),
                    size: 14,
                    mtime_secs: 1_700_000_002,
                },
            )]),
        },
    };
    let json = serde_json::to_string(&result).expect("serialize");
    let deserialized: WorkorderResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        result.built_packages.len(),
        deserialized.built_packages.len()
    );
    assert_eq!(
        result.failed_packages.len(),
        deserialized.failed_packages.len()
    );
    assert_eq!(result.binhost_uri, deserialized.binhost_uri);
    assert_eq!(result.parity_manifest, deserialized.parity_manifest);
}

/// BuildEvent tagged enum serialization.
#[test]
fn build_event_serde() {
    let events = vec![
        BuildEvent::StatusChanged {
            from: WorkorderStatus::Pending,
            to: WorkorderStatus::Building,
        },
        BuildEvent::Log {
            line: ">>> Emerging dev-libs/openssl".into(),
        },
        BuildEvent::PackageBuilt {
            atom: "dev-libs/openssl-3.1.4".into(),
            duration_secs: 120,
        },
        BuildEvent::PackageFailed {
            atom: "dev-libs/foo-1.0".into(),
            reason: "missing dep".into(),
        },
        BuildEvent::Finished {
            built: vec!["dev-libs/openssl".into()],
            failed: vec![],
        },
    ];
    for event in events {
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: BuildEvent = serde_json::from_str(&json).expect("deserialize");
        // Just verify it doesn't panic — BuildEvent doesn't impl PartialEq.
        let _ = format!("{:?}", deserialized);
    }
}

#[test]
fn snapshot_blob_control_messages_serialize_with_type_and_version() {
    let workorder_id = uuid::Uuid::new_v4();
    let client_message = SnapshotBlobClientControlMessage::UploadInit {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: "ab".repeat(32),
        total_size_bytes: 123,
        chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        capability_flags: Vec::new(),
        offered_encodings: vec![SnapshotBlobEncodingOffer {
            encoding: SNAPSHOT_BLOB_ENCODING_ZSTD.to_string(),
            size_bytes: 77,
        }],
    };
    let client_json = serde_json::to_value(&client_message).expect("serialize client control");
    assert_eq!(client_json["type"], "upload_init");
    assert_eq!(client_json["version"], SNAPSHOT_BLOB_PROTOCOL_VERSION);
    assert_eq!(client_json["offered_encodings"][0]["encoding"], "zstd");

    let server_message = SnapshotBlobServerControlMessage::UploadResume {
        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
        workorder_id,
        digest: "cd".repeat(32),
        next_offset_bytes: 10,
        next_sequence: 1,
        selected_encoding: Some(SNAPSHOT_BLOB_ENCODING_ZSTD.to_string()),
        expected_size_bytes: 20,
    };
    let server_json = serde_json::to_value(&server_message).expect("serialize server control");
    assert_eq!(server_json["type"], "upload_resume");
    assert_eq!(server_json["version"], SNAPSHOT_BLOB_PROTOCOL_VERSION);
    assert_eq!(server_json["selected_encoding"], "zstd");

    let roundtrip: SnapshotBlobServerControlMessage =
        serde_json::from_value(server_json).expect("deserialize server control");
    assert_eq!(roundtrip, server_message);
}

#[test]
fn snapshot_blob_chunk_header_roundtrip_and_checksum_validation() {
    let payload = b"chunk-payload";
    let header = SnapshotBlobChunkHeader::from_payload(7, 42, payload);
    let frame = header
        .encode_with_payload(payload)
        .expect("encode chunk frame");
    assert_eq!(frame.len(), SNAPSHOT_BLOB_CHUNK_HEADER_LEN + payload.len());

    let (decoded, decoded_payload) = SnapshotBlobChunkHeader::decode(&frame).expect("decode frame");
    assert_eq!(decoded, header);
    assert_eq!(decoded_payload, payload);

    let mut corrupted = frame.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x01;
    let error =
        SnapshotBlobChunkHeader::decode(&corrupted).expect_err("corrupted frame should fail");
    assert!(matches!(
        error,
        SnapshotBlobChunkFrameError::ChecksumMismatch { .. }
    ));
}

// ── validate_atom exhaustive ────────────────────────────────────────

/// Qualified atoms should be accepted.
#[test]
fn validate_atom_qualified() {
    assert!(validate_atom("dev-libs/openssl").is_ok());
    assert!(validate_atom("sys-kernel/gentoo-sources").is_ok());
    assert!(validate_atom("app-misc/screen").is_ok());
    assert!(validate_atom("x11-libs/gtk+").is_ok());
}

/// Versioned atoms with various operators should be accepted.
#[test]
fn validate_atom_versioned() {
    assert!(validate_atom("=dev-libs/openssl-3.1.0").is_ok());
    assert!(validate_atom(">=dev-libs/openssl-3.0").is_ok());
    assert!(validate_atom("<=dev-libs/openssl-3.0").is_ok());
    assert!(validate_atom(">dev-libs/openssl-3.0").is_ok());
    assert!(validate_atom("<dev-libs/openssl-4.0").is_ok());
    assert!(validate_atom("~dev-libs/openssl-3.1.0").is_ok());
    assert!(validate_atom("=dev-libs/openssl-3.1*").is_ok());
}

/// Package sets should be accepted.
#[test]
fn validate_atom_sets() {
    assert!(validate_atom("@world").is_ok());
    assert!(validate_atom("@system").is_ok());
    assert!(validate_atom("@preserved-rebuild").is_ok());
}

/// Unqualified atoms (no category) should be accepted.
#[test]
fn validate_atom_unqualified() {
    assert!(validate_atom("gcc").is_ok());
    assert!(validate_atom("firefox").is_ok());
    assert!(validate_atom("gentoo-sources").is_ok());
}

/// Empty atom should be rejected.
#[test]
fn validate_atom_reject_empty() {
    let err = validate_atom("").unwrap_err();
    assert!(matches!(err, AtomValidationError::Empty));
}

/// Shell injection should be rejected.
#[test]
fn validate_atom_reject_shell_injection() {
    assert!(matches!(
        validate_atom("; rm -rf /").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("$(evil)").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("`evil`").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("dev-libs/openssl\"").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("foo\\bar").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("foo'bar").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("foo&bar").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
    assert!(matches!(
        validate_atom("foo|bar").unwrap_err(),
        AtomValidationError::DangerousCharacters
    ));
}

/// Versioned unqualified atom should be rejected.
#[test]
fn validate_atom_reject_versioned_unqualified() {
    assert!(validate_atom("=gcc-12").is_err());
    assert!(validate_atom(">=openssl-3.0").is_err());
}

/// Empty set name should be rejected.
#[test]
fn validate_atom_reject_empty_set() {
    assert!(validate_atom("@").is_err());
}

/// Empty category should be rejected.
#[test]
fn validate_atom_reject_empty_category() {
    assert!(validate_atom("/openssl").is_err());
}

/// Empty package name should be rejected.
#[test]
fn validate_atom_reject_empty_package() {
    assert!(validate_atom("dev-libs/").is_err());
}

/// Multiple slashes should be rejected.
#[test]
fn validate_atom_reject_multiple_slashes() {
    assert!(validate_atom("dev-libs/openssl/extra").is_err());
}

// ── ClientRole Display+FromStr round-trip ───────────────────────────

/// ClientRole Display and FromStr should round-trip.
#[test]
fn client_role_display_fromstr() {
    let main_str = ClientRole::Main.to_string();
    assert_eq!(main_str, "main");
    let parsed: ClientRole = main_str.parse().expect("parse main");
    assert_eq!(parsed, ClientRole::Main);

    let follower_str = ClientRole::Follower.to_string();
    assert_eq!(follower_str, "follower");
    let parsed: ClientRole = follower_str.parse().expect("parse follower");
    assert_eq!(parsed, ClientRole::Follower);
}

/// ClientRole FromStr rejects unknown values.
#[test]
fn client_role_fromstr_rejects_unknown() {
    assert!("admin".parse::<ClientRole>().is_err());
    assert!("".parse::<ClientRole>().is_err());
}

// ── AuthMode Display+FromStr round-trip ─────────────────────────────

/// AuthMode Display and FromStr should round-trip.
#[test]
fn auth_mode_display_fromstr() {
    for mode in [AuthMode::None, AuthMode::Mtls, AuthMode::Mixed] {
        let s = mode.to_string();
        let parsed: AuthMode = s.parse().expect("parse auth mode");
        assert_eq!(parsed, mode, "AuthMode round-trip failed for {s}");
    }
}

/// AuthMode FromStr rejects unknown values.
#[test]
fn auth_mode_fromstr_rejects_unknown() {
    assert!("tls".parse::<AuthMode>().is_err());
    assert!("".parse::<AuthMode>().is_err());
}

/// ClientRole default is Main.
#[test]
fn client_role_default_is_main() {
    assert_eq!(ClientRole::default(), ClientRole::Main);
}

/// AuthMode default is None.
#[test]
fn auth_mode_default_is_none() {
    assert_eq!(AuthMode::default(), AuthMode::None);
}

/// ConfigDiff is_empty works correctly.
#[test]
fn config_diff_is_empty() {
    use remerge_types::client::ConfigDiff;
    let empty = ConfigDiff {
        portage_changed: false,
        system_changed: false,
    };
    assert!(empty.is_empty());

    let portage = ConfigDiff {
        portage_changed: true,
        system_changed: false,
    };
    assert!(!portage.is_empty());

    let system = ConfigDiff {
        portage_changed: false,
        system_changed: true,
    };
    assert!(!system.is_empty());

    let both = ConfigDiff {
        portage_changed: true,
        system_changed: true,
    };
    assert!(!both.is_empty());
}

/// Full PortageConfig has all fields populated.
#[test]
fn full_portage_config_every_field() {
    let config = common::fixtures::full_portage_config();
    assert!(!config.make_conf.cflags.is_empty());
    assert!(!config.make_conf.chost.is_empty());
    assert!(config.make_conf.use_flags_resolved);
    assert!(config.make_conf.cpu_flags.is_some());
    assert!(config.make_conf.original_cflags.is_some());
    assert!(!config.make_conf.use_expand.is_empty());
    assert!(!config.make_conf.extra.is_empty());
    assert!(!config.package_use.is_empty());
    assert!(!config.package_accept_keywords.is_empty());
    assert!(!config.package_license.is_empty());
    assert!(!config.package_mask.is_empty());
    assert!(!config.package_unmask.is_empty());
    assert!(!config.package_env.is_empty());
    assert!(!config.env_files.is_empty());
    assert!(!config.repos_conf.is_empty());
    assert!(!config.patches.is_empty());
    assert!(!config.profile_overlay.is_empty());
    assert!(!config.world.is_empty());
}
