//! CLI argument parsing.
//!
//! `remerge` accepts the exact same arguments as `emerge`.  It intercepts them,
//! builds a workorder, and after the remote build completes runs `emerge`
//! locally with `--getbinpkg` so that pre-built packages are used.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::future::Future;
use std::io::IsTerminal;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::client::RemergeClient;
use crate::config::{self, CliConfig};
use crate::portage::PortageReader;
use crate::status_bar::StatusBar;
use crate::verbosity::Verbosity;
use remerge_types::client::ClientRole;
use remerge_types::portage::{PortageConfig, SnapshotEntry};
use remerge_types::validation::validate_atom;
use remerge_types::workorder::{
    BuiltPackage, ParityDirectoryEntry, ParityFileEntry, ParityManifest, ParitySymlinkEntry,
    WorkorderResult,
};

pub async fn reconcile_final_state_parity_into(
    parity_root: &Path,
    client: &RemergeClient,
    result: &WorkorderResult,
) -> Result<()> {
    if result.parity_manifest.files.is_empty()
        && result.parity_manifest.directories.is_empty()
        && result.parity_manifest.symlinks.is_empty()
    {
        return Ok(());
    }

    println!(
        "Reconciling final-state parity for {} file(s), {} director(ies), and {} symlink(s)…",
        result.parity_manifest.files.len(),
        result.parity_manifest.directories.len(),
        result.parity_manifest.symlinks.len()
    );

    let mut issues = Vec::new();

    for relative_path in result.parity_manifest.directories.keys() {
        if let Err(error) = ensure_parity_directory_exists(parity_root, relative_path).await {
            issues.push(error.to_string());
        }
    }

    for (relative_path, entry) in &result.parity_manifest.symlinks {
        if let Err(error) =
            restore_single_parity_symlink(client, parity_root, relative_path, entry).await
        {
            issues.push(error.to_string());
        }
    }

    for (relative_path, entry) in &result.parity_manifest.files {
        if let Err(error) =
            restore_single_parity_file(client, parity_root, relative_path, entry).await
        {
            issues.push(error.to_string());
        }
    }

    for (relative_path, entry) in &result.parity_manifest.directories {
        if let Err(error) = finalize_parity_directory_mtime(parity_root, relative_path, entry).await
        {
            issues.push(error.to_string());
        }
    }

    if issues.is_empty()
        && let Err(mut verification_issues) =
            verify_parity_manifest(parity_root, &result.parity_manifest).await
    {
        issues.append(&mut verification_issues);
    }

    if !issues.is_empty() {
        println!("Final-state parity reconciliation failed:");
        for issue in &issues {
            println!("  - {issue}");
        }
        anyhow::bail!(
            "Final-state parity reconciliation failed for {} path(s): {}",
            issues.len(),
            issues.join("; ")
        );
    }

    println!("Final-state parity reconciled in {}", parity_root.display());
    Ok(())
}

pub async fn reconcile_fetched_distfiles_into(
    distdir: &Path,
    client: &RemergeClient,
    result: &WorkorderResult,
) -> Result<()> {
    if result.fetched_distfiles.is_empty() {
        return Ok(());
    }

    println!(
        "Reconciling fetched distfiles for {} file(s)…",
        result.fetched_distfiles.len()
    );

    let mut issues = Vec::new();
    for (relative_path, entry) in &result.fetched_distfiles {
        if let Err(error) =
            restore_single_snapshot_file(client, distdir, relative_path, entry, "distfile").await
        {
            issues.push(error.to_string());
        }
    }

    if !issues.is_empty() {
        println!("Fetched distfile reconciliation failed:");
        for issue in &issues {
            println!("  - {issue}");
        }
        anyhow::bail!(
            "Fetched distfile reconciliation failed for {} path(s): {}",
            issues.len(),
            issues.join("; ")
        );
    }

    println!("Fetched distfiles reconciled in {}", distdir.display());
    Ok(())
}

pub async fn run_local_emerge_with_program(
    program: &Path,
    args: &[String],
    binhost_uri: Option<&str>,
) -> Result<()> {
    use tokio::process::Command;

    let mut command = Command::new(program);
    command.args(args);

    if let Some(portage_binhost) = portage_binhost_env(binhost_uri) {
        command.env("PORTAGE_BINHOST", portage_binhost);
    }

    let status = command.status().await.context("Failed to execute emerge")?;

    if !status.success() {
        anyhow::bail!("emerge exited with status {}", status);
    }
    Ok(())
}

async fn verify_parity_manifest(
    parity_root: &Path,
    manifest: &ParityManifest,
) -> Result<(), Vec<String>> {
    let mut issues = Vec::new();

    for (relative_path, entry) in &manifest.directories {
        match resolve_parity_target(parity_root, relative_path)
            .and_then(|target| parity_directory_matches(&target, entry))
        {
            Ok(true) => {}
            Ok(false) => issues.push(format!(
                "mismatched parity directory {relative_path}: verification failed after restore"
            )),
            Err(error) => issues.push(error.to_string()),
        }
    }

    for (relative_path, entry) in &manifest.symlinks {
        match resolve_parity_target(parity_root, relative_path)
            .and_then(|target| parity_symlink_matches(&target, entry))
        {
            Ok(true) => {}
            Ok(false) => issues.push(format!(
                "mismatched parity symlink {relative_path}: verification failed after restore"
            )),
            Err(error) => issues.push(error.to_string()),
        }
    }

    for (relative_path, entry) in &manifest.files {
        let outcome = match resolve_parity_target(parity_root, relative_path) {
            Ok(target) => parity_file_matches(&target, entry).await,
            Err(error) => Err(error),
        };

        match outcome {
            Ok(true) => {}
            Ok(false) => issues.push(format!(
                "mismatched parity path {relative_path}: verification failed after restore"
            )),
            Err(error) => issues.push(error.to_string()),
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

async fn snapshot_file_matches(target: &Path, entry: &SnapshotEntry) -> Result<bool> {
    let metadata = match tokio::fs::metadata(target).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", target.display()));
        }
    };

    if !metadata.is_file() || metadata.len() != entry.size || metadata.mtime() != entry.mtime_secs {
        return Ok(false);
    }

    Ok(sha256_file(target).await? == entry.digest)
}

async fn ensure_parity_directory_exists(parity_root: &Path, relative_path: &str) -> Result<()> {
    let target = resolve_parity_target(parity_root, relative_path)
        .with_context(|| format!("excluded parity directory {relative_path}"))?;
    match tokio::fs::symlink_metadata(&target).await {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => anyhow::bail!(
            "mismatched parity directory {relative_path}: existing path is not a directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(&target)
                .await
                .with_context(|| format!("Failed to create {}", target.display()))
        }
        Err(error) => Err(error).with_context(|| format!("Failed to stat {}", target.display())),
    }
}

async fn finalize_parity_directory_mtime(
    parity_root: &Path,
    relative_path: &str,
    entry: &ParityDirectoryEntry,
) -> Result<()> {
    let target = resolve_parity_target(parity_root, relative_path)
        .with_context(|| format!("excluded parity directory {relative_path}"))?;
    let metadata = tokio::fs::symlink_metadata(&target)
        .await
        .with_context(|| format!("Failed to stat {}", target.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!(
            "mismatched parity directory {relative_path}: existing path is not a directory"
        );
    }
    set_file_mtime(&target, entry.mtime_secs)
}

async fn restore_single_parity_file(
    client: &RemergeClient,
    parity_root: &Path,
    relative_path: &str,
    entry: &ParityFileEntry,
) -> Result<()> {
    let target = match resolve_parity_target(parity_root, relative_path) {
        Ok(target) => target,
        Err(error) => {
            anyhow::bail!("excluded parity path {relative_path}: {error}");
        }
    };
    if parity_file_matches(&target, entry).await? {
        return Ok(());
    }

    let temporary = target.with_extension("remerge-parity.part");
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    client
        .download_blob(&entry.digest, &temporary)
        .await
        .with_context(|| format!("mismatched parity path {relative_path}: download failed"))?;

    let metadata = tokio::fs::metadata(&temporary)
        .await
        .with_context(|| format!("Failed to stat {}", temporary.display()))?;
    if metadata.len() != entry.size {
        anyhow::bail!(
            "mismatched parity path {relative_path}: downloaded {} byte(s) but expected {}",
            metadata.len(),
            entry.size,
        );
    }

    let digest = sha256_file(&temporary).await?;
    if digest != entry.digest {
        anyhow::bail!(
            "mismatched parity path {relative_path}: expected digest {}, got {}",
            entry.digest,
            digest
        );
    }

    tokio::fs::rename(&temporary, &target)
        .await
        .with_context(|| {
            format!(
                "Failed to move {} into {}",
                temporary.display(),
                target.display()
            )
        })?;
    set_file_mtime(&target, entry.mtime_secs)?;

    if !parity_file_matches(&target, entry).await? {
        anyhow::bail!("mismatched parity path {relative_path}: verification failed after restore");
    }

    Ok(())
}

async fn restore_single_snapshot_file(
    client: &RemergeClient,
    root: &Path,
    relative_path: &str,
    entry: &SnapshotEntry,
    label: &str,
) -> Result<()> {
    let target = match resolve_relative_target(root, relative_path) {
        Ok(target) => target,
        Err(error) => {
            anyhow::bail!("invalid {label} path {relative_path}: {error}");
        }
    };
    if snapshot_file_matches(&target, entry).await? {
        return Ok(());
    }

    let temporary = target.with_extension(format!("remerge-{label}.part"));
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    client
        .download_blob(&entry.digest, &temporary)
        .await
        .with_context(|| format!("mismatched {label} path {relative_path}: download failed"))?;

    let metadata = tokio::fs::metadata(&temporary)
        .await
        .with_context(|| format!("Failed to stat {}", temporary.display()))?;
    if metadata.len() != entry.size {
        anyhow::bail!(
            "mismatched {label} path {relative_path}: downloaded {} byte(s) but expected {}",
            metadata.len(),
            entry.size,
        );
    }

    let digest = sha256_file(&temporary).await?;
    if digest != entry.digest {
        anyhow::bail!(
            "mismatched {label} path {relative_path}: expected digest {}, got {}",
            entry.digest,
            digest
        );
    }

    tokio::fs::rename(&temporary, &target)
        .await
        .with_context(|| {
            format!(
                "Failed to move {} into {}",
                temporary.display(),
                target.display()
            )
        })?;
    set_file_mtime(&target, entry.mtime_secs)?;

    if !snapshot_file_matches(&target, entry).await? {
        anyhow::bail!("mismatched {label} path {relative_path}: verification failed after restore");
    }

    Ok(())
}

async fn restore_single_parity_symlink(
    client: &RemergeClient,
    parity_root: &Path,
    relative_path: &str,
    entry: &ParitySymlinkEntry,
) -> Result<()> {
    let target = match resolve_parity_target(parity_root, relative_path) {
        Ok(target) => target,
        Err(error) => {
            anyhow::bail!("excluded parity symlink {relative_path}: {error}");
        }
    };
    if parity_symlink_matches(&target, entry)? {
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let temporary = target.with_extension("remerge-parity-symlink.part");
    client
        .download_blob(&entry.digest, &temporary)
        .await
        .with_context(|| format!("mismatched parity symlink {relative_path}: download failed"))?;

    let metadata = tokio::fs::metadata(&temporary)
        .await
        .with_context(|| format!("Failed to stat {}", temporary.display()))?;
    if metadata.len() != entry.size {
        anyhow::bail!(
            "mismatched parity symlink {relative_path}: downloaded {} byte(s) but expected {}",
            metadata.len(),
            entry.size,
        );
    }

    let digest = sha256_file(&temporary).await?;
    if digest != entry.digest {
        anyhow::bail!(
            "mismatched parity symlink {relative_path}: expected digest {}, got {}",
            entry.digest,
            digest
        );
    }

    let target_bytes = tokio::fs::read(&temporary)
        .await
        .with_context(|| format!("Failed to read {}", temporary.display()))?;
    tokio::fs::remove_file(&temporary)
        .await
        .with_context(|| format!("Failed to remove {}", temporary.display()))?;

    match tokio::fs::symlink_metadata(&target).await {
        Ok(existing) if existing.is_dir() => {
            anyhow::bail!(
                "mismatched parity symlink {relative_path}: existing path is a directory"
            );
        }
        Ok(_) => tokio::fs::remove_file(&target)
            .await
            .with_context(|| format!("Failed to remove {}", target.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", target.display()));
        }
    }

    let temporary_link = target.with_extension("remerge-parity-symlink.link");
    let _ = tokio::fs::remove_file(&temporary_link).await;
    std::os::unix::fs::symlink(OsString::from_vec(target_bytes), &temporary_link)
        .with_context(|| format!("Failed to create {}", temporary_link.display()))?;
    tokio::fs::rename(&temporary_link, &target)
        .await
        .with_context(|| {
            format!(
                "Failed to move {} into {}",
                temporary_link.display(),
                target.display()
            )
        })?;
    set_symlink_mtime(&target, entry.mtime_secs)
}

fn resolve_parity_target(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let sanitized = sanitize_binpkg_path(relative_path)?;
    if !is_approved_parity_path(&sanitized) {
        anyhow::bail!("Parity path is outside the approved include set: {relative_path}");
    }
    Ok(if root == Path::new("/") {
        Path::new("/").join(sanitized)
    } else {
        root.join(sanitized)
    })
}

fn resolve_relative_target(root: &Path, relative_path: &str) -> Result<PathBuf> {
    let sanitized = sanitize_binpkg_path(relative_path)?;
    Ok(if root == Path::new("/") {
        Path::new("/").join(sanitized)
    } else {
        root.join(sanitized)
    })
}

fn is_approved_parity_path(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .map(|component| match component {
            Component::Normal(segment) => segment.to_string_lossy().into_owned(),
            _ => String::new(),
        })
        .collect();
    let components: Vec<&str> = components.iter().map(String::as_str).collect();

    matches!(
        components.as_slice(),
        ["var", "cache", "binpkgs", "Packages"]
            | ["var", "cache", "eclass"]
            | ["var", "cache", "eclass", _, ..]
            | ["var", "db", "repos", _, "Packages"]
            | ["var", "db", "repos", _, "metadata"]
            | ["var", "db", "repos", _, "metadata", _, ..]
            | ["var", "lib", "portage"]
            | ["var", "lib", "portage", _, ..]
    )
}

fn parity_directory_matches(path: &Path, entry: &ParityDirectoryEntry) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
        }
    };

    Ok(metadata.is_dir() && metadata.mtime() == entry.mtime_secs)
}

fn parity_symlink_matches(path: &Path, entry: &ParitySymlinkEntry) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
        }
    };
    if !metadata.file_type().is_symlink() || metadata.mtime() != entry.mtime_secs {
        return Ok(false);
    }

    let target = std::fs::read_link(path)
        .with_context(|| format!("Failed to read symlink {}", path.display()))?;
    let target_bytes = target.as_os_str().as_bytes();
    if target_bytes.len() as u64 != entry.size {
        return Ok(false);
    }

    Ok(hex::encode(Sha256::digest(target_bytes)) == entry.digest)
}

