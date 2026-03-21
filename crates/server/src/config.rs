//! Server configuration.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::auth::AuthConfig;

/// Top-level server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Directory where binary packages are stored and served from.
    #[serde(default = "default_binpkg_dir")]
    pub binpkg_dir: PathBuf,

    /// Base URL that clients use to reach the binhost.
    /// This is written into binrepos.conf on the client side.
    #[serde(default = "default_binhost_url")]
    pub binhost_url: String,

    /// Docker socket path.
    #[serde(default = "default_docker_socket")]
    pub docker_socket: String,

    /// Docker image prefix for worker images (e.g. `remerge-worker`).
    #[serde(default = "default_worker_image_prefix")]
    pub worker_image_prefix: String,

    /// Maximum concurrent worker containers.
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,

    /// How long (seconds) to keep idle worker containers before removing them.
    #[serde(default = "default_worker_idle_timeout")]
    pub worker_idle_timeout: u64,

    /// Volume mount path inside workers for the binpkg output.
    #[serde(default = "default_worker_binpkg_mount")]
    pub worker_binpkg_mount: String,

    /// Authentication / mTLS configuration.
    #[serde(default)]
    pub auth: AuthConfig,

    /// Binary package OpenPGP signing configuration.
    #[serde(default)]
    pub signing: SigningConfig,

    /// Number of parallel emerge jobs (`-j` flag) for the worker.
    ///
    /// If `None`, auto-detected from available CPU count.
    #[serde(default)]
    pub parallel_jobs: Option<u32>,

    /// Maximum system load average (`-l` flag) for emerge builds.
    ///
    /// If `None`, auto-detected from available CPU count.
    #[serde(default)]
    pub load_average: Option<f32>,

    /// Optional TLS configuration for direct HTTPS without a reverse proxy.
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// Directory for persisting state (workorders, results, clients) across
    /// restarts.
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,

    /// How many hours to keep completed/failed workorders before automatic
    /// eviction.
    #[serde(default = "default_retention_hours")]
    pub retention_hours: u64,

    /// Path to the `remerge-worker` binary for injection into worker images.
    ///
    /// When set, the binary is `COPY`ed into the Docker image during build.
    #[serde(default)]
    pub worker_binary: Option<PathBuf>,

    /// Maximum number of completed/failed workorders to keep in memory.
    ///
    /// When exceeded, the oldest terminal workorders are evicted first.
    /// Set to `0` to disable the cap (rely only on `retention_hours`).
    #[serde(default = "default_max_retained_workorders")]
    pub max_retained_workorders: usize,

    /// Enable JSON-formatted structured log output.
    ///
    /// When `true`, all log lines are emitted as single-line JSON objects
    /// suitable for log aggregation pipelines.  Default is `false`
    /// (human-readable).
    #[serde(default)]
    pub log_json: bool,

    /// Host path to the portage ebuild repositories (e.g. `/var/db/repos`).
    ///
    /// When set, this directory is bind-mounted read-only into worker
    /// containers, and the per-build `emerge --sync` is skipped.  This
    /// ensures all workers use the exact same ebuild tree as the server
    /// and eliminates the 2–3 minute sync penalty on every build.
    ///
    /// Keep the directory in sync via a cron job or systemd timer:
    /// ```sh
    /// emaint sync -a   # or emerge --sync
    /// ```
    #[serde(default)]
    pub repos_dir: Option<PathBuf>,

    /// Disk usage warning threshold for the binpkg directory, in bytes.
    ///
    /// When the repository exceeds this size, a warning metric is exposed
    /// and a log message is emitted.  Set to `0` to disable.
    /// Default: 10 GiB.
    #[serde(default = "default_binpkg_disk_warn_bytes")]
    pub binpkg_disk_warn_bytes: u64,
}

fn default_binpkg_dir() -> PathBuf {
    "/var/cache/remerge/binpkgs".into()
}
fn default_binhost_url() -> String {
    "http://localhost:7654/binpkgs".into()
}
fn default_docker_socket() -> String {
    "unix:///var/run/docker.sock".into()
}
fn default_worker_image_prefix() -> String {
    "remerge-worker".into()
}
fn default_max_workers() -> usize {
    4
}
fn default_worker_idle_timeout() -> u64 {
    3600
}
fn default_worker_binpkg_mount() -> String {
    "/var/cache/binpkgs".into()
}
fn default_state_dir() -> PathBuf {
    "/var/lib/remerge".into()
}
fn default_retention_hours() -> u64 {
    24
}
fn default_max_retained_workorders() -> usize {
    1000
}
fn default_binpkg_disk_warn_bytes() -> u64 {
    10 * 1024 * 1024 * 1024 // 10 GiB
}

/// Binary package OpenPGP signing configuration.
///
/// When configured, the server mounts the GPG keyring into worker containers
/// and instructs portage to sign all generated `.gpkg` packages.
///
/// See <https://wiki.gentoo.org/wiki/Binary_package_guide#Binary_package_OpenPGP_signing>
///
/// ```toml
/// [signing]
/// gpg_key = "0x1234567890ABCDEF"
/// gpg_home = "/var/cache/remerge/gnupg"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SigningConfig {
    /// GPG key fingerprint for signing binary packages.
    ///
    /// Maps to portage's `BINPKG_GPG_SIGNING_KEY`.
    #[serde(default)]
    pub gpg_key: Option<String>,

    /// Path to the GPG home directory containing the signing keyring.
    ///
    /// Mounted read-only into worker containers.  Maps to portage's
    /// `BINPKG_GPG_SIGNING_GPG_HOME`.
    #[serde(default)]
    pub gpg_home: Option<String>,
}

impl SigningConfig {
    /// Returns `true` if binary package signing is fully configured.
    pub fn enabled(&self) -> bool {
        self.gpg_key.is_some() && self.gpg_home.is_some()
    }
}

