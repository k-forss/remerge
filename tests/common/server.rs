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

/// Ensure the Gentoo stage3 base image is available locally.
///
/// If the image is not present it will be pulled from Docker Hub.
/// Panics when the pull fails so that the test surfaces a clear error
/// instead of silently skipping.
pub fn ensure_stage3() {
    let already_present = std::process::Command::new("docker")
        .args(["inspect", "--type=image", "gentoo/stage3:latest"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if already_present {
        return;
    }

    eprintln!("gentoo/stage3:latest not found locally — pulling from Docker Hub …");
    let status = std::process::Command::new("docker")
        .args(["pull", "gentoo/stage3:latest"])
        .status()
        .expect("failed to run docker pull");

    assert!(
        status.success(),
        "docker pull gentoo/stage3:latest failed (exit {})",
        status,
    );
}

pub const TEST_STAGE3_IMAGE: &str = "remerge/test-stage3:latest";

/// Ensure the pre-synced test stage3 image is available locally.
///
/// This image has portage already synced and `app-misc/hello` installed,
/// so worker image builds are fast (no 5+ minute `emerge --sync`).
///
/// If the image is not present it is built from
/// `docker/test-stage3.Dockerfile`.  The base `gentoo/stage3:latest`
/// image is pulled first if needed.
pub fn ensure_test_stage3() {
    let already_present = std::process::Command::new("docker")
        .args(["inspect", "--type=image", TEST_STAGE3_IMAGE])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if already_present {
        return;
    }

    // The base image must exist before building.
    ensure_stage3();

    eprintln!("{TEST_STAGE3_IMAGE} not found — building from docker/test-stage3.Dockerfile …");

    // Resolve the repository root (the Dockerfile expects to be built
    // from the repo root).
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let status = std::process::Command::new("docker")
        .args([
            "build",
            "-f",
            "docker/test-stage3.Dockerfile",
            "-t",
            TEST_STAGE3_IMAGE,
            ".",
        ])
        .current_dir(repo_root)
        .status()
        .expect("failed to run docker build");

    assert!(
        status.success(),
        "docker build of {TEST_STAGE3_IMAGE} failed (exit {status})",
    );
}

/// A test server handle — keeps the server alive for the test duration.
pub struct TestServer {
    pub port: u16,
    pub base_url: String,
    pub state: std::sync::Arc<remerge_server::state::AppState>,
    _handle: tokio::task::JoinHandle<()>,
    _queue_handle: Option<tokio::task::JoinHandle<()>>,
    _binpkg_dir: tempfile::TempDir,
    _state_dir: tempfile::TempDir,
}

impl TestServer {
    /// Start an in-process test server. Requires Docker to be available.
    pub async fn start() -> Option<Self> {
        Self::start_inner(None, false).await
    }

    /// Start an in-process test server with the queue processor enabled.
    ///
    /// The queue processor picks up pending workorders and runs them in
    /// Docker containers, just like the production server.  This requires
    /// a compiled `remerge-worker` binary (found next to the test binary
    /// or in `target/debug`).
    ///
    /// Uses the pre-synced `remerge/test-stage3:latest` image as the
    /// worker base to avoid the slow `emerge --sync` during image builds.
    pub async fn start_with_queue() -> Option<Self> {
        ensure_test_stage3();

        let mut config = remerge_server::config::ServerConfig {
            worker_base_image: Some(TEST_STAGE3_IMAGE.to_string()),
            skip_worker_sync: true,
            ..Default::default()
        };
        config.worker_binary = find_worker_binary();
        assert!(
            config.worker_binary.is_some(),
            "Queue processor enabled but remerge-worker binary not found. \
             Build it first with: cargo build -p remerge-worker"
        );

        Self::start_inner(Some(config), true).await
    }

    /// Start an in-process test server with a custom config override.
    /// If `config_override` is None, uses the default config.
    pub async fn start_with_config(
        config_override: Option<remerge_server::config::ServerConfig>,
    ) -> Option<Self> {
        Self::start_inner(config_override, false).await
    }

    async fn start_inner(
        config_override: Option<remerge_server::config::ServerConfig>,
        enable_queue: bool,
    ) -> Option<Self> {
        if !docker_available() {
            return None;
        }

        let port = super::free_port();
        let binpkg_dir = tempfile::TempDir::new().expect("temp dir");
        let state_dir = tempfile::TempDir::new().expect("temp dir");

        let mut config = config_override.unwrap_or_default();

        // Always override dirs and URL with test-local temp paths so
        // tests never collide and never touch the real filesystem.
        config.binpkg_dir = binpkg_dir.path().to_path_buf();
        config.binhost_url = format!("http://127.0.0.1:{port}/binpkgs");
        config.state_dir = state_dir.path().to_path_buf();

        if enable_queue && config.worker_binary.is_none() {
            config.worker_binary = find_worker_binary();
            assert!(
                config.worker_binary.is_some(),
                "Queue processor enabled but remerge-worker binary not found. \
                 Build it first with: cargo build -p remerge-worker"
            );
        }

        let state = std::sync::Arc::new(
            remerge_server::state::AppState::new(config)
                .await
                .expect("AppState::new failed — Docker is available but state init errored"),
        );

        let queue_handle = if enable_queue {
            let q = state.clone();
            Some(tokio::spawn(async move {
                remerge_server::queue::process_queue(q).await;
            }))
        } else {
            None
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
            _queue_handle: queue_handle,
            _binpkg_dir: binpkg_dir,
            _state_dir: state_dir,
        })
    }
}

/// Locate the compiled `remerge-worker` binary.
///
/// Checks, in order:
/// 1. Next to the currently-running test binary (same directory).
/// 2. `target/debug/remerge-worker` relative to `CARGO_MANIFEST_DIR`.
fn find_worker_binary() -> Option<std::path::PathBuf> {
    // Check next to the current executable.
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap().join("remerge-worker");
        if sibling.is_file() {
            return Some(sibling);
        }
        // cargo puts test binaries under target/debug/deps but the worker
        // binary lives in target/debug — walk up one level.
        if let Some(parent) = exe.parent().and_then(|p| p.parent()) {
            let sibling = parent.join("remerge-worker");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    // Fallback: relative to manifest dir.
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let candidate = std::path::PathBuf::from(dir)
            .join("target")
            .join("debug")
            .join("remerge-worker");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
