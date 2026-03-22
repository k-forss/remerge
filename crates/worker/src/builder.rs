//! Package builder — runs `emerge` (or `emerge-<CHOST>` for cross-builds)
//! inside the worker container.
//!
//! The worker inherits the container's PTY for stdin/stdout/stderr so that
//! emerge output flows directly through the Docker attach stream to the
//! server, which relays it as raw binary WebSocket frames to the CLI.
//! The server parses the output stream for structured events (package
//! built/failed, etc.) — the worker does not need to parse emerge output.

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::info;

use remerge_types::workorder::Workorder;

use crate::portage_setup::{self, RepoSection};

/// Build all packages in the workorder using emerge.
///
/// `emerge_cmd` is either `"emerge"` (native) or `"emerge-<CHOST>"` (cross).
pub async fn build_packages(workorder: &Workorder, emerge_cmd: &str) -> Result<()> {
    // Sync the portage tree first.
    sync_portage().await?;

    // Build each atom with --buildpkg so binary packages are created.
    let mut args = vec![
        "--buildpkg".to_string(),
        "--usepkg".to_string(),
        "--verbose".to_string(),
        "--color=y".to_string(),
        "--keep-going".to_string(),
        // --newuse and --update are essential: the container may have
        // pre-installed packages built with different USE/PYTHON_TARGETS
        // than the client's config.  Without these, emerge would report
        // slot conflicts instead of rebuilding the mismatched packages.
        "--newuse".to_string(),
        "--update".to_string(),
        // Auto-apply USE / keyword changes that emerge suggests (e.g.
        // REQUIRED_USE constraints like `wayland? ( gles2 )`, missing
        // keywords, etc.) and continue the build without prompting.
        "--autounmask-write".to_string(),
        "--autounmask-continue".to_string(),
    ];

    // Forward any additional emerge arguments from the workorder,
    // but filter out arguments that conflict with our flags.
    for arg in &workorder.emerge_args {
        match arg.as_str() {
            // Skip arguments we already set or that don't make sense in the worker.
            "--pretend" | "-p" | "--getbinpkg" | "-g" |
            "--newuse" | "-N" | "--update" | "-u" |
            "--autounmask-write" | "--autounmask-continue" |
            // Dangerous flags that must never run in the worker.
            "--depclean" | "--unmerge" | "-C" | "--deselect" |
            "--sync" | "--info" | "--search" | "-s" | "--searchdesc" | "-S" |
            "--config" | "--rage-clean" => continue,
            _ => args.push(arg.clone()),
        }
    }

    // Add the package atoms.
    args.extend(workorder.atoms.iter().cloned());

    // Warn about expensive operations that are technically valid but risky.
    if args.iter().any(|a| a == "--emptytree" || a == "-e") {
        tracing::warn!(
            "--emptytree requested — this will rebuild the entire dependency \
             tree from scratch and may take many hours"
        );
    }

    info!(cmd = %emerge_cmd, ?args, "Running emerge");

    // Inherit the container PTY for all stdio — emerge output goes directly
    // through the Docker attach stream to the server.  The server handles
    // parsing for structured events; we just need the exit code.
    let status = Command::new(emerge_cmd)
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("Failed to spawn {emerge_cmd}"))?;

    if !status.success() {
        anyhow::bail!("{emerge_cmd} exited with status {status}");
    }

    info!("{emerge_cmd} completed successfully");
    Ok(())
}

/// Sync the portage tree.
///
/// When `REMERGE_SKIP_SYNC=1` is set (i.e. the server bind-mounted its own
/// repos directory into the container), the main tree sync is skipped.
/// However, overlay repos that are NOT present in the bind-mount are still
/// synced individually so the worker has the exact same ebuild set as the
/// client.
///
/// When `REMERGE_SKIP_SYNC` is not set, `emerge --sync` syncs ALL
/// configured repositories (gentoo + overlays) in one go.
async fn sync_portage() -> Result<()> {
    if std::env::var("REMERGE_SKIP_SYNC").is_ok() {
        info!("Main repo sync skipped (repos are bind-mounted from the server)");
        // Overlays not present on the server still need to be synced.
        sync_missing_repos().await?;
        return Ok(());
    }

    info!("Syncing all configured repos");

    let status = Command::new("emerge")
        .args(["--sync", "--quiet"])
        .status()
        .await
        .context("Failed to sync portage")?;

    if !status.success() {
        // Non-fatal — the tree might already be reasonably up to date.
        tracing::warn!("Portage sync returned non-zero (continuing anyway)");
    }

    Ok(())
}

