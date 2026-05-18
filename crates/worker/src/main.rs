use remerge_worker::{builder, crossdev, parity, portage_setup};

use anyhow::{Context, Result};
use tracing::{Instrument, info};

use remerge_types::workorder::Workorder;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = remerge_observability::init_tracing("remerge-worker", false)?;

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
        let (emerge_cmd, is_cross) = crossdev::emerge_command(&worker_chost, target_chost);

        if is_cross {
            info!(
                target = %target_chost,
                worker = %worker_chost,
                "Cross-compilation required — setting up crossdev"
            );
            crossdev::setup_crossdev(target_chost).await?;
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
