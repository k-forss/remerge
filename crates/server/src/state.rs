//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use bytes::Bytes;
use tokio::sync::{RwLock, Semaphore, broadcast, mpsc};

use remerge_types::workorder::{BuildProgress, Workorder, WorkorderId, WorkorderResult};

use crate::auth::CertRegistry;
use crate::config::ServerConfig;
use crate::docker::DockerManager;
use crate::metrics::Metrics;
use crate::registry::ClientRegistry;
use crate::repo::BinpkgRepo;

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

    /// Server start time (for uptime reporting).
    pub started_at: Instant,

    /// Prometheus-compatible metrics counters.
    pub metrics: Metrics,

    /// Binary package repository.
    pub binpkg_repo: BinpkgRepo,
}

impl AppState {
    pub async fn new(config: ServerConfig) -> Result<Self> {
        let docker = DockerManager::new(&config).await?;
        let auth = CertRegistry::new(&config.auth);

        // Ensure binpkg directory exists.
        tokio::fs::create_dir_all(&config.binpkg_dir).await?;

        // Ensure state directory exists.
        tokio::fs::create_dir_all(&config.state_dir).await?;

        // Load persisted state.
        let persisted_results = crate::persistence::load_results(&config.state_dir)
            .await
            .unwrap_or_default();
        let persisted_clients = crate::persistence::load_clients(&config.state_dir)
            .await
            .unwrap_or_default();

        let max_workers = config.max_workers;
        let binpkg_repo = BinpkgRepo::new(config.binpkg_dir.clone());

        Ok(Self {
            config,
            docker,
            auth,
            clients: ClientRegistry::from_persisted(persisted_clients),
            workorders: RwLock::new(HashMap::new()),
            results: RwLock::new(persisted_results),
            progress_txs: RwLock::new(HashMap::new()),
            raw_output_txs: RwLock::new(HashMap::new()),
            stdin_txs: RwLock::new(HashMap::new()),
            worker_semaphore: Arc::new(Semaphore::new(max_workers)),
            container_ids: RwLock::new(HashMap::new()),
            image_last_used: RwLock::new(HashMap::new()),
            started_at: Instant::now(),
            metrics: Metrics::new(),
            binpkg_repo,
        })
    }

    /// Create a broadcast channel for a workorder and return the sender.
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

        tx
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

    /// Remove all per-workorder channels (progress, raw output, stdin) when a
    /// workorder finishes.
    ///
    /// Note: the raw output channel may already have been removed by
    /// `process_workorder` before the `Finished` event is broadcast.
    /// The WS handler tolerates a missing raw channel by falling back
    /// to text-only mode.
    pub async fn remove_workorder_channels(&self, id: &WorkorderId) {
        self.progress_txs.write().await.remove(id);
        self.raw_output_txs.write().await.remove(id);
        self.stdin_txs.write().await.remove(id);
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
            }
            evicted += to_evict.len();
        }

        evicted
    }
}
