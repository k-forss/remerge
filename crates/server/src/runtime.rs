use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tracing::{debug, info};

use crate::blob_store;
use crate::config::ServerConfig;
use crate::tree_store;
use remerge_types::portage::{
    PortageConfig, RepoSnapshotManifest, SnapshotEntry, SnapshotManifest,
    SNAPSHOT_MANIFEST_VERSION_V1,
};
use remerge_types::workorder::{ParityManifest, Workorder, WorkorderId};

pub const RUNTIME_SUBDIR: &str = "runtime";

#[derive(Debug, Clone)]
pub struct StagedWorkorderRuntime {
    pub runtime_dir: PathBuf,
    pub workorder_json_path: PathBuf,
    pub snapshot_references: SnapshotReferenceSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReferenceSet {
    pub blob_digests: BTreeSet<String>,
    pub tree_digests: BTreeSet<String>,
    pub total_blob_bytes: u64,
    pub total_tree_bytes: u64,
    pub last_referenced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotCleanupSummary {
    pub deleted_blobs: usize,
    pub deleted_trees: usize,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone)]
enum EntryKind {
    Blob,
    Tree,
}

#[derive(Debug, Clone)]
struct CleanupEntry {
    kind: EntryKind,
    digest: String,
    last_referenced_at: DateTime<Utc>,
    retained_bytes: u64,
}

pub async fn collect_snapshot_references(
    state_dir: &Path,
    portage_config: &PortageConfig,
) -> Result<SnapshotReferenceSet> {
    let manifest = effective_snapshot_manifest(state_dir, portage_config).await?;
    let blob_digests: BTreeSet<String> = manifest
        .repo_snapshots
        .values()
        .flat_map(|repo| repo.entries.values().map(|entry| entry.digest.clone()))
        .chain(manifest.distfiles.values().map(|entry| entry.digest.clone()))
        .collect();
    let tree_digests: BTreeSet<String> = manifest
        .repo_snapshots
        .values()
        .map(|repo| repo.tree_digest.clone())
        .filter(|digest| !digest.is_empty())
        .collect();

    let mut total_blob_bytes = 0;
    for digest in &blob_digests {
        let path = blob_store::blob_path(state_dir, digest)?;
        let metadata = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("Failed to stat {}", path.display()))?;
        total_blob_bytes += metadata.len();
    }

    let mut total_tree_bytes = 0;
    for digest in &tree_digests {
        let path = tree_store::tree_path(state_dir, digest)?;
        let metadata = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("Failed to stat {}", path.display()))?;
        total_tree_bytes += metadata.len();
    }

    let references = SnapshotReferenceSet {
        blob_digests,
        tree_digests,
        total_blob_bytes,
        total_tree_bytes,
        last_referenced_at: Utc::now(),
    };
    debug!(
        blob_count = references.blob_digests.len(),
        tree_count = references.tree_digests.len(),
        total_blob_bytes = references.total_blob_bytes,
        total_tree_bytes = references.total_tree_bytes,
        "Collected snapshot references"
    );
    Ok(references)
}

pub async fn normalize_portage_config_snapshots(
    state_dir: &Path,
    portage_config: &PortageConfig,
) -> Result<PortageConfig> {
    let mut normalized = portage_config.clone();
    let manifest = effective_snapshot_manifest(state_dir, portage_config).await?;
    let mut repo_snapshot_refs = std::collections::BTreeMap::new();
    let mut repo_snapshot_trees = std::collections::BTreeMap::new();
    let mut distfile_snapshot_refs = std::collections::BTreeMap::new();

    for (repo_name, repo_manifest) in &manifest.repo_snapshots {
        let snapshot_refs = repo_manifest
            .entries
            .iter()
            .map(|(relative_path, entry)| (relative_path.clone(), entry.digest.clone()))
            .collect();
        repo_snapshot_refs.insert(repo_name.clone(), snapshot_refs);
        repo_snapshot_trees.insert(repo_name.clone(), repo_manifest.tree_digest.clone());
    }

    for (name, entry) in &manifest.distfiles {
        distfile_snapshot_refs.insert(name.clone(), entry.digest.clone());
    }

    normalized.repo_snapshots.clear();
    normalized.distfile_snapshots.clear();
    normalized.snapshot_manifest = manifest;
    normalized.repo_snapshot_refs = repo_snapshot_refs;
    normalized.repo_snapshot_trees = repo_snapshot_trees;
    normalized.distfile_snapshot_refs = distfile_snapshot_refs;

    debug!(
        repo_count = normalized.snapshot_manifest.repo_snapshots.len(),
        distfile_count = normalized.snapshot_manifest.distfiles.len(),
        "Normalized portage snapshot payloads into manifest-backed refs"
    );

    Ok(normalized)
}

