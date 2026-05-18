use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use remerge_types::workorder::{
    ParityDirectoryEntry, ParityFileEntry, ParityManifest, ParitySymlinkEntry,
};
use remerge_types::portage::SnapshotEntry;

const REPO_METADATA_ROOT: &str = "/var/db/repos";
const ECLASS_CACHE_ROOT: &str = "/var/cache/eclass";
const BINPKG_PACKAGES_INDEX: &str = "/var/cache/binpkgs/Packages";
const PORTAGE_STATE_ROOT: &str = "/var/lib/portage";
const DISTFILES_ROOT: &str = "/var/cache/distfiles";

pub async fn capture_final_state_parity(output_dir: &Path) -> Result<ParityManifest> {
    capture_final_state_parity_from(
        output_dir,
        Path::new(REPO_METADATA_ROOT),
        Path::new(ECLASS_CACHE_ROOT),
        Path::new(BINPKG_PACKAGES_INDEX),
        Path::new(PORTAGE_STATE_ROOT),
    )
    .await
}

pub async fn capture_final_state_parity_from(
    output_dir: &Path,
    repo_root: &Path,
    eclass_root: &Path,
    binpkg_packages_index: &Path,
    portage_state_root: &Path,
) -> Result<ParityManifest> {
    tokio::fs::create_dir_all(output_dir)
        .await
        .with_context(|| format!("Failed to create {}", output_dir.display()))?;
    let blobs_dir = output_dir.join("blobs");
    tokio::fs::create_dir_all(&blobs_dir)
        .await
        .with_context(|| format!("Failed to create {}", blobs_dir.display()))?;

    let mut manifest = ParityManifest::default();
    capture_repo_metadata_and_indexes(repo_root, &blobs_dir, &mut manifest).await?;
    capture_tree_if_present(eclass_root, "var/cache/eclass", &blobs_dir, &mut manifest).await?;
    capture_file_if_present(
        binpkg_packages_index,
        "var/cache/binpkgs/Packages",
        &blobs_dir,
        &mut manifest,
    )
    .await?;
    capture_tree_if_present(
        portage_state_root,
        "var/lib/portage",
        &blobs_dir,
        &mut manifest,
    )
    .await?;

    write_manifest(output_dir, &manifest).await?;
    Ok(manifest)
}

pub async fn capture_fetched_distfiles(output_dir: &Path) -> Result<std::collections::BTreeMap<String, SnapshotEntry>> {
    capture_fetched_distfiles_from(output_dir, Path::new(DISTFILES_ROOT)).await
}

pub async fn capture_fetched_distfiles_from(
    output_dir: &Path,
    distfiles_root: &Path,
) -> Result<std::collections::BTreeMap<String, SnapshotEntry>> {
    tokio::fs::create_dir_all(output_dir)
        .await
        .with_context(|| format!("Failed to create {}", output_dir.display()))?;
    let blobs_dir = output_dir.join("blobs");
    tokio::fs::create_dir_all(&blobs_dir)
        .await
        .with_context(|| format!("Failed to create {}", blobs_dir.display()))?;

    let mut manifest = std::collections::BTreeMap::new();
    let root_metadata = match tokio::fs::symlink_metadata(distfiles_root).await {
        Ok(metadata) if metadata.is_dir() => metadata,
        Ok(_) => anyhow::bail!("Expected distfiles dir for capture: {}", distfiles_root.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_distfile_manifest(output_dir, &manifest).await?;
            return Ok(manifest);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to stat {}", distfiles_root.display()));
        }
    };
    if !root_metadata.is_dir() {
        anyhow::bail!("Expected distfiles dir for capture: {}", distfiles_root.display());
    }

    let mut stack = vec![distfiles_root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&current)
            .await
            .with_context(|| format!("Failed to read {}", current.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&entry_path)
                .await
                .with_context(|| format!("Failed to stat {}", entry_path.display()))?;
            if metadata.is_dir() {
                stack.push(entry_path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            let relative = entry_path.strip_prefix(distfiles_root).with_context(|| {
                format!("{} is outside {}", entry_path.display(), distfiles_root.display())
            })?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            let bytes = tokio::fs::read(&entry_path)
                .await
                .with_context(|| format!("Failed to read {}", entry_path.display()))?;
            let digest = store_blob_bytes(&blobs_dir, &bytes).await?;
            manifest.insert(
                relative,
                SnapshotEntry {
                    digest,
                    size: metadata.len(),
                    mtime_secs: metadata.mtime(),
                },
            );
        }
    }

    write_distfile_manifest(output_dir, &manifest).await?;
    Ok(manifest)
}

fn is_runtime_noise_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };

    name == ".lock"
        || name.ends_with(".lock")
        || name.ends_with(".tmp")
        || name.ends_with(".temp")
        || name.ends_with(".part")
        || name.ends_with(".swp")
        || name.ends_with(".pid")
        || name.ends_with(".running")
        || name.ends_with('~')
}

