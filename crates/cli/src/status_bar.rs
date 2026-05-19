//! Dynamic status line shown at the bottom of stderr output.
//!
//! The bar renders the current phase and elapsed time, updating every 100 ms.
//! It is only active when stderr is a TTY; all methods silently no-op
//! otherwise.
//!
//! ## Output coordination
//!
//! Raw emerge PTY bytes flow to **stdout**.  The status bar writes to
//! **stderr**.  Since both point at the same terminal on a normal interactive
//! session the bar must be hidden before any section that produces heavy
//! stdout output (PTY relay, local emerge invocation, SyncProgressReporter).
//! Use [`StatusBar::hide`] / [`StatusBar::show`] around those sections, and
//! [`StatusBar::println`] instead of `eprintln!` for structured messages
//! emitted while the bar is visible.

use std::io::{IsTerminal, Write, stderr};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

static INSTANCE: OnceLock<Arc<StatusBar>> = OnceLock::new();

/// How the status bar delivers progress feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusBarMode {
    /// TTY stderr — full ANSI overwrite bar with background redraw.
    Tty,
    /// Non-TTY stderr, not quiet — emit plain `remerge: <phase>` lines when
    /// the phase changes so CI pipelines and pipes get progress output.
    LogLine,
    /// Quiet mode (`-q`) — all methods are no-ops; no output whatsoever.
    Silent,
}

/// A dynamic one-line status bar rendered on stderr.
pub struct StatusBar {
    state: Arc<Mutex<State>>,
    mode: StatusBarMode,
}

struct State {
    phase: String,
    phase_started: Instant,
    /// When `true` the bar is not drawn (e.g. during PTY streaming).
    hidden: bool,
    /// Set when the bar is permanently finished / dropped.
    finished: bool,
}

/// Strip a trailing ` (Ns)…` or ` (Ns)…` heartbeat suffix from a phase
/// string so that LogLine mode can deduplicate against the base phase.
fn base_phase(phase: &str) -> &str {
    // Heartbeat format: "Stage name (42s)…"
    if let Some(pos) = phase.rfind(" (") {
        let tail = &phase[pos..];
        if (tail.ends_with("s)\u{2026}") || tail.ends_with("s)..."))
            && tail[2..tail.len() - 4].chars().all(|c| c.is_ascii_digit())
        {
            return &phase[..pos];
        }
    }
    phase
}