async fn parity_file_matches(path: &Path, entry: &ParityFileEntry) -> Result<bool> {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
        }
    };

    if metadata.len() != entry.size || metadata.mtime() != entry.mtime_secs {
        return Ok(false);
    }

    Ok(sha256_file(path).await? == entry.digest)
}

fn set_file_mtime(path: &Path, mtime_secs: i64) -> Result<()> {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(mtime_secs, 0))
        .with_context(|| format!("Failed to set mtime on {}", path.display()))
}

fn set_symlink_mtime(path: &Path, mtime_secs: i64) -> Result<()> {
    let file_time = filetime::FileTime::from_unix_time(mtime_secs, 0);
    filetime::set_symlink_file_times(path, file_time, file_time)
        .with_context(|| format!("Failed to set symlink mtime on {}", path.display()))
}

fn portage_binhost_env(binhost_uri: Option<&str>) -> Option<String> {
    let uri = binhost_uri?.trim();
    if uri.is_empty() {
        None
    } else {
        Some(uri.to_string())
    }
}

fn sanitize_binpkg_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        anyhow::bail!("absolute paths are not allowed");
    }
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => sanitized.push(segment),
            _ => anyhow::bail!("path traversal is not allowed"),
        }
    }
    if sanitized.as_os_str().is_empty() {
        anyhow::bail!("empty paths are not allowed");
    }
    Ok(sanitized)
}

async fn sha256_file(path: &Path) -> Result<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }

    Ok(hex::encode(digest.finalize()))
}

/// Extract package atoms from emerge arguments.
///
/// Anything that doesn't start with `-` is treated as a potential atom.
/// This covers qualified (`www-client/firefox`), unqualified (`firefox`),
/// versioned (`=www-client/firefox-128.0`), and set (`@world`) forms.
/// Emerge itself will reject truly invalid atoms.
pub fn extract_package_atoms(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect()
}

/// remerge — distributed Gentoo binary host builder.
///
/// Drop-in wrapper for `emerge`.  Forwards your arguments to a remote build
/// worker, waits for binary packages, then runs `emerge` locally with
/// `--getbinpkg` to install them.
#[derive(Parser, Debug)]
#[command(
    name = "remerge",
    version,
    about = "Distributed Gentoo binary host builder",
    long_about = None,
    // Allow arbitrary trailing arguments so we can forward them to emerge.
    trailing_var_arg = true,
)]
pub struct Cli {
    /// URL of the remerge server (overrides /etc/remerge.conf).
    #[arg(long, env = "REMERGE_SERVER")]
    server: Option<String>,

    /// Override the client ID from the config file.
    ///
    /// Use this to explicitly set a client ID, e.g. to share an identity
    /// across multiple machines.
    #[arg(long, env = "REMERGE_CLIENT_ID")]
    client_id: Option<uuid::Uuid>,

    /// Client role: `main` (default) or `follower`.
    ///
    /// Followers share the main client's portage config and cannot push
    /// configuration changes.
    #[arg(long, env = "REMERGE_ROLE")]
    role: Option<ClientRole>,

    /// Path to the CLI configuration file.
    #[arg(long, default_value = config::CONFIG_PATH)]
    config: String,

    /// Only submit the workorder — don't wait or run emerge locally.
    #[arg(long)]
    submit_only: bool,

    /// Don't run emerge locally after the remote build.
    /// Useful if you just want to populate the binhost.
    #[arg(long)]
    no_local: bool,

    /// Print what would be done without actually doing it.
    #[arg(long)]
    dry_run: bool,

    /// Force remote build even for packages that appear to be installed
    /// and up-to-date locally.
    #[arg(long)]
    force: bool,

    /// Suppress all non-essential output (Portage-style -q).
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    quiet: bool,

    /// Increase output verbosity.  May be repeated: -v info, -vv debug, -vvv trace.
    #[arg(short = 'v', long, action = clap::ArgAction::Count, conflicts_with = "quiet")]
    verbose: u8,

    /// Emit build events as newline-delimited JSON (NDJSON) instead of
    /// human-readable output.  Implies --quiet for human messages so the JSON
    /// stream is machine-parseable.  Can also be enabled with the
    /// REMERGE_LOG_JSON=1 environment variable.
    #[arg(long, env = "REMERGE_LOG_JSON")]
    log_json: bool,

    /// All remaining arguments are forwarded to emerge.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    emerge_args: Vec<String>,
}

