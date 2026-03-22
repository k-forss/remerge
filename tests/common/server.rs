/// Check if Docker is available on this system.
pub fn docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if the Gentoo stage3 base image is available locally.
/// E2E tests that perform actual builds need this image.
pub fn stage3_available() -> bool {
    std::process::Command::new("docker")
        .args(["inspect", "--type=image", "gentoo/stage3:latest"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A test server handle — keeps the server alive for the test duration.
pub struct TestServer {
    pub port: u16,
    pub base_url: String,
    pub state: std::sync::Arc<remerge_server::state::AppState>,
    _handle: tokio::task::JoinHandle<()>,
    _binpkg_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
}

impl TestServer {
    /// Start an in-process test server. Requires Docker to be available.
    pub async fn start() -> Option<Self> {
        Self::start_with_config(None).await
    }

    /// Start an in-process test server with a custom config override.
    /// If `config_override` is None, uses the default config.
    pub async fn start_with_config(
        config_override: Option<remerge_server::config::ServerConfig>,
    ) -> Option<Self> {
        if !docker_available() {
            return None;
        }

        let port = super::free_port();
        let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
        let state_dir = tempfile::TempDir::new().expect("temp dir");

        let config = config_override.unwrap_or_else(|| remerge_server::config::ServerConfig {
            binpkg_dir: binpkg_dir.path().to_path_buf(),
            binhost_url: format!("http://127.0.0.1:{port}/binpkgs"),
            state_dir: state_dir.path().to_path_buf(),
            ..Default::default()
        });

        let state = match remerge_server::state::AppState::new(config).await {
            Ok(s) => std::sync::Arc::new(s),
            Err(_) => return None,
        };

        let app = remerge_server::api::router(state.clone());
        let addr = format!("127.0.0.1:{port}");
        let listener = tokio::net::TcpListener::bind(&addr).await.ok()?;

        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // Wait a bit for the server to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        Some(TestServer {
            port,
            base_url: format!("http://127.0.0.1:{port}"),
            state,
            _handle: handle,
            _binpkg_dir: binpkg_dir,
            _state_dir: state_dir,
        })
    }
}
