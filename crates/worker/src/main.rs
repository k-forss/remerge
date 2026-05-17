use remerge_worker::{builder, crossdev, portage_setup};

use anyhow::{Context, Result};
use tracing::{Instrument, info};

use remerge_types::workorder::Workorder;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = remerge_observability::init_tracing("remerge-worker", false)?;

    info!("remerge-worker starting");

    // Read the workorder from the REMERGE_WORKORDER environment variable.
    let workorder_json = std::env::var("REMERGE_WORKORDER")
        .context("REMERGE_WORKORDER environment variable not set")?;

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
