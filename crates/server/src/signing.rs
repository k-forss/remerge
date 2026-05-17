use std::{fs, path::Path, process::Command};

use anyhow::Result;

use crate::config::SigningConfig;

#[derive(Debug, Clone)]
pub struct ExportedSigningKey {
    pub configured_key: String,
    pub fingerprint: String,
    pub armored_key: String,
}

pub fn validate_signing_config(signing: &SigningConfig) -> Result<()> {
    match (signing.gpg_key.as_deref(), signing.gpg_home.as_deref()) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(anyhow::anyhow!(
            "Binary package signing must set both `gpg_key` and `gpg_home` before starting remerge-server."
        )),
        (Some(key), Some(home)) => validate_gpg_key(Path::new(home), key),
    }
}

pub fn export_public_key(signing: &SigningConfig) -> Result<Option<ExportedSigningKey>> {
    match (signing.gpg_key.as_deref(), signing.gpg_home.as_deref()) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(anyhow::anyhow!(
            "Binary package signing must set both `gpg_key` and `gpg_home` before exporting the public key."
        )),
        (Some(key), Some(home)) => {
            let gpg_home = Path::new(home);
            let fingerprint = export_public_fingerprint(gpg_home, key)?;
            let armored_key = export_public_key_block(gpg_home, key)?;
            Ok(Some(ExportedSigningKey {
                configured_key: key.to_string(),
                fingerprint,
                armored_key,
            }))
        }
    }
}

pub fn validate_gpg_key(gpg_home: &Path, gpg_key: &str) -> Result<()> {
    if gpg_key.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "Configured signing key is empty. Set `gpg_key` to the signing key fingerprint or key ID."
        ));
    }

    let metadata = fs::metadata(gpg_home).map_err(|e| {
        anyhow::anyhow!(
            "Configured GPG home {:?} is not readable: {e}. Fix `gpg_home` before starting remerge-server.",
            gpg_home
        )
    })?;

    if !metadata.is_dir() {
        return Err(anyhow::anyhow!(
            "Configured GPG home {:?} is not a directory. Fix `gpg_home` before starting remerge-server.",
            gpg_home
        ));
    }

    let output = Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_home)
        .arg("--batch")
        .arg("--list-secret-keys")
        .arg("--with-colons")
        .arg(gpg_key)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gpg` for signing-key validation: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            "gpg did not find the configured signing key".to_string()
        } else {
            stderr
        };
        return Err(anyhow::anyhow!(
            "Configured signing key {gpg_key:?} is not available in {:?}: {detail}",
            gpg_home
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout
        .lines()
        .any(|line| line.starts_with("sec:") || line.starts_with("ssb:"))
    {
        return Err(anyhow::anyhow!(
            "Configured signing key {gpg_key:?} is not present as a secret key in {:?}.",
            gpg_home
        ));
    }

    tracing::info!(gpg_home = ?gpg_home, gpg_key, "Signing key validated");
    Ok(())
}

fn export_public_fingerprint(gpg_home: &Path, gpg_key: &str) -> Result<String> {
    let output = Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_home)
        .arg("--batch")
        .arg("--list-keys")
        .arg("--with-colons")
        .arg("--fingerprint")
        .arg(gpg_key)
        .output()
        .map_err(|e| {
            anyhow::anyhow!("Failed to run `gpg` for signing-key fingerprint export: {e}")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow::anyhow!(
            "Failed to export signing-key fingerprint for {gpg_key:?} from {:?}: {}",
            gpg_home,
            if stderr.is_empty() {
                "gpg returned a non-zero status"
            } else {
                &stderr
            }
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| line.starts_with("fpr:"))
        .and_then(|line| line.split(':').nth(9))
        .filter(|fingerprint| !fingerprint.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to parse exported signing-key fingerprint for {gpg_key:?} from {:?}.",
                gpg_home
            )
        })
}

fn export_public_key_block(gpg_home: &Path, gpg_key: &str) -> Result<String> {
    let output = Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_home)
        .arg("--batch")
        .arg("--armor")
        .arg("--export-options")
        .arg("export-minimal")
        .arg("--export")
        .arg(gpg_key)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run `gpg` for signing-key export: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow::anyhow!(
            "Failed to export signing public key for {gpg_key:?} from {:?}: {}",
            gpg_home,
            if stderr.is_empty() {
                "gpg returned a non-zero status"
            } else {
                &stderr
            }
        ));
    }

    let armored_key = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("Exported signing public key was not valid UTF-8: {e}"))?;
    if !armored_key.contains("BEGIN PGP PUBLIC KEY BLOCK") {
        return Err(anyhow::anyhow!(
            "Exported signing public key for {gpg_key:?} from {:?} was empty or malformed.",
            gpg_home
        ));
    }

    Ok(armored_key)
}
