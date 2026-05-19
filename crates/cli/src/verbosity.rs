//! CLI verbosity level, derived from -q/--quiet and -v flags.
//!
//! Mirrors the Portage convention:
//!   `-q` / `--quiet`  → suppress non-essential output
//!   `-v`              → verbose (one level)
//!   `-v -v` / `-vv`   → debug
//!   `-v -v -v`        → trace
//!
//! When no explicit flag is given, `EMERGE_DEFAULT_OPTS` is inspected first so
//! that a user who has `--quiet` or `--verbose` in their make.conf gets the
//! same behaviour here by default.

/// CLI verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Verbosity {
    /// Suppress all non-essential output.
    Quiet,
    /// Standard output (default).
    #[default]
    Normal,
    /// Show informational messages and more detailed emerge output  (`-v`).
    Verbose,
    /// Enable debug-level tracing output (`-vv`).
    VerboseDebug,
    /// Enable trace-level output — maximum verbosity (`-vvv`).
    VerboseTrace,
}

impl Verbosity {
    /// Derive verbosity from explicit flags.
    ///
    /// If no explicit flags are given, falls back to parsing
    /// `emerge_default_opts` (the value of `EMERGE_DEFAULT_OPTS` from
    /// make.conf / portageq).
    pub fn from_flags(quiet: bool, verbose_count: u8, emerge_default_opts: &str) -> Self {
        if quiet {
            return Verbosity::Quiet;
        }
        match verbose_count {
            0 => Self::from_emerge_default_opts(emerge_default_opts),
            1 => Verbosity::Verbose,
            2 => Verbosity::VerboseDebug,
            _ => Verbosity::VerboseTrace,
        }
    }

    /// Quick pre-parse of raw argv to detect verbosity before clap runs.
    ///
    /// Used in `main` to set `RUST_LOG` before the tracing subscriber is
    /// initialised (clap full-parse happens after tracing).
    pub fn early_detect() -> Self {
        let mut verbose_count: u8 = 0;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "-q" | "--quiet" => return Verbosity::Quiet,
                "-v" => verbose_count = verbose_count.saturating_add(1),
                // clustered flags like -vv / -vvv
                s if s.starts_with('-')
                    && !s.starts_with("--")
                    && s[1..].chars().all(|c| c == 'v') =>
                {
                    verbose_count = verbose_count.saturating_add((s.len() - 1) as u8);
                }
                _ => {}
            }
        }
        match verbose_count {
            0 => Verbosity::Normal,
            1 => Verbosity::Verbose,
            2 => Verbosity::VerboseDebug,
            _ => Verbosity::VerboseTrace,
        }
    }

    /// Parse verbosity from the `EMERGE_DEFAULT_OPTS` value.
    fn from_emerge_default_opts(opts: &str) -> Self {
        let mut verbose_count: u8 = 0;
        for token in opts.split_whitespace() {
            match token {
                "--quiet" | "-q" => return Verbosity::Quiet,
                "--verbose" | "-v" => verbose_count = verbose_count.saturating_add(1),
                // Clustered short flags: -vv (debug), -vvv (trace), -qq, etc.
                t if t.starts_with('-')
                    && t.len() > 1
                    && t[1..].chars().all(|c| c == 'v' || c == 'q') =>
                {
                    let ch = t.chars().nth(1).unwrap();
                    if ch == 'q' {
                        return Verbosity::Quiet;
                    }
                    // Each 'v' in the cluster counts as one verbose flag.
                    verbose_count = verbose_count.saturating_add((t.len() - 1) as u8);
                }
                _ => {}
            }
        }
        match verbose_count {
            0 => Verbosity::Normal,
            1 => Verbosity::Verbose,
            2 => Verbosity::VerboseDebug,
            _ => Verbosity::VerboseTrace,
        }
    }

    /// The `RUST_LOG` level string to set when `RUST_LOG` is not already in
    /// the environment.
    ///
    /// Portage convention: default output is steady but not quiet — internal
    /// `warn!` records should surface at normal verbosity so operator-relevant
    /// issues (pool exhausted, key missing, etc.) are visible.  Only `quiet`
    /// suppresses those by elevating the threshold to `error`.
    pub fn rust_log_level(self) -> &'static str {
        match self {
            Verbosity::Quiet => "error",
            Verbosity::Normal => "warn",
            Verbosity::Verbose => "info",
            Verbosity::VerboseDebug => "debug",
            Verbosity::VerboseTrace => "trace",
        }
    }

    /// The emerge flag to inject into `emerge_args` before submitting a
    /// workorder, if any.
    pub fn emerge_flag(self) -> Option<&'static str> {
        match self {
            Verbosity::Quiet => Some("--quiet"),
            Verbosity::Normal => None,
            Verbosity::Verbose | Verbosity::VerboseDebug | Verbosity::VerboseTrace => {
                Some("--verbose")
            }
        }
    }

    /// Whether this level shows informational messages.
    pub fn is_verbose(self) -> bool {
        self >= Verbosity::Verbose
    }

    /// Whether this level suppresses non-essential messages.
    pub fn is_quiet(self) -> bool {
        self == Verbosity::Quiet
    }
}
