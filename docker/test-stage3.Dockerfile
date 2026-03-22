# Remerge integration test image.
#
# Provides a Gentoo stage3 environment with portageq and a synced portage
# tree for running the full E2E and Gentoo-specific integration tests.
#
# Build:
#   docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .
#
# Usage:
#   cargo test --workspace --features e2e

FROM gentoo/stage3:latest

# Sync portage tree (the slow part — ~5 min).
RUN emerge --sync

# Install test dependencies.
RUN emerge -1 app-misc/hello app-portage/cpuid2cpuflags

# Create a world file for expand_set tests.
RUN echo "app-misc/hello" >> /var/lib/portage/world

# Verify portageq works.
RUN portageq envvar USE

LABEL org.opencontainers.image.source=https://github.com/k-forss/remerge
LABEL org.opencontainers.image.description="Remerge integration test image"
