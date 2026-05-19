//! Workorder queue processor.
//!
//! Runs as a background task, picking up pending workorders, provisioning
//! worker containers, and streaming progress back to connected clients.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt;
use regex::Regex;
use tokio::io::AsyncWriteExt;

use tracing::{Instrument, error, info, warn};

use remerge_types::workorder::*;

use crate::persistence;
use crate::repo::BinpkgRepo;
use crate::runtime;
use crate::state::AppState;

fn failed_packages_for_atoms(atoms: &[String], reason: &str) -> Vec<FailedPackage> {
    atoms
        .iter()
        .map(|atom| FailedPackage {
            atom: atom.clone(),
            reason: reason.to_string(),
            build_log: None,
        })
        .collect()
}

fn workorder_trace_id(workorder: &Workorder) -> Option<String> {
    workorder
        .trace_context
        .as_ref()
        .map(|ctx| ctx.trace_id.clone())
}

fn build_progress(
    workorder_id: WorkorderId,
    trace_id: &Option<String>,
    event: BuildEvent,
) -> BuildProgress {
    BuildProgress {
        workorder_id,
        trace_id: trace_id.clone(),
        event,
        timestamp: Utc::now(),
    }
}

async fn set_workorder_status(
    state: &Arc<AppState>,
    tx: &tokio::sync::broadcast::Sender<BuildProgress>,
    workorder_id: WorkorderId,
    trace_id: &Option<String>,
    new_status: WorkorderStatus,
) {
    let old_status = {
        let mut workorders = state.workorders.write().await;
        if let Some(workorder) = workorders.get_mut(&workorder_id) {
            let old = workorder.status.clone();
            workorder.status = new_status.clone();
            workorder.updated_at = Utc::now();
            old
        } else {
            return;
        }
    };

    let _ = tx.send(build_progress(
        workorder_id,
        trace_id,
        BuildEvent::StatusChanged {
            from: old_status,
            to: new_status,
        },
    ));

    let workorders = {
        let workorders = state.workorders.read().await;
        workorders.clone()
    };
    if let Err(error) =
        persistence::save_workorders(state.config.state_dir.as_path(), &workorders).await
    {
        warn!(
            ?workorder_id,
            "Failed to persist workorder state change: {error:#}"
        );
    }
}

