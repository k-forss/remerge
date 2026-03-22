//! Client registry — tracks known clients and their configuration state.
//!
//! When a workorder arrives, the server computes a hash of the client's
//! portage configuration and system identity.  If a matching [`ClientState`]
//! already exists, the server can tell the worker to skip (or partially
//! skip) the config-apply step.
//!
//! ## Client-ID sharing
//!
//! Multiple machines may share the same [`ClientId`].  One of them is the
//! **main** client that is allowed to push portage configuration; the others
//! are **followers** that reuse the configuration already registered by the
//! main client.
//!
//! Only one active workorder per [`ClientId`] is allowed at a time.

use std::collections::HashMap;

use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::info;

use remerge_types::{
    client::{ClientId, ClientRole, ClientState, ConfigDiff},
    portage::{PortageConfig, SystemIdentity},
    workorder::WorkorderId,
};

/// In-memory registry of known clients.
///
/// In a future iteration this could be backed by a file or database so state
/// survives server restarts.
pub struct ClientRegistry {
    clients: RwLock<HashMap<ClientId, ClientState>>,
}

/// Error returned when a workorder cannot be accepted.
#[derive(Debug, Clone)]
pub enum RegistryError {
    /// A workorder for this client ID is already in progress.
    ActiveWorkorder(WorkorderId),
    /// A follower tried to push config but no main client has registered yet.
    NoMainClient,
    /// A follower's config differs from the main client's stored config
    /// (followers cannot change config).
    ConfigMismatch,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActiveWorkorder(id) => {
                write!(f, "workorder {id} is already active for this client ID")
            }
            Self::NoMainClient => {
                write!(f, "no main client has registered for this client ID yet")
            }
            Self::ConfigMismatch => {
                write!(
                    f,
                    "follower config differs from main client — followers cannot change config"
                )
            }
        }
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientRegistry {
    /// Create an empty registry (used when no persisted state exists).
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry from persisted state.
    pub fn from_persisted(clients: HashMap<ClientId, ClientState>) -> Self {
        Self {
            clients: RwLock::new(clients),
        }
    }

    /// Register or update a client and return what changed.
    ///
    /// - If the client is new, returns a diff with everything marked changed.
    /// - If the client is known, compares hashes and returns what differs.
    /// - Returns `Err` if there is already an active workorder for this client
    ///   ID, or if a follower tries to push new config.
    pub async fn update(
        &self,
        client_id: ClientId,
        role: ClientRole,
        portage_config: &PortageConfig,
        system_id: &SystemIdentity,
    ) -> Result<ConfigDiff, RegistryError> {
        let config_hash = hash_json(portage_config);
        let system_hash = hash_json(system_id);

        let mut clients = self.clients.write().await;

        // ── Check for active workorder ──────────────────────────────
        if let Some(existing) = clients.get(&client_id)
            && let Some(active_id) = existing.active_workorder
        {
            return Err(RegistryError::ActiveWorkorder(active_id));
        }

        let diff = if let Some(existing) = clients.get(&client_id) {
            let diff = ConfigDiff {
                portage_changed: existing.config_hash != config_hash,
                system_changed: existing.system_hash != system_hash,
            };

            // ── Follower enforcement ────────────────────────────────
            if role == ClientRole::Follower && (diff.portage_changed || diff.system_changed) {
                return Err(RegistryError::ConfigMismatch);
            }

            diff
        } else {
            // New client.
            if role == ClientRole::Follower {
                // A follower can only register if a main client already set
                // up the config.  Since we track per client_id and this is
                // the first time we see this id, there's nothing to follow.
                return Err(RegistryError::NoMainClient);
            }
            ConfigDiff {
                portage_changed: true,
                system_changed: true,
            }
        };

        if diff.portage_changed || diff.system_changed {
            info!(
                %client_id,
                %role,
                portage_changed = diff.portage_changed,
                system_changed = diff.system_changed,
                "Client configuration changed — worker will re-apply"
            );
        } else {
            info!(%client_id, %role, "Client configuration unchanged — reusing worker state");
        }

        // Update the stored state (followers don't change hashes).
        if role == ClientRole::Main {
            clients.insert(
                client_id,
                ClientState {
                    client_id,
                    config_hash,
                    system_hash,
                    last_seen: Utc::now(),
                    active_workorder: None,
                },
            );
        } else {
            // Just touch `last_seen` for followers.
            if let Some(existing) = clients.get_mut(&client_id) {
                existing.last_seen = Utc::now();
            }
        }

        Ok(diff)
    }

    /// Mark a workorder as active for a client ID.
    pub async fn set_active_workorder(&self, client_id: &ClientId, workorder_id: WorkorderId) {
        let mut clients = self.clients.write().await;
        if let Some(state) = clients.get_mut(client_id) {
            state.active_workorder = Some(workorder_id);
        }
    }

    /// Clear the active workorder for a client ID (on completion or cancellation).
    pub async fn clear_active_workorder(&self, client_id: &ClientId) {
        let mut clients = self.clients.write().await;
        if let Some(state) = clients.get_mut(client_id) {
            state.active_workorder = None;
        }
    }

    /// Look up the last known state for a client.
    pub async fn get(&self, client_id: &ClientId) -> Option<ClientState> {
        self.clients.read().await.get(client_id).cloned()
    }

    /// List all known clients.
    pub async fn list_all(&self) -> Vec<ClientState> {
        self.clients.read().await.values().cloned().collect()
    }

    /// Get a snapshot of all clients for persistence.
    pub async fn snapshot(&self) -> HashMap<ClientId, ClientState> {
        self.clients.read().await.clone()
    }
}

