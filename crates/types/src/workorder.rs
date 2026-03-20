//! Workorder types.
//!
//! A [`Workorder`] represents a request from a client to build one or more
//! packages with a specific portage configuration.  The server queues the
//! workorder, matches or creates a worker container, and reports progress back
//! to the originating CLI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::client::{ClientId, ClientRole};
use crate::portage::{PortageConfig, SystemIdentity};

/// Unique identifier for a workorder.
pub type WorkorderId = Uuid;

/// A build request submitted by the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workorder {
    /// Server-assigned unique ID.
    pub id: WorkorderId,

    /// Persistent client identifier — ties this workorder to a known client
    /// so the server can track configuration changes and reuse workers.
    pub client_id: ClientId,

    /// Role of the submitting client (`main` or `follower`).
    pub role: ClientRole,

    /// Packages to build, as portage atoms (e.g. `dev-libs/openssl`).
    pub atoms: Vec<String>,

    /// The raw emerge arguments the user passed (for reference / replay).
    pub emerge_args: Vec<String>,

    /// Full portage configuration snapshot from the requesting host.
    pub portage_config: PortageConfig,

    /// System identity of the requesting host (determines worker image).
    pub system_id: SystemIdentity,

    /// Current status.
    pub status: WorkorderStatus,

    /// When the workorder was created.
    pub created_at: DateTime<Utc>,

    /// When the workorder was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Status progression of a workorder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkorderStatus {
    /// Queued, waiting for a worker.
    Pending,
    /// A worker container is being created / started.
    Provisioning,
    /// Building packages.
    Building,
    /// Build succeeded — binary packages are available.
    Completed,
    /// Build failed.
    Failed { reason: String },
    /// Cancelled by user.
    Cancelled,
}

/// Progress event streamed back to the CLI during a build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProgress {
    /// Which workorder this belongs to.
    pub workorder_id: WorkorderId,
    /// The event payload.
    pub event: BuildEvent,
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Individual build events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum BuildEvent {
    /// Status transition.
    StatusChanged {
        from: WorkorderStatus,
        to: WorkorderStatus,
    },
    /// A log line from the build output.
    Log { line: String },
    /// A package finished building.
    PackageBuilt {
        atom: String,
        /// Time in seconds.
        duration_secs: u64,
    },
    /// A package failed to build.
    PackageFailed { atom: String, reason: String },
    /// Overall build completed.
    Finished {
        /// Atoms that were successfully built.
        built: Vec<String>,
        /// Atoms that failed.
        failed: Vec<String>,
    },
}

/// Result summary returned when a workorder completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkorderResult {
    pub workorder_id: WorkorderId,
    /// Packages that were successfully built as binpkgs.
    pub built_packages: Vec<BuiltPackage>,
    /// Packages that failed to build.
    pub failed_packages: Vec<FailedPackage>,
    /// URI of the binhost directory for this build.
    pub binhost_uri: String,
}

/// A successfully built binary package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltPackage {
    /// Full atom, e.g. `dev-libs/openssl-3.1.4`.
    pub atom: String,
    /// Path inside the binhost repository.
    pub binpkg_path: String,
    /// SHA256 of the binpkg file.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

/// A package that failed to build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedPackage {
    pub atom: String,
    pub reason: String,
    pub build_log: Option<String>,
}
