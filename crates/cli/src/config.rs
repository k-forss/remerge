//! CLI configuration file (`/etc/remerge.conf`).
//!
//! Manages a persistent TOML config that stores:
//!
//! - **`server`** — URL of the remerge server.
//! - **`client_id`** — UUID that identifies this machine (or group of machines).
//! - **`role`** — `main` (default) or `follower`.
//!
//! The `client_id` can be:
//! - auto-generated on first run (the default),
//! - explicitly set to share an identity across machines, or
//! - overridden via `--client-id` on the command line.
//!
//! When multiple machines share the same `client_id`, one must be `role = "main"`
//! and the others `role = "follower"`.  Only the main client may push
//! configuration changes; followers reuse the existing configuration but can
//! still request package builds.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use remerge_types::client::{ClientId, ClientRole};

/// Default config file path.
pub const CONFIG_PATH: &str = "/etc/remerge.conf";

/// CLI configuration loaded from `/etc/remerge.conf`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// URL of the remerge server (e.g. `http://remerge.example.com:7654`).
    #[serde(default = "default_server")]
    pub server: String,

    /// Persistent client identifier.
    ///
    /// Auto-generated on first run.  Set this explicitly to share an identity
    /// across multiple machines (see [`role`](Self::role)).
    #[serde(default = "generate_client_id")]
    pub client_id: ClientId,

    /// Role within the client-ID group.
    ///
    /// - `main` (default) — may push portage configuration and submit builds.
    /// - `follower` — may submit builds but uses the main client's config.
    #[serde(default)]
    pub role: ClientRole,
}

fn default_server() -> String {
    "http://localhost:7654".into()
}

fn generate_client_id() -> ClientId {
    Uuid::new_v4()
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            client_id: generate_client_id(),
            role: ClientRole::default(),
        }
    }
}

impl CliConfig {
    /// Load the config from disk, or create it with defaults if it doesn't
    /// exist yet.
    ///
    /// If the file exists but doesn't contain a `client_id`, one is generated
    /// and the file is rewritten so the ID persists.
    pub fn load_or_create(path: &str) -> Result<Self> {
        let p = Path::new(path);

        if p.exists() {
            let content =
                std::fs::read_to_string(p).with_context(|| format!("Failed to read {path}"))?;

            let mut config: Self =
                toml::from_str(&content).with_context(|| format!("Failed to parse {path}"))?;

            // Persist client_id if it wasn't in the file (serde default
            // generates a fresh UUID each time, so we must write it back).
            if !content.contains("client_id") || config.client_id.is_nil() {
                if config.client_id.is_nil() {
                    config.client_id = generate_client_id();
                }
                config.save(path)?;
            }

            Ok(config)
        } else {
            let config = Self::default();
            config.save(path)?;
            Ok(config)
        }
    }

    /// Write the config back to disk.
    fn save(&self, path: &str) -> Result<()> {
        let content = format!(
            "# remerge CLI configuration\n\
             #\n\
             # server    — URL of the remerge server.\n\
             # client_id — identifies this machine (or group) to the server.\n\
             # role      — \"main\" (pushes config) or \"follower\" (reuses config).\n\
             \n\
             server = \"{}\"\n\
             client_id = \"{}\"\n\
             role = \"{}\"\n",
            self.server, self.client_id, self.role,
        );

        // Try to write — may fail if not root.  That's OK; we'll use the
        // defaults in memory.
        match std::fs::write(path, &content) {
            Ok(()) => {
                tracing::info!("Wrote config to {path}");
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Could not write {path}: {e} — using in-memory defaults");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_valid_client_id() {
        let config = CliConfig::default();
        assert!(!config.client_id.is_nil());
        assert_eq!(config.server, "http://localhost:7654");
        assert_eq!(config.role, ClientRole::Main);
    }

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"server = "http://remerge.lan:7654""#;
        let config: CliConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server, "http://remerge.lan:7654");
        // client_id should be auto-generated via serde default.
        assert!(!config.client_id.is_nil());
        // role defaults to main.
        assert_eq!(config.role, ClientRole::Main);
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
server = "http://remerge.lan:7654"
client_id = "550e8400-e29b-41d4-a716-446655440000"
role = "follower"
"#;
        let config: CliConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server, "http://remerge.lan:7654");
        assert_eq!(
            config.client_id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(config.role, ClientRole::Follower);
    }

    #[test]
    fn parse_shared_client_id() {
        // Two machines sharing a client ID — one main, one follower.
        let main_toml = r#"
server = "http://remerge.lan:7654"
client_id = "550e8400-e29b-41d4-a716-446655440000"
role = "main"
"#;
        let follower_toml = r#"
server = "http://remerge.lan:7654"
client_id = "550e8400-e29b-41d4-a716-446655440000"
role = "follower"
"#;
        let main: CliConfig = toml::from_str(main_toml).unwrap();
        let follower: CliConfig = toml::from_str(follower_toml).unwrap();
        assert_eq!(main.client_id, follower.client_id);
        assert_eq!(main.role, ClientRole::Main);
        assert_eq!(follower.role, ClientRole::Follower);
    }

    #[test]
    fn load_or_create_persists_client_id() {
        // Simulate the ebuild-installed config (no client_id).
        let dir = std::env::temp_dir().join(format!("remerge-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remerge.conf");
        let initial = "server = \"http://localhost:7654\"\n";
        std::fs::write(&path, initial).unwrap();

        let path_str = path.to_str().unwrap();
        let cfg1 = CliConfig::load_or_create(path_str).unwrap();
        let cfg2 = CliConfig::load_or_create(path_str).unwrap();

        // The client_id should be persisted and identical across loads.
        assert_eq!(cfg1.client_id, cfg2.client_id);
        assert!(!cfg1.client_id.is_nil());

        // The file should now contain client_id.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("client_id"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
