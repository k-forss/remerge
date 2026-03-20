# Reference worker image for manual deployment and testing.
#
# In production, the server generates worker Dockerfiles dynamically
# based on the target system identity (CHOST, profile, GCC version).
# This file provides a standalone build.
#
# Build from the repository root:
#   docker build -f docker/worker.Dockerfile -t remerge-worker .

FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin remerge-worker

FROM gentoo/stage3:latest

# Sync portage tree.
RUN emerge --sync --quiet || true

# Configure portage for binary package creation.
RUN echo 'FEATURES="buildpkg noclean"' >> /etc/portage/make.conf && \
    echo 'PKGDIR="/var/cache/binpkgs"' >> /etc/portage/make.conf && \
    mkdir -p /var/cache/binpkgs

COPY --from=builder /build/target/release/remerge-worker /usr/local/bin/remerge-worker

ENTRYPOINT ["remerge-worker"]
