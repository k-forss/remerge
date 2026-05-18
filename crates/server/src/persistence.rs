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
use remerge_types::workorder::{Workorder, WorkorderId, WorkorderResult, WorkorderStatus};

fn workorders_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("workorders.json")
}

/// Save workorders to disk.
pub async fn save_workorders(
    state_dir: &Path,
    workorders: &HashMap<WorkorderId, Workorder>,
) -> Result<()> {
    let path = workorders_path(state_dir);
    let json =
        serde_json::to_string_pretty(workorders).context("Failed to serialise workorders")?;
    tokio::fs::write(&path, json)
        .await
        .context("Failed to write workorders.json")?;
    Ok(())
}

/// Load workorders from disk.
///
/// In-flight workorders are reset to `Pending` after a server restart because
/// worker containers are disposable and progress channels do not survive.
pub async fn load_workorders(state_dir: &Path) -> Result<HashMap<WorkorderId, Workorder>> {
    let path = workorders_path(state_dir);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let json = tokio::fs::read_to_string(&path)
        .await
        .context("Failed to read workorders.json")?;
    let mut workorders: HashMap<WorkorderId, Workorder> =
        serde_json::from_str(&json).context("Failed to parse workorders.json")?;

    for workorder in workorders.values_mut() {
        if matches!(
            workorder.status,
            WorkorderStatus::Provisioning | WorkorderStatus::Building
        ) {
            warn!(
                workorder_id = %workorder.id,
                "Resetting persisted in-flight workorder to pending after restart"
            );
            workorder.status = WorkorderStatus::Pending;
            workorder.updated_at = chrono::Utc::now();
        }
    }

    info!(count = workorders.len(), "Loaded persisted workorders");
    Ok(workorders)
}

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
            let results = {
                let results = state.results.read().await;
                results.clone()
            };
            if let Err(e) = save_results(state_dir, &results).await {
                warn!("Failed to persist results: {e:#}");
            }
        }

        // Save workorders.
        {
            let workorders = {
                let workorders = state.workorders.read().await;
                workorders.clone()
            };
            if let Err(e) = save_workorders(state_dir, &workorders).await {
                warn!("Failed to persist workorders: {e:#}");
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use remerge_types::client::ClientRole;
    use remerge_types::portage::{MakeConf, PortageConfig, SystemIdentity};
    use remerge_types::workorder::{Workorder, WorkorderStatus};

    use super::{load_workorders, save_workorders};

    fn test_workorder(status: WorkorderStatus) -> Workorder {
        Workorder {
            id: uuid::Uuid::new_v4(),
            client_id: uuid::Uuid::new_v4(),
            role: ClientRole::Main,
            atoms: vec!["app-misc/hello".to_string()],
            emerge_args: vec!["app-misc/hello".to_string()],
            portage_config: PortageConfig {
                make_conf: MakeConf::default(),
                package_use: Vec::new(),
                package_accept_keywords: Vec::new(),
                package_license: Vec::new(),
                package_mask: Vec::new(),
                package_unmask: Vec::new(),
                package_env: Vec::new(),
                env_files: BTreeMap::new(),
                repos_conf: BTreeMap::new(),
                snapshot_manifest: Default::default(),
                repo_snapshots: BTreeMap::new(),
                repo_snapshot_refs: BTreeMap::new(),
                repo_snapshot_trees: BTreeMap::new(),
                patches: BTreeMap::new(),
                profile_overlay: BTreeMap::new(),
                distfile_snapshots: BTreeMap::new(),
                distfile_snapshot_refs: BTreeMap::new(),
                profile: "default/linux/amd64/23.0".to_string(),
                world: Vec::new(),
            },
            system_id: SystemIdentity {
                arch: "amd64".to_string(),
                chost: "x86_64-pc-linux-gnu".to_string(),
                gcc_version: "14.2.1".to_string(),
                libc_version: "2.40".to_string(),
                kernel_version: "6.12.0".to_string(),
                python_targets: vec!["python3_12".to_string()],
                profile: "default/linux/amd64/23.0".to_string(),
            },
            trace_context: None,
            status,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn load_workorders_resets_inflight_statuses_to_pending() {
        let state_dir = tempfile::TempDir::new().expect("temp state dir");
        let building = test_workorder(WorkorderStatus::Building);
        let completed = test_workorder(WorkorderStatus::Completed);
        let workorders = std::collections::HashMap::from([
            (building.id, building.clone()),
            (completed.id, completed.clone()),
        ]);

        save_workorders(state_dir.path(), &workorders)
            .await
            .expect("save workorders");
        let loaded = load_workorders(state_dir.path())
            .await
            .expect("load workorders");

        assert!(matches!(
            loaded[&building.id].status,
            WorkorderStatus::Pending
        ));
        assert!(matches!(
            loaded[&completed.id].status,
            WorkorderStatus::Completed
        ));
    }

    #[tokio::test]
    async fn save_and_load_workorders_roundtrip_refs_only_payloads() {
        let state_dir = tempfile::TempDir::new().expect("temp state dir");
        let mut workorder = test_workorder(WorkorderStatus::Pending);
        workorder.portage_config.repo_snapshot_refs.insert(
            "local-overlay".to_string(),
            BTreeMap::from([("dev-libs/demo/demo-1.0.ebuild".to_string(), "ab".repeat(32))]),
        );
        workorder
            .portage_config
            .repo_snapshot_trees
            .insert("local-overlay".to_string(), "cd".repeat(32));
        workorder
            .portage_config
            .distfile_snapshot_refs
            .insert("demo-1.0.tar.xz".to_string(), "ef".repeat(32));
        let workorders = std::collections::HashMap::from([(workorder.id, workorder.clone())]);

        save_workorders(state_dir.path(), &workorders)
            .await
            .expect("save workorders");
        let loaded = load_workorders(state_dir.path())
            .await
            .expect("load workorders");

        assert_eq!(
            loaded[&workorder.id].portage_config.repo_snapshot_refs,
            workorder.portage_config.repo_snapshot_refs
        );
        assert_eq!(
            loaded[&workorder.id].portage_config.repo_snapshot_trees,
            workorder.portage_config.repo_snapshot_trees
        );
        assert_eq!(
            loaded[&workorder.id].portage_config.distfile_snapshot_refs,
            workorder.portage_config.distfile_snapshot_refs
        );
        assert!(
            loaded[&workorder.id]
                .portage_config
                .repo_snapshots
                .is_empty()
        );
        assert!(
            loaded[&workorder.id]
                .portage_config
                .distfile_snapshots
                .is_empty()
        );
    }
}
