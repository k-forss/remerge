//! Client authentication — resolves client identity from mTLS headers or
//! request body.
//!
//! The remerge server sits behind a TLS-terminating reverse proxy
//! (traefik / caddy / nginx).  When mTLS is enabled on the proxy, it
//! validates the client certificate and forwards the fingerprint in a
//! configurable HTTP header (default: `X-Client-Cert-Fingerprint`).
//!
//! The server maintains a registry of known certificate fingerprints, each
//! mapped to a [`ClientId`] and [`ClientRole`].  On every request the
//! [`CertRegistry::resolve`] method determines the effective client identity
//! based on the configured [`AuthMode`].

use std::collections::HashMap;
use std::fmt;

use axum::http::{HeaderMap, HeaderName};
use serde::{Deserialize, Serialize};

use remerge_types::auth::AuthMode;
use remerge_types::client::{ClientId, ClientRole};

/// Default header name used by the reverse proxy to forward the client
/// certificate SHA-256 fingerprint.
pub const DEFAULT_CERT_HEADER: &str = "X-Client-Cert-Fingerprint";

// ─── Configuration ──────────────────────────────────────────────────

/// Authentication section of the server configuration.
///
/// ```toml
/// [auth]
/// mode = "mtls"
/// cert_header = "X-Client-Cert-Fingerprint"
///
/// [[auth.clients]]
/// fingerprint = "sha256:AB:CD:EF:..."
/// client_id = "550e8400-e29b-41d4-a716-446655440000"
/// role = "main"
/// label = "build-master-01"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Authentication mode.
    #[serde(default)]
    pub mode: AuthMode,

    /// HTTP header that carries the client certificate SHA-256 fingerprint.
    ///
    /// Populated by the reverse proxy when mTLS is enabled.
    #[serde(default = "default_cert_header")]
    pub cert_header: String,

    /// Registered client certificates.
    #[serde(default)]
    pub clients: Vec<CertEntry>,
}

fn default_cert_header() -> String {
    DEFAULT_CERT_HEADER.into()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            cert_header: default_cert_header(),
            clients: Vec::new(),
        }
    }
}

/// A registered client certificate mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertEntry {
    /// SHA-256 fingerprint of the client certificate (hex-encoded).
    ///
    /// May optionally be prefixed with `sha256:` and may contain `:` or
    /// space separators.  All forms are normalised to lowercase hex during
    /// lookup.
    pub fingerprint: String,

    /// Client ID to assign when this certificate is presented.
    pub client_id: ClientId,

    /// Role to assign (`main` or `follower`).
    pub role: ClientRole,

    /// Optional human-readable label for logging.
    #[serde(default)]
    pub label: Option<String>,
}

// ─── Runtime Registry ───────────────────────────────────────────────

/// In-memory lookup table built from [`AuthConfig`].
///
/// Constructed once at server start-up and shared (immutably) across
/// request handlers via [`AppState`](crate::state::AppState).
pub struct CertRegistry {
    mode: AuthMode,
    cert_header: HeaderName,
    /// Fingerprint (normalised, lowercase hex, no separators) → entry.
    entries: HashMap<String, CertEntry>,
}

impl CertRegistry {
    /// Build a registry from the auth configuration.
    pub fn new(config: &AuthConfig) -> Self {
        let entries: HashMap<String, CertEntry> = config
            .clients
            .iter()
            .map(|e| (normalise_fingerprint(&e.fingerprint), e.clone()))
            .collect();

        if !entries.is_empty() {
            tracing::info!(
                mode = %config.mode,
                registered_certs = entries.len(),
                "Certificate registry initialised"
            );
        }

        let cert_header = config
            .cert_header
            .parse::<HeaderName>()
            .unwrap_or_else(|_| {
                tracing::warn!(
                    header = %config.cert_header,
                    "Invalid cert header name — falling back to default"
                );
                HeaderName::from_static("x-client-cert-fingerprint")
            });

        Self {
            mode: config.mode,
            cert_header,
            entries,
        }
    }

    /// The configured authentication mode.
    pub fn mode(&self) -> AuthMode {
        self.mode
    }

