//! Lightweight Prometheus-compatible metrics.
//!
//! Tracks key server counters and gauges using atomics.  Exposed via
//! `GET /metrics` in Prometheus text exposition format.
//!
//! Counters: workorders submitted/completed/failed/cancelled, build duration.
//! Gauges: active builds, queue depth, binpkg disk usage.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_PACKAGE_BUILD_METRIC_SERIES: usize = 128;

fn lock_package_builds(
    mutex: &Mutex<BTreeMap<String, PackageBuildMetrics>>,
) -> std::sync::MutexGuard<'_, BTreeMap<String, PackageBuildMetrics>> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct PackageBuildMetrics {
    builds_total: u64,
    duration_secs_total: u64,
}

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
    /// Number of worker image build attempts.
    pub worker_image_builds_total: AtomicU64,
    /// Cumulative worker image build duration in seconds.
    pub worker_image_build_duration_secs_total: AtomicU64,
    /// Number of worker container start attempts.
    pub worker_container_starts_total: AtomicU64,
    /// Cumulative worker container startup duration in seconds.
    pub worker_container_startup_duration_secs_total: AtomicU64,
    /// Successful runtime cleanup operations.
    pub cleanup_success_total: AtomicU64,
    /// Failed runtime cleanup operations.
    pub cleanup_failure_total: AtomicU64,
    /// Cumulative bytes reclaimed by snapshot cleanup.
    pub cleanup_reclaimed_bytes_total: AtomicU64,
    /// Total snapshot missing-blob discovery requests.
    pub snapshot_missing_blob_queries_total: AtomicU64,
    /// Total digests examined by snapshot missing-blob discovery.
    pub snapshot_missing_blob_digests_total: AtomicU64,
    /// Total digests reported missing by snapshot missing-blob discovery.
    pub snapshot_missing_blob_digests_missing_total: AtomicU64,
    /// Total snapshot blob upload requests accepted by the server.
    pub snapshot_blob_uploads_total: AtomicU64,
    /// Upload requests that reused an already stored canonical blob.
    pub snapshot_blob_upload_dedup_hits_total: AtomicU64,
    /// Cumulative raw snapshot blob bytes accepted by the server.
    pub snapshot_blob_upload_bytes_total: AtomicU64,
    /// Total snapshot blob download responses served.
    pub snapshot_blob_downloads_total: AtomicU64,
    /// Cumulative snapshot blob bytes served to clients.
    pub snapshot_blob_download_bytes_total: AtomicU64,
    /// Snapshot runtime staging passes completed successfully.
    pub snapshot_runtime_stages_total: AtomicU64,
    /// Cumulative bytes materialized while staging snapshot runtimes.
    pub snapshot_runtime_stage_bytes_total: AtomicU64,
    /// Best-effort package build completions detected from emerge output.
    pub package_builds_total: AtomicU64,
    /// Cumulative best-effort package build duration in seconds.
    pub package_build_duration_secs_total: AtomicU64,
    /// Bounded per-package timing aggregates for Prometheus label output.
    package_builds_by_atom: Mutex<BTreeMap<String, PackageBuildMetrics>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
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
            worker_image_builds_total: AtomicU64::new(0),
            worker_image_build_duration_secs_total: AtomicU64::new(0),
            worker_container_starts_total: AtomicU64::new(0),
            worker_container_startup_duration_secs_total: AtomicU64::new(0),
            cleanup_success_total: AtomicU64::new(0),
            cleanup_failure_total: AtomicU64::new(0),
            cleanup_reclaimed_bytes_total: AtomicU64::new(0),
            snapshot_missing_blob_queries_total: AtomicU64::new(0),
            snapshot_missing_blob_digests_total: AtomicU64::new(0),
            snapshot_missing_blob_digests_missing_total: AtomicU64::new(0),
            snapshot_blob_uploads_total: AtomicU64::new(0),
            snapshot_blob_upload_dedup_hits_total: AtomicU64::new(0),
            snapshot_blob_upload_bytes_total: AtomicU64::new(0),
            snapshot_blob_downloads_total: AtomicU64::new(0),
            snapshot_blob_download_bytes_total: AtomicU64::new(0),
            snapshot_runtime_stages_total: AtomicU64::new(0),
            snapshot_runtime_stage_bytes_total: AtomicU64::new(0),
            package_builds_total: AtomicU64::new(0),
            package_build_duration_secs_total: AtomicU64::new(0),
            package_builds_by_atom: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn record_worker_image_build(&self, duration_secs: u64) {
        self.worker_image_builds_total
            .fetch_add(1, Ordering::Relaxed);
        self.worker_image_build_duration_secs_total
            .fetch_add(duration_secs, Ordering::Relaxed);
    }

    pub fn record_worker_container_start(&self, duration_secs: u64) {
        self.worker_container_starts_total
            .fetch_add(1, Ordering::Relaxed);
        self.worker_container_startup_duration_secs_total
            .fetch_add(duration_secs, Ordering::Relaxed);
    }

    pub fn record_cleanup(&self, success: bool) {
        let counter = if success {
            &self.cleanup_success_total
        } else {
            &self.cleanup_failure_total
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cleanup_reclaimed_bytes(&self, reclaimed_bytes: u64) {
        self.cleanup_reclaimed_bytes_total
            .fetch_add(reclaimed_bytes, Ordering::Relaxed);
    }

    pub fn record_snapshot_missing_blob_query(&self, digest_count: usize, missing_count: usize) {
        self.snapshot_missing_blob_queries_total
            .fetch_add(1, Ordering::Relaxed);
        self.snapshot_missing_blob_digests_total
            .fetch_add(digest_count as u64, Ordering::Relaxed);
        self.snapshot_missing_blob_digests_missing_total
            .fetch_add(missing_count as u64, Ordering::Relaxed);
    }

    pub fn record_snapshot_blob_upload(&self, raw_size_bytes: u64, uploaded: bool) {
        self.snapshot_blob_uploads_total
            .fetch_add(1, Ordering::Relaxed);
        self.snapshot_blob_upload_bytes_total
            .fetch_add(raw_size_bytes, Ordering::Relaxed);
        if !uploaded {
            self.snapshot_blob_upload_dedup_hits_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_snapshot_blob_download(&self, served_bytes: u64) {
        self.snapshot_blob_downloads_total
            .fetch_add(1, Ordering::Relaxed);
        self.snapshot_blob_download_bytes_total
            .fetch_add(served_bytes, Ordering::Relaxed);
    }

    pub fn record_snapshot_runtime_stage(&self, materialized_bytes: u64) {
        self.snapshot_runtime_stages_total
            .fetch_add(1, Ordering::Relaxed);
        self.snapshot_runtime_stage_bytes_total
            .fetch_add(materialized_bytes, Ordering::Relaxed);
    }

    pub fn record_package_build(&self, atom: &str, duration_secs: u64) {
        self.package_builds_total.fetch_add(1, Ordering::Relaxed);
        self.package_build_duration_secs_total
            .fetch_add(duration_secs, Ordering::Relaxed);

        let mut by_atom = lock_package_builds(&self.package_builds_by_atom);
        if !by_atom.contains_key(atom) && by_atom.len() >= MAX_PACKAGE_BUILD_METRIC_SERIES {
            return;
        }
        let entry = by_atom.entry(atom.to_string()).or_default();
        entry.builds_total += 1;
        entry.duration_secs_total += duration_secs;
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
        write_counter(
            &mut out,
            "remerge_worker_image_builds_total",
            "Total number of worker image build attempts.",
            self.worker_image_builds_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_worker_image_build_duration_seconds_total",
            "Total cumulative worker image build duration in seconds.",
            self.worker_image_build_duration_secs_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_worker_container_starts_total",
            "Total number of worker container start attempts.",
            self.worker_container_starts_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_worker_container_startup_duration_seconds_total",
            "Total cumulative worker container startup duration in seconds.",
            self.worker_container_startup_duration_secs_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_cleanup_success_total",
            "Total successful worker runtime cleanup operations.",
            self.cleanup_success_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_cleanup_failure_total",
            "Total failed worker runtime cleanup operations.",
            self.cleanup_failure_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_cleanup_reclaimed_bytes_total",
            "Total snapshot cache bytes reclaimed by cleanup.",
            self.cleanup_reclaimed_bytes_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_missing_blob_queries_total",
            "Total snapshot missing-blob discovery requests.",
            self.snapshot_missing_blob_queries_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_missing_blob_digests_total",
            "Total digests examined by snapshot missing-blob discovery.",
            self.snapshot_missing_blob_digests_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_missing_blob_digests_missing_total",
            "Total digests reported missing by snapshot missing-blob discovery.",
            self.snapshot_missing_blob_digests_missing_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_blob_uploads_total",
            "Total snapshot blob upload requests accepted by the server.",
            self.snapshot_blob_uploads_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_blob_upload_dedup_hits_total",
            "Snapshot blob upload requests that reused an already stored canonical blob.",
            self.snapshot_blob_upload_dedup_hits_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_blob_upload_bytes_total",
            "Total raw snapshot blob bytes accepted by the server.",
            self.snapshot_blob_upload_bytes_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_blob_downloads_total",
            "Total snapshot blob download responses served.",
            self.snapshot_blob_downloads_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_blob_download_bytes_total",
            "Total snapshot blob bytes served to clients.",
            self.snapshot_blob_download_bytes_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_runtime_stages_total",
            "Total snapshot runtime staging passes completed successfully.",
            self.snapshot_runtime_stages_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_snapshot_runtime_stage_bytes_total",
            "Total bytes materialized while staging snapshot runtimes.",
            self.snapshot_runtime_stage_bytes_total
                .load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_package_builds_total",
            "Total best-effort package build completions detected from emerge output.",
            self.package_builds_total.load(Ordering::Relaxed),
        );
        write_counter(
            &mut out,
            "remerge_package_build_duration_seconds_total",
            "Total cumulative best-effort package build duration in seconds.",
            self.package_build_duration_secs_total
                .load(Ordering::Relaxed),
        );

        out.push_str(
            "# HELP remerge_package_builds_by_atom_total Best-effort package build completions grouped by atom.\n",
        );
        out.push_str("# TYPE remerge_package_builds_by_atom_total counter\n");
        out.push_str(
            "# HELP remerge_package_build_duration_seconds_by_atom_total Best-effort package build duration grouped by atom.\n",
        );
        out.push_str("# TYPE remerge_package_build_duration_seconds_by_atom_total counter\n");

        let by_atom = lock_package_builds(&self.package_builds_by_atom);
        for (atom, metrics) in by_atom.iter() {
            let atom = prometheus_escape_label_value(atom);
            out.push_str(&format!(
                "remerge_package_builds_by_atom_total{{atom=\"{atom}\"}} {}\n",
                metrics.builds_total
            ));
            out.push_str(&format!(
                "remerge_package_build_duration_seconds_by_atom_total{{atom=\"{atom}\"}} {}\n",
                metrics.duration_secs_total
            ));
        }
        out.push('\n');

        out
    }
}

fn prometheus_escape_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
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

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn prometheus_output_includes_package_labels() {
        let metrics = Metrics::new();
        metrics.record_package_build("dev-libs/openssl", 12);
        metrics.record_cleanup(true);
        metrics.record_snapshot_blob_upload(42, false);

        let body = metrics.to_prometheus();
        assert!(body.contains("remerge_package_builds_by_atom_total{atom=\"dev-libs/openssl\"} 1"));
        assert!(body.contains("remerge_cleanup_success_total 1"));
        assert!(body.contains("remerge_snapshot_blob_upload_dedup_hits_total 1"));
    }
}
