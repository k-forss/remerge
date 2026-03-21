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

        // Disable local terminal echo so we don't double-echo input.
        // The container's PTY already echoes back everything we send.
        let _echo_guard = EchoGuard::disable();

        // Spawn a task that reads from terminal stdin and sends to the
        // server via the WebSocket.  This enables interactive emerge
        // prompts like --ask to work through the worker container.
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // Stdin reader task — reads raw bytes (not lines) so that input
        // reaches the container immediately without waiting for Enter.
        let stdin_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stdin = tokio::io::stdin();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        if stdin_tx.send(buf[..n].to_vec()).await.is_err() {
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

                        // If we get a Finished event, fetch the result and exit.
                        if matches!(progress.event, BuildEvent::Finished { .. }) {
                            final_result = self.fetch_result(progress.workorder_id).await.ok();
                            break;
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
        use std::io::Write;
        match event {
            BuildEvent::StatusChanged { from: _, to } => {
                println!(">>> Status: {to:?}");
            }
            BuildEvent::Log { line } => {
                // Write the raw log output without adding a newline.
                // The container PTY output already includes \r\n where
                // appropriate, and prompts intentionally omit it so the
                // cursor stays on the same line for user input.
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
