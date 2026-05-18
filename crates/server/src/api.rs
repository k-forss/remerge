//! HTTP + WebSocket API.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State, WebSocketUpgrade, ws},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use futures::{SinkExt, StreamExt};
use remerge_types::compression;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use remerge_types::trace::TRACEPARENT_HEADER;
use remerge_types::validation::validate_atom;
use remerge_types::{api::*, workorder::*};

use crate::blob_store;
use crate::persistence;
use crate::runtime;
use crate::state::AppState;

type SharedState = Arc<AppState>;

/// Build the axum router.
pub fn router(state: SharedState) -> Router {
    let public_router = Router::new()
        // Public API
        .route("/api/v1/info", get(server_info))
        .route("/api/v1/signing-key", get(signing_key))
        .route("/api/v1/health", get(health))
        .route("/api/v1/workorders", post(submit_workorder))
        .route("/api/v1/workorders", get(list_workorders))
        .route("/api/v1/workorders/{id}", get(get_workorder))
        .route("/api/v1/workorders/{id}", delete(cancel_workorder))
        .route("/api/v1/workorders/{id}/progress", get(ws_progress))
        .route("/api/v1/snapshots/missing-blobs", post(find_missing_blobs))
        .route("/api/v1/snapshots/blobs/stream", get(ws_blob_upload))
        .route(
            "/api/v1/snapshots/blobs/{digest}",
            get(download_blob).put(upload_blob),
        )
        // Admin / status endpoints.
        .route("/api/v1/clients", get(list_clients))
        .route("/api/v1/clients/{id}", get(get_client));

    let observability_router = Router::new()
        .route("/metrics", get(metrics))
        .nest_service(
            "/binpkgs",
            tower_http::services::ServeDir::new(&state.config.binpkg_dir),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            protect_observability_and_binpkgs,
        ));

    public_router
        .merge(observability_router)
        .layer(DefaultBodyLimit::max(state.config.request_body_size_bytes))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

async fn protect_observability_and_binpkgs(
    State(state): State<SharedState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();
    let requires_auth = match state.auth.mode() {
        remerge_types::auth::AuthMode::None => false,
        remerge_types::auth::AuthMode::Mtls => path == "/metrics" || path.starts_with("/binpkgs"),
        remerge_types::auth::AuthMode::Mixed => path == "/metrics",
    };

    if requires_auth && state.auth.resolve_header_only(request.headers()).is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
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
        signing_key_fingerprint: state
            .signing_key
            .as_ref()
            .map(|key| key.fingerprint.clone()),
        signing_key_endpoint: state
            .signing_key
            .as_ref()
            .map(|_| "/api/v1/signing-key".to_string()),
    })
}

async fn signing_key(State(state): State<SharedState>) -> Result<impl IntoResponse, StatusCode> {
    let signing_key = state.signing_key.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/pgp-keys; charset=utf-8"),
        )],
        signing_key.armored_key.clone(),
    ))
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
    let trace_context = resolve_request_trace_context(&headers);
    let trace_id = trace_context.trace_id.clone();
    let normalized_portage_config = runtime::normalize_portage_config_snapshots(
        state.config.state_dir.as_path(),
        &req.portage_config,
    )
    .await
    .map_err(bad_request)?;

    tracing::info!(
        %client_id,
        %role,
        trace_id,
        auth_method = %identity.method,
        "Request authenticated"
    );

    let id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let (diff, workorder) = {
        let _submission_guard = state.submission_lock.lock().await;

        if state.config.max_active_workorders > 0 {
            let active_workorders = state
                .workorders
                .read()
                .await
                .values()
                .filter(|w| {
                    matches!(
                        w.status,
                        WorkorderStatus::Pending
                            | WorkorderStatus::Provisioning
                            | WorkorderStatus::Building
                    )
                })
                .count();
            if active_workorders >= state.config.max_active_workorders {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "Workorder capacity reached (limit {}). Retry later.",
                        state.config.max_active_workorders
                    ),
                ));
            }
        }

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

        let workorder = Workorder {
            id,
            client_id,
            role,
            atoms: req.atoms,
            emerge_args: req.emerge_args,
            portage_config: normalized_portage_config,
            system_id: req.system_id,
            trace_context: Some(trace_context.clone()),
            status: WorkorderStatus::Pending,
            created_at: now,
            updated_at: now,
        };

        state.create_progress_channel(id).await;
        state.workorders.write().await.insert(id, workorder.clone());
        state.clients.set_active_workorder(&client_id, id).await;

        (diff, workorder)
    };

    info!(
        ?id,
        %client_id,
        %role,
        trace_id,
        atoms = ?workorder.atoms,
        portage_changed = diff.portage_changed,
        system_changed = diff.system_changed,
        "New workorder submitted"
    );

    // Track submission in metrics.
    state
        .metrics
        .workorders_submitted
        .fetch_add(1, Ordering::Relaxed);
    state.metrics.queue_depth.fetch_add(1, Ordering::Relaxed);

    {
        let workorders = state.workorders.read().await;
        if let Err(error) =
            persistence::save_workorders(state.config.state_dir.as_path(), &workorders).await
        {
            warn!(error = ?error, workorder_id = %id, "Failed to persist submitted workorder");
        }
    }
    {
        let clients = state.clients.snapshot().await;
        if let Err(error) =
            persistence::save_clients(state.config.state_dir.as_path(), &clients).await
        {
            warn!(error = ?error, workorder_id = %id, "Failed to persist client registry after submission");
        }
    }

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
        trace_id: Some(trace_context.trace_id),
    }))
}

