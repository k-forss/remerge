//! Compiler flag resolution.
//!
//! Translates `-march=native` (and friends) into concrete micro-architecture
//! flags so the worker container — which runs on a different machine — produces
//! binaries compatible with the *requesting* host.
//!
//! Also detects `CPU_FLAGS_*` via `cpuid2cpuflags` when available.

use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Result of resolving "native" flags on the current system.
#[derive(Debug)]
pub struct ResolvedFlags {
    /// The resolved `-march=` value (e.g. `skylake`, `znver3`), if detected.
    pub march: Option<String>,

    /// CPU capability flags — e.g. `("CPU_FLAGS_X86", ["aes", "avx", "avx2", …])`.
    pub cpu_flags: Option<(String, Vec<String>)>,

    /// The `CHOST` tuple detected on this system.
    pub chost: String,
}

/// Resolve compiler and CPU flags on the running system.
///
/// This function:
/// 1. Asks GCC what `-march=native` expands to.
/// 2. Runs `cpuid2cpuflags` (if available) to get `CPU_FLAGS_*`.
/// 3. Detects `CHOST` from `gcc -dumpmachine` or portageq.
pub fn resolve_native_flags() -> Result<ResolvedFlags> {
    let march = resolve_march_native();
    let cpu_flags = resolve_cpu_flags();
    let chost = detect_chost()?;

    if let Some(ref m) = march {
        info!(march = %m, "Resolved -march=native");
    }
    if let Some((ref var, ref flags)) = cpu_flags {
        info!(%var, flags = flags.join(" "), "Resolved CPU flags");
    }
    info!(%chost, "Detected CHOST");

    Ok(ResolvedFlags {
        march,
        cpu_flags,
        chost,
    })
}

/// Resolve `-march=native` to a concrete micro-architecture name.
///
/// Strategy:
/// 1. Try `resolve-march-native` (Gentoo utility from app-misc/resolve-march-native).
/// 2. Fall back to asking GCC directly via `-Q --help=target`.
fn resolve_march_native() -> Option<String> {
    // Strategy 1: resolve-march-native utility (most accurate on Gentoo).
    if let Some(march) = try_resolve_march_native_utility() {
        return Some(march);
    }

    // Strategy 2: Ask GCC.
    if let Some(march) = try_gcc_march_native() {
        return Some(march);
    }

    warn!(
        "Could not resolve -march=native — the worker will use generic flags. \
         Install app-misc/resolve-march-native for best results."
    );
    None
}

