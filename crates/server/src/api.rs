//! HTTP + WebSocket API.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::{
    Router,
    extract::{Path, State, WebSocketUpgrade, ws},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
};
use tracing::{error, info, warn};
use uuid::Uuid;

use futures::StreamExt;

use remerge_types::validation::validate_atom;
use remerge_types::{api::*, workorder::*};

use crate::state::AppState;

type SharedState = Arc<AppState>;

/// Build the axum router.
pub fn router(state: SharedState) -> Router {
    Router::new()
        // Public API
        .route("/api/v1/info", get(server_info))
        .route("/api/v1/health", get(health))
        .route("/api/v1/workorders", post(submit_workorder))
        .route("/api/v1/workorders", get(list_workorders))
        .route("/api/v1/workorders/{id}", get(get_workorder))
        .route("/api/v1/workorders/{id}", delete(cancel_workorder))
        .route("/api/v1/workorders/{id}/progress", get(ws_progress))
        // Admin / status endpoints.
        .route("/api/v1/clients", get(list_clients))
        .route("/api/v1/clients/{id}", get(get_client))
        // Observability.
        .route("/metrics", get(metrics))
        // Static file serving for binpkgs.
        .nest_service(
            "/binpkgs",
            tower_http::services::ServeDir::new(&state.config.binpkg_dir),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

// ─── Handlers ───────────────────────────────────────────────────────

async fn server_info(State(state): State<SharedState>) -> impl IntoResponse {
    let workorders = state.workorders.read().await;
    let active = workorders
        .values()
        .filter(|w| {
            matches!(
                w.status,
                WorkorderStatus::Building | WorkorderStatus::Provisioning
            )
        })
        .count();
    let queued = workorders
        .values()
        .filter(|w| matches!(w.status, WorkorderStatus::Pending))
        .count();

    axum::Json(ServerInfoResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        binhost_base_url: state.config.binhost_url.clone(),
        active_workers: active,
        queued_workorders: queued,
        auth_mode: state.auth.mode(),
        binpkg_signing: state.config.signing.enabled(),
    })
}

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    axum::Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.started_at.elapsed().as_secs(),
    })
}

async fn submit_workorder(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SubmitWorkorderRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Extract the Host header for building the WebSocket URL.
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:7654");

    // ── Validate atoms ──────────────────────────────────────────────
    for atom in &req.atoms {
        if let Err(e) = validate_atom(atom) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Invalid atom '{atom}': {e}"),
            ));
        }
    }

    // ── Authenticate ────────────────────────────────────────────────
    let identity = state
        .auth
        .resolve(&headers, req.client_id, req.role)
        .map_err(|e| {
            tracing::warn!(error = %e, "Authentication failed");
            (e.status_code(), e.to_string())
        })?;

    let client_id = identity.client_id;
    let role = identity.role;

    tracing::info!(
        %client_id,
        %role,
        auth_method = %identity.method,
        "Request authenticated"
    );

    // ── Validate client role and check for active workorders ────────
    let diff = state
        .clients
        .update(client_id, role, &req.portage_config, &req.system_id)
        .await
        .map_err(|e| {
            tracing::warn!(
                %client_id,
                %role,
                "Workorder rejected: {e}"
            );
            (StatusCode::CONFLICT, e.to_string())
        })?;

    let id = Uuid::new_v4();
    let now = chrono::Utc::now();

    info!(
        ?id,
        %client_id,
        %role,
        atoms = ?req.atoms,
        portage_changed = diff.portage_changed,
        system_changed = diff.system_changed,
        "New workorder submitted"
    );

    let workorder = Workorder {
        id,
        client_id,
        role,
        atoms: req.atoms,
        emerge_args: req.emerge_args,
        portage_config: req.portage_config,
        system_id: req.system_id,
        status: WorkorderStatus::Pending,
        created_at: now,
        updated_at: now,
    };

    // Mark this client as having an active workorder.
    state.clients.set_active_workorder(&client_id, id).await;

    // Create the progress channel BEFORE inserting the workorder so the
    // queue processor never finds a workorder without its channel.
    state.create_progress_channel(id).await;
    state.workorders.write().await.insert(id, workorder);

    // Track submission in metrics.
    state
        .metrics
        .workorders_submitted
        .fetch_add(1, Ordering::Relaxed);

    // Build the WebSocket URL from the request's Host header.
    // Detect TLS from X-Forwarded-Proto (set by reverse proxies).
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|proto| {
            if proto.eq_ignore_ascii_case("https") {
                "wss"
            } else {
                "ws"
            }
        })
        .unwrap_or(if state.config.tls.is_some() {
            "wss"
        } else {
            "ws"
        });
    let progress_ws_url = format!("{scheme}://{host}/api/v1/workorders/{id}/progress");

    Ok(axum::Json(SubmitWorkorderResponse {
        workorder_id: id,
        progress_ws_url,
    }))
}