pub async fn stage_workorder_runtime(
    state_dir: &Path,
    workorder: &Workorder,
) -> Result<StagedWorkorderRuntime> {
    let runtime_dir = runtime_dir(state_dir, workorder.id);
    if tokio::fs::metadata(&runtime_dir).await.is_ok() {
        tokio::fs::remove_dir_all(&runtime_dir)
            .await
            .with_context(|| format!("Failed to clear {}", runtime_dir.display()))?;
    }

    let repos_dir = runtime_dir.join("snapshots/repos");
    let distfiles_dir = runtime_dir.join("snapshots/distfiles");
    let parity_dir = runtime_dir.join("parity");
    tokio::fs::create_dir_all(&repos_dir).await?;
    tokio::fs::create_dir_all(&distfiles_dir).await?;
    tokio::fs::create_dir_all(parity_dir.join("blobs")).await?;

    let manifest = effective_snapshot_manifest(state_dir, &workorder.portage_config).await?;

    let mut repo_snapshot_refs = std::collections::BTreeMap::new();
    let mut repo_snapshot_trees = std::collections::BTreeMap::new();
    let mut distfile_snapshot_refs = std::collections::BTreeMap::new();

    for (repo_name, repo_manifest) in &manifest.repo_snapshots {
        let repo_base = repos_dir.join(sanitize_file_name(&repo_name)?);
        let snapshot_refs = repo_manifest
            .entries
            .iter()
            .map(|(relative_path, entry)| (relative_path.clone(), entry.digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (relative_path, entry) in &repo_manifest.entries {
            let safe_relative = sanitize_relative_path(relative_path)?;
            let target = repo_base.join(&safe_relative);
            materialize_blob_for_digest(state_dir, &target, &entry.digest).await?;
        }

        if snapshot_refs.is_empty() {
            continue;
        }

        let tree = tree_store::store_tree(state_dir, &snapshot_refs).await?;
        if !repo_manifest.tree_digest.is_empty() && tree.digest != repo_manifest.tree_digest {
            anyhow::bail!("repo snapshot tree digest mismatch for '{repo_name}'");
        }
        repo_snapshot_refs.insert(repo_name.clone(), snapshot_refs);
        repo_snapshot_trees.insert(repo_name.clone(), tree.digest);
    }

    for (name, entry) in &manifest.distfiles {
        let target = distfiles_dir.join(sanitize_file_name(&name)?);
        let digest = materialize_blob_for_digest(state_dir, &target, &entry.digest).await?;
        distfile_snapshot_refs.insert(name.clone(), digest);
    }

    let mut staged_workorder = workorder.clone();
    staged_workorder.portage_config.repo_snapshots.clear();
    staged_workorder.portage_config.distfile_snapshots.clear();
    staged_workorder.portage_config.snapshot_manifest = manifest;
    staged_workorder.portage_config.repo_snapshot_refs = repo_snapshot_refs;
    staged_workorder.portage_config.repo_snapshot_trees = repo_snapshot_trees;
    staged_workorder.portage_config.distfile_snapshot_refs = distfile_snapshot_refs;

    let workorder_json_path = runtime_dir.join("workorder.json");
    tokio::fs::write(&workorder_json_path, serde_json::to_vec(&staged_workorder)?)
        .await
        .with_context(|| format!("Failed to write {}", workorder_json_path.display()))?;
    let snapshot_references =
        collect_snapshot_references(state_dir, &staged_workorder.portage_config).await?;
    touch_snapshot_references(state_dir, &snapshot_references).await?;

    info!(
        workorder_id = %workorder.id,
        runtime_dir = %runtime_dir.display(),
        repo_count = staged_workorder.portage_config.snapshot_manifest.repo_snapshots.len(),
        distfile_count = staged_workorder.portage_config.snapshot_manifest.distfiles.len(),
        blob_count = snapshot_references.blob_digests.len(),
        tree_count = snapshot_references.tree_digests.len(),
        total_blob_bytes = snapshot_references.total_blob_bytes,
        total_tree_bytes = snapshot_references.total_tree_bytes,
        "Staged workorder runtime from snapshot store"
    );

    Ok(StagedWorkorderRuntime {
        runtime_dir,
        workorder_json_path,
        snapshot_references,
    })
}

pub async fn touch_snapshot_references(
    state_dir: &Path,
    references: &SnapshotReferenceSet,
) -> Result<()> {
    for digest in &references.blob_digests {
        blob_store::touch_blob(state_dir, digest, references.last_referenced_at).await?;
    }
    for digest in &references.tree_digests {
        tree_store::touch_tree(state_dir, digest, references.last_referenced_at).await?;
    }
    Ok(())
}

pub async fn cleanup_snapshot_storage(
    state_dir: &Path,
    config: &ServerConfig,
    active_references: &[SnapshotReferenceSet],
) -> Result<SnapshotCleanupSummary> {
    cleanup_snapshot_storage_at(state_dir, config, active_references, Utc::now()).await
}

pub async fn cleanup_snapshot_storage_at(
    state_dir: &Path,
    config: &ServerConfig,
    active_references: &[SnapshotReferenceSet],
    now: DateTime<Utc>,
) -> Result<SnapshotCleanupSummary> {
    let active_blob_digests: BTreeSet<String> = active_references
        .iter()
        .flat_map(|references| references.blob_digests.iter().cloned())
        .collect();
    let active_tree_digests: BTreeSet<String> = active_references
        .iter()
        .flat_map(|references| references.tree_digests.iter().cloned())
        .collect();
    let grace_cutoff =
        now - chrono::Duration::hours(config.snapshot_cache_grace_period_hours as i64);
    let hard_delete_cutoff =
        now - chrono::Duration::hours(config.snapshot_cache_hard_delete_hours as i64);

    let mut total_retained_bytes = 0u64;
    let mut floor_eligible = Vec::new();
    let mut hard_delete = Vec::new();

    for record in blob_store::list_blob_metadata(state_dir).await? {
        let retained_bytes = record.metadata.raw_size_bytes
            + record
                .metadata
                .encoded_variants
                .values()
                .copied()
                .sum::<u64>();
        total_retained_bytes = total_retained_bytes.saturating_add(retained_bytes);
        if active_blob_digests.contains(&record.digest) {
            continue;
        }

        let entry = CleanupEntry {
            kind: EntryKind::Blob,
            digest: record.digest,
            last_referenced_at: record.metadata.last_referenced_at,
            retained_bytes,
        };
        if entry.last_referenced_at <= hard_delete_cutoff {
            hard_delete.push(entry);
        } else if entry.last_referenced_at <= grace_cutoff {
            floor_eligible.push(entry);
        }
    }

    for record in tree_store::list_tree_metadata(state_dir).await? {
        let retained_bytes = record.metadata.raw_size_bytes
            + record
                .metadata
                .encoded_variants
                .values()
                .copied()
                .sum::<u64>();
        total_retained_bytes = total_retained_bytes.saturating_add(retained_bytes);
        if active_tree_digests.contains(&record.digest) {
            continue;
        }

        let entry = CleanupEntry {
            kind: EntryKind::Tree,
            digest: record.digest,
            last_referenced_at: record.metadata.last_referenced_at,
            retained_bytes,
        };
        if entry.last_referenced_at <= hard_delete_cutoff {
            hard_delete.push(entry);
        } else if entry.last_referenced_at <= grace_cutoff {
            floor_eligible.push(entry);
        }
    }

    if total_retained_bytes <= config.snapshot_min_retained_bytes {
        // The retained-size floor applies to both grace-tier and hard-delete-tier
        // entries. Once the cache is at or below the floor, cleanup stops.
        debug!(
            total_retained_bytes,
            retained_size_floor_bytes = config.snapshot_min_retained_bytes,
            "Skipping snapshot cleanup because retained data is already at or below the floor"
        );
        return Ok(SnapshotCleanupSummary::default());
    }

    let mut summary = SnapshotCleanupSummary::default();
    for entry in &hard_delete {
        if total_retained_bytes <= config.snapshot_min_retained_bytes {
            break;
        }
        delete_cleanup_entry(state_dir, entry, &mut summary).await?;
        total_retained_bytes = total_retained_bytes.saturating_sub(entry.retained_bytes);
    }

    floor_eligible.sort_by_key(|entry| entry.last_referenced_at);
    for entry in floor_eligible {
        if total_retained_bytes <= config.snapshot_min_retained_bytes {
            break;
        }
        delete_cleanup_entry(state_dir, &entry, &mut summary).await?;
        total_retained_bytes = total_retained_bytes.saturating_sub(entry.retained_bytes);
    }

    if summary.deleted_blobs != 0 || summary.deleted_trees != 0 {
        info!(
            deleted_blobs = summary.deleted_blobs,
            deleted_trees = summary.deleted_trees,
            reclaimed_bytes = summary.reclaimed_bytes,
            remaining_retained_bytes = total_retained_bytes,
            retained_size_floor_bytes = config.snapshot_min_retained_bytes,
            "Completed snapshot cache cleanup pass"
        );
    } else {
        debug!(
            total_retained_bytes,
            retained_size_floor_bytes = config.snapshot_min_retained_bytes,
            "Snapshot cleanup pass found no deletable entries"
        );
    }

    Ok(summary)
}

pub fn parity_runtime_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("parity")
}

pub fn distfile_output_manifest_path(runtime_dir: &Path) -> PathBuf {
    parity_runtime_dir(runtime_dir).join("distfiles.json")
}

pub fn distfile_output_blobs_dir(runtime_dir: &Path) -> PathBuf {
    parity_runtime_dir(runtime_dir).join("blobs")
}

pub async fn ingest_final_state_parity(
    state_dir: &Path,
    runtime_dir: &Path,
) -> Result<ParityManifest> {
    let parity_dir = parity_runtime_dir(runtime_dir);
    let manifest_path = parity_dir.join("manifest.json");
    let manifest_bytes = match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ParityManifest::default());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read {}", manifest_path.display()));
        }
    };
    let manifest: ParityManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("Failed to parse {}", manifest_path.display()))?;

    for entry in manifest.files.values() {
        let blob_path = parity_dir.join("blobs").join(&entry.digest);
        let bytes = tokio::fs::read(&blob_path)
            .await
            .with_context(|| format!("Failed to read {}", blob_path.display()))?;
        blob_store::store_blob_for_digest(state_dir, &entry.digest, &bytes).await?;
    }

    info!(
        runtime_dir = %runtime_dir.display(),
        file_count = manifest.files.len(),
        directory_count = manifest.directories.len(),
        symlink_count = manifest.symlinks.len(),
        "Ingested final-state parity manifest into snapshot store"
    );

    Ok(manifest)
}

