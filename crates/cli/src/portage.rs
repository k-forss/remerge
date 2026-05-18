//! Portage configuration reader.
//!
//! Reads make.conf, package.use, package.accept_keywords, and system identity
//! from the local Gentoo installation and produces types suitable for shipping
//! as a workorder.
//!
//! Also provides:
//! - VDB lookup (`is_installed()`) to skip already-installed packages
//! - Package set expansion (`expand_set()`) for `@world` and `@system`
//!
//! When `-march=native` is detected in `CFLAGS`, it is resolved to the
//! concrete micro-architecture name so the remote worker container can build
//! compatible binaries.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use remerge_types::portage::*;

use crate::cflags;

/// Reads portage configuration from the local system.
pub struct PortageReader {
    root: PathBuf,
}

impl PortageReader {
    /// Create a reader rooted at `/` (or `$ROOT` if set).
    pub fn new() -> Result<Self> {
        let root = std::env::var("ROOT").unwrap_or_else(|_| "/".into()).into();
        Ok(Self { root })
    }

    /// Read the full portage configuration snapshot.
    pub fn read_config(&self) -> Result<PortageConfig> {
        let make_conf = self.read_make_conf()?;
        let package_use = self.read_package_use()?;
        let package_accept_keywords =
            self.read_package_accept_keywords(&make_conf.accept_keywords)?;
        let package_license = self.read_package_license()?;
        let package_mask = self.read_package_mask()?;
        let package_unmask = self.read_package_unmask()?;
        let package_env = self.read_package_env()?;
        let env_files = self.read_env_files()?;
        let repos_conf = self.read_repos_conf()?;
        let repo_snapshots = self.read_repo_snapshots(&repos_conf)?;
        let repo_snapshot_refs = Self::compute_repo_snapshot_refs(&repo_snapshots);
        let repo_snapshot_trees = Self::compute_repo_snapshot_trees(&repo_snapshot_refs)?;
        let patches = self.read_patches()?;
        let distfile_snapshots = self.read_distfile_snapshots(&repo_snapshots)?;
        let distfile_snapshot_refs = Self::compute_distfile_snapshot_refs(&distfile_snapshots);
        let snapshot_manifest = self.build_snapshot_manifest(
            &repos_conf,
            &repo_snapshots,
            &repo_snapshot_refs,
            &repo_snapshot_trees,
            &distfile_snapshots,
            &distfile_snapshot_refs,
        )?;
        let profile_overlay = self.read_profile_overlay()?;
        let profile = self.read_profile()?;
        let world = self.read_world()?;

        Ok(PortageConfig {
            make_conf,
            package_use,
            package_accept_keywords,
            package_license,
            package_mask,
            package_unmask,
            package_env,
            env_files,
            repos_conf,
            snapshot_manifest,
            repo_snapshots,
            repo_snapshot_refs,
            repo_snapshot_trees,
            patches,
            distfile_snapshots,
            distfile_snapshot_refs,
            profile_overlay,
            profile,
            world,
        })
    }

