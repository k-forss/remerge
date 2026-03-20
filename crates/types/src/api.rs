//! HTTP API request/response types shared between CLI (client) and server.

use serde::{Deserialize, Serialize};

use crate::auth::AuthMode;
use crate::client::{ClientId, ClientRole};
use crate::portage::{PortageConfig, SystemIdentity};
use crate::workorder::{WorkorderId, WorkorderResult, WorkorderStatus};

// ─── Submit workorder ───────────────────────────────────────────────

/// POST `/api/v1/workorders`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitWorkorderRequest {
    /// Persistent client identifier.
    pub client_id: ClientId,
    /// Role of this client (`main` or `follower`).
    #[serde(default)]
    pub role: ClientRole,
    /// Packages to build (portage atoms).
    pub atoms: Vec<String>,
    /// The raw emerge arguments.
    pub emerge_args: Vec<String>,
    /// Portage configuration snapshot.
    ///
    /// **Followers** must still send their local config for reference, but
    /// the server ignores it in favour of the main client's stored config.
    pub portage_config: PortageConfig,
    /// System identity for worker selection.
    pub system_id: SystemIdentity,
}

/// Response to a workorder submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitWorkorderResponse {
    /// Assigned workorder ID.
    pub workorder_id: WorkorderId,
    /// WebSocket URL to stream build progress.
    pub progress_ws_url: String,
}

// ─── Query workorder ────────────────────────────────────────────────

/// GET `/api/v1/workorders/{id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkorderStatusResponse {
    pub workorder_id: WorkorderId,
    pub status: WorkorderStatus,
    pub result: Option<WorkorderResult>,
}

// ─── List workorders ────────────────────────────────────────────────

/// GET `/api/v1/workorders`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListWorkordersResponse {
    pub workorders: Vec<WorkorderSummary>,
}

/// Short summary for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkorderSummary {
    pub id: WorkorderId,
    pub atoms: Vec<String>,
    pub status: WorkorderStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── Cancel workorder ───────────────────────────────────────────────

/// DELETE `/api/v1/workorders/{id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelWorkorderResponse {
    pub workorder_id: WorkorderId,
    pub cancelled: bool,
}

// ─── Health ─────────────────────────────────────────────────────────

/// GET `/api/v1/health`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
}

// ─── Client admin ───────────────────────────────────────────────────

/// GET `/api/v1/clients`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientListResponse {
    pub clients: Vec<ClientSummary>,
}

/// Summary of a registered client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSummary {
    pub client_id: ClientId,
    pub config_hash: String,
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub active_workorder: Option<WorkorderId>,
}

/// GET `/api/v1/clients/{id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientDetailResponse {
    pub client_id: ClientId,
    pub config_hash: String,
    pub system_hash: String,
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub active_workorder: Option<WorkorderId>,
}

// ─── Server info ────────────────────────────────────────────────────

/// GET `/api/v1/info`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfoResponse {
    pub version: String,
    pub binhost_base_url: String,
    pub active_workers: usize,
    pub queued_workorders: usize,
    /// Authentication mode the server is operating in.
    #[serde(default)]
    pub auth_mode: AuthMode,
    /// Whether the server signs binary packages with GPG.
    #[serde(default)]
    pub binpkg_signing: bool,
}
