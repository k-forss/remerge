//! Client identity and configuration tracking.
//!
//! Each CLI installation is assigned a persistent **client ID** (UUID).
//! The ID can be shared across machines: one machine acts as the **main**
//! client that pushes portage configuration, while **follower** clients
//! reuse the same worker/config but are not allowed to change it.
//!
//! The server keeps a registry of known clients and their last-seen portage
//! configuration.  When a workorder arrives from a known client whose
//! configuration has not changed, the existing worker container can be reused
//! without re-applying the full portage config from scratch.
//!
//! Only one active workorder per client ID is allowed at a time.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A persistent identifier for a CLI installation (or group of installations
/// sharing the same build environment).
///
/// Stored in `/etc/remerge.conf`.  Can be auto-generated or explicitly set
/// so that multiple machines share a single identity.
pub type ClientId = Uuid;

/// Role a client plays within a client-ID group.
///
/// When multiple machines share the same [`ClientId`], exactly one should be
/// the **main** client that is allowed to push portage configuration changes.
/// The others are **followers** that can request builds but must use the
/// configuration already registered by the main client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    /// Full access: may push portage configuration and submit workorders.
    #[default]
    Main,
    /// Read-only config: may submit workorders but must use the existing
    /// portage configuration registered by the main client.
    Follower,
}

impl fmt::Display for ClientRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Main => write!(f, "main"),
            Self::Follower => write!(f, "follower"),
        }
    }
}

impl std::str::FromStr for ClientRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "main" => Ok(Self::Main),
            "follower" => Ok(Self::Follower),
            other => Err(format!(
                "unknown role '{other}' (expected 'main' or 'follower')"
            )),
        }
    }
}

/// Snapshot of a client's portage configuration state.
///
/// The server stores this alongside the [`ClientId`] and compares it on
/// subsequent workorders.  If nothing changed, the worker container can skip
/// the config-apply step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientState {
    /// The client's persistent ID.
    pub client_id: ClientId,

    /// SHA-256 digest of the serialised [`PortageConfig`](crate::portage::PortageConfig).
    ///
    /// A cheap equality check: if the hash matches, the config hasn't changed.
    pub config_hash: String,

    /// SHA-256 of the [`SystemIdentity`](crate::portage::SystemIdentity).
    pub system_hash: String,

    /// When this state was last seen.
    pub last_seen: chrono::DateTime<chrono::Utc>,

    /// Whether a workorder for this client ID is currently in progress.
    pub active_workorder: Option<crate::workorder::WorkorderId>,
}

/// What changed between two config snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    /// `true` when the full portage config needs to be re-applied.
    pub portage_changed: bool,

    /// `true` when the system identity (CHOST, profile, GCC, …) changed,
    /// meaning a different worker image is needed.
    pub system_changed: bool,
}

impl ConfigDiff {
    /// Nothing changed — the existing worker can be reused as-is.
    pub fn is_empty(&self) -> bool {
        !self.portage_changed && !self.system_changed
    }
}