async fn cleanup_workorder_runtime(
    state: &Arc<AppState>,
    workorder: &Workorder,
    id: WorkorderId,
    container_id: Option<&str>,
) {
    state.container_ids.write().await.remove(&id);
    state.clear_staged_workorder_references(&id).await;
    state.remove_workorder_channels(&id).await;
    state
        .clients
        .clear_active_workorder(&workorder.client_id)
        .await;

    let cleanup_success = if let Some(container_id) = container_id {
        match state.docker.remove_container(container_id).await {
            Ok(()) => true,
            Err(e) => {
                warn!(
                    ?id,
                    trace_id = workorder
                        .trace_context
                        .as_ref()
                        .map(|ctx| ctx.trace_id.as_str())
                        .unwrap_or("unknown"),
                    "Failed to remove container: {e}"
                );
                false
            }
        }
    } else {
        true
    };

    if let Err(e) = runtime::cleanup_workorder_runtime(&state.config.state_dir, id).await {
        warn!(?id, "Failed to clean up staged runtime dir: {e:#}");
    }

    let cleanup_state = Arc::clone(state);
    tokio::spawn(async move {
        let active_references: Vec<_> = cleanup_state
            .staged_workorder_references
            .read()
            .await
            .values()
            .cloned()
            .collect();
        match runtime::cleanup_snapshot_storage(
            &cleanup_state.config.state_dir,
            &cleanup_state.config,
            &active_references,
        )
        .await
        {
            Ok(summary) if summary.deleted_blobs != 0 || summary.deleted_trees != 0 => {
                cleanup_state
                    .metrics
                    .record_cleanup_reclaimed_bytes(summary.reclaimed_bytes);
                info!(
                    deleted_blobs = summary.deleted_blobs,
                    deleted_trees = summary.deleted_trees,
                    reclaimed_bytes = summary.reclaimed_bytes,
                    "Cleaned unreferenced snapshot cache entries"
                )
            }
            Ok(_) => {}
            Err(error) => warn!(?id, "Failed snapshot cache cleanup pass: {error:#}"),
        }
    });

    state.metrics.record_cleanup(cleanup_success);
    if cleanup_success {
        info!(
            ?id,
            trace_id = workorder
                .trace_context
                .as_ref()
                .map(|ctx| ctx.trace_id.as_str())
                .unwrap_or("unknown"),
            "Cleaned up workorder runtime resources"
        );
    }
}

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
            let claimed = {
                let mut workorders = state.workorders.write().await;
                if let Some(w) = workorders.get_mut(&workorder.id) {
                    if w.status != WorkorderStatus::Pending {
                        false
                    } else {
                        w.status = WorkorderStatus::Provisioning;
                        w.updated_at = chrono::Utc::now();
                        true
                    }
                } else {
                    false
                }
            };

            if !claimed {
                continue;
            }

            state.metrics.queue_depth.fetch_sub(1, Ordering::Relaxed);

            if let Some(tx) = state.progress_txs.read().await.get(&workorder.id).cloned() {
                let trace_id = workorder_trace_id(&workorder);
                let _ = tx.send(build_progress(
                    workorder.id,
                    &trace_id,
                    BuildEvent::StatusChanged {
                        from: WorkorderStatus::Pending,
                        to: WorkorderStatus::Provisioning,
                    },
                ));
            }

            // Acquire a semaphore permit before starting the container.
            let permit = state.worker_semaphore.clone().acquire_owned().await;
            match permit {
                Ok(permit) => {
                    let state = state.clone();
                    let trace_id = workorder_trace_id(&workorder);
                    let span = tracing::info_span!(
                        "process_workorder",
                        workorder_id = %workorder.id,
                        trace_id = trace_id.as_deref().unwrap_or("unknown")
                    );
                    remerge_observability::set_span_parent(&span, workorder.trace_context.as_ref());

                    tokio::spawn(
                        async move {
                            if let Err(e) = process_workorder(&state, workorder).await {
                                error!("Workorder processing failed: {e:#}");
                            }
                            drop(permit); // Release the worker slot.
                        }
                        .instrument(span),
                    );
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

/// Run the emerge-output regex matchers against a single complete line and
/// dispatch the appropriate `WorkerEvent` when a pattern matches.
///
/// This is factored out so the streaming PTY filter can call it from two
/// different code paths (short line at line-start, and end-of-line in
/// passthrough mode) without duplicating the match logic.
#[allow(clippy::too_many_arguments)]
async fn match_emerge_line(
    line: &str,
    re_emerging: &Regex,
    re_completed: &Regex,
    re_error: &Regex,
    re_missing_dep: &Regex,
    re_use_conflict: &Regex,
    re_fetch_fail: &Regex,
    current_package: &mut Option<(String, Instant)>,
    event_tx: &tokio::sync::mpsc::Sender<WorkerEvent>,
) {
    if let Some(caps) = re_emerging.captures(line) {
        if let Some(atom) = caps.get(1) {
            *current_package = Some((atom.as_str().to_string(), Instant::now()));
        }
    } else if let Some(caps) = re_completed.captures(line) {
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
            *current_package = None;
        }
    } else if let Some(caps) = re_error.captures(line) {
        if let Some(atom_match) = caps.get(1) {
            let _ = event_tx
                .send(WorkerEvent::PackageFailed {
                    atom: atom_match.as_str().to_string(),
                    reason: line.to_string(),
                })
                .await;
        }
    } else if let Some(caps) = re_missing_dep.captures(line) {
        if let Some(dep) = caps.get(1) {
            let _ = event_tx
                .send(WorkerEvent::PackageFailed {
                    atom: dep.as_str().to_string(),
                    reason: format!("Missing dependency: {line}"),
                })
                .await;
        }
    } else if re_use_conflict.is_match(line) {
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
    } else if re_fetch_fail.is_match(line) {
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

/// Process a single workorder end-to-end.
async fn process_workorder(state: &Arc<AppState>, workorder: Workorder) -> anyhow::Result<()> {
    let id = workorder.id;
    let build_start = Instant::now();
    let trace_id = workorder_trace_id(&workorder);
    info!(
        ?id,
        trace_id = trace_id.as_deref().unwrap_or("unknown"),
        "Processing workorder"
    );

    state.metrics.builds_active.fetch_add(1, Ordering::Relaxed);

    let tx = match state.progress_txs.read().await.get(&id).cloned() {
        Some(tx) => tx,
        None => {
            error!(?id, "Progress channel missing — skipping workorder");
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            return Ok(());
        }
    };

    // ── 1. Worker image prep ───────────────────────────────────────
    let image_tag = state.docker.image_tag(&workorder.system_id);

    // Build image if it doesn't exist or the worker binary has changed.
    if state.docker.image_needs_rebuild(&image_tag).await {
        info!(%image_tag, "Worker image needs (re)building");
        let image_build_start = Instant::now();
        let image_build_result = state
            .docker
            .build_worker_image(&workorder.system_id, &image_tag)
            .await;
        state
            .metrics
            .record_worker_image_build(image_build_start.elapsed().as_secs());

        if let Err(e) = image_build_result {
            let reason = format!("Failed to build worker image: {e:#}");
            set_workorder_status(
                state,
                &tx,
                id,
                &trace_id,
                WorkorderStatus::Failed {
                    reason: reason.clone(),
                },
            )
            .await;
            let _ = tx.send(build_progress(
                id,
                &trace_id,
                BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
            ));

            state.results.write().await.insert(
                id,
                WorkorderResult {
                    workorder_id: id,
                    built_packages: Vec::new(),
                    failed_packages: failed_packages_for_atoms(&workorder.atoms, &reason),
                    binhost_uri: state.config.binhost_url.clone(),
                    fetched_distfiles: Default::default(),
                    parity_manifest: ParityManifest::default(),
                },
            );

            state
                .metrics
                .builds_total_duration_secs
                .fetch_add(build_start.elapsed().as_secs(), Ordering::Relaxed);
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            cleanup_workorder_runtime(state, &workorder, id, None).await;
            return Ok(());
        }
    }

    // Record image usage for idle timeout tracking.
    state
        .image_last_used
        .write()
        .await
        .insert(image_tag.clone(), Instant::now());

    // ── 2. Start worker container ───────────────────────────────────
    set_workorder_status(state, &tx, id, &trace_id, WorkorderStatus::Building).await;

    let container_name = format!("remerge-worker-{}", id.as_simple());
    let staged_runtime =
        match runtime::stage_workorder_runtime(&state.config.state_dir, &workorder).await {
            Ok(runtime) => runtime,
            Err(e) => {
                let reason = format!("Failed to stage worker runtime: {e:#}");
                set_workorder_status(
                    state,
                    &tx,
                    id,
                    &trace_id,
                    WorkorderStatus::Failed {
                        reason: reason.clone(),
                    },
                )
                .await;
                let _ = tx.send(build_progress(
                    id,
                    &trace_id,
                    BuildEvent::Finished {
                        built: Vec::new(),
                        failed: workorder.atoms.clone(),
                    },
                ));
                state.results.write().await.insert(
                    id,
                    WorkorderResult {
                        workorder_id: id,
                        built_packages: Vec::new(),
                        failed_packages: failed_packages_for_atoms(&workorder.atoms, &reason),
                        binhost_uri: state.config.binhost_url.clone(),
                        fetched_distfiles: Default::default(),
                        parity_manifest: ParityManifest::default(),
                    },
                );
                state
                    .metrics
                    .builds_total_duration_secs
                    .fetch_add(build_start.elapsed().as_secs(), Ordering::Relaxed);
                state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
                state
                    .metrics
                    .workorders_failed
                    .fetch_add(1, Ordering::Relaxed);
                cleanup_workorder_runtime(state, &workorder, id, None).await;
                return Ok(());
            }
        };
    state.metrics.record_snapshot_runtime_stage(
        staged_runtime.snapshot_references.total_blob_bytes
            + staged_runtime.snapshot_references.total_tree_bytes,
    );
    state
        .track_staged_workorder_references(id, staged_runtime.snapshot_references.clone())
        .await;
    info!(
        ?id,
        trace_id,
        runtime_dir = %staged_runtime.runtime_dir.display(),
        blob_count = staged_runtime.snapshot_references.blob_digests.len(),
        tree_count = staged_runtime.snapshot_references.tree_digests.len(),
        total_blob_bytes = staged_runtime.snapshot_references.total_blob_bytes,
        total_tree_bytes = staged_runtime.snapshot_references.total_tree_bytes,
        "Prepared staged runtime for worker startup"
    );

    let container_start = Instant::now();
    let container_result = state
        .docker
        .start_worker(
            &container_name,
            &image_tag,
            id,
            &staged_runtime.runtime_dir,
            workorder
                .trace_context
                .as_ref()
                .map(|ctx| ctx.traceparent.as_str()),
            &state.config,
        )
        .await;
    state
        .metrics
        .record_worker_container_start(container_start.elapsed().as_secs());

    let container_id = match container_result {
        Ok(id) => id,
        Err(e) => {
            let reason = format!("Failed to start worker: {e:#}");
            set_workorder_status(
                state,
                &tx,
                id,
                &trace_id,
                WorkorderStatus::Failed {
                    reason: reason.clone(),
                },
            )
            .await;
            let _ = tx.send(build_progress(
                id,
                &trace_id,
                BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
            ));

            state.results.write().await.insert(
                id,
                WorkorderResult {
                    workorder_id: id,
                    built_packages: Vec::new(),
                    failed_packages: failed_packages_for_atoms(&workorder.atoms, &reason),
                    binhost_uri: state.config.binhost_url.clone(),
                    fetched_distfiles: Default::default(),
                    parity_manifest: ParityManifest::default(),
                },
            );

            state
                .metrics
                .builds_total_duration_secs
                .fetch_add(build_start.elapsed().as_secs(), Ordering::Relaxed);
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            cleanup_workorder_runtime(state, &workorder, id, None).await;
            return Ok(());
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

        // ── Streaming REMERGE_EVENT: filter ─────────────────────────────
        //
        // PTY bytes are forwarded to WS clients immediately (no line-level
        // buffering) so that `\r`-terminated progress updates (e.g. download
        // progress bars) remain low-latency.  `REMERGE_EVENT:` control lines
        // are detected at line-start and consumed without forwarding.
        //
        // State machine:
        //   at_line_start – the next incoming byte begins a new logical line
        //   prefix_buf    – leading bytes of the current line during the
        //                   prefix-check phase (≤ REMERGE_PREFIX.len() bytes)
        //   skip_line     – consuming the tail of a confirmed REMERGE_EVENT:
        //                   line (do not forward these bytes)
        //   event_line_buf – full bytes of the current control line, built
        //                   while skip_line is true; parsed on '\n'
        //   pattern_buf   – secondary per-line buffer for emerge-output regex
        //                   matching; kept in sync with each logical line but
        //                   never gates the PTY relay
        const REMERGE_PREFIX: &[u8] = b"REMERGE_EVENT:";
        let mut at_line_start = true;
        let mut prefix_buf: Vec<u8> = Vec::with_capacity(REMERGE_PREFIX.len() + 1);
        let mut skip_line = false;
        let mut event_line_buf: Vec<u8> = Vec::new();
        let mut pattern_buf: Vec<u8> = Vec::with_capacity(4096);

        while let Some(result) = output.next().await {
            match result {
                Ok(log_output) => {
                    let bytes = log_output.into_bytes();

                    // Optimisation: avoid allocating a Vec unless a
                    // REMERGE_EVENT: line actually needs to be stripped.
                    // In the common case (no control lines) the original
                    // Bytes is forwarded as a zero-copy slice.
                    //
                    // `forward_buf`: None  = no control lines yet; forward
                    //                       original at the end.
                    //                Some  = assembled output with each
                    //                       control line's bytes removed.
                    //
                    // `clean_start`: first byte in `bytes` not yet flushed
                    //   into forward_buf.  Only meaningful when Some.
                    //
                    // `prefix_chunk_start`: position in `bytes` where the
                    //   current line-prefix accumulation began.
                    //   0 for cross-chunk prefix continuations.
                    let mut forward_buf: Option<Vec<u8>> = None;
                    let mut clean_start: usize = 0;
                    let mut prefix_chunk_start: usize = 0;

                    // If we are mid-REMERGE_EVENT: from the previous chunk,
                    // pre-initialise forward_buf so the control-line bytes in
                    // this chunk are not forwarded to PTY clients.  Without
                    // this, the final forwarding path would emit the whole
                    // chunk because forward_buf is None at chunk start.
                    if skip_line {
                        forward_buf = Some(Vec::new());
                    }

                    for (bi, &b) in bytes.iter().enumerate() {
                        if skip_line {
                            // Consuming REMERGE_EVENT: tail — accumulate for
                            // parsing but do NOT forward to PTY clients.
                            event_line_buf.push(b);
                            if b == b'\n' {
                                // Full control line — parse and dispatch.
                                let raw = String::from_utf8_lossy(&event_line_buf);
                                let json_str =
                                    raw[REMERGE_PREFIX.len()..].trim_end_matches(['\r', '\n']);
                                if let Ok(event) = serde_json::from_str::<WorkerEvent>(json_str) {
                                    match event {
                                        WorkerEvent::Log { event: log_event } => {
                                            // Scope-filter: reject events from
                                            // other workorders so a misbehaving
                                            // container cannot pollute the ring
                                            // buffer of a concurrent build.
                                            if log_event.workorder_id == id {
                                                log_state.push_log_event(id, log_event).await;
                                            }
                                        }
                                        other => {
                                            let _ = event_tx.send(other).await;
                                        }
                                    }
                                }
                                event_line_buf.clear();
                                skip_line = false;
                                at_line_start = true;
                                // forward_buf is always Some here (set when
                                // skip_line was entered); advance past line.
                                clean_start = bi + 1;
                            } else if event_line_buf.len() > 256 * 1024 {
                                // Guard against runaway/unterminated control
                                // lines to prevent unbounded memory growth.
                                event_line_buf.clear();
                                skip_line = false;
                                at_line_start = true;
                                clean_start = bi + 1;
                            }
                            continue;
                        }

                        if at_line_start {
                            if prefix_buf.is_empty() {
                                // Fresh prefix — record where it started.
                                prefix_chunk_start = bi;
                            }
                            prefix_buf.push(b);
                            let n = prefix_buf.len();
                            // Bytes in prefix_buf from previous chunk(s).
                            let prev_in_pfx = n - (bi + 1 - prefix_chunk_start);

                            if b == b'\n' || b == b'\r' {
                                // Line ended (\n = newline, \r = carriage-return
                                // used by PTY progress bars like download meters)
                                // before we accumulated enough bytes to match the
                                // prefix — short/empty line.  Forward and run
                                // pattern matching.
                                if prev_in_pfx > 0 {
                                    // Prev-chunk bytes aren't in bytes[]; if we
                                    // need to forward this line, flush them in
                                    // chronological order first.
                                    let fw = forward_buf.get_or_insert_with(Vec::new);
                                    fw.extend_from_slice(
                                        &bytes[clean_start..prefix_chunk_start],
                                    );
                                    fw.extend_from_slice(&prefix_buf[..prev_in_pfx]);
                                    clean_start = prefix_chunk_start;
                                }
                                if let Some(ref mut fw) = forward_buf {
                                    fw.extend_from_slice(&bytes[clean_start..=bi]);
                                    clean_start = bi + 1;
                                }
                                if prev_in_pfx > 0 {
                                    pattern_buf.extend_from_slice(&prefix_buf[..prev_in_pfx]);
                                }
                                pattern_buf.extend_from_slice(&bytes[prefix_chunk_start..=bi]);
                                let line = String::from_utf8_lossy(&pattern_buf)
                                    .trim_end_matches(['\r', '\n'])
                                    .to_string();
                                pattern_buf.clear();
                                prefix_buf.clear();
                                // at_line_start stays true
                                match_emerge_line(
                                    &line,
                                    &re_emerging,
                                    &re_completed,
                                    &re_error,
                                    &re_missing_dep,
                                    &re_use_conflict,
                                    &re_fetch_fail,
                                    &mut current_package,
                                    &event_tx,
                                )
                                .await;
                            } else if REMERGE_PREFIX[..n] != prefix_buf[..] {
                                // Prefix mismatch — regular PTY line; switch
                                // to passthrough for the rest of the line.
                                if prev_in_pfx > 0 {
                                    // Prev-chunk bytes aren't in bytes[]; must
                                    // flush them explicitly.
                                    let fw = forward_buf.get_or_insert_with(Vec::new);
                                    fw.extend_from_slice(&bytes[clean_start..prefix_chunk_start]);
                                    fw.extend_from_slice(&prefix_buf[..prev_in_pfx]);
                                    clean_start = prefix_chunk_start;
                                    // bytes[prefix_chunk_start..] remain in
                                    // the clean region — no copy needed.
                                }
                                if prev_in_pfx > 0 {
                                    pattern_buf.extend_from_slice(&prefix_buf[..prev_in_pfx]);
                                }
                                pattern_buf.extend_from_slice(&bytes[prefix_chunk_start..=bi]);
                                prefix_buf.clear();
                                at_line_start = false;
                            } else if n == REMERGE_PREFIX.len() {
                                // Exact prefix match — REMERGE_EVENT: line.
                                // Flush clean bytes up to the control-line
                                // start; do NOT forward the prefix itself.
                                let fw = forward_buf.get_or_insert_with(Vec::new);
                                fw.extend_from_slice(&bytes[clean_start..prefix_chunk_start]);
                                // Prev-chunk + current-chunk prefix bytes are
                                // the control-line header — drop them.
                                event_line_buf.extend_from_slice(REMERGE_PREFIX);
                                pattern_buf.clear(); // control line not for emerge patterns
                                prefix_buf.clear();
                                skip_line = true;
                                at_line_start = false;
                                // clean_start is advanced when skip_line ends at '\n'
                            }
                            // else: still building prefix — keep accumulating
                        } else {
                            // Passthrough: byte contributes to emerge pattern
                            // tracking only.  PTY forwarding is handled by
                            // the clean region [clean_start..) in bytes[] —
                            // no per-byte copy needed.
                            pattern_buf.push(b);

                            if b == b'\n' || b == b'\r' {
                                // \r resets line-start so a REMERGE_EVENT:
                                // prefix right after a PTY carriage-return
                                // will be detected on the next iteration.
                                let line = String::from_utf8_lossy(&pattern_buf)
                                    .trim_end_matches(['\r', '\n'])
                                    .to_string();
                                pattern_buf.clear();
                                at_line_start = true;
                                if b == b'\n' {
                                    match_emerge_line(
                                        &line,
                                        &re_emerging,
                                        &re_completed,
                                        &re_error,
                                        &re_missing_dep,
                                        &re_use_conflict,
                                        &re_fetch_fail,
                                        &mut current_package,
                                        &event_tx,
                                    )
                                    .await;
                                }
                            }
                        }
                    }

                    // Determine how many bytes from the tail of this chunk
                    // are sitting in prefix_buf awaiting resolution in the
                    // next chunk.  Those must not be forwarded yet.
                    let prefix_tail = if at_line_start && !prefix_buf.is_empty() {
                        bytes.len() - prefix_chunk_start
                    } else {
                        0
                    };
                    let forward_end = bytes.len() - prefix_tail;

                    let forward_bytes = if let Some(mut fw) = forward_buf {
                        // Control lines were stripped — finalise output.
                        fw.extend_from_slice(&bytes[clean_start..forward_end]);
                        bytes::Bytes::from(fw)
                    } else {
                        // Common case: no control lines — zero-copy slice.
                        bytes.slice(..forward_end)
                    };
                    if !forward_bytes.is_empty() {
                        let _ = raw_tx.send(forward_bytes);
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
    let wait_outcome = if state.config.build_timeout_secs == 0 {
        match state.docker.wait_container(&container_id).await {
            Ok(exit_code) => {
                info!(?id, exit_code, "Worker container finished");
                Ok(exit_code)
            }
            Err(e) => {
                warn!(?id, "Failed to wait for worker container: {e:#}");
                Err(format!("Failed to wait for worker container: {e:#}"))
            }
        }
    } else {
        match tokio::time::timeout(
            Duration::from_secs(state.config.build_timeout_secs),
            state.docker.wait_container(&container_id),
        )
        .await
        {
            Ok(Ok(exit_code)) => {
                info!(?id, exit_code, "Worker container finished");
                Ok(exit_code)
            }
            Ok(Err(e)) => {
                warn!(?id, "Failed to wait for worker container: {e:#}");
                Err(format!("Failed to wait for worker container: {e:#}"))
            }
            Err(_) => {
                warn!(
                    ?id,
                    timeout_secs = state.config.build_timeout_secs,
                    "Worker build exceeded timeout"
                );
                let _ = state.docker.stop_container(&container_id).await;
                Err(format!(
                    "Build exceeded configured timeout of {} seconds",
                    state.config.build_timeout_secs
                ))
            }
        }
    };

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
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), log_handle).await;
    }

    // Close the raw PTY channel *before* sending Finished so the WS
    // handler transitions to text-only mode and is guaranteed to pick
    // up the Finished event on the progress channel.
    state.raw_output_txs.write().await.remove(&id);

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
                    trace_id: trace_id.clone(),
                    event: BuildEvent::PackageBuilt {
                        atom: atom.clone(),
                        duration_secs,
                    },
                    timestamp: Utc::now(),
                });
                state.metrics.record_package_build(&atom, duration_secs);
                built_atoms.push(atom);
            }
            WorkerEvent::PackageFailed { atom, reason } => {
                let _ = tx.send(build_progress(
                    id,
                    &trace_id,
                    BuildEvent::PackageFailed {
                        atom: atom.clone(),
                        reason: reason.clone(),
                    },
                ));
                failed_atoms.push(FailedPackage {
                    atom,
                    reason,
                    build_log: None,
                });
            }
            // Log events are forwarded in real time by the log reading task;
            // any that arrive late in the drain are silently ignored.
            WorkerEvent::Log { .. } => {}
        }
    }

    // ── 6. Scan binpkg directory for real results ───────────────────
    let build_duration = build_start.elapsed().as_secs();
    let parity_manifest = match runtime::ingest_final_state_parity(
        &state.config.state_dir,
        &staged_runtime.runtime_dir,
    )
    .await
    {
        Ok(manifest) => manifest,
        Err(error) => {
            let reason = format!("Failed to ingest final-state parity: {error:#}");
            set_workorder_status(
                state,
                &tx,
                id,
                &trace_id,
                WorkorderStatus::Failed {
                    reason: reason.clone(),
                },
            )
            .await;

            let result = WorkorderResult {
                workorder_id: id,
                built_packages: Vec::new(),
                failed_packages: failed_packages_for_atoms(&workorder.atoms, &reason),
                binhost_uri: state.config.binhost_url.clone(),
                fetched_distfiles: Default::default(),
                parity_manifest: ParityManifest::default(),
            };
            state.results.write().await.insert(id, result);
            let _ = tx.send(build_progress(
                id,
                &trace_id,
                BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
            ));
            state
                .metrics
                .builds_total_duration_secs
                .fetch_add(build_duration, Ordering::Relaxed);
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            cleanup_workorder_runtime(state, &workorder, id, Some(&container_id)).await;
            return Ok(());
        }
    };
    let fetched_distfiles = match runtime::ingest_fetched_distfiles(
        &state.config.state_dir,
        &staged_runtime.runtime_dir,
    )
    .await
    {
        Ok(manifest) => manifest,
        Err(error) => {
            let reason = format!("Failed to ingest fetched distfiles: {error:#}");
            set_workorder_status(
                state,
                &tx,
                id,
                &trace_id,
                WorkorderStatus::Failed {
                    reason: reason.clone(),
                },
            )
            .await;

            let result = WorkorderResult {
                workorder_id: id,
                built_packages: Vec::new(),
                failed_packages: failed_packages_for_atoms(&workorder.atoms, &reason),
                binhost_uri: state.config.binhost_url.clone(),
                fetched_distfiles: Default::default(),
                parity_manifest,
            };
            state.results.write().await.insert(id, result);
            let _ = tx.send(build_progress(
                id,
                &trace_id,
                BuildEvent::Finished {
                    built: Vec::new(),
                    failed: workorder.atoms.clone(),
                },
            ));
            state
                .metrics
                .builds_total_duration_secs
                .fetch_add(build_duration, Ordering::Relaxed);
            state.metrics.builds_active.fetch_sub(1, Ordering::Relaxed);
            state
                .metrics
                .workorders_failed
                .fetch_add(1, Ordering::Relaxed);
            cleanup_workorder_runtime(state, &workorder, id, Some(&container_id)).await;
            return Ok(());
        }
    };

    if matches!(wait_outcome, Ok(0)) {
        set_workorder_status(state, &tx, id, &trace_id, WorkorderStatus::Completed).await;

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
            fetched_distfiles,
            parity_manifest,
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

        let _ = tx.send(build_progress(
            id,
            &trace_id,
            BuildEvent::Finished {
                built: built_list,
                failed: failed_list,
            },
        ));

        state
            .metrics
            .workorders_completed
            .fetch_add(1, Ordering::Relaxed);
    } else {
        let reason = match wait_outcome {
            Ok(exit_code) => format!("Worker exited with code {exit_code}"),
            Err(reason) => reason,
        };
        set_workorder_status(
            state,
            &tx,
            id,
            &trace_id,
            WorkorderStatus::Failed {
                reason: reason.clone(),
            },
        )
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
                failed_packages_for_atoms(&workorder.atoms, &reason)
            } else {
                failed_atoms
            },
            binhost_uri: state.config.binhost_url.clone(),
            fetched_distfiles,
            parity_manifest,
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

        let _ = tx.send(build_progress(
            id,
            &trace_id,
            BuildEvent::Finished {
                built: built_list,
                failed: failed_list,
            },
        ));

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
    cleanup_workorder_runtime(state, &workorder, id, Some(&container_id)).await;

    Ok(())
}

/// Structured event emitted by the worker process.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerEvent {
    PackageBuilt {
        atom: String,
        duration_secs: u64,
    },
    PackageFailed {
        atom: String,
        reason: String,
    },
    /// Tracing log event forwarded from the worker's WsLogLayer.
    ///
    /// The worker serialises this as `{"type":"log", <LogEvent fields...>}`
    /// using `#[serde(tag = "type")]` on a tuple variant, so `LogEvent`
    /// fields are inlined at the top level — `#[serde(flatten)]` matches that.
    Log {
        #[serde(flatten)]
        event: remerge_types::api::LogEvent,
    },
}