/// Sync overlay repos whose location directory is empty or missing.
///
/// When repos are bind-mounted from the server, the main gentoo tree is
/// already available.  But client overlays (layman, eselect-repository,
/// GURU, etc.) may not be present on the server.  This function detects
/// those and syncs them individually via `emaint sync -r <name>`.
///
/// Overlays whose `sync-uri` uses an SSH or authenticated URI scheme
/// (`git@`, `ssh://`, etc.) are skipped because the worker container
/// lacks the client's SSH keys.
async fn sync_missing_repos() -> Result<()> {
    let repos = discover_configured_repos().await;
    let mut synced = 0usize;
    let mut skipped = 0usize;

    for repo in &repos {
        if is_repo_populated(&repo.location) {
            continue;
        }

        // Skip repos whose sync-uri requires authentication.
        if let Some(ref uri) = repo.sync_uri
            && requires_auth(uri)
        {
            info!(
                repo = %repo.name,
                sync_uri = %uri,
                "Skipping overlay — sync-uri requires authentication"
            );
            skipped += 1;
            continue;
        }

        info!(repo = %repo.name, location = %repo.location, "Syncing missing overlay");
        let status = Command::new("emaint")
            .args(["sync", "-r", &repo.name])
            .status()
            .await
            .with_context(|| format!("Failed to sync overlay {}", repo.name))?;

        if status.success() {
            synced += 1;
        } else {
            tracing::warn!(repo = %repo.name, "Overlay sync failed (continuing anyway)");
        }
    }

    if synced > 0 {
        info!(synced, "Synced missing overlay repos");
    }
    if skipped > 0 {
        info!(skipped, "Skipped overlays requiring authentication");
    }
    Ok(())
}

/// Returns `true` if a sync-uri requires authentication (SSH, etc.)
/// and would fail inside an ephemeral worker container.
fn requires_auth(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();

    // Explicit SSH scheme URIs.
    if lower.starts_with("ssh://") || lower.starts_with("git+ssh://") {
        return true;
    }

    // scp-like syntax: user@host:path — no scheme present, '@' before ':',
    // no '/' before '@' (which would indicate a path component in an HTTP URL).
    if !uri.contains("://")
        && let Some(at_pos) = uri.find('@')
        && uri[at_pos + 1..].starts_with(|c: char| c.is_ascii_alphanumeric())
        && uri[..at_pos].chars().all(|c| c != '/')
    {
        return true;
    }

    false
}

/// Read `/etc/portage/repos.conf/` and return metadata for all configured
/// repositories — including sync-uri when present.
async fn discover_configured_repos() -> Vec<RepoSection> {
    let conf_path = std::path::Path::new("/etc/portage/repos.conf");
    let mut repos = Vec::new();

    if conf_path.is_dir() {
        let Ok(mut dir) = tokio::fs::read_dir(conf_path).await else {
            return repos;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            if let Ok(content) = tokio::fs::read_to_string(entry.path()).await {
                repos.extend(portage_setup::parse_repo_sections_full(&content));
            }
        }
    } else if conf_path.is_file()
        && let Ok(content) = tokio::fs::read_to_string(conf_path).await
    {
        repos.extend(portage_setup::parse_repo_sections_full(&content));
    }

    repos
}

/// A repo is "populated" if its location directory contains a `profiles/`
/// subdirectory (the minimum structure for a valid portage repository).
/// Symlinked directories (pointing to bind-mounted repos) also pass this
/// check because the symlink target is a fully populated repo tree.
fn is_repo_populated(location: &str) -> bool {
    std::path::Path::new(location).join("profiles").exists()
}
