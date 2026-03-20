//! HTTP + WebSocket client for communicating with the remerge server.

use anyhow::{Context, Result};
use futures::StreamExt;
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
    /// Returns the final [`WorkorderResult`] when the build completes.
    pub async fn stream_progress(&self, ws_url: &str) -> Result<WorkorderResult> {
        use tokio_tungstenite::connect_async;

        let (ws, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to progress WebSocket")?;

        let (_, mut read) = ws.split();

        let mut final_result: Option<WorkorderResult> = None;

        while let Some(msg) = read.next().await {
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
