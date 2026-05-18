//! Phase 4 — Server runtime staging tests.

mod common;

use remerge::portage::PortageReader;
use sha2::{Digest, Sha256};

fn count_blob_files(root: &std::path::Path) -> usize {
    fn walk(path: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };

        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .map(|path| {
                if path.is_dir() {
                    walk(&path)
                } else {
                    usize::from(path.extension().is_none())
                }
            })
            .sum()
    }

    walk(root)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn count_tree_files(root: &std::path::Path) -> usize {
    fn walk(path: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };

        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .map(|path| {
                if path.is_dir() {
                    walk(&path)
                } else {
                    usize::from(path.file_name().and_then(|name| name.to_str()).is_some_and(
                        |name| name.ends_with(".json") && !name.ends_with(".meta.json"),
                    ))
                }
            })
            .sum()
    }

    walk(root)
}

#[tokio::test]
async fn stage_workorder_runtime_writes_snapshot_files_and_strips_payload() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let workorder_id = uuid::Uuid::new_v4();

    let mut workorder = remerge_types::workorder::Workorder {
        id: workorder_id,
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/demo".into()],
        emerge_args: vec!["dev-libs/demo".into()],
        portage_config: common::fixtures::full_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Pending,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    workorder.portage_config.repo_snapshots.insert(
        "local-overlay".into(),
        std::collections::BTreeMap::from([(
            "dev-libs/demo/demo-1.0.ebuild".into(),
            "EAPI=8\n".into(),
        )]),
    );
    workorder
        .portage_config
        .distfile_snapshots
        .insert("demo-1.0.tar.xz".into(), b"demo-distfile".to_vec());

    let staged = remerge_server::runtime::stage_workorder_runtime(state_dir.path(), &workorder)
        .await
        .expect("stage_workorder_runtime");

    assert!(staged.workorder_json_path.is_file());
    assert!(
        staged
            .runtime_dir
            .join("snapshots/repos/local-overlay/dev-libs/demo/demo-1.0.ebuild")
            .is_file()
    );
    assert!(
        staged
            .runtime_dir
            .join("snapshots/distfiles/demo-1.0.tar.xz")
            .is_file()
    );

    let staged_workorder: remerge_types::workorder::Workorder =
        serde_json::from_slice(&tokio::fs::read(&staged.workorder_json_path).await.unwrap())
            .unwrap();
    assert!(staged_workorder.portage_config.repo_snapshots.is_empty());
    assert!(
        staged_workorder
            .portage_config
            .distfile_snapshots
            .is_empty()
    );
    assert_eq!(
        staged_workorder.portage_config.repo_snapshot_refs["local-overlay"]["dev-libs/demo/demo-1.0.ebuild"],
        sha256_hex(b"EAPI=8\n")
    );
    assert_eq!(
        staged_workorder.portage_config.distfile_snapshot_refs["demo-1.0.tar.xz"],
        sha256_hex(b"demo-distfile")
    );
    assert!(!staged_workorder.portage_config.repo_snapshot_trees["local-overlay"].is_empty());
}

#[tokio::test]
async fn stage_workorder_runtime_allows_distfile_basenames_with_literal_double_dot() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let workorder_id = uuid::Uuid::new_v4();

    let mut workorder = remerge_types::workorder::Workorder {
        id: workorder_id,
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/demo".into()],
        emerge_args: vec!["dev-libs/demo".into()],
        portage_config: common::fixtures::full_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Pending,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    workorder
        .portage_config
        .distfile_snapshots
        .insert("foo..bar.tar.xz".into(), b"demo-distfile".to_vec());

    let staged = remerge_server::runtime::stage_workorder_runtime(state_dir.path(), &workorder)
        .await
        .expect("stage_workorder_runtime");

    assert!(
        staged
            .runtime_dir
            .join("snapshots/distfiles/foo..bar.tar.xz")
            .is_file()
    );
}

