//! Docker container management for worker containers.

use anyhow::{Context, Result};
use bollard::{
    Docker,
    models::{ContainerCreateBody, HostConfig},
    query_parameters::{
        BuildImageOptions, CreateContainerOptions, LogsOptions, RemoveContainerOptions,
        StartContainerOptions,
    },
};
use futures::StreamExt;
use tracing::{debug, info};

use remerge_types::portage::SystemIdentity;

use crate::config::ServerConfig;

/// Manages Docker containers for build workers.
pub struct DockerManager {
    docker: Docker,
    image_prefix: String,
    binpkg_dir: String,
    worker_binpkg_mount: String,
    /// Maximum concurrent worker containers (enforced via semaphore in AppState).
    #[allow(dead_code)]
    max_workers: usize,
    /// GPG key fingerprint for binary package signing.
    gpg_key: Option<String>,
    /// Host path to the GPG keyring directory.
    gpg_home: Option<String>,
    /// Path to the remerge-worker binary for injection into images.
    worker_binary: Option<String>,
}

impl DockerManager {
    pub async fn new(config: &ServerConfig) -> Result<Self> {
        let docker =
            Docker::connect_with_socket(&config.docker_socket, 120, bollard::API_DEFAULT_VERSION)
                .context("Failed to connect to Docker")?;

        // Verify connection.
        let info = docker.info().await.context("Docker ping failed")?;
        info!(
            server_version = ?info.server_version,
            "Connected to Docker"
        );

        Ok(Self {
            docker,
            image_prefix: config.worker_image_prefix.clone(),
            binpkg_dir: config.binpkg_dir.to_string_lossy().to_string(),
            worker_binpkg_mount: config.worker_binpkg_mount.clone(),
            max_workers: config.max_workers,
            gpg_key: config.signing.gpg_key.clone(),
            gpg_home: config.signing.gpg_home.clone(),
            worker_binary: config
                .worker_binary
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        })
    }

    /// Derive a Docker image tag from the system identity.
    ///
    /// Each unique `(CHOST, profile, gcc_version)` combination gets its own
    /// image so the toolchain matches the requester.
    pub fn image_tag(&self, sys: &SystemIdentity) -> String {
        // Sanitise for Docker tag rules.
        let chost_slug = sys.chost.replace('.', "_");
        let profile_slug = sys.profile.replace('/', "-");
        let gcc_short = sys
            .gcc_version
            .split_whitespace()
            .last()
            .unwrap_or("unknown")
            .replace('.', "_");

        format!(
            "{}:{}-{}-gcc{}",
            self.image_prefix, chost_slug, profile_slug, gcc_short
        )
    }

    /// Check if a worker image already exists.
    pub async fn image_exists(&self, tag: &str) -> bool {
        self.docker.inspect_image(tag).await.is_ok()
    }