async fn find_missing_blobs(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<FindMissingBlobsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_snapshot_auth(&state, &headers)?;

    let digest_count = req.digests.len();
    let mut missing_digests = Vec::new();
    for digest in req.digests {
        let present = blob_store::has_blob(state.config.state_dir.as_path(), &digest)
            .await
            .map_err(bad_request)?;
        if !present {
            missing_digests.push(digest);
        }
    }

    state
        .metrics
        .record_snapshot_missing_blob_query(digest_count, missing_digests.len());
    debug!(
        digest_count,
        missing_count = missing_digests.len(),
        "Resolved snapshot missing-blob query"
    );

    Ok(axum::Json(FindMissingBlobsResponse { missing_digests }))
}

async fn upload_blob(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(digest): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_snapshot_auth(&state, &headers)?;

    let raw_size_bytes = body.len() as u64;

    let stored =
        blob_store::store_blob_for_digest(state.config.state_dir.as_path(), &digest, &body)
            .await
            .map_err(bad_request)?;
    state
        .metrics
        .record_snapshot_blob_upload(raw_size_bytes, stored.uploaded);
    debug!(
        digest,
        raw_size_bytes,
        uploaded = stored.uploaded,
        "Handled snapshot blob upload request"
    );

    Ok(axum::Json(UploadBlobResponse {
        digest: stored.blob.digest,
        uploaded: stored.uploaded,
    }))
}

async fn download_blob(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(digest): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_snapshot_auth(&state, &headers)?;

    let state_dir = state.config.state_dir.as_path();
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response_headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));

    if request_accepts_zstd(&headers)
        && let Ok(metadata) = blob_store::load_blob_metadata(state_dir, &digest).await
        && metadata
            .encoded_variants
            .contains_key(&blob_store::BlobEncoding::Zstd)
    {
        let encoded_path =
            blob_store::encoded_blob_path(state_dir, &digest, blob_store::BlobEncoding::Zstd)
                .map_err(bad_request)?;
        match tokio::fs::read(&encoded_path).await {
            Ok(bytes) => {
                response_headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
                state
                    .metrics
                    .record_snapshot_blob_download(bytes.len() as u64);
                debug!(
                    digest,
                    served_bytes = bytes.len(),
                    encoding = "zstd",
                    "Served snapshot blob download"
                );
                return Ok((response_headers, bytes));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to read {}: {error}", encoded_path.display()),
                ));
            }
        }
    }

    let blob_path = blob_store::blob_path(state_dir, &digest).map_err(bad_request)?;
    let bytes = tokio::fs::read(&blob_path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => (
                StatusCode::NOT_FOUND,
                format!("Snapshot blob {digest} not found"),
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read {}: {error}", blob_path.display()),
            ),
        })?;
    state
        .metrics
        .record_snapshot_blob_download(bytes.len() as u64);
    debug!(
        digest,
        served_bytes = bytes.len(),
        encoding = "raw",
        "Served snapshot blob download"
    );

    Ok((response_headers, bytes))
}

async fn ws_blob_upload(
    State(state): State<SharedState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_snapshot_auth(&state, &headers)?;

    let ws_state = state.clone();
    Ok(ws.on_upgrade(move |socket| handle_blob_upload_ws(socket, ws_state)))
}

#[derive(Debug)]
struct BlobUploadSession {
    workorder_id: Uuid,
    digest: String,
    raw_size_bytes: u64,
    stream_size_bytes: u64,
    chunk_size_bytes: u64,
    selected_encoding: Option<String>,
    next_sequence: u64,
    received_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedBlobUploadState {
    digest: String,
    raw_size_bytes: u64,
    stream_size_bytes: u64,
    chunk_size_bytes: u64,
    selected_encoding: Option<String>,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default)]
    next_offset_bytes: u64,
    received_bytes: u64,
}

const BLOB_UPLOAD_SESSION_SUBDIR: &str = "blob-upload-sessions";