const SNAPSHOT_UPLOAD_MAX_ATTEMPTS: usize = 3;
const SNAPSHOT_UPLOAD_RETRY_DELAY: Duration = Duration::from_millis(200);
const LOCAL_FOLLOWUP_RESTORE_TIMEOUT: Duration = Duration::from_secs(300);
const LOCAL_FOLLOWUP_WATCHDOG_HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageSyncDisposition {
    Downloaded,
    CacheHit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageSyncStatus {
    atom: String,
    size: u64,
    disposition: PackageSyncDisposition,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct BinpkgSyncSummary {
    downloaded_packages: usize,
    downloaded_bytes: u64,
    reused_packages: usize,
    reused_bytes: u64,
    failed_package: Option<String>,
    elapsed: Duration,
}

struct SyncProgressReporter {
    total_packages: usize,
    binhost_uri: String,
    started_at: Instant,
    last_progress_draw: Option<Instant>,
    summary: BinpkgSyncSummary,
    output_mode: SyncProgressOutputMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncProgressOutputMode {
    Interactive,
    Plain,
    /// Quiet mode (`-q`) — all output suppressed.
    Silent,
}

const SYNC_PROGRESS_DRAW_INTERVAL: Duration = Duration::from_millis(100);
const SYNC_PROGRESS_DRAW_INTERVAL_PLAIN: Duration = Duration::from_secs(1);

impl SyncProgressReporter {
    #[cfg(test)]
    fn new(total_packages: usize, binhost_uri: impl Into<String>) -> Self {
        Self::with_output_mode(
            total_packages,
            binhost_uri,
            if std::io::stderr().is_terminal() {
                SyncProgressOutputMode::Interactive
            } else {
                SyncProgressOutputMode::Plain
            },
        )
    }

    fn with_output_mode(
        total_packages: usize,
        binhost_uri: impl Into<String>,
        output_mode: SyncProgressOutputMode,
    ) -> Self {
        let reporter = Self {
            total_packages,
            binhost_uri: binhost_uri.into(),
            started_at: Instant::now(),
            last_progress_draw: None,
            summary: BinpkgSyncSummary::default(),
            output_mode,
        };

        reporter.print_line(&format!(
            "Syncing {} package(s) from {}",
            reporter.total_packages, reporter.binhost_uri
        ));

        reporter
    }

    fn record_cache_hit(&mut self, package: &BuiltPackage) -> PackageSyncStatus {
        self.print_line(&format!(
            "[CACHE-HIT] {} ({})",
            package.atom,
            Self::format_byte_count(package.size)
        ));
        PackageSyncStatus {
            atom: package.atom.clone(),
            size: package.size,
            disposition: PackageSyncDisposition::CacheHit,
        }
    }

    fn start_download(&mut self, package: &BuiltPackage) {
        self.last_progress_draw = None;
        self.print_line(&format!(
            "[DOWNLOAD] {} ({})",
            package.atom,
            Self::format_byte_count(package.size)
        ));
    }

    fn update_download_progress(
        &mut self,
        atom: &str,
        received_bytes: u64,
        total_bytes: u64,
        elapsed: Duration,
        stalled: bool,
    ) {
        let now = Instant::now();
        let draw_interval = match self.output_mode {
            SyncProgressOutputMode::Interactive => SYNC_PROGRESS_DRAW_INTERVAL,
            SyncProgressOutputMode::Plain => SYNC_PROGRESS_DRAW_INTERVAL_PLAIN,
            SyncProgressOutputMode::Silent => return,
        };
        let should_draw = self
            .last_progress_draw
            .is_none_or(|last_draw| now.duration_since(last_draw) >= draw_interval)
            || received_bytes >= total_bytes;
        if !should_draw {
            return;
        }

        self.last_progress_draw = Some(now);

        let line = Self::format_progress_line(atom, received_bytes, total_bytes, elapsed, stalled);
        match self.output_mode {
            SyncProgressOutputMode::Silent => {}
            SyncProgressOutputMode::Interactive => {
                use std::io::Write as _;

                eprint!("\r{line}");
                let _ = std::io::stderr().flush();
            }
            SyncProgressOutputMode::Plain => eprintln!("{line}"),
        }
    }

    fn finish_download(&mut self, package: &BuiltPackage, elapsed: Duration) {
        let line =
            Self::format_progress_line(&package.atom, package.size, package.size, elapsed, false);
        match self.output_mode {
            SyncProgressOutputMode::Silent => {}
            SyncProgressOutputMode::Interactive => eprintln!("\r{line}"),
            SyncProgressOutputMode::Plain => eprintln!("{line}"),
        }
        self.last_progress_draw = None;
    }

    fn refresh_index(&self, packages_url: &str) {
        self.print_line(&format!("[INDEX] Refreshing Packages from {packages_url}"));
    }

    fn record_result(&mut self, status: &PackageSyncStatus) {
        match status.disposition {
            PackageSyncDisposition::Downloaded => {
                self.summary.downloaded_packages += 1;
                self.summary.downloaded_bytes += status.size;
            }
            PackageSyncDisposition::CacheHit => {
                self.summary.reused_packages += 1;
                self.summary.reused_bytes += status.size;
            }
        }
    }

    fn record_failure(&mut self, atom: &str, error: &anyhow::Error) {
        self.summary.failed_package = Some(atom.to_string());
        self.print_line(&format!("[FAILED] {atom} — {error:#}"));
    }

    fn finish(mut self, pkgdir: &Path) -> BinpkgSyncSummary {
        self.summary.elapsed = self.started_at.elapsed();
        if self.summary.failed_package.is_some() {
            self.print_line("Sync incomplete:");
        } else {
            self.print_line("Sync complete:");
        }
        self.print_line(&format!(
            "  Downloaded: {} package(s), {}",
            self.summary.downloaded_packages,
            Self::format_byte_count(self.summary.downloaded_bytes)
        ));
        self.print_line(&format!(
            "  Reused:     {} package(s), {}",
            self.summary.reused_packages,
            Self::format_byte_count(self.summary.reused_bytes)
        ));
        if let Some(atom) = &self.summary.failed_package {
            self.print_line(&format!("  Failed:      {atom}"));
        }
        self.print_line(&format!(
            "  Elapsed:    {:.1}s",
            self.summary.elapsed.as_secs_f64()
        ));
        self.print_line(&format!("  Location:   {}", pkgdir.display()));
        self.summary
    }

    fn print_line(&self, line: &str) {
        if self.output_mode != SyncProgressOutputMode::Silent {
            eprintln!("{line}");
        }
    }

    fn format_progress_line(
        atom: &str,
        received_bytes: u64,
        total_bytes: u64,
        elapsed: Duration,
        stalled: bool,
    ) -> String {
        let total_bytes = total_bytes.max(1);
        let ratio = (received_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0);
        let filled = (ratio * 20.0).round() as usize;
        let empty = 20usize.saturating_sub(filled);
        let throughput = if elapsed.is_zero() {
            0.0
        } else {
            received_bytes as f64 / elapsed.as_secs_f64()
        };
        let remaining_bytes = total_bytes.saturating_sub(received_bytes);
        let eta_seconds = if throughput > 0.0 {
            remaining_bytes as f64 / throughput
        } else {
            0.0
        };
        let trailing_status = if stalled && received_bytes < total_bytes {
            "STALLED".to_string()
        } else {
            format!("ETA {:>4.1}s", eta_seconds)
        };

        format!(
            "[SYNC] {atom} [{}{}] {:>5.1}% {}/{} {}/s {trailing_status}",
            "#".repeat(filled),
            "-".repeat(empty),
            ratio * 100.0,
            Self::format_byte_count(received_bytes),
            Self::format_byte_count(total_bytes),
            Self::format_rate(throughput)
        )
    }

    fn format_byte_count(bytes: u64) -> String {
        const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

        let mut value = bytes as f64;
        let mut unit = 0usize;
        while value >= 1024.0 && unit < UNITS.len() - 1 {
            value /= 1024.0;
            unit += 1;
        }

        if unit == 0 {
            format!("{} {}", bytes, UNITS[unit])
        } else {
            format!("{value:.1} {}", UNITS[unit])
        }
    }

    fn format_rate(bytes_per_second: f64) -> String {
        if bytes_per_second <= 0.0 {
            return "0 B".to_string();
        }

        Self::format_byte_count(bytes_per_second.round() as u64)
    }
}

impl Cli {
    /// Parse CLI arguments from `std::env::args`.
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Derive the effective verbosity, consulting `EMERGE_DEFAULT_OPTS` as a
    /// fallback when no explicit flag is given.  Requires the portage config
    /// to already have been read; pass an empty string before that point.
    fn verbosity(&self, emerge_default_opts: &str) -> Verbosity {
        Verbosity::from_flags(self.quiet, self.verbose, emerge_default_opts)
    }

    /// Build the `emerge_args` slice to submit in the workorder, injecting a
    /// verbosity flag and filtering flags already handled by the worker.
    ///
    /// **Invariant**: at most one verbosity flag (`--quiet` OR `--verbose`)
    /// appears in the returned vec.  `VerboseDebug` and `VerboseTrace` only
    /// elevate `RUST_LOG` — emerge still receives at most one `--verbose`
    /// regardless of how many `-v` flags the operator typed.
    fn workorder_emerge_args(&self, verbosity: Verbosity) -> Vec<String> {
        let mut args = self.emerge_args.clone();

        // Inject a verbosity flag if the user hasn't already provided one and
        // the verbosity differs from Normal.
        if let Some(flag) = verbosity.emerge_flag() {
            let already_present = args
                .iter()
                .any(|a| matches!(a.as_str(), "-q" | "--quiet" | "-v" | "--verbose"));
            if !already_present {
                // Prepend so it applies before any user-supplied flags.
                args.insert(0, flag.to_string());
            }
        }

        args
    }
    pub async fn run(&self) -> Result<()> {
        if self.emerge_args.is_empty() {
            anyhow::bail!(
                "No emerge arguments provided.  Usage: remerge [OPTIONS] <emerge-args>..."
            );
        }

        let bar = StatusBar::global();
        // In log_json mode suppress all status bar output so ANSI codes don't
        // pollute the NDJSON stream on stderr and CI systems receive clean JSON.
        let bar = if self.log_json { None } else { bar };

        // 0. Load persistent config (server URL + client ID).
        let cfg = CliConfig::load_or_create(&self.config).unwrap_or_else(|e| {
            tracing::warn!("Failed to load config: {e:#} — using defaults");
            CliConfig::default()
        });

        // CLI flag / env var overrides the config file.
        let server = self.server.as_deref().unwrap_or(&cfg.server);

        // Client ID: CLI flag > config file.
        let client_id = self.client_id.unwrap_or(cfg.client_id);

        // Role: CLI flag > config file.
        let role = self.role.unwrap_or(cfg.role);

        // 1. Extract package atoms from the emerge arguments.
        let raw_atoms = self.extract_atoms();
        if raw_atoms.is_empty() {
            info!("No package atoms detected — falling through to plain emerge");
            return self.run_emerge_locally(&self.emerge_args, None).await;
        }

        // 1a. Expand set references (@world, @system) into individual atoms.
        if let Some(ref b) = bar {
            b.set_phase("Expanding package sets…");
        }
        let reader_for_sets = PortageReader::new()?;
        let atoms: Vec<String> = raw_atoms
            .into_iter()
            .flat_map(|a| {
                if a.starts_with('@') {
                    reader_for_sets.expand_set(&a)
                } else {
                    vec![a]
                }
            })
            .collect();

        // 1b. Validate all atoms before submitting.
        for atom in &atoms {
            // Sets that couldn't be expanded are passed through verbatim.
            if atom.starts_with('@') {
                continue;
            }
            if let Err(e) = validate_atom(atom) {
                anyhow::bail!("Invalid package atom '{atom}': {e}");
            }
        }

        // 1c. Check if packages are already installed (unless --force).
        let atoms = if self.force {
            atoms
        } else {
            if let Some(ref b) = bar {
                b.set_phase("Checking installed packages…");
            }
            let reader_for_vdb = PortageReader::new()?;
            let mut filtered = Vec::new();
            for atom in atoms {
                if reader_for_vdb.is_installed(&atom) {
                    if let Some(ref b) = bar {
                        b.println(&format!(
                            "  ⏭  {atom} — already installed (use --force to rebuild)"
                        ));
                    } else {
                        println!("  ⏭  {atom} — already installed (use --force to rebuild)");
                    }
                } else {
                    filtered.push(atom);
                }
            }
            if filtered.is_empty() {
                if let Some(ref b) = bar {
                    b.finish();
                }
                println!("All packages are already installed. Nothing to do.");
                return Ok(());
            }
            filtered
        };

        info!(
            ?atoms,
            client_id = %client_id,
            %role,
            "Packages to build remotely"
        );

        // 2. Read local portage configuration.
        //    This can take several seconds on a large Portage tree — show progress.
        if let Some(ref b) = bar {
            b.set_phase("Reading portage configuration…");
        }
        let reader = PortageReader::new()?;
        let portage_config = reader
            .read_config()
            .context("Failed to read portage configuration")?;
        let system_id = reader
            .read_system_identity()
            .context("Failed to determine system identity")?;

        // Derive final verbosity now that we have EMERGE_DEFAULT_OPTS.
        let verbosity = self.verbosity(&portage_config.make_conf.emerge_default_opts);
        // Retroactively silence the status bar if the real verbosity (which
        // considers EMERGE_DEFAULT_OPTS from make.conf) turns out to be quiet,
        // even though the bar was initialised from early_detect() which only
        // sees raw CLI flags.
        if verbosity.is_quiet()
            && let Some(bar) = crate::status_bar::StatusBar::global()
        {
            bar.silence();
        }

        if self.dry_run {
            if let Some(ref b) = bar {
                b.finish();
            }
            println!("Would submit workorder for: {}", atoms.join(", "));
            println!("  Server:    {server}");
            println!("  Client ID: {}", client_id);
            println!("  Role:      {}", role);
            println!("  Profile:   {}", system_id.profile);
            println!("  Arch:      {}", system_id.arch);
            println!("  CHOST:     {}", system_id.chost);
            println!("  CFLAGS:    {}", portage_config.make_conf.cflags);
            if let Some(ref orig) = portage_config.make_conf.original_cflags {
                println!("    (was:    {})", orig);
            }
            if let Some((ref var, ref flags)) = portage_config.make_conf.cpu_flags {
                println!("  {var}: {}", flags.join(" "));
            }
            println!(
                "  USE:       {}",
                portage_config.make_conf.use_flags.join(" ")
            );
            return Ok(());
        }

        // 3. Upload any snapshot blobs the server is missing.
        if let Some(ref b) = bar {
            b.set_phase("Checking snapshot blobs…");
        }
        let client = RemergeClient::new(server)?;
        let submitted_portage_config = self
            .prepare_manifest_submission(&client, &portage_config, bar.as_deref())
            .await
            .context("Failed to negotiate snapshot blob upload")?;

        // Build the emerge_args for the workorder, injecting a verbosity flag.
        let workorder_emerge_args = self.workorder_emerge_args(verbosity);

        // 4. Submit workorder.
        if let Some(ref b) = bar {
            b.set_phase("Submitting workorder…");
        }
        let resp = client
            .submit_workorder(
                client_id,
                role,
                &atoms,
                &workorder_emerge_args,
                &submitted_portage_config,
                &system_id,
            )
            .await
            .context("Failed to submit workorder")?;

        if let Some(ref b) = bar {
            b.println(&format!(
                "Workorder {} submitted — streaming progress…",
                resp.workorder_id
            ));
            if let Some(ref trace_id) = resp.trace_id
                && verbosity.is_verbose()
            {
                b.println(&format!("Trace ID: {trace_id}"));
            }
        } else {
            // Status messages go to stderr so they do not corrupt the NDJSON
            // stream when --log-json is active.
            eprintln!(
                "Workorder {} submitted — streaming progress…",
                resp.workorder_id
            );
            if let Some(ref trace_id) = resp.trace_id {
                eprintln!("Trace ID: {trace_id}");
            }
        }

        if self.submit_only {
            if let Some(ref b) = bar {
                b.finish();
            }
            println!("Workorder ID: {}", resp.workorder_id);
            return Ok(());
        }

        // 5. Stream build progress via WebSocket.
        //    PTY bytes from the remote build flow directly to stdout — hide the
        //    status bar for the duration and restore it after.
        if let Some(ref b) = bar {
            b.set_phase("Waiting for build to start…");
        }
        let result = client
            .stream_progress(&resp.progress_ws_url, verbosity, self.log_json)
            .await
            .context("Failed to stream build progress")?;

        if let Some(ref b) = bar {
            b.show();
        }

        // 6. Report results.
        let built_list = result
            .built_packages
            .iter()
            .map(|p| p.atom.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // In log_json mode the NDJSON stream already contains the Finished frame;
        // suppress the human-readable banner to keep stdout clean for CI tooling.
        if !self.log_json {
            if let Some(ref b) = bar {
                b.println("\n─── Build complete ───");
                b.println(&format!("  Built: {built_list}"));
                if verbosity.is_verbose()
                    && let Some(ref trace_id) = resp.trace_id
                {
                    b.println(&format!("  Trace: {trace_id}"));
                }
            } else {
                eprintln!("\n─── Build complete ───");
                eprintln!("  Built: {built_list}");
                if verbosity.is_verbose()
                    && let Some(ref trace_id) = resp.trace_id
                {
                    eprintln!("  Trace: {trace_id}");
                }
            }
        }
        if !result.failed_packages.is_empty() {
            let failed_list = result
                .failed_packages
                .iter()
                .map(|p| p.atom.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("  Failed: {failed_list}");
            anyhow::bail!(
                "Remote build failed: {} package(s) failed",
                result.failed_packages.len()
            );
        }

        if let Some(ref b) = bar {
            b.set_phase("Syncing binary packages…");
        }
        let local_binhost_uri = match self
            .sync_local_binpkg_cache(&client, &result, verbosity)
            .await
        {
            Ok(uri) => uri,
            Err(error) => {
                tracing::warn!(%error, "Failed to sync local binpkg cache");
                None
            }
        };

        let local_args = Self::build_local_emerge_args(&self.emerge_args);
        let binhost_uri = local_binhost_uri.or_else(|| Some(result.binhost_uri.clone()));
        self.complete_local_followup(
            bar.as_deref(),
            || async { self.restore_local_followup_state(&client, &result).await },
            || async {
                self.run_emerge_locally(&local_args, binhost_uri.as_deref())
                    .await
            },
        )
        .await
    }

    fn extract_atoms(&self) -> Vec<String> {
        extract_package_atoms(&self.emerge_args)
    }

    fn build_local_emerge_args(emerge_args: &[String]) -> Vec<String> {
        let mut local_args = vec!["--getbinpkg".to_string(), "--usepkg".to_string()];
        local_args.extend(emerge_args.iter().cloned());
        local_args
    }

    async fn prepare_manifest_submission(
        &self,
        client: &RemergeClient,
        portage_config: &PortageConfig,
        bar: Option<&StatusBar>,
    ) -> Result<PortageConfig> {
        let blob_payloads = Self::snapshot_blob_payloads(portage_config)?;
        if !blob_payloads.is_empty() {
            let digests: Vec<String> = blob_payloads.keys().cloned().collect();
            if let Some(b) = bar {
                b.set_phase(format!(
                    "Checking snapshot blobs ({} repo snapshots)…",
                    digests.len()
                ));
            }
            let missing = client.find_missing_blobs(&digests).await?;
            let total_missing = missing.len();
            for (idx, digest) in missing.into_iter().enumerate() {
                if let Some(b) = bar {
                    b.set_phase(format!(
                        "Uploading snapshot blob {}/{total_missing}…",
                        idx + 1
                    ));
                }
                let payload = blob_payloads.get(&digest).with_context(|| {
                    format!("Server requested missing blob {digest}, but the client has no payload")
                })?;
                Self::retry_blob_upload(
                    &digest,
                    SNAPSHOT_UPLOAD_MAX_ATTEMPTS,
                    SNAPSHOT_UPLOAD_RETRY_DELAY,
                    || client.stream_upload_blob(&digest, payload),
                )
                .await?;
            }
        }

        let mut submitted = portage_config.clone();
        submitted.repo_snapshots.clear();
        submitted.distfile_snapshots.clear();
        Ok(submitted)
    }

    fn snapshot_blob_payloads(portage_config: &PortageConfig) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut payloads = BTreeMap::new();

        for (repo_name, snapshot) in &portage_config.repo_snapshots {
            let manifest_entries = portage_config
                .snapshot_manifest
                .repo_snapshots
                .get(repo_name)
                .map(|manifest| &manifest.entries);
            let snapshot_refs = portage_config.repo_snapshot_refs.get(repo_name);

            for (relative_path, content) in snapshot {
                let digest = manifest_entries
                    .and_then(|entries| {
                        entries
                            .get(relative_path)
                            .map(|entry| entry.digest.as_str())
                    })
                    .or_else(|| {
                        snapshot_refs.and_then(|refs| refs.get(relative_path).map(String::as_str))
                    })
                    .with_context(|| {
                        format!("Missing repo snapshot digest for '{repo_name}:{relative_path}'")
                    })?;
                Self::insert_blob_payload(&mut payloads, digest, content.as_bytes())?;
            }
        }

        for (filename, bytes) in &portage_config.distfile_snapshots {
            let digest = portage_config
                .snapshot_manifest
                .distfiles
                .get(filename)
                .map(|entry| entry.digest.as_str())
                .or_else(|| {
                    portage_config
                        .distfile_snapshot_refs
                        .get(filename)
                        .map(String::as_str)
                })
                .with_context(|| format!("Missing distfile snapshot digest for '{filename}'"))?;
            Self::insert_blob_payload(&mut payloads, digest, bytes)?;
        }

        Ok(payloads)
    }

    fn insert_blob_payload(
        payloads: &mut BTreeMap<String, Vec<u8>>,
        digest: &str,
        bytes: &[u8],
    ) -> Result<()> {
        match payloads.get(digest) {
            Some(existing) if existing != bytes => {
                anyhow::bail!("Digest collision for snapshot blob {digest}");
            }
            Some(_) => {}
            None => {
                payloads.insert(digest.to_string(), bytes.to_vec());
            }
        }
        Ok(())
    }

    async fn retry_blob_upload<F, Fut>(
        digest: &str,
        max_attempts: usize,
        retry_delay: Duration,
        mut upload_once: F,
    ) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<bool>>,
    {
        let attempts = max_attempts.max(1);
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 1..=attempts {
            match upload_once().await {
                Ok(_) => return Ok(()),
                Err(error) if attempt < attempts => {
                    tracing::warn!(
                        %error,
                        digest,
                        attempt,
                        attempts,
                        "Snapshot blob upload failed; retrying"
                    );
                    last_error = Some(error);
                    let backoff = retry_delay
                        .checked_mul(attempt as u32)
                        .unwrap_or(retry_delay);
                    tokio::time::sleep(backoff).await;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Failed to upload snapshot blob {digest} after {attempts} attempt(s)"
                        )
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("snapshot blob upload retry loop ended unexpectedly for {digest}")
        }))
    }

    async fn sync_local_binpkg_cache(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
        verbosity: Verbosity,
    ) -> Result<Option<String>> {
        if result.built_packages.is_empty() {
            return Ok(None);
        }

        let pkgdir = Self::detect_local_pkgdir().await?;
        self.sync_local_binpkg_cache_into(client, result, &pkgdir, verbosity)
            .await?;

        Ok(Self::file_binhost_uri(&pkgdir))
    }

    async fn sync_local_binpkg_cache_into(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
        pkgdir: &Path,
        verbosity: Verbosity,
    ) -> Result<()> {
        tokio::fs::create_dir_all(&pkgdir)
            .await
            .with_context(|| format!("Failed to create {}", pkgdir.display()))?;

        let output_mode = if verbosity.is_quiet() {
            SyncProgressOutputMode::Silent
        } else if std::io::stderr().is_terminal() {
            SyncProgressOutputMode::Interactive
        } else {
            SyncProgressOutputMode::Plain
        };
        let mut reporter = SyncProgressReporter::with_output_mode(
            result.built_packages.len(),
            result.binhost_uri.clone(),
            output_mode,
        );

        for package in &result.built_packages {
            match self
                .sync_single_binpkg(client, result, pkgdir, package, &mut reporter)
                .await
            {
                Ok(status) => reporter.record_result(&status),
                Err(error) => {
                    reporter.record_failure(&package.atom, &error);
                    reporter.finish(pkgdir);
                    return Err(error).with_context(|| {
                        format!("Failed to sync local binpkg cache for {}", package.atom)
                    });
                }
            }
        }

        let packages_path = pkgdir.join("Packages");
        let packages_url = format!("{}/Packages", result.binhost_uri.trim_end_matches('/'));
        reporter.refresh_index(&packages_url);
        if let Err(error) = client.download_file(&packages_url, &packages_path).await {
            tracing::warn!(%error, url = %packages_url, "Failed to refresh local Packages index");
        }

        reporter.finish(pkgdir);
        Ok(())
    }

    async fn restore_local_followup_state(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
    ) -> Result<()> {
        self.restore_local_followup_state_into(
            client,
            result,
            &Self::distdir_root(),
            &Self::parity_root(),
        )
        .await
    }

    async fn restore_local_followup_state_into(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
        distdir: &Path,
        parity_root: &Path,
    ) -> Result<()> {
        println!("Restoring fetched distfiles into {}…", distdir.display());
        reconcile_fetched_distfiles_into(distdir, client, result).await?;
        if self.no_local {
            println!("Skipping final-state parity restore because --no-local was requested.");
            return Ok(());
        }

        println!(
            "Restoring final-state parity into {}…",
            parity_root.display()
        );
        reconcile_final_state_parity_into(parity_root, client, result).await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    async fn restore_final_state_parity_into(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
        parity_root: &Path,
    ) -> Result<()> {
        reconcile_fetched_distfiles_into(&Self::distdir_root(), client, result).await?;
        reconcile_final_state_parity_into(parity_root, client, result).await
    }

    async fn complete_local_followup<Reconcile, ReconcileFuture, RunLocal, RunLocalFuture>(
        &self,
        bar: Option<&StatusBar>,
        reconcile: Reconcile,
        run_local: RunLocal,
    ) -> Result<()>
    where
        Reconcile: FnOnce() -> ReconcileFuture,
        ReconcileFuture: Future<Output = Result<()>>,
        RunLocal: FnOnce() -> RunLocalFuture,
        RunLocalFuture: Future<Output = Result<()>>,
    {
        if let Some(b) = bar {
            b.set_phase("Restoring local follow-up state…");
        } else {
            println!("Restoring local follow-up state…");
        }
        Self::run_with_watchdog(
            "restoring local follow-up state",
            LOCAL_FOLLOWUP_RESTORE_TIMEOUT,
            reconcile(),
        )
        .await
        .context("Failed to restore local follow-up state")?;

        if let Some(b) = bar {
            b.println("Local follow-up state restored.");
        } else {
            println!("Local follow-up state restored.");
        }

        if self.no_local {
            println!("Skipping local emerge because --no-local was requested.");
            if let Some(b) = bar {
                b.finish();
            }
            return Ok(());
        }

        if let Some(b) = bar {
            b.hide();
            eprintln!("\nRunning emerge locally with binary packages…\n");
        } else {
            println!("\nRunning emerge locally with binary packages…\n");
        }
        let result = run_local().await;
        if let Some(b) = bar {
            b.finish();
        }
        result
    }

    async fn run_with_watchdog<T, Fut>(stage: &str, timeout: Duration, future: Fut) -> Result<T>
    where
        Fut: Future<Output = Result<T>>,
    {
        let started_at = Instant::now();
        let mut heartbeat = tokio::time::interval(LOCAL_FOLLOWUP_WATCHDOG_HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        tokio::pin!(future);

        loop {
            tokio::select! {
                result = &mut future => return result,
                _ = &mut deadline => {
                    anyhow::bail!(
                        "CLI watchdog timed out after {}s while {stage}; the client stopped making progress in that stage",
                        timeout.as_secs(),
                    );
                }
                _ = heartbeat.tick() => {
                    let elapsed = started_at.elapsed().as_secs();
                    // Update the status bar if present; otherwise emit a
                    // timed log line so non-TTY environments (CI, pipes) see
                    // progress.
                    if let Some(bar) = StatusBar::global() {
                        bar.set_phase(format!("{stage} ({elapsed}s)…"));
                    } else {
                        tracing::info!(
                            stage,
                            elapsed_secs = elapsed,
                            timeout_secs = timeout.as_secs(),
                            "still working"
                        );
                    }
                }
            }
        }
    }

    async fn sync_single_binpkg(
        &self,
        client: &RemergeClient,
        result: &WorkorderResult,
        pkgdir: &Path,
        package: &BuiltPackage,
        reporter: &mut SyncProgressReporter,
    ) -> Result<PackageSyncStatus> {
        let relative_path = Self::sanitize_binpkg_path(&package.binpkg_path)
            .with_context(|| format!("Invalid binpkg path '{}'", package.binpkg_path))?;
        let destination = pkgdir.join(&relative_path);
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        if Self::is_package_cached(&destination, package).await? {
            return Ok(reporter.record_cache_hit(package));
        }

        let temporary = destination.with_extension("part");
        let url = format!(
            "{}/{}",
            result.binhost_uri.trim_end_matches('/'),
            package.binpkg_path.trim_start_matches('/'),
        );
        reporter.start_download(package);
        let progress_started_at = Instant::now();
        client
            .download_file_with_progress(
                &url,
                &temporary,
                |received_bytes, total_bytes, stalled| {
                    reporter.update_download_progress(
                        &package.atom,
                        received_bytes,
                        total_bytes.unwrap_or(package.size),
                        progress_started_at.elapsed(),
                        stalled,
                    );
                },
            )
            .await?;

        let metadata = tokio::fs::metadata(&temporary)
            .await
            .with_context(|| format!("Failed to stat {}", temporary.display()))?;
        if metadata.len() != package.size {
            anyhow::bail!(
                "Downloaded {} but expected {} bytes for {}",
                metadata.len(),
                package.size,
                package.atom
            );
        }

        let sha256 = Self::sha256_file(&temporary).await?;
        if !sha256.eq_ignore_ascii_case(&package.sha256) {
            anyhow::bail!(
                "SHA256 mismatch for {}: expected {}, got {}",
                package.atom,
                package.sha256,
                sha256
            );
        }

        tokio::fs::rename(&temporary, &destination)
            .await
            .with_context(|| {
                format!(
                    "Failed to move {} into {}",
                    temporary.display(),
                    destination.display()
                )
            })?;
        reporter.finish_download(package, progress_started_at.elapsed());

        Ok(PackageSyncStatus {
            atom: package.atom.clone(),
            size: package.size,
            disposition: PackageSyncDisposition::Downloaded,
        })
    }

    async fn is_package_cached(path: &Path, package: &BuiltPackage) -> Result<bool> {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to stat {}", path.display()));
            }
        };

        if metadata.len() != package.size {
            return Ok(false);
        }

        let sha256 = Self::sha256_file(path).await?;
        Ok(sha256.eq_ignore_ascii_case(&package.sha256))
    }

    fn parity_root() -> PathBuf {
        std::env::var("ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"))
    }

    async fn detect_local_pkgdir() -> Result<PathBuf> {
        if let Ok(value) = std::env::var("PKGDIR") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }

        let output = tokio::process::Command::new("portageq")
            .args(["envvar", "PKGDIR"])
            .output()
            .await;
        if let Ok(output) = output
            && output.status.success()
        {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !value.is_empty() {
                return Ok(PathBuf::from(value));
            }
        }

        Ok(PathBuf::from("/var/cache/binpkgs"))
    }

    fn file_binhost_uri(pkgdir: &Path) -> Option<String> {
        if !pkgdir.is_absolute() {
            return None;
        }
        Some(format!("file://{}", pkgdir.display()))
    }

    fn distdir_root() -> PathBuf {
        if let Ok(value) = std::env::var("DISTDIR") {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return path;
            }
        }

        PathBuf::from("/var/cache/distfiles")
    }

    fn sanitize_binpkg_path(path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        if path.is_absolute() {
            anyhow::bail!("absolute paths are not allowed");
        }
        let mut sanitized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(segment) => sanitized.push(segment),
                _ => anyhow::bail!("path traversal is not allowed"),
            }
        }
        if sanitized.as_os_str().is_empty() {
            anyhow::bail!("empty paths are not allowed");
        }
        Ok(sanitized)
    }

    async fn sha256_file(path: &Path) -> Result<String> {
        use tokio::io::AsyncReadExt;

        let mut file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let read = file
                .read(&mut buffer)
                .await
                .with_context(|| format!("Failed to read {}", path.display()))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }

        Ok(hex::encode(digest.finalize()))
    }

    /// Run emerge as a child process with the given arguments.
    async fn run_emerge_locally(&self, args: &[String], binhost_uri: Option<&str>) -> Result<()> {
        run_local_emerge_with_program(Path::new("emerge"), args, binhost_uri).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::Path as AxumPath;
    use axum::extract::State;
    use axum::http::{Response, StatusCode};
    use axum::routing::get;
    use sha2::Digest;
    use tempfile::TempDir;

    use super::{
        BinpkgSyncSummary, Cli, PackageSyncDisposition, SyncProgressOutputMode,
        SyncProgressReporter, extract_package_atoms, portage_binhost_env,
        reconcile_fetched_distfiles_into, resolve_parity_target, set_file_mtime,
    };
    use crate::client::RemergeClient;
    use crate::verbosity::Verbosity;
    use remerge_types::portage::{MakeConf, PortageConfig, SnapshotEntry};
    use remerge_types::workorder::{
        BuiltPackage, ParityFileEntry, ParityManifest, WorkorderResult,
    };

    fn test_cli() -> Cli {
        Cli {
            server: None,
            client_id: None,
            role: None,
            config: "test-config.toml".to_string(),
            submit_only: false,
            no_local: false,
            dry_run: false,
            force: false,
            quiet: false,
            verbose: 0,
            log_json: false,
            emerge_args: Vec::new(),
        }
    }

    fn test_package(payload: &[u8]) -> BuiltPackage {
        let sha256 = sha2::Sha256::digest(payload);

        BuiltPackage {
            atom: "dev-libs/demo-1.0".to_string(),
            binpkg_path: "dev-libs/demo-1.0.gpkg.tar".to_string(),
            sha256: hex::encode(sha256),
            size: payload.len() as u64,
        }
    }

    async fn spawn_binpkg_server(
        payload: Vec<u8>,
        packages_index: Vec<u8>,
        requests: Arc<AtomicUsize>,
    ) -> String {
        #[derive(Clone)]
        struct TestState {
            payload: Arc<Vec<u8>>,
            packages_index: Arc<Vec<u8>>,
            requests: Arc<AtomicUsize>,
        }

        async fn serve_binpkg(State(state): State<TestState>) -> Response<Body> {
            state.requests.fetch_add(1, Ordering::SeqCst);
            Response::builder()
                .status(StatusCode::OK)
                .header("content-length", state.payload.len().to_string())
                .body(Body::from((*state.payload).clone()))
                .expect("binpkg response")
        }

        async fn serve_packages(State(state): State<TestState>) -> Response<Body> {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-length", state.packages_index.len().to_string())
                .body(Body::from((*state.packages_index).clone()))
                .expect("packages response")
        }

        let state = TestState {
            payload: Arc::new(payload),
            packages_index: Arc::new(packages_index),
            requests,
        };

        let app = Router::new()
            .route("/binpkgs/dev-libs/demo-1.0.gpkg.tar", get(serve_binpkg))
            .route("/binpkgs/Packages", get(serve_packages))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });

        format!("http://{address}/binpkgs")
    }

    async fn spawn_multi_binpkg_server(
        assets: HashMap<String, Result<Vec<u8>, StatusCode>>,
        packages_index: Vec<u8>,
        requests: Arc<Mutex<Vec<String>>>,
    ) -> String {
        #[derive(Clone)]
        struct TestState {
            assets: Arc<HashMap<String, Result<Vec<u8>, StatusCode>>>,
            packages_index: Arc<Vec<u8>>,
            requests: Arc<Mutex<Vec<String>>>,
        }

        async fn serve_asset(
            AxumPath(path): AxumPath<String>,
            State(state): State<TestState>,
        ) -> Response<Body> {
            state.requests.lock().unwrap().push(path.clone());

            match state.assets.get(&path) {
                Some(Ok(payload)) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-length", payload.len().to_string())
                    .body(Body::from(payload.clone()))
                    .expect("asset response"),
                Some(Err(status)) => Response::builder()
                    .status(*status)
                    .body(Body::from(format!("forced failure for {path}")))
                    .expect("error response"),
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from(format!("missing asset {path}")))
                    .expect("missing response"),
            }
        }

        async fn serve_packages(State(state): State<TestState>) -> Response<Body> {
            state.requests.lock().unwrap().push("Packages".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("content-length", state.packages_index.len().to_string())
                .body(Body::from((*state.packages_index).clone()))
                .expect("packages response")
        }

        let state = TestState {
            assets: Arc::new(assets),
            packages_index: Arc::new(packages_index),
            requests,
        };

        let app = Router::new()
            .route("/binpkgs/Packages", get(serve_packages))
            .route("/binpkgs/{*path}", get(serve_asset))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind multi test server");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve multi test app");
        });

        format!("http://{address}/binpkgs")
    }

    async fn spawn_blob_server(
        blobs: HashMap<String, Vec<u8>>,
        requests: Arc<Mutex<Vec<String>>>,
    ) -> String {
        #[derive(Clone)]
        struct BlobState {
            blobs: Arc<HashMap<String, Vec<u8>>>,
            requests: Arc<Mutex<Vec<String>>>,
        }

        async fn serve_blob(
            AxumPath(digest): AxumPath<String>,
            State(state): State<BlobState>,
        ) -> Response<Body> {
            state.requests.lock().unwrap().push(digest.clone());

            match state.blobs.get(&digest) {
                Some(payload) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-length", payload.len().to_string())
                    .body(Body::from(payload.clone()))
                    .expect("blob response"),
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from(format!("missing blob {digest}")))
                    .expect("missing blob response"),
            }
        }

        let state = BlobState {
            blobs: Arc::new(blobs),
            requests,
        };

        let app = Router::new()
            .route("/api/v1/snapshots/blobs/{digest}", get(serve_blob))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind blob test server");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve blob test app");
        });

        format!("http://{address}")
    }

    #[test]
    fn extract_package_atoms_preserves_sets_and_versioned_atoms() {
        let args = vec![
            "--ask".to_string(),
            "@world".to_string(),
            "=dev-libs/openssl-3.1.4".to_string(),
            "--with-bdeps=y".to_string(),
            "firefox".to_string(),
        ];

        assert_eq!(
            extract_package_atoms(&args),
            vec![
                "@world".to_string(),
                "=dev-libs/openssl-3.1.4".to_string(),
                "firefox".to_string(),
            ]
        );
    }

    #[test]
    fn extract_package_atoms_ignores_option_like_values() {
        let args = vec![
            "--jobs=8".to_string(),
            "--keep-going".to_string(),
            "-av".to_string(),
        ];

        assert!(extract_package_atoms(&args).is_empty());
    }

    #[test]
    fn pc_010_local_install_binhost_handoff_contract() {
        let args = vec!["dev-libs/openssl".to_string(), "--ask".to_string()];
        let local_args = Cli::build_local_emerge_args(&args);

        assert_eq!(
            local_args,
            vec![
                "--getbinpkg".to_string(),
                "--usepkg".to_string(),
                "dev-libs/openssl".to_string(),
                "--ask".to_string(),
            ]
        );
        assert_eq!(
            portage_binhost_env(Some("https://binhost.example.invalid/binpkgs")),
            Some("https://binhost.example.invalid/binpkgs".to_string())
        );
        assert_eq!(
            Cli::file_binhost_uri(std::path::Path::new("/var/cache/binpkgs")),
            Some("file:///var/cache/binpkgs".to_string())
        );
        assert_eq!(portage_binhost_env(Some("   ")), None);
    }

    #[test]
    fn binpkg_path_sanitizer_rejects_traversal() {
        assert!(Cli::sanitize_binpkg_path("dev-libs/openssl-3.4.0.gpkg.tar").is_ok());
        assert!(Cli::sanitize_binpkg_path("dev-libs/../shadow.gpkg.tar").is_err());
        assert!(Cli::sanitize_binpkg_path("/tmp/shadow.gpkg.tar").is_err());
    }

    #[test]
    fn snapshot_blob_payloads_collects_repo_and_distfile_bytes_once_per_digest() {
        let config = PortageConfig {
            make_conf: MakeConf {
                cflags: String::new(),
                cxxflags: String::new(),
                ldflags: String::new(),
                makeopts: String::new(),
                use_flags: Vec::new(),
                features: Vec::new(),
                accept_license: String::new(),
                accept_keywords: String::new(),
                emerge_default_opts: String::new(),
                chost: String::new(),
                cpu_flags: None,
                original_cflags: None,
                use_expand: BTreeMap::new(),
                extra: BTreeMap::new(),
                use_flags_resolved: false,
            },
            package_use: Vec::new(),
            package_accept_keywords: Vec::new(),
            package_license: Vec::new(),
            package_mask: Vec::new(),
            package_unmask: Vec::new(),
            package_env: Vec::new(),
            env_files: BTreeMap::new(),
            repos_conf: BTreeMap::new(),
            snapshot_manifest: Default::default(),
            repo_snapshots: BTreeMap::from([(
                "local-overlay".to_string(),
                BTreeMap::from([(
                    "dev-libs/demo/demo-1.0.ebuild".to_string(),
                    "EAPI=8\n".to_string(),
                )]),
            )]),
            repo_snapshot_refs: BTreeMap::from([(
                "local-overlay".to_string(),
                BTreeMap::from([(
                    "dev-libs/demo/demo-1.0.ebuild".to_string(),
                    "abc123".to_string(),
                )]),
            )]),
            repo_snapshot_trees: BTreeMap::new(),
            patches: BTreeMap::new(),
            profile_overlay: BTreeMap::new(),
            distfile_snapshots: BTreeMap::from([(
                "demo-1.0.tar.xz".to_string(),
                b"demo-distfile".to_vec(),
            )]),
            distfile_snapshot_refs: BTreeMap::from([(
                "demo-1.0.tar.xz".to_string(),
                "def456".to_string(),
            )]),
            profile: String::new(),
            world: Vec::new(),
        };

        let payloads = Cli::snapshot_blob_payloads(&config).expect("snapshot payloads");

        assert_eq!(payloads["abc123"], b"EAPI=8\n");
        assert_eq!(payloads["def456"], b"demo-distfile");
    }

    #[tokio::test]
    async fn retry_blob_upload_succeeds_after_transient_failures() {
        let attempts = Arc::new(AtomicUsize::new(0));

        Cli::retry_blob_upload("abc123", 3, Duration::ZERO, {
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        anyhow::bail!("transient upload failure");
                    }
                    Ok(true)
                }
            }
        })
        .await
        .expect("upload should succeed after retry");

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_blob_upload_fails_after_max_attempts() {
        let attempts = Arc::new(AtomicUsize::new(0));

        let error = Cli::retry_blob_upload("abc123", 3, Duration::ZERO, {
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("persistent upload failure")
                }
            }
        })
        .await
        .expect_err("upload should fail after exhausting retries");

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(
            error
                .to_string()
                .contains("Failed to upload snapshot blob abc123 after 3 attempt(s)"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn sync_progress_line_includes_package_bytes_throughput_and_eta() {
        let line = SyncProgressReporter::format_progress_line(
            "dev-libs/demo-1.0",
            512 * 1024,
            1024 * 1024,
            Duration::from_secs(2),
            false,
        );

        assert!(line.contains("[SYNC] dev-libs/demo-1.0"), "line: {line}");
        assert!(line.contains("ETA"), "line: {line}");
        assert!(line.contains("/s"), "line: {line}");
        assert!(line.contains("50.0%"), "line: {line}");
        assert!(line.contains("[##########----------]"), "line: {line}");
    }

    #[test]
    fn sync_progress_line_shows_stalled_instead_of_eta() {
        let line = SyncProgressReporter::format_progress_line(
            "dev-libs/demo-1.0",
            512 * 1024,
            1024 * 1024,
            Duration::from_secs(5),
            true,
        );

        assert!(line.contains("STALLED"), "line: {line}");
        assert!(!line.contains("ETA"), "line: {line}");
    }

    #[test]
    fn sync_progress_reporter_can_use_plain_mode() {
        let reporter = SyncProgressReporter::with_output_mode(
            1,
            "https://binhost.invalid",
            SyncProgressOutputMode::Plain,
        );

        assert_eq!(reporter.output_mode, SyncProgressOutputMode::Plain);
    }

    #[tokio::test]
    async fn repeated_sync_reuses_verified_local_binpkg_without_redownloading() {
        let payload = vec![0x5a; 512 * 1024];
        let package = test_package(&payload);
        let requests = Arc::new(AtomicUsize::new(0));
        let binhost_uri =
            spawn_binpkg_server(payload, b"PACKAGES\n".to_vec(), requests.clone()).await;
        let client = RemergeClient::new(&binhost_uri).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: vec![package.clone()],
            failed_packages: Vec::new(),
            binhost_uri: binhost_uri.clone(),
            fetched_distfiles: BTreeMap::new(),
            parity_manifest: ParityManifest::default(),
        };
        let pkgdir = TempDir::new().expect("temp pkgdir");
        let cli = test_cli();
        let mut first_reporter = SyncProgressReporter::new(1, binhost_uri.clone());

        let first_status = cli
            .sync_single_binpkg(
                &client,
                &result,
                pkgdir.path(),
                &package,
                &mut first_reporter,
            )
            .await
            .expect("initial sync should download package");
        first_reporter.record_result(&first_status);
        let first_summary = first_reporter.finish(pkgdir.path());

        let mut second_reporter = SyncProgressReporter::new(1, binhost_uri);
        let second_status = cli
            .sync_single_binpkg(
                &client,
                &result,
                pkgdir.path(),
                &package,
                &mut second_reporter,
            )
            .await
            .expect("repeat sync should use cached package");
        second_reporter.record_result(&second_status);
        let second_summary = second_reporter.finish(pkgdir.path());

        assert_eq!(first_status.disposition, PackageSyncDisposition::Downloaded);
        assert_eq!(second_status.disposition, PackageSyncDisposition::CacheHit);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "cached file should skip redownload"
        );
        assert_eq!(
            first_summary,
            BinpkgSyncSummary {
                downloaded_packages: 1,
                downloaded_bytes: package.size,
                reused_packages: 0,
                reused_bytes: 0,
                failed_package: None,
                elapsed: first_summary.elapsed,
            }
        );
        assert_eq!(
            second_summary,
            BinpkgSyncSummary {
                downloaded_packages: 0,
                downloaded_bytes: 0,
                reused_packages: 1,
                reused_bytes: package.size,
                failed_package: None,
                elapsed: second_summary.elapsed,
            }
        );
    }

    #[tokio::test]
    async fn sync_stops_on_download_failure_without_refreshing_index() {
        let payload_one = vec![0x11; 128 * 1024];
        let payload_three = vec![0x33; 64 * 1024];
        let package_one = BuiltPackage {
            atom: "dev-libs/one-1.0".to_string(),
            binpkg_path: "dev-libs/one-1.0.gpkg.tar".to_string(),
            sha256: hex::encode(sha2::Sha256::digest(&payload_one)),
            size: payload_one.len() as u64,
        };
        let package_two = BuiltPackage {
            atom: "dev-libs/two-2.0".to_string(),
            binpkg_path: "dev-libs/two-2.0.gpkg.tar".to_string(),
            sha256: "00".repeat(32),
            size: 32,
        };
        let package_three = BuiltPackage {
            atom: "dev-libs/three-3.0".to_string(),
            binpkg_path: "dev-libs/three-3.0.gpkg.tar".to_string(),
            sha256: hex::encode(sha2::Sha256::digest(&payload_three)),
            size: payload_three.len() as u64,
        };

        let requests = Arc::new(Mutex::new(Vec::new()));
        let binhost_uri = spawn_multi_binpkg_server(
            HashMap::from([
                (package_one.binpkg_path.clone(), Ok(payload_one.clone())),
                (
                    package_two.binpkg_path.clone(),
                    Err(StatusCode::INTERNAL_SERVER_ERROR),
                ),
                (package_three.binpkg_path.clone(), Ok(payload_three.clone())),
            ]),
            b"PACKAGES\n".to_vec(),
            requests.clone(),
        )
        .await;
        let client = RemergeClient::new(&binhost_uri).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: vec![
                package_one.clone(),
                package_two.clone(),
                package_three.clone(),
            ],
            failed_packages: Vec::new(),
            binhost_uri,
            fetched_distfiles: BTreeMap::new(),
            parity_manifest: ParityManifest::default(),
        };
        let pkgdir = TempDir::new().expect("temp pkgdir");
        let cli = test_cli();

        let error = cli
            .sync_local_binpkg_cache_into(&client, &result, pkgdir.path(), Verbosity::Normal)
            .await
            .expect_err("sync should stop on the failing package");

        assert!(
            error
                .to_string()
                .contains("Failed to sync local binpkg cache for dev-libs/two-2.0"),
            "unexpected error: {error:#}"
        );

        let downloaded_one = pkgdir.path().join(&package_one.binpkg_path);
        let missing_three = pkgdir.path().join(&package_three.binpkg_path);
        let packages_index = pkgdir.path().join("Packages");
        assert_eq!(tokio::fs::read(downloaded_one).await.unwrap(), payload_one);
        assert!(!tokio::fs::try_exists(&missing_three).await.unwrap());
        assert!(!tokio::fs::try_exists(&packages_index).await.unwrap());

        let seen_requests = requests.lock().unwrap().clone();
        assert_eq!(
            seen_requests,
            vec![
                package_one.binpkg_path.clone(),
                package_two.binpkg_path.clone()
            ]
        );
    }

    #[tokio::test]
    async fn restore_final_state_parity_downloads_only_mismatched_files() {
        let root = TempDir::new().expect("temp root");
        let local_repo_metadata = root.path().join("var/db/repos/gentoo/metadata");
        let local_eclass = root.path().join("var/cache/eclass/5-23");
        let local_portage = root.path().join("var/lib/portage");
        tokio::fs::create_dir_all(local_repo_metadata.join("md5-cache"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&local_eclass).await.unwrap();
        tokio::fs::create_dir_all(&local_portage).await.unwrap();

        let reused_path = local_repo_metadata.join("timestamp.chk");
        tokio::fs::write(&reused_path, b"current\n").await.unwrap();
        set_file_mtime(&reused_path, 1_700_000_001).unwrap();

        let restored_relative = "var/db/repos/gentoo/metadata/md5-cache/dev-libs-demo-1.0";
        let eclass_relative = "var/cache/eclass/5-23/amd64.cache";
        let world_relative = "var/lib/portage/world";
        let restored_digest = hex::encode(sha2::Sha256::digest(b"EAPI=8\n"));
        let eclass_digest = hex::encode(sha2::Sha256::digest(b"cache\n"));
        let world_digest = hex::encode(sha2::Sha256::digest(b"app-misc/hello\n"));
        let reused_digest = hex::encode(sha2::Sha256::digest(b"current\n"));

        let requests = Arc::new(Mutex::new(Vec::new()));
        let base_url = spawn_blob_server(
            HashMap::from([
                (restored_digest.clone(), b"EAPI=8\n".to_vec()),
                (eclass_digest.clone(), b"cache\n".to_vec()),
                (world_digest.clone(), b"app-misc/hello\n".to_vec()),
            ]),
            requests.clone(),
        )
        .await;
        let client = RemergeClient::new(&base_url).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: Vec::new(),
            failed_packages: Vec::new(),
            binhost_uri: String::new(),
            fetched_distfiles: BTreeMap::new(),
            parity_manifest: ParityManifest {
                files: std::collections::BTreeMap::from([
                    (
                        "var/db/repos/gentoo/metadata/timestamp.chk".into(),
                        ParityFileEntry {
                            digest: reused_digest,
                            size: 8,
                            mtime_secs: 1_700_000_001,
                        },
                    ),
                    (
                        restored_relative.into(),
                        ParityFileEntry {
                            digest: restored_digest.clone(),
                            size: 7,
                            mtime_secs: 1_700_000_002,
                        },
                    ),
                    (
                        eclass_relative.into(),
                        ParityFileEntry {
                            digest: eclass_digest.clone(),
                            size: 6,
                            mtime_secs: 1_700_000_003,
                        },
                    ),
                    (
                        world_relative.into(),
                        ParityFileEntry {
                            digest: world_digest.clone(),
                            size: 15,
                            mtime_secs: 1_700_000_004,
                        },
                    ),
                ]),
                directories: BTreeMap::new(),
                symlinks: BTreeMap::new(),
            },
        };

        let cli = test_cli();
        cli.restore_final_state_parity_into(&client, &result, root.path())
            .await
            .expect("restore final-state parity");

        let restored_path = root.path().join(restored_relative);
        let eclass_path = root.path().join(eclass_relative);
        let world_path = root.path().join(world_relative);
        assert_eq!(tokio::fs::read(&restored_path).await.unwrap(), b"EAPI=8\n");
        assert_eq!(tokio::fs::read(&eclass_path).await.unwrap(), b"cache\n");
        assert_eq!(
            tokio::fs::read(&world_path).await.unwrap(),
            b"app-misc/hello\n"
        );
        assert_eq!(
            tokio::fs::metadata(&restored_path).await.unwrap().mtime(),
            1_700_000_002
        );
        assert_eq!(
            tokio::fs::metadata(&eclass_path).await.unwrap().mtime(),
            1_700_000_003
        );
        assert_eq!(
            tokio::fs::metadata(&world_path).await.unwrap().mtime(),
            1_700_000_004
        );
        let mut seen_requests = requests.lock().unwrap().clone();
        seen_requests.sort();
        let mut expected_requests = vec![restored_digest, eclass_digest, world_digest];
        expected_requests.sort();
        assert_eq!(seen_requests, expected_requests);
    }

    #[tokio::test]
    async fn no_local_followup_restores_distfiles_without_attempting_parity() {
        let distdir = TempDir::new().expect("distdir root");
        let parity_root = TempDir::new().expect("parity root");

        let distfile_digest = hex::encode(sha2::Sha256::digest(b"hello source"));
        let parity_digest = hex::encode(sha2::Sha256::digest(b"world state"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let base_url = spawn_blob_server(
            HashMap::from([
                (distfile_digest.clone(), b"hello source".to_vec()),
                (parity_digest.clone(), b"world state".to_vec()),
            ]),
            requests.clone(),
        )
        .await;
        let client = RemergeClient::new(&base_url).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: Vec::new(),
            failed_packages: Vec::new(),
            binhost_uri: String::new(),
            fetched_distfiles: BTreeMap::from([(
                "hello-1.0.tar.xz".into(),
                SnapshotEntry {
                    digest: distfile_digest.clone(),
                    size: 12,
                    mtime_secs: 1_700_000_010,
                },
            )]),
            parity_manifest: ParityManifest {
                files: BTreeMap::from([(
                    "var/lib/portage/world".into(),
                    ParityFileEntry {
                        digest: parity_digest,
                        size: 11,
                        mtime_secs: 1_700_000_020,
                    },
                )]),
                directories: BTreeMap::new(),
                symlinks: BTreeMap::new(),
            },
        };

        let mut cli = test_cli();
        cli.no_local = true;
        cli.restore_local_followup_state_into(&client, &result, distdir.path(), parity_root.path())
            .await
            .expect("no-local follow-up should skip parity restore");

        assert_eq!(
            tokio::fs::read(distdir.path().join("hello-1.0.tar.xz"))
                .await
                .unwrap(),
            b"hello source"
        );
        assert!(
            !tokio::fs::try_exists(parity_root.path().join("var/lib/portage/world"))
                .await
                .unwrap()
        );
        assert_eq!(requests.lock().unwrap().clone(), vec![distfile_digest]);
    }

    #[tokio::test]
    async fn reconcile_fetched_distfiles_restores_missing_and_stale_files() {
        let distdir = TempDir::new().expect("temp distdir");
        let reused_path = distdir.path().join("cached.tar.xz");
        tokio::fs::write(&reused_path, b"cached\n").await.unwrap();
        set_file_mtime(&reused_path, 1_700_000_101).unwrap();

        let restored_relative = "dev-libs/demo-1.0.tar.xz";
        let restored_bytes = b"downloaded\n".to_vec();
        let restored_digest = hex::encode(sha2::Sha256::digest(&restored_bytes));
        let reused_digest = hex::encode(sha2::Sha256::digest(b"cached\n"));

        let requests = Arc::new(Mutex::new(Vec::new()));
        let base_url = spawn_blob_server(
            HashMap::from([(restored_digest.clone(), restored_bytes.clone())]),
            requests.clone(),
        )
        .await;
        let client = RemergeClient::new(&base_url).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: Vec::new(),
            failed_packages: Vec::new(),
            binhost_uri: String::new(),
            fetched_distfiles: BTreeMap::from([
                (
                    "cached.tar.xz".into(),
                    SnapshotEntry {
                        digest: reused_digest,
                        size: 7,
                        mtime_secs: 1_700_000_101,
                    },
                ),
                (
                    restored_relative.into(),
                    SnapshotEntry {
                        digest: restored_digest.clone(),
                        size: restored_bytes.len() as u64,
                        mtime_secs: 1_700_000_102,
                    },
                ),
            ]),
            parity_manifest: ParityManifest::default(),
        };

        reconcile_fetched_distfiles_into(distdir.path(), &client, &result)
            .await
            .expect("reconcile fetched distfiles");

        let restored_path = distdir.path().join(restored_relative);
        assert_eq!(
            tokio::fs::read(&restored_path).await.unwrap(),
            restored_bytes
        );
        assert_eq!(
            tokio::fs::metadata(&restored_path).await.unwrap().mtime(),
            1_700_000_102
        );
        assert_eq!(requests.lock().unwrap().clone(), vec![restored_digest]);
    }

    #[test]
    fn resolve_parity_target_rejects_excluded_paths() {
        let error = resolve_parity_target(
            std::path::Path::new("/"),
            "var/db/pkg/sys-apps/portage-3.0.0/CONTENTS",
        )
        .expect_err("excluded VDB path should be rejected");

        assert!(
            error
                .to_string()
                .contains("outside the approved include set"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn restore_final_state_parity_reports_excluded_and_mismatched_paths() {
        let root = TempDir::new().expect("temp root");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let wrong_payload = b"wrong\n".to_vec();
        let expected_digest = hex::encode(sha2::Sha256::digest(b"expected\n"));
        let base_url = spawn_blob_server(
            HashMap::from([(expected_digest.clone(), wrong_payload)]),
            requests,
        )
        .await;
        let client = RemergeClient::new(&base_url).expect("client");
        let result = WorkorderResult {
            workorder_id: uuid::Uuid::nil(),
            built_packages: Vec::new(),
            failed_packages: Vec::new(),
            binhost_uri: String::new(),
            fetched_distfiles: BTreeMap::new(),
            parity_manifest: ParityManifest {
                files: std::collections::BTreeMap::from([
                    (
                        "var/db/pkg/sys-apps/portage-3.0.0/CONTENTS".into(),
                        ParityFileEntry {
                            digest: "ab".repeat(32),
                            size: 4,
                            mtime_secs: 1_700_000_010,
                        },
                    ),
                    (
                        "var/lib/portage/world".into(),
                        ParityFileEntry {
                            digest: expected_digest,
                            size: 9,
                            mtime_secs: 1_700_000_011,
                        },
                    ),
                ]),
                directories: BTreeMap::new(),
                symlinks: BTreeMap::new(),
            },
        };

        let error = test_cli()
            .restore_final_state_parity_into(&client, &result, root.path())
            .await
            .expect_err("parity reconciliation should fail with explicit issue report");

        let message = error.to_string();
        assert!(
            message.contains("excluded parity path var/db/pkg/sys-apps/portage-3.0.0/CONTENTS"),
            "unexpected error: {error:#}"
        );
        assert!(
            message.contains("mismatched parity path var/lib/portage/world"),
            "unexpected error: {error:#}"
        );
        assert!(
            message.contains("failed for 2 path(s)"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn complete_local_followup_waits_for_parity_before_local_emerge() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));

        test_cli()
            .complete_local_followup(
                None,
                {
                    let events = events.clone();
                    move || async move {
                        events.lock().unwrap().push("reconcile-start".to_string());
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        events.lock().unwrap().push("reconcile-done".to_string());
                        Ok(())
                    }
                },
                {
                    let events = events.clone();
                    move || async move {
                        let snapshot = events.lock().unwrap().clone();
                        assert_eq!(
                            snapshot,
                            vec!["reconcile-start".to_string(), "reconcile-done".to_string()]
                        );
                        events.lock().unwrap().push("local-emerge".to_string());
                        Ok(())
                    }
                },
            )
            .await
            .expect("local follow-up should succeed");

        assert_eq!(
            events.lock().unwrap().clone(),
            vec![
                "reconcile-start".to_string(),
                "reconcile-done".to_string(),
                "local-emerge".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn complete_local_followup_blocks_local_emerge_on_parity_failure() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));

        let error = test_cli()
            .complete_local_followup(
                None,
                {
                    let events = events.clone();
                    move || async move {
                        events.lock().unwrap().push("reconcile-failed".to_string());
                        anyhow::bail!("forced parity failure")
                    }
                },
                {
                    let events = events.clone();
                    move || async move {
                        events.lock().unwrap().push("local-emerge".to_string());
                        Ok(())
                    }
                },
            )
            .await
            .expect_err("parity failure should block local emerge");

        assert!(
            error
                .to_string()
                .contains("Failed to restore local follow-up state"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            events.lock().unwrap().clone(),
            vec!["reconcile-failed".to_string()]
        );
    }

    #[tokio::test]
    async fn local_followup_watchdog_reports_the_stuck_stage() {
        let error = Cli::run_with_watchdog(
            "restoring local follow-up state",
            Duration::from_millis(10),
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            },
        )
        .await
        .expect_err("watchdog should time out");

        let message = error.to_string();
        assert!(
            message.contains("CLI watchdog timed out"),
            "unexpected error: {error:#}"
        );
        assert!(
            message.contains("restoring local follow-up state"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn test_workorder_emerge_args_at_most_one_verbosity_flag() {
        // VerboseTrace → exactly one --verbose injected
        let cli = test_cli();
        let args = cli.workorder_emerge_args(Verbosity::VerboseTrace);
        assert_eq!(
            args.iter().filter(|a| *a == "--verbose").count(),
            1,
            "VerboseTrace must inject exactly one --verbose"
        );

        // If --verbose is already present, no extra flag is added
        let cli_v = Cli {
            emerge_args: vec!["--verbose".to_string(), "@world".to_string()],
            ..test_cli()
        };
        let args = cli_v.workorder_emerge_args(Verbosity::VerboseDebug);
        let count_v = args
            .iter()
            .filter(|a| *a == "--verbose" || *a == "-v")
            .count();
        assert_eq!(
            count_v, 1,
            "should not duplicate --verbose when already present"
        );

        // If -v is already present, no extra --verbose is added
        let cli_short = Cli {
            emerge_args: vec!["-v".to_string(), "=dev-libs/foo-1.0".to_string()],
            ..test_cli()
        };
        let args = cli_short.workorder_emerge_args(Verbosity::Verbose);
        let count_sv = args
            .iter()
            .filter(|a| *a == "--verbose" || *a == "-v")
            .count();
        assert_eq!(
            count_sv, 1,
            "should not add --verbose when -v already present"
        );

        // Quiet → exactly one --quiet injected
        let cli = test_cli();
        let args = cli.workorder_emerge_args(Verbosity::Quiet);
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--quiet" || *a == "-q")
                .count(),
            1,
            "Quiet must inject exactly one --quiet"
        );
    }
}
