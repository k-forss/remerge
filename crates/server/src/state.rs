//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
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
    pub raw_output_txs: RwLock<HashMap<WorkorderId, broadcast::Sender<Vec<u8>>>>,

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
        let (tx, _) = broadcast::channel(256);
        self.progress_txs.write().await.insert(id, tx.clone());

        // Also create the raw output channel.
        let (raw_tx, _) = broadcast::channel(512);
        self.raw_output_txs.write().await.insert(id, raw_tx);

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
    ) -> Option<broadcast::Receiver<Vec<u8>>> {
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

    /// Remove the stdin channel when a workorder finishes.
    pub async fn remove_stdin_channel(&self, id: &WorkorderId) {
        self.stdin_txs.write().await.remove(id);
        self.raw_output_txs.write().await.remove(id);
    }
}