/// Optional TLS configuration for serving HTTPS directly.
///
/// ```toml
/// [tls]
/// cert = "/etc/remerge/cert.pem"
/// key = "/etc/remerge/key.pem"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to the PEM-encoded certificate chain.
    pub cert: PathBuf,
    /// Path to the PEM-encoded private key.
    pub key: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            binpkg_dir: default_binpkg_dir(),
            binhost_url: default_binhost_url(),
            docker_socket: default_docker_socket(),
            worker_image_prefix: default_worker_image_prefix(),
            max_workers: default_max_workers(),
            worker_idle_timeout: default_worker_idle_timeout(),
            worker_binpkg_mount: default_worker_binpkg_mount(),
            auth: AuthConfig::default(),
            signing: SigningConfig::default(),
            parallel_jobs: None,
            load_average: None,
            tls: None,
            state_dir: default_state_dir(),
            retention_hours: default_retention_hours(),
            worker_binary: None,
            max_retained_workorders: default_max_retained_workorders(),
            log_json: false,
            repos_dir: None,
            binpkg_disk_warn_bytes: default_binpkg_disk_warn_bytes(),
        }
    }
}

impl ServerConfig {
    /// Validate configuration values and log warnings for problems.
    pub fn validate(&mut self) {
        if let Some(ref path) = self.repos_dir {
            if !path.exists() {
                tracing::warn!(path = %path.display(), "repos_dir does not exist — ignoring");
                self.repos_dir = None;
            } else if !path.is_dir() {
                tracing::warn!(path = %path.display(), "repos_dir is not a directory — ignoring");
                self.repos_dir = None;
            }
        }
    }

    /// Load configuration from a TOML file, then apply environment variable
    /// overrides (`REMERGE_*`).
    pub fn load(path: &str) -> Result<Self> {
        let mut config = match std::fs::read_to_string(path) {
            Ok(content) => toml::from_str(&content).context("Failed to parse server config")?,
            Err(_) => {
                tracing::warn!("Config file {path} not found — using defaults");
                Self::default()
            }
        };

        // Environment variable overrides (for docker-compose / container use).
        if let Ok(v) = std::env::var("REMERGE_BINPKG_DIR") {
            config.binpkg_dir = v.into();
        }
        if let Ok(v) = std::env::var("REMERGE_BINHOST_URL") {
            config.binhost_url = v;
        }
        if let Ok(v) = std::env::var("REMERGE_DOCKER_SOCKET") {
            config.docker_socket = v;
        }
        if let Ok(v) = std::env::var("REMERGE_WORKER_IMAGE_PREFIX") {
            config.worker_image_prefix = v;
        }
        if let Ok(v) = std::env::var("REMERGE_MAX_WORKERS")
            && let Ok(n) = v.parse()
        {
            config.max_workers = n;
        }
        if let Ok(v) = std::env::var("REMERGE_WORKER_IDLE_TIMEOUT")
            && let Ok(n) = v.parse()
        {
            config.worker_idle_timeout = n;
        }
        if let Ok(v) = std::env::var("REMERGE_WORKER_BINPKG_MOUNT") {
            config.worker_binpkg_mount = v;
        }
        if let Ok(v) = std::env::var("REMERGE_AUTH_MODE")
            && let Ok(mode) = v.parse()
        {
            config.auth.mode = mode;
        }
        if let Ok(v) = std::env::var("REMERGE_AUTH_CERT_HEADER") {
            config.auth.cert_header = v;
        }
        if let Ok(v) = std::env::var("REMERGE_GPG_KEY") {
            config.signing.gpg_key = Some(v);
        }
        if let Ok(v) = std::env::var("REMERGE_GPG_HOME") {
            config.signing.gpg_home = Some(v);
        }
        if let Ok(v) = std::env::var("REMERGE_PARALLEL_JOBS")
            && let Ok(n) = v.parse()
        {
            config.parallel_jobs = Some(n);
        }
        if let Ok(v) = std::env::var("REMERGE_LOAD_AVERAGE")
            && let Ok(n) = v.parse()
        {
            config.load_average = Some(n);
        }
        if let Ok(v) = std::env::var("REMERGE_STATE_DIR") {
            config.state_dir = v.into();
        }
        if let Ok(v) = std::env::var("REMERGE_RETENTION_HOURS")
            && let Ok(n) = v.parse()
        {
            config.retention_hours = n;
        }
        if let Ok(v) = std::env::var("REMERGE_WORKER_BINARY") {
            config.worker_binary = Some(v.into());
        }

        // Auto-discover the worker binary if not explicitly configured.
        // Look for `remerge-worker` next to the running server binary
        // (e.g. both installed in /usr/bin/).
        if config.worker_binary.is_none()
            && let Ok(exe) = std::env::current_exe()
        {
            let sibling = exe.with_file_name("remerge-worker");
            if sibling.is_file() {
                tracing::info!(path = %sibling.display(), "Auto-discovered worker binary");
                config.worker_binary = Some(sibling);
            }
        }
        if let Ok(v) = std::env::var("REMERGE_MAX_RETAINED_WORKORDERS")
            && let Ok(n) = v.parse()
        {
            config.max_retained_workorders = n;
        }
        if let Ok(v) = std::env::var("REMERGE_LOG_JSON") {
            config.log_json = v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Ok(v) = std::env::var("REMERGE_REPOS_DIR") {
            config.repos_dir = Some(v.into());
        }
        if let Ok(v) = std::env::var("REMERGE_BINPKG_DISK_WARN_BYTES")
            && let Ok(n) = v.parse()
        {
            config.binpkg_disk_warn_bytes = n;
        }

        config.validate();

        Ok(config)
    }
}