pub async fn ingest_fetched_distfiles(
    state_dir: &Path,
    runtime_dir: &Path,
) -> Result<std::collections::BTreeMap<String, SnapshotEntry>> {
    let manifest_path = distfile_output_manifest_path(runtime_dir);
    let manifest_bytes = match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(std::collections::BTreeMap::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read {}", manifest_path.display()));
        }
    };
    let manifest: std::collections::BTreeMap<String, SnapshotEntry> =
        serde_json::from_slice(&manifest_bytes)
            .with_context(|| format!("Failed to parse {}", manifest_path.display()))?;

    let blobs_dir = distfile_output_blobs_dir(runtime_dir);
    for entry in manifest.values() {
        let blob_path = blobs_dir.join(&entry.digest);
        let bytes = tokio::fs::read(&blob_path)
            .await
            .with_context(|| format!("Failed to read {}", blob_path.display()))?;
        blob_store::store_blob_for_digest(state_dir, &entry.digest, &bytes).await?;
    }

    info!(
        runtime_dir = %runtime_dir.display(),
        distfile_count = manifest.len(),
        "Ingested fetched distfile manifest into snapshot store"
    );

    Ok(manifest)
}

pub async fn cleanup_workorder_runtime(state_dir: &Path, id: WorkorderId) -> Result<()> {
    let runtime_dir = runtime_dir(state_dir, id);
    match tokio::fs::remove_dir_all(&runtime_dir).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to remove {}", runtime_dir.display()))
        }
    }
}