async fn handle_blob_upload_ws(socket: ws::WebSocket, state: SharedState) {
    let (mut ws_write, mut ws_read) = socket.split();

    let init_message = match ws_read.next().await {
        Some(Ok(ws::Message::Text(text))) => text,
        Some(Ok(_)) => {
            let _ = send_blob_upload_error(
                &mut ws_write,
                None,
                None,
                "invalid_init_frame",
                "The first blob-stream frame must be an upload_init text message".to_string(),
            )
            .await;
            return;
        }
        Some(Err(error)) => {
            warn!(error = ?error, "Blob upload websocket failed before init");
            return;
        }
        None => return,
    };

    let init = match serde_json::from_str::<SnapshotBlobClientControlMessage>(&init_message) {
        Ok(init) => init,
        Err(error) => {
            let _ = send_blob_upload_error(
                &mut ws_write,
                None,
                None,
                "invalid_init_json",
                format!("Failed to parse upload_init: {error}"),
            )
            .await;
            return;
        }
    };

    let SnapshotBlobClientControlMessage::UploadInit {
        version,
        workorder_id,
        digest,
        total_size_bytes,
        chunk_size_bytes,
        capability_flags,
        offered_encodings,
    } = init;

    debug!(
        %workorder_id,
        digest,
        raw_size_bytes = total_size_bytes,
        requested_chunk_size_bytes = chunk_size_bytes,
        offered_encodings = offered_encodings.len(),
        "Received snapshot blob upload_init"
    );

    if version != SNAPSHOT_BLOB_PROTOCOL_VERSION {
        let _ = send_blob_upload_error(
            &mut ws_write,
            Some(workorder_id),
            Some(digest),
            "unsupported_version",
            format!(
                "Blob stream protocol version {version} is unsupported; expected {}",
                SNAPSHOT_BLOB_PROTOCOL_VERSION
            ),
        )
        .await;
        return;
    }

    if chunk_size_bytes != SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES {
        let _ = send_blob_upload_error(
            &mut ws_write,
            Some(workorder_id),
            Some(digest),
            "unsupported_chunk_size",
            format!(
                "Blob stream chunk size must be {} bytes in v1",
                SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES
            ),
        )
        .await;
        return;
    }

    let digest_for_errors = digest.clone();
    let (selected_encoding, stream_size_bytes) =
        match select_upload_encoding(total_size_bytes, &capability_flags, &offered_encodings) {
            Ok(selection) => selection,
            Err(error) => {
                let _ = send_blob_upload_error(
                    &mut ws_write,
                    Some(workorder_id),
                    Some(digest.clone()),
                    "unsupported_encoding",
                    error.to_string(),
                )
                .await;
                return;
            }
        };
    let blob_present = match blob_store::has_blob(state.config.state_dir.as_path(), &digest).await {
        Ok(present) => present,
        Err(error) => {
            let _ = send_blob_upload_error(
                &mut ws_write,
                Some(workorder_id),
                Some(digest),
                "invalid_digest",
                error.to_string(),
            )
            .await;
            return;
        }
    };

    if blob_present {
        if let Err(error) =
            remove_blob_upload_state(state.config.state_dir.as_path(), &digest).await
        {
            warn!(error = ?error, digest = %digest, "Failed to clean stale blob upload session state");
        }
        state
            .metrics
            .record_snapshot_blob_upload(total_size_bytes, false);
        debug!(
            %workorder_id,
            digest,
            raw_size_bytes = total_size_bytes,
            "Snapshot blob upload hit existing canonical blob"
        );
        let _ = send_blob_upload_control(
            &mut ws_write,
            SnapshotBlobServerControlMessage::UploadComplete {
                version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                workorder_id,
                digest,
                uploaded: false,
            },
        )
        .await;
        let _ = ws_write.send(ws::Message::Close(None)).await;
        return;
    }

    let resumed = match prepare_blob_upload_session(
        state.config.state_dir.as_path(),
        &digest,
        total_size_bytes,
        stream_size_bytes,
        chunk_size_bytes,
        selected_encoding.clone(),
    )
    .await
    {
        Ok(resumed) => resumed,
        Err(error) => {
            let _ = send_blob_upload_error(
                &mut ws_write,
                Some(workorder_id),
                Some(digest.clone()),
                "resume_prepare_failed",
                error.to_string(),
            )
            .await;
            return;
        }
    };

    if resumed.received_bytes == stream_size_bytes {
        let stored =
            match finalize_blob_upload_session(state.config.state_dir.as_path(), &digest).await {
                Ok(stored) => stored,
                Err(error) => {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(workorder_id),
                        Some(digest.clone()),
                        "store_failed",
                        error.to_string(),
                    )
                    .await;
                    return;
                }
            };
        state
            .metrics
            .record_snapshot_blob_upload(total_size_bytes, stored.uploaded);
        debug!(
            %workorder_id,
            digest,
            raw_size_bytes = total_size_bytes,
            stream_size_bytes,
            selected_encoding = selected_encoding.as_deref().unwrap_or("raw"),
            uploaded = stored.uploaded,
            "Completed resumed snapshot blob upload without additional chunks"
        );
        let _ = send_blob_upload_control(
            &mut ws_write,
            SnapshotBlobServerControlMessage::UploadComplete {
                version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                workorder_id,
                digest: stored.blob.digest,
                uploaded: stored.uploaded,
            },
        )
        .await;
        let _ = ws_write.send(ws::Message::Close(None)).await;
        return;
    }

    if send_blob_upload_control(
        &mut ws_write,
        SnapshotBlobServerControlMessage::UploadResume {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id,
            digest: digest.clone(),
            next_offset_bytes: resumed.next_offset_bytes,
            next_sequence: resumed.next_sequence,
            selected_encoding: resumed.selected_encoding.clone(),
            expected_size_bytes: resumed.stream_size_bytes,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    debug!(
        %workorder_id,
        digest,
        next_offset_bytes = resumed.next_offset_bytes,
        next_sequence = resumed.next_sequence,
        expected_size_bytes = resumed.stream_size_bytes,
        selected_encoding = resumed.selected_encoding.as_deref().unwrap_or("raw"),
        "Prepared snapshot blob upload resume state"
    );

    let mut session = BlobUploadSession {
        workorder_id,
        digest,
        raw_size_bytes: total_size_bytes,
        stream_size_bytes,
        chunk_size_bytes,
        selected_encoding: selected_encoding.clone(),
        next_sequence: resumed.next_sequence,
        received_bytes: resumed.next_offset_bytes,
    };

    if session.stream_size_bytes == 0 {
        let stored =
            match finalize_blob_upload_session(state.config.state_dir.as_path(), &session.digest)
                .await
            {
                Ok(stored) => stored,
                Err(error) => {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(session.workorder_id),
                        Some(digest_for_errors),
                        "store_failed",
                        error.to_string(),
                    )
                    .await;
                    return;
                }
            };
        state
            .metrics
            .record_snapshot_blob_upload(session.raw_size_bytes, stored.uploaded);
        debug!(
            %session.workorder_id,
            digest = %session.digest,
            raw_size_bytes = session.raw_size_bytes,
            stream_size_bytes = session.stream_size_bytes,
            selected_encoding = session.selected_encoding.as_deref().unwrap_or("raw"),
            uploaded = stored.uploaded,
            "Completed zero-length snapshot blob upload"
        );

        let _ = send_blob_upload_control(
            &mut ws_write,
            SnapshotBlobServerControlMessage::UploadComplete {
                version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                workorder_id: session.workorder_id,
                digest: stored.blob.digest,
                uploaded: stored.uploaded,
            },
        )
        .await;
        let _ = ws_write.send(ws::Message::Close(None)).await;
        return;
    }

    while let Some(message) = ws_read.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                warn!(error = ?error, digest = %session.digest, "Blob upload websocket read failed");
                return;
            }
        };

        match message {
            ws::Message::Binary(frame) => {
                let (header, payload) = match SnapshotBlobChunkHeader::decode(&frame) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let _ = send_blob_upload_error(
                            &mut ws_write,
                            Some(session.workorder_id),
                            Some(session.digest.clone()),
                            "invalid_chunk_frame",
                            error.to_string(),
                        )
                        .await;
                        return;
                    }
                };

                if header.sequence != session.next_sequence {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(session.workorder_id),
                        Some(session.digest.clone()),
                        "unexpected_sequence",
                        format!(
                            "Expected chunk sequence {}, got {}",
                            session.next_sequence, header.sequence
                        ),
                    )
                    .await;
                    return;
                }

                if header.offset_bytes != session.received_bytes {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(session.workorder_id),
                        Some(session.digest.clone()),
                        "unexpected_offset",
                        format!(
                            "Expected chunk offset {}, got {}",
                            session.received_bytes, header.offset_bytes
                        ),
                    )
                    .await;
                    return;
                }

                if header.payload_size_bytes > session.chunk_size_bytes {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(session.workorder_id),
                        Some(session.digest.clone()),
                        "chunk_too_large",
                        format!(
                            "Chunk size {} exceeds the negotiated {} bytes",
                            header.payload_size_bytes, session.chunk_size_bytes
                        ),
                    )
                    .await;
                    return;
                }

                let next_received = session
                    .received_bytes
                    .saturating_add(header.payload_size_bytes);
                if next_received > session.stream_size_bytes {
                    let _ = send_blob_upload_error(
                        &mut ws_write,
                        Some(session.workorder_id),
                        Some(session.digest.clone()),
                        "blob_too_large",
                        format!(
                            "Received {} bytes, which exceeds the declared blob length {}",
                            next_received, session.stream_size_bytes
                        ),
                    )
                    .await;
                    return;
                }

                let persisted = match append_blob_upload_chunk(
                    state.config.state_dir.as_path(),
                    &session.digest,
                    session.raw_size_bytes,
                    session.stream_size_bytes,
                    session.chunk_size_bytes,
                    session.selected_encoding.clone(),
                    payload,
                )
                .await
                {
                    Ok(persisted) => persisted,
                    Err(error) => {
                        let _ = send_blob_upload_error(
                            &mut ws_write,
                            Some(session.workorder_id),
                            Some(session.digest.clone()),
                            "persist_failed",
                            error.to_string(),
                        )
                        .await;
                        return;
                    }
                };

                session.received_bytes = persisted.received_bytes;
                session.next_sequence = persisted.next_sequence;

                if send_blob_upload_control(
                    &mut ws_write,
                    SnapshotBlobServerControlMessage::UploadAck {
                        version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                        workorder_id: session.workorder_id,
                        digest: session.digest.clone(),
                        sequence: header.sequence,
                        offset_bytes: header.offset_bytes,
                        size_bytes: header.payload_size_bytes,
                        received_bytes: session.received_bytes,
                    },
                )
                .await
                .is_err()
                {
                    return;
                }

                if session.received_bytes == session.stream_size_bytes {
                    let stored = match finalize_blob_upload_session(
                        state.config.state_dir.as_path(),
                        &session.digest,
                    )
                    .await
                    {
                        Ok(stored) => stored,
                        Err(error) => {
                            let _ = send_blob_upload_error(
                                &mut ws_write,
                                Some(session.workorder_id),
                                Some(session.digest.clone()),
                                "store_failed",
                                error.to_string(),
                            )
                            .await;
                            return;
                        }
                    };
                    state
                        .metrics
                        .record_snapshot_blob_upload(session.raw_size_bytes, stored.uploaded);
                    debug!(
                        %session.workorder_id,
                        digest = %session.digest,
                        raw_size_bytes = session.raw_size_bytes,
                        stream_size_bytes = session.stream_size_bytes,
                        selected_encoding = session.selected_encoding.as_deref().unwrap_or("raw"),
                        chunk_count = session.next_sequence,
                        uploaded = stored.uploaded,
                        "Completed snapshot blob websocket upload"
                    );

                    let _ = send_blob_upload_control(
                        &mut ws_write,
                        SnapshotBlobServerControlMessage::UploadComplete {
                            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                            workorder_id: session.workorder_id,
                            digest: stored.blob.digest,
                            uploaded: stored.uploaded,
                        },
                    )
                    .await;
                    let _ = ws_write.send(ws::Message::Close(None)).await;
                    return;
                }
            }
            ws::Message::Text(_) => {
                let _ = send_blob_upload_error(
                    &mut ws_write,
                    Some(session.workorder_id),
                    Some(session.digest.clone()),
                    "unexpected_control_frame",
                    "Chunk payloads must be sent as binary frames after upload_init".to_string(),
                )
                .await;
                return;
            }
            ws::Message::Close(_) => return,
            ws::Message::Ping(_) | ws::Message::Pong(_) => continue,
        }
    }
}