    /// Read and parse `/etc/portage/make.conf`.
    ///
    /// If `CFLAGS` contains `-march=native`, it is resolved to the concrete
    /// micro-architecture flag for the current CPU.  The original value is
    /// preserved in [`MakeConf::original_cflags`].
    fn read_make_conf(&self) -> Result<MakeConf> {
        let vars = self.read_make_conf_vars()?;
        let can_query_portageq = self.root == Path::new("/");
        let get = |key: &str| -> String { vars.get(key).cloned().unwrap_or_default() };
        let get_effective = |key: &str| -> String {
            if can_query_portageq {
                match Self::portageq_envvar(key) {
                    Ok(value) if !value.is_empty() => value,
                    Ok(_) => get(key),
                    Err(error) => {
                        debug!(%error, key, "Failed to resolve make.conf variable via portageq");
                        get(key)
                    }
                }
            } else {
                debug!(key, root = %self.root.display(), "Skipping portageq resolution for non-root snapshot tree");
                get(key)
            }
        };
        let split_flags = |key: &str| -> Vec<String> {
            get_effective(key)
                .split_whitespace()
                .map(String::from)
                .collect()
        };

        // ── Resolve native flags ─────────────────────────────────────
        let resolved =
            cflags::resolve_native_flags().context("Failed to resolve native compiler flags")?;

        let raw_cflags = get_effective("CFLAGS");
        let (cflags, original_cflags) = if let Some(ref march) = resolved.march {
            let (resolved_cflags, was_modified) = cflags::resolve_cflags(&raw_cflags, march);
            if was_modified {
                info!(
                    original = %raw_cflags,
                    resolved = %resolved_cflags,
                    "Translated -march=native in CFLAGS"
                );
                (resolved_cflags, Some(raw_cflags))
            } else {
                (raw_cflags, None)
            }
        } else {
            (raw_cflags, None)
        };

        // Also resolve CXXFLAGS if it doesn't just reference ${CFLAGS}.
        let raw_cxxflags = get_effective("CXXFLAGS");
        let cxxflags = if raw_cxxflags.contains("-march=native") {
            if let Some(ref march) = resolved.march {
                let (resolved, _) = cflags::resolve_cflags(&raw_cxxflags, march);
                resolved
            } else {
                raw_cxxflags
            }
        } else {
            raw_cxxflags
        };

        // CHOST: prefer make.conf, fall back to detected.
        let chost = match get_effective("CHOST") {
            value if !value.is_empty() => value,
            _ => resolved.chost,
        };

        // ── Resolve USE flags ────────────────────────────────────────
        // Profile-inherited flags (e.g. `dbus` from the desktop profile)
        // are NOT present in make.conf.  Use `portageq envvar USE` to get
        // the fully merged value (profile defaults + make.conf + profile
        // force/mask).
        let (use_flags, use_flags_resolved) = match can_query_portageq
            .then(|| Self::portageq_envvar("USE"))
            .transpose()
        {
            Ok(resolved_use) => {
                let resolved_use = resolved_use.unwrap_or_default();
                let flags: Vec<String> =
                    resolved_use.split_whitespace().map(String::from).collect();

                // `portageq envvar USE` returns the *fully expanded* USE
                // string, which includes USE_EXPAND flags like `abi_x86_32`,
                // `python_targets_python3_12`, etc.  These must be stripped
                // because they're sent separately as USE_EXPAND variables
                // and would conflict if duplicated in the USE line (causing
                // slot conflicts with ABI_X86, PYTHON_TARGETS, etc.).
                let flags = Self::strip_use_expand_flags(flags);

                info!(
                    count = flags.len(),
                    "Resolved USE flags via portageq (includes profile defaults)"
                );
                if tracing::enabled!(tracing::Level::DEBUG) {
                    let make_conf_use = split_flags("USE");
                    let extra: Vec<_> = flags
                        .iter()
                        .filter(|f| !make_conf_use.contains(f))
                        .collect();
                    if !extra.is_empty() {
                        debug!(?extra, "USE flags from profile defaults (not in make.conf)");
                    }
                }
                (flags, true)
            }
            Err(e) => {
                warn!(
                    %e,
                    "Failed to resolve USE via portageq — falling back to make.conf"
                );
                (split_flags("USE"), false)
            }
        };

        // ── Collect USE_EXPAND variables ─────────────────────────────
        // Dynamically discover all USE_EXPAND variable names from portage
        // so we capture LLVM_SLOT, LLVM_TARGETS, and any other vars that
        // eselect or profiles define.  Without this, the worker container
        // won't know which LLVM slot, Python targets, etc. to use.
        //
        // Some variables (e.g. LLVM_SLOT) live in USE_EXPAND_UNPREFIXED
        // or USE_EXPAND_HIDDEN rather than USE_EXPAND.  Query all three
        // and merge the results so nothing is silently dropped.
        let use_expand_keys: Vec<String> = {
            let mut keys = std::collections::BTreeSet::new();
            let mut any_succeeded = false;

            for var in ["USE_EXPAND", "USE_EXPAND_UNPREFIXED", "USE_EXPAND_HIDDEN"] {
                if !can_query_portageq {
                    break;
                }
                match Self::portageq_envvar(var) {
                    Ok(expand_str) => {
                        let found: Vec<String> =
                            expand_str.split_whitespace().map(String::from).collect();
                        debug!(var, count = found.len(), "Discovered USE_EXPAND variables");
                        keys.extend(found);
                        any_succeeded = true;
                    }
                    Err(e) => {
                        debug!(%e, var, "Failed to query USE_EXPAND variant");
                    }
                }
            }

            if any_succeeded {
                info!(
                    count = keys.len(),
                    "Discovered USE_EXPAND variables via portageq"
                );
                keys.into_iter().collect()
            } else {
                warn!("Failed to query any USE_EXPAND variants — using hardcoded fallback");
                [
                    "ABI_X86",
                    "VIDEO_CARDS",
                    "INPUT_DEVICES",
                    "L10N",
                    "PYTHON_TARGETS",
                    "PYTHON_SINGLE_TARGET",
                    "RUBY_TARGETS",
                    "LUA_TARGETS",
                    "LUA_SINGLE_TARGET",
                    "LLVM_SLOT",
                    "LLVM_TARGETS",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect()
            }
        };

        let mut use_expand = BTreeMap::new();
        for key in &use_expand_keys {
            // CPU_FLAGS_* are handled separately via cpuid2cpuflags.
            if key.starts_with("CPU_FLAGS_") {
                continue;
            }
            // Try portageq first for fully-resolved values, fall back to make.conf.
            let vals = match can_query_portageq
                .then(|| Self::portageq_envvar(key))
                .transpose()
            {
                Ok(resolved) => {
                    let resolved = resolved.unwrap_or_default();
                    let v: Vec<String> = resolved.split_whitespace().map(String::from).collect();
                    if !v.is_empty() {
                        debug!(key = %key, ?v, "USE_EXPAND resolved via portageq");
                    }
                    v
                }
                Err(_) => split_flags(key),
            };
            if !vals.is_empty() {
                use_expand.insert(key.clone(), vals);
            }
        }

        // Collect extra vars we don't model explicitly.
        let known_keys: std::collections::HashSet<&str> = [
            "CFLAGS",
            "CXXFLAGS",
            "LDFLAGS",
            "MAKEOPTS",
            "USE",
            "FEATURES",
            "ACCEPT_LICENSE",
            "ACCEPT_KEYWORDS",
            "EMERGE_DEFAULT_OPTS",
            "CHOST",
        ]
        .iter()
        .copied()
        .chain(use_expand_keys.iter().map(|s| s.as_str()))
        .collect();

        let extra: BTreeMap<String, String> = vars
            .iter()
            .filter(|(k, _)| !known_keys.contains(k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(MakeConf {
            cflags,
            cxxflags,
            ldflags: get_effective("LDFLAGS"),
            makeopts: get_effective("MAKEOPTS"),
            use_flags,
            features: split_flags("FEATURES"),
            accept_license: get_effective("ACCEPT_LICENSE"),
            accept_keywords: get_effective("ACCEPT_KEYWORDS"),
            emerge_default_opts: get_effective("EMERGE_DEFAULT_OPTS"),
            chost,
            cpu_flags: resolved.cpu_flags,
            original_cflags,
            use_expand,
            extra,
            use_flags_resolved,
        })
    }

    fn read_make_conf_vars(&self) -> Result<BTreeMap<String, String>> {
        let preferred = self.root.join("etc/portage/make.conf");
        let legacy = self.root.join("etc/make.conf");
        let mut vars = BTreeMap::new();
        let mut loaded_any = false;

        for path in [&preferred, &legacy] {
            if path.is_file() {
                let content = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read {}", path.display()))?;
                vars.extend(Self::parse_make_conf_vars(&content));
                loaded_any = true;
            }
        }

        if !loaded_any {
            return Err(anyhow!(
                "Failed to read {} or {}",
                preferred.display(),
                legacy.display()
            ));
        }

        Ok(vars)
    }

    /// Read per-package USE flags from `/etc/portage/package.use`.
    fn read_package_use(&self) -> Result<Vec<PackageUseEntry>> {
        self.read_package_entries("package.use", |line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            Some(PackageUseEntry {
                atom: parts[0].to_string(),
                flags: parts[1..].iter().map(|s| s.to_string()).collect(),
            })
        })
    }

    /// Read package keywords from `/etc/portage/package.accept_keywords`.
    fn read_package_accept_keywords(
        &self,
        global_accept_keywords: &str,
    ) -> Result<Vec<PackageKeywordEntry>> {
        let default_keywords = Self::empty_accept_keywords_defaults(global_accept_keywords);
        self.read_package_entries("package.accept_keywords", |line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                return None;
            }
            Some(PackageKeywordEntry {
                atom: parts[0].to_string(),
                keywords: if parts.len() > 1 {
                    parts[1..].iter().map(|s| s.to_string()).collect()
                } else {
                    default_keywords.clone()
                },
            })
        })
    }

    fn empty_accept_keywords_defaults(global_accept_keywords: &str) -> Vec<String> {
        global_accept_keywords
            .split_whitespace()
            .filter(|keyword| !keyword.is_empty())
            .filter(|keyword| !keyword.starts_with('~') && !keyword.starts_with('-'))
            .map(|keyword| format!("~{keyword}"))
            .collect()
    }

