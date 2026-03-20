# ── Planner stage (cargo-chef) ────────────────────────────────────────
FROM rust:1.88-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo chef prepare --recipe-path recipe.json

# ── Build stage ───────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
COPY Cargo.toml Cargo.lock ./

# Pre-build dependencies (this layer is cached unless Cargo.toml/lock changes).
RUN cargo chef cook --release --recipe-path recipe.json --bin remerge-server

# Copy actual source and build.
COPY crates/ crates/
RUN cargo build --release --bin remerge-server

# ── Runtime stage ────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/remerge-server /usr/local/bin/remerge-server

# Create default directories.
RUN mkdir -p /var/cache/remerge/binpkgs /etc/remerge

EXPOSE 7654

ENTRYPOINT ["remerge-server"]
CMD ["--listen", "0.0.0.0:7654"]
