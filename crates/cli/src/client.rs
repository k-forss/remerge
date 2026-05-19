//! HTTP + WebSocket client for communicating with the remerge server.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use remerge_types::trace::TRACEPARENT_HEADER;
use remerge_types::{
    api::{
        FindMissingBlobsRequest, FindMissingBlobsResponse, LogEvent, LogLevel,
        SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES, SNAPSHOT_BLOB_ENCODING_ZSTD,
        SNAPSHOT_BLOB_PROTOCOL_VERSION, SnapshotBlobChunkHeader, SnapshotBlobClientControlMessage,
        SnapshotBlobEncodingOffer, SnapshotBlobServerControlMessage, SubmitWorkorderRequest,
        SubmitWorkorderResponse, UploadBlobResponse,
    },
    client::{ClientId, ClientRole},
    compression,
    portage::{PortageConfig, SystemIdentity},
    workorder::{BuildEvent, BuildProgress, WorkorderResult},
};
use tracing::{debug, info, warn};

const SNAPSHOT_BLOB_STREAM_MAX_CONNECTION_ATTEMPTS: usize = 4;
const SNAPSHOT_BLOB_STREAM_RECONNECT_DELAY: Duration = Duration::from_millis(200);
const SNAPSHOT_BLOB_STREAM_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const SNAPSHOT_BLOB_STREAM_SLOW_ACK_THRESHOLD: Duration = Duration::from_millis(150);
const SNAPSHOT_BLOB_STREAM_MIN_CHUNK_SIZE_BYTES: u64 = 256 * 1024;
const SNAPSHOT_BLOB_STREAM_GROW_AFTER_HEALTHY_ACKS: usize = 2;
const FILE_DOWNLOAD_STALL_THRESHOLD: Duration = Duration::from_secs(2);
const WORKORDER_RESULT_POLL_INTERVAL: Duration = Duration::from_millis(200);
const WORKORDER_RESULT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const PROGRESS_STREAM_CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct SnapshotBlobChunkPolicy {
    current_chunk_size_bytes: u64,
    max_chunk_size_bytes: u64,
    min_chunk_size_bytes: u64,
    healthy_ack_streak: usize,
}

impl SnapshotBlobChunkPolicy {
    fn new(max_chunk_size_bytes: u64) -> Self {
        Self {
            current_chunk_size_bytes: max_chunk_size_bytes,
            max_chunk_size_bytes,
            min_chunk_size_bytes: SNAPSHOT_BLOB_STREAM_MIN_CHUNK_SIZE_BYTES
                .min(max_chunk_size_bytes.max(1)),
            healthy_ack_streak: 0,
        }
    }

    fn current_chunk_size_bytes(&self) -> u64 {
        self.current_chunk_size_bytes
    }

    fn chunk_size_for_remaining(&self, remaining_bytes: u64) -> usize {
        self.current_chunk_size_bytes.min(remaining_bytes.max(1)) as usize
    }

    fn on_ack(&mut self, ack_latency: Duration) {
        if ack_latency >= SNAPSHOT_BLOB_STREAM_SLOW_ACK_THRESHOLD {
            self.shrink();
            self.healthy_ack_streak = 0;
            return;
        }

        self.healthy_ack_streak += 1;
        if self.healthy_ack_streak >= SNAPSHOT_BLOB_STREAM_GROW_AFTER_HEALTHY_ACKS {
            self.grow();
            self.healthy_ack_streak = 0;
        }
    }

    fn on_reconnect(&mut self) {
        self.shrink();
        self.healthy_ack_streak = 0;
    }

    fn shrink(&mut self) {
        self.current_chunk_size_bytes =
            (self.current_chunk_size_bytes / 2).max(self.min_chunk_size_bytes);
    }

    fn grow(&mut self) {
        self.current_chunk_size_bytes = (self.current_chunk_size_bytes * 2)
            .min(self.max_chunk_size_bytes)
            .max(self.min_chunk_size_bytes);
    }
}

#[derive(Debug, Clone)]
struct SnapshotBlobUploadPayload {
    raw_bytes: Vec<u8>,
    zstd_bytes: Option<Vec<u8>>,
    offered_encodings: Vec<SnapshotBlobEncodingOffer>,
}

/// Client for the remerge server.
pub struct RemergeClient {
    base_url: String,
    http: reqwest::Client,
}

impl RemergeClient {
    /// Create a new client pointing at the given server URL.
    pub fn new(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("remerge/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    /// Submit a workorder to the server.
    pub async fn submit_workorder(
        &self,
        client_id: ClientId,
        role: ClientRole,
        atoms: &[String],
        emerge_args: &[String],
        portage_config: &PortageConfig,
        system_id: &SystemIdentity,
    ) -> Result<SubmitWorkorderResponse> {
        let req = SubmitWorkorderRequest {
            client_id,
            role,
            atoms: atoms.to_vec(),
            emerge_args: emerge_args.to_vec(),
            portage_config: portage_config.clone(),
            system_id: system_id.clone(),
        };
        let trace_context = remerge_observability::new_trace_context();

        debug!(
            trace_id = %trace_context.trace_id,
            %client_id,
            atom_count = atoms.len(),
            "Submitting workorder with distributed trace context"
        );

        let resp = self
            .http
            .post(format!("{}/api/v1/workorders", self.base_url))
            .header(TRACEPARENT_HEADER, &trace_context.traceparent)
            .json(&req)
            .send()
            .await
            .context("Failed to send workorder")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Server returned {status}: {body}");
        }

        resp.json::<SubmitWorkorderResponse>()
            .await
            .context("Failed to parse workorder response")
    }

    /// Ask the server which blob digests are missing from its shared snapshot store.
    pub async fn find_missing_blobs(&self, digests: &[String]) -> Result<Vec<String>> {
        let resp = self
            .http
            .post(format!("{}/api/v1/snapshots/missing-blobs", self.base_url))
            .json(&FindMissingBlobsRequest {
                digests: digests.to_vec(),
            })
            .send()
            .await
            .context("Failed to query missing snapshot blobs")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Server returned {status} while querying missing blobs: {body}");
        }

        let body = resp
            .json::<FindMissingBlobsResponse>()
            .await
            .context("Failed to parse missing-blobs response")?;
        debug!(
            requested_digests = digests.len(),
            missing_digests = body.missing_digests.len(),
            "Resolved snapshot missing-blob query"
        );
        Ok(body.missing_digests)
    }

