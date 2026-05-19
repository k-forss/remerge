//! HTTP API request/response types shared between CLI (client) and server.

use adler2::adler32_slice;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthMode;
use crate::client::{ClientId, ClientRole};
use crate::portage::{PortageConfig, SystemIdentity};
use crate::workorder::{WorkorderId, WorkorderResult, WorkorderStatus};

pub const SNAPSHOT_BLOB_PROTOCOL_VERSION: u8 = 1;
pub const SNAPSHOT_BLOB_CHUNK_MAGIC: [u8; 4] = *b"RMCH";
pub const SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES: u64 = 10 * 1024 * 1024;
pub const SNAPSHOT_BLOB_CHUNK_HEADER_LEN: usize = 36;
pub const SNAPSHOT_BLOB_ENCODING_ZSTD: &str = "zstd";

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
    /// Root trace ID attached to this workorder.
    #[serde(default)]
    pub trace_id: Option<String>,
}

// ─── Snapshot transport ────────────────────────────────────────────

/// POST `/api/v1/snapshots/missing-blobs`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindMissingBlobsRequest {
    pub digests: Vec<String>,
}

/// Response for snapshot blob discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindMissingBlobsResponse {
    pub missing_digests: Vec<String>,
}

/// PUT `/api/v1/snapshots/blobs/{digest}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadBlobResponse {
    pub digest: String,
    pub uploaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotBlobEncodingOffer {
    pub encoding: String,
    pub size_bytes: u64,
}

/// GET `/api/v1/snapshots/blobs/stream`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SnapshotBlobClientControlMessage {
    UploadInit {
        version: u8,
        workorder_id: Uuid,
        digest: String,
        total_size_bytes: u64,
        chunk_size_bytes: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capability_flags: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        offered_encodings: Vec<SnapshotBlobEncodingOffer>,
    },
}