    /// Resolve the effective client identity for a request.
    ///
    /// Depending on [`AuthMode`]:
    ///
    /// - **`None`** — returns the body-supplied identity unchanged.
    /// - **`Mtls`** — requires a valid certificate header; body identity is
    ///   ignored.
    /// - **`Mixed`** — uses certificate identity when present; otherwise
    ///   allows the body identity but forces `role = Follower`.
    pub fn resolve(
        &self,
        headers: &HeaderMap,
        body_client_id: ClientId,
        body_role: ClientRole,
    ) -> Result<ResolvedIdentity, AuthError> {
        let cert_fp = self.extract_fingerprint(headers);

        match self.mode {
            // ── No auth — pass through ──────────────────────────────
            AuthMode::None => Ok(ResolvedIdentity {
                client_id: body_client_id,
                role: body_role,
                method: AuthMethod::SelfDeclared,
            }),

            // ── Full mTLS — cert required for everyone ──────────────
            AuthMode::Mtls => {
                let fp = cert_fp.ok_or(AuthError::CertificateRequired)?;
                let entry = self
                    .entries
                    .get(&fp)
                    .ok_or(AuthError::UnknownCertificate(fp))?;
                Ok(ResolvedIdentity {
                    client_id: entry.client_id,
                    role: entry.role,
                    method: AuthMethod::Certificate {
                        fingerprint: normalise_fingerprint(&entry.fingerprint),
                        label: entry.label.clone(),
                    },
                })
            }

            // ── Mixed — cert for mains, optional for followers ──────
            AuthMode::Mixed => match cert_fp {
                Some(fp) => {
                    let entry = self
                        .entries
                        .get(&fp)
                        .ok_or(AuthError::UnknownCertificate(fp))?;
                    Ok(ResolvedIdentity {
                        client_id: entry.client_id,
                        role: entry.role,
                        method: AuthMethod::Certificate {
                            fingerprint: normalise_fingerprint(&entry.fingerprint),
                            label: entry.label.clone(),
                        },
                    })
                }
                None => {
                    // No cert — only followers are allowed without one.
                    if body_role == ClientRole::Main {
                        Err(AuthError::MainRequiresCert)
                    } else {
                        Ok(ResolvedIdentity {
                            client_id: body_client_id,
                            role: ClientRole::Follower,
                            method: AuthMethod::SelfDeclared,
                        })
                    }
                }
            },
        }
    }

    /// Extract the certificate fingerprint from the configured header.
    ///
    /// Returns `None` if the header is absent, empty, or not valid UTF-8.
    fn extract_fingerprint(&self, headers: &HeaderMap) -> Option<String> {
        let raw = headers.get(&self.cert_header)?.to_str().ok()?;
        let normalised = normalise_fingerprint(raw);
        if normalised.is_empty() {
            return None;
        }
        Some(normalised)
    }

    /// Attempt to resolve a client ID from the certificate header alone,
    /// without requiring a request body.
    ///
    /// Used for authenticating read/cancel endpoints (GET, DELETE) where
    /// the request has no JSON body.
    pub fn resolve_header_only(&self, headers: &HeaderMap) -> Option<ClientId> {
        let fp = self.extract_fingerprint(headers)?;
        self.entries.get(&fp).map(|entry| entry.client_id)
    }
}

// ─── Types ──────────────────────────────────────────────────────────

/// The authenticated client identity after going through auth resolution.
#[derive(Debug, Clone)]
pub struct ResolvedIdentity {
    pub client_id: ClientId,
    pub role: ClientRole,
    pub method: AuthMethod,
}

/// How the client was authenticated.
#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// Client self-declared identity (no certificate).
    SelfDeclared,
    /// Authenticated via client certificate fingerprint.
    Certificate {
        fingerprint: String,
        label: Option<String>,
    },
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfDeclared => write!(f, "self-declared"),
            Self::Certificate {
                label: Some(label), ..
            } => write!(f, "certificate ({label})"),
            Self::Certificate { fingerprint, .. } => {
                write!(
                    f,
                    "certificate ({}…)",
                    &fingerprint[..12.min(fingerprint.len())]
                )
            }
        }
    }
}

/// Authentication error.
#[derive(Debug, Clone)]
pub enum AuthError {
    /// mTLS is required but no valid client certificate was presented.
    CertificateRequired,
    /// In mixed mode, main clients must authenticate via mTLS.
    MainRequiresCert,
    /// A certificate was presented but is not registered on the server.
    UnknownCertificate(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CertificateRequired => {
                write!(f, "client certificate required (mTLS mode)")
            }
            Self::MainRequiresCert => {
                write!(f, "main clients must authenticate via mTLS (mixed mode)")
            }
            Self::UnknownCertificate(fp) => {
                write!(f, "unknown client certificate: {fp}")
            }
        }
    }
}

