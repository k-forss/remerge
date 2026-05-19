//! Shared application state.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast, mpsc};
use tokio::task;

use remerge_types::api::{LogEvent, LogLevel};
use remerge_types::workorder::{
    BuildProgress, Workorder, WorkorderId, WorkorderResult, WorkorderStatus,
};

use crate::auth::CertRegistry;
use crate::config::ServerConfig;
use crate::docker::DockerManager;
use crate::metrics::Metrics;
use crate::registry::ClientRegistry;
use crate::repo::BinpkgRepo;
use crate::runtime::SnapshotReferenceSet;
use crate::signing::ExportedSigningKey;

/// Maximum number of log events retained per workorder for WS replay.
const LOG_RING_CAPACITY: usize = 256;

/// Central application state shared across handlers and the queue processor.
pub struct AppState {
    pub config: ServerConfig,
    pub docker: DockerManager,

    /// Certificate / mTLS authentication registry.
    pub auth: CertRegistry,

    /// Known-client registry — tracks portage config hashes per client ID.
    pub clients: ClientRegistry,

    /// Pending + active workorders.
    pub workorders: RwLock<HashMap<WorkorderId, Workorder>>,

    /// Serializes submission admission so capacity checks and inserts stay atomic.
    pub submission_lock: Mutex<()>,

    /// Completed workorder results.
    pub results: RwLock<HashMap<WorkorderId, WorkorderResult>>,

    /// Per-workorder broadcast channels for progress streaming.
    pub progress_txs: RwLock<HashMap<WorkorderId, broadcast::Sender<BuildProgress>>>,

    /// Per-workorder broadcast channels for raw PTY output (binary bytes).
    /// Sent as WS Binary frames — this is the primary output channel.
    /// Uses `Bytes` (reference-counted) to avoid a full allocation clone per receiver.
    pub raw_output_txs: RwLock<HashMap<WorkorderId, broadcast::Sender<Bytes>>>,

    /// Per-workorder stdin channels for forwarding client input to the
    /// worker container (supports interactive emerge, `--ask`, etc.).
    pub stdin_txs: RwLock<HashMap<WorkorderId, mpsc::Sender<Vec<u8>>>>,

    /// Semaphore to limit concurrent worker containers to `max_workers`.
    pub worker_semaphore: Arc<Semaphore>,

    /// Maps workorder IDs to their Docker container IDs (for cancellation).
    pub container_ids: RwLock<HashMap<WorkorderId, String>>,

    /// Tracks last-used time of worker images (for idle timeout reaping).
    pub image_last_used: RwLock<HashMap<String, Instant>>,

    /// Tracks which staged workorders currently reference which snapshot blobs and trees.
    pub staged_workorder_references: RwLock<HashMap<WorkorderId, SnapshotReferenceSet>>,

    /// Server start time (for uptime reporting).
    pub started_at: Instant,

    /// Prometheus-compatible metrics counters.
    pub metrics: Metrics,

    /// Binary package repository.
    pub binpkg_repo: BinpkgRepo,

    /// Exported public signing key, if binpkg signing is enabled.
    pub signing_key: Option<ExportedSigningKey>,

    /// Per-workorder ring buffers holding the most recent log events.
    /// Replayed to newly connected WS clients so they receive log history
    /// even when connecting after the worker has already emitted events.
    pub log_ring_bufs: RwLock<HashMap<WorkorderId, VecDeque<LogEvent>>>,

    /// Per-workorder broadcast channels for live log event forwarding.
    pub log_event_txs: RwLock<HashMap<WorkorderId, broadcast::Sender<LogEvent>>>,
}

