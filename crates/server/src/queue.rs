//! Workorder queue processor.
//!
//! Runs as a background task, picking up pending workorders, provisioning
//! worker containers, and streaming progress back to connected clients.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use chrono::Utc;
use futures::StreamExt;
use regex::Regex;
use tokio::io::AsyncWriteExt;

use tracing::{error, info, warn};

use remerge_types::workorder::*;

use crate::repo::BinpkgRepo;
use crate::state::AppState;

/// Main queue loop — polls for pending workorders and processes them (FIFO).
///
/// The worker semaphore limits concurrent container starts to `max_workers`.
pub async fn process_queue(state: Arc<AppState>) {
    info!("Workorder queue processor started");

    loop {
        // Find the oldest pending workorder (FIFO).
        let next = {
            let workorders = state.workorders.read().await;
            workorders
                .values()
                .filter(|w| w.status == WorkorderStatus::Pending)
                .min_by_key(|w| w.created_at)
                .cloned()
        };

        if let Some(workorder) = next {
            // Mark as Provisioning *before* spawning so the queue loop
            // never picks up the same workorder on the next iteration.
            {
                let mut workorders = state.workorders.write().await;
                if let Some(w) = workorders.get_mut(&workorder.id) {
                    if w.status != WorkorderStatus::Pending {
                        // Another task already claimed it — skip.
                        continue;
                    }
                    w.status = WorkorderStatus::Provisioning;
                    w.updated_at = chrono::Utc::now();
                }
            }

            // Acquire a semaphore permit before starting the container.
            let permit = state.worker_semaphore.clone().acquire_owned().await;
            match permit {
                Ok(permit) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = process_workorder(&state, workorder).await {
                            error!("Workorder processing failed: {e:#}");
                        }
                        drop(permit); // Release the worker slot.
                    });
                }
                Err(_) => {
                    error!("Worker semaphore closed — shutting down queue processor");
                    break;
                }
            }
        } else {
            // No work — sleep briefly.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}

/// Process a single workorder end-to-end.
async fn process_workorder(state: &Arc<AppState>, workorder: Workorder) -> anyhow::Result<()> {
    let id = workorder.id;
    let build_start = Instant::now();
    info!(?id, "Processing workorder");

    state.metrics.builds_active.fetch_add(1, Ordering::Relaxed);

    let tx = match state.progress_txs.read().await.get(&id).cloned() {
        Some(tx) => tx,
        None => {
            error!(?id, "Progress channel missing — skipping workorder");
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            return Ok(());
        }
    };

    // Helper to update status.
    let set_status = |new_status: WorkorderStatus| {
        let state = state.clone();
        let tx = tx.clone();
        async move {
            let old_status = {
                let mut workorders = state.workorders.write().await;
                if let Some(w) = workorders.get_mut(&id) {
                    let old = w.status.clone();
                    w.status = new_status.clone();
                    w.updated_at = Utc::now();
                    old
                } else {
                    return;
                }
            };
            let _ = tx.send(BuildProgress {
                workorder_id: id,
                event: BuildEvent::StatusChanged {
                    from: old_status,
                    to: new_status,
                },
                timestamp: Utc::now(),
            });
        }
    };

    // ── 1. Provisioning ─────────────────────────────────────────────
    set_status(WorkorderStatus::Provisioning).await;

    let image_tag = state.docker.image_tag(&workorder.system_id);

    // Build image if it doesn't exist or the worker binary has changed.
    if state.docker.image_needs_rebuild(&image_tag).await {
        info!(%image_tag, "Worker image needs (re)building");
        if let Err(e) = state
            .docker
            .build_worker_image(&workorder.system_id, &image_tag)
            .await
        {
            let reason = format!("Failed to build worker image: {e:#}");
            set_status(WorkorderStatus::Failed {
                reason: reason.clone(),
            })
            .await;
            let _ = tx.send(BuildProgress {
                workorder_id: id,
                event: BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
                timestamp: Utc::now(),
            });
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            state
                .clients
                .clear_active_workorder(&workorder.client_id)
                .await;
            return Err(e);
        }
    }

    // Record image usage for idle timeout tracking.
    state
        .image_last_used
        .write()
        .await
        .insert(image_tag.clone(), Instant::now());

    // ── 2. Start worker container ───────────────────────────────────
    set_status(WorkorderStatus::Building).await;

    let container_name = format!("remerge-worker-{}", id.as_simple());
    let workorder_json = serde_json::to_string(&workorder)?;

    let container_id = match state
        .docker
        .start_worker(&container_name, &image_tag, &workorder_json, &state.config)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            let reason = format!("Failed to start worker: {e:#}");
            set_status(WorkorderStatus::Failed {
                reason: reason.clone(),
            })
            .await;
            let _ = tx.send(BuildProgress {
                workorder_id: id,
                event: BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
                timestamp: Utc::now(),
            });
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            state
                .clients
                .clear_active_workorder(&workorder.client_id)
                .await;
            return Err(e);
        }
    };

    // Track container ID for cancellation support.
    state
        .container_ids
        .write()
        .await
        .insert(id, container_id.clone());

    // ── 3. Attach to container for bidirectional I/O ─────────────────
    //
    // We use Docker attach (not logs) so that we get both an output stream
    // AND a stdin writer.  This lets interactive emerge prompts (--ask,
    // USE flag changes, etc.) flow through to the connected client.
    let log_container_id = container_id.clone();
    let log_state = state.clone();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<WorkerEvent>(64);

    // Get the raw output broadcast sender for binary PTY relay.
    let raw_tx = state
        .raw_output_txs
        .read()
        .await
        .get(&id)
        .cloned()
        .expect("raw_output_tx must exist — created alongside progress channel");

    // Create the stdin channel so the WebSocket handler can forward input.
    let mut stdin_rx = state.create_stdin_channel(id).await;

    let mut log_handle = tokio::spawn(async move {
        let re_emerging = Regex::new(r">>> Emerging \(\d+ of \d+\) (.+)::").unwrap();
        let re_completed = Regex::new(r">>> Completed \(\d+ of \d+\) (.+)::").unwrap();
        let re_error = Regex::new(r"\* ERROR: (.+)::(\S+) failed").unwrap();

        // Specialised failure patterns.
        let re_missing_dep =
            Regex::new(r#"emerge: there are no ebuilds to satisfy "(.+?)""#).unwrap();
        let re_use_conflict = Regex::new(r"The following USE changes are necessary").unwrap();
        let re_fetch_fail = Regex::new(r"(?:Couldn't download|!!! Fetch failed)").unwrap();

        // Attach to the container to get bidirectional streams.
        let attach_result = log_state.docker.attach_container(&log_container_id).await;
        let (mut output, mut input) = match attach_result {
            Ok(result) => (result.output, result.input),
            Err(e) => {
                error!("Failed to attach to container: {e:#}");
                return;
            }
        };

        // Spawn a task that forwards stdin from the WebSocket → container.
        let stdin_handle = tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if input.write_all(&data).await.is_err() {
                    break;
                }
                if input.flush().await.is_err() {
                    break;
                }
            }
        });

        let mut current_package: Option<(String, Instant)> = None;
        let mut line_buf = Vec::with_capacity(8192);

        while let Some(result) = output.next().await {
            match result {
                Ok(log_output) => {
                    // `into_bytes()` already returns `Bytes` (reference-counted),
                    // so the broadcast clone is a cheap pointer increment rather
                    // than a full copy.
                    let raw_bytes: bytes::Bytes = log_output.into_bytes();

                    // Accumulate bytes for line-based event detection before
                    // broadcasting, so we only hold one allocation.
                    line_buf.extend_from_slice(&raw_bytes);

                    // Send raw bytes to connected clients for direct PTY relay.
                    let _ = raw_tx.send(raw_bytes);

                    // Cap buffer to prevent unbounded growth.
                    const MAX_LINE_BUF: usize = 64 * 1024;
                    if line_buf.len() > MAX_LINE_BUF {
                        line_buf.drain(..line_buf.len() - MAX_LINE_BUF);
                    }

                    // Process complete lines for structured event detection.
                    while let Some(newline_pos) = line_buf.iter().position(|&b| b == b'\n') {
                        let line = String::from_utf8_lossy(&line_buf[..newline_pos])
                            .trim_end_matches('\r')
                            .to_string();
                        line_buf.drain(..=newline_pos);

                        // Check for structured events emitted by the worker.
                        if let Some(json_str) = line.strip_prefix("REMERGE_EVENT:")
                            && let Ok(event) = serde_json::from_str::<WorkerEvent>(json_str)
                        {
                            let _ = event_tx.send(event).await;
                            continue;
                        }

                        // Parse emerge output patterns.
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
                                let _ = event_tx
                                    .send(WorkerEvent::PackageBuilt {
                                        atom: atom.clone(),
                                        duration_secs: duration,
                                    })
                                    .await;
                                current_package = None;
                            }
                        } else if let Some(caps) = re_error.captures(&line) {
                            if let Some(atom_match) = caps.get(1) {
                                let _ = event_tx
                                    .send(WorkerEvent::PackageFailed {
                                        atom: atom_match.as_str().to_string(),
                                        reason: line.clone(),
                                    })
                                    .await;
                            }
                        } else if let Some(caps) = re_missing_dep.captures(&line) {
                            if let Some(dep) = caps.get(1) {
                                let _ = event_tx
                                    .send(WorkerEvent::PackageFailed {
                                        atom: dep.as_str().to_string(),
                                        reason: format!("Missing dependency: {line}"),
                                    })
                                    .await;
                            }
                        } else if re_use_conflict.is_match(&line) {
                            let atom = current_package
                                .as_ref()
                                .map(|(a, _)| a.clone())
                                .unwrap_or_else(|| "unknown".into());
                            let _ = event_tx
                                .send(WorkerEvent::PackageFailed {
                                    atom,
                                    reason: format!("USE flag conflict: {line}"),
                                })
                                .await;
                        } else if re_fetch_fail.is_match(&line) {
                            let atom = current_package
                                .as_ref()
                                .map(|(a, _)| a.clone())
                                .unwrap_or_else(|| "unknown".into());
                            let _ = event_tx
                                .send(WorkerEvent::PackageFailed {
                                    atom,
                                    reason: format!("Fetch failure: {line}"),
                                })
                                .await;
                        }
                    }
                }
                Err(e) => {
                    warn!("Attach stream error: {e}");
                    break;
                }
            }
        }

        // Output stream ended — abort the stdin forwarder.
        stdin_handle.abort();
    });

    // ── 4. Wait for container to finish ─────────────────────────────
    let exit_code = state.docker.wait_container(&container_id).await?;

    // Give the log stream a moment to flush final lines, then abort if
    // it hasn't finished.  This avoids losing the last few log lines.
    let log_task_finished = tokio::select! {
        _ = &mut log_handle => true,
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            warn!("Log stream did not finish within 5 s — aborting");
            log_handle.abort();
            false
        }
    };

    // Ensure the task is fully complete so its captured `raw_tx` clone
    // is dropped.  Without this, the raw broadcast channel stays open
    // and the WS handler never transitions to text-only mode.
    // Only await here if the select above did not already consume it;
    // awaiting a JoinHandle a second time after it completed is unsafe.
    if !log_task_finished {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            log_handle,
        )
        .await;
    }

    // Close the raw PTY channel *before* sending Finished so the WS
    // handler transitions to text-only mode and is guaranteed to pick
    // up the Finished event on the progress channel.
    state.raw_output_txs.write().await.remove(&id);

    info!(?id, exit_code, "Worker container finished");

    // ── 5. Collect structured events ────────────────────────────────
    let mut built_atoms = Vec::new();
    let mut failed_atoms = Vec::new();

    // Drain remaining events from the channel.
    drop(state.progress_txs.read().await); // ensure log task can finish
    while let Ok(event) = event_rx.try_recv() {
        match event {
            WorkerEvent::PackageBuilt {
                atom,
                duration_secs,
            } => {
                let _ = tx.send(BuildProgress {
                    workorder_id: id,
                    event: BuildEvent::PackageBuilt {
                        atom: atom.clone(),
                        duration_secs,
                    },
                    timestamp: Utc::now(),
                });
                built_atoms.push(atom);
            }
            WorkerEvent::PackageFailed { atom, reason } => {
                let _ = tx.send(BuildProgress {
                    workorder_id: id,
                    event: BuildEvent::PackageFailed {
                        atom: atom.clone(),
                        reason: reason.clone(),
                    },
                    timestamp: Utc::now(),
                });
                failed_atoms.push(FailedPackage {
                    atom,
                    reason,
                    build_log: None,
                });
            }
        }
    }

    // ── 6. Scan binpkg directory for real results ───────────────────
    let build_duration = build_start.elapsed().as_secs();

    if exit_code == 0 {
        set_status(WorkorderStatus::Completed).await;

        // Scan the binpkg directory for actual files and compute hashes.
        let repo = BinpkgRepo::new(state.config.binpkg_dir.clone());
        let built_packages = match repo.scan_packages().await {
            Ok(metas) => metas
                .into_iter()
                .filter(|m| {
                    // Match scanned files to requested or completed atoms via
                    // exact category/package-name (not a loose substring).
                    let scanned_base = crate::repo::extract_package_base(&m.cpv);
                    workorder.atoms.iter().any(|atom| {
                        let atom_base = atom.trim_start_matches(|c: char| ">=<~!".contains(c));
                        crate::repo::extract_package_base(atom_base) == scanned_base
                    }) || built_atoms
                        .iter()
                        .any(|a| crate::repo::extract_package_base(a) == scanned_base)
                })
                .map(|m| BuiltPackage {
                    atom: m.cpv.clone(),
                    binpkg_path: m.relative_path.clone(),
                    sha256: m.sha256.clone(),
                    size: m.size,
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                warn!("Failed to scan binpkg directory: {e:#}");
                // Fall back to reporting from structured events.
                built_atoms
                    .iter()
                    .map(|atom| BuiltPackage {
                        atom: atom.clone(),
                        binpkg_path: format!("{atom}.gpkg.tar"),
                        sha256: String::new(),
                        size: 0,
                    })
                    .collect()
            }
        };

        // Regenerate the Packages index for portage.
        if let Err(e) = repo.regenerate_index().await {
            warn!("Failed to regenerate Packages index: {e:#}");
        }

        let result = WorkorderResult {
            workorder_id: id,
            built_packages: if built_packages.is_empty() {
                // If scanning didn't match, use events.
                built_atoms
                    .iter()
                    .map(|atom| BuiltPackage {
                        atom: atom.clone(),
                        binpkg_path: format!("{atom}.gpkg.tar"),
                        sha256: String::new(),
                        size: 0,
                    })
                    .collect()
            } else {
                built_packages
            },
            failed_packages: failed_atoms,
            binhost_uri: state.config.binhost_url.clone(),
        };

        let built_list: Vec<String> = result
            .built_packages
            .iter()
            .map(|p| p.atom.clone())
            .collect();
        let failed_list: Vec<String> = result
            .failed_packages
            .iter()
            .map(|p| p.atom.clone())
            .collect();

        // Store result before broadcasting Finished so the client's
        // REST fetch (triggered by the Finished event) always finds it.
        state.results.write().await.insert(id, result);

        let _ = tx.send(BuildProgress {
            workorder_id: id,
            event: BuildEvent::Finished {
                built: built_list,
                failed: failed_list,
            },
            timestamp: Utc::now(),
        });

        state
            .metrics
            .workorders_completed
            .fetch_add(1, Ordering::Relaxed);
    } else {
        let reason = format!("Worker exited with code {exit_code}");
        set_status(WorkorderStatus::Failed {
            reason: reason.clone(),
        })
        .await;

        // Store a result even on failure so the client's REST fetch
        // returns something useful instead of "no result".
        let result = WorkorderResult {
            workorder_id: id,
            built_packages: built_atoms
                .iter()
                .map(|atom| BuiltPackage {
                    atom: atom.clone(),
                    binpkg_path: format!("{atom}.gpkg.tar"),
                    sha256: String::new(),
                    size: 0,
                })
                .collect(),
            failed_packages: if failed_atoms.is_empty() {
                // No structured failure events — report all atoms as failed.
                workorder
                    .atoms
                    .iter()
                    .map(|atom| FailedPackage {
                        atom: atom.clone(),
                        reason: reason.clone(),
                        build_log: None,
                    })
                    .collect()
            } else {
                failed_atoms
            },
            binhost_uri: state.config.binhost_url.clone(),
        };

        let built_list: Vec<String> = result
            .built_packages
            .iter()
            .map(|p| p.atom.clone())
            .collect();
        let failed_list: Vec<String> = result
            .failed_packages
            .iter()
            .map(|p| p.atom.clone())
            .collect();

        // Store result before broadcasting Finished so the client's
        // REST fetch (triggered by the Finished event) always finds it.
        state.results.write().await.insert(id, result);

        let _ = tx.send(BuildProgress {
            workorder_id: id,
            event: BuildEvent::Finished {
                built: built_list,
                failed: failed_list,
            },
            timestamp: Utc::now(),
        });

        state
            .metrics
            .workorders_failed
            .fetch_add(1, Ordering::Relaxed);
    }

    state
        .metrics
        .builds_total_duration_secs
        .fetch_add(build_duration, Ordering::Relaxed);
    state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);

    // ── 7. Cleanup ──────────────────────────────────────────────────
    state.container_ids.write().await.remove(&id);
    state.remove_workorder_channels(&id).await;
    state
        .clients
        .clear_active_workorder(&workorder.client_id)
        .await;

    if let Err(e) = state.docker.remove_container(&container_id).await {
        warn!("Failed to remove container: {e}");
    }

    Ok(())
}

/// Structured event emitted by the worker process.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerEvent {
    PackageBuilt { atom: String, duration_secs: u64 },
    PackageFailed { atom: String, reason: String },
}