impl AuthError {
    /// Map to an appropriate HTTP status code.
    pub fn status_code(&self) -> axum::http::StatusCode {
        match self {
            Self::CertificateRequired | Self::UnknownCertificate(_) => {
                axum::http::StatusCode::UNAUTHORIZED
            }
            Self::MainRequiresCert => axum::http::StatusCode::FORBIDDEN,
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

/// Normalise a certificate fingerprint to lowercase hex without separators.
///
/// Handles common formats:
/// - `sha256:AB:CD:EF:...` → `abcdef...`
/// - `SHA256:AB:CD:EF:...` → `abcdef...`
/// - `AB:CD:EF:...`        → `abcdef...`
/// - `ab cd ef ...`        → `abcdef...`
/// - `abcdef...`           → `abcdef...` (already normalised)
fn normalise_fingerprint(fp: &str) -> String {
    let lower = fp.trim().to_lowercase();
    // Strip optional sha256: prefix (already lowercased).
    let without_prefix = lower.strip_prefix("sha256:").unwrap_or(&lower);
    without_prefix.replace([':', ' '], "")
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_config(mode: AuthMode, entries: Vec<CertEntry>) -> AuthConfig {
        AuthConfig {
            mode,
            cert_header: DEFAULT_CERT_HEADER.into(),
            clients: entries,
        }
    }

    fn main_entry() -> CertEntry {
        CertEntry {
            fingerprint: "sha256:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99".into(),
            client_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            role: ClientRole::Main,
            label: Some("build-master".into()),
        }
    }

    fn follower_entry() -> CertEntry {
        CertEntry {
            fingerprint: "ff:ee:dd:cc".into(),
            client_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            role: ClientRole::Follower,
            label: Some("build-node-02".into()),
        }
    }

    fn headers_with_cert(fp: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            DEFAULT_CERT_HEADER.parse::<HeaderName>().unwrap(),
            fp.parse().unwrap(),
        );
        headers
    }

    // ── normalise_fingerprint ───────────────────────────────────────

    #[test]
    fn normalise_strips_prefix_and_colons() {
        assert_eq!(normalise_fingerprint("sha256:AB:CD:EF:01"), "abcdef01");
    }

    #[test]
    fn normalise_handles_uppercase_prefix() {
        assert_eq!(normalise_fingerprint("SHA256:AB:CD:EF"), "abcdef");
    }

    #[test]
    fn normalise_handles_mixed_case_prefix() {
        assert_eq!(normalise_fingerprint("Sha256:AB:CD:EF"), "abcdef");
    }

    #[test]
    fn normalise_handles_spaces() {
        assert_eq!(normalise_fingerprint("ab cd ef"), "abcdef");
    }

    #[test]
    fn normalise_already_clean() {
        assert_eq!(normalise_fingerprint("abcdef01"), "abcdef01");
    }

    #[test]
    fn normalise_empty() {
        assert_eq!(normalise_fingerprint(""), "");
        assert_eq!(normalise_fingerprint("   "), "");
    }

    // ── AuthMode::None ──────────────────────────────────────────────

    #[test]
    fn none_mode_passes_body_identity() {
        let reg = CertRegistry::new(&make_config(AuthMode::None, vec![main_entry()]));
        let body_id = Uuid::new_v4();

        let id = reg
            .resolve(&HeaderMap::new(), body_id, ClientRole::Main)
            .unwrap();

        assert_eq!(id.client_id, body_id);
        assert_eq!(id.role, ClientRole::Main);
        assert!(matches!(id.method, AuthMethod::SelfDeclared));
    }

    #[test]
    fn none_mode_ignores_cert_header() {
        let entry = main_entry();
        let reg = CertRegistry::new(&make_config(AuthMode::None, vec![entry.clone()]));
        let body_id = Uuid::new_v4();

        // Even with a valid cert header, body identity is used.
        let headers = headers_with_cert(&entry.fingerprint);
        let id = reg
            .resolve(&headers, body_id, ClientRole::Follower)
            .unwrap();

        assert_eq!(id.client_id, body_id);
        assert_eq!(id.role, ClientRole::Follower);
    }

    // ── AuthMode::Mtls ──────────────────────────────────────────────

    #[test]
    fn mtls_mode_uses_cert_identity() {
        let entry = main_entry();
        let reg = CertRegistry::new(&make_config(AuthMode::Mtls, vec![entry.clone()]));
        let headers = headers_with_cert(&entry.fingerprint);

        // Body identity is ignored — cert identity wins.
        let body_id = Uuid::new_v4();
        let id = reg
            .resolve(&headers, body_id, ClientRole::Follower)
            .unwrap();

        assert_eq!(id.client_id, entry.client_id);
        assert_eq!(id.role, ClientRole::Main);
        assert!(matches!(id.method, AuthMethod::Certificate { .. }));
    }

    #[test]
    fn mtls_mode_rejects_missing_cert() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mtls, vec![main_entry()]));