/// Try the `resolve-march-native` Gentoo utility.
///
/// It outputs the resolved CFLAGS, e.g.:
/// ```text
/// -march=skylake -mmmx -mpopcnt …
/// ```
fn try_resolve_march_native_utility() -> Option<String> {
    let output = Command::new("resolve-march-native").output().ok()?;

    if !output.status.success() {
        debug!("`resolve-march-native` exited non-zero");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Extract the -march=<value> token.
    extract_march_value(&stdout)
}

/// Ask GCC what `-march=native` resolves to.
///
/// ```text
/// $ gcc -march=native -Q --help=target 2>/dev/null | grep '^\s*-march='
///   -march=                           skylake
/// ```
fn try_gcc_march_native() -> Option<String> {
    let output = Command::new("gcc")
        .args(["-march=native", "-Q", "--help=target"])
        .output()
        .ok()?;

    if !output.status.success() {
        debug!("`gcc -march=native -Q --help=target` failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-march=") {
            // Format: "  -march=                           skylake"
            let rest = trimmed.strip_prefix("-march=")?;
            let value = rest.trim();
            if !value.is_empty() && value != "native" {
                debug!(march = %value, "GCC resolved -march=native");
                return Some(value.to_string());
            }
        }
    }

    None
}

/// Extract `-march=<value>` from a string of compiler flags.
fn extract_march_value(flags: &str) -> Option<String> {
    for token in flags.split_whitespace() {
        if let Some(value) = token.strip_prefix("-march=")
            && value != "native"
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

/// Replace `-march=native` in a CFLAGS string with the resolved architecture.
///
/// Returns `(resolved_cflags, was_modified)`.
pub fn resolve_cflags(cflags: &str, resolved_march: &str) -> (String, bool) {
    let mut modified = false;
    let resolved: Vec<String> = cflags
        .split_whitespace()
        .map(|token| {
            if token == "-march=native" {
                modified = true;
                format!("-march={resolved_march}")
            } else if token == "-mtune=native" {
                modified = true;
                format!("-mtune={resolved_march}")
            } else {
                token.to_string()
            }
        })
        .collect();

    (resolved.join(" "), modified)
}

/// Resolve `CPU_FLAGS_*` using `cpuid2cpuflags`.
///
/// Output format:
/// ```text
/// CPU_FLAGS_X86: aes avx avx2 f16c fma3 mmx mmxext pclmul popcnt rdrand …
/// ```
fn resolve_cpu_flags() -> Option<(String, Vec<String>)> {
    let output = Command::new("cpuid2cpuflags").output().ok()?;

    if !output.status.success() {
        debug!("`cpuid2cpuflags` not available or failed");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();

    // Parse "CPU_FLAGS_X86: flag1 flag2 …"
    if let Some((var, flags_str)) = line.split_once(':') {
        let var = var.trim().to_string();
        let flags: Vec<String> = flags_str.split_whitespace().map(String::from).collect();

        if !flags.is_empty() {
            return Some((var, flags));
        }
    }

    None
}

/// Detect `CHOST` for the running system.
///
/// Strategy:
/// 1. Try `portageq envvar CHOST` (most authoritative on Gentoo).
/// 2. Fall back to `gcc -dumpmachine`.
/// 3. Fall back to deriving from `uname -m`.
fn detect_chost() -> Result<String> {
    // Strategy 1: portageq.
    if let Some(chost) = try_portageq_chost() {
        return Ok(chost);
    }

    // Strategy 2: gcc -dumpmachine.
    if let Some(chost) = try_gcc_dumpmachine() {
        return Ok(chost);
    }

    // Strategy 3: uname-based guess.
    let arch = Command::new("uname")
        .arg("-m")
        .output()
        .context("Failed to run uname")?;
    let arch = String::from_utf8_lossy(&arch.stdout).trim().to_string();

    let chost = match arch.as_str() {
        "x86_64" => "x86_64-pc-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        "armv7l" => "armv7a-unknown-linux-gnueabihf",
        "i686" => "i686-pc-linux-gnu",
        "riscv64" => "riscv64-unknown-linux-gnu",
        "ppc64le" => "powerpc64le-unknown-linux-gnu",
        other => {
            warn!(arch = %other, "Unknown architecture — guessing CHOST");
            return Ok(format!("{other}-unknown-linux-gnu"));
        }
    };

    Ok(chost.to_string())
}

/// Try `portageq envvar CHOST`.
fn try_portageq_chost() -> Option<String> {
    let output = Command::new("portageq")
        .args(["envvar", "CHOST"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let chost = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if chost.is_empty() {
        return None;
    }

    debug!(chost = %chost, "CHOST from portageq");
    Some(chost)
}

/// Try `gcc -dumpmachine`.
fn try_gcc_dumpmachine() -> Option<String> {
    let output = Command::new("gcc").arg("-dumpmachine").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let chost = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if chost.is_empty() {
        return None;
    }

    debug!(chost = %chost, "CHOST from gcc -dumpmachine");
    Some(chost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_march_native_in_cflags() {
        let (resolved, modified) =
            resolve_cflags("-O2 -pipe -march=native -fomit-frame-pointer", "skylake");
        assert!(modified);
        assert_eq!(resolved, "-O2 -pipe -march=skylake -fomit-frame-pointer");
    }

    #[test]
    fn resolve_march_and_mtune_native() {
        let (resolved, modified) = resolve_cflags("-O2 -march=native -mtune=native", "znver3");
        assert!(modified);
        assert_eq!(resolved, "-O2 -march=znver3 -mtune=znver3");
    }

    #[test]
    fn no_native_flags_unchanged() {
        let (resolved, modified) = resolve_cflags("-O2 -pipe -march=skylake", "skylake");
        assert!(!modified);
        assert_eq!(resolved, "-O2 -pipe -march=skylake");
    }

    #[test]
    fn extract_march_from_flags() {
        assert_eq!(
            extract_march_value("-march=skylake -mmmx -mpopcnt"),
            Some("skylake".to_string())
        );
        assert_eq!(extract_march_value("-O2 -pipe"), None);
        assert_eq!(extract_march_value("-march=native"), None);
    }
}