fn runtime_dir(state_dir: &Path, id: WorkorderId) -> PathBuf {
    state_dir.join(RUNTIME_SUBDIR).join(id.to_string())
}

fn sanitize_file_name(name: &str) -> Result<&str> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("invalid file name '{name}'");
    }
    Ok(name)
}

fn sanitize_relative_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        anyhow::bail!("invalid relative path '{path:?}'");
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => safe.push(segment),
            _ => anyhow::bail!("invalid relative path '{path:?}'"),
        }
    }
    Ok(safe)
}

async fn effective_snapshot_manifest(
    state_dir: &Path,
    portage_config: &PortageConfig,
) -> Result<SnapshotManifest> {
    if portage_config.snapshot_manifest.version != SNAPSHOT_MANIFEST_VERSION_V1 {
        anyhow::bail!(
            "unsupported snapshot manifest version {}",
            portage_config.snapshot_manifest.version
        );
    }

    let mut manifest = portage_config.snapshot_manifest.clone();

    let repo_names: BTreeSet<String> = portage_config
        .repo_snapshots
        .keys()
        .chain(portage_config.repo_snapshot_refs.keys())
        .chain(manifest.repo_snapshots.keys())
        .cloned()
        .collect();
    for repo_name in repo_names {
        let repo_manifest = build_repo_snapshot_manifest(state_dir, portage_config, &manifest, &repo_name).await?;
        if !repo_manifest.entries.is_empty() {
            manifest.repo_snapshots.insert(repo_name, repo_manifest);
        }
    }

    let distfile_names: BTreeSet<String> = portage_config
        .distfile_snapshots
        .keys()
        .chain(portage_config.distfile_snapshot_refs.keys())
        .chain(manifest.distfiles.keys())
        .cloned()
        .collect();
    for name in distfile_names {
        let entry = build_distfile_snapshot_entry(state_dir, portage_config, &manifest, &name).await?;
        if let Some(entry) = entry {
            manifest.distfiles.insert(name, entry);
        }
    }

    Ok(manifest)
}

