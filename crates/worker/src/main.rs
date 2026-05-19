use remerge_worker::{builder, crossdev, parity, portage_setup};

use anyhow::{Context, Result};
use tracing::{Instrument, info};

use remerge_types::workorder::Workorder;

/// Envelope used to emit log events back to the server via the REMERGE_EVENT
/// stdout protocol.  The `#[serde(tag = "type")]` discriminant lets the server
/// distinguish log events from other worker events using the same prefix.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerEventEnvelope {
    Log(remerge_types::api::LogEvent),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Build an optional WsLogLayer if the server provided a workorder ID.
    // The channel is drained by a background thread that writes
    // `REMERGE_EVENT:<json>` to stdout so the server can relay log events
    // over the WebSocket progress stream.
    let ws_log = std::env::var("REMERGE_WORKORDER_ID")
        .ok()
        .and_then(|s| s.parse::<remerge_types::workorder::WorkorderId>().ok())
        .map(|workorder_id| {
            let max_level = std::env::var("REMERGE_WORKER_LOG_LEVEL")
                .ok()
                .and_then(|s| s.parse::<remerge_types::api::LogLevel>().ok())
                .unwrap_or(remerge_types::api::LogLevel::Info);

            let (tx, rx) = std::sync::mpsc::sync_channel::<remerge_types::api::LogEvent>(512);

            std::thread::spawn(move || {
                while let Ok(event) = rx.recv() {
                    if let Ok(json) = serde_json::to_string(&WorkerEventEnvelope::Log(event)) {
                        // Emit the control message as a single line so we do
                        // not inject an extra blank line into the PTY stream
                        // for every forwarded event.
                        println!("REMERGE_EVENT:{json}");
                    }
                }
            });

            remerge_observability::ws_log::WsLogLayer::new(tx, workorder_id, max_level)
        });

    let _telemetry =
        remerge_observability::init_tracing_with_ws_log("remerge-worker", false, ws_log)?;

    info!("remerge-worker starting");

    // Read the staged workorder JSON path from the environment.
    let workorder_path = std::env::var("REMERGE_WORKORDER_PATH")
        .context("REMERGE_WORKORDER_PATH environment variable not set")?;
    let workorder_json = tokio::fs::read_to_string(&workorder_path)
        .await
        .with_context(|| format!("Failed to read staged workorder JSON from {workorder_path}"))?;

    let workorder: Workorder =
        serde_json::from_str(&workorder_json).context("Failed to parse workorder JSON")?;

    let traceparent = std::env::var("REMERGE_TRACEPARENT")
        .ok()
        .and_then(|value| remerge_observability::parse_trace_context(&value));

    let workorder_span = tracing::info_span!(
        "worker_workorder",
        workorder_id = %workorder.id,
        trace_id = workorder.trace_context.as_ref().map(|ctx| ctx.trace_id.as_str()).unwrap_or("unknown")
    );
    remerge_observability::set_span_parent(
        &workorder_span,
        traceparent.as_ref().or(workorder.trace_context.as_ref()),
    );

    async move {
        info!(
            id = %workorder.id,
            trace_id = workorder.trace_context.as_ref().map(|ctx| ctx.trace_id.as_str()).unwrap_or("unknown"),
            atoms = ?workorder.atoms,
            chost = %workorder.portage_config.make_conf.chost,
            "Received workorder"
        );

        // Detect the worker's own CHOST (the build machine).
        let worker_chost = crossdev::detect_worker_chost().await?;
        info!(worker_chost = %worker_chost, "Worker CHOST detected");

        // Determine whether we need crossdev.
        let target_chost = &workorder.portage_config.make_conf.chost;
        let verbose = workorder
            .emerge_args
            .iter()
            .any(|a| a == "--verbose" || a == "-v");
        let (emerge_cmd, is_cross) = crossdev::emerge_command(&worker_chost, target_chost);

        if is_cross {
            info!(
                target = %target_chost,
                worker = %worker_chost,
                "Cross-compilation required — setting up crossdev"
            );
            crossdev::setup_crossdev(target_chost, verbose).await?;
        }

        // Read optional signing configuration injected by the server.
        let gpg_key = std::env::var("REMERGE_GPG_KEY").ok();
        let gpg_home = std::env::var("REMERGE_GPG_HOME").ok();

        if gpg_key.is_some() {
            info!("Binary package GPG signing enabled");
        }

        // 1. Apply portage configuration from the workorder.
        portage_setup::apply_config(
            &workorder.portage_config,
            &worker_chost,
            gpg_key.as_deref(),
            gpg_home.as_deref(),
        )
        .await
        .context("Failed to apply portage configuration")?;

        // 2. Build the packages.
        match builder::build_packages(&workorder, &emerge_cmd).await {
            Ok(()) => {
                if let Ok(parity_output_dir) = std::env::var("REMERGE_PARITY_OUTPUT_DIR") {
                    let parity_output_dir = std::path::Path::new(&parity_output_dir);
                    parity::capture_final_state_parity(parity_output_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to capture final-state parity into {}",
                                parity_output_dir.display()
                            )
                        })?;
                    parity::capture_fetched_distfiles(parity_output_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to capture fetched distfiles into {}",
                                parity_output_dir.display()
                            )
                        })?;
                }
                info!("Build completed successfully");
                Ok(())
            }
            Err(error) => {
                tracing::error!("Build failed: {error:#}");
                Err(error)
            }
        }
    }
    .instrument(workorder_span)
    .await
}
