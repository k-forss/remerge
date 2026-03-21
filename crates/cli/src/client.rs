//! HTTP + WebSocket client for communicating with the remerge server.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use remerge_types::{
    api::{SubmitWorkorderRequest, SubmitWorkorderResponse},
    client::{ClientId, ClientRole},
    portage::{PortageConfig, SystemIdentity},
    workorder::{BuildEvent, BuildProgress, WorkorderResult},
};
use tracing::debug;

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

        let resp = self
            .http
            .post(format!("{}/api/v1/workorders", self.base_url))
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

    /// Connect to the progress WebSocket and stream events to stdout.
    ///
    /// The connection is bidirectional: build output is printed to stdout,
    /// and local stdin is forwarded to the worker container so interactive
    /// emerge features (`--ask`, USE prompts, etc.) work transparently.
    ///
    /// Returns the final [`WorkorderResult`] when the build completes.
    pub async fn stream_progress(&self, ws_url: &str) -> Result<WorkorderResult> {
        use tokio_tungstenite::connect_async;

        let (ws, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to progress WebSocket")?;

        let (mut ws_write, mut ws_read) = ws.split();

        let mut final_result: Option<WorkorderResult> = None;

        // Spawn a task that reads from terminal stdin and sends to the
        // server via the WebSocket.  This enables interactive emerge
        // prompts like --ask to work through the worker container.
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // Stdin reader task — runs on a blocking thread since stdin I/O
        // blocks.  Only active while the WebSocket connection is alive.
        let stdin_handle = tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let stdin = tokio::io::stdin();
            let mut reader = tokio::io::BufReader::new(stdin);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if stdin_tx.send(line.as_bytes().to_vec()).await.is_err() {
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

        // Read build progress events from the WebSocket.
        while let Some(msg) = ws_read.next().await {
            let msg = msg.context("WebSocket error")?;
            match msg {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    if let Ok(progress) = serde_json::from_str::<BuildProgress>(&text) {
                        Self::print_event(&progress.event);

                        // If we get a Finished event, try to fetch the result.
                        if matches!(progress.event, BuildEvent::Finished { .. }) {
                            final_result = self.fetch_result(progress.workorder_id).await.ok();
                        }
                    } else {
                        debug!("Unrecognised WS message: {text}");
                    }
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => break,
                _ => {}
            }
        }

        // Clean up stdin tasks.
        stdin_handle.abort();
        ws_stdin_handle.abort();

        final_result.context("Build finished but no result was received")
    }

    /// Print a build event to the terminal.
    fn print_event(event: &BuildEvent) {
        match event {
            BuildEvent::StatusChanged { from: _, to } => {
                println!(">>> Status: {to:?}");
            }
            BuildEvent::Log { line } => {
                println!("{line}");
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

    /// Fetch the workorder result from the REST API.
    async fn fetch_result(
        &self,
        workorder_id: remerge_types::workorder::WorkorderId,
    ) -> Result<WorkorderResult> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/workorders/{workorder_id}",
                self.base_url
            ))
            .send()
            .await?;

        let status_resp = resp
            .json::<remerge_types::api::WorkorderStatusResponse>()
            .await?;

        status_resp.result.context("Workorder has no result yet")
    }
}
