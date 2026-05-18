use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use remerge_types::compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::blob_store::{write_bytes_atomically, write_bytes_with_atomic_replace};

pub const TREES_SUBDIR: &str = "trees";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeManifest {
    pub entries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeMetadata {
    pub canonical_digest: String,
    pub raw_size_bytes: u64,
    #[serde(default = "default_last_referenced_at")]
    pub last_referenced_at: DateTime<Utc>,
    #[serde(default)]
    pub encoded_variants: BTreeMap<crate::blob_store::BlobEncoding, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMetadataRecord {
    pub digest: String,
    pub metadata: TreeMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTree {
    pub digest: String,
    pub path: PathBuf,
}

pub async fn store_tree(
    state_dir: &Path,
    entries: &BTreeMap<String, String>,
) -> Result<StoredTree> {
    let manifest = TreeManifest {
        entries: entries.clone(),
    };
    let bytes = serde_json::to_vec(&manifest)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    let path = tree_path(state_dir, &digest)?;
    let raw_size_bytes = bytes.len() as u64;

    if tokio::fs::metadata(&path).await.is_ok() {
        ensure_tree_metadata(state_dir, &digest, raw_size_bytes).await?;
        maybe_store_zstd_variant(state_dir, &digest, &bytes).await?;
        touch_tree(state_dir, &digest, Utc::now()).await?;
        return Ok(StoredTree { digest, path });
    }

    let parent = path
        .parent()
        .context("tree path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    let file_name = format!("{digest}.json");
    write_bytes_atomically(parent, &path, &file_name, &bytes).await?;
    ensure_tree_metadata(state_dir, &digest, raw_size_bytes).await?;
    maybe_store_zstd_variant(state_dir, &digest, &bytes).await?;
    touch_tree(state_dir, &digest, Utc::now()).await?;

    Ok(StoredTree { digest, path })
}

pub async fn touch_tree(
    state_dir: &Path,
    digest: &str,
    last_referenced_at: DateTime<Utc>,
) -> Result<TreeMetadata> {
    validate_digest(digest)?;
    let path = tree_path(state_dir, digest)?;
    let raw_size_bytes = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("Failed to stat {}", path.display()))?
        .len();
    let mut metadata = ensure_tree_metadata(state_dir, digest, raw_size_bytes).await?;
    metadata.last_referenced_at = last_referenced_at;
    persist_tree_metadata(state_dir, digest, &metadata).await?;
    Ok(metadata)
}

pub async fn list_tree_metadata(state_dir: &Path) -> Result<Vec<TreeMetadataRecord>> {
    let tree_root = state_dir.join(TREES_SUBDIR);
    let mut records = Vec::new();
    let mut entries = match tokio::fs::read_dir(&tree_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", tree_root.display()));
        }
    };

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.ends_with(".meta.json") {
            continue;
        }

        let digest = name[..name.len() - ".meta.json".len()].to_string();
        let metadata = load_tree_metadata(state_dir, &digest).await?;
        records.push(TreeMetadataRecord { digest, metadata });
    }

    Ok(records)
}

pub async fn delete_tree(state_dir: &Path, digest: &str) -> Result<()> {
    let metadata = load_tree_metadata(state_dir, digest).await.ok();
    let raw_path = tree_path(state_dir, digest)?;
    remove_file_if_exists(&raw_path).await?;

    if let Some(metadata) = metadata {
        for encoding in metadata.encoded_variants.keys() {
            let encoded_path = encoded_tree_path(state_dir, digest, *encoding)?;
            remove_file_if_exists(&encoded_path).await?;
        }
    }

    let metadata_path = tree_metadata_path(state_dir, digest)?;
    remove_file_if_exists(&metadata_path).await
}

pub fn tree_path(state_dir: &Path, digest: &str) -> Result<PathBuf> {
    validate_digest(digest)?;
    Ok(state_dir.join(TREES_SUBDIR).join(format!("{digest}.json")))
}

pub fn encoded_tree_path(
    state_dir: &Path,
    digest: &str,
    encoding: crate::blob_store::BlobEncoding,
) -> Result<PathBuf> {
    validate_digest(digest)?;
    Ok(state_dir.join(TREES_SUBDIR).join(format!(
        "{digest}.{}",
        match encoding {
            crate::blob_store::BlobEncoding::Zstd => "zst",
        }
    )))
}

pub fn tree_metadata_path(state_dir: &Path, digest: &str) -> Result<PathBuf> {
    validate_digest(digest)?;
    Ok(state_dir
        .join(TREES_SUBDIR)
        .join(format!("{digest}.meta.json")))
}

pub async fn load_tree_metadata(state_dir: &Path, digest: &str) -> Result<TreeMetadata> {
    let path = tree_metadata_path(state_dir, digest)?;
    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("Failed to parse {}", path.display()))
}

async fn ensure_tree_metadata(
    state_dir: &Path,
    digest: &str,
    raw_size_bytes: u64,
) -> Result<TreeMetadata> {
    match load_tree_metadata(state_dir, digest).await {
        Ok(metadata) => Ok(metadata),
        Err(error) if is_not_found_error(&error) => {
            let metadata = TreeMetadata {
                canonical_digest: digest.to_string(),
                raw_size_bytes,
                last_referenced_at: Utc::now(),
                encoded_variants: BTreeMap::new(),
            };
            persist_tree_metadata(state_dir, digest, &metadata).await?;
            Ok(metadata)
        }
        Err(error) => Err(error),
    }
}

async fn persist_tree_metadata(
    state_dir: &Path,
    digest: &str,
    metadata: &TreeMetadata,
) -> Result<()> {
    let path = tree_metadata_path(state_dir, digest)?;
    let parent = path
        .parent()
        .context("tree metadata path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    let bytes = serde_json::to_vec(metadata)?;
    write_bytes_with_atomic_replace(parent, &path, &format!("{digest}.meta.json"), &bytes)
        .await
        .map(|_| ())
}

async fn maybe_store_zstd_variant(state_dir: &Path, digest: &str, bytes: &[u8]) -> Result<()> {
    let raw_size_bytes = bytes.len() as u64;
    let metadata = ensure_tree_metadata(state_dir, digest, raw_size_bytes).await?;
    let encoded_path = encoded_tree_path(state_dir, digest, crate::blob_store::BlobEncoding::Zstd)?;
    if metadata
        .encoded_variants
        .contains_key(&crate::blob_store::BlobEncoding::Zstd)
        && tokio::fs::metadata(&encoded_path).await.is_ok()
    {
        return Ok(());
    }

    let input = bytes.to_vec();
    let Some(compressed) =
        tokio::task::spawn_blocking(move || compression::encode_zstd_if_worthwhile(&input))
            .await
            .context("tree zstd compression task failed to join")??
    else {
        return Ok(());
    };

    let parent = encoded_path
        .parent()
        .context("encoded tree path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    tokio::fs::write(&encoded_path, &compressed)
        .await
        .with_context(|| format!("Failed to write {}", encoded_path.display()))?;

    let mut metadata = metadata;
    metadata.encoded_variants.insert(
        crate::blob_store::BlobEncoding::Zstd,
        compressed.len() as u64,
    );
    persist_tree_metadata(state_dir, digest, &metadata).await
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|inner| inner.kind() == std::io::ErrorKind::NotFound)
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid sha256 digest '{digest}'");
    }
    Ok(())
}

async fn remove_file_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

fn default_last_referenced_at() -> DateTime<Utc> {
    Utc::now()
}
