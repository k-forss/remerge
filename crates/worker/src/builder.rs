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
/// repos directory into the container), syncing is skipped entirely.  This
/// avoids re-downloading the tree on every build and ensures the worker
/// uses the exact same ebuild repo as the server.
async fn sync_portage() -> Result<()> {
    if std::env::var("REMERGE_SKIP_SYNC").is_ok() {
        info!("Skipping portage sync (repos are bind-mounted from the server)");
        return Ok(());
    }

    info!("Syncing portage tree");

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