impl AppState {
    pub async fn new(config: ServerConfig) -> Result<Self> {
        let docker = DockerManager::new(&config).await?;
        let auth = CertRegistry::new(&config.auth);
        let signing_config = config.signing.clone();
        let signing_key =
            task::spawn_blocking(move || crate::signing::export_public_key(&signing_config))
                .await
                .context("signing-key export task panicked")??;

        // Ensure binpkg directory exists.
        tokio::fs::create_dir_all(&config.binpkg_dir).await?;

        // Ensure state directory exists.
        tokio::fs::create_dir_all(&config.state_dir).await?;

        // Load persisted state.
        let persisted_workorders = crate::persistence::load_workorders(&config.state_dir)
            .await
            .unwrap_or_default();
        let persisted_results = crate::persistence::load_results(&config.state_dir)
            .await
            .unwrap_or_default();
        let persisted_clients = crate::persistence::load_clients(&config.state_dir)
            .await
            .unwrap_or_default();

        let mut progress_txs = HashMap::new();
        let mut raw_output_txs = HashMap::new();
        for id in persisted_workorders.keys().copied().filter(|id| {
            persisted_workorders.get(id).is_some_and(|workorder| {
                matches!(
                    workorder.status,
                    WorkorderStatus::Pending
                        | WorkorderStatus::Provisioning
                        | WorkorderStatus::Building
                )
            })
        }) {
            let (raw_tx, _) = broadcast::channel(512);
            raw_output_txs.insert(id, raw_tx);
            let (tx, _) = broadcast::channel(256);
            progress_txs.insert(id, tx);
        }

        let max_workers = config.max_workers;
        let binpkg_repo = BinpkgRepo::new(config.binpkg_dir.clone());

        Ok(Self {
            config,
            docker,
            auth,
            clients: ClientRegistry::from_persisted(persisted_clients),
            workorders: RwLock::new(persisted_workorders),
            submission_lock: Mutex::new(()),
            results: RwLock::new(persisted_results),
            progress_txs: RwLock::new(progress_txs),
            raw_output_txs: RwLock::new(raw_output_txs),
            stdin_txs: RwLock::new(HashMap::new()),
            worker_semaphore: Arc::new(Semaphore::new(max_workers)),
            container_ids: RwLock::new(HashMap::new()),
            image_last_used: RwLock::new(HashMap::new()),
            staged_workorder_references: RwLock::new(HashMap::new()),
            started_at: Instant::now(),
            metrics: Metrics::new(),
            binpkg_repo,
            signing_key,
            log_ring_bufs: RwLock::new(HashMap::new()),
            log_event_txs: RwLock::new(HashMap::new()),
        })
    }

    /// Create broadcast channels for a workorder and return the progress sender.
    ///
    /// Also creates the log ring buffer and log event broadcast channel for
    /// the workorder so log events can be stored and forwarded from the
    /// moment the worker starts.
    pub async fn create_progress_channel(
        &self,
        id: WorkorderId,
    ) -> broadcast::Sender<BuildProgress> {
        // Insert raw channel first so that a subscriber who observes the
        // progress channel is guaranteed to also find the raw channel.
        let (raw_tx, _) = broadcast::channel(512);
        self.raw_output_txs.write().await.insert(id, raw_tx);

        let (tx, _) = broadcast::channel(256);
        self.progress_txs.write().await.insert(id, tx.clone());

        // Set up log event storage and broadcasting.
        self.log_ring_bufs
            .write()
            .await
            .insert(id, VecDeque::with_capacity(LOG_RING_CAPACITY));
        let (log_tx, _) = broadcast::channel(LOG_RING_CAPACITY);
        self.log_event_txs.write().await.insert(id, log_tx);

        tx
    }

    /// Push a log event into the workorder's ring buffer and broadcast it to
    /// any connected WS subscribers.
    ///
    /// Older events are evicted from the ring buffer when capacity is reached.
    /// Silently does nothing if the workorder is unknown (e.g. after cleanup).
    pub async fn push_log_event(&self, id: WorkorderId, event: LogEvent) {
        // Store in ring buffer — evict oldest if full.
        if let Some(buf) = self.log_ring_bufs.write().await.get_mut(&id) {
            if buf.len() >= LOG_RING_CAPACITY {
                buf.pop_front();
            }
            buf.push_back(event.clone());
        }
        // Broadcast to live subscribers (it is not an error if there are none).
        if let Some(tx) = self.log_event_txs.read().await.get(&id) {
            let _ = tx.send(event);
        }
    }

