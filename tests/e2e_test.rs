//! Phase 6 — End-to-end pipeline tests.
//!
//! Full CLI → Server → Worker pipeline. Requires Docker, network,
//! and a Gentoo stage3 image.
//!
//! Gated behind `#[cfg(feature = "e2e")]`.

mod common;

#[cfg(feature = "e2e")]
mod e2e_tests {
    use super::common;

    /// Placeholder for full pipeline smoke test.
    #[tokio::test]
    async fn full_pipeline_smoke() {
        if !common::server::docker_available() {
            eprintln!("Docker not available — skipping e2e tests");
            return;
        }

        // Full pipeline test:
        // 1. Start server
        // 2. Submit workorder
        // 3. Wait for completion
        // 4. Verify artifacts
        //
        // This requires a Gentoo stage3 image and network access.
        // Placeholder until full E2E infrastructure is in place.
        eprintln!("e2e tests require Gentoo stage3 image — skipping in default CI");
    }
}