        let result = reg.resolve(&HeaderMap::new(), Uuid::new_v4(), ClientRole::Main);
        assert!(matches!(result, Err(AuthError::CertificateRequired)));
    }

    #[test]
    fn mtls_mode_rejects_unknown_cert() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mtls, vec![main_entry()]));
        let headers = headers_with_cert("00:00:00:00");

        let result = reg.resolve(&headers, Uuid::new_v4(), ClientRole::Main);
        assert!(matches!(result, Err(AuthError::UnknownCertificate(_))));
    }

    #[test]
    fn mtls_mode_rejects_empty_header() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mtls, vec![main_entry()]));
        let headers = headers_with_cert("");

        let result = reg.resolve(&headers, Uuid::new_v4(), ClientRole::Main);
        assert!(matches!(result, Err(AuthError::CertificateRequired)));
    }

    // ── AuthMode::Mixed ─────────────────────────────────────────────

    #[test]
    fn mixed_mode_cert_overrides_body() {
        let entry = main_entry();
        let reg = CertRegistry::new(&make_config(AuthMode::Mixed, vec![entry.clone()]));
        let headers = headers_with_cert(&entry.fingerprint);

        let body_id = Uuid::new_v4();
        let id = reg
            .resolve(&headers, body_id, ClientRole::Follower)
            .unwrap();

        assert_eq!(id.client_id, entry.client_id);
        assert_eq!(id.role, ClientRole::Main);
    }

    #[test]
    fn mixed_mode_follower_without_cert_allowed() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mixed, vec![main_entry()]));
        let body_id = Uuid::new_v4();

        let id = reg
            .resolve(&HeaderMap::new(), body_id, ClientRole::Follower)
            .unwrap();

        assert_eq!(id.client_id, body_id);
        assert_eq!(id.role, ClientRole::Follower);
        assert!(matches!(id.method, AuthMethod::SelfDeclared));
    }

    #[test]
    fn mixed_mode_main_without_cert_rejected() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mixed, vec![main_entry()]));

        let result = reg.resolve(&HeaderMap::new(), Uuid::new_v4(), ClientRole::Main);
        assert!(matches!(result, Err(AuthError::MainRequiresCert)));
    }

    #[test]
    fn mixed_mode_unknown_cert_rejected() {
        let reg = CertRegistry::new(&make_config(AuthMode::Mixed, vec![main_entry()]));
        let headers = headers_with_cert("de:ad:be:ef");

        let result = reg.resolve(&headers, Uuid::new_v4(), ClientRole::Follower);
        assert!(matches!(result, Err(AuthError::UnknownCertificate(_))));
    }

    #[test]
    fn mixed_mode_follower_with_valid_cert_authenticated() {
        let entry = follower_entry();
        let reg = CertRegistry::new(&make_config(
            AuthMode::Mixed,
            vec![main_entry(), entry.clone()],
        ));
        let headers = headers_with_cert(&entry.fingerprint);

        let body_id = Uuid::new_v4();
        let id = reg.resolve(&headers, body_id, ClientRole::Main).unwrap();

        // Cert identity wins — role comes from the cert entry, not the body.
        assert_eq!(id.client_id, entry.client_id);
        assert_eq!(id.role, ClientRole::Follower);
    }

    // ── Fingerprint normalisation in lookup ─────────────────────────

    #[test]
    fn lookup_normalises_header_value() {
        let entry = main_entry();
        let reg = CertRegistry::new(&make_config(AuthMode::Mtls, vec![entry.clone()]));

        // Header has same fingerprint but formatted differently (no prefix,
        // different case).
        let raw = entry
            .fingerprint
            .strip_prefix("sha256:")
            .unwrap()
            .to_uppercase();
        let headers = headers_with_cert(&raw);

        let id = reg
            .resolve(&headers, Uuid::new_v4(), ClientRole::Main)
            .unwrap();
        assert_eq!(id.client_id, entry.client_id);
    }
}