#[tokio::test]
async fn normalize_portage_config_snapshots_converts_inline_payloads_to_refs_only() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let mut portage_config = common::fixtures::minimal_portage_config();
    portage_config.repo_snapshots.insert(
        "local-overlay".into(),
        std::collections::BTreeMap::from([(
            "dev-libs/demo/demo-1.0.ebuild".into(),
            "EAPI=8\n".into(),
        )]),
    );
    portage_config
        .distfile_snapshots
        .insert("demo-1.0.tar.xz".into(), b"demo-distfile".to_vec());

    let normalized = remerge_server::runtime::normalize_portage_config_snapshots(
        state_dir.path(),
        &portage_config,
    )
    .await
    .expect("normalize snapshot payloads");

    assert!(normalized.repo_snapshots.is_empty());
    assert!(normalized.distfile_snapshots.is_empty());
    assert_eq!(
        normalized.repo_snapshot_refs["local-overlay"]["dev-libs/demo/demo-1.0.ebuild"],
        sha256_hex(b"EAPI=8\n")
    );
    assert_eq!(
        normalized.distfile_snapshot_refs["demo-1.0.tar.xz"],
        sha256_hex(b"demo-distfile")
    );
    assert!(
        remerge_server::blob_store::has_blob(
            state_dir.path(),
            &normalized.repo_snapshot_refs["local-overlay"]["dev-libs/demo/demo-1.0.ebuild"],
        )
        .await
        .unwrap()
    );
    assert!(
        remerge_server::blob_store::has_blob(
            state_dir.path(),
            &normalized.distfile_snapshot_refs["demo-1.0.tar.xz"],
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn stage_workorder_runtime_reuses_blob_store_for_identical_snapshots() {
    let state_dir = tempfile::TempDir::new().unwrap();

    let mut first = remerge_types::workorder::Workorder {
        id: uuid::Uuid::new_v4(),
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/demo".into()],
        emerge_args: vec!["dev-libs/demo".into()],
        portage_config: common::fixtures::full_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Pending,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    first.portage_config.repo_snapshots.insert(
        "local-overlay".into(),
        std::collections::BTreeMap::from([(
            "dev-libs/demo/demo-1.0.ebuild".into(),
            "EAPI=8\n".into(),
        )]),
    );
    first
        .portage_config
        .distfile_snapshots
        .insert("demo-1.0.tar.xz".into(), b"demo-distfile".to_vec());

    let mut workorders = vec![first];
    for _ in 0..3 {
        let mut cloned = workorders[0].clone();
        cloned.id = uuid::Uuid::new_v4();
        cloned.client_id = uuid::Uuid::new_v4();
        workorders.push(cloned);
    }

    for workorder in &workorders {
        remerge_server::runtime::stage_workorder_runtime(state_dir.path(), workorder)
            .await
            .expect("stage workorder");
    }

    let blob_root = state_dir.path().join("blobs");
    let tree_root = state_dir.path().join("trees");
    assert_eq!(count_blob_files(&blob_root), 2);
    assert_eq!(count_tree_files(&tree_root), 1);
}

#[tokio::test]
async fn client_overlay_snapshot_roundtrip_supports_local_only_package_versions() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let (_tmp, root, overlay_name, distfile_name) =
        common::fixtures::portage_tree_with_local_overlay();
    let _root_guard = common::set_root_env(&root);

    let portage_config = PortageReader::new()
        .expect("portage reader")
        .read_config()
        .expect("read config");
    let overlay_entry = portage_config.snapshot_manifest.repo_snapshots[&overlay_name]
        .entries
        .get("dev-libs/demo/demo-1.0.ebuild")
        .expect("overlay entry");
    let overlay_bytes = b"EAPI=8\nDESCRIPTION=\"demo\"\nSRC_URI=\"https://example.invalid/demo-1.0.tar.xz\"\nSLOT=\"0\"\nKEYWORDS=\"~amd64\"\n";
    let distfile_entry = portage_config
        .snapshot_manifest
        .distfiles
        .get(&distfile_name)
        .expect("distfile entry");
    assert_eq!(overlay_entry.size, overlay_bytes.len() as u64);
    assert_eq!(distfile_entry.size, b"demo-distfile".len() as u64);

    let workorder = remerge_types::workorder::Workorder {
        id: uuid::Uuid::new_v4(),
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["=dev-libs/demo-1.0".into()],
        emerge_args: vec!["=dev-libs/demo-1.0".into()],
        portage_config,
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Pending,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let staged = remerge_server::runtime::stage_workorder_runtime(state_dir.path(), &workorder)
        .await
        .expect("stage overlay workorder");

    assert_eq!(
        tokio::fs::read(staged.runtime_dir.join(format!(
            "snapshots/repos/{overlay_name}/dev-libs/demo/demo-1.0.ebuild"
        )))
        .await
        .unwrap(),
        overlay_bytes
    );
    assert_eq!(
        tokio::fs::read(
            staged
                .runtime_dir
                .join(format!("snapshots/distfiles/{distfile_name}"))
        )
        .await
        .unwrap(),
        b"demo-distfile"
    );
}

#[tokio::test]
async fn stage_workorder_runtime_materializes_ref_only_snapshots() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let started_at = chrono::Utc::now();
    let repo_bytes = b"EAPI=8\n";
    let distfile_bytes = b"demo-distfile";
    let repo_digest = sha256_hex(repo_bytes);
    let distfile_digest = sha256_hex(distfile_bytes);

    remerge_server::blob_store::store_blob(state_dir.path(), repo_bytes)
        .await
        .expect("store repo blob");
    remerge_server::blob_store::store_blob(state_dir.path(), distfile_bytes)
        .await
        .expect("store distfile blob");

    let repo_refs = std::collections::BTreeMap::from([(
        "dev-libs/demo/demo-1.0.ebuild".to_string(),
        repo_digest.clone(),
    )]);
    let tree = remerge_server::tree_store::store_tree(state_dir.path(), &repo_refs)
        .await
        .expect("store tree");

    let mut workorder = remerge_types::workorder::Workorder {
        id: uuid::Uuid::new_v4(),
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/demo".into()],
        emerge_args: vec!["dev-libs/demo".into()],
        portage_config: common::fixtures::minimal_portage_config(),
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Pending,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    workorder
        .portage_config
        .repo_snapshot_refs
        .insert("local-overlay".into(), repo_refs);
    workorder
        .portage_config
        .repo_snapshot_trees
        .insert("local-overlay".into(), tree.digest.clone());
    workorder
        .portage_config
        .distfile_snapshot_refs
        .insert("demo-1.0.tar.xz".into(), distfile_digest.clone());

    let staged = remerge_server::runtime::stage_workorder_runtime(state_dir.path(), &workorder)
        .await
        .expect("stage ref-only workorder runtime");

    let repo_path = staged
        .runtime_dir
        .join("snapshots/repos/local-overlay/dev-libs/demo/demo-1.0.ebuild");
    let distfile_path = staged
        .runtime_dir
        .join("snapshots/distfiles/demo-1.0.tar.xz");
    let expected_blob_bytes = (repo_bytes.len() + distfile_bytes.len()) as u64;
    let expected_tree_bytes = tokio::fs::metadata(&tree.path).await.unwrap().len();
    assert_eq!(tokio::fs::read(repo_path).await.unwrap(), repo_bytes);
    assert_eq!(
        tokio::fs::read(distfile_path).await.unwrap(),
        distfile_bytes
    );

    let staged_workorder: remerge_types::workorder::Workorder =
        serde_json::from_slice(&tokio::fs::read(&staged.workorder_json_path).await.unwrap())
            .unwrap();
    assert!(staged_workorder.portage_config.repo_snapshots.is_empty());
    assert!(
        staged_workorder
            .portage_config
            .distfile_snapshots
            .is_empty()
    );
    assert_eq!(
        staged_workorder.portage_config.repo_snapshot_refs["local-overlay"]["dev-libs/demo/demo-1.0.ebuild"],
        repo_digest
    );
    assert_eq!(
        staged_workorder.portage_config.repo_snapshot_trees["local-overlay"],
        tree.digest
    );
    assert_eq!(
        staged_workorder.portage_config.distfile_snapshot_refs["demo-1.0.tar.xz"],
        distfile_digest
    );
    assert_eq!(
        staged.snapshot_references.blob_digests,
        std::collections::BTreeSet::from([repo_digest, distfile_digest])
    );
    assert_eq!(
        staged.snapshot_references.tree_digests,
        std::collections::BTreeSet::from([tree.digest])
    );
    assert_eq!(
        staged.snapshot_references.total_blob_bytes,
        expected_blob_bytes
    );
    assert_eq!(
        staged.snapshot_references.total_tree_bytes,
        expected_tree_bytes
    );
    assert!(staged.snapshot_references.last_referenced_at >= started_at);
    assert!(staged.snapshot_references.last_referenced_at <= chrono::Utc::now());
}

#[tokio::test]
async fn persisted_refs_only_workorder_can_be_reloaded_and_restaged_after_restart() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let mut portage_config = common::fixtures::minimal_portage_config();
    portage_config.repo_snapshots.insert(
        "local-overlay".into(),
        std::collections::BTreeMap::from([(
            "dev-libs/demo/demo-1.0.ebuild".into(),
            "EAPI=8\n".into(),
        )]),
    );
    portage_config
        .distfile_snapshots
        .insert("demo-1.0.tar.xz".into(), b"demo-distfile".to_vec());
    let normalized = remerge_server::runtime::normalize_portage_config_snapshots(
        state_dir.path(),
        &portage_config,
    )
    .await
    .expect("normalize snapshot payloads");

    let workorder = remerge_types::workorder::Workorder {
        id: uuid::Uuid::new_v4(),
        client_id: uuid::Uuid::new_v4(),
        role: remerge_types::client::ClientRole::Main,
        atoms: vec!["dev-libs/demo".into()],
        emerge_args: vec!["dev-libs/demo".into()],
        portage_config: normalized,
        system_id: common::fixtures::minimal_system_identity(),
        trace_context: None,
        status: remerge_types::workorder::WorkorderStatus::Provisioning,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    remerge_server::persistence::save_workorders(
        state_dir.path(),
        &std::collections::HashMap::from([(workorder.id, workorder)]),
    )
    .await
    .expect("save persisted workorder");

    let reloaded = remerge_server::persistence::load_workorders(state_dir.path())
        .await
        .expect("load persisted workorders");
    let reloaded_workorder = reloaded.values().next().expect("reloaded workorder");
    assert!(matches!(
        reloaded_workorder.status,
        remerge_types::workorder::WorkorderStatus::Pending
    ));
    assert!(reloaded_workorder.portage_config.repo_snapshots.is_empty());
    assert!(
        reloaded_workorder
            .portage_config
            .distfile_snapshots
            .is_empty()
    );

    let staged =
        remerge_server::runtime::stage_workorder_runtime(state_dir.path(), reloaded_workorder)
            .await
            .expect("restage persisted workorder");
    assert_eq!(
        tokio::fs::read(
            staged
                .runtime_dir
                .join("snapshots/repos/local-overlay/dev-libs/demo/demo-1.0.ebuild"),
        )
        .await
        .unwrap(),
        b"EAPI=8\n"
    );
    assert_eq!(
        tokio::fs::read(
            staged
                .runtime_dir
                .join("snapshots/distfiles/demo-1.0.tar.xz")
        )
        .await
        .unwrap(),
        b"demo-distfile"
    );
}

#[tokio::test]
async fn ingest_final_state_parity_stores_captured_blobs() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let runtime_dir = state_dir.path().join("runtime/workorder-1");
    let parity_dir = runtime_dir.join("parity/blobs");
    tokio::fs::create_dir_all(&parity_dir).await.unwrap();

    let payload = b"metadata-cache\n";
    let digest = sha256_hex(payload);
    tokio::fs::write(parity_dir.join(&digest), payload)
        .await
        .unwrap();
    tokio::fs::write(
        runtime_dir.join("parity/manifest.json"),
        serde_json::to_vec(&remerge_types::workorder::ParityManifest {
            files: std::collections::BTreeMap::from([(
                "var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0".into(),
                remerge_types::workorder::ParityFileEntry {
                    digest: digest.clone(),
                    size: payload.len() as u64,
                    mtime_secs: 1_700_000_123,
                },
            )]),
            directories: std::collections::BTreeMap::new(),
            symlinks: std::collections::BTreeMap::new(),
        })
        .unwrap(),
    )
    .await
    .unwrap();

    let manifest =
        remerge_server::runtime::ingest_final_state_parity(state_dir.path(), &runtime_dir)
            .await
            .expect("ingest final-state parity");

    assert_eq!(manifest.files.len(), 1);
    let stored_path = remerge_server::blob_store::blob_path(state_dir.path(), &digest).unwrap();
    assert_eq!(tokio::fs::read(stored_path).await.unwrap(), payload);
}

#[tokio::test]
async fn ingest_fetched_distfiles_stores_captured_blobs() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let runtime_dir = state_dir.path().join("runtime/workorder-1");
    let blobs_dir = runtime_dir.join("parity/blobs");
    tokio::fs::create_dir_all(&blobs_dir).await.unwrap();

    let payload = b"fetched-distfile\n";
    let digest = sha256_hex(payload);
    tokio::fs::write(blobs_dir.join(&digest), payload)
        .await
        .unwrap();
    tokio::fs::write(
        runtime_dir.join("parity/distfiles.json"),
        serde_json::to_vec(&std::collections::BTreeMap::from([(
            "demo-1.0.tar.xz".to_string(),
            remerge_types::portage::SnapshotEntry {
                digest: digest.clone(),
                size: payload.len() as u64,
                mtime_secs: 1_700_000_456,
            },
        )]))
        .unwrap(),
    )
    .await
    .unwrap();

    let manifest =
        remerge_server::runtime::ingest_fetched_distfiles(state_dir.path(), &runtime_dir)
            .await
            .expect("ingest fetched distfiles");

    assert_eq!(manifest.len(), 1);
    assert_eq!(manifest["demo-1.0.tar.xz"].digest, digest);
    let stored_path = remerge_server::blob_store::blob_path(
        state_dir.path(),
        &manifest["demo-1.0.tar.xz"].digest,
    )
    .unwrap();
    assert_eq!(tokio::fs::read(stored_path).await.unwrap(), payload);
}

#[tokio::test]
async fn store_tree_creates_zstd_variant_for_worthwhile_manifest() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let entries = (0..4_000)
        .map(|index| {
            (
                format!("dev-libs/demo/file-{index}.ebuild"),
                "ab".repeat(32),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    let stored = remerge_server::tree_store::store_tree(state_dir.path(), &entries)
        .await
        .expect("store tree");
    let metadata = remerge_server::tree_store::load_tree_metadata(state_dir.path(), &stored.digest)
        .await
        .expect("load tree metadata");
    let encoded_path = remerge_server::tree_store::encoded_tree_path(
        state_dir.path(),
        &stored.digest,
        remerge_server::blob_store::BlobEncoding::Zstd,
    )
    .expect("tree zstd path");

    assert!(encoded_path.is_file());
    assert!(
        metadata
            .encoded_variants
            .contains_key(&remerge_server::blob_store::BlobEncoding::Zstd)
    );
}

#[tokio::test]
async fn cleanup_snapshot_storage_deletes_grace_eligible_unreferenced_entries() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 0,
        ..Default::default()
    };

    let blob_bytes = b"stale-blob";
    let blob = remerge_server::blob_store::store_blob(state_dir.path(), blob_bytes)
        .await
        .expect("store blob");
    let tree = remerge_server::tree_store::store_tree(
        state_dir.path(),
        &std::collections::BTreeMap::from([("file.txt".to_string(), blob.digest.clone())]),
    )
    .await
    .expect("store tree");

    let stale_at = chrono::Utc::now() - chrono::Duration::days(8);
    remerge_server::blob_store::touch_blob(state_dir.path(), &blob.digest, stale_at)
        .await
        .expect("touch stale blob");
    remerge_server::tree_store::touch_tree(state_dir.path(), &tree.digest, stale_at)
        .await
        .expect("touch stale tree");

    let summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("cleanup snapshot storage");

    assert_eq!(summary.deleted_blobs, 1);
    assert_eq!(summary.deleted_trees, 1);
    assert!(summary.reclaimed_bytes >= blob_bytes.len() as u64);
    assert!(
        !remerge_server::blob_store::has_blob(state_dir.path(), &blob.digest)
            .await
            .unwrap()
    );
    assert!(
        !remerge_server::tree_store::tree_path(state_dir.path(), &tree.digest)
            .unwrap()
            .exists()
    );
}

#[tokio::test]
async fn cleanup_snapshot_storage_handles_many_eligible_entries() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 24,
        snapshot_cache_hard_delete_hours: 24 * 30,
        snapshot_min_retained_bytes: 0,
        ..Default::default()
    };

    let stale_at = chrono::Utc::now() - chrono::Duration::days(2);
    let mut tree_digests = Vec::new();
    let mut blob_digests = Vec::new();
    for index in 0..24 {
        let payload = format!("payload-{index}").into_bytes();
        let blob = remerge_server::blob_store::store_blob(state_dir.path(), &payload)
            .await
            .expect("store blob");
        let tree = remerge_server::tree_store::store_tree(
            state_dir.path(),
            &std::collections::BTreeMap::from([(format!("file-{index}.txt"), blob.digest.clone())]),
        )
        .await
        .expect("store tree");
        remerge_server::blob_store::touch_blob(state_dir.path(), &blob.digest, stale_at)
            .await
            .expect("touch blob");
        remerge_server::tree_store::touch_tree(state_dir.path(), &tree.digest, stale_at)
            .await
            .expect("touch tree");
        blob_digests.push(blob.digest);
        tree_digests.push(tree.digest);
    }

    let summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("cleanup snapshot storage");

    assert_eq!(summary.deleted_blobs, 24);
    assert_eq!(summary.deleted_trees, 24);
    for digest in blob_digests {
        assert!(
            !remerge_server::blob_store::has_blob(state_dir.path(), &digest)
                .await
                .unwrap()
        );
    }
    for digest in tree_digests {
        let tree_path = remerge_server::tree_store::tree_path(state_dir.path(), &digest).unwrap();
        assert!(!tokio::fs::try_exists(&tree_path).await.unwrap());
    }
}

