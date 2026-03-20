mod builder;
mod crossdev;
mod portage_setup;

use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;

use remerge_types::workorder::Workorder;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    info!("remerge-worker starting");

    // Read the workorder from the REMERGE_WORKORDER environment variable.
    let workorder_json = std::env::var("REMERGE_WORKORDER")
        .context("REMERGE_WORKORDER environment variable not set")?;

    let workorder: Workorder =
        serde_json::from_str(&workorder_json).context("Failed to parse workorder JSON")?;

    info!(
        id = %workorder.id,
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
    let result = builder::build_packages(&workorder, &emerge_cmd).await;

    match &result {
        Ok(()) => info!("Build completed successfully"),
        Err(e) => {
            tracing::error!("Build failed: {e:#}");
            std::process::exit(1);
        }
    }

    result
}
