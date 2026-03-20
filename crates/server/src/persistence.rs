//! State persistence — saves and loads server state to/from disk.
//!
//! State is persisted as JSON files in the configured `state_dir`:
//! - `results.json` — completed build results.
//! - `clients.json` — client registry.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{info, warn};

use remerge_types::client::ClientState;
use remerge_types::workorder::{WorkorderId, WorkorderResult};

/// Save workorder results to disk.
pub async fn save_results(
    state_dir: &Path,
    results: &HashMap<WorkorderId, WorkorderResult>,
) -> Result<()> {
    let path = state_dir.join("results.json");
    let json = serde_json::to_string_pretty(results).context("Failed to serialise results")?;
    tokio::fs::write(&path, json)
        .await
        .context("Failed to write results.json")?;
    Ok(())
}

/// Load workorder results from disk.
pub async fn load_results(state_dir: &Path) -> Result<HashMap<WorkorderId, WorkorderResult>> {
    let path = state_dir.join("results.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let json = tokio::fs::read_to_string(&path)
        .await
        .context("Failed to read results.json")?;
    let results: HashMap<WorkorderId, WorkorderResult> =
        serde_json::from_str(&json).context("Failed to parse results.json")?;
    info!(count = results.len(), "Loaded persisted results");
    Ok(results)
}

/// Save client registry to disk.
pub async fn save_clients(
    state_dir: &Path,
    clients: &HashMap<remerge_types::client::ClientId, ClientState>,
) -> Result<()> {
    let path = state_dir.join("clients.json");
    let json = serde_json::to_string_pretty(clients).context("Failed to serialise clients")?;
    tokio::fs::write(&path, json)
        .await
        .context("Failed to write clients.json")?;
    Ok(())
}

/// Load client registry from disk.
///
/// Any stale `active_workorder` references are cleared since the containers
/// are gone after a server restart.
pub async fn load_clients(
    state_dir: &Path,
) -> Result<HashMap<remerge_types::client::ClientId, ClientState>> {
    let path = state_dir.join("clients.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let json = tokio::fs::read_to_string(&path)
        .await
        .context("Failed to read clients.json")?;
    let mut clients: HashMap<remerge_types::client::ClientId, ClientState> =
        serde_json::from_str(&json).context("Failed to parse clients.json")?;

    // Clear stale active workorders — the containers are gone after restart.
    for state in clients.values_mut() {
        if state.active_workorder.is_some() {
            warn!(
                client_id = %state.client_id,
                workorder_id = ?state.active_workorder,
                "Clearing stale active workorder from persisted state"
            );
            state.active_workorder = None;
        }
    }

    info!(count = clients.len(), "Loaded persisted client registry");
    Ok(clients)
}

/// Periodic state saver — runs every `interval` and persists state to disk.
pub async fn run_periodic_save(
    state: std::sync::Arc<crate::state::AppState>,
    interval: std::time::Duration,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tick.tick().await;

        let state_dir = &state.config.state_dir;

        // Save results.
        {
            let results = state.results.read().await;
            if let Err(e) = save_results(state_dir, &results).await {
                warn!("Failed to persist results: {e:#}");
            }
        }

        // Save client registry.
        {
            let clients = state.clients.snapshot().await;
            if let Err(e) = save_clients(state_dir, &clients).await {
                warn!("Failed to persist clients: {e:#}");
            }
        }
    }
}
