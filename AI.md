# AI Disclosure

This project was scaffolded and developed with the assistance of **GitHub
Copilot** (Claude) operating as an AI coding agent.

## What was AI-generated

| Area | Scope |
|------|-------|
| **Initial scaffold** | Full Cargo workspace, four crates (`types`, `cli`, `server`, `worker`), directory layout, and `Cargo.toml` dependency graph. |
| **Portage config reader** | `crates/cli/src/portage.rs` — `make.conf` parser, `package.use` / `package.accept_keywords` / `package.license` readers. |
| **Compiler flag resolution** | `crates/cli/src/cflags.rs` — `-march=native` → concrete arch translation, `cpuid2cpuflags` integration, CHOST detection. |
| **Crossdev support** | `crates/worker/src/crossdev.rs` — cross-compilation toolchain setup, `emerge-<CHOST>` wrapper detection. |
| **Docker management** | `crates/server/src/docker.rs` — dynamic Dockerfile generation, container lifecycle, log streaming. |
| **Client identity system** | `crates/types/src/client.rs`, `crates/server/src/registry.rs`, `crates/cli/src/config.rs` — UUID-based client tracking, config-diff detection, `/etc/remerge.conf`. |
| **CI/CD** | `.github/workflows/docker.yml` — GitHub Actions workflow for GHCR image publishing. |
| **Packaging** | `overlay/` — full Gentoo overlay with five packages: CLI source/binary, server source/binary, and `openpgp-keys-remerge` for verify-sig integration. OpenRC init/conf scripts and hardened systemd service file. |
| **Docker Compose** | `docker/docker-compose.yml` — example deployment with environment variable configuration. |
| **Server Dockerfile** | `docker/server.Dockerfile` — multi-stage Rust build for the server binary. |
| **Unit tests** | All `#[test]` and `#[tokio::test]` functions across the workspace. |
| **Documentation** | `README.md` (user-facing), `DEVELOPMENT.md` (maintainer/CI internals, key rotation), `CHANGELOG.md`, inline doc-comments, this file. |
| **mTLS authentication** | `crates/types/src/auth.rs`, `crates/server/src/auth.rs` — three auth modes, certificate registry, fingerprint normalisation. |
| **Binary package signing** | `crates/server/src/config.rs` `SigningConfig`, `crates/worker/src/portage_setup.rs` GPG integration — optional OpenPGP signing via portage's `binpkg-signing`. |
| **Build result accuracy** | `crates/server/src/queue.rs` — structured event parsing, binpkg directory scanning with SHA256/size, `Packages` index regeneration. |
| **Worker concurrency** | Semaphore-based `max_workers` enforcement in `queue.rs`, FIFO scheduling. |
| **State persistence** | `crates/server/src/persistence.rs` — JSON-based save/load of workorders, results, and client registry. |
| **Eviction & retention** | TTL-based eviction, max-entry-cap enforcement, idle image reaper with per-tuple preservation. |
| **Admin & observability** | `GET /api/v1/clients`, `/health`, `/metrics` — Prometheus counters/gauges, disk usage monitoring, JSON log output. |
| **Failure detection** | Specialised regex patterns in `builder.rs` and `queue.rs` for missing deps, USE conflicts, fetch failures. |
| **CLI enhancements** | VDB installed check (`--force`), `@world`/`@system` set expansion, atom validation. |
| **Native TLS** | `tokio-rustls` direct HTTPS serving with ALPN negotiation. |
| **CI / CD pipeline** | `.github/workflows/` — CI (fmt, clippy, test), security audit, Docker image publishing, release binary packaging with attestation, RC branch preparation, automatic tag creation. |
| **Release signing** | `.github/workflows/release.yml` SLSA attestation, `.github/workflows/virustotal.yml` VirusTotal scanning, `.github/workflows/sign-release.yml` PGP signing — three-phase pipeline with clear-signed release body as final seal. Multi-arch cross-compilation for five architectures. |
| **PGP key management** | `keys/release-signing.pub.asc`, key rotation documentation in `DEVELOPMENT.md`, 2-year key expiry with documented rotation/extension procedures. |
| **Project governance** | `SECURITY.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `DEVELOPMENT.md`, `deny.toml`, `.github/rulesets/`, `.github/ISSUE_TEMPLATE/`, `.github/dependabot.yml`. |

## What was human-directed

All features, architecture decisions, and design choices were specified by the
project maintainer.  The AI implemented requirements from natural-language
descriptions; the human reviewed, edited, and approved every change before
committing.

Specific human contributions include:

- Overall system architecture (CLI → server → Docker worker → binhost).
- Decision to use portage's native binhost mechanism (`--getbinpkg`).
- Cross-compilation strategy via `crossdev`.
- Client identity model (main / follower roles, configurable client IDs).
- mTLS authentication strategy and auth mode design.
- Binary package signing approach (GPG keyring mounted into workers).
- Release workflow design (RC branches → auto-tag → release).
- Release artifact signing strategy (PGP + SLSA attestation + VirusTotal + sealed release body).
- Branch/tag ruleset hardening strategy (signed commits, linear history, restricted refs).
- Documentation split strategy (user-facing README vs internal DEVELOPMENT.md).
- PGP key lifecycle decisions (2-year expiry, no passphrase, private key only in GitHub Secrets).
- Gentoo ebuild packaging approach.
- All configuration decisions (file paths, defaults, environment variables).

## Ongoing use

AI tooling continues to be used for development.  All AI-generated code is
reviewed before merge.  The project's test suite and CI pipeline validate
correctness independently of the generation method.

## Model

GitHub Copilot using Anthropic Claude.
