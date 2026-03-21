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
use std::path::PathBuf;

use anyhow::{Context, Result};
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
        let package_accept_keywords = self.read_package_accept_keywords()?;
        let package_license = self.read_package_license()?;
        let package_mask = self.read_package_mask()?;
        let package_unmask = self.read_package_unmask()?;
        let package_env = self.read_package_env()?;
        let env_files = self.read_env_files()?;
        let repos_conf = self.read_repos_conf()?;
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
        let path = self.root.join("etc/portage/make.conf");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let vars = Self::parse_shell_vars(&content);

        let split_flags = |key: &str| -> Vec<String> {
            vars.get(key)
                .map(|v| v.split_whitespace().map(String::from).collect())
                .unwrap_or_default()
        };

        let get = |key: &str| -> String { vars.get(key).cloned().unwrap_or_default() };

        // ── Resolve native flags ─────────────────────────────────────
        let resolved =
            cflags::resolve_native_flags().context("Failed to resolve native compiler flags")?;

        let raw_cflags = get("CFLAGS");
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
        let raw_cxxflags = get("CXXFLAGS");
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
        let chost = vars.get("CHOST").cloned().unwrap_or(resolved.chost);

        // ── Resolve USE flags ────────────────────────────────────────
        // Profile-inherited flags (e.g. `dbus` from the desktop profile)
        // are NOT present in make.conf.  Use `portageq envvar USE` to get
        // the fully merged value (profile defaults + make.conf + profile
        // force/mask).
        let (use_flags, use_flags_resolved) = match Self::portageq_envvar("USE") {
            Ok(resolved_use) => {
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
        let use_expand_keys: Vec<String> = match Self::portageq_envvar("USE_EXPAND") {
            Ok(expand_str) => {
                let keys: Vec<String> =
                    expand_str.split_whitespace().map(String::from).collect();
                info!(
                    count = keys.len(),
                    "Discovered USE_EXPAND variables via portageq"
                );
                keys
            }
            Err(e) => {
                warn!(
                    %e,
                    "Failed to query USE_EXPAND — using hardcoded fallback"
                );
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
            let vals = match Self::portageq_envvar(key) {
                Ok(resolved) => {
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
            ldflags: get("LDFLAGS"),
            makeopts: get("MAKEOPTS"),
            use_flags,
            features: split_flags("FEATURES"),
            accept_license: get("ACCEPT_LICENSE"),
            accept_keywords: get("ACCEPT_KEYWORDS"),
            emerge_default_opts: get("EMERGE_DEFAULT_OPTS"),
            chost,
            cpu_flags: resolved.cpu_flags,
            original_cflags,
            use_expand,
            extra,
            use_flags_resolved,
        })
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
    fn read_package_accept_keywords(&self) -> Result<Vec<PackageKeywordEntry>> {
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
                    // No explicit keyword = ~ARCH
                    vec!["~*".to_string()]
                },
            })
        })
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

    /// Generic reader for `/etc/portage/<name>` which may be a file or directory.
    fn read_package_entries<T>(
        &self,
        name: &str,
        parser: impl Fn(&str) -> Option<T>,
    ) -> Result<Vec<T>> {
        let path = self.root.join("etc/portage").join(name);
        let mut entries = Vec::new();

        if path.is_dir() {
            // Read all files in the directory (non-recursive, skip hidden).
            if let Ok(dir) = fs::read_dir(&path) {
                for entry in dir.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        let fname = entry.file_name();
                        if !fname.to_string_lossy().starts_with('.')
                            && let Ok(content) = fs::read_to_string(entry.path())
                        {
                            Self::parse_lines(&content, &parser, &mut entries);
                        }
                    }
                }
            }
        } else if path.is_file() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            Self::parse_lines(&content, &parser, &mut entries);
        } else {
            debug!("{} does not exist — skipping", path.display());
        }

        Ok(entries)
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
    /// Checks `/var/db/pkg/<category>/<name>*/` to determine if the package
    /// is installed.  This is a rough heuristic — it does not evaluate version
    /// constraints, only whether *some* version of the package is present.
    ///
    /// Package sets (`@world`, `@system`) are never considered "installed"
    /// for this check — they always pass through.
    pub fn is_installed(&self, atom: &str) -> bool {
        // Sets are never "installed" in the traditional sense.
        if atom.starts_with('@') {
            return false;
        }

        // Strip version operator prefix.
        let stripped = atom.trim_start_matches(|c: char| ">=<~!".contains(c));

        let Some((category, name_maybe_version)) = stripped.split_once('/') else {
            return false;
        };

        // Extract just the package name (strip version).
        let pkg_name = split_name_version(name_maybe_version).0;

        let pkg_dir = self.root.join("var/db/pkg").join(category);
        let Ok(entries) = std::fs::read_dir(&pkg_dir) else {
            return false;
        };

        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            let installed_name = split_name_version(&fname_str).0;
            if installed_name == pkg_name {
                return true;
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
            let vars = Self::parse_shell_vars(&content);
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
            let vars = Self::parse_shell_vars(&content);
            if let Some(targets) = vars.get("PYTHON_TARGETS") {
                return targets.split_whitespace().map(String::from).collect();
            }
        }
        Vec::new()
    }

    /// Minimalist shell variable parser for make.conf.
    ///
    /// Handles `VAR="value"` and `VAR='value'` and `VAR=value` forms.
    /// Does NOT handle multi-line values, command substitution, etc.
    ///
    /// For variables that need full resolution (including `source` directives
    /// and profile inheritance), use [`portageq_envvar`] instead.
    fn parse_shell_vars(content: &str) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();

        // Join continuation lines (backslash-newline).
        let joined = content.replace("\\\n", " ");

        for line in joined.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Look for KEY=VALUE.
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                if key.contains(' ') || key.is_empty() {
                    continue; // Not a simple assignment.
                }
                let val = line[eq_pos + 1..].trim();

                // Strip surrounding quotes.
                let val = if (val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\''))
                {
                    &val[1..val.len() - 1]
                } else {
                    val
                };

                vars.insert(key.to_string(), val.to_string());
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
    /// We query `portageq envvar USE_EXPAND` for the authoritative list of
    /// USE_EXPAND variable names, then filter any flag matching their
    /// lowercased prefix.
    fn strip_use_expand_flags(flags: Vec<String>) -> Vec<String> {
        // Get the list of USE_EXPAND variables from portage.
        let prefixes: Vec<String> = match Self::portageq_envvar("USE_EXPAND") {
            Ok(expand_str) => expand_str
                .split_whitespace()
                .map(|var| format!("{}_", var.to_ascii_lowercase()))
                .collect(),
            Err(e) => {
                // Fall back to a hardcoded list of common USE_EXPAND vars.
                warn!(%e, "Could not query USE_EXPAND — using hardcoded fallback");
                [
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
                    "VIDEO_CARDS",
                    "INPUT_DEVICES",
                    "L10N",
                ]
                .iter()
                .map(|var| format!("{}_", var.to_ascii_lowercase()))
                .collect()
            }
        };

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
fn split_name_version(s: &str) -> (&str, Option<&str>) {
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            return (&s[..i], Some(&s[i + 1..]));
        }
    }
    (s, None)
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
        let vars = PortageReader::parse_shell_vars(input);
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
        let vars = PortageReader::parse_shell_vars(input);
        assert_eq!(vars["USE"], "foo  bar  baz");
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
}