async fn send_blob_upload_control(
    ws_write: &mut futures::stream::SplitSink<ws::WebSocket, ws::Message>,
    message: SnapshotBlobServerControlMessage,
) -> Result<(), ()> {
    let text = serde_json::to_string(&message).map_err(|error| {
        warn!(error = ?error, "Failed to serialize blob upload control frame");
    })?;
    ws_write
        .send(ws::Message::Text(text.into()))
        .await
        .map_err(|error| {
            warn!(error = ?error, "Failed to send blob upload control frame");
        })
}

async fn send_blob_upload_error(
    ws_write: &mut futures::stream::SplitSink<ws::WebSocket, ws::Message>,
    workorder_id: Option<Uuid>,
    digest: Option<String>,
    code: &str,
    message: String,
) -> Result<(), ()> {
    warn!(
        ?workorder_id,
        ?digest,
        code,
        error_message = %message,
        "Rejecting snapshot blob websocket upload"
    );
    let result = send_blob_upload_control(
        ws_write,
        SnapshotBlobServerControlMessage::UploadError {
            version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
            workorder_id,
            digest,
            code: code.to_string(),
            message,
        },
    )
    .await;
    let _ = ws_write.send(ws::Message::Close(None)).await;
    result
}

fn blob_upload_session_dir(state_dir: &FsPath) -> PathBuf {
    state_dir.join(BLOB_UPLOAD_SESSION_SUBDIR)
}

