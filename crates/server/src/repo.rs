//! Binary package repository management.
//!
//! Manages the on-disk binpkg repository: indexing packages, computing hashes,
//! generating the `Packages` index file that portage expects.
//!
//! Also provides old-version cleanup (`cleanup_old_versions`) and disk usage
//! monitoring (`disk_usage`) for the binpkg directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Manages the binary package repository on disk.
pub struct BinpkgRepo {
    root: PathBuf,
}

impl BinpkgRepo {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Ensure the repo directory structure exists.
    pub async fn init(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .context("Failed to create binpkg repo directory")?;
        Ok(())
    }

    /// Compute total disk usage of the repository in bytes.
    ///
    /// Walks the directory tree recursively and sums file sizes.
    pub async fn disk_usage(&self) -> Result<u64> {
        let root = self.root.clone();
        // Blocking directory walk on the blocking thread pool.
        tokio::task::spawn_blocking(move || dir_size(&root))
            .await
            .context("Disk usage task panicked")?
    }

    /// Scan the repository for package files, including category subdirectories.
    ///
    /// Supports both flat layout (`*.gpkg.tar`, `*.tbz2`) and portage's
    /// category-aware layout (`category/name-version.gpkg.tar`).
    pub async fn scan_packages(&self) -> Result<Vec<PackageMeta>> {
        let mut entries = Vec::new();

        // Scan top-level files.
        self.scan_directory(&self.root, None, &mut entries).await?;

        // Scan category subdirectories.
        let mut dir = tokio::fs::read_dir(&self.root).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                // Category dirs look like "dev-libs", "sys-apps", etc.
                if dir_name.contains('-') && !dir_name.starts_with('.') && dir_name != "Packages" {
                    self.scan_directory(&path, Some(&dir_name), &mut entries)
                        .await?;
                }
            }
        }

        Ok(entries)
    }

    /// Scan a single directory for package files.
    async fn scan_directory(
        &self,
        dir_path: &Path,
        category: Option<&str>,
        entries: &mut Vec<PackageMeta>,
    ) -> Result<()> {
        let mut dir = match tokio::fs::read_dir(dir_path).await {
            Ok(d) => d,
            Err(_) => return Ok(()),
        };

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext_match = path.to_string_lossy().ends_with(".gpkg.tar")
                || path.extension().and_then(|e| e.to_str()) == Some("tbz2");

            if ext_match {
                match self.read_package_meta(&path, category).await {
                    Ok(meta) => entries.push(meta),
                    Err(e) => {
                        warn!(path = %path.display(), "Failed to read package metadata: {e}");
                    }
                }
            }
        }

        Ok(())
    }

    /// Scan the repository and regenerate the `Packages` index.
    ///
    /// The `Packages` file is the metadata index that portage reads when
    /// configured to use a binhost.
    pub async fn regenerate_index(&self) -> Result<()> {
        let entries = self.scan_packages().await?;

        // Write the Packages index file.
        let index_path = self.root.join("Packages");
        let mut content = String::new();

        // Header.
        content.push_str("PACKAGES: ");
        content.push_str(&entries.len().to_string());
        content.push('\n');
        content.push('\n');

        for entry in &entries {
            content.push_str(&format!("CPV: {}\n", entry.cpv));
            content.push_str(&format!("SIZE: {}\n", entry.size));
            content.push_str(&format!("SHA256: {}\n", entry.sha256));
            content.push_str(&format!("PATH: {}\n", entry.relative_path));
            if entry.is_gpkg {
                content.push_str("BINPKG_FORMAT: gpkg\n");
            }
            content.push('\n');
        }

        tokio::fs::write(&index_path, &content).await?;
        info!(count = entries.len(), "Regenerated Packages index");

        Ok(())
    }

    /// Remove old versions of packages, keeping the `keep` most recent
    /// per CPV base (category/name without version).
    pub async fn cleanup_old_versions(&self, keep: usize) -> Result<usize> {
        let entries = self.scan_packages().await?;

        // Group by package base name (category/name without version).
        let mut groups: std::collections::HashMap<String, Vec<PackageMeta>> =
            std::collections::HashMap::new();
        for entry in entries {
            let base = extract_package_base(&entry.cpv);
            groups.entry(base).or_default().push(entry);
        }

        let mut removed = 0;
        for (_base, mut versions) in groups {
            if versions.len() <= keep {
                continue;
            }

            // Sort by filename (rough version ordering — newer versions
            // have higher version numbers which sort later).
            versions.sort_by(|a, b| a.filename.cmp(&b.filename));

            // Remove the oldest, keeping `keep` most recent.
            let to_remove = versions.len() - keep;
            for entry in versions.into_iter().take(to_remove) {
                let path = self.root.join(&entry.relative_path);
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    warn!(path = %path.display(), "Failed to remove old package: {e}");
                } else {
                    info!(cpv = %entry.cpv, "Removed old package version");
                    removed += 1;
                }
            }
        }

        if removed > 0 {
            self.regenerate_index().await?;
        }

        Ok(removed)
    }

    /// Read metadata for a single package file.
    async fn read_package_meta(&self, path: &Path, category: Option<&str>) -> Result<PackageMeta> {
        let filename = path
            .file_name()
            .context("No filename")?
            .to_string_lossy()
            .to_string();

        let is_gpkg = filename.ends_with(".gpkg.tar");

        // Derive CPV from filename (strip extension).
        let name_without_ext = filename
            .strip_suffix(".gpkg.tar")
            .or_else(|| filename.strip_suffix(".tbz2"))
            .unwrap_or(&filename)
            .to_string();

        // Build full CPV with category if available.
        let cpv = match category {
            Some(cat) => format!("{cat}/{name_without_ext}"),
            None => name_without_ext,
        };

        let relative_path = match category {
            Some(cat) => format!("{cat}/{filename}"),
            None => filename.clone(),
        };

        let data = tokio::fs::read(path).await?;
        let size = data.len() as u64;

        let mut hasher = Sha256::new();
        hasher.update(&data);
        let sha256 = hex::encode(hasher.finalize());

        Ok(PackageMeta {
            cpv,
            filename,
            relative_path,
            size,
            sha256,
            is_gpkg,
        })
    }
}

/// Metadata for a single binary package.
pub struct PackageMeta {
    pub cpv: String,
    pub filename: String,
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
    pub is_gpkg: bool,
}

/// Extract the package base name (category/name) from a CPV string.
///
/// Uses the portage convention: the version starts at the last `-` followed
/// by a digit.
pub fn extract_package_base(cpv: &str) -> String {
    let bytes = cpv.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            return cpv[..i].to_string();
        }
    }
    cpv.to_string()
}

/// Recursively compute the total size (in bytes) of a directory.
fn dir_size(path: &std::path::Path) -> Result<u64> {
    let mut total: u64 = 0;
    if !path.is_dir() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_file() {
            total += entry.metadata()?.len();
        } else if ft.is_dir() {
            total += dir_size(&entry.path())?;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_base_from_cpv() {
        assert_eq!(
            extract_package_base("dev-libs/openssl-3.1.4"),
            "dev-libs/openssl"
        );
        assert_eq!(
            extract_package_base("sys-libs/glibc-2.38-r10"),
            "sys-libs/glibc"
        );
        assert_eq!(extract_package_base("dev-libs/foo"), "dev-libs/foo");
    }
}