    /// Upload a single verified snapshot blob by digest.
    pub async fn upload_blob(&self, digest: &str, bytes: &[u8]) -> Result<bool> {
        let resp = self
            .http
            .put(format!("{}/api/v1/snapshots/blobs/{digest}", self.base_url))
            .body(bytes.to_vec())
            .send()
            .await
            .with_context(|| format!("Failed to upload snapshot blob {digest}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Server returned {status} while uploading blob {digest}: {body}");
        }

        let body = resp
            .json::<UploadBlobResponse>()
            .await
            .context("Failed to parse upload-blob response")?;
        debug!(
            digest,
            raw_size_bytes = bytes.len(),
            uploaded = body.uploaded,
            "Completed HTTP snapshot blob upload"
        );
        Ok(body.uploaded)
    }

    /// Download a stored snapshot blob by digest into a local destination.
    pub async fn download_blob(&self, digest: &str, destination: &Path) -> Result<()> {
        self.download_file(
            &format!("{}/api/v1/snapshots/blobs/{digest}", self.base_url),
            destination,
        )
        .await
    }

    /// Upload a snapshot blob through the hybrid websocket chunk-streaming transport.
    pub async fn stream_upload_blob(&self, digest: &str, bytes: &[u8]) -> Result<bool> {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message;

        let workorder_id = uuid::Uuid::new_v4();
        let ws_url = self.snapshot_blob_stream_url();
        let attempts = SNAPSHOT_BLOB_STREAM_MAX_CONNECTION_ATTEMPTS.max(1);
        let mut last_error: Option<anyhow::Error> = None;
        let mut chunk_policy = SnapshotBlobChunkPolicy::new(SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES);
        let upload_payload = Self::prepare_upload_payload(bytes).await?;
        debug!(
            digest,
            raw_size_bytes = bytes.len(),
            transport_encoding = if upload_payload.zstd_bytes.is_some() {
                "zstd_optional"
            } else {
                "raw_only"
            },
            attempts,
            "Starting websocket snapshot blob upload"
        );

        for attempt in 1..=attempts {
            let upload_result = async {
                let (mut ws, _) = connect_async(&ws_url)
                    .await
                    .with_context(|| format!("Failed to connect to snapshot blob stream {ws_url}"))?;

                let init = SnapshotBlobClientControlMessage::UploadInit {
                    version: SNAPSHOT_BLOB_PROTOCOL_VERSION,
                    workorder_id,
                    digest: digest.to_string(),
                    total_size_bytes: bytes.len() as u64,
                    chunk_size_bytes: SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
                    capability_flags: upload_payload
                        .offered_encodings
                        .iter()
                        .map(|offer| offer.encoding.clone())
                        .collect(),
                    offered_encodings: upload_payload.offered_encodings.clone(),
                };
                ws.send(Message::Text(
                    serde_json::to_string(&init)
                        .context("Failed to serialize upload_init")?
                        .into(),
                ))
                .await
                .with_context(|| format!("Failed to send upload_init for blob {digest}"))?;

                let resume = Self::recv_blob_upload_control_with_timeout(
                    &mut ws,
                    digest,
                    SNAPSHOT_BLOB_STREAM_CONTROL_TIMEOUT,
                )
                .await?;
                let (selected_bytes, mut next_offset, mut next_sequence) = match resume {
                    SnapshotBlobServerControlMessage::UploadResume {
                        version,
                        workorder_id: response_workorder_id,
                        digest: response_digest,
                        next_offset_bytes,
                        next_sequence,
                        selected_encoding,
                        expected_size_bytes,
                    } => {
                        Self::validate_blob_upload_envelope(
                            version,
                            workorder_id,
                            digest,
                            &response_digest,
                            Some(response_workorder_id),
                        )?;
                        let selected_bytes =
                            Self::payload_for_selected_encoding(&upload_payload, selected_encoding.as_deref())?;
                        if expected_size_bytes != selected_bytes.len() as u64 {
                            anyhow::bail!(
                                "Blob stream server selected an unexpected payload length for {digest}: expected {}, got {expected_size_bytes}",
                                selected_bytes.len()
                            );
                        }
                        debug!(
                            digest,
                            attempt,
                            next_offset_bytes,
                            next_sequence,
                            selected_encoding = selected_encoding.as_deref().unwrap_or("raw"),
                            expected_size_bytes,
                            "Received snapshot blob upload resume state"
                        );
                        (selected_bytes, next_offset_bytes, next_sequence)
                    }
                    SnapshotBlobServerControlMessage::UploadError { code, message, .. } => {
                        anyhow::bail!("Blob stream upload failed during init ({code}): {message}");
                    }
                    SnapshotBlobServerControlMessage::UploadComplete {
                        version,
                        workorder_id: response_workorder_id,
                        digest: response_digest,
                        uploaded,
                    } => {
                        Self::validate_blob_upload_envelope(
                            version,
                            workorder_id,
                            digest,
                            &response_digest,
                            Some(response_workorder_id),
                        )?;
                        debug!(
                            digest,
                            attempt,
                            uploaded,
                            "Snapshot blob upload short-circuited at init"
                        );
                        return Ok(uploaded);
                    }
                    other => {
                        anyhow::bail!(
                            "Unexpected blob upload control frame before streaming: {other:?}"
                        );
                    }
                };

                while next_offset < selected_bytes.len() as u64 {
                    let chunk_size_bytes =
                        chunk_policy.chunk_size_for_remaining(selected_bytes.len() as u64 - next_offset);
                    let chunk_end = next_offset as usize + chunk_size_bytes;
                    let chunk = &selected_bytes[next_offset as usize..chunk_end];
                    let header =
                        SnapshotBlobChunkHeader::from_payload(next_sequence, next_offset, chunk);
                    let frame = header
                        .encode_with_payload(chunk)
                        .context("Failed to encode snapshot chunk frame")?;
                    let send_started = std::time::Instant::now();
                    ws.send(Message::Binary(frame.into())).await.with_context(|| {
                        format!("Failed to send blob chunk {next_sequence} for {digest}")
                    })?;

                    let ack = Self::recv_blob_upload_control_with_timeout(
                        &mut ws,
                        digest,
                        SNAPSHOT_BLOB_STREAM_CONTROL_TIMEOUT,
                    )
                    .await?;
                    match ack {
                        SnapshotBlobServerControlMessage::UploadAck {
                            version,
                            workorder_id: response_workorder_id,
                            digest: response_digest,
                            sequence,
                            offset_bytes,
                            size_bytes,
                            received_bytes,
                        } => {
                            Self::validate_blob_upload_envelope(
                                version,
                                workorder_id,
                                digest,
                                &response_digest,
                                Some(response_workorder_id),
                            )?;
                            if sequence != next_sequence {
                                anyhow::bail!(
                                    "Blob stream ack sequence mismatch for {digest}: expected {next_sequence}, got {sequence}"
                                );
                            }
                            if offset_bytes != next_offset {
                                anyhow::bail!(
                                    "Blob stream ack offset mismatch for {digest}: expected {next_offset}, got {offset_bytes}"
                                );
                            }
                            if size_bytes != chunk.len() as u64 {
                                anyhow::bail!(
                                    "Blob stream ack size mismatch for {digest}: expected {}, got {size_bytes}",
                                    chunk.len()
                                );
                            }
                            chunk_policy.on_ack(send_started.elapsed());
                            next_offset = received_bytes;
                            next_sequence += 1;
                        }
                        SnapshotBlobServerControlMessage::UploadError { code, message, .. } => {
                            anyhow::bail!(
                                "Blob stream upload failed while sending {digest} chunk {next_sequence} ({code}): {message}"
                            );
                        }
                        other => {
                            anyhow::bail!(
                                "Unexpected blob upload control frame after chunk {next_sequence} for {digest}: {other:?}"
                            );
                        }
                    }
                }

                let complete = Self::recv_blob_upload_control_with_timeout(
                    &mut ws,
                    digest,
                    SNAPSHOT_BLOB_STREAM_CONTROL_TIMEOUT,
                )
                .await?;
                match complete {
                    SnapshotBlobServerControlMessage::UploadComplete {
                        version,
                        workorder_id: response_workorder_id,
                        digest: response_digest,
                        uploaded,
                    } => {
                        Self::validate_blob_upload_envelope(
                            version,
                            workorder_id,
                            digest,
                            &response_digest,
                            Some(response_workorder_id),
                        )?;
                        debug!(
                            digest,
                            attempt,
                            uploaded,
                            final_chunk_size_bytes = chunk_policy.current_chunk_size_bytes(),
                            "Completed websocket snapshot blob upload"
                        );
                        Ok(uploaded)
                    }
                    SnapshotBlobServerControlMessage::UploadError { code, message, .. } => {
                        anyhow::bail!(
                            "Blob stream upload failed while completing {digest} ({code}): {message}"
                        );
                    }
                    other => anyhow::bail!(
                        "Unexpected blob upload completion frame for {digest}: {other:?}"
                    ),
                }
            }
            .await;

            match upload_result {
                Ok(uploaded) => return Ok(uploaded),
                Err(error)
                    if attempt < attempts && Self::is_retryable_blob_stream_error(&error) =>
                {
                    chunk_policy.on_reconnect();
                    debug!(
                        %error,
                        digest,
                        attempt,
                        attempts,
                        chunk_size_bytes = chunk_policy.current_chunk_size_bytes(),
                        "Blob stream connection failed; reconnecting from upload_resume"
                    );
                    last_error = Some(error);
                    tokio::time::sleep(
                        SNAPSHOT_BLOB_STREAM_RECONNECT_DELAY.mul_f32(attempt as f32),
                    )
                    .await;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Failed to stream snapshot blob {digest} after {attempts} connection attempt(s)"
                        )
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("snapshot blob stream loop ended unexpectedly for {digest}")
        }))
    }

    async fn prepare_upload_payload(bytes: &[u8]) -> Result<SnapshotBlobUploadPayload> {
        let raw_bytes = bytes.to_vec();
        let mut zstd_bytes = None;
        let mut offered_encodings = Vec::new();

        let input = raw_bytes.clone();
        if let Some(compressed) =
            tokio::task::spawn_blocking(move || compression::encode_zstd_if_worthwhile(&input))
                .await
                .context("zstd compression task failed to join")??
        {
            offered_encodings.push(SnapshotBlobEncodingOffer {
                encoding: SNAPSHOT_BLOB_ENCODING_ZSTD.to_string(),
                size_bytes: compressed.len() as u64,
            });
            zstd_bytes = Some(compressed);
        }

        Ok(SnapshotBlobUploadPayload {
            raw_bytes,
            zstd_bytes,
            offered_encodings,
        })
    }

    fn payload_for_selected_encoding<'a>(
        payload: &'a SnapshotBlobUploadPayload,
        selected_encoding: Option<&str>,
    ) -> Result<&'a [u8]> {
        match selected_encoding {
            None => Ok(&payload.raw_bytes),
            Some(SNAPSHOT_BLOB_ENCODING_ZSTD) => payload
                .zstd_bytes
                .as_deref()
                .context("server selected zstd upload without a prepared zstd payload"),
            Some(other) => {
                anyhow::bail!("Blob stream server selected unsupported upload encoding '{other}'")
            }
        }
    }

    /// Download a file from the binhost or API to a local destination.
    pub async fn download_file(&self, url: &str, destination: &Path) -> Result<()> {
        self.download_file_with_progress(url, destination, |_received, _total, _stalled| {})
            .await
    }

    /// Download a file from the binhost or API to a local destination while reporting progress.
    pub async fn download_file_with_progress<F>(
        &self,
        url: &str,
        destination: &Path,
        mut on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(u64, Option<u64>, bool),
    {
        use tokio::io::AsyncWriteExt;

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to download {url}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Server returned {status} for {url}: {body}");
        }

        let total_bytes = resp.content_length();
        debug!(
            url,
            destination = %destination.display(),
            total_bytes,
            "Starting file download"
        );

        let mut file = tokio::fs::File::create(destination)
            .await
            .with_context(|| format!("Failed to create {}", destination.display()))?;
        let mut stream = resp.bytes_stream();
        let mut received_bytes = 0u64;
        let mut stalled = false;
        loop {
            let next_chunk = stream.next();
            tokio::pin!(next_chunk);

            let chunk = loop {
                match tokio::time::timeout(FILE_DOWNLOAD_STALL_THRESHOLD, &mut next_chunk).await {
                    Ok(chunk) => break chunk,
                    Err(_) => {
                        if !stalled {
                            warn!(
                                url,
                                destination = %destination.display(),
                                received_bytes,
                                total_bytes,
                                "File download stalled waiting for the next chunk"
                            );
                            stalled = true;
                        }
                        on_progress(received_bytes, total_bytes, true)
                    }
                }
            };

            let Some(chunk) = chunk else {
                break;
            };

            let chunk = chunk.with_context(|| format!("Failed while reading {url}"))?;
            if stalled {
                debug!(
                    url,
                    destination = %destination.display(),
                    received_bytes,
                    total_bytes,
                    "File download resumed after a stall"
                );
                stalled = false;
            }
            file.write_all(&chunk)
                .await
                .with_context(|| format!("Failed to write {}", destination.display()))?;
            received_bytes += chunk.len() as u64;
            on_progress(received_bytes, total_bytes, false);
        }
        file.flush()
            .await
            .with_context(|| format!("Failed to flush {}", destination.display()))?;
        info!(
            url,
            destination = %destination.display(),
            received_bytes,
            total_bytes,
            "Completed file download"
        );
        Ok(())
    }

    /// Connect to the progress WebSocket and stream events to stdout.
    ///
    /// The connection is bidirectional:
    /// - **Server → Client (Binary frames):** Raw PTY bytes written directly
    ///   to stdout — preserves ANSI escapes, colour, progress bars, etc.
    /// - **Server → Client (Text frames):** Structured build events (status
    ///   changes, package built/failed, finished).
    /// - **Client → Server (Binary frames):** Raw stdin bytes forwarded to
    ///   the worker container for interactive emerge prompts.
    ///
    /// Returns the final [`WorkorderResult`] when the build completes.
    pub async fn stream_progress(
        &self,
        ws_url: &str,
        verbosity: crate::verbosity::Verbosity,
        log_json: bool,
    ) -> Result<WorkorderResult> {
        use tokio_tungstenite::connect_async;

        // Append the log-level ceiling as a query parameter so the server
        // only sends events the CLI can usefully display.
        let log_level_str = verbosity.rust_log_level();
        let ws_url_with_level = format!("{ws_url}?log_level={log_level_str}");
        let ws_url = ws_url_with_level.as_str();

        let (ws, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to progress WebSocket")?;

        let (mut ws_write, mut ws_read) = ws.split();

        let mut final_result: Option<WorkorderResult> = None;
        let workorder_id = Self::workorder_id_from_progress_url(ws_url);
        let mut result_poll = tokio::time::interval(WORKORDER_RESULT_POLL_INTERVAL);
        result_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        result_poll.tick().await;
        // Whether the status bar has been hidden for PTY streaming yet.
        let mut bar_hidden = false;

        // Disable local terminal echo so we don't double-echo input.
        // The container's PTY already echoes back everything we send.
        let _echo_guard = EchoGuard::disable();

        // Spawn a thread that reads from terminal stdin and sends to the
        // server via the WebSocket. This enables interactive emerge prompts
        // like --ask without tying runtime shutdown to Tokio's stdin reader.
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // Stdin reader thread — reads raw bytes (not lines) so that input
        // reaches the container immediately without waiting for Enter.
        // This thread is intentionally detached; a blocked stdin read must
        // never keep the async runtime alive after the build is done.
        let stdin_thread = std::thread::spawn(move || {
            use std::io::Read;
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break; // Channel closed — build finished.
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Forward stdin data to the WebSocket.
        let ws_stdin_handle = tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                let msg = tokio_tungstenite::tungstenite::Message::Binary(data.into());
                if ws_write.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Read build output from the WebSocket.
        loop {
            tokio::select! {
                _ = result_poll.tick(), if workorder_id.is_some() => {
                    if let Some(id) = workorder_id
                        && let Ok(result) = self.fetch_result(id).await
                    {
                        final_result = Some(result);
                        break;
                    }
                }
                maybe_msg = ws_read.next() => {
                    let Some(msg) = maybe_msg else {
                        break;
                    };
                    let msg = msg.context("WebSocket error")?;
                    match msg {
                        // Binary frames carry raw PTY bytes — write directly to stdout.
                        // In log_json mode they are skipped to avoid corrupting the
                        // NDJSON stream; CI tooling should not need raw terminal bytes.
                        tokio_tungstenite::tungstenite::Message::Binary(data) => {
                            if !log_json {
                                // Hide the status bar on the first PTY frame —
                                // raw bytes from the container would overwrite it.
                                if !bar_hidden {
                                    if let Some(bar) = crate::status_bar::StatusBar::global() {
                                        bar.hide();
                                    }
                                    bar_hidden = true;
                                }
                                use std::io::Write;
                                let stdout = std::io::stdout();
                                let mut out = stdout.lock();
                                let _ = out.write_all(&data);
                                let _ = out.flush();
                            }
                        }
                        // Text frames carry structured JSON events.
                        tokio_tungstenite::tungstenite::Message::Text(text) => {
                            if log_json {
                                // Emit each frame as a newline-delimited JSON record
                                // so CI log-capture tooling can consume the stream.
                                use std::io::Write;
                                let stdout = std::io::stdout();
                                let mut out = stdout.lock();
                                let _ = out.write_all(text.as_bytes());
                                let _ = out.write_all(b"\n");
                                let _ = out.flush();
                            }
                            // Parse the event to handle flow control (Finished)
                            // regardless of log_json.
                            if let Ok(progress) = serde_json::from_str::<BuildProgress>(&text) {
                                if !log_json {
                                    Self::print_event(&progress.event, verbosity);
                                }
                                // If we get a Finished event, fetch the result and exit.
                                if matches!(progress.event, BuildEvent::Finished { .. }) {
                                    final_result = Some(
                                        self.await_result_ready(progress.workorder_id).await?
                                    );
                                    break;
                                }
                            } else if let Ok(log_event) =
                                serde_json::from_str::<LogEvent>(&text)
                            {
                                if !log_json {
                                    Self::print_log_event(&log_event, verbosity);
                                }
                            } else {
                                debug!("Unrecognised WS message: {text}");
                            }
                        }
                        tokio_tungstenite::tungstenite::Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }

        // Clean up stdin tasks.
        // Tokio's stdin reader may remain blocked in a background read even
        // after abort, so awaiting it here can hang the CLI after all useful
        // work has completed.
        ws_stdin_handle.abort();
        drop(stdin_thread);
        Self::wait_for_progress_cleanup(
            "shutting down progress-stream stdin forwarding",
            ws_stdin_handle,
        )
        .await?;

        // If we didn't receive a Finished event but the connection closed,
        // try to fetch the result from the REST API as a fallback.  This
        // handles the edge case where the WS Close frame arrives before
        // or instead of a Finished event (e.g. channel lagged, server
        // shutdown, etc.).
        if final_result.is_none() {
            debug!("No Finished event received — attempting REST fallback");
            if let Some(id) = workorder_id {
                final_result = Some(self.await_result_ready(id).await?);
            }
        }

        final_result.context("Build finished but no result was received")
    }

    async fn await_result_ready(
        &self,
        workorder_id: remerge_types::workorder::WorkorderId,
    ) -> Result<WorkorderResult> {
        let deadline = tokio::time::Instant::now() + WORKORDER_RESULT_WAIT_TIMEOUT;
        let result = loop {
            match self.fetch_result_status(workorder_id).await {
                Ok(Some(result)) => break Ok(result),
                Ok(None) if tokio::time::Instant::now() >= deadline => {
                    break Err(anyhow::anyhow!("Workorder has no result yet"));
                }
                Ok(None) => {
                    tokio::time::sleep(WORKORDER_RESULT_POLL_INTERVAL).await;
                }
                Err(error) => return Err(error),
            }
        };

        result.context("Workorder finished but the final result was not available")
    }

    async fn wait_for_progress_cleanup(
        stage: &str,
        handle: tokio::task::JoinHandle<()>,
    ) -> Result<()> {
        match tokio::time::timeout(PROGRESS_STREAM_CLEANUP_TIMEOUT, handle).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => {
                Err(error).with_context(|| format!("Progress stream cleanup failed while {stage}"))
            }
            Err(_) => anyhow::bail!(
                "CLI watchdog timed out after {}s while {stage}; the remote build already finished, so the remaining hang is in local progress-stream cleanup",
                PROGRESS_STREAM_CLEANUP_TIMEOUT.as_secs(),
            ),
        }
    }

    fn workorder_id_from_progress_url(
        ws_url: &str,
    ) -> Option<remerge_types::workorder::WorkorderId> {
        // Strip query string before splitting so `?log_level=...` does not
        // get included in the last path segment.
        let path = ws_url.split('?').next().unwrap_or(ws_url);
        let segments: Vec<&str> = path.split('/').collect();
        segments.iter().rev().nth(1)?.parse::<uuid::Uuid>().ok()
    }

    /// Print a forwarded worker log event based on the current verbosity.
    ///
    /// Quiet     → suppressed entirely (server already hard-limits to Error
    ///             ceiling, but we double-check here)
    /// Normal    → Warn + Error only
    /// Verbose+  → all events the server forwarded
    fn print_log_event(event: &LogEvent, verbosity: crate::verbosity::Verbosity) {
        if !log_event_is_visible(event, verbosity) {
            return;
        }
        let prefix = match event.level {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        };
        if verbosity.is_verbose() {
            // Show target for verbose modes so the caller can correlate.
            eprintln!("[{prefix}] {}: {}", event.target, event.message);
        } else {
            eprintln!("{prefix}: {}", event.message);
        }
    }

    /// Print a build event to the terminal.
    fn print_event(event: &BuildEvent, verbosity: crate::verbosity::Verbosity) {
        match event {
            BuildEvent::StatusChanged { from: _, to } => {
                // Status transitions are low-signal in the default view;
                // only show them at verbose level.
                if verbosity.is_verbose() {
                    let friendly = format!("{to:?}")
                        .replace("WaitingForWorker", "waiting for worker")
                        .replace("SyncingPortage", "syncing portage tree")
                        .replace("Building", "building packages")
                        .replace("Uploading", "uploading artefacts")
                        .replace("Done", "done");
                    if let Some(bar) = crate::status_bar::StatusBar::global() {
                        bar.set_phase(format!("Build: {friendly}"));
                    } else {
                        eprintln!("→ Build status: {friendly}");
                    }
                } else if let Some(bar) = crate::status_bar::StatusBar::global() {
                    // Update phase silently so elapsed time resets.
                    let friendly = format!("{to:?}")
                        .replace("WaitingForWorker", "waiting for a worker…")
                        .replace("SyncingPortage", "syncing portage tree…")
                        .replace("Building", "building packages…")
                        .replace("Uploading", "uploading artefacts…")
                        .replace("Done", "finishing…");
                    bar.set_phase(format!("Remote build: {friendly}"));
                }
            }
            // Log events are delivered as raw binary frames now — this
            // arm is kept for backward compatibility with older servers.
            BuildEvent::Log { line } => {
                use std::io::Write;
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
            }
            BuildEvent::PackageBuilt {
                atom,
                duration_secs,
            } => {
                println!("✔ Built {atom} ({duration_secs}s)");
            }
            BuildEvent::PackageFailed { atom, reason } => {
                eprintln!("✘ Failed {atom}: {reason}");
            }
            BuildEvent::Finished { built, failed } => {
                println!(
                    "── Finished: {} built, {} failed ──",
                    built.len(),
                    failed.len()
                );
            }
        }
    }

    fn snapshot_blob_stream_url(&self) -> String {
        if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}/api/v1/snapshots/blobs/stream")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}/api/v1/snapshots/blobs/stream")
        } else {
            format!("{}/api/v1/snapshots/blobs/stream", self.base_url)
        }
    }

    async fn recv_blob_upload_control(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        digest: &str,
    ) -> Result<SnapshotBlobServerControlMessage> {
        use tokio_tungstenite::tungstenite::Message;

        loop {
            let message = ws
                .next()
                .await
                .with_context(|| {
                    format!("Blob stream closed unexpectedly while uploading {digest}")
                })?
                .with_context(|| format!("Blob stream read failed while uploading {digest}"))?;

            match message {
                Message::Text(text) => {
                    return serde_json::from_str::<SnapshotBlobServerControlMessage>(&text)
                        .with_context(|| {
                            format!("Failed to parse blob upload control frame for {digest}")
                        });
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) => {
                    anyhow::bail!("Blob stream closed before completing upload for {digest}");
                }
                Message::Binary(_) => {
                    anyhow::bail!(
                        "Blob stream server sent an unexpected binary frame while uploading {digest}"
                    );
                }
                Message::Frame(_) => continue,
            }
        }
    }

    async fn recv_blob_upload_control_with_timeout(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        digest: &str,
        timeout: Duration,
    ) -> Result<SnapshotBlobServerControlMessage> {
        match tokio::time::timeout(timeout, Self::recv_blob_upload_control(ws, digest)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "Timed out waiting for blob upload control frame for {digest} after {:?}",
                timeout
            ),
        }
    }

    fn validate_blob_upload_envelope(
        version: u8,
        workorder_id: uuid::Uuid,
        digest: &str,
        response_digest: &str,
        response_workorder_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        if version != SNAPSHOT_BLOB_PROTOCOL_VERSION {
            anyhow::bail!(
                "Blob stream protocol version mismatch for {digest}: expected {}, got {version}",
                SNAPSHOT_BLOB_PROTOCOL_VERSION
            );
        }
        if response_digest != digest {
            anyhow::bail!("Blob stream digest mismatch: expected {digest}, got {response_digest}");
        }
        if response_workorder_id != Some(workorder_id) {
            anyhow::bail!(
                "Blob stream workorder ID mismatch: expected {workorder_id}, got {:?}",
                response_workorder_id
            );
        }
        Ok(())
    }

    fn is_retryable_blob_stream_error(error: &anyhow::Error) -> bool {
        let message = format!("{error:#}").to_ascii_lowercase();
        message.contains("failed to connect to snapshot blob stream")
            || message.contains("failed to send blob chunk")
            || message.contains("blob stream read failed")
            || message.contains("blob stream closed unexpectedly")
            || message.contains("blob stream closed before completing upload")
            || message.contains("timed out waiting for blob upload control frame")
    }

    /// Fetch the workorder result from the REST API.
    async fn fetch_result(
        &self,
        workorder_id: remerge_types::workorder::WorkorderId,
    ) -> Result<WorkorderResult> {
        self.fetch_result_status(workorder_id)
            .await?
            .context("Workorder has no result yet")
    }

    async fn fetch_result_status(
        &self,
        workorder_id: remerge_types::workorder::WorkorderId,
    ) -> Result<Option<WorkorderResult>> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/workorders/{workorder_id}",
                self.base_url
            ))
            .send()
            .await?
            .error_for_status()?;

        let status_resp = resp
            .json::<remerge_types::api::WorkorderStatusResponse>()
            .await?;

        Ok(status_resp.result)
    }
}