    /// Subscribe to live log events for `id` and return a snapshot of buffered
    /// events together with the live receiver.
    ///
    /// The snapshot is taken AFTER subscribing to the channel to avoid a
    /// race where a new event is emitted between the snapshot and subscribe.
    /// Callers replay the snapshot first, then drain from the receiver,
    /// deduplicating any overlap by timestamp.
    ///
    /// Returns `None` if the workorder has no log channel (not yet started or
    /// already cleaned up).
    pub async fn subscribe_logs(
        &self,
        id: &WorkorderId,
        level: LogLevel,
    ) -> Option<(Vec<LogEvent>, broadcast::Receiver<LogEvent>)> {
        let rx = self
            .log_event_txs
            .read()
            .await
            .get(id)
            .map(|tx| tx.subscribe())?;
        let snapshot: Vec<LogEvent> = self
            .log_ring_bufs
            .read()
            .await
            .get(id)
            .map(|buf| buf.iter().filter(|e| e.level <= level).cloned().collect())
            .unwrap_or_default();
        Some((snapshot, rx))
    }

    /// Get a receiver for an existing workorder's progress channel.
    pub async fn subscribe_progress(
        &self,
        id: &WorkorderId,
    ) -> Option<broadcast::Receiver<BuildProgress>> {
        self.progress_txs
            .read()
            .await
            .get(id)
            .map(|tx| tx.subscribe())
    }

    /// Subscribe to the raw PTY output channel for a workorder.
    pub async fn subscribe_raw_output(
        &self,
        id: &WorkorderId,
    ) -> Option<broadcast::Receiver<Bytes>> {
        self.raw_output_txs
            .read()
            .await
            .get(id)
            .map(|tx| tx.subscribe())
    }

    /// Create an mpsc channel for forwarding stdin data to a workorder's
    /// worker container.  Returns the receiver; the sender is stored in
    /// `stdin_txs` so WebSocket handlers can write to it.
    pub async fn create_stdin_channel(&self, id: WorkorderId) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel(64);
        self.stdin_txs.write().await.insert(id, tx);
        rx
    }

    /// Get the stdin sender for an existing workorder (used by the WS handler).
    pub async fn get_stdin_tx(&self, id: &WorkorderId) -> Option<mpsc::Sender<Vec<u8>>> {
        self.stdin_txs.read().await.get(id).cloned()
    }

    /// Remove all per-workorder channels (progress, raw output, stdin, log)
    /// when a workorder finishes.
    ///
    /// Note: the raw output channel may already have been removed by
    /// `process_workorder` before the `Finished` event is broadcast.
    /// The WS handler tolerates a missing raw channel by falling back
    /// to text-only mode.
    pub async fn remove_workorder_channels(&self, id: &WorkorderId) {
        self.progress_txs.write().await.remove(id);
        self.raw_output_txs.write().await.remove(id);
        self.stdin_txs.write().await.remove(id);
        self.log_event_txs.write().await.remove(id);
        self.log_ring_bufs.write().await.remove(id);
    }

    pub async fn track_staged_workorder_references(
        &self,
        id: WorkorderId,
        references: SnapshotReferenceSet,
    ) {
        let mut tracked = self.staged_workorder_references.write().await;
        track_staged_workorder_references(&mut tracked, id, references);
    }

    pub async fn clear_staged_workorder_references(&self, id: &WorkorderId) {
        let mut tracked = self.staged_workorder_references.write().await;
        clear_staged_workorder_references(&mut tracked, id);
    }

    /// Run a single eviction pass — remove stale terminal workorders and
    /// enforce the max-retained cap.
    ///
    /// Returns the number of workorders evicted.
    pub async fn evict_workorders(&self) -> usize {
        use remerge_types::workorder::WorkorderStatus;

        let cutoff =
            chrono::Utc::now() - chrono::Duration::hours(self.config.retention_hours as i64);

        let mut workorders = self.workorders.write().await;
        let mut results = self.results.write().await;
        let mut progress_txs = self.progress_txs.write().await;
        let mut raw_output_txs = self.raw_output_txs.write().await;
        let mut stdin_txs = self.stdin_txs.write().await;
        let mut container_ids = self.container_ids.write().await;
        let mut log_event_txs = self.log_event_txs.write().await;
        let mut log_ring_bufs = self.log_ring_bufs.write().await;

        let mut evicted = 0;

        // Phase 1: remove workorders past the retention cutoff.
        let stale_ids: Vec<_> = workorders
            .iter()
            .filter(|(_, w)| {
                matches!(
                    w.status,
                    WorkorderStatus::Completed
                        | WorkorderStatus::Cancelled
                        | WorkorderStatus::Failed { .. }
                ) && w.updated_at < cutoff
            })
            .map(|(id, _)| *id)
            .collect();

        for id in &stale_ids {
            workorders.remove(id);
            results.remove(id);
            progress_txs.remove(id);
            raw_output_txs.remove(id);
            stdin_txs.remove(id);
            container_ids.remove(id);
            log_event_txs.remove(id);
            log_ring_bufs.remove(id);
        }
        evicted += stale_ids.len();

        // Phase 2: enforce max-entry cap.
        let cap = self.config.max_retained_workorders;
        if cap > 0 && workorders.len() > cap {
            let excess = workorders.len() - cap;
            let mut terminal: Vec<(uuid::Uuid, chrono::DateTime<chrono::Utc>)> = workorders
                .iter()
                .filter(|(_, w)| {
                    matches!(
                        w.status,
                        WorkorderStatus::Completed
                            | WorkorderStatus::Cancelled
                            | WorkorderStatus::Failed { .. }
                    )
                })
                .map(|(id, w)| (*id, w.updated_at))
                .collect();
            terminal.sort_by_key(|&(_, ts)| ts);

            let to_evict: Vec<_> = terminal
                .into_iter()
                .take(excess)
                .map(|(id, _)| id)
                .collect();
            for id in &to_evict {
                workorders.remove(id);
                results.remove(id);
                progress_txs.remove(id);
                raw_output_txs.remove(id);
                stdin_txs.remove(id);
                container_ids.remove(id);
                log_event_txs.remove(id);
                log_ring_bufs.remove(id);
            }
            evicted += to_evict.len();
        }

        evicted
    }
}