    /// Read package licenses from `/etc/portage/package.license`.
    fn read_package_license(&self) -> Result<Vec<PackageLicenseEntry>> {
        self.read_package_entries("package.license", |line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            Some(PackageLicenseEntry {
                atom: parts[0].to_string(),
                licenses: parts[1..].iter().map(|s| s.to_string()).collect(),
            })
        })
    }

    /// Read masked atoms from `/etc/portage/package.mask`.
    ///
    /// Each non-comment, non-empty line is a package atom to mask.
    fn read_package_mask(&self) -> Result<Vec<String>> {
        self.read_package_atoms("package.mask")
    }

    /// Read unmasked atoms from `/etc/portage/package.unmask`.
    ///
    /// Each non-comment, non-empty line is a package atom to unmask
    /// (overriding profile or repository masks).
    fn read_package_unmask(&self) -> Result<Vec<String>> {
        self.read_package_atoms("package.unmask")
    }

    /// Read a simple atom-per-line file (used for package.mask / package.unmask).
    fn read_package_atoms(&self, name: &str) -> Result<Vec<String>> {
        self.read_package_entries(name, |line| {
            let atom = line.split_whitespace().next()?;
            if atom.is_empty() {
                return None;
            }
            Some(atom.to_string())
        })
    }

    /// Read per-package environment overrides from `/etc/portage/package.env`.
    ///
    /// Each line maps an atom to an env file name:
    /// ```text
    /// dev-qt/qtwebengine no-lto.conf
    /// sys-apps/systemd custom-cflags.conf
    /// ```
    fn read_package_env(&self) -> Result<Vec<PackageEnvEntry>> {
        self.read_package_entries("package.env", |line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            Some(PackageEnvEntry {
                atom: parts[0].to_string(),
                env_file: parts[1].to_string(),
            })
        })
    }

    /// Read all environment override files from `/etc/portage/env/`.
    ///
    /// Returns a map of filename → content.  These files are referenced
    /// by `package.env` entries and can set per-package variables like
    /// `CFLAGS`, `MAKEOPTS`, `CMAKE_BUILD_TYPE`, etc.
    fn read_env_files(&self) -> Result<BTreeMap<String, String>> {
        let path = self.root.join("etc/portage/env");
        let mut files = BTreeMap::new();

        if path.is_dir() {
            if let Ok(dir) = fs::read_dir(&path) {
                for entry in dir.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        let fname = entry.file_name();
                        let fname_str = fname.to_string_lossy();
                        if !fname_str.starts_with('.')
                            && let Ok(content) = fs::read_to_string(entry.path())
                        {
                            debug!(file = %fname_str, "Read env file");
                            files.insert(fname_str.into_owned(), content);
                        }
                    }
                }
            }
        } else {
            debug!("No /etc/portage/env/ directory — skipping");
        }

        if !files.is_empty() {
            info!(count = files.len(), "Read portage env files");
        }
        Ok(files)
    }

    /// Read portage repository configuration from `/etc/portage/repos.conf`.
    ///
    /// May be a single file or a directory of `.conf` files.  Returns a map
    /// of filename → raw INI content.  This captures custom overlay
    /// definitions, sync URIs, and repo priorities.
    fn read_repos_conf(&self) -> Result<BTreeMap<String, String>> {
        let path = self.root.join("etc/portage/repos.conf");
        let mut files = BTreeMap::new();

        if path.is_dir() {
            if let Ok(dir) = fs::read_dir(&path) {
                for entry in dir.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        let fname = entry.file_name();
                        let fname_str = fname.to_string_lossy();
                        if !fname_str.starts_with('.')
                            && let Ok(content) = fs::read_to_string(entry.path())
                        {
                            debug!(file = %fname_str, "Read repos.conf file");
                            files.insert(fname_str.into_owned(), content);
                        }
                    }
                }
            }
        } else if path.is_file() {
            if let Ok(content) = fs::read_to_string(&path) {
                files.insert("repos.conf".to_string(), content);
            }
        } else {
            debug!("No /etc/portage/repos.conf — skipping");
        }

        if !files.is_empty() {
            info!(count = files.len(), "Read repos.conf files");
        }
        Ok(files)
    }

    fn read_repo_snapshots(
        &self,
        repos_conf: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, RepoSnapshot>> {
        let mut snapshots = BTreeMap::new();

        for content in repos_conf.values() {
            for (name, location) in Self::parse_repo_sections(content) {
                if !Self::should_snapshot_repo(&name, &location) {
                    continue;
                }

                let location_path = Path::new(&location);
                if !location_path.is_dir() {
                    debug!(repo = %name, location = %location, "Repo location missing — skipping snapshot");
                    continue;
                }

                let mut files = BTreeMap::new();
                Self::read_text_tree_recursive(location_path, location_path, &mut files);
                if !files.is_empty() {
                    info!(repo = %name, file_count = files.len(), "Captured local repo snapshot");
                    snapshots.insert(name, files);
                }
            }
        }

        Ok(snapshots)
    }

    fn read_distfile_snapshots(
        &self,
        repo_snapshots: &BTreeMap<String, RepoSnapshot>,
    ) -> Result<BTreeMap<String, Vec<u8>>> {
        let distdir = self.detect_distdir();
        let mut files = BTreeMap::new();

        for snapshot in repo_snapshots.values() {
            for (path, content) in snapshot {
                if !path.ends_with("/Manifest") && path != "Manifest" {
                    continue;
                }

                for distfile in Self::parse_manifest_distfiles(content) {
                    if files.contains_key(&distfile) {
                        continue;
                    }

                    let source_path = distdir.join(&distfile);
                    match fs::read(&source_path) {
                        Ok(data) => {
                            info!(distfile = %distfile, size = data.len(), "Captured distfile snapshot");
                            files.insert(distfile, data);
                        }
                        Err(error) => {
                            warn!(%error, path = %source_path.display(), "Manifest distfile missing locally — skipping snapshot");
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    fn compute_repo_snapshot_refs(
        repo_snapshots: &BTreeMap<String, RepoSnapshot>,
    ) -> BTreeMap<String, SnapshotRefs> {
        repo_snapshots
            .iter()
            .map(|(repo, snapshot)| {
                let refs = snapshot
                    .iter()
                    .map(|(path, content)| (path.clone(), Self::sha256_hex(content.as_bytes())))
                    .collect();
                (repo.clone(), refs)
            })
            .collect()
    }

    fn compute_repo_snapshot_trees(
        repo_snapshot_refs: &BTreeMap<String, SnapshotRefs>,
    ) -> Result<BTreeMap<String, String>> {
        repo_snapshot_refs
            .iter()
            .map(|(repo, refs)| Ok((repo.clone(), Self::tree_digest(refs)?)))
            .collect()
    }

    fn compute_distfile_snapshot_refs(
        distfile_snapshots: &BTreeMap<String, Vec<u8>>,
    ) -> BTreeMap<String, String> {
        distfile_snapshots
            .iter()
            .map(|(name, bytes)| (name.clone(), Self::sha256_hex(bytes)))
            .collect()
    }

    fn build_snapshot_manifest(
        &self,
        repos_conf: &BTreeMap<String, String>,
        repo_snapshots: &BTreeMap<String, RepoSnapshot>,
        repo_snapshot_refs: &BTreeMap<String, SnapshotRefs>,
        repo_snapshot_trees: &BTreeMap<String, String>,
        distfile_snapshots: &BTreeMap<String, Vec<u8>>,
        distfile_snapshot_refs: &BTreeMap<String, String>,
    ) -> Result<SnapshotManifest> {
        let mut manifest = SnapshotManifest::default();
        let repo_locations: BTreeMap<String, PathBuf> = repos_conf
            .values()
            .flat_map(|content| Self::parse_repo_sections(content).into_iter())
            .map(|(name, location)| (name, PathBuf::from(location)))
            .collect();

        for (repo_name, snapshot) in repo_snapshots {
            let refs = repo_snapshot_refs
                .get(repo_name)
                .with_context(|| format!("Missing repo snapshot refs for repo '{repo_name}'"))?;
            let tree_digest = repo_snapshot_trees
                .get(repo_name)
                .cloned()
                .unwrap_or_default();
            let repo_root = repo_locations
                .get(repo_name)
                .with_context(|| format!("Missing repo location for repo '{repo_name}'"))?;
            let mut entries = BTreeMap::new();

            for relative_path in snapshot.keys() {
                let digest = refs.get(relative_path).with_context(|| {
                    format!("Missing repo snapshot digest for '{repo_name}:{relative_path}'")
                })?;
                let full_path = repo_root.join(relative_path);
                let metadata = fs::metadata(&full_path)
                    .with_context(|| format!("Failed to stat {}", full_path.display()))?;
                entries.insert(
                    relative_path.clone(),
                    SnapshotEntry {
                        digest: digest.clone(),
                        size: metadata.len(),
                        mtime_secs: metadata.mtime(),
                    },
                );
            }

            manifest.repo_snapshots.insert(
                repo_name.clone(),
                RepoSnapshotManifest {
                    tree_digest,
                    entries,
                },
            );
        }

        let distdir = self.detect_distdir();
        for (filename, bytes) in distfile_snapshots {
            let digest = distfile_snapshot_refs
                .get(filename)
                .with_context(|| format!("Missing distfile snapshot digest for '{filename}'"))?;
            let full_path = distdir.join(filename);
            let metadata = fs::metadata(&full_path)
                .with_context(|| format!("Failed to stat {}", full_path.display()))?;
            if metadata.len() != bytes.len() as u64 {
                anyhow::bail!(
                    "Distfile snapshot size mismatch for '{filename}': captured {} bytes, filesystem reports {}",
                    bytes.len(),
                    metadata.len()
                );
            }
            manifest.distfiles.insert(
                filename.clone(),
                SnapshotEntry {
                    digest: digest.clone(),
                    size: metadata.len(),
                    mtime_secs: metadata.mtime(),
                },
            );
        }

        Ok(manifest)
    }

    /// Read user patches from `/etc/portage/patches/`.
    ///
    /// Returns a map of relative path → file content.  The directory
    /// structure mirrors portage's user-patch layout:
    ///
    /// ```text
    /// /etc/portage/patches/
    ///   dev-libs/openssl/
    ///     fix-cve.patch
    ///   sys-apps/systemd/
    ///     no-telemetry.patch
    /// ```
    fn read_patches(&self) -> Result<BTreeMap<String, String>> {
        let path = self.root.join("etc/portage/patches");
        let mut patches = BTreeMap::new();

        if !path.is_dir() {
            debug!("No /etc/portage/patches/ directory — skipping");
            return Ok(patches);
        }

        Self::read_patches_recursive(&path, &path, &mut patches);

        if !patches.is_empty() {
            info!(count = patches.len(), "Read user patches");
        }
        Ok(patches)
    }

    /// Recursively collect patch files under a directory.
    fn read_patches_recursive(
        base: &std::path::Path,
        dir: &std::path::Path,
        out: &mut BTreeMap<String, String>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::read_patches_recursive(base, &path, out);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                if let Ok(content) = fs::read_to_string(&path) {
                    debug!(patch = %relative, "Read user patch");
                    out.insert(relative, content);
                }
            }
        }
    }

    /// Read the local profile overlay from `/etc/portage/profile/`.
    ///
    /// This directory overrides profile-level settings without creating a
    /// custom profile.  Common files include `package.provided`,
    /// `use.mask`, `use.force`, `package.mask`, `packages`, and
    /// `make.defaults`.
    fn read_profile_overlay(&self) -> Result<BTreeMap<String, String>> {
        let path = self.root.join("etc/portage/profile");
        let mut files = BTreeMap::new();

        if !path.is_dir() {
            debug!("No /etc/portage/profile/ directory — skipping");
            return Ok(files);
        }

        Self::read_dir_recursive(&path, &path, &mut files);

        if !files.is_empty() {
            info!(count = files.len(), "Read profile overlay files");
        }
        Ok(files)
    }

    /// Recursively collect files under a directory, storing them with
    /// paths relative to `base`.
    fn read_dir_recursive(
        base: &std::path::Path,
        dir: &std::path::Path,
        out: &mut BTreeMap<String, String>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::read_dir_recursive(base, &path, out);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                if let Ok(content) = fs::read_to_string(&path) {
                    out.insert(relative, content);
                }
            }
        }
    }

    fn read_text_tree_recursive(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                Self::read_text_tree_recursive(base, &path, out);
            } else if path.is_file()
                && let Ok(content) = fs::read_to_string(&path)
            {
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(relative, content);
            }
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn tree_digest(entries: &SnapshotRefs) -> Result<String> {
        #[derive(Serialize)]
        struct TreeManifest<'a> {
            entries: &'a SnapshotRefs,
        }

        let bytes = serde_json::to_vec(&TreeManifest { entries })?;
        Ok(Self::sha256_hex(&bytes))
    }

    fn should_snapshot_repo(name: &str, location: &str) -> bool {
        if name.eq_ignore_ascii_case("gentoo") {
            return false;
        }

        let location = location.trim_end_matches('/');
        !location.ends_with("/gentoo")
    }

    fn parse_repo_sections(content: &str) -> Vec<(String, String)> {
        let mut repos = Vec::new();
        let mut current_name = String::new();
        let mut current_location: Option<String> = None;

        let flush = |name: &str, location: Option<String>, out: &mut Vec<(String, String)>| {
            if !name.is_empty()
                && !name.eq_ignore_ascii_case("DEFAULT")
                && let Some(location) = location
            {
                out.push((name.to_string(), location));
            }
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                flush(&current_name, current_location.take(), &mut repos);
                current_name = trimmed[1..trimmed.len() - 1].trim().to_string();
                continue;
            }
            if let Some((key, value)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("location")
            {
                current_location = Some(value.trim().to_string());
            }
        }

        flush(&current_name, current_location.take(), &mut repos);
        repos
    }

    fn parse_manifest_distfiles(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                match (parts.next(), parts.next()) {
                    (Some("DIST"), Some(name)) if Self::is_valid_distfile_basename(name) => {
                        Some(name.to_string())
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn is_valid_distfile_basename(name: &str) -> bool {
        use std::path::Component;

        if name.trim().is_empty() {
            return false;
        }

        let mut components = Path::new(name).components();
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
    }

    fn detect_distdir(&self) -> PathBuf {
        if let Ok(value) = std::env::var("DISTDIR") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }

        let root_distdir = self.root.join("var/cache/distfiles");
        if root_distdir.is_dir() {
            return root_distdir;
        }

        if self.root != Path::new("/") {
            return PathBuf::from("/var/cache/distfiles");
        }

        match Self::portageq_envvar("DISTDIR") {
            Ok(value) if !value.is_empty() => PathBuf::from(value),
            _ => PathBuf::from("/var/cache/distfiles"),
        }
    }

    /// Generic reader for `/etc/portage/<name>` which may be a file or directory.
    fn read_package_entries<T>(
        &self,
        name: &str,
        parser: impl Fn(&str) -> Option<T>,
    ) -> Result<Vec<T>> {
        let path = self.root.join("etc/portage").join(name);
        let mut entries = Vec::new();

        if path.is_dir() {
            Self::read_package_entries_recursive(&path, &parser, &mut entries);
        } else if path.is_file() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            Self::parse_lines(&content, &parser, &mut entries);
        } else {
            debug!("{} does not exist — skipping", path.display());
        }

        Ok(entries)
    }

    fn read_package_entries_recursive<T>(
        dir: &std::path::Path,
        parser: &impl Fn(&str) -> Option<T>,
        out: &mut Vec<T>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();

        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                Self::read_package_entries_recursive(&path, parser, out);
            } else if path.is_file()
                && let Ok(content) = fs::read_to_string(&path)
            {
                Self::parse_lines(&content, parser, out);
            }
        }
    }

    /// Parse non-comment, non-empty lines.
    fn parse_lines<T>(content: &str, parser: &impl Fn(&str) -> Option<T>, out: &mut Vec<T>) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(entry) = parser(line) {
                out.push(entry);
            }
        }
    }

    /// Read the active profile.
    fn read_profile(&self) -> Result<String> {
        let link = self.root.join("etc/portage/make.profile");
        match fs::read_link(&link) {
            Ok(target) => {
                // Extract the meaningful part after "profiles/".
                let s = target.to_string_lossy();
                if let Some(idx) = s.find("profiles/") {
                    Ok(s[idx + 9..].to_string())
                } else {
                    Ok(s.into_owned())
                }
            }
            Err(_) => {
                // Try eselect profile show fallback.
                warn!("Could not read make.profile symlink");
                Ok("unknown".into())
            }
        }
    }

    /// Read the world set.
    fn read_world(&self) -> Result<Vec<String>> {
        let path = self.root.join("var/lib/portage/world");
        match fs::read_to_string(&path) {
            Ok(content) => Ok(content
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect()),
            Err(_) => {
                debug!("No world file found");
                Ok(Vec::new())
            }
        }
    }

    /// Expand a portage package set into its constituent atoms.
    ///
    /// Currently supports `@world` and `@system`.
    ///
    /// - `@world`: reads `/var/lib/portage/world`
    /// - `@system`: reads `/var/lib/portage/world_sets` for `@system` and
    ///   falls back to packages listed in the active profile's `packages`
    ///   file.
    ///
    /// Returns the original set name wrapped in a `Vec` if the set is
    /// unrecognised — the server may still know what to do with it.
    pub fn expand_set(&self, set_name: &str) -> Vec<String> {
        match set_name {
            "@world" => {
                let atoms = self.read_world().unwrap_or_default();
                if atoms.is_empty() {
                    warn!("@world set is empty — passing through");
                    return vec![set_name.to_string()];
                }
                info!(count = atoms.len(), "Expanded @world set");
                atoms
            }
            "@system" => {
                let atoms = self.read_system_set();
                if atoms.is_empty() {
                    warn!("@system set is empty — passing through");
                    return vec![set_name.to_string()];
                }
                info!(count = atoms.len(), "Expanded @system set");
                atoms
            }
            _ => {
                debug!(set_name, "Unknown set — passing through");
                vec![set_name.to_string()]
            }
        }
    }

    /// Read the `@system` set.
    ///
    /// Tries `/var/lib/portage/world_sets` first (if it lists `@system`),
    /// then falls back to the profile `packages` file which lists the system
    /// set with `*` prefix.
    fn read_system_set(&self) -> Vec<String> {
        // 1. Try world_sets file.
        let world_sets_path = self.root.join("var/lib/portage/world_sets");
        if let Ok(content) = fs::read_to_string(&world_sets_path)
            && content.lines().any(|l| l.trim() == "@system")
        {
            debug!("Found @system in world_sets");
        }

        // 2. Read from profile packages file (the authoritative source).
        let profile_link = self.root.join("etc/portage/make.profile");
        if let Ok(target) = fs::read_link(&profile_link) {
            let profile_dir = if target.is_absolute() {
                target
            } else {
                profile_link.parent().unwrap_or(&self.root).join(&target)
            };
            return self.read_profile_packages(&profile_dir);
        }

        Vec::new()
    }

    /// Read `packages` files from a profile directory and its parents.
    ///
    /// Lines prefixed with `*` denote system packages.
    fn read_profile_packages(&self, profile_dir: &std::path::Path) -> Vec<String> {
        let mut atoms = Vec::new();

        // Read parent profiles first.
        let parent_file = profile_dir.join("parent");
        if let Ok(content) = fs::read_to_string(&parent_file) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let parent_dir = profile_dir.join(line);
                if parent_dir.is_dir() {
                    atoms.extend(self.read_profile_packages(&parent_dir));
                }
            }
        }

        // Read this profile's packages file.
        let packages_file = profile_dir.join("packages");
        if let Ok(content) = fs::read_to_string(&packages_file) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                // Lines starting with '*' are system packages.
                if let Some(atom) = line.strip_prefix('*') {
                    let atom = atom.trim();
                    if !atom.is_empty() {
                        atoms.push(atom.to_string());
                    }
                }
            }
        }

        atoms
    }

    /// Check if a portage package atom is installed locally.
    ///
    /// Evaluates the version constraint from the atom against installed
    /// package versions in `/var/db/pkg/<category>/<name>-<version>/`.
    ///
    /// Supported atom forms:
    /// - `category/package` — any version installed
    /// - `=category/package-version` — exact version (with revision)
    /// - `>=category/package-version` — version or newer
    /// - `<=category/package-version` — version or older
    /// - `>category/package-version` — strictly newer
    /// - `<category/package-version` — strictly older
    /// - `~category/package-version` — same version, any revision
    /// - `=category/package-version*` — version glob
    ///
    /// Package sets (`@world`, `@system`) are never considered "installed".
    pub fn is_installed(&self, atom: &str) -> bool {
        // Sets are never "installed" in the traditional sense.
        if atom.starts_with('@') {
            return false;
        }

        // Parse the operator and strip it.
        let (op, stripped) = parse_atom_operator(atom);

        let Some((category, name_maybe_version)) = stripped.split_once('/') else {
            return false;
        };

        // When there is no operator, treat the entire right-hand side as the
        // package name — no version constraint.  This avoids mis-splitting
        // names that contain `-<digit>` (e.g. `python-exec-2`).
        let (pkg_name, constraint_version) = match op {
            AtomOp::None => (name_maybe_version, None),
            _ => split_name_version(name_maybe_version),
        };

        let pkg_dir = self.root.join("var/db/pkg").join(category);
        let Ok(entries) = std::fs::read_dir(&pkg_dir) else {
            return false;
        };

        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            let (installed_name, installed_version) = split_name_version(&fname_str);
            if installed_name != pkg_name {
                continue;
            }

            // If no operator, any installed version satisfies.
            let Some(constraint_ver) = constraint_version else {
                return true;
            };
            let Some(installed_ver) = installed_version else {
                continue;
            };

            match op {
                AtomOp::None => return true,
                AtomOp::Eq => {
                    if installed_ver == constraint_ver {
                        return true;
                    }
                }
                AtomOp::EqGlob => {
                    // =cat/pkg-1.2* matches 1.2, 1.2.3, 1.2.3-r1, etc.
                    let prefix = constraint_ver.trim_end_matches('*');
                    if installed_ver.starts_with(prefix) {
                        return true;
                    }
                }
                AtomOp::Tilde => {
                    // ~cat/pkg-1.2.3 matches any revision of 1.2.3
                    let (constraint_base, _) = split_revision(constraint_ver);
                    let (installed_base, _) = split_revision(installed_ver);
                    if installed_base == constraint_base {
                        return true;
                    }
                }
                AtomOp::Ge => {
                    if compare_versions(installed_ver, constraint_ver) != std::cmp::Ordering::Less {
                        return true;
                    }
                }
                AtomOp::Le => {
                    if compare_versions(installed_ver, constraint_ver)
                        != std::cmp::Ordering::Greater
                    {
                        return true;
                    }
                }
                AtomOp::Gt => {
                    if compare_versions(installed_ver, constraint_ver)
                        == std::cmp::Ordering::Greater
                    {
                        return true;
                    }
                }
                AtomOp::Lt => {
                    if compare_versions(installed_ver, constraint_ver) == std::cmp::Ordering::Less {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Read system identity by probing installed packages.
    pub fn read_system_identity(&self) -> Result<SystemIdentity> {
        let profile = self.read_profile()?;

        // Determine arch from ACCEPT_KEYWORDS or profile.
        let arch = self.detect_arch(&profile);

        // Detect CHOST — prefer make.conf, then portageq, then gcc, then uname.
        let chost = self.detect_chost()?;

        let gcc_version = Self::detect_version("gcc");
        let libc_version = Self::detect_libc_version();
        let kernel_version = Self::detect_kernel_version();
        let python_targets = self.detect_python_targets();

        Ok(SystemIdentity {
            arch,
            chost,
            gcc_version,
            libc_version,
            kernel_version,
            python_targets,
            profile,
        })
    }

    /// Detect CHOST from make.conf or system probes.
    fn detect_chost(&self) -> Result<String> {
        // 1. Try reading from make.conf.
        let path = self.root.join("etc/portage/make.conf");
        if let Ok(content) = fs::read_to_string(&path) {
            let vars = Self::parse_make_conf_vars(&content);
            if let Some(chost) = vars.get("CHOST")
                && !chost.is_empty()
            {
                debug!(chost = %chost, "CHOST from make.conf");
                return Ok(chost.clone());
            }
        }

        // 2. Fall back to the cflags module's detection (portageq → gcc → uname).
        cflags::resolve_native_flags()
            .map(|r| r.chost)
            .context("Failed to detect CHOST")
    }

    fn detect_arch(&self, profile: &str) -> String {
        // Try to extract from profile path.
        for part in profile.split('/') {
            match part {
                "amd64" | "arm64" | "arm" | "x86" | "ppc64" | "riscv" | "s390" => {
                    return part.to_string();
                }
                _ => {}
            }
        }
        // Fallback to uname.
        std::process::Command::new("uname")
            .arg("-m")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                let s = s.trim();
                match s {
                    "x86_64" => "amd64".to_string(),
                    "aarch64" => "arm64".to_string(),
                    other => other.to_string(),
                }
            })
            .unwrap_or_else(|| "amd64".into())
    }

    fn detect_version(program: &str) -> String {
        std::process::Command::new(program)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                // First line usually has the version.
                s.lines().next().map(|l| l.trim().to_string())
            })
            .unwrap_or_else(|| "unknown".into())
    }

    fn detect_libc_version() -> String {
        // Try ldd --version (glibc).
        std::process::Command::new("ldd")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
            .unwrap_or_else(|| "unknown".into())
    }

    fn detect_kernel_version() -> String {
        std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".into())
    }

    fn detect_python_targets(&self) -> Vec<String> {
        let path = self.root.join("etc/portage/make.conf");
        if let Ok(content) = fs::read_to_string(&path) {
            let vars = Self::parse_make_conf_vars(&content);
            if let Some(targets) = vars.get("PYTHON_TARGETS") {
                return targets.split_whitespace().map(String::from).collect();
            }
        }
        Vec::new()
    }

    /// Minimalist shell variable parser for make.conf.
    ///
    /// Handles `VAR="value"`, `VAR='value'`, and `VAR=value` forms,
    /// including multiline quoted values and multiple assignments on one line.
    /// Does NOT evaluate command substitution or execute shell code.
    ///
    /// For variables that need full resolution (including `source` directives
    /// and profile inheritance), use [`portageq_envvar`] instead.
    pub fn parse_make_conf_vars(content: &str) -> BTreeMap<String, String> {
        fn is_var_start(ch: char) -> bool {
            ch == '_' || ch.is_ascii_alphabetic()
        }

        fn is_var_char(ch: char) -> bool {
            ch == '_' || ch.is_ascii_alphanumeric()
        }

        let mut vars = BTreeMap::new();

        let chars: Vec<char> = content.chars().collect();
        let mut index = 0;

        while index < chars.len() {
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            if index >= chars.len() {
                break;
            }

            if chars[index] == '#' {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }

            if !is_var_start(chars[index]) {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }

            let key_start = index;
            index += 1;
            while index < chars.len() && is_var_char(chars[index]) {
                index += 1;
            }
            let key: String = chars[key_start..index].iter().collect();

            while index < chars.len() && chars[index].is_whitespace() && chars[index] != '\n' {
                index += 1;
            }
            if index >= chars.len() || chars[index] != '=' {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }
            index += 1;

            while index < chars.len() && chars[index].is_whitespace() && chars[index] != '\n' {
                index += 1;
            }

            let mut value = String::new();
            if index < chars.len() && matches!(chars[index], '"' | '\'') {
                let quote = chars[index];
                index += 1;
                let mut terminated = false;
                while index < chars.len() {
                    let ch = chars[index];
                    if ch == quote {
                        index += 1;
                        terminated = true;
                        break;
                    }
                    if ch == '\\' {
                        if index + 1 < chars.len() && chars[index + 1] == '\n' {
                            index += 2;
                            continue;
                        }
                        if quote == '"' && index + 1 < chars.len() {
                            let escaped = chars[index + 1];
                            if matches!(escaped, '\\' | '"' | '$' | '`') {
                                value.push(escaped);
                                index += 2;
                                continue;
                            }
                        }
                    }
                    value.push(ch);
                    index += 1;
                }
                if !terminated {
                    value.insert(0, quote);
                }
            } else {
                while index < chars.len() {
                    let ch = chars[index];
                    if ch == '\\' && index + 1 < chars.len() && chars[index + 1] == '\n' {
                        index += 2;
                        continue;
                    }
                    if ch == '\n' || ch.is_whitespace() || ch == '#' {
                        break;
                    }
                    value.push(ch);
                    index += 1;
                }
            }

            vars.insert(key, value);

            while index < chars.len() && chars[index].is_whitespace() && chars[index] != '\n' {
                index += 1;
            }
            if index < chars.len() && chars[index] == '#' {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
        }

        vars
    }

    /// Query a fully-resolved portage variable via `portageq envvar`.
    ///
    /// This uses portage's own resolution logic, which handles:
    /// - `source` directives in `make.conf`
    /// - `${VAR}` variable expansion
    /// - Profile-inherited defaults (`make.defaults`, `use.force`, `use.mask`)
    /// - Parent profile chain
    ///
    /// Falls back with an error if `portageq` is unavailable (non-Gentoo host).
    fn portageq_envvar(var: &str) -> Result<String> {
        let output = std::process::Command::new("portageq")
            .args(["envvar", var])
            .output()
            .with_context(|| format!("Failed to run `portageq envvar {var}`"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`portageq envvar {var}` exited with {}: {stderr}",
                output.status
            );
        }

        Ok(String::from_utf8(output.stdout)
            .context("portageq output is not valid UTF-8")?
            .trim()
            .to_string())
    }

    /// Strip USE_EXPAND flags from a resolved USE flag list.
    ///
    /// `portageq envvar USE` returns every flag including expanded forms like
    /// `abi_x86_32`, `python_targets_python3_12`, `video_cards_amdgpu`, etc.
    /// These must be removed because the corresponding USE_EXPAND variables
    /// (`ABI_X86`, `PYTHON_TARGETS`, `VIDEO_CARDS`) are sent separately.
    /// Keeping them in USE would cause slot conflicts in the worker container
    /// (e.g. forcing ABI_X86="32 64" on a non-multilib container).
    ///
    /// We query `portageq envvar USE_EXPAND`, `USE_EXPAND_UNPREFIXED`, and
    /// `USE_EXPAND_HIDDEN` for the authoritative list of USE_EXPAND variable
    /// names, then filter any flag matching their lowercased prefix.
    fn strip_use_expand_flags(flags: Vec<String>) -> Vec<String> {
        // Collect prefixes from all USE_EXPAND variants.
        let mut prefixes = Vec::new();
        let mut any_succeeded = false;

        for var in ["USE_EXPAND", "USE_EXPAND_UNPREFIXED", "USE_EXPAND_HIDDEN"] {
            if let Ok(expand_str) = Self::portageq_envvar(var) {
                prefixes.extend(
                    expand_str
                        .split_whitespace()
                        .map(|v| format!("{}_", v.to_ascii_lowercase())),
                );
                any_succeeded = true;
            }
        }

        if !any_succeeded {
            // Fall back to a hardcoded list of common USE_EXPAND vars.
            warn!("Could not query any USE_EXPAND variant — using hardcoded fallback");
            prefixes = [
                "ABI_X86",
                "ABI_MIPS",
                "ABI_S390",
                "CPU_FLAGS_X86",
                "CPU_FLAGS_ARM",
                "PYTHON_TARGETS",
                "PYTHON_SINGLE_TARGET",
                "RUBY_TARGETS",
                "LUA_TARGETS",
                "LUA_SINGLE_TARGET",
                "LLVM_SLOT",
                "LLVM_TARGETS",
                "VIDEO_CARDS",
                "INPUT_DEVICES",
                "L10N",
            ]
            .iter()
            .map(|var| format!("{}_", var.to_ascii_lowercase()))
            .collect();
        }

        let before = flags.len();
        let filtered: Vec<String> = flags
            .into_iter()
            .filter(|flag| {
                !prefixes
                    .iter()
                    .any(|prefix| flag.starts_with(prefix.as_str()))
            })
            .collect();
        let stripped = before - filtered.len();
        if stripped > 0 {
            debug!(
                stripped,
                remaining = filtered.len(),
                "Stripped USE_EXPAND flags from resolved USE"
            );
        }
        filtered
    }
}