/// Whether a [`LogEvent`] should be displayed at a given verbosity level.
///
/// This is the client-side defence-in-depth filter.  The server already
/// applies a per-connection ceiling before sending any frame; this function
/// provides a second layer so that even if the server sends more than
/// requested (e.g. during reconnect overlap), the CLI only prints what the
/// operator asked for.
///
/// Extracted as a free function so it can be covered by unit tests without
/// needing to capture `eprintln!` output.
pub(crate) fn log_event_is_visible(
    event: &LogEvent,
    verbosity: crate::verbosity::Verbosity,
) -> bool {
    use crate::verbosity::Verbosity;
    let max_level = match verbosity {
        Verbosity::Quiet => return false,
        Verbosity::Normal => LogLevel::Warn,
        Verbosity::Verbose => LogLevel::Info,
        Verbosity::VerboseDebug => LogLevel::Debug,
        Verbosity::VerboseTrace => LogLevel::Trace,
    };
    event.level <= max_level
}

// ─── Terminal echo control ──────────────────────────────────────────

/// RAII guard that disables terminal echo on creation and restores it on drop.
///
/// When the container has a TTY (`tty: true`), it echoes all input back in
/// its output stream.  If the client's terminal ALSO echoes locally, every
/// keystroke appears twice.  This guard disables local echo so only the
/// remote PTY echo is visible.
struct EchoGuard {
    original: Option<libc::termios>,
}