fn blob_upload_session_meta_path(state_dir: &FsPath, digest: &str) -> PathBuf {
    blob_upload_session_dir(state_dir).join(format!("{digest}.json"))
}

fn blob_upload_session_part_path(state_dir: &FsPath, digest: &str) -> PathBuf {
    blob_upload_session_dir(state_dir).join(format!("{digest}.part"))
}

async fn load_blob_upload_state(
    state_dir: &FsPath,
    digest: &str,
) -> anyhow::Result<Option<PersistedBlobUploadState>> {
    let meta_path = blob_upload_session_meta_path(state_dir, digest);
    match tokio::fs::read(&meta_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(anyhow::Error::from)
            .map_err(|error| error.context(format!("Failed to parse {}", meta_path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(anyhow::Error::from(error))
            .map_err(|error| error.context(format!("Failed to read {}", meta_path.display()))),
    }
}

async fn persist_blob_upload_state(
    state_dir: &FsPath,
    state: &PersistedBlobUploadState,
) -> anyhow::Result<()> {
    let session_dir = blob_upload_session_dir(state_dir);
    tokio::fs::create_dir_all(&session_dir)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to create {}", session_dir.display())))?;

    let meta_path = blob_upload_session_meta_path(state_dir, &state.digest);
    let temp_path = session_dir.join(format!("{}.json.tmp-{}", state.digest, Uuid::new_v4()));
    let body = serde_json::to_vec(state).map_err(anyhow::Error::from)?;
    tokio::fs::write(&temp_path, body)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to write {}", temp_path.display())))?;
    tokio::fs::rename(&temp_path, &meta_path)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to persist {}", meta_path.display())))?;
    Ok(())
}

async fn remove_blob_upload_state(state_dir: &FsPath, digest: &str) -> anyhow::Result<()> {
    for path in [
        blob_upload_session_meta_path(state_dir, digest),
        blob_upload_session_part_path(state_dir, digest),
    ] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow::Error::from(error)).map_err(|error| {
                    error.context(format!("Failed to remove {}", path.display()))
                });
            }
        }
    }
    Ok(())
}

