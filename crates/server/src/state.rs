//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::{RwLock, Semaphore, broadcast};

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
}