async fn list_workorders(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let workorders = state.workorders.read().await;

    // In authenticated modes, require a valid cert and scope results.
    let auth_client_id = if state.auth.mode() != remerge_types::auth::AuthMode::None {
        Some(
            state
                .auth
                .resolve_header_only(&headers)
                .ok_or(StatusCode::UNAUTHORIZED)?,
        )
    } else {
        None
    };

    let summaries: Vec<WorkorderSummary> = workorders
        .values()
        .filter(|w| match auth_client_id {
            Some(cid) => w.client_id == cid,
            None => true,
        })
        .map(|w| WorkorderSummary {
            id: w.id,
            atoms: w.atoms.clone(),
            status: w.status.clone(),
            created_at: w.created_at,
        })
        .collect();

    Ok(axum::Json(ListWorkordersResponse {
        workorders: summaries,
    }))
}

async fn get_workorder(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, StatusCode> {
    let workorders = state.workorders.read().await;
    let workorder = workorders.get(&id).ok_or(StatusCode::NOT_FOUND)?;

    // In authenticated modes, only the owning client can view their workorder.
    if state.auth.mode() != remerge_types::auth::AuthMode::None {
        let auth_client_id = state.auth.resolve_header_only(&headers);
        match auth_client_id {
            Some(cid) if cid == workorder.client_id => { /* owner — OK */ }
            Some(_) => return Err(StatusCode::FORBIDDEN),
            None => return Err(StatusCode::UNAUTHORIZED),
        }
    }

    let result = state.results.read().await.get(&id).cloned();

    Ok(axum::Json(WorkorderStatusResponse {
        workorder_id: id,
        status: workorder.status.clone(),
        result,
    }))
}

async fn cancel_workorder(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // ── Authenticate cancel requests in non-None auth modes ─────────
    if state.auth.mode() != remerge_types::auth::AuthMode::None {
        let auth_client_id = state.auth.resolve_header_only(&headers);
        let workorders = state.workorders.read().await;
        if let Some(workorder) = workorders.get(&id) {
            match auth_client_id {
                Some(cid) if cid == workorder.client_id => {
                    // Authenticated and owns this workorder — proceed.
                }
                Some(_) => {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "You do not own this workorder".to_string(),
                    ));
                }
                None => {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        "Authentication required to cancel workorders".to_string(),
                    ));
                }
            }
        }
    }

    let mut workorders = state.workorders.write().await;
    let workorder = workorders
        .get_mut(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workorder not found".to_string()))?;

    let was_cancellable = matches!(
        workorder.status,
        WorkorderStatus::Pending | WorkorderStatus::Provisioning | WorkorderStatus::Building
    );

    if was_cancellable {
        let old_status = workorder.status.clone();
        let was_building = matches!(old_status, WorkorderStatus::Building);
        workorder.status = WorkorderStatus::Cancelled;
        let client_id = workorder.client_id;
        drop(workorders); // Release the write lock before async calls.

        // Broadcast StatusChanged event so WebSocket clients see the cancellation.
        if let Some(tx) = state.progress_txs.read().await.get(&id) {
            let _ = tx.send(BuildProgress {
                workorder_id: id,
                event: BuildEvent::StatusChanged {
                    from: old_status,
                    to: WorkorderStatus::Cancelled,
                },
                timestamp: chrono::Utc::now(),
            });
        }

        state.clients.clear_active_workorder(&client_id).await;

        // If the workorder was Building, stop the Docker container.
        if was_building && let Some(container_id) = state.container_ids.read().await.get(&id) {
            info!(?id, "Stopping container for cancelled workorder");
            if let Err(e) = state.docker.stop_container(container_id).await {
                warn!(?id, "Failed to stop container: {e}");
            }
        }

        state
            .metrics
            .workorders_cancelled
            .fetch_add(1, Ordering::Relaxed);
    }

    Ok(axum::Json(CancelWorkorderResponse {
        workorder_id: id,
        cancelled: was_cancellable,
    }))
}