/// SHA-256 hash of a serde-serialisable value.
fn hash_json<T: serde::Serialize>(value: &T) -> String {
    let json = match serde_json::to_string(value) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("Failed to serialise value for hashing: {e}");
            // Fall back to empty string — callers will see a config-changed diff.
            String::new()
        }
    };
    let digest = Sha256::digest(json.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use remerge_types::portage::MakeConf;
    use uuid::Uuid;

    fn dummy_config() -> PortageConfig {
        PortageConfig {
            make_conf: MakeConf::default(),
            package_use: Vec::new(),
            package_accept_keywords: Vec::new(),
            package_license: Vec::new(),
            package_mask: Vec::new(),
            package_unmask: Vec::new(),
            package_env: Vec::new(),
            env_files: BTreeMap::new(),
            repos_conf: BTreeMap::new(),
            patches: BTreeMap::new(),
            profile_overlay: BTreeMap::new(),
            profile: "default/linux/amd64/23.0".into(),
            world: Vec::new(),
        }
    }

    fn dummy_system() -> SystemIdentity {
        SystemIdentity {
            arch: "amd64".into(),
            chost: "x86_64-pc-linux-gnu".into(),
            gcc_version: "13.2.0".into(),
            libc_version: "2.38".into(),
            kernel_version: "6.6.0".into(),
            python_targets: vec!["python3_12".into()],
            profile: "default/linux/amd64/23.0".into(),
        }
    }

    #[tokio::test]
    async fn new_client_returns_full_diff() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let diff = registry
            .update(id, ClientRole::Main, &dummy_config(), &dummy_system())
            .await
            .unwrap();
        assert!(diff.portage_changed);
        assert!(diff.system_changed);
    }

    #[tokio::test]
    async fn same_config_returns_empty_diff() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let config = dummy_config();
        let system = dummy_system();

        let _ = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();

        let diff = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();
        assert!(diff.is_empty());
    }

    #[tokio::test]
    async fn changed_use_flags_detected() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let mut config = dummy_config();
        let system = dummy_system();

        let _ = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();

        config.make_conf.use_flags.push("wayland".into());
        let diff = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();
        assert!(diff.portage_changed);
        assert!(!diff.system_changed);
    }

    #[tokio::test]
    async fn active_workorder_prevents_new_submission() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let config = dummy_config();
        let system = dummy_system();

        let _ = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();

        let wo_id = Uuid::new_v4();
        registry.set_active_workorder(&id, wo_id).await;

        let result = registry
            .update(id, ClientRole::Main, &config, &system)
            .await;
        assert!(matches!(result, Err(RegistryError::ActiveWorkorder(_))));

        // After clearing, it works again.
        registry.clear_active_workorder(&id).await;
        let diff = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();
        assert!(diff.is_empty());
    }

    #[tokio::test]
    async fn follower_reuses_main_config() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let config = dummy_config();
        let system = dummy_system();

        // Main client registers.
        let _ = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();

        // Follower with same config succeeds.
        let diff = registry
            .update(id, ClientRole::Follower, &config, &system)
            .await
            .unwrap();
        assert!(diff.is_empty());
    }

    #[tokio::test]
    async fn follower_cannot_change_config() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let config = dummy_config();
        let system = dummy_system();

        let _ = registry
            .update(id, ClientRole::Main, &config, &system)
            .await
            .unwrap();

        let mut different_config = config;
        different_config.make_conf.use_flags.push("systemd".into());
        let result = registry
            .update(id, ClientRole::Follower, &different_config, &system)
            .await;
        assert!(matches!(result, Err(RegistryError::ConfigMismatch)));
    }

    #[tokio::test]
    async fn follower_without_main_rejected() {
        let registry = ClientRegistry::new();
        let id = Uuid::new_v4();
        let config = dummy_config();
        let system = dummy_system();

        let result = registry
            .update(id, ClientRole::Follower, &config, &system)
            .await;
        assert!(matches!(result, Err(RegistryError::NoMainClient)));
    }
}
