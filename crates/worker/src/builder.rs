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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    let mut child = Command::new(emerge_cmd)
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn {emerge_cmd}"))?;

    // Spawn stderr streaming — forward raw bytes to preserve formatting.
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut writer = tokio::io::stderr();
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = writer.write_all(&buf[..n]).await;
                        let _ = writer.flush().await;
                    }
                }
            }
        })
    });

    // Read and parse stdout for structured build events.
    //
    // We read raw bytes and forward them to container stdout immediately,
    // preserving exact formatting (ANSI escape codes, \r\n line endings,
    // partial lines for progress bars, etc.).  A separate line buffer
    // accumulates bytes for regex-based event detection.
    if let Some(mut stdout) = child.stdout.take() {
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

        let mut stdout_writer = tokio::io::stdout();
        let mut line_buf = Vec::with_capacity(8192);
        let mut read_buf = [0u8; 4096];
        let mut current_package: Option<(String, Instant)> = None;
        // Track whether the last byte written to stdout ended on a newline,
        // so we can ensure REMERGE_EVENT lines always start on a fresh line.
        // Starts `true` because we haven't written anything yet (BOF = fresh line).
        #[allow(unused_assignments)]
        let mut last_was_newline = true;

        loop {
            let n = match stdout.read(&mut read_buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };

            // Forward raw bytes to container stdout immediately.
            let _ = stdout_writer.write_all(&read_buf[..n]).await;
            let _ = stdout_writer.flush().await;
            last_was_newline = read_buf[n - 1] == b'\n';

            // Accumulate for line-based event parsing.
            line_buf.extend_from_slice(&read_buf[..n]);

            // Cap buffer to prevent unbounded growth from long lines
            // (e.g. progress bars using \r without \n).
            const MAX_LINE_BUF: usize = 64 * 1024;
            if line_buf.len() > MAX_LINE_BUF {
                line_buf.drain(..line_buf.len() - MAX_LINE_BUF);
            }

            // Process complete lines from the buffer.
            while let Some(newline_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&line_buf[..newline_pos])
                    .trim_end_matches('\r')
                    .to_string();
                line_buf.drain(..=newline_pos);

                // Helper: emit a structured event on a guaranteed fresh line.
                let mut emit_event = |json: serde_json::Value| {
                    let prefix = if last_was_newline { "" } else { "\n" };
                    let msg = format!("{prefix}REMERGE_EVENT:{json}\n");
                    let _ = std::io::Write::write_all(&mut std::io::stdout(), msg.as_bytes());
                    last_was_newline = true;
                };

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

                        emit_event(serde_json::json!({
                            "type": "package_built",
                            "atom": atom,
                            "duration_secs": duration,
                        }));
                        current_package = None;
                    }
                } else if let Some(caps) = re_error.captures(&line) {
                    if let Some(atom_match) = caps.get(1) {
                        let atom = atom_match.as_str().to_string();
                        emit_event(serde_json::json!({
                            "type": "package_failed",
                            "atom": atom,
                            "reason": line.trim(),
                            "failure_kind": "build_error",
                        }));
                    }
                } else if let Some(caps) = re_missing_dep.captures(&line) {
                    if let Some(dep) = caps.get(1) {
                        emit_event(serde_json::json!({
                            "type": "package_failed",
                            "atom": dep.as_str(),
                            "reason": line.trim(),
                            "failure_kind": "missing_dependency",
                        }));
                    }
                } else if re_use_conflict.is_match(&line) {
                    emit_event(serde_json::json!({
                        "type": "package_failed",
                        "atom": current_package.as_ref().map(|(a, _)| a.as_str()).unwrap_or("unknown"),
                        "reason": line.trim(),
                        "failure_kind": "use_conflict",
                    }));
                } else if re_fetch_fail.is_match(&line) {
                    emit_event(serde_json::json!({
                        "type": "package_failed",
                        "atom": current_package.as_ref().map(|(a, _)| a.as_str()).unwrap_or("unknown"),
                        "reason": line.trim(),
                        "failure_kind": "fetch_failure",
                    }));
                }
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
