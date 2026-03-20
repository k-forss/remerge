mod api;
mod auth;
mod config;
mod docker;
mod metrics;
mod persistence;
mod queue;
mod registry;
mod repo;
mod state;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use remerge_types::workorder::WorkorderStatus;

use crate::config::ServerConfig;
use crate::state::AppState;

/// remerge-server — binary host build coordinator.
#[derive(Parser, Debug)]
#[command(name = "remerge-server", version)]
struct Cli {
    /// Path to the server configuration file.
    #[arg(short, long, default_value = "/etc/remerge/server.toml")]
    config: String,

    /// Listen address.
    #[arg(long, default_value = "0.0.0.0:7654")]
    listen: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI first so we can peek at config before installing the
    // subscriber.
    let cli = Cli::parse();
    let config = ServerConfig::load(&cli.config)?;

    // Install tracing subscriber — JSON or human-readable.
    let env_filter = EnvFilter::from_default_env();
    if config.log_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    info!(?config, "Loaded configuration");

    let state = Arc::new(AppState::new(config).await?);

    // Ensure binpkg repository directory exists.
    state.binpkg_repo.init().await?;

    // ── Background tasks ────────────────────────────────────────────

    // Workorder queue processor.
    let queue_state = state.clone();
    tokio::spawn(async move {
        queue::process_queue(queue_state).await;
    });

    // Periodic state persistence (every 60 s).
    let persist_state = state.clone();
    tokio::spawn(async move {
        persistence::run_periodic_save(persist_state, Duration::from_secs(60)).await;
    });

    // TTL-based workorder eviction.
    let eviction_state = state.clone();
    tokio::spawn(async move {
        run_eviction_task(eviction_state).await;
    });

    // Worker image reaper (idle timeout).
    let reaper_state = state.clone();
    tokio::spawn(async move {
        run_image_reaper(reaper_state).await;
    });

    // Binpkg disk usage monitoring.
    let disk_state = state.clone();
    tokio::spawn(async move {
        run_disk_usage_monitor(disk_state).await;
    });

    // ── HTTP server ─────────────────────────────────────────────────

    let app = api::router(state.clone());

    if let Some(ref tls) = state.config.tls {
        info!("TLS enabled — serving HTTPS on {}", cli.listen);
        serve_tls(app, &cli.listen, tls).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
        info!("Listening on {}", cli.listen);
        axum::serve(listener, app).await?;
    }

    Ok(())
}

/// Serve the application with TLS using `tokio-rustls`.
async fn serve_tls(app: axum::Router, addr: &str, tls_cfg: &config::TlsConfig) -> Result<()> {
    use tokio_rustls::TlsAcceptor;

    let cert_pem = std::fs::read(&tls_cfg.cert)
        .with_context(|| format!("Failed to read TLS cert: {}", tls_cfg.cert.display()))?;
    let key_pem = std::fs::read(&tls_cfg.key)
        .with_context(|| format!("Failed to read TLS key: {}", tls_cfg.key.display()))?;

    use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

    let certs = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to parse TLS certificate")?;
    let key =
        PrivateKeyDer::from_pem_slice(&key_pem).context("No private key found in key file")?;

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("Invalid TLS configuration")?;
    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let acceptor = TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("TLS listener bound to {addr}");

    loop {
        let (stream, _remote_addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            let Ok(tls_stream) = acceptor.accept(stream).await else {
                return;
            };

            let io = hyper_util::rt::TokioIo::new(tls_stream);

            // Create a hyper service that bridges Incoming → axum::body::Body.
            let service =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let mut router = app.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let req = hyper::Request::from_parts(parts, axum::body::Body::new(body));
                        // Router::call error is Infallible.
                        <axum::Router as tower::Service<hyper::Request<axum::body::Body>>>::call(
                            &mut router,
                            req,
                        )
                        .await
                    }
                });

            let builder =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            let _ = builder.serve_connection_with_upgrades(io, service).await;
        });
    }
}