impl EchoGuard {
    /// Disable echo on stdin.  Returns a guard that restores the original
    /// settings on drop.  If stdin is not a TTY (e.g. piped), this is a
    /// no-op.
    fn disable() -> Self {
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) != 0 {
                return Self { original: None };
            }
            let original = termios;

            // Disable ECHO and ICANON (canonical mode) so we get raw
            // character-at-a-time input without local echo.
            termios.c_lflag &= !(libc::ECHO | libc::ICANON);
            // Read returns after 1 byte.
            termios.c_cc[libc::VMIN] = 1;
            termios.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios);

            Self {
                original: Some(original),
            }
        }
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        if let Some(ref original) = self.original {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PROGRESS_STREAM_CLEANUP_TIMEOUT, RemergeClient, SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES,
        SnapshotBlobChunkPolicy,
    };
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get,
    };
    use remerge_types::{
        api::WorkorderStatusResponse,
        workorder::{WorkorderResult, WorkorderStatus},
    };
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    #[derive(Clone)]
    enum ResultResponsePlan {
        PendingThenReady {
            polls_before_ready: usize,
            counter: Arc<AtomicUsize>,
            result: WorkorderResult,
        },
        Unauthorized,
    }

    async fn spawn_result_server(plan: ResultResponsePlan) -> String {
        async fn result_handler(
            State(plan): State<ResultResponsePlan>,
            axum::extract::Path(workorder_id): axum::extract::Path<uuid::Uuid>,
        ) -> impl IntoResponse {
            match plan {
                ResultResponsePlan::PendingThenReady {
                    polls_before_ready,
                    counter,
                    result,
                } => {
                    let poll_number = counter.fetch_add(1, Ordering::SeqCst);
                    let response = WorkorderStatusResponse {
                        workorder_id,
                        status: if poll_number >= polls_before_ready {
                            WorkorderStatus::Completed
                        } else {
                            WorkorderStatus::Building
                        },
                        result: (poll_number >= polls_before_ready).then_some(result),
                        trace_id: None,
                    };
                    (StatusCode::OK, Json(response)).into_response()
                }
                ResultResponsePlan::Unauthorized => {
                    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
                }
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind result server");
        let addr = listener.local_addr().expect("result server addr");
        let app = Router::new()
            .route("/api/v1/workorders/{id}", get(result_handler))
            .with_state(plan);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve result server");
        });

        format!("http://{addr}")
    }

    fn sample_workorder_result(workorder_id: uuid::Uuid) -> WorkorderResult {
        WorkorderResult {
            workorder_id,
            built_packages: Vec::new(),
            failed_packages: Vec::new(),
            binhost_uri: "https://example.invalid/binpkgs".to_string(),
            fetched_distfiles: BTreeMap::new(),
            parity_manifest: Default::default(),
        }
    }

    #[test]
    fn parses_workorder_id_from_progress_url() {
        let id = uuid::Uuid::parse_str("23137eac-2455-45cf-a09f-cbdbd3a01fcc").unwrap();

        assert_eq!(
            RemergeClient::workorder_id_from_progress_url(
                "ws://localhost/api/v1/workorders/23137eac-2455-45cf-a09f-cbdbd3a01fcc/progress"
            ),
            Some(id)
        );
    }

    #[test]
    fn rejects_invalid_progress_url_workorder_id() {
        assert_eq!(
            RemergeClient::workorder_id_from_progress_url(
                "ws://localhost/api/v1/workorders/not-a-uuid/progress"
            ),
            None
        );
    }

    #[test]
    fn snapshot_blob_chunk_policy_starts_at_default_chunk_size() {
        let policy = SnapshotBlobChunkPolicy::new(SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES);
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES
        );
    }

    #[test]
    fn snapshot_blob_chunk_policy_shrinks_on_slow_ack_and_reconnect() {
        let mut policy = SnapshotBlobChunkPolicy::new(SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES);

        policy.on_ack(Duration::from_millis(250));
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 2
        );

        policy.on_reconnect();
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 4
        );
    }

    #[test]
    fn snapshot_blob_chunk_policy_grows_only_after_healthy_ack_streak() {
        let mut policy = SnapshotBlobChunkPolicy::new(SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES);
        policy.on_ack(Duration::from_millis(250));
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 2
        );

        policy.on_ack(Duration::from_millis(10));
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES / 2
        );

        policy.on_ack(Duration::from_millis(10));
        assert_eq!(
            policy.current_chunk_size_bytes(),
            SNAPSHOT_BLOB_DEFAULT_CHUNK_SIZE_BYTES
        );
    }

    #[tokio::test]
    async fn prepare_upload_payload_offers_zstd_when_worthwhile() {
        let payload = vec![b'a'; 256 * 1024];

        let prepared = RemergeClient::prepare_upload_payload(&payload)
            .await
            .expect("prepare upload payload");

        assert_eq!(prepared.offered_encodings.len(), 1);
        assert_eq!(prepared.offered_encodings[0].encoding, "zstd");
        assert!(prepared.zstd_bytes.is_some());
    }

    #[tokio::test]
    async fn prepare_upload_payload_keeps_small_payloads_raw() {
        let prepared = RemergeClient::prepare_upload_payload(b"small-payload")
            .await
            .expect("prepare upload payload");

        assert!(prepared.offered_encodings.is_empty());
        assert!(prepared.zstd_bytes.is_none());
        assert_eq!(prepared.raw_bytes, b"small-payload");
    }

    #[tokio::test]
    async fn progress_cleanup_watchdog_reports_the_stuck_stage() {
        let handle = tokio::spawn(async move {
            tokio::time::sleep(PROGRESS_STREAM_CLEANUP_TIMEOUT + Duration::from_secs(1)).await;
        });

        let error = RemergeClient::wait_for_progress_cleanup("testing cleanup watchdog", handle)
            .await
            .expect_err("watchdog should time out");

        let message = error.to_string();
        assert!(
            message.contains("CLI watchdog timed out"),
            "unexpected error: {error:#}"
        );
        assert!(
            message.contains("testing cleanup watchdog"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn await_result_ready_retries_until_result_is_present() {
        let workorder_id = uuid::Uuid::new_v4();
        let expected = sample_workorder_result(workorder_id);
        let polls = Arc::new(AtomicUsize::new(0));
        let base_url = spawn_result_server(ResultResponsePlan::PendingThenReady {
            polls_before_ready: 2,
            counter: polls.clone(),
            result: expected.clone(),
        })
        .await;
        let client = RemergeClient::new(&base_url).expect("client");

        let result = client
            .await_result_ready(workorder_id)
            .await
            .expect("result should become available");

        assert_eq!(result.workorder_id, expected.workorder_id);
        assert!(polls.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn await_result_ready_fails_fast_on_http_errors() {
        let workorder_id = uuid::Uuid::new_v4();
        let base_url = spawn_result_server(ResultResponsePlan::Unauthorized).await;
        let client = RemergeClient::new(&base_url).expect("client");

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            client.await_result_ready(workorder_id),
        )
        .await
        .expect("http errors should not be retried until timeout")
        .expect_err("unauthorized result fetch should fail");

        let message = format!("{error:#}");
        assert!(
            message.contains("401") || message.to_ascii_lowercase().contains("unauthorized"),
            "unexpected error: {error:#}"
        );
    }
}

// ─── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod verbosity_filter_tests {
    use chrono::Utc;
    use remerge_types::api::{LogEvent, LogLevel};
    use uuid::Uuid;

    use super::log_event_is_visible;
    use crate::verbosity::Verbosity;

    fn make_event(level: LogLevel) -> LogEvent {
        LogEvent {
            level,
            target: "remerge_worker::builder".to_string(),
            message: "test message".to_string(),
            workorder_id: Uuid::new_v4(),
            span: None,
            timestamp: Utc::now(),
        }
    }

    // ── Quiet: all events suppressed ──────────────────────────────

    #[test]
    fn quiet_suppresses_error() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Error),
            Verbosity::Quiet
        ));
    }

    #[test]
    fn quiet_suppresses_warn() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Warn),
            Verbosity::Quiet
        ));
    }

    #[test]
    fn quiet_suppresses_info() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Info),
            Verbosity::Quiet
        ));
    }

    // ── Normal: only Warn and Error pass ──────────────────────────

    #[test]
    fn normal_shows_error() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Error),
            Verbosity::Normal
        ));
    }

    #[test]
    fn normal_shows_warn() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Warn),
            Verbosity::Normal
        ));
    }

    #[test]
    fn normal_suppresses_info() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Info),
            Verbosity::Normal
        ));
    }

    #[test]
    fn normal_suppresses_debug() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Debug),
            Verbosity::Normal
        ));
    }

    #[test]
    fn normal_suppresses_trace() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Trace),
            Verbosity::Normal
        ));
    }

    // ── Verbose: Info and above pass ─────────────────────────────

    #[test]
    fn verbose_shows_error() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Error),
            Verbosity::Verbose
        ));
    }

    #[test]
    fn verbose_shows_warn() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Warn),
            Verbosity::Verbose
        ));
    }

    #[test]
    fn verbose_shows_info() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Info),
            Verbosity::Verbose
        ));
    }

    #[test]
    fn verbose_suppresses_debug() {
        // Server sends at most Info at -v; local filter still matches server ceiling.
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Debug),
            Verbosity::Verbose
        ));
    }

    // ── VerboseDebug: Debug and above pass ───────────────────────

    #[test]
    fn verbose_debug_shows_debug() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Debug),
            Verbosity::VerboseDebug
        ));
    }

    #[test]
    fn verbose_debug_suppresses_trace() {
        assert!(!log_event_is_visible(
            &make_event(LogLevel::Trace),
            Verbosity::VerboseDebug
        ));
    }

    // ── VerboseTrace: everything passes ──────────────────────────

    #[test]
    fn verbose_trace_shows_trace() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Trace),
            Verbosity::VerboseTrace
        ));
    }

    #[test]
    fn verbose_trace_shows_error() {
        assert!(log_event_is_visible(
            &make_event(LogLevel::Error),
            Verbosity::VerboseTrace
        ));
    }
}