async fn prepare_blob_upload_session(
    state_dir: &FsPath,
    digest: &str,
    raw_size_bytes: u64,
    stream_size_bytes: u64,
    chunk_size_bytes: u64,
    selected_encoding: Option<String>,
) -> anyhow::Result<PersistedBlobUploadState> {
    let session_dir = blob_upload_session_dir(state_dir);
    tokio::fs::create_dir_all(&session_dir)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to create {}", session_dir.display())))?;

    let part_path = blob_upload_session_part_path(state_dir, digest);
    let mut state =
        load_blob_upload_state(state_dir, digest)
            .await?
            .unwrap_or(PersistedBlobUploadState {
                digest: digest.to_string(),
                raw_size_bytes,
                stream_size_bytes,
                chunk_size_bytes,
                selected_encoding: selected_encoding.clone(),
                next_sequence: 0,
                next_offset_bytes: 0,
                received_bytes: 0,
            });

    if state.raw_size_bytes != raw_size_bytes {
        anyhow::bail!(
            "Upload session for {digest} expects raw size {}, got {}",
            state.raw_size_bytes,
            raw_size_bytes
        );
    }
    if state.stream_size_bytes != stream_size_bytes {
        anyhow::bail!(
            "Upload session for {digest} expects stream size {}, got {}",
            state.stream_size_bytes,
            stream_size_bytes
        );
    }
    if state.chunk_size_bytes != chunk_size_bytes {
        anyhow::bail!(
            "Upload session for {digest} expects chunk size {}, got {}",
            state.chunk_size_bytes,
            chunk_size_bytes
        );
    }
    if state.selected_encoding != selected_encoding {
        anyhow::bail!(
            "Upload session for {digest} expects encoding {:?}, got {:?}",
            state.selected_encoding,
            selected_encoding
        );
    }

    let received_bytes = match tokio::fs::metadata(&part_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(anyhow::Error::from(error))
                .map_err(|error| error.context(format!("Failed to stat {}", part_path.display())));
        }
    };
    if received_bytes > stream_size_bytes {
        remove_blob_upload_state(state_dir, digest).await?;
        anyhow::bail!(
            "Upload session for {digest} exceeds declared size: {} > {}",
            received_bytes,
            stream_size_bytes
        );
    }
    if state.received_bytes != received_bytes || state.next_offset_bytes != received_bytes {
        if received_bytes == 0 {
            state.received_bytes = 0;
            state.next_offset_bytes = 0;
            state.next_sequence = 0;
        } else {
            anyhow::bail!(
                "Upload session for {digest} is inconsistent: metadata records offset {} / {} but staged bytes total {}",
                state.received_bytes,
                state.next_offset_bytes,
                received_bytes
            );
        }
    }
    if received_bytes > 0 && state.next_sequence == 0 {
        anyhow::bail!(
            "Upload session for {digest} is missing exact resume sequence metadata at offset {}",
            received_bytes
        );
    }

    state.received_bytes = received_bytes;
    state.next_offset_bytes = received_bytes;
    persist_blob_upload_state(state_dir, &state).await?;
    Ok(state)
}

async fn append_blob_upload_chunk(
    state_dir: &FsPath,
    digest: &str,
    raw_size_bytes: u64,
    stream_size_bytes: u64,
    chunk_size_bytes: u64,
    selected_encoding: Option<String>,
    payload: &[u8],
) -> anyhow::Result<PersistedBlobUploadState> {
    let mut state = prepare_blob_upload_session(
        state_dir,
        digest,
        raw_size_bytes,
        stream_size_bytes,
        chunk_size_bytes,
        selected_encoding,
    )
    .await?;
    let next_received = state.received_bytes + payload.len() as u64;
    if next_received > stream_size_bytes {
        anyhow::bail!(
            "Upload chunk would exceed declared size for {digest}: {} > {}",
            next_received,
            stream_size_bytes
        );
    }
    if payload.is_empty() && next_received < stream_size_bytes {
        anyhow::bail!("Non-final chunk for {digest} must not be empty");
    }

    let part_path = blob_upload_session_part_path(state_dir, digest);
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to open {}", part_path.display())))?;
    file.write_all(payload)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to append to {}", part_path.display())))?;
    file.flush()
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to flush {}", part_path.display())))?;

    state.received_bytes = next_received;
    state.next_offset_bytes = next_received;
    state.next_sequence = state.next_sequence.saturating_add(1);
    persist_blob_upload_state(state_dir, &state).await?;
    Ok(state)
}