/// Background task: evict completed/failed workorders older than retention_hours.
async fn run_eviction_task(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        let cutoff =
            chrono::Utc::now() - chrono::Duration::hours(state.config.retention_hours as i64);

        let mut workorders = state.workorders.write().await;
        let mut results = state.results.write().await;
        let mut progress_txs = state.progress_txs.write().await;

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

        if !stale_ids.is_empty() {
            info!(count = stale_ids.len(), "Evicting stale workorders");
        }

        for id in stale_ids {
            workorders.remove(&id);
            results.remove(&id);
            progress_txs.remove(&id);
        }

        // Enforce max-entry cap: if the workorder count still exceeds the
        // configured limit, evict the oldest terminal entries first.
        let cap = state.config.max_retained_workorders;
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
            if !to_evict.is_empty() {
                info!(
                    count = to_evict.len(),
                    "Evicting workorders (max cap exceeded)"
                );
            }
            for id in to_evict {
                workorders.remove(&id);
                results.remove(&id);
                progress_txs.remove(&id);
            }
        }

        // Also clean up old package versions, keeping the 3 most recent per
        // package.  This avoids unbounded disk usage.
        drop(workorders);
        drop(results);
        drop(progress_txs);
        match state.binpkg_repo.cleanup_old_versions(3).await {
            Ok(removed) if removed > 0 => {
                info!(removed, "Cleaned up old package versions");
            }
            Ok(_) => {}
            Err(e) => {
                warn!("Failed to clean up old package versions: {e:#}");
            }
        }
    }
}

/// Background task: remove unused worker images after idle timeout.
///
/// Preserves the most recently used image per `(CHOST, profile)` group
/// (the GCC version suffix is stripped, so different GCC versions within
/// the same CHOST+profile share a protection group).  Only truly idle
/// duplicates are evicted.
async fn run_image_reaper(state: Arc<AppState>) {
    let timeout = Duration::from_secs(state.config.worker_idle_timeout);
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // Check every 5 min.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        let now = Instant::now();

        let expired: Vec<String> = {
            let images = state.image_last_used.read().await;

            // Group by tuple key — everything before the `-gccVER` suffix,
            // i.e. the `(CHOST, profile)` pair.  Different GCC versions
            // within the same group share one protection slot.
            // Image tags look like: prefix:chost-profile-gccVER
            let mut newest_per_tuple: std::collections::HashMap<String, (String, Instant)> =
                std::collections::HashMap::new();

            for (tag, last_used) in images.iter() {
                let tuple_key = image_tuple_key(tag);
                match newest_per_tuple.get(&tuple_key) {
                    Some((_, existing_ts)) if last_used > existing_ts => {
                        newest_per_tuple.insert(tuple_key, (tag.clone(), *last_used));
                    }
                    None => {
                        newest_per_tuple.insert(tuple_key, (tag.clone(), *last_used));
                    }
                    _ => {}
                }
            }

            let protected: std::collections::HashSet<&str> = newest_per_tuple
                .values()
                .map(|(tag, _)| tag.as_str())
                .collect();

            images
                .iter()
                .filter(|(tag, last_used)| {
                    now.duration_since(**last_used) > timeout && !protected.contains(tag.as_str())
                })
                .map(|(tag, _)| tag.clone())
                .collect()
        };

        for tag in expired {
            info!(%tag, "Removing idle worker image");
            if let Err(e) = state.docker.remove_image(&tag).await {
                warn!(%tag, "Failed to remove idle image: {e}");
            }
            state.image_last_used.write().await.remove(&tag);
        }
    }
}

/// Extract the tuple key from an image tag for grouping.
///
/// Image tags are `prefix:chost-profile-gccVER`.  The tuple key is
/// everything up to and including the profile segment, so different GCC
/// versions within the same (CHOST, profile) share a group.
fn image_tuple_key(tag: &str) -> String {
    // Strip the prefix (everything up to ':').
    let suffix = tag.split_once(':').map_or(tag, |(_, s)| s);
    // The suffix looks like "chost-profile-gccVER".  Remove the last
    // `-gccXX_YY_ZZ` segment.
    match suffix.rfind("-gcc") {
        Some(idx) => suffix[..idx].to_string(),
        None => suffix.to_string(),
    }
}

/// Background task: periodically check binpkg directory size and update metrics.
async fn run_disk_usage_monitor(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 min.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        match state.binpkg_repo.disk_usage().await {
            Ok(bytes) => {
                state
                    .metrics
                    .binpkg_disk_usage_bytes
                    .store(bytes, std::sync::atomic::Ordering::Relaxed);

                let threshold = state.config.binpkg_disk_warn_bytes;
                if threshold > 0 && bytes > threshold {
                    warn!(
                        bytes,
                        threshold, "Binpkg directory exceeds disk usage threshold"
                    );
                }
            }
            Err(e) => {
                warn!("Failed to check binpkg disk usage: {e:#}");
            }
        }
    }
}