// ─── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::WorkerEvent;
    use remerge_types::api::LogLevel;

    // ── WorkerEvent::Log serde regression ─────────────────────────────────

    /// Regression guard for the critical serde shape mismatch: the worker
    /// serialises `WorkerEventEnvelope::Log(LogEvent)` (tuple variant with
    /// `#[serde(tag="type")]`) as:
    ///
    ///   `{"type":"log","level":"info","target":"…","message":"…", …}`
    ///
    /// The server's `WorkerEvent::Log { event: LogEvent }` previously
    /// deserialised from:
    ///
    ///   `{"type":"log","event":{"level":"info",…}}`   ← WRONG (nested)
    ///
    /// With `#[serde(flatten)]` on `event` the server now accepts the flat
    /// layout the worker actually emits.  Without the fix, every log event
    /// from the worker was silently dropped.
    #[test]
    fn worker_event_log_deserializes_from_flat_wire_format() {
        let id = uuid::Uuid::parse_str("23137eac-2455-45cf-a09f-cbdbd3a01fcc").unwrap();
        let json = format!(
            r#"{{"type":"log","level":"info","target":"remerge_worker::builder","message":"starting","workorder_id":"{id}","timestamp":"2026-01-01T00:00:00Z"}}"#
        );

        let event: WorkerEvent =
            serde_json::from_str(&json).expect("flat log wire format must deserialise");

        match event {
            WorkerEvent::Log { event: log_event } => {
                assert_eq!(log_event.level, LogLevel::Info);
                assert_eq!(log_event.target, "remerge_worker::builder");
                assert_eq!(log_event.message, "starting");
                assert_eq!(log_event.workorder_id, id);
            }
            other => panic!("expected Log variant, got {other:?}"),
        }
    }

    /// The old (broken) wire format had the LogEvent nested under an "event"
    /// key.  Ensure that format is now rejected, confirming the fix is active.
    #[test]
    fn worker_event_log_rejects_nested_event_key_format() {
        let id = uuid::Uuid::parse_str("23137eac-2455-45cf-a09f-cbdbd3a01fcc").unwrap();
        let json = format!(
            r#"{{"type":"log","event":{{"level":"info","target":"t","message":"m","workorder_id":"{id}","timestamp":"2026-01-01T00:00:00Z"}}}}"#
        );

        // The nested format must not successfully produce a Log variant.
        let result = serde_json::from_str::<WorkerEvent>(&json);
        match result {
            Err(_) => {} // expected — unknown fields with strict deserialise, or missing top-level fields
            Ok(WorkerEvent::Log { event }) => {
                // If it round-tripped by chance, the message must be wrong since
                // the fields were under "event", not at the top level.
                assert_ne!(
                    event.message, "m",
                    "nested 'event' key format must not be silently accepted"
                );
            }
            Ok(_) => {} // PackageBuilt/Failed — also fine
        }
    }

    #[test]
    fn worker_event_package_built_deserializes() {
        let json = r#"{"type":"package_built","atom":"dev-libs/openssl-3.0","duration_secs":42}"#;
        let event: WorkerEvent = serde_json::from_str(json).expect("package_built must parse");
        match event {
            WorkerEvent::PackageBuilt {
                atom,
                duration_secs,
            } => {
                assert_eq!(atom, "dev-libs/openssl-3.0");
                assert_eq!(duration_secs, 42);
            }
            other => panic!("expected PackageBuilt, got {other:?}"),
        }
    }

    #[test]
    fn worker_event_package_failed_deserializes() {
        let json =
            r#"{"type":"package_failed","atom":"dev-libs/foo-1.0","reason":"emerge failed"}"#;
        let event: WorkerEvent = serde_json::from_str(json).expect("package_failed must parse");
        match event {
            WorkerEvent::PackageFailed { atom, reason } => {
                assert_eq!(atom, "dev-libs/foo-1.0");
                assert_eq!(reason, "emerge failed");
            }
            other => panic!("expected PackageFailed, got {other:?}"),
        }
    }
}