/// Split a portage package-version string into (name, optional version).
///
/// Uses the PMS rule: the version starts at the last `-` followed by a digit.
/// e.g. `openssl-3.1.4-r1` → `("openssl", Some("3.1.4-r1"))`
///       `openssl`          → `("openssl", None)`
///       `lib3ds-1.2`       → `("lib3ds", Some("1.2"))`
pub fn split_name_version(s: &str) -> (&str, Option<&str>) {
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            return (&s[..i], Some(&s[i + 1..]));
        }
    }
    (s, None)
}

/// Version comparison operator parsed from a portage atom.
#[derive(Debug, PartialEq, Eq)]
pub enum AtomOp {
    /// No operator — bare `category/package`.
    None,
    /// `=` — exact version match.
    Eq,
    /// `=....*` — version glob.
    EqGlob,
    /// `~` — same version, any revision.
    Tilde,
    /// `>=` — greater than or equal.
    Ge,
    /// `<=` — less than or equal.
    Le,
    /// `>` — strictly greater.
    Gt,
    /// `<` — strictly less.
    Lt,
}

/// Parse the version operator prefix from a portage atom.
///
/// Returns `(operator, remaining_atom)`.  For `=cat/pkg-1.2*` atoms the
/// glob star is left in the version and the operator is `EqGlob`.
pub fn parse_atom_operator(atom: &str) -> (AtomOp, &str) {
    if let Some(rest) = atom.strip_prefix(">=") {
        (AtomOp::Ge, rest)
    } else if let Some(rest) = atom.strip_prefix("<=") {
        (AtomOp::Le, rest)
    } else if let Some(rest) = atom.strip_prefix('=') {
        // Check for glob: =cat/pkg-1.2*
        if rest.ends_with('*') {
            (AtomOp::EqGlob, rest)
        } else {
            (AtomOp::Eq, rest)
        }
    } else if let Some(rest) = atom.strip_prefix('~') {
        (AtomOp::Tilde, rest)
    } else if let Some(rest) = atom.strip_prefix('>') {
        (AtomOp::Gt, rest)
    } else if let Some(rest) = atom.strip_prefix('<') {
        (AtomOp::Lt, rest)
    } else {
        (AtomOp::None, atom)
    }
}