/// Text control frames sent by the server while streaming a snapshot blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SnapshotBlobServerControlMessage {
    UploadResume {
        version: u8,
        workorder_id: Uuid,
        digest: String,
        next_offset_bytes: u64,
        next_sequence: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_encoding: Option<String>,
        expected_size_bytes: u64,
    },
    UploadAck {
        version: u8,
        workorder_id: Uuid,
        digest: String,
        sequence: u64,
        offset_bytes: u64,
        size_bytes: u64,
        received_bytes: u64,
    },
    UploadComplete {
        version: u8,
        workorder_id: Uuid,
        digest: String,
        uploaded: bool,
    },
    UploadError {
        version: u8,
        workorder_id: Option<Uuid>,
        digest: Option<String>,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBlobChunkHeader {
    pub sequence: u64,
    pub offset_bytes: u64,
    pub payload_size_bytes: u64,
    pub payload_checksum: u32,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotBlobChunkFrameError {
    #[error("chunk frame too short: expected at least {expected} bytes, got {actual}")]
    FrameTooShort { expected: usize, actual: usize },
    #[error("invalid chunk frame magic")]
    InvalidMagic,
    #[error("unsupported chunk frame version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported chunk frame flags {0}")]
    UnsupportedFlags(u8),
    #[error("reserved chunk frame bytes must be zero")]
    NonZeroReserved,
    #[error("chunk payload size mismatch: header says {declared} bytes, frame has {actual}")]
    PayloadSizeMismatch { declared: u64, actual: usize },
    #[error("chunk payload checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: u32, actual: u32 },
    #[error("chunk payload too large to encode")]
    PayloadTooLarge,
}

impl SnapshotBlobChunkHeader {
    pub fn from_payload(sequence: u64, offset_bytes: u64, payload: &[u8]) -> Self {
        Self {
            sequence,
            offset_bytes,
            payload_size_bytes: payload.len() as u64,
            payload_checksum: adler32_slice(payload),
        }
    }

    pub fn encode_with_payload(
        &self,
        payload: &[u8],
    ) -> Result<Vec<u8>, SnapshotBlobChunkFrameError> {
        if payload.len() != self.payload_size_bytes as usize {
            return Err(SnapshotBlobChunkFrameError::PayloadSizeMismatch {
                declared: self.payload_size_bytes,
                actual: payload.len(),
            });
        }
        if adler32_slice(payload) != self.payload_checksum {
            return Err(SnapshotBlobChunkFrameError::ChecksumMismatch {
                expected: self.payload_checksum,
                actual: adler32_slice(payload),
            });
        }

        let mut frame = Vec::with_capacity(SNAPSHOT_BLOB_CHUNK_HEADER_LEN + payload.len());
        frame.extend_from_slice(&SNAPSHOT_BLOB_CHUNK_MAGIC);
        frame.push(SNAPSHOT_BLOB_PROTOCOL_VERSION);
        frame.push(0);
        frame.extend_from_slice(&0u16.to_be_bytes());
        frame.extend_from_slice(&self.sequence.to_be_bytes());
        frame.extend_from_slice(&self.offset_bytes.to_be_bytes());
        frame.extend_from_slice(&self.payload_size_bytes.to_be_bytes());
        frame.extend_from_slice(&self.payload_checksum.to_be_bytes());
        frame.extend_from_slice(payload);
        Ok(frame)
    }

    pub fn decode(frame: &[u8]) -> Result<(Self, &[u8]), SnapshotBlobChunkFrameError> {
        if frame.len() < SNAPSHOT_BLOB_CHUNK_HEADER_LEN {
            return Err(SnapshotBlobChunkFrameError::FrameTooShort {
                expected: SNAPSHOT_BLOB_CHUNK_HEADER_LEN,
                actual: frame.len(),
            });
        }
        if frame[..4] != SNAPSHOT_BLOB_CHUNK_MAGIC {
            return Err(SnapshotBlobChunkFrameError::InvalidMagic);
        }

        let version = frame[4];
        if version != SNAPSHOT_BLOB_PROTOCOL_VERSION {
            return Err(SnapshotBlobChunkFrameError::UnsupportedVersion(version));
        }

        let flags = frame[5];
        if flags != 0 {
            return Err(SnapshotBlobChunkFrameError::UnsupportedFlags(flags));
        }

        let reserved = u16::from_be_bytes([frame[6], frame[7]]);
        if reserved != 0 {
            return Err(SnapshotBlobChunkFrameError::NonZeroReserved);
        }

        let sequence = u64::from_be_bytes(frame[8..16].try_into().unwrap());
        let offset_bytes = u64::from_be_bytes(frame[16..24].try_into().unwrap());
        let payload_size_bytes = u64::from_be_bytes(frame[24..32].try_into().unwrap());
        let payload_checksum = u32::from_be_bytes(frame[32..36].try_into().unwrap());
        let payload = &frame[SNAPSHOT_BLOB_CHUNK_HEADER_LEN..];
        if payload.len() != payload_size_bytes as usize {
            return Err(SnapshotBlobChunkFrameError::PayloadSizeMismatch {
                declared: payload_size_bytes,
                actual: payload.len(),
            });
        }
        let actual_checksum = adler32_slice(payload);
        if actual_checksum != payload_checksum {
            return Err(SnapshotBlobChunkFrameError::ChecksumMismatch {
                expected: payload_checksum,
                actual: actual_checksum,
            });
        }

        Ok((
            Self {
                sequence,
                offset_bytes,
                payload_size_bytes,
                payload_checksum,
            },
            payload,
        ))
    }
}

// ─── Query workorder ────────────────────────────────────────────────

/// GET `/api/v1/workorders/{id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkorderStatusResponse {
    pub workorder_id: WorkorderId,
    pub status: WorkorderStatus,
    pub result: Option<WorkorderResult>,
    #[serde(default)]
    pub trace_id: Option<String>,
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
    /// Full fingerprint of the public signing key exposed by the server.
    #[serde(default)]
    pub signing_key_fingerprint: Option<String>,
    /// Public endpoint that serves the ASCII-armored signing key.
    #[serde(default)]
    pub signing_key_endpoint: Option<String>,
}

// ─── Structured log forwarding ─────────────────────────────────────────────────

/// Severity of a forwarded log event.
///
/// Matches the `tracing` crate level hierarchy.  The server applies a
/// per-connection ceiling based on the verbosity the CLI requested at WS
/// upgrade time (`?log_level=`), so the client only receives the events it
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// A structured tracing event forwarded from the server or worker back to the
/// CLI over the progress WebSocket as a text frame.
///
/// The server only forwards events that belong to the requesting client's
/// workorder (`workorder_id` must match, and `target` must be prefixed with
/// `remerge_worker::` or tagged with the matching workorder ID).
/// Server-wide events (auth failures, pool state, scheduler internals) are
/// never forwarded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub level: LogLevel,
    /// Rust module path of the emitting log site (e.g. `remerge_worker::builder`).
    pub target: String,
    pub message: String,
    pub workorder_id: WorkorderId,
    /// Name of the active span at the time the event was emitted, if any.
    pub span: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(format!("unknown log level: {other}")),
        }
    }
}
