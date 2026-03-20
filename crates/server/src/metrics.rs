//! Lightweight Prometheus-compatible metrics.
//!
//! Tracks key server counters and gauges using atomics.  Exposed via
//! `GET /metrics` in Prometheus text exposition format.
//!
//! Counters: workorders submitted/completed/failed/cancelled, build duration.
//! Gauges: active builds, queue depth, binpkg disk usage.

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters and gauges for server metrics.
pub struct Metrics {
    /// Total workorders submitted.
    pub workorders_submitted: AtomicU64,
    /// Workorders that completed successfully.
    pub workorders_completed: AtomicU64,
    /// Workorders that failed.
    pub workorders_failed: AtomicU64,
    /// Workorders that were cancelled.
    pub workorders_cancelled: AtomicU64,
    /// Currently active (Building) workorders.
    pub builds_active: AtomicU64,
    /// Cumulative build duration in seconds.
    pub builds_total_duration_secs: AtomicU64,
    /// Number of workorders waiting in the queue (Queued status).
    pub queue_depth: AtomicU64,
    /// Total size of the binpkg repository directory in bytes.
    pub binpkg_disk_usage_bytes: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            workorders_submitted: AtomicU64::new(0),
            workorders_completed: AtomicU64::new(0),
            workorders_failed: AtomicU64::new(0),
            workorders_cancelled: AtomicU64::new(0),
            builds_active: AtomicU64::new(0),
            builds_total_duration_secs: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            binpkg_disk_usage_bytes: AtomicU64::new(0),
        }
    }

    /// Format metrics in Prometheus text exposition format.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::new();

        write_counter(
            &mut out,
            "remerge_workorders_submitted_total",
            "Total number of workorders submitted.",
            self.workorders_submitted.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_workorders_completed_total",
            "Total number of workorders completed successfully.",
            self.workorders_completed.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_workorders_failed_total",
            "Total number of workorders that failed.",
            self.workorders_failed.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_workorders_cancelled_total",
            "Total number of workorders cancelled.",
            self.workorders_cancelled.load(Ordering::Relaxed),
        );
        write_gauge(
            &mut out,
            "remerge_builds_active",
            "Number of builds currently in progress.",
            self.builds_active.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_builds_duration_seconds_total",
            "Total cumulative build duration in seconds.",
            self.builds_total_duration_secs.load(Ordering::Relaxed),
        );
        write_gauge(
            &mut out,
            "remerge_queue_depth",
            "Number of workorders waiting in the queue.",
            self.queue_depth.load(Ordering::Relaxed),
        );
        write_gauge(
            &mut out,
            "remerge_binpkg_disk_usage_bytes",
            "Total size of the binpkg repository in bytes.",
            self.binpkg_disk_usage_bytes.load(Ordering::Relaxed),
        );

        out
    }
}

fn write_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} counter\n"));
    out.push_str(&format!("{name} {value}\n\n"));
}

fn write_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} gauge\n"));
    out.push_str(&format!("{name} {value}\n\n"));
}