// ─── Admin / Status ─────────────────────────────────────────────────

async fn list_clients(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    // Admin endpoints require authentication in non-None modes.
    if state.auth.mode() != remerge_types::auth::AuthMode::None {
        state
            .auth
            .resolve_header_only(&headers)
            .ok_or(StatusCode::UNAUTHORIZED)?;
    }

    let clients = state.clients.list_all().await;
    let summaries: Vec<ClientSummary> = clients
        .into_iter()
        .map(|c| ClientSummary {
            client_id: c.client_id,
            config_hash: c.config_hash,
            last_seen: c.last_seen,
            active_workorder: c.active_workorder,
        })
        .collect();

    Ok(axum::Json(ClientListResponse { clients: summaries }))
}

async fn get_client(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, StatusCode> {
    if state.auth.mode() != remerge_types::auth::AuthMode::None {
        state
            .auth
            .resolve_header_only(&headers)
            .ok_or(StatusCode::UNAUTHORIZED)?;
    }

    let client = state.clients.get(&id).await.ok_or(StatusCode::NOT_FOUND)?;

    Ok(axum::Json(ClientDetailResponse {
        client_id: client.client_id,
        config_hash: client.config_hash,
        system_hash: client.system_hash,
        last_seen: client.last_seen,
        active_workorder: client.active_workorder,
    }))
}

// ─── Observability ──────────────────────────────────────────────────

async fn metrics(State(state): State<SharedState>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.to_prometheus(),
    )
}

// ─── WebSocket ──────────────────────────────────────────────────────

/// WebSocket endpoint that streams [`BuildProgress`] events.
///
/// The connection is bidirectional:
/// - **Server → Client:** Build progress events (log lines, status changes, etc.)
/// - **Client → Server:** Stdin data forwarded to the worker container for
///   interactive emerge support (`--ask`, USE prompts, etc.)
async fn ws_progress(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    let rx = state
        .subscribe_progress(&id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let stdin_tx = state.get_stdin_tx(&id).await;

    Ok(ws.on_upgrade(move |socket| handle_ws(socket, rx, stdin_tx)))
}

async fn handle_ws(
    socket: ws::WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<BuildProgress>,
    stdin_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
) {
    let (mut ws_write, mut ws_read) = socket.split();

    // Task 1: Forward build progress events to the client.
    let send_task = tokio::spawn(async move {
        while let Ok(progress) = rx.recv().await {
            let text = match serde_json::to_string(&progress) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to serialise progress: {e}");
                    continue;
                }
            };

            use futures::SinkExt;
            if ws_write.send(ws::Message::Text(text.into())).await.is_err() {
                break; // Client disconnected.
            }

            // If the build is finished, close the socket.
            if matches!(progress.event, BuildEvent::Finished { .. }) {
                let _ = ws_write.send(ws::Message::Close(None)).await;
                break;
            }
        }
    });

    // Task 2: Read client messages (stdin data) and forward to the container.
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_read.next().await {
            match msg {
                ws::Message::Text(text) => {
                    if let Some(ref tx) = stdin_tx {
                        // Send the text as raw bytes to the container's stdin.
                        let _ = tx.send(text.as_bytes().to_vec()).await;
                    }
                }
                ws::Message::Binary(data) => {
                    if let Some(ref tx) = stdin_tx {
                        let _ = tx.send(data.to_vec()).await;
                    }
                }
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish, then abort the other.
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}
