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

/// A test server handle — keeps the server alive for the test duration.
pub struct TestServer {
    pub port: u16,
    pub base_url: String,
    _handle: tokio::task::JoinHandle<()>,
    _binpkg_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
}

impl TestServer {
    /// Start an in-process test server. Requires Docker to be available.
    pub async fn start() -> Option<Self> {
        if !docker_available() {
            return None;
        }

        let port = super::free_port();
        let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
        let state_dir = tempfile::TempDir::new().expect("temp dir");

        // Create a minimal server config
        let config = remerge_server::config::ServerConfig {
            binpkg_dir: binpkg_dir.path().to_path_buf(),
            binhost_url: format!("http://127.0.0.1:{port}/binpkgs"),
            state_dir: state_dir.path().to_path_buf(),
            ..Default::default()
        };

        let state = match remerge_server::state::AppState::new(config).await {
            Ok(s) => std::sync::Arc::new(s),
            Err(_) => return None,
        };

        let app = remerge_server::api::router(state);
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
            _handle: handle,
            _binpkg_dir: binpkg_dir,
            _state_dir: state_dir,
        })
    }
}