async fn capture_repo_metadata_and_indexes(
    repo_root: &Path,
    blobs_dir: &Path,
    manifest: &mut ParityManifest,
) -> Result<()> {
    let mut repo_entries = match tokio::fs::read_dir(repo_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", repo_root.display()));
        }
    };

    while let Some(entry) = repo_entries.next_entry().await? {
        let repo_path = entry.path();
        let metadata_path = repo_path.join("metadata");
        let repo_name = match repo_path.file_name().and_then(|value| value.to_str()) {
            Some(name) if !name.is_empty() => name,
            _ => continue,
        };

        if tokio::fs::metadata(&metadata_path).await.is_ok() {
            capture_tree_if_present(
                &metadata_path,
                &format!("var/db/repos/{repo_name}/metadata"),
                blobs_dir,
                manifest,
            )
            .await?;
        }

        capture_file_if_present(
            &repo_path.join("Packages"),
            &format!("var/db/repos/{repo_name}/Packages"),
            blobs_dir,
            manifest,
        )
        .await?;
    }

    Ok(())
}

async fn write_manifest(output_dir: &Path, manifest: &ParityManifest) -> Result<()> {
    let manifest_path = output_dir.join("manifest.json");
    tokio::fs::write(&manifest_path, serde_json::to_vec(manifest)?)
        .await
        .with_context(|| format!("Failed to write {}", manifest_path.display()))
}

async fn write_distfile_manifest(
    output_dir: &Path,
    manifest: &std::collections::BTreeMap<String, SnapshotEntry>,
) -> Result<()> {
    let manifest_path = output_dir.join("distfiles.json");
    tokio::fs::write(&manifest_path, serde_json::to_vec(manifest)?)
        .await
        .with_context(|| format!("Failed to write {}", manifest_path.display()))
}

async fn capture_tree_if_present(
    path: &Path,
    manifest_root: &str,
    blobs_dir: &Path,
    manifest: &mut ParityManifest,
) -> Result<()> {
    let root_metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() => metadata,
        Ok(_) => anyhow::bail!("Expected directory for parity capture: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
        }
    };

    insert_directory_entry(manifest, manifest_root.to_string(), root_metadata.mtime())?;

    let mut stack = vec![path.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&current)
            .await
            .with_context(|| format!("Failed to read {}", current.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&entry_path)
                .await
                .with_context(|| format!("Failed to stat {}", entry_path.display()))?;
            let file_type = metadata.file_type();
            let relative = entry_path.strip_prefix(path).with_context(|| {
                format!("{} is outside {}", entry_path.display(), path.display())
            })?;
            let manifest_key = manifest_path_for_relative(manifest_root, relative)?;

            if file_type.is_dir() {
                insert_directory_entry(manifest, manifest_key, metadata.mtime())?;
                stack.push(entry_path);
                continue;
            }

            if is_runtime_noise_path(relative) {
                continue;
            }

            if file_type.is_symlink() {
                let target = tokio::fs::read_link(&entry_path)
                    .await
                    .with_context(|| format!("Failed to read symlink {}", entry_path.display()))?;
                let target_bytes = target.as_os_str().as_bytes().to_vec();
                let digest = store_blob_bytes(blobs_dir, &target_bytes).await?;
                insert_symlink_entry(
                    manifest,
                    manifest_key,
                    digest,
                    target_bytes.len() as u64,
                    metadata.mtime(),
                )?;
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let bytes = tokio::fs::read(&entry_path)
                .await
                .with_context(|| format!("Failed to read {}", entry_path.display()))?;
            let digest = store_blob_bytes(blobs_dir, &bytes).await?;
            insert_manifest_entry(
                manifest,
                manifest_key,
                digest,
                metadata.len(),
                metadata.mtime(),
            )?;
        }
    }

    Ok(())
}