    /// Build a worker image from the bundled Dockerfile.
    pub async fn build_worker_image(&self, sys: &SystemIdentity, tag: &str) -> Result<()> {
        info!(%tag, "Building worker image");

        // The Dockerfile is generated dynamically based on the system identity.
        let dockerfile = self.generate_dockerfile(sys);

        // Create a tar archive containing only the Dockerfile.
        let mut ar = tar::Builder::new(Vec::new());
        let dockerfile_bytes = dockerfile.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_path("Dockerfile")?;
        header.set_size(dockerfile_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        ar.append(&header, dockerfile_bytes)?;

        // If a worker binary path is configured, include it in the build context.
        if let Some(ref binary_path) = self.worker_binary {
            match std::fs::read(binary_path) {
                Ok(binary_data) => {
                    let mut bin_header = tar::Header::new_gnu();
                    bin_header.set_path("remerge-worker")?;
                    bin_header.set_size(binary_data.len() as u64);
                    bin_header.set_mode(0o755);
                    bin_header.set_cksum();
                    ar.append(&bin_header, &binary_data[..])?;
                    info!("Included worker binary in Docker build context");
                }
                Err(e) => {
                    tracing::warn!(path = %binary_path, "Failed to read worker binary: {e} — image will use pre-installed binary");
                }
            }
        }

        let tar_bytes = ar.into_inner()?;

        let mut stream = self.docker.build_image(
            BuildImageOptions {
                t: Some(tag.to_string()),
                rm: true,
                ..Default::default()
            },
            None,
            Some(bollard::body_full(tar_bytes.into())),
        );

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    if let Some(s) = output.stream {
                        debug!("{}", s.trim_end());
                    }
                }
                Err(e) => {
                    anyhow::bail!("Image build failed: {e}");
                }
            }
        }

        info!(%tag, "Worker image built successfully");
        Ok(())
    }

    /// Generate a Dockerfile for a worker matching the given system identity.
    ///
    /// For native builds, uses the matching `gentoo/stage3` variant.
    /// For cross-architecture builds, uses an `amd64` stage3 with `crossdev`
    /// pre-installed so the worker can build for the target CHOST.
    fn generate_dockerfile(&self, sys: &SystemIdentity) -> String {
        let target_arch = sys.chost.split('-').next().unwrap_or("x86_64");

        // Determine if this is a cross-build.
        // The server always runs on amd64 — if the target isn't x86_64, it's cross.
        let is_cross = !matches!(target_arch, "x86_64" | "i686");

        let base_image = if is_cross {
            // For cross-builds we always start from amd64 and set up crossdev.
            "gentoo/stage3:latest".to_string()
        } else {
            // Native build — match the stage3 variant.
            match sys.arch.as_str() {
                "amd64" => "gentoo/stage3:latest".into(),
                "arm64" => "gentoo/stage3:arm64-latest".into(),
                _ => "gentoo/stage3:latest".into(),
            }
        };

        let crossdev_block = if is_cross {
            format!(
                r#"
# ── Crossdev toolchain for {chost} ──────────────────────────
# Install crossdev and build the cross toolchain.
# The emerge-{chost} wrapper is created automatically.
RUN emerge --oneshot --quiet sys-devel/crossdev && \
    crossdev --stable -t {chost}

# Set CHOST and CBUILD so portage knows this is a cross-build.
# (remerge-worker will also write these, but having them in the
# base image speeds up subsequent builds.)
RUN echo 'CHOST="{chost}"' >> /etc/portage/make.conf && \
    echo 'CBUILD="x86_64-pc-linux-gnu"' >> /etc/portage/make.conf
"#,
                chost = sys.chost,
            )
        } else {
            String::new()
        };

        let binary_install = if self.worker_binary.is_some() {
            "# Install remerge-worker binary from build context.\nCOPY remerge-worker /usr/local/bin/remerge-worker\nRUN chmod +x /usr/local/bin/remerge-worker"
        } else {
            "# Worker binary must be pre-installed in the image or mounted at runtime."
        };

        format!(
            r#"FROM {base_image}

{binary_install}

# Set up portage for building.
RUN emerge --sync --quiet || true
{crossdev_block}
# Create binpkg output directory.
RUN mkdir -p {binpkg_mount}

# Default FEATURES for binary package creation.
RUN echo 'FEATURES="buildpkg noclean"' >> /etc/portage/make.conf && \
    echo 'PKGDIR="{binpkg_mount}"' >> /etc/portage/make.conf

WORKDIR /root

# The entrypoint will be the remerge-worker binary.
ENTRYPOINT ["/usr/local/bin/remerge-worker"]
"#,
            base_image = base_image,
            binary_install = binary_install,
            crossdev_block = crossdev_block,
            binpkg_mount = self.worker_binpkg_mount,
        )
    }

    /// Create and start a worker container for the given workorder.
    ///
    /// Returns the container ID.
    pub async fn start_worker(
        &self,
        container_name: &str,
        image_tag: &str,
        workorder_json: &str,
        server_config: &ServerConfig,
    ) -> Result<String> {
        info!(%container_name, %image_tag, "Starting worker container");

        let mut binds = vec![
            // Mount the binpkg output directory.
            format!("{}:{}", self.binpkg_dir, self.worker_binpkg_mount),
        ];
        if let Some(ref gpg_home) = self.gpg_home {
            binds.push(format!("{gpg_home}:/var/cache/remerge/gnupg:ro"));
        }

        let host_config = HostConfig {
            binds: Some(binds),
            ..Default::default()
        };

        let mut env = vec![format!("REMERGE_WORKORDER={workorder_json}")];
        if let Some(ref key) = self.gpg_key {
            env.push(format!("REMERGE_GPG_KEY={key}"));
        }
        if self.gpg_home.is_some() {
            env.push("REMERGE_GPG_HOME=/var/cache/remerge/gnupg".to_string());
        }

        // Pass multithreading configuration to the worker.
        let parallel_jobs = server_config.parallel_jobs.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1)
        });
        let load_average = server_config.load_average.unwrap_or(parallel_jobs as f32);
        env.push(format!("REMERGE_PARALLEL_JOBS={parallel_jobs}"));
        env.push(format!("REMERGE_LOAD_AVERAGE={load_average:.1}"));

        let config = ContainerCreateBody {
            image: Some(image_tag.to_string()),
            env: Some(env),
            host_config: Some(host_config),
            ..Default::default()
        };

        let resp = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(container_name.to_string()),
                    ..Default::default()
                }),
                config,
            )
            .await
            .context("Failed to create worker container")?;

        if let Err(e) = self
            .docker
            .start_container(&resp.id, None::<StartContainerOptions>)
            .await
        {
            // Container was created but failed to start — remove it so the
            // name is freed for future attempts.
            let _ = self
                .docker
                .remove_container(
                    &resp.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
            return Err(e).context("Failed to start worker container");
        }

        info!(id = %resp.id, "Worker container started");
        Ok(resp.id)
    }

    /// Stream logs from a running container.
    pub fn stream_logs(
        &self,
        container_id: &str,
    ) -> impl futures::Stream<Item = Result<String, bollard::errors::Error>> + '_ {
        self.docker
            .logs(
                container_id,
                Some(LogsOptions {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            )
            .map(|r| r.map(|o| o.to_string()))
    }

    /// Wait for a container to finish and return exit code.
    pub async fn wait_container(&self, container_id: &str) -> Result<i64> {
        use bollard::query_parameters::WaitContainerOptions;
        use futures::TryStreamExt;

        let exit = self
            .docker
            .wait_container(
                container_id,
                Some(WaitContainerOptions {
                    condition: String::from("not-running"),
                }),
            )
            .try_next()
            .await?
            .context("Container wait stream ended without result")?;

        Ok(exit.status_code)
    }

    /// Remove a container.
    pub async fn remove_container(&self, container_id: &str) -> Result<()> {
        self.docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .context("Failed to remove container")?;
        Ok(())
    }

    /// Remove a Docker image by tag.
    pub async fn remove_image(&self, tag: &str) -> Result<()> {
        use bollard::query_parameters::RemoveImageOptions;
        self.docker
            .remove_image(
                tag,
                Some(RemoveImageOptions {
                    force: true,
                    ..Default::default()
                }),
                None,
            )
            .await
            .context("Failed to remove image")?;
        Ok(())
    }

    /// Stop a running container (for build cancellation).
    pub async fn stop_container(&self, container_id: &str) -> Result<()> {
        use bollard::query_parameters::StopContainerOptions;
        self.docker
            .stop_container(
                container_id,
                Some(StopContainerOptions {
                    t: Some(10),
                    ..Default::default()
                }),
            )
            .await
            .context("Failed to stop container")?;
        Ok(())
    }

    /// Get the configured maximum number of workers.
    #[allow(dead_code)]
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }
}