async fn build_repo_snapshot_manifest(
    state_dir: &Path,
    portage_config: &PortageConfig,
    manifest: &SnapshotManifest,
    repo_name: &str,
) -> Result<RepoSnapshotManifest> {
    if let Some(snapshot) = portage_config.repo_snapshots.get(repo_name) {
        let mut entries = std::collections::BTreeMap::new();
        let existing_manifest = manifest.repo_snapshots.get(repo_name);
        for (relative_path, content) in snapshot {
            let digest = blob_store::store_blob(state_dir, content.as_bytes()).await?.digest;
            let existing_entry = existing_manifest.and_then(|repo| repo.entries.get(relative_path));
            if let Some(expected_digest) = portage_config
                .repo_snapshot_refs
                .get(repo_name)
                .and_then(|refs| refs.get(relative_path))
                && &digest != expected_digest
            {
                anyhow::bail!("repo snapshot refs for '{repo_name}' do not match inline payloads");
            }
            entries.insert(
                relative_path.clone(),
                SnapshotEntry {
                    digest,
                    size: content.len() as u64,
                    mtime_secs: existing_entry.map(|entry| entry.mtime_secs).unwrap_or(0),
                },
            );
        }

        let snapshot_refs = entries
            .iter()
            .map(|(relative_path, entry)| (relative_path.clone(), entry.digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let tree = tree_store::store_tree(state_dir, &snapshot_refs).await?;
        let expected_tree = existing_manifest
            .map(|repo| repo.tree_digest.as_str())
            .filter(|digest| !digest.is_empty())
            .or_else(|| {
                portage_config
                    .repo_snapshot_trees
                    .get(repo_name)
                    .map(String::as_str)
                    .filter(|digest| !digest.is_empty())
            });
        if let Some(expected_tree) = expected_tree && tree.digest != expected_tree {
            anyhow::bail!("repo snapshot tree digest mismatch for '{repo_name}'");
        }

        return Ok(RepoSnapshotManifest {
            tree_digest: tree.digest,
            entries,
        });
    }

    if let Some(existing_manifest) = manifest.repo_snapshots.get(repo_name) {
        for entry in existing_manifest.entries.values() {
            ensure_blob_exists(state_dir, &entry.digest).await?;
        }
        let snapshot_refs = existing_manifest
            .entries
            .iter()
            .map(|(relative_path, entry)| (relative_path.clone(), entry.digest.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let tree = tree_store::store_tree(state_dir, &snapshot_refs).await?;
        if !existing_manifest.tree_digest.is_empty() && tree.digest != existing_manifest.tree_digest {
            anyhow::bail!("repo snapshot tree digest mismatch for '{repo_name}'");
        }
        return Ok(RepoSnapshotManifest {
            tree_digest: tree.digest,
            entries: existing_manifest.entries.clone(),
        });
    }

    if let Some(existing_refs) = portage_config.repo_snapshot_refs.get(repo_name) {
        let mut entries = std::collections::BTreeMap::new();
        for (relative_path, digest) in existing_refs {
            let metadata = blob_store::load_blob_metadata(state_dir, digest).await?;
            entries.insert(
                relative_path.clone(),
                SnapshotEntry {
                    digest: digest.clone(),
                    size: metadata.raw_size_bytes,
                    mtime_secs: 0,
                },
            );
        }
        let tree = tree_store::store_tree(state_dir, existing_refs).await?;
        if let Some(expected_tree) = portage_config
            .repo_snapshot_trees
            .get(repo_name)
            .filter(|digest| !digest.is_empty())
            && &tree.digest != expected_tree
        {
            anyhow::bail!("repo snapshot tree digest mismatch for '{repo_name}'");
        }
        return Ok(RepoSnapshotManifest {
            tree_digest: tree.digest,
            entries,
        });
    }

    Ok(RepoSnapshotManifest::default())
}

async fn build_distfile_snapshot_entry(
    state_dir: &Path,
    portage_config: &PortageConfig,
    manifest: &SnapshotManifest,
    name: &str,
) -> Result<Option<SnapshotEntry>> {
    if let Some(bytes) = portage_config.distfile_snapshots.get(name) {
        let digest = blob_store::store_blob(state_dir, bytes).await?.digest;
        if let Some(expected_digest) = portage_config.distfile_snapshot_refs.get(name)
            && &digest != expected_digest
        {
            anyhow::bail!("distfile snapshot digest mismatch for '{name}'");
        }
        return Ok(Some(SnapshotEntry {
            digest,
            size: bytes.len() as u64,
            mtime_secs: manifest
                .distfiles
                .get(name)
                .map(|entry| entry.mtime_secs)
                .unwrap_or(0),
        }));
    }

    if let Some(entry) = manifest.distfiles.get(name) {
        ensure_blob_exists(state_dir, &entry.digest).await?;
        return Ok(Some(entry.clone()));
    }

    if let Some(digest) = portage_config.distfile_snapshot_refs.get(name) {
        let metadata = blob_store::load_blob_metadata(state_dir, digest).await?;
        return Ok(Some(SnapshotEntry {
            digest: digest.clone(),
            size: metadata.raw_size_bytes,
            mtime_secs: 0,
        }));
    }

    Ok(None)
}

async fn ensure_blob_exists(state_dir: &Path, digest: &str) -> Result<()> {
    let source = blob_store::blob_path(state_dir, digest)?;
    tokio::fs::metadata(&source).await.with_context(|| {
        format!("Missing referenced blob {} at {}", digest, source.display())
    })?;
    Ok(())
}

async fn materialize_blob_for_digest(
    state_dir: &Path,
    target: &Path,
    digest: &str,
) -> Result<String> {
    let source = blob_store::blob_path(state_dir, digest)?;
    tokio::fs::metadata(&source)
        .await
        .with_context(|| format!("Missing referenced blob {} at {}", digest, source.display()))?;
    link_blob_into_target(&source, target).await?;
    Ok(digest.to_string())
}

async fn link_blob_into_target(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    if try_reflink(source, target).await? {
        return Ok(());
    }

    match tokio::fs::hard_link(source, target).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::CrossesDevices
            ) =>
        {
            tokio::fs::copy(source, target).await.with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
            Ok(())
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to link {} into {}",
                source.display(),
                target.display()
            )
        }),
    }
}

