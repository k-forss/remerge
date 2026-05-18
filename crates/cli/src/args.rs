//! CLI argument parsing.
//!
//! `remerge` accepts the exact same arguments as `emerge`.  It intercepts them,
//! builds a workorder, and after the remote build completes runs `emerge`
//! locally with `--getbinpkg` so that pre-built packages are used.

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

use crate::client::RemergeClient;
use crate::config::{self, CliConfig};
use crate::portage::PortageReader;
use remerge_types::client::ClientRole;
use remerge_types::validation::validate_atom;

/// Extract package atoms from emerge arguments.
///
/// Anything that doesn't start with `-` is treated as a potential atom.
/// This covers qualified (`www-client/firefox`), unqualified (`firefox`),
/// versioned (`=www-client/firefox-128.0`), and set (`@world`) forms.
/// Emerge itself will reject truly invalid atoms.
pub fn extract_package_atoms(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect()
}

/// remerge — distributed Gentoo binary host builder.
///
/// Drop-in wrapper for `emerge`.  Forwards your arguments to a remote build
/// worker, waits for binary packages, then runs `emerge` locally with
/// `--getbinpkg` to install them.
#[derive(Parser, Debug)]
#[command(
    name = "remerge",
    version,
    about = "Distributed Gentoo binary host builder",
    long_about = None,
    // Allow arbitrary trailing arguments so we can forward them to emerge.
    trailing_var_arg = true,
)]
pub struct Cli {
    /// URL of the remerge server (overrides /etc/remerge.conf).
    #[arg(long, env = "REMERGE_SERVER")]
    server: Option<String>,

    /// Override the client ID from the config file.
    ///
    /// Use this to explicitly set a client ID, e.g. to share an identity
    /// across multiple machines.
    #[arg(long, env = "REMERGE_CLIENT_ID")]
    client_id: Option<uuid::Uuid>,

    /// Client role: `main` (default) or `follower`.
    ///
    /// Followers share the main client's portage config and cannot push
    /// configuration changes.
    #[arg(long, env = "REMERGE_ROLE")]
    role: Option<ClientRole>,

    /// Path to the CLI configuration file.
    #[arg(long, default_value = config::CONFIG_PATH)]
    config: String,

    /// Only submit the workorder — don't wait or run emerge locally.
    #[arg(long)]
    submit_only: bool,

    /// Don't run emerge locally after the remote build.
    /// Useful if you just want to populate the binhost.
    #[arg(long)]
    no_local: bool,

    /// Print what would be done without actually doing it.
    #[arg(long)]
    dry_run: bool,

    /// Force remote build even for packages that appear to be installed
    /// and up-to-date locally.
    #[arg(long)]
    force: bool,

    /// All remaining arguments are forwarded to emerge.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    emerge_args: Vec<String>,
}

