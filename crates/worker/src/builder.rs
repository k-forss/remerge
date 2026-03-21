//! Package builder — runs `emerge` (or `emerge-<CHOST>` for cross-builds)
//! inside the worker container.
//!
//! Parses emerge output for per-package build events and emits structured
//! `REMERGE_EVENT:` lines that the server can parse from the Docker log stream.
//!
//! Detects common failure patterns:
//! - Missing dependencies (`emerge: there are no ebuilds to satisfy`)
//! - USE flag conflicts (`The following USE changes are necessary`)
//! - Fetch failures (`Couldn't download` / `!!! Fetch failed`)

use std::process::Stdio;
use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
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
    ];

    // Forward any additional emerge arguments from the workorder,
    // but filter out arguments that conflict with our flags.
    for arg in &workorder.emerge_args {
        match arg.as_str() {
            // Skip arguments we already set or that don't make sense in the worker.
            "--pretend" | "-p" | "--getbinpkg" | "-g" |
            // Dangerous flags that must never run in the worker.
            "--depclean" | "--unmerge" | "-C" | "--deselect" |
            "--sync" | "--info" | "--search" | "-s" | "--searchdesc" | "-S" |
            "--config" | "--rage-clean" => continue,
            _ => args.push(arg.clone()),
        }
    }

    // Add the package atoms.
    args.extend(workorder.atoms.iter().cloned());

    info!(cmd = %emerge_cmd, ?args, "Running emerge");

    let mut child = Command::new(emerge_cmd)
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn {emerge_cmd}"))?;

    // Spawn stderr streaming.
    let stderr_handle = child.stderr.take().map(|stderr| {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("{line}");
            }
        })
    });

    // Read and parse stdout for structured build events.
    if let Some(stdout) = child.stdout.take() {
        let re_emerging = Regex::new(r">>> Emerging \(\d+ of \d+\) (.+)::").expect("valid regex");
        let re_completed = Regex::new(r">>> Completed \(\d+ of \d+\) (.+)::").expect("valid regex");
        let re_error = Regex::new(r"\* ERROR: (.+)::(\S+) failed").expect("valid regex");

        // Specialised failure patterns.
        let re_missing_dep =
            Regex::new(r#"emerge: there are no ebuilds to satisfy "(.+?)""#).expect("valid regex");
        let re_use_conflict =
            Regex::new(r"The following USE changes are necessary").expect("valid regex");
        let re_fetch_fail =
            Regex::new(r"(?:Couldn't download|!!! Fetch failed)").expect("valid regex");

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut current_package: Option<(String, Instant)> = None;

        while let Ok(Some(line)) = lines.next_line().await {
            // Always print the raw line to container stdout.
            println!("{line}");

            // Parse for structured events.
            if let Some(caps) = re_emerging.captures(&line) {
                if let Some(atom) = caps.get(1) {
                    current_package = Some((atom.as_str().to_string(), Instant::now()));
                }
            } else if let Some(caps) = re_completed.captures(&line) {
                if let Some(atom_match) = caps.get(1) {
                    let atom = atom_match.as_str().to_string();
                    let duration = current_package
                        .as_ref()
                        .filter(|(pkg, _)| *pkg == atom)
                        .map(|(_, start)| start.elapsed().as_secs())
                        .unwrap_or(0);

                    // Emit structured event for the server to parse.
                    println!(
                        "REMERGE_EVENT:{}",
                        serde_json::json!({
                            "type": "package_built",
                            "atom": atom,
                            "duration_secs": duration,
                        })
                    );
                    current_package = None;
                }
            } else if let Some(caps) = re_error.captures(&line) {
                if let Some(atom_match) = caps.get(1) {
                    let atom = atom_match.as_str().to_string();
                    println!(
                        "REMERGE_EVENT:{}",
                        serde_json::json!({
                            "type": "package_failed",
                            "atom": atom,
                            "reason": line.trim(),
                            "failure_kind": "build_error",
                        })
                    );
                }
            } else if let Some(caps) = re_missing_dep.captures(&line) {
                if let Some(dep) = caps.get(1) {
                    println!(
                        "REMERGE_EVENT:{}",
                        serde_json::json!({
                            "type": "package_failed",
                            "atom": dep.as_str(),
                            "reason": line.trim(),
                            "failure_kind": "missing_dependency",
                        })
                    );
                }
            } else if re_use_conflict.is_match(&line) {
                println!(
                    "REMERGE_EVENT:{}",
                    serde_json::json!({
                        "type": "package_failed",
                        "atom": current_package.as_ref().map(|(a, _)| a.as_str()).unwrap_or("unknown"),
                        "reason": line.trim(),
                        "failure_kind": "use_conflict",
                    })
                );
            } else if re_fetch_fail.is_match(&line) {
                println!(
                    "REMERGE_EVENT:{}",
                    serde_json::json!({
                        "type": "package_failed",
                        "atom": current_package.as_ref().map(|(a, _)| a.as_str()).unwrap_or("unknown"),
                        "reason": line.trim(),
                        "failure_kind": "fetch_failure",
                    })
                );
            }
        }
    }

    let status = child
        .wait()
        .await
        .with_context(|| format!("Failed to wait for {emerge_cmd}"))?;

    // Wait for stderr task to finish.
    if let Some(h) = stderr_handle {
        let _ = h.await;
    }

    if !status.success() {
        anyhow::bail!("{emerge_cmd} exited with status {status}");
    }

    info!("{emerge_cmd} completed successfully");
    Ok(())
}

/// Sync the portage tree.
async fn sync_portage() -> Result<()> {
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