async fn try_reflink(source: &Path, target: &Path) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        let source = source.to_path_buf();
        let target = target.to_path_buf();
        return tokio::task::spawn_blocking(move || try_reflink_blocking(&source, &target))
            .await
            .context("reflink task failed to join")?;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, target);
        Ok(false)
    }
}

#[cfg(target_os = "linux")]
fn try_reflink_blocking(source: &Path, target: &Path) -> Result<bool> {
    use std::fs::OpenOptions;
    use std::io::ErrorKind;
    use std::os::fd::AsRawFd;

    const FICLONE: libc::c_ulong = 0x4004_9409;

    let source_file = std::fs::File::open(source)
        .with_context(|| format!("Failed to open {}", source.display()))?;
    let target_file = match OpenOptions::new().write(true).create_new(true).open(target) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to create {}", target.display()));
        }
    };

    let result = unsafe { libc::ioctl(target_file.as_raw_fd(), FICLONE, source_file.as_raw_fd()) };
    if result == 0 {
        return Ok(true);
    }

    let error = std::io::Error::last_os_error();
    let _ = std::fs::remove_file(target);
    if matches!(
        error.raw_os_error(),
        Some(libc::EOPNOTSUPP)
            | Some(libc::EXDEV)
            | Some(libc::EINVAL)
            | Some(libc::ENOTTY)
            | Some(libc::ENOSYS)
            | Some(libc::EPERM)
    ) {
        return Ok(false);
    }

    Err(error).with_context(|| {
        format!(
            "Failed to reflink {} into {}",
            source.display(),
            target.display()
        )
    })
}

async fn delete_cleanup_entry(
    state_dir: &Path,
    entry: &impl CleanupEntryView,
    summary: &mut SnapshotCleanupSummary,
) -> Result<()> {
    match entry.kind() {
        CleanupEntryKind::Blob => {
            blob_store::delete_blob(state_dir, entry.digest()).await?;
            summary.deleted_blobs += 1;
        }
        CleanupEntryKind::Tree => {
            tree_store::delete_tree(state_dir, entry.digest()).await?;
            summary.deleted_trees += 1;
        }
    }
    summary.reclaimed_bytes = summary
        .reclaimed_bytes
        .saturating_add(entry.retained_bytes());
    Ok(())
}

enum CleanupEntryKind {
    Blob,
    Tree,
}

trait CleanupEntryView {
    fn kind(&self) -> CleanupEntryKind;
    fn digest(&self) -> &str;
    fn retained_bytes(&self) -> u64;
}

impl CleanupEntryView for CleanupEntry {
    fn kind(&self) -> CleanupEntryKind {
        match self.kind {
            EntryKind::Blob => CleanupEntryKind::Blob,
            EntryKind::Tree => CleanupEntryKind::Tree,
        }
    }

    fn digest(&self) -> &str {
        &self.digest
    }

    fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }
}