async fn finalize_blob_upload_session(
    state_dir: &FsPath,
    digest: &str,
) -> anyhow::Result<blob_store::VerifiedBlobStoreResult> {
    let mut state = load_blob_upload_state(state_dir, digest)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No upload session found for {digest}"))?;
    let part_path = blob_upload_session_part_path(state_dir, digest);
    let received_bytes = match tokio::fs::metadata(&part_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(anyhow::Error::from(error))
                .map_err(|error| error.context(format!("Failed to stat {}", part_path.display())));
        }
    };
    state.received_bytes = received_bytes;
    state.next_offset_bytes = received_bytes;
    if state.received_bytes != state.stream_size_bytes {
        anyhow::bail!(
            "Upload session for {digest} is incomplete: {} of {} bytes received",
            state.received_bytes,
            state.stream_size_bytes
        );
    }

    let stream_bytes = tokio::fs::read(&part_path)
        .await
        .map_err(anyhow::Error::from)
        .map_err(|error| error.context(format!("Failed to read {}", part_path.display())))?;
    let raw_bytes = match state.selected_encoding.as_deref() {
        None => stream_bytes.clone(),
        Some(SNAPSHOT_BLOB_ENCODING_ZSTD) => {
            let encoded = stream_bytes.clone();
            tokio::task::spawn_blocking(move || compression::decode_zstd(&encoded))
                .await
                .map_err(anyhow::Error::from)
                .map_err(|error| error.context("zstd decompression task failed to join"))??
        }
        Some(other) => anyhow::bail!("unsupported upload encoding '{other}'"),
    };
    if raw_bytes.len() as u64 != state.raw_size_bytes {
        anyhow::bail!(
            "Upload session for {digest} decoded to {} raw bytes, expected {}",
            raw_bytes.len(),
            state.raw_size_bytes
        );
    }

    let stored = blob_store::store_blob_for_digest(state_dir, digest, &raw_bytes).await?;
    if state.selected_encoding.as_deref() == Some(SNAPSHOT_BLOB_ENCODING_ZSTD) {
        blob_store::store_encoded_blob_variant(
            state_dir,
            digest,
            state.raw_size_bytes,
            blob_store::BlobEncoding::Zstd,
            &stream_bytes,
        )
        .await?;
    }
    remove_blob_upload_state(state_dir, digest).await?;
    Ok(stored)
}

fn select_upload_encoding(
    raw_size_bytes: u64,
    capability_flags: &[String],
    offered_encodings: &[SnapshotBlobEncodingOffer],
) -> anyhow::Result<(Option<String>, u64)> {
    if capability_flags
        .iter()
        .any(|flag| flag == SNAPSHOT_BLOB_ENCODING_ZSTD)
        && let Some(offer) = offered_encodings
            .iter()
            .find(|offer| offer.encoding == SNAPSHOT_BLOB_ENCODING_ZSTD)
    {
        return Ok((Some(offer.encoding.clone()), offer.size_bytes));
    }

    if let Some(offer) = offered_encodings
        .iter()
        .find(|offer| offer.encoding != SNAPSHOT_BLOB_ENCODING_ZSTD)
    {
        anyhow::bail!("unsupported offered upload encoding '{}'", offer.encoding);
    }

    Ok((None, raw_size_bytes))
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
        trace_id: workorder
            .trace_context
            .as_ref()
            .map(|ctx| ctx.trace_id.clone()),
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
        let was_pending = matches!(old_status, WorkorderStatus::Pending);
        let trace_id = workorder
            .trace_context
            .as_ref()
            .map(|ctx| ctx.trace_id.clone());
        workorder.status = WorkorderStatus::Cancelled;
        let client_id = workorder.client_id;
        drop(workorders); // Release the write lock before async calls.

        // Broadcast StatusChanged event so WebSocket clients see the cancellation.
        if let Some(tx) = state.progress_txs.read().await.get(&id) {
            let _ = tx.send(BuildProgress {
                workorder_id: id,
                trace_id,
                event: BuildEvent::StatusChanged {
                    from: old_status,
                    to: WorkorderStatus::Cancelled,
                },
                timestamp: chrono::Utc::now(),
            });
        }

        state.clients.clear_active_workorder(&client_id).await;

        if was_pending {
            state.metrics.queue_depth.fetch_sub(1, Ordering::Relaxed);
        }

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

fn resolve_request_trace_context(headers: &HeaderMap) -> remerge_types::trace::TraceContext {
    let header_value = headers
        .get(TRACEPARENT_HEADER)
        .and_then(|value| value.to_str().ok());

    if let Some(traceparent) = header_value {
        if let Some(trace_context) = remerge_observability::parse_trace_context(traceparent) {
            return trace_context;
        }

        warn!(
            traceparent,
            "Ignoring invalid traceparent header on workorder submission"
        );
    }

    remerge_observability::new_trace_context()
}

fn require_snapshot_auth(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    if state.auth.mode() != remerge_types::auth::AuthMode::None {
        state.auth.resolve_header_only(headers).ok_or((
            StatusCode::UNAUTHORIZED,
            "Authentication required".to_string(),
        ))?;
    }
    Ok(())
}

fn bad_request(error: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.to_string())
}