async fn capture_file_if_present(
    path: &Path,
    manifest_path: &str,
    blobs_dir: &Path,
    manifest: &mut ParityManifest,
) -> Result<()> {
    if is_runtime_noise_path(path) {
        return Ok(());
    }

    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => anyhow::bail!(
            "Expected regular file for parity capture: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
        }
    };

    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let digest = store_blob_bytes(blobs_dir, &bytes).await?;

    insert_manifest_entry(
        manifest,
        manifest_path.to_string(),
        digest,
        metadata.len(),
        metadata.mtime(),
    )
}

fn manifest_path_for_relative(manifest_root: &str, relative: &Path) -> Result<String> {
    let mut normalized = PathBuf::from(manifest_root);
    for component in relative.components() {
        match component {
            std::path::Component::Normal(segment) => normalized.push(segment),
            _ => anyhow::bail!("Invalid parity path {}", relative.display()),
        }
    }

    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn insert_manifest_entry(
    manifest: &mut ParityManifest,
    path: String,
    digest: String,
    size: u64,
    mtime_secs: i64,
) -> Result<()> {
    match manifest.files.insert(
        path.clone(),
        ParityFileEntry {
            digest,
            size,
            mtime_secs,
        },
    ) {
        Some(_) => anyhow::bail!("Duplicate parity entry for {path}"),
        None => Ok(()),
    }
}

fn insert_directory_entry(
    manifest: &mut ParityManifest,
    path: String,
    mtime_secs: i64,
) -> Result<()> {
    match manifest
        .directories
        .insert(path.clone(), ParityDirectoryEntry { mtime_secs })
    {
        Some(_) => anyhow::bail!("Duplicate parity directory entry for {path}"),
        None => Ok(()),
    }
}

fn insert_symlink_entry(
    manifest: &mut ParityManifest,
    path: String,
    digest: String,
    size: u64,
    mtime_secs: i64,
) -> Result<()> {
    match manifest.symlinks.insert(
        path.clone(),
        ParitySymlinkEntry {
            digest,
            size,
            mtime_secs,
        },
    ) {
        Some(_) => anyhow::bail!("Duplicate parity symlink entry for {path}"),
        None => Ok(()),
    }
}

async fn store_blob_bytes(blobs_dir: &Path, bytes: &[u8]) -> Result<String> {
    let digest = hex::encode(Sha256::digest(bytes));
    let blob_path = blobs_dir.join(&digest);
    if tokio::fs::metadata(&blob_path).await.is_err() {
        tokio::fs::write(&blob_path, bytes)
            .await
            .with_context(|| format!("Failed to write {}", blob_path.display()))?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::{capture_fetched_distfiles_from, capture_final_state_parity_from};

    #[tokio::test]
    async fn captures_approved_final_state_files_into_manifest_and_blob_dir() {
        let repo_root = tempfile::TempDir::new().unwrap();
        let eclass_root = tempfile::TempDir::new().unwrap();
        let binpkg_root = tempfile::TempDir::new().unwrap();
        let portage_root = tempfile::TempDir::new().unwrap();
        let output = tempfile::TempDir::new().unwrap();
        let metadata_dir = repo_root.path().join("gentoo/metadata/md5-cache");
        tokio::fs::create_dir_all(&metadata_dir).await.unwrap();
        let timestamp = repo_root.path().join("gentoo/metadata/timestamp.chk");
        let md5_cache = metadata_dir.join("dev-libs-demo-1.0");
        #[cfg(unix)]
        let metadata_symlink = repo_root.path().join("gentoo/metadata/cache-link");
        let repo_packages = repo_root.path().join("gentoo/Packages");
        let eclass_file = eclass_root.path().join("5-23/amd64.cache");
        let local_packages = binpkg_root.path().join("Packages");
        let world_file = portage_root.path().join("world");
        let world_sets = portage_root.path().join("sets/custom");
        tokio::fs::write(&timestamp, b"1234\n").await.unwrap();
        tokio::fs::write(&md5_cache, b"EAPI=8\n").await.unwrap();
        tokio::fs::write(&repo_packages, b"repo packages\n")
            .await
            .unwrap();
        tokio::fs::create_dir_all(eclass_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&eclass_file, b"cache entry\n")
            .await
            .unwrap();
        tokio::fs::write(&local_packages, b"PKGDIR index\n")
            .await
            .unwrap();
        tokio::fs::create_dir_all(world_sets.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&world_file, b"app-misc/hello\n")
            .await
            .unwrap();
        tokio::fs::write(&world_sets, b"@world\n").await.unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("md5-cache/dev-libs-demo-1.0", &metadata_symlink).unwrap();
        tokio::fs::write(repo_root.path().join("gentoo/not-parity.txt"), b"ignore\n")
            .await
            .unwrap();

        let manifest = capture_final_state_parity_from(
            output.path(),
            repo_root.path(),
            eclass_root.path(),
            &local_packages,
            portage_root.path(),
        )
        .await
        .expect("capture parity manifest");

        assert_eq!(manifest.files.len(), 7);
        assert!(
            manifest
                .directories
                .contains_key("var/db/repos/gentoo/metadata")
        );
        assert!(
            manifest
                .directories
                .contains_key("var/db/repos/gentoo/metadata/md5-cache")
        );
        assert!(manifest.directories.contains_key("var/cache/eclass"));
        assert!(manifest.directories.contains_key("var/lib/portage"));
        assert!(
            manifest
                .files
                .contains_key("var/db/repos/gentoo/metadata/timestamp.chk")
        );
        assert!(
            manifest
                .files
                .contains_key("var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0")
        );
        assert!(manifest.files.contains_key("var/db/repos/gentoo/Packages"));
        assert!(
            manifest
                .files
                .contains_key("var/cache/eclass/5-23/amd64.cache")
        );
        assert!(manifest.files.contains_key("var/cache/binpkgs/Packages"));
        assert!(manifest.files.contains_key("var/lib/portage/world"));
        assert!(manifest.files.contains_key("var/lib/portage/sets/custom"));
        #[cfg(unix)]
        assert!(
            manifest
                .symlinks
                .contains_key("var/db/repos/gentoo/metadata/cache-link")
        );
        assert!(
            !manifest
                .files
                .contains_key("var/db/repos/gentoo/not-parity.txt")
        );

        let blobs = output.path().join("blobs");
        #[cfg(unix)]
        assert_eq!(std::fs::read_dir(blobs).unwrap().count(), 8);
        #[cfg(not(unix))]
        assert_eq!(std::fs::read_dir(blobs).unwrap().count(), 7);
        assert!(output.path().join("manifest.json").is_file());
    }

    #[tokio::test]
    async fn captures_fetched_distfiles_into_manifest_and_blob_dir() {
        let distfiles = tempfile::TempDir::new().unwrap();
        let output = tempfile::TempDir::new().unwrap();
        let nested = distfiles.path().join("dev-libs");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let first = distfiles.path().join("demo-1.0.tar.xz");
        let second = nested.join("helper.patch");
        tokio::fs::write(&first, b"distfile-one").await.unwrap();
        tokio::fs::write(&second, b"distfile-two").await.unwrap();

        let manifest = capture_fetched_distfiles_from(output.path(), distfiles.path())
            .await
            .expect("capture fetched distfiles");

        assert_eq!(manifest.len(), 2);
        assert!(manifest.contains_key("demo-1.0.tar.xz"));
        assert!(manifest.contains_key("dev-libs/helper.patch"));
        assert!(output.path().join("distfiles.json").is_file());
        assert_eq!(std::fs::read_dir(output.path().join("blobs")).unwrap().count(), 2);
    }
}