/// Split a version string into (base_version, revision).
///
/// e.g. `3.1.4-r1` → `("3.1.4", Some("r1"))`
///       `3.1.4`   → `("3.1.4", None)`
pub fn split_revision(version: &str) -> (&str, Option<&str>) {
    // Revision is always the last `-rN` suffix.
    if let Some(pos) = version.rfind("-r") {
        let after = &version[pos + 2..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            return (&version[..pos], Some(&version[pos + 1..]));
        }
    }
    (version, None)
}

/// Compare two portage version strings per PMS rules.
///
/// Handles:
/// - Numeric component comparison (`1.9` < `1.10`)
/// - Trailing letter comparison (`1.1.1a` < `1.1.1z`)
/// - Suffix ordering (`_alpha` < `_beta` < `_pre` < `_rc` < (none) < `_p`)
/// - Revision comparison (`-r0` < `-r1`)
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (a_base, a_rev) = split_revision(a);
    let (b_base, b_rev) = split_revision(b);

    // Split base version into components on `.` and `_`.
    let a_parts: Vec<&str> = a_base.split(['.', '_']).collect();
    let b_parts: Vec<&str> = b_base.split(['.', '_']).collect();

    /// Identify a PMS suffix kind (e.g. "alpha", "rc", "p") with an optional
    /// numeric component, returning a `(kind_order, numeric_part)` tuple that
    /// can be compared directly.
    ///
    /// `kind_order` encodes precedence (alpha < beta < pre < rc < none < p),
    /// and `numeric_part` refines the order within the same kind:
    /// e.g. alpha0 < alpha1 < alpha2, p1 < p2, p20230101 < p20240101.
    fn suffix_kind_with_number(s: &str, prefix: &str, kind: i32) -> Option<(i32, u64)> {
        if !s.starts_with(prefix) {
            return None;
        }
        let rest = &s[prefix.len()..];
        if rest.is_empty() {
            // Unnumbered suffix — treat as kind0.
            return Some((kind, 0));
        }
        // Only a PMS suffix with a number if the remainder is all digits.
        if !rest.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let num: u64 = match rest.parse() {
            Ok(n) => n,
            Err(_) => return None,
        };
        Some((kind, num))
    }

    // Returns `Some((kind_order, numeric_part))` for PMS suffixes, or
    // `None` for a plain version component that is not a suffix keyword.
    let suffix_order = |s: &str| -> Option<(i32, u64)> {
        // PMS suffix ordering (kind, then numeric): _alpha < _beta < _pre < _rc
        // < (no suffix) < _p.  Unnumbered forms like "_alpha" are treated as
        // "_alpha0"; numbered forms like "_alpha1" increment within the kind.
        suffix_kind_with_number(s, "alpha", -4)
            .or_else(|| suffix_kind_with_number(s, "beta", -3))
            .or_else(|| suffix_kind_with_number(s, "pre", -2))
            .or_else(|| suffix_kind_with_number(s, "rc", -1))
            .or_else(|| suffix_kind_with_number(s, "p", 1))
    };

    /// Split a version component into its numeric prefix and optional
    /// trailing letter (PMS §3.2).  e.g. `"1w"` → `(Some(1), Some('w'))`.
    fn split_numeric_letter(s: &str) -> (Option<u64>, Option<char>) {
        let num_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let num = if num_end > 0 {
            s[..num_end].parse::<u64>().ok()
        } else {
            None
        };
        let letter = s[num_end..]
            .chars()
            .next()
            .filter(|c| c.is_ascii_lowercase());
        (num, letter)
    }

    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let pa = a_parts.get(i).copied().unwrap_or("");
        let pb = b_parts.get(i).copied().unwrap_or("");

        // Check for PMS suffixes first.
        let sa = suffix_order(pa);
        let sb = suffix_order(pb);

        match (sa, sb) {
            (Some(a_key), Some(b_key)) => {
                // Both are PMS suffix components — compare as tuples.
                let cmp = a_key.cmp(&b_key);
                if cmp != Ordering::Equal {
                    return cmp;
                }
                continue;
            }
            (Some(a_key), None) => {
                // `pa` is a suffix, `pb` is a plain component: the suffix
                // modifies the preceding numeric part.  Negative kind_order
                // means the suffix makes the version smaller than the plain
                // component (e.g. `1.0_rc` < `1.0`); positive means larger
                // (e.g. `1.0_p` > `1.0`).
                return a_key.0.cmp(&0i32);
            }
            (None, Some(b_key)) => {
                return 0i32.cmp(&b_key.0);
            }
            (None, None) => {
                // Neither is a suffix — fall through to numeric/letter comparison.
            }
        }

        // Split each component into numeric + optional trailing letter.
        let (na, la) = split_numeric_letter(pa);
        let (nb, lb) = split_numeric_letter(pb);

        match (na, nb) {
            (Some(a_num), Some(b_num)) => {
                let cmp = a_num.cmp(&b_num);
                if cmp != Ordering::Equal {
                    return cmp;
                }
                // Numeric parts equal — compare trailing letters.
                // No letter < letter (PMS: `1.1.1` < `1.1.1a`).
                match (la, lb) {
                    (None, None) => {}
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(a_c), Some(b_c)) => {
                        let cmp = a_c.cmp(&b_c);
                        if cmp != Ordering::Equal {
                            return cmp;
                        }
                    }
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => {
                // Both non-numeric, non-suffix — lexicographic.
                let cmp = pa.cmp(pb);
                if cmp != Ordering::Equal {
                    return cmp;
                }
            }
        }
    }

    // Base versions are equal — compare revisions.
    let ra = a_rev
        .and_then(|r| r.strip_prefix('r'))
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    let rb = b_rev
        .and_then(|r| r.strip_prefix('r'))
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    ra.cmp(&rb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_make_conf_variables() {
        let input = r#"
# Compiler flags
CFLAGS="-O2 -pipe -march=native"
CXXFLAGS="${CFLAGS}"
MAKEOPTS="-j12"
USE="X wayland -systemd pulseaudio"
ACCEPT_KEYWORDS="~amd64"
VIDEO_CARDS="amdgpu radeonsi"
"#;
        let vars = PortageReader::parse_make_conf_vars(input);
        assert_eq!(vars["CFLAGS"], "-O2 -pipe -march=native");
        assert_eq!(vars["CXXFLAGS"], "${CFLAGS}");
        assert_eq!(vars["MAKEOPTS"], "-j12");
        assert_eq!(vars["USE"], "X wayland -systemd pulseaudio");
        assert_eq!(vars["ACCEPT_KEYWORDS"], "~amd64");
        assert_eq!(vars["VIDEO_CARDS"], "amdgpu radeonsi");
    }

    #[test]
    fn parse_continuation_lines() {
        let input = "USE=\"foo \\\nbar \\\nbaz\"";
        let vars = PortageReader::parse_make_conf_vars(input);
        assert_eq!(vars["USE"], "foo bar baz");
    }

    #[test]
    fn parse_make_conf_vars_does_not_panic_on_single_quote_value() {
        let vars = PortageReader::parse_make_conf_vars("BROKEN='");
        assert_eq!(vars["BROKEN"], "'");
    }

    #[test]
    fn parse_make_conf_vars_supports_multiline_quoted_values() {
        let vars = PortageReader::parse_make_conf_vars(
            "FEATURES=\"\nparallel-fetch\nparallel-install\ncandy\n\"\nUSE=\"\nwayland\nssl\n\"\n",
        );

        assert_eq!(
            vars["FEATURES"],
            "\nparallel-fetch\nparallel-install\ncandy\n"
        );
        assert_eq!(vars["USE"], "\nwayland\nssl\n");
    }

    #[test]
    fn parse_make_conf_vars_supports_multiple_assignments_on_one_line() {
        let vars = PortageReader::parse_make_conf_vars(
            "COMMON_FLAGS=\"-O2 -pipe\" CFLAGS=\"${COMMON_FLAGS}\" MAKEOPTS=\"-j8\"",
        );

        assert_eq!(vars["COMMON_FLAGS"], "-O2 -pipe");
        assert_eq!(vars["CFLAGS"], "${COMMON_FLAGS}");
        assert_eq!(vars["MAKEOPTS"], "-j8");
    }

    #[test]
    fn split_name_version_with_version() {
        assert_eq!(
            split_name_version("openssl-3.1.4"),
            ("openssl", Some("3.1.4"))
        );
    }

    #[test]
    fn split_name_version_with_revision() {
        assert_eq!(
            split_name_version("openssl-3.1.4-r1"),
            ("openssl", Some("3.1.4-r1"))
        );
    }

    #[test]
    fn split_name_version_no_version() {
        assert_eq!(split_name_version("openssl"), ("openssl", None));
    }

    #[test]
    fn split_name_version_numeric_name() {
        assert_eq!(split_name_version("lib3ds-1.2"), ("lib3ds", Some("1.2")));
    }

    // ── Version comparison tests ─────────────────────────────────

    #[test]
    fn compare_versions_basic() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0", "1.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "2.0"), Ordering::Less);
        assert_eq!(compare_versions("2.0", "1.0"), Ordering::Greater);
    }

    #[test]
    fn compare_versions_numeric_not_lexicographic() {
        use std::cmp::Ordering;
        // 1.10 > 1.9 numerically, but < lexicographically.
        assert_eq!(compare_versions("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.2", "1.10"), Ordering::Less);
    }

    #[test]
    fn compare_versions_different_depth() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.2.3", "1.2"), Ordering::Greater);
        assert_eq!(compare_versions("1.2", "1.2.1"), Ordering::Less);
    }

    #[test]
    fn compare_versions_revisions() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("3.1.4-r1", "3.1.4"), Ordering::Greater);
        assert_eq!(compare_versions("3.1.4-r1", "3.1.4-r2"), Ordering::Less);
        assert_eq!(compare_versions("3.1.4-r1", "3.1.4-r1"), Ordering::Equal);
    }

    #[test]
    fn compare_versions_suffixes() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0_alpha", "1.0_beta"), Ordering::Less);
        assert_eq!(compare_versions("1.0_beta", "1.0_pre"), Ordering::Less);
        assert_eq!(compare_versions("1.0_pre", "1.0_rc"), Ordering::Less);
        assert_eq!(compare_versions("1.0_rc", "1.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0", "1.0_p"), Ordering::Less);
    }

    #[test]
    fn compare_versions_numeric_suffixes() {
        use std::cmp::Ordering;
        // Unnumbered suffix is treated as number 0:
        // _alpha == _alpha0 < _alpha1 < _alpha2 < _beta
        assert_eq!(compare_versions("1.0_alpha", "1.0_alpha0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0_alpha", "1.0_alpha1"), Ordering::Less);
        assert_eq!(compare_versions("1.0_alpha1", "1.0_alpha2"), Ordering::Less);
        assert_eq!(compare_versions("1.0_alpha2", "1.0_beta"), Ordering::Less);
        // _rc1 < _rc2 < release < _p1 < _p2
        assert_eq!(compare_versions("1.0_rc1", "1.0_rc2"), Ordering::Less);
        assert_eq!(compare_versions("1.0_rc2", "1.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0_p1", "1.0_p2"), Ordering::Less);
        assert_eq!(compare_versions("1.0", "1.0_p1"), Ordering::Less);
        // Date-style _p suffix (common for gentoo-kernel-bin etc.)
        assert_eq!(
            compare_versions("1.0_p20230101", "1.0_p20240101"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0_p20240101", "1.0_p20240101"),
            Ordering::Equal
        );
    }

    #[test]
    fn compare_versions_trailing_letter() {
        use std::cmp::Ordering;
        // PMS §3.2: trailing letter sorts after bare version.
        assert_eq!(compare_versions("1.1.1", "1.1.1a"), Ordering::Less);
        assert_eq!(compare_versions("1.1.1a", "1.1.1b"), Ordering::Less);
        assert_eq!(compare_versions("1.1.1w", "1.1.1z"), Ordering::Less);
        assert_eq!(compare_versions("1.1.1z", "1.1.2"), Ordering::Less);
        // Typical openssl version: 1.1.1w
        assert_eq!(compare_versions("1.1.1w", "1.1.1w"), Ordering::Equal);
        assert_eq!(compare_versions("1.1.1w", "3.1.0"), Ordering::Less);
    }

    #[test]
    fn split_revision_works() {
        assert_eq!(split_revision("3.1.4-r1"), ("3.1.4", Some("r1")));
        assert_eq!(split_revision("3.1.4"), ("3.1.4", None));
        assert_eq!(split_revision("3.1.4-r0"), ("3.1.4", Some("r0")));
    }

    #[test]
    fn parse_atom_operator_works() {
        assert_eq!(
            parse_atom_operator("dev-libs/foo"),
            (AtomOp::None, "dev-libs/foo")
        );
        assert_eq!(
            parse_atom_operator("=dev-libs/foo-1.0"),
            (AtomOp::Eq, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator(">=dev-libs/foo-1.0"),
            (AtomOp::Ge, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator("<=dev-libs/foo-1.0"),
            (AtomOp::Le, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator(">dev-libs/foo-1.0"),
            (AtomOp::Gt, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator("<dev-libs/foo-1.0"),
            (AtomOp::Lt, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator("~dev-libs/foo-1.0"),
            (AtomOp::Tilde, "dev-libs/foo-1.0")
        );
        assert_eq!(
            parse_atom_operator("=dev-libs/foo-1.0*"),
            (AtomOp::EqGlob, "dev-libs/foo-1.0*")
        );
    }

    #[test]
    fn parse_manifest_distfiles_allows_literal_double_dot_basenames() {
        let distfiles = PortageReader::parse_manifest_distfiles(
            "DIST foo..bar.tar.xz 123 BLAKE2B deadbeef SHA512 cafe\nDIST ../escape.tar.xz 1 BLAKE2B dead SHA512 beef\nDIST nested/file.tar.xz 1 BLAKE2B dead SHA512 beef\nDIST hello-1.0.tar.xz 123 BLAKE2B deadbeef SHA512 cafe\n",
        );

        assert_eq!(
            distfiles,
            vec![
                "foo..bar.tar.xz".to_string(),
                "hello-1.0.tar.xz".to_string()
            ]
        );
    }
}