impl StatusBar {
    /// Initialise the global status bar and spawn the background redraw task.
    ///
    /// Must be called from within a Tokio async context (after
    /// `#[tokio::main]` runtime is up).
    ///
    /// `quiet` should be `true` when the operator passed `-q`; the bar
    /// becomes completely silent in that case.
    pub fn init(quiet: bool) -> Arc<Self> {
        let mode = if quiet {
            StatusBarMode::Silent
        } else if stderr().is_terminal() {
            StatusBarMode::Tty
        } else {
            StatusBarMode::LogLine
        };
        let bar = Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                phase: String::new(),
                phase_started: Instant::now(),
                hidden: true,
                finished: false,
            })),
            mode,
        });
        let _ = INSTANCE.set(bar.clone());

        if mode == StatusBarMode::Tty {
            let weak: Weak<Mutex<State>> = Arc::downgrade(&bar.state);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    let Some(arc) = weak.upgrade() else {
                        break;
                    };
                    let guard = arc.lock().unwrap();
                    if guard.finished {
                        break;
                    }
                    if !guard.hidden && !guard.phase.is_empty() {
                        let phase = guard.phase.clone();
                        let elapsed = guard.phase_started.elapsed();
                        drop(guard);
                        draw_line(&phase, elapsed);
                    }
                }
            });
        }

        bar
    }

    /// Return the global instance if it has been initialised.
    pub fn global() -> Option<Arc<Self>> {
        INSTANCE.get().cloned()
    }

    /// Set the current phase label and make the bar visible.
    ///
    /// - `Tty`: immediately redraws the bar so the new phase appears without
    ///   waiting for the next 100 ms tick.
    /// - `LogLine`: emits `remerge: <phase>` to stderr when the base phase
    ///   changes; heartbeat time-suffix updates are suppressed to avoid spam.
    /// - `Silent`: no-op.
    pub fn set_phase(&self, phase: impl Into<String>) {
        match self.mode {
            StatusBarMode::Silent => return,
            StatusBarMode::LogLine => {
                let phase = phase.into();
                let new_base = base_phase(&phase).to_owned();
                let mut state = self.state.lock().unwrap();
                let prev_base = base_phase(&state.phase).to_owned();
                state.phase = phase;
                drop(state);
                if new_base != prev_base {
                    eprintln!("remerge: {new_base}");
                }
                return;
            }
            StatusBarMode::Tty => {}
        }
        let mut state = self.state.lock().unwrap();
        state.phase = phase.into();
        state.phase_started = Instant::now();
        state.hidden = false;
        let phase = state.phase.clone();
        drop(state);
        draw_line(&phase, Duration::ZERO);
    }

    /// Hide the status bar without discarding the current phase.
    ///
    /// Call [`show`] to make it visible again with the same phase, or
    /// [`set_phase`] to show it with a new phase.
    pub fn hide(&self) {
        if self.mode != StatusBarMode::Tty {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let was_hidden = state.hidden;
        state.hidden = true;
        drop(state);
        if !was_hidden {
            clear_line();
        }
    }

    /// Show the status bar again with the current phase.
    pub fn show(&self) {
        if self.mode != StatusBarMode::Tty {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.hidden = false;
        let phase = state.phase.clone();
        let elapsed = state.phase_started.elapsed();
        drop(state);
        if !phase.is_empty() {
            draw_line(&phase, elapsed);
        }
    }

    /// Mark the bar as permanently finished and clear it.
    ///
    /// The background redraw task exits on the next tick.
    pub fn finish(&self) {
        if self.mode != StatusBarMode::Tty {
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.finished = true;
        state.hidden = true;
        drop(state);
        clear_line();
    }

    /// Print a message line, coordinating with the status bar.
    ///
    /// This clears the current status line, prints `msg` (followed by a
    /// newline) to stderr, then immediately redraws the status bar — all as
    /// one atomic write so the output is not interleaved with the
    /// background redraw task.
    ///
    /// Use this instead of `eprintln!` for structured messages emitted while
    /// the bar might be visible.
    pub fn println(&self, msg: &str) {
        if self.mode == StatusBarMode::Tty {
            let state = self.state.lock().unwrap();
            if !state.hidden && !state.phase.is_empty() {
                let phase = state.phase.clone();
                let elapsed = state.phase_started.elapsed();
                drop(state);

                // Compose one atomic write: clear bar + message + redrawn bar.
                let bar_line = render(&phase, elapsed);
                let composed = format!("\r\x1b[2K{msg}\n\r\x1b[2m{bar_line}\x1b[0m");
                let mut err = stderr();
                let _ = err.write_all(composed.as_bytes());
                let _ = err.flush();
                return;
            }
        }
        // Fallback: plain stderr line.
        eprintln!("{msg}");
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        // Make sure the terminal line is clean even if `finish()` was never
        // called (e.g. early return via `?` propagation).
        if self.mode == StatusBarMode::Tty {
            clear_line();
        }
    }
}

// ─── Internal rendering helpers ────────────────────────────────────────────────

/// Write the status bar on the current stderr line using carriage-return
/// overwrite.  The text is wrapped in dim ANSI styling so it is visually
/// subordinate to real output.
fn draw_line(phase: &str, elapsed: Duration) {
    let line = render(phase, elapsed);
    let mut err = stderr();
    let _ = write!(err, "\r\x1b[2K\x1b[2m{line}\x1b[0m");
    let _ = err.flush();
}

/// Erase the status bar line without moving the cursor.
fn clear_line() {
    let mut err = stderr();
    let _ = write!(err, "\r\x1b[2K");
    let _ = err.flush();
}

/// Render the status bar text, truncating to the terminal width.
fn render(phase: &str, elapsed: Duration) -> String {
    // Query terminal width; fall back to 80 columns if unavailable.
    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);

    let prefix = " ⋯ ";
    let secs = elapsed.as_secs();
    let suffix = if secs > 0 {
        format!(" [{secs}s]")
    } else {
        String::new()
    };

    let budget = width
        .saturating_sub(prefix.len())
        .saturating_sub(suffix.len());

    let phase_str = if phase.len() > budget && budget > 1 {
        format!("{}…", &phase[..budget.saturating_sub(1)])
    } else {
        phase.to_string()
    };

    format!("{prefix}{phase_str}{suffix}")
}