impl Cli {
    /// Parse CLI arguments from `std::env::args`.
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Run the main CLI flow.
    pub async fn run(&self) -> Result<()> {
        if self.emerge_args.is_empty() {
            anyhow::bail!(
                "No emerge arguments provided.  Usage: remerge [OPTIONS] <emerge-args>..."
            );
        }

        // 0. Load persistent config (server URL + client ID).
        let cfg = CliConfig::load_or_create(&self.config).unwrap_or_else(|e| {
            tracing::warn!("Failed to load config: {e:#} — using defaults");
            CliConfig::default()
        });

        // CLI flag / env var overrides the config file.
        let server = self.server.as_deref().unwrap_or(&cfg.server);

        // Client ID: CLI flag > config file.
        let client_id = self.client_id.unwrap_or(cfg.client_id);

        // Role: CLI flag > config file.
        let role = self.role.unwrap_or(cfg.role);

        // 1. Extract package atoms from the emerge arguments.
        let raw_atoms = self.extract_atoms();
        if raw_atoms.is_empty() {
            info!("No package atoms detected — falling through to plain emerge");
            return self.run_emerge_locally(&self.emerge_args, None).await;
        }

        // 1a. Expand set references (@world, @system) into individual atoms.
        let reader_for_sets = PortageReader::new()?;
        let atoms: Vec<String> = raw_atoms
            .into_iter()
            .flat_map(|a| {
                if a.starts_with('@') {
                    reader_for_sets.expand_set(&a)
                } else {
                    vec![a]
                }
            })
            .collect();

        // 1b. Validate all atoms before submitting.
        for atom in &atoms {
            // Sets that couldn't be expanded are passed through verbatim.
            if atom.starts_with('@') {
                continue;
            }
            if let Err(e) = validate_atom(atom) {
                anyhow::bail!("Invalid package atom '{atom}': {e}");
            }
        }

        // 1b. Check if packages are already installed (unless --force).
        let atoms = if self.force {
            atoms
        } else {
            let reader_for_vdb = PortageReader::new()?;
            let mut filtered = Vec::new();
            for atom in atoms {
                if reader_for_vdb.is_installed(&atom) {
                    println!("  ⏭  {atom} — already installed (use --force to rebuild)");
                } else {
                    filtered.push(atom);
                }
            }
            if filtered.is_empty() {
                println!("All packages are already installed. Nothing to do.");
                return Ok(());
            }
            filtered
        };

        info!(
            ?atoms,
            client_id = %client_id,
            %role,
            "Packages to build remotely"
        );

        // 2. Read local portage configuration.
        let reader = PortageReader::new()?;
        let portage_config = reader
            .read_config()
            .context("Failed to read portage configuration")?;
        let system_id = reader
            .read_system_identity()
            .context("Failed to determine system identity")?;

        if self.dry_run {
            println!("Would submit workorder for: {}", atoms.join(", "));
            println!("  Server:    {server}");
            println!("  Client ID: {}", client_id);
            println!("  Role:      {}", role);
            println!("  Profile:   {}", system_id.profile);
            println!("  Arch:      {}", system_id.arch);
            println!("  CHOST:     {}", system_id.chost);
            println!("  CFLAGS:    {}", portage_config.make_conf.cflags);
            if let Some(ref orig) = portage_config.make_conf.original_cflags {
                println!("    (was:    {})", orig);
            }
            if let Some((ref var, ref flags)) = portage_config.make_conf.cpu_flags {
                println!("  {var}: {}", flags.join(" "));
            }
            println!(
                "  USE:       {}",
                portage_config.make_conf.use_flags.join(" ")
            );
            return Ok(());
        }

        // 3. Submit workorder to server.
        let client = RemergeClient::new(server)?;
        let resp = client
            .submit_workorder(
                client_id,
                role,
                &atoms,
                &self.emerge_args,
                &portage_config,
                &system_id,
            )
            .await
            .context("Failed to submit workorder")?;

        println!(
            "Workorder {} submitted — streaming progress…",
            resp.workorder_id
        );
        if let Some(trace_id) = &resp.trace_id {
            println!("Trace ID: {trace_id}");
        }

        if self.submit_only {
            println!("Workorder ID: {}", resp.workorder_id);
            return Ok(());
        }

        // 4. Stream build progress via WebSocket.
        let result = client
            .stream_progress(&resp.progress_ws_url)
            .await
            .context("Failed to stream build progress")?;

        // 5. Report results.
        println!("\n─── Build complete ───");
        println!(
            "  Built: {}",
            result
                .built_packages
                .iter()
                .map(|p| p.atom.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !result.failed_packages.is_empty() {
            println!(
                "  Failed: {}",
                result
                    .failed_packages
                    .iter()
                    .map(|p| p.atom.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            anyhow::bail!(
                "Remote build failed: {} package(s) failed",
                result.failed_packages.len()
            );
        }

        if self.no_local {
            return Ok(());
        }

        // 6. Run emerge locally with --getbinpkg.
        println!("\nRunning emerge locally with binary packages…\n");
        let local_args = Self::build_local_emerge_args(&self.emerge_args);
        self.run_emerge_locally(&local_args, Some(&result.binhost_uri))
            .await
    }

    fn extract_atoms(&self) -> Vec<String> {
        extract_package_atoms(&self.emerge_args)
    }

    fn build_local_emerge_args(emerge_args: &[String]) -> Vec<String> {
        let mut local_args = vec!["--getbinpkg".to_string(), "--usepkg".to_string()];
        local_args.extend(emerge_args.iter().cloned());
        local_args
    }

    fn portage_binhost_env(binhost_uri: Option<&str>) -> Option<String> {
        let uri = binhost_uri?.trim();
        if uri.is_empty() {
            None
        } else {
            Some(uri.to_string())
        }
    }

    /// Run emerge as a child process with the given arguments.
    async fn run_emerge_locally(&self, args: &[String], binhost_uri: Option<&str>) -> Result<()> {
        use tokio::process::Command;

        let mut command = Command::new("emerge");
        command.args(args);

        if let Some(portage_binhost) = Self::portage_binhost_env(binhost_uri) {
            command.env("PORTAGE_BINHOST", portage_binhost);
        }

        let status = command.status().await.context("Failed to execute emerge")?;

        if !status.success() {
            anyhow::bail!("emerge exited with status {}", status);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, extract_package_atoms};

    #[test]
    fn extract_package_atoms_preserves_sets_and_versioned_atoms() {
        let args = vec![
            "--ask".to_string(),
            "@world".to_string(),
            "=dev-libs/openssl-3.1.4".to_string(),
            "--with-bdeps=y".to_string(),
            "firefox".to_string(),
        ];

        assert_eq!(
            extract_package_atoms(&args),
            vec![
                "@world".to_string(),
                "=dev-libs/openssl-3.1.4".to_string(),
                "firefox".to_string(),
            ]
        );
    }

    #[test]
    fn extract_package_atoms_ignores_option_like_values() {
        let args = vec![
            "--jobs=8".to_string(),
            "--keep-going".to_string(),
            "-av".to_string(),
        ];

        assert!(extract_package_atoms(&args).is_empty());
    }

    #[test]
    fn pc_010_local_install_binhost_handoff_contract() {
        let args = vec!["dev-libs/openssl".to_string(), "--ask".to_string()];
        let local_args = Cli::build_local_emerge_args(&args);

        assert_eq!(
            local_args,
            vec![
                "--getbinpkg".to_string(),
                "--usepkg".to_string(),
                "dev-libs/openssl".to_string(),
                "--ask".to_string(),
            ]
        );
        assert_eq!(
            Cli::portage_binhost_env(Some("https://binhost.example.invalid/binpkgs")),
            Some("https://binhost.example.invalid/binpkgs".to_string())
        );
        assert_eq!(Cli::portage_binhost_env(Some("   ")), None);
    }
}
