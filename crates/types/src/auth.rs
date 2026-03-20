//! Authentication mode.
//!
//! Determines how the server verifies client identity on incoming requests.
//!
//! The remerge server itself does **not** terminate TLS.  When mTLS is used, a
//! reverse proxy (traefik / caddy / nginx) terminates TLS and forwards the
//! client certificate fingerprint in a configurable HTTP header.
//!
//! # Modes
//!
//! - **`none`** — clients self-identify via `client_id` + `role` in the request
//!   body.  No certificate required.  Suitable for trusted networks.
//! - **`mtls`** — every request must carry a valid client-certificate
//!   fingerprint header.  The server looks up the fingerprint in a pre-
//!   configured registry to determine `client_id` and `role`.  Body values
//!   are ignored.
//! - **`mixed`** — main clients require mTLS; followers may self-identify with
//!   just a `client_id`.  A follower that *does* present a cert is
//!   authenticated by it.  Useful when you want strong authentication for
//!   config-pushers but low friction for build consumers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How the server authenticates incoming requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// No authentication — clients self-identify.
    #[default]
    None,
    /// All clients must present a valid client certificate (via reverse proxy).
    Mtls,
    /// Main clients require mTLS; followers may self-identify.
    Mixed,
}

impl fmt::Display for AuthMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Mtls => write!(f, "mtls"),
            Self::Mixed => write!(f, "mixed"),
        }
    }
}

impl std::str::FromStr for AuthMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "mtls" => Ok(Self::Mtls),
            "mixed" => Ok(Self::Mixed),
            other => Err(format!(
                "unknown auth mode '{other}' (expected 'none', 'mtls', or 'mixed')"
            )),
        }
    }
}
