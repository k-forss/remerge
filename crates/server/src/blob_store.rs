use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use remerge_types::compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BLOBS_SUBDIR: &str = "blobs";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobEncoding {
    Zstd,
}

impl BlobEncoding {
    fn file_extension(self) -> &'static str {
        match self {
            Self::Zstd => "zst",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobMetadata {
    pub canonical_digest: String,
    pub raw_size_bytes: u64,
    #[serde(default = "default_last_referenced_at")]
    pub last_referenced_at: DateTime<Utc>,
    #[serde(default)]
    pub encoded_variants: BTreeMap<BlobEncoding, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMetadataRecord {
    pub digest: String,
    pub metadata: BlobMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlob {
    pub digest: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBlobStoreResult {
    pub blob: StoredBlob,
    pub uploaded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEncodedBlobVariant {
    pub digest: String,
    pub encoding: BlobEncoding,
    pub path: PathBuf,
    pub encoded_size_bytes: u64,
}

pub async fn store_blob(state_dir: &Path, bytes: &[u8]) -> Result<StoredBlob> {
    let digest = hex::encode(Sha256::digest(bytes));
    store_blob_for_digest(state_dir, &digest, bytes)
        .await
        .map(|result| result.blob)
}

pub async fn store_blob_for_digest(
    state_dir: &Path,
    digest: &str,
    bytes: &[u8],
) -> Result<VerifiedBlobStoreResult> {
    validate_digest(digest)?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual != digest {
        anyhow::bail!("blob digest mismatch: expected {digest}, got {actual}");
    }

    let target = blob_path(state_dir, &digest)?;
    let raw_size_bytes = bytes.len() as u64;

    if tokio::fs::metadata(&target).await.is_ok() {
        ensure_blob_metadata(state_dir, digest, raw_size_bytes).await?;
        maybe_store_zstd_variant(state_dir, digest, bytes).await?;
        touch_blob(state_dir, digest, Utc::now()).await?;
        return Ok(VerifiedBlobStoreResult {
            blob: StoredBlob {
                digest: digest.to_string(),
                path: target,
            },
            uploaded: false,
        });
    }

    let parent = target
        .parent()
        .context("blob path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;

    let uploaded = write_bytes_atomically(parent, &target, file_name(&digest)?, bytes).await?;
    ensure_blob_metadata(state_dir, digest, raw_size_bytes).await?;
    maybe_store_zstd_variant(state_dir, digest, bytes).await?;
    touch_blob(state_dir, digest, Utc::now()).await?;

    Ok(VerifiedBlobStoreResult {
        blob: StoredBlob {
            digest: digest.to_string(),
            path: target,
        },
        uploaded,
    })
}

pub async fn load_blob_metadata(state_dir: &Path, digest: &str) -> Result<BlobMetadata> {
    let path = metadata_path(state_dir, digest)?;
    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("Failed to parse {}", path.display()))
}

pub async fn store_encoded_blob_variant(
    state_dir: &Path,
    digest: &str,
    raw_size_bytes: u64,
    encoding: BlobEncoding,
    bytes: &[u8],
) -> Result<StoredEncodedBlobVariant> {
    let metadata = ensure_blob_metadata(state_dir, digest, raw_size_bytes).await?;
    if metadata.raw_size_bytes != raw_size_bytes {
        anyhow::bail!(
            "blob raw size mismatch for {digest}: expected {}, got {}",
            metadata.raw_size_bytes,
            raw_size_bytes
        );
    }

    let target = encoded_blob_path(state_dir, digest, encoding)?;
    let parent = target
        .parent()
        .context("encoded blob path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    write_bytes_atomically(
        parent,
        &target,
        &format!("{}.{}", file_name(digest)?, encoding.file_extension()),
        bytes,
    )
    .await?;

    let mut metadata = metadata;
    metadata
        .encoded_variants
        .insert(encoding, bytes.len() as u64);
    persist_blob_metadata(state_dir, digest, &metadata).await?;

    Ok(StoredEncodedBlobVariant {
        digest: digest.to_string(),
        encoding,
        path: target,
        encoded_size_bytes: bytes.len() as u64,
    })
}

pub async fn has_blob(state_dir: &Path, digest: &str) -> Result<bool> {
    let target = blob_path(state_dir, digest)?;
    Ok(tokio::fs::metadata(target).await.is_ok())
}

pub async fn touch_blob(
    state_dir: &Path,
    digest: &str,
    last_referenced_at: DateTime<Utc>,
) -> Result<BlobMetadata> {
    validate_digest(digest)?;
    let target = blob_path(state_dir, digest)?;
    let raw_size_bytes = tokio::fs::metadata(&target)
        .await
        .with_context(|| format!("Failed to stat {}", target.display()))?
        .len();
    let mut metadata = ensure_blob_metadata(state_dir, digest, raw_size_bytes).await?;
    metadata.last_referenced_at = last_referenced_at;
    persist_blob_metadata(state_dir, digest, &metadata).await?;
    Ok(metadata)
}

pub async fn list_blob_metadata(state_dir: &Path) -> Result<Vec<BlobMetadataRecord>> {
    let blob_root = state_dir.join(BLOBS_SUBDIR);
    let mut records = Vec::new();
    let mut shard_dirs = match tokio::fs::read_dir(&blob_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", blob_root.display()));
        }
    };

    while let Some(shard) = shard_dirs.next_entry().await? {
        let shard_path = shard.path();
        if !shard.file_type().await?.is_dir() {
            continue;
        }
        let mut entries = tokio::fs::read_dir(&shard_path)
            .await
            .with_context(|| format!("Failed to read {}", shard_path.display()))?;
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

            let digest = format!(
                "{}{}",
                shard.file_name().to_string_lossy(),
                &name[..name.len() - ".meta.json".len()]
            );
            let metadata = load_blob_metadata(state_dir, &digest).await?;
            records.push(BlobMetadataRecord { digest, metadata });
        }
    }

    Ok(records)
}

pub async fn delete_blob(state_dir: &Path, digest: &str) -> Result<()> {
    let metadata = load_blob_metadata(state_dir, digest).await.ok();
    let raw_path = blob_path(state_dir, digest)?;
    remove_file_if_exists(&raw_path).await?;

    if let Some(metadata) = metadata {
        for encoding in metadata.encoded_variants.keys() {
            let encoded_path = encoded_blob_path(state_dir, digest, *encoding)?;
            remove_file_if_exists(&encoded_path).await?;
        }
    }

    let metadata_path = metadata_path(state_dir, digest)?;
    remove_file_if_exists(&metadata_path).await
}

pub fn blob_path(state_dir: &Path, digest: &str) -> Result<PathBuf> {
    validate_digest(digest)?;
    Ok(state_dir
        .join(BLOBS_SUBDIR)
        .join(&digest[..2])
        .join(file_name(digest)?))
}

pub fn encoded_blob_path(
    state_dir: &Path,
    digest: &str,
    encoding: BlobEncoding,
) -> Result<PathBuf> {
    let path = blob_path(state_dir, digest)?;
    Ok(path.with_extension(encoding.file_extension()))
}

pub fn metadata_path(state_dir: &Path, digest: &str) -> Result<PathBuf> {
    let path = blob_path(state_dir, digest)?;
    Ok(path.with_extension("meta.json"))
}

async fn maybe_store_zstd_variant(
    state_dir: &Path,
    digest: &str,
    bytes: &[u8],
) -> Result<Option<StoredEncodedBlobVariant>> {
    let raw_size_bytes = bytes.len() as u64;
    let metadata = ensure_blob_metadata(state_dir, digest, raw_size_bytes).await?;
    let encoded_path = encoded_blob_path(state_dir, digest, BlobEncoding::Zstd)?;
    if metadata.encoded_variants.contains_key(&BlobEncoding::Zstd)
        && tokio::fs::metadata(&encoded_path).await.is_ok()
    {
        return Ok(None);
    }

    let input = bytes.to_vec();
    let Some(compressed) =
        tokio::task::spawn_blocking(move || compression::encode_zstd_if_worthwhile(&input))
            .await
            .context("zstd compression task failed to join")??
    else {
        return Ok(None);
    };

    store_encoded_blob_variant(
        state_dir,
        digest,
        raw_size_bytes,
        BlobEncoding::Zstd,
        &compressed,
    )
    .await
    .map(Some)
}

async fn ensure_blob_metadata(
    state_dir: &Path,
    digest: &str,
    raw_size_bytes: u64,
) -> Result<BlobMetadata> {
    match load_blob_metadata(state_dir, digest).await {
        Ok(metadata) => {
            if metadata.canonical_digest != digest {
                anyhow::bail!(
                    "blob metadata digest mismatch: expected {digest}, got {}",
                    metadata.canonical_digest
                );
            }
            if metadata.raw_size_bytes != raw_size_bytes {
                anyhow::bail!(
                    "blob metadata size mismatch for {digest}: expected {}, got {}",
                    raw_size_bytes,
                    metadata.raw_size_bytes
                );
            }
            Ok(metadata)
        }
        Err(error) if is_not_found_error(&error) => {
            let metadata = BlobMetadata {
                canonical_digest: digest.to_string(),
                raw_size_bytes,
                last_referenced_at: Utc::now(),
                encoded_variants: BTreeMap::new(),
            };
            persist_blob_metadata(state_dir, digest, &metadata).await?;
            Ok(metadata)
        }
        Err(error) => Err(error),
    }
}

async fn persist_blob_metadata(
    state_dir: &Path,
    digest: &str,
    metadata: &BlobMetadata,
) -> Result<()> {
    let target = metadata_path(state_dir, digest)?;
    let parent = target
        .parent()
        .context("blob metadata path should always have a parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    let bytes = serde_json::to_vec(metadata)?;
    write_bytes_atomically(
        parent,
        &target,
        &format!("{}.meta.json", file_name(digest)?),
        &bytes,
    )
    .await
    .map(|_| ())
}

async fn write_bytes_atomically(
    parent: &Path,
    target: &Path,
    stem: &str,
    bytes: &[u8],
) -> Result<bool> {
    let temp = parent.join(format!("{stem}.tmp-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&temp, bytes)
        .await
        .with_context(|| format!("Failed to write {}", temp.display()))?;

    match tokio::fs::rename(&temp, target).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(&temp).await;
            Ok(false)
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            Err(error).with_context(|| {
                format!(
                    "Failed to move {} into {}",
                    temp.display(),
                    target.display()
                )
            })
        }
    }
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|inner| inner.kind() == std::io::ErrorKind::NotFound)
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

fn file_name(digest: &str) -> Result<&str> {
    validate_digest(digest)?;
    Ok(&digest[2..])
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid sha256 digest '{digest}'");
    }
    Ok(())
}