fn request_accepts_zstd(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .filter_map(|token| token.split(';').next())
                .any(|token| token.trim().eq_ignore_ascii_case("zstd"))
        })
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

    // Raw PTY output channel — may be absent if the workorder has already
    // finished streaming (the sender was removed before this connection
    // arrived).  In that case the WS handler starts in text-only mode.
    let raw_rx = state.subscribe_raw_output(&id).await;

    let ws_state = state.clone();

    Ok(ws.on_upgrade(move |socket| handle_ws(socket, rx, raw_rx, ws_state, id)))
}

async fn handle_ws(
    socket: ws::WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<BuildProgress>,
    raw_rx: Option<tokio::sync::broadcast::Receiver<bytes::Bytes>>,
    state: SharedState,
    workorder_id: uuid::Uuid,
) {
    let (mut ws_write, mut ws_read) = socket.split();

    // Task 1: Forward raw PTY output as Binary frames and structured
    // build events as Text frames.  Binary frames carry the lossless
    // terminal byte stream; Text frames carry only status / result events
    // (StatusChanged, PackageBuilt, PackageFailed, Finished).
    let mut send_task = tokio::spawn(async move {
        use futures::SinkExt;
        use tokio::sync::broadcast::error::RecvError;

        // Start in text-only mode if the raw channel is already gone
        // (workorder finished streaming before this WS connection arrived).
        let mut raw_done = raw_rx.is_none();
        let mut raw_rx = raw_rx;

        loop {
            if raw_done {
                // Raw channel closed — only wait for structured events.
                match rx.recv().await {
                    Ok(progress) => {
                        // Log events are superseded by the raw channel.
                        if matches!(progress.event, BuildEvent::Log { .. }) {
                            continue;
                        }
                        match serde_json::to_string(&progress) {
                            Ok(text) => {
                                if ws_write.send(ws::Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(error = ?e, "Failed to serialize build progress to JSON; dropping message");
                            }
                        }
                        if matches!(progress.event, BuildEvent::Finished { .. }) {
                            let _ = ws_write.send(ws::Message::Close(None)).await;
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!(skipped = n, "Progress receiver lagged");
                    }
                    Err(RecvError::Closed) => {
                        // Channel closed without a Finished event — send a
                        // Close frame so the client doesn't hang.
                        let _ = ws_write.send(ws::Message::Close(None)).await;
                        break;
                    }
                }
            } else {
                // raw_done is false only when raw_rx is Some — unwrap is safe.
                let raw_recv = raw_rx.as_mut().unwrap();
                tokio::select! {
                    biased; // prefer raw output — highest throughput path
                    result = raw_recv.recv() => {
                        match result {
                            Ok(bytes) => {
                                if ws_write.send(ws::Message::Binary(bytes)).await.is_err() {
                                    break;
                                }
                            }
                            Err(RecvError::Lagged(n)) => {
                                warn!(skipped = n, "Raw output receiver lagged");
                            }
                            Err(RecvError::Closed) => {
                                raw_done = true;
                            }
                        }
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(progress) => {
                                if matches!(progress.event, BuildEvent::Log { .. }) {
                                    continue;
                                }
                                match serde_json::to_string(&progress) {
                                    Ok(text) => {
                                        if ws_write
                                            .send(ws::Message::Text(text.into()))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = ?e, "Failed to serialize build progress to JSON; dropping message");
                                    }
                                }
                                if matches!(progress.event, BuildEvent::Finished { .. }) {
                                    let _ = ws_write.send(ws::Message::Close(None)).await;
                                    break;
                                }
                            }
                            Err(RecvError::Lagged(n)) => {
                                warn!(skipped = n, "Progress receiver lagged");
                            }
                            Err(RecvError::Closed) => {
                                // Channel closed without a Finished event —
                                // send a Close frame so the client exits.
                                let _ = ws_write.send(ws::Message::Close(None)).await;
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    // Task 2: Read client messages (stdin data) and forward to the container.
    //
    // The stdin channel is created by the queue processor when it attaches
    // to the container, which may happen *after* the WebSocket connects.
    // We look up the sender dynamically on each message to handle this race.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_read.next().await {
            let data = match msg {
                ws::Message::Text(text) => text.as_bytes().to_vec(),
                ws::Message::Binary(data) => data.to_vec(),
                ws::Message::Close(_) => break,
                _ => continue,
            };

            // Retry lookup — the channel may not exist yet during provisioning.
            let mut sent = false;
            for _ in 0..50 {
                if let Some(tx) = state.get_stdin_tx(&workorder_id).await {
                    let _ = tx.send(data).await;
                    sent = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !sent {
                warn!(id = ?workorder_id, "Dropped stdin data — no stdin channel available");
            }
        }
    });

    // Wait for either task to finish, then abort the other so it
    // doesn't linger and hold the WebSocket connection open.
    tokio::select! {
        _ = &mut send_task => { recv_task.abort(); },
        _ = &mut recv_task => { send_task.abort(); },
    }
}
