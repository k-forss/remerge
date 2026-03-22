//! Crossdev toolchain management for cross-architecture builds.
//!
//! When the worker's host architecture differs from the target `CHOST`,
//! we need to set up a crossdev toolchain so that `emerge-<CHOST>` can build
//! packages for the target architecture.
//!
//! For example, building `aarch64-unknown-linux-gnu` packages on an `amd64`
//! worker requires:
//!
//! ```bash
//! crossdev --stable -t aarch64-unknown-linux-gnu
//! # Then use emerge-aarch64-unknown-linux-gnu instead of emerge.
//! ```

use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

/// Determine whether cross-compilation is needed and, if so, what command to
/// use instead of plain `emerge`.
///
/// Returns `(emerge_command, is_cross)`:
/// - `("emerge".into(), false)` for native builds.
/// - `("emerge-<chost>".into(), true)` for cross builds.
pub fn emerge_command(worker_chost: &str, target_chost: &str) -> (String, bool) {
    // Normalise the comparison — strip minor differences like
    // `x86_64-pc-linux-gnu` vs `x86_64-unknown-linux-gnu`.
    let worker_arch = chost_arch(worker_chost);
    let target_arch = chost_arch(target_chost);

    if worker_arch == target_arch {
        // Same architecture — native build.
        ("emerge".into(), false)
    } else {
        // Different architecture — cross-build.
        // The crossdev emerge wrapper is named `emerge-<CHOST>`.
        (format!("emerge-{target_chost}"), true)
    }
}

/// Install a crossdev toolchain for the given target CHOST.
///
/// This runs `crossdev --stable -t <chost>` which installs the full cross
/// toolchain (binutils, gcc, glibc, linux-headers) and creates the
/// `emerge-<chost>` wrapper script.
pub async fn setup_crossdev(target_chost: &str) -> Result<()> {
    info!(target = %target_chost, "Setting up crossdev toolchain");

    // First, ensure crossdev itself is installed.
    ensure_crossdev_installed().await?;

    // Install the cross toolchain.
    let status = Command::new("crossdev")
        .args(["--stable", "-t", target_chost])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("Failed to run crossdev")?;

    if !status.success() {
        anyhow::bail!("crossdev --stable -t {target_chost} failed with status {status}");
    }

    // Verify the emerge wrapper was created.
    let wrapper = format!("emerge-{target_chost}");
    let check = Command::new("which").arg(&wrapper).output().await;

    match check {
        Ok(output) if output.status.success() => {
            info!(wrapper = %wrapper, "Crossdev emerge wrapper available");
        }
        _ => {
            warn!(
                wrapper = %wrapper,
                "Crossdev finished but emerge wrapper not found — \
                 the build may still work if the toolchain is in PATH"
            );
        }
    }

    Ok(())
}

/// Ensure `crossdev` is installed in the worker container.
async fn ensure_crossdev_installed() -> Result<()> {
    let check = Command::new("which").arg("crossdev").output().await;

    match check {
        Ok(output) if output.status.success() => {
            info!("crossdev is already installed");
            return Ok(());
        }
        _ => {
            info!("Installing crossdev...");
        }
    }

    let status = Command::new("emerge")
        .args(["--oneshot", "--quiet", "sys-devel/crossdev"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("Failed to install crossdev")?;

    if !status.success() {
        anyhow::bail!("Failed to install crossdev (emerge returned {status})");
    }

    Ok(())
}

/// Extract the primary architecture from a CHOST tuple.
///
/// ```text
/// chost_arch("x86_64-pc-linux-gnu")      => "x86_64"
/// chost_arch("aarch64-unknown-linux-gnu") => "aarch64"
/// chost_arch("armv7a-unknown-linux-gnueabihf") => "armv7a"
/// ```
fn chost_arch(chost: &str) -> &str {
    chost.split('-').next().unwrap_or(chost)
}

/// Determine the CHOST of the worker container (the build machine).
///
/// This is the `CBUILD` in cross-compilation terminology.
pub async fn detect_worker_chost() -> Result<String> {
    // Try gcc -dumpmachine first (most reliable inside a container).
    let output = Command::new("gcc")
        .arg("-dumpmachine")
        .output()
        .await
        .context("Failed to run gcc -dumpmachine")?;

    if output.status.success() {
        let chost = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !chost.is_empty() {
            return Ok(chost);
        }
    }

    // Fallback: uname-based guess.
    let output = Command::new("uname")
        .arg("-m")
        .output()
        .await
        .context("Failed to run uname")?;

    let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let chost = match arch.as_str() {
        "x86_64" => "x86_64-pc-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => {
            warn!(arch = %other, "Unknown worker architecture");
            return Ok(format!("{other}-unknown-linux-gnu"));
        }
    };

    Ok(chost.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_build_detection() {
        let (cmd, is_cross) = emerge_command("x86_64-pc-linux-gnu", "x86_64-pc-linux-gnu");
        assert_eq!(cmd, "emerge");
        assert!(!is_cross);
    }

    #[test]
    fn native_build_minor_chost_diff() {
        // Same arch, different vendor field — should still be native.
        let (cmd, is_cross) = emerge_command("x86_64-pc-linux-gnu", "x86_64-unknown-linux-gnu");
        assert_eq!(cmd, "emerge");
        assert!(!is_cross);
    }

    #[test]
    fn cross_build_detection() {
        let (cmd, is_cross) = emerge_command("x86_64-pc-linux-gnu", "aarch64-unknown-linux-gnu");
        assert_eq!(&cmd, "emerge-aarch64-unknown-linux-gnu");
        assert!(is_cross);
    }

    #[test]
    fn chost_arch_extraction() {
        assert_eq!(chost_arch("x86_64-pc-linux-gnu"), "x86_64");
        assert_eq!(chost_arch("aarch64-unknown-linux-gnu"), "aarch64");
        assert_eq!(chost_arch("armv7a-unknown-linux-gnueabihf"), "armv7a");
    }
}