#[tokio::test]
async fn cleanup_snapshot_storage_is_idempotent_after_deletion() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 0,
        ..Default::default()
    };

    let blob = remerge_server::blob_store::store_blob(state_dir.path(), b"stale-blob")
        .await
        .expect("store blob");
    let stale_at = chrono::Utc::now() - chrono::Duration::days(8);
    remerge_server::blob_store::touch_blob(state_dir.path(), &blob.digest, stale_at)
        .await
        .expect("touch stale blob");

    let first = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("first cleanup snapshot storage");
    let second = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("second cleanup snapshot storage");

    assert_eq!(first.deleted_blobs, 1);
    assert_eq!(second.deleted_blobs, 0);
    assert_eq!(second.deleted_trees, 0);
    assert_eq!(second.reclaimed_bytes, 0);
}

#[tokio::test]
async fn cleanup_snapshot_storage_keeps_active_and_floor_protected_entries() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 1_000_000,
        ..Default::default()
    };

    let active_blob = remerge_server::blob_store::store_blob(state_dir.path(), b"active-blob")
        .await
        .expect("store active blob");
    let floor_blob = remerge_server::blob_store::store_blob(state_dir.path(), b"floor-blob")
        .await
        .expect("store floor blob");
    let expired_blob = remerge_server::blob_store::store_blob(state_dir.path(), b"expired-blob")
        .await
        .expect("store expired blob");

    let old_grace = chrono::Utc::now() - chrono::Duration::days(8);
    let old_hard_delete = chrono::Utc::now() - chrono::Duration::days(31);
    remerge_server::blob_store::touch_blob(state_dir.path(), &active_blob.digest, old_hard_delete)
        .await
        .expect("touch active blob");
    remerge_server::blob_store::touch_blob(state_dir.path(), &floor_blob.digest, old_grace)
        .await
        .expect("touch floor blob");
    remerge_server::blob_store::touch_blob(state_dir.path(), &expired_blob.digest, old_hard_delete)
        .await
        .expect("touch expired blob");

    let active_references = [remerge_server::runtime::SnapshotReferenceSet {
        blob_digests: std::collections::BTreeSet::from([active_blob.digest.clone()]),
        tree_digests: std::collections::BTreeSet::new(),
        total_blob_bytes: 0,
        total_tree_bytes: 0,
        last_referenced_at: chrono::Utc::now(),
    }];

    let summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &active_references,
        chrono::Utc::now(),
    )
    .await
    .expect("cleanup snapshot storage");

    assert_eq!(summary.deleted_blobs, 0);
    assert!(
        remerge_server::blob_store::has_blob(state_dir.path(), &active_blob.digest)
            .await
            .unwrap()
    );
    assert!(
        remerge_server::blob_store::has_blob(state_dir.path(), &floor_blob.digest)
            .await
            .unwrap()
    );
    assert!(
        remerge_server::blob_store::has_blob(state_dir.path(), &expired_blob.digest)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn cleanup_snapshot_storage_keeps_recently_unreferenced_entries_for_grace_window() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 0,
        ..Default::default()
    };

    let blob = remerge_server::blob_store::store_blob(state_dir.path(), b"recently-unreferenced")
        .await
        .expect("store blob");
    let recent_at = chrono::Utc::now() - chrono::Duration::hours(12);
    remerge_server::blob_store::touch_blob(state_dir.path(), &blob.digest, recent_at)
        .await
        .expect("touch recent blob");

    let summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("cleanup snapshot storage");

    assert_eq!(summary.deleted_blobs, 0);
    assert!(
        remerge_server::blob_store::has_blob(state_dir.path(), &blob.digest)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn cleanup_snapshot_storage_keeps_hard_delete_eligible_entries_below_floor() {
    let state_dir = tempfile::TempDir::new().unwrap();
    let config = remerge_server::config::ServerConfig {
        snapshot_cache_grace_period_hours: 7 * 24,
        snapshot_cache_hard_delete_hours: 30 * 24,
        snapshot_min_retained_bytes: 1_000_000,
        ..Default::default()
    };

    let expired_blob = remerge_server::blob_store::store_blob(state_dir.path(), b"expired-blob")
        .await
        .expect("store expired blob");
    let expired_at = chrono::Utc::now() - chrono::Duration::days(31);
    remerge_server::blob_store::touch_blob(state_dir.path(), &expired_blob.digest, expired_at)
        .await
        .expect("touch expired blob");

    let summary = remerge_server::runtime::cleanup_snapshot_storage_at(
        state_dir.path(),
        &config,
        &[],
        chrono::Utc::now(),
    )
    .await
    .expect("cleanup snapshot storage");

    assert_eq!(summary.deleted_blobs, 0);
    assert!(
        remerge_server::blob_store::has_blob(state_dir.path(), &expired_blob.digest)
            .await
            .unwrap()
    );
}
