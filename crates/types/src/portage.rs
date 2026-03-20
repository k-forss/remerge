//! Portage configuration types.
//!
//! Models the relevant parts of a Gentoo system's portage configuration
//! (`make.conf`, `package.use`, `package.accept_keywords`, profile, etc.)
//! so they can be shipped as part of a [`Workorder`](crate::workorder::Workorder).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A complete snapshot of the portage configuration relevant for binary builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortageConfig {
    /// Parsed key-value pairs from `/etc/portage/make.conf`.
    pub make_conf: MakeConf,

    /// Per-package USE flag overrides from `/etc/portage/package.use`.
    pub package_use: Vec<PackageUseEntry>,

    /// Per-package keyword unmasks from `/etc/portage/package.accept_keywords`.
    pub package_accept_keywords: Vec<PackageKeywordEntry>,

    /// Per-package license acceptances from `/etc/portage/package.license`.
    pub package_license: Vec<PackageLicenseEntry>,

    /// The active portage profile path (e.g. `default/linux/amd64/23.0`).
    pub profile: String,

    /// Installed package list (world set) — optional, for dependency resolution.
    pub world: Vec<String>,
}

/// Key fields extracted from `/etc/portage/make.conf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MakeConf {
    /// `CFLAGS` – compiler flags.
    ///
    /// When the CLI detects `-march=native`, it resolves it to the concrete
    /// micro-architecture flag (e.g. `-march=skylake`) before shipping the
    /// workorder.  The original value is preserved in [`original_cflags`].
    pub cflags: String,

    /// `CXXFLAGS` – C++ compiler flags (often `${CFLAGS}`).
    pub cxxflags: String,

    /// `LDFLAGS` – linker flags.
    pub ldflags: String,

    /// `MAKEOPTS` – e.g. `-j12`.
    pub makeopts: String,

    /// Global `USE` flags.
    pub use_flags: Vec<String>,

    /// `FEATURES` – portage features.
    pub features: Vec<String>,

    /// `ACCEPT_LICENSE` – e.g. `* -@EULA`.
    pub accept_license: String,

    /// `ACCEPT_KEYWORDS` – e.g. `amd64` or `~amd64`.
    pub accept_keywords: String,

    /// `EMERGE_DEFAULT_OPTS` – default emerge options.
    pub emerge_default_opts: String,

    /// `CHOST` – target system tuple (e.g. `x86_64-pc-linux-gnu`).
    ///
    /// Read from `make.conf` or detected from the running system.
    #[serde(default)]
    pub chost: String,

    /// `CPU_FLAGS_*` – CPU capability flags from `cpuid2cpuflags`.
    ///
    /// e.g. `CPU_FLAGS_X86="aes avx avx2 mmx sse sse2 sse3 ssse3 sse4_1 sse4_2"`.
    /// Stored as `(VAR_NAME, [flags])` — typically `CPU_FLAGS_X86` on x86.
    #[serde(default)]
    pub cpu_flags: Option<(String, Vec<String>)>,

    /// The original `CFLAGS` before `-march=native` was resolved.
    ///
    /// `None` when no translation was necessary (no `-march=native` present).
    #[serde(default)]
    pub original_cflags: Option<String>,

    /// `VIDEO_CARDS`, `INPUT_DEVICES`, and similar `USE_EXPAND` variables.
    pub use_expand: BTreeMap<String, Vec<String>>,

    /// Any additional variables we should forward.
    pub extra: BTreeMap<String, String>,
}

/// A single entry from `/etc/portage/package.use`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageUseEntry {
    /// Atom, e.g. `dev-libs/openssl`.
    pub atom: String,
    /// USE flags (prefixed with `-` for disable).
    pub flags: Vec<String>,
}

/// A single entry from `/etc/portage/package.accept_keywords`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageKeywordEntry {
    /// Atom, e.g. `sys-kernel/gentoo-sources`.
    pub atom: String,
    /// Keywords, e.g. `~amd64`.
    pub keywords: Vec<String>,
}

/// A single entry from `/etc/portage/package.license`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLicenseEntry {
    /// Atom, e.g. `sys-kernel/linux-firmware`.
    pub atom: String,
    /// Licenses, e.g. `linux-fw-redistributable`.
    pub licenses: Vec<String>,
}

/// Host system identity — describes the build environment needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemIdentity {
    /// Architecture — `amd64`, `arm64`, etc.
    pub arch: String,

    /// `CHOST` — the target system tuple (e.g. `x86_64-pc-linux-gnu`).
    ///
    /// This is the most important field for binary package compatibility:
    /// packages built with a different CHOST are not interchangeable.
    pub chost: String,

    /// GCC version on the host (for ABI compatibility).
    pub gcc_version: String,

    /// Glibc version on the host.
    pub libc_version: String,

    /// Kernel version (for `linux-headers` compat).
    pub kernel_version: String,

    /// Python targets — e.g. `python3_12`.
    pub python_targets: Vec<String>,

    /// Profile path — e.g. `default/linux/amd64/23.0`.
    pub profile: String,
}

impl Default for MakeConf {
    fn default() -> Self {
        Self {
            cflags: "-O2 -pipe".into(),
            cxxflags: "${CFLAGS}".into(),
            ldflags: "-Wl,-O1 -Wl,--as-needed".into(),
            makeopts: "-j1".into(),
            use_flags: Vec::new(),
            features: Vec::new(),
            accept_license: "-* @FREE".into(),
            accept_keywords: "amd64".into(),
            emerge_default_opts: String::new(),
            chost: "x86_64-pc-linux-gnu".into(),
            cpu_flags: None,
            original_cflags: None,
            use_expand: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}