pub fn track_staged_workorder_references(
    tracked: &mut HashMap<WorkorderId, SnapshotReferenceSet>,
    id: WorkorderId,
    references: SnapshotReferenceSet,
) {
    tracked.insert(id, references);
}

pub fn clear_staged_workorder_references(
    tracked: &mut HashMap<WorkorderId, SnapshotReferenceSet>,
    id: &WorkorderId,
) {
    tracked.remove(id);
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use chrono::{Duration, Utc};
    use remerge_types::workorder::WorkorderId;

    use super::{
        SnapshotReferenceSet, clear_staged_workorder_references, track_staged_workorder_references,
    };

    #[test]
    fn staged_workorder_reference_tracking_replaces_and_clears_entries() {
        let id = WorkorderId::new_v4();
        let mut tracked = HashMap::new();
        let first_seen = Utc::now() - Duration::minutes(5);
        let refreshed_at = first_seen + Duration::minutes(3);

        track_staged_workorder_references(
            &mut tracked,
            id,
            SnapshotReferenceSet {
                blob_digests: BTreeSet::from(["blob-a".to_string()]),
                tree_digests: BTreeSet::from(["tree-a".to_string()]),
                total_blob_bytes: 11,
                total_tree_bytes: 22,
                last_referenced_at: first_seen,
            },
        );
        assert_eq!(
            tracked[&id].blob_digests,
            BTreeSet::from(["blob-a".to_string()])
        );
        assert_eq!(tracked[&id].total_blob_bytes, 11);
        assert_eq!(tracked[&id].total_tree_bytes, 22);
        assert_eq!(tracked[&id].last_referenced_at, first_seen);

        track_staged_workorder_references(
            &mut tracked,
            id,
            SnapshotReferenceSet {
                blob_digests: BTreeSet::from(["blob-b".to_string()]),
                tree_digests: BTreeSet::from(["tree-b".to_string()]),
                total_blob_bytes: 33,
                total_tree_bytes: 44,
                last_referenced_at: refreshed_at,
            },
        );
        assert_eq!(
            tracked[&id].blob_digests,
            BTreeSet::from(["blob-b".to_string()])
        );
        assert_eq!(
            tracked[&id].tree_digests,
            BTreeSet::from(["tree-b".to_string()])
        );
        assert_eq!(tracked[&id].total_blob_bytes, 33);
        assert_eq!(tracked[&id].total_tree_bytes, 44);
        assert_eq!(tracked[&id].last_referenced_at, refreshed_at);

        clear_staged_workorder_references(&mut tracked, &id);
        assert!(!tracked.contains_key(&id));
    }
}
