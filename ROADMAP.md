# Remerge Roadmap

This is the single active project-state and task-tracking document for remerge.
It replaces the previous active trackers (`TODO.md`, `TASKS.md`, and
`PROMPT.md`). User documentation stays in `README.md`, maintainer procedures
stay in `DEVELOPMENT.md`, security reporting stays in `SECURITY.md`, and release
history stays in `CHANGELOG.md`.

## How to use this file

- Work items are ordered by implementation dependency, not by convenience.
- Do not add parallel task lists elsewhere in the repository.
- Each new task should include a status, rationale, affected files, and a
  concrete verification step.
- Tests must assert one expected outcome. Do not use always-pass patterns such
  as `assert!(status == A || status == B)` or silent skip branches.
- Mark a task complete only after its verification command or review step has
  actually been run.

## Status legend

| Status | Meaning |
|--------|---------|
| `todo` | Not started. |
| `audit` | Needs a focused audit before implementation. |
| `doing` | In progress. |
| `blocked` | Cannot proceed without an external dependency or decision. |
| `done` | Implemented and verified. |

## Current snapshot

Date: 2026-05-17

Remerge has a substantial implemented core: CLI configuration capture,
server-side workorder APIs, Docker worker orchestration, worker-side Portage
setup, binary package hosting, optional TLS, mTLS modes via reverse-proxy
headers, OpenPGP package signing integration, persistence, metrics, Docker
image cleanup, CI workflows, release workflows, and a completed integration-test
suite milestone.

The project should be treated as beta-ready in code shape but not yet fully
production-ready operationally. The next work should focus on validating the
current state, closing correctness/security gaps, documenting deployment, and
making the system operable for real Gentoo hosts.

## Release readiness gates

Before any public beta or v0.1.0 release, record the result of each gate here.

| Gate | Command or check | Required result | Status |
|------|------------------|-----------------|--------|
| Formatting | `cargo fmt --all -- --check` | Passes | done |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | Passes | done |
| Unit and filesystem tests | `cargo test --workspace` | Passes | done |
| Docker integration tests | `cargo test --workspace --features integration` | Passes with Docker available | done |
| Full E2E tests | `cargo test --workspace --features integration,e2e` | Passes with Docker and stage3 image available | done |
| Documentation consistency | Search for stale task/status claims | No active contradictions | done |
| Deployment dry run | Fresh server + fresh Gentoo client | Workorder builds and client installs binpkg | todo |

## P0 — Correctness and known bugs

### P0.1 — Verify and wire queue depth metric

Status: done

Rationale: `remerge_queue_depth` is exported as a Prometheus gauge, but the
previous TODO and a quick code audit indicate it may not be updated by the queue
processor.

Affected files:

- `crates/server/src/metrics.rs`
- `crates/server/src/queue.rs`
- `tests/server_api_test.rs` or a new metrics-focused integration test

Verification:

- Submit queued workorders and assert `remerge_queue_depth` increases and then
  returns to zero.
- `cargo test --workspace --features integration` passes.

Result:

- `remerge_queue_depth` is now incremented on submission and decremented when a
  pending workorder is claimed or cancelled.
- `tests/server_api_test.rs::queue_depth_metric_tracks_pending_workorders`
  covers the metric behavior.

### P0.2 — Remove or use `DockerManager::max_workers()`

Status: done

Rationale: `DockerManager::max_workers()` is marked with `#[allow(dead_code)]`.
Dead accessors should either become part of a real API/test path or be removed.

Affected files:

- `crates/server/src/docker.rs`
- Any tests that should inspect the configured worker limit

Verification:

- No dead-code allowance is needed for this accessor, or the accessor is used by
  meaningful production/test code.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.

Result:

- The unused `DockerManager::max_workers()` field/accessor was removed.
- Worker concurrency remains enforced by `AppState::worker_semaphore`.

### P0.3 — Reconfirm integration and E2E test claims

Status: done

Rationale: The completed integration-test milestone was previously audited after
several false-positive iterations. Re-run the suite in the current environment
before relying on the milestone for release readiness.

Affected files:

- `tests/`
- `.github/workflows/ci.yml`
- `.github/workflows/test-image.yml`
- `docs/archive/integration-test-suite.md`

Verification:

- `cargo test --workspace`
- `cargo test --workspace --features integration`
- `cargo test --workspace --features integration,e2e` when Docker and the
  stage3 image are available.

Result:

- The Docker wait path now handles already-exited worker containers instead of
  leaving fast-failing workorders stuck in `Building`.
- Worker image build/start failures now persist failed results and clean up
  runtime state instead of leaking per-workorder channels.
- `tests/error_test.rs` now uses deterministic worker-failure fixtures and
  asserts that container/channel state is fully cleaned up after terminal
  failures.
- Verified passing commands: `cargo test -p remerge-server validate_worker_binary -- --nocapture`,
  `cargo test -p remerge-integration-tests --test error_test --features integration,e2e`, and
  `cargo test --workspace --features integration,e2e`.

### P0.4 — Validate worker binary distribution path

Status: done

Rationale: Worker images depend on a server-supplied worker binary or image
build context. The operational failure mode for a missing or mismatched worker
binary must be explicit and documented.

Affected files:

- `crates/server/src/config.rs`
- `crates/server/src/docker.rs`
- `docker/`
- `README.md`
- `DEVELOPMENT.md`

Verification:

- Starting a server without a usable worker binary fails early or produces a
  clear actionable error before accepting real workorders.
- Deployment docs explain the expected worker binary/image setup.

Result:

- `remerge-server` now validates `worker_binary` during startup and exits with
  an actionable error if the path is missing, unreadable, or not a regular
  file.
- README, development notes, and the example server config now document the
  required worker-binary installation path.

## P1 — Security and hardening

### P1.1 — Document and harden the mTLS trust boundary

Status: done

Rationale: mTLS mode depends on trusted reverse-proxy certificate validation and
forwarded fingerprint headers. Operators need exact guidance to avoid trusting
spoofed client headers.

Affected files:

- `crates/server/src/auth.rs`
- `config/server.example.toml`
- `README.md`
- `SECURITY.md`
- `DEVELOPMENT.md`

Verification:

- Security docs describe which headers are trusted, which proxy must strip
  inbound client-supplied headers, and how fingerprints are normalized.
- Tests cover missing/invalid auth headers in mTLS and mixed modes.

### P1.2 — Decide protection model for `/metrics` and `/binpkgs`

Status: done

Rationale: Metrics expose system state and binpkg hosting may expose package
inventory. Decide whether this is intentionally public, reverse-proxy protected,
or authenticated by the application.

Affected files:

- `crates/server/src/api.rs`
- `README.md`
- `SECURITY.md`
- Deployment examples

Verification:

- The chosen exposure model is documented.
- If protection is required, tests assert unauthorized requests are rejected.

### P1.3 — Review signing key handling in worker containers

Status: done

Rationale: The server currently injects `REMERGE_GPG_KEY` into worker container
environment while mounting the GPG home read-only. Confirm whether the key value
is only a public key identifier or whether the approach should be replaced by a
file/config-only mechanism.

Affected files:

- `crates/server/src/docker.rs`
- `crates/worker/src/main.rs`
- `crates/worker/src/portage_setup.rs`
- `README.md`

Verification:

- No private key material is passed through environment variables.
- Signing setup documentation clearly separates public key ID, private keyring,
  and mount permissions.

### P1.4 — Add request, queue, and worker resource limits

Status: done

Rationale: A deployable build service needs explicit limits for HTTP body size,
queued/running workorders, build duration, CPU, memory, disk, and possibly
network access.

Affected files:

- `crates/server/src/api.rs`
- `crates/server/src/queue.rs`
- `crates/server/src/docker.rs`
- `crates/server/src/config.rs`
- `config/server.example.toml`

Verification:

- Oversized requests, stuck builds, and over-limit containers have deterministic
  failure modes.
- Limits are configurable and documented.

### P1.5 — Add rate limiting guidance or implementation

Status: done

Rationale: Public submission endpoints should not be exposed without rate
limits. This may be application-level or delegated to the reverse proxy, but the
choice must be explicit.

Affected files:

- `README.md`
- `SECURITY.md`
- Deployment examples
- `crates/server/src/api.rs` if implemented in-process

Verification:

- Deployment docs include a rate-limit example or application tests cover the
  configured limit.

## P2 — Deployability and operations

### P2.1 — Write a production deployment guide

Status: done

Rationale: The repository has installation and release-process docs, but needs a
single operator-focused guide for running a real build server safely.

Affected files:

- `README.md`
- `DEVELOPMENT.md`
- New deployment documentation if needed
- `docker/docker-compose.yml`
- `config/server.example.toml`

Verification:

- A new operator can provision Docker, persistent volumes, optional TLS/mTLS,
  worker binary/image setup, binhost URL, and a Gentoo client from the guide.

### P2.2 — Define backup and restore procedures

Status: done

Rationale: Operators need to preserve state and binary package repositories, and
know what can be rebuilt after loss.

Affected files:

- Deployment documentation
- `config/server.example.toml`

Verification:

- Docs identify required backup paths for state and binpkgs.
- Restore steps include validation commands.

### P2.3 — Add upgrade and rollback runbooks

Status: done

Rationale: Server, worker, image cache, state format, and client versions can
change independently. Operators need a safe upgrade path.

Affected files:

- `DEVELOPMENT.md`
- Deployment documentation
- Release documentation

Verification:

- Runbooks cover server binary upgrade, worker image rebuild, state backup,
  rollback, and client compatibility checks.

### P2.4 — Document monitoring and alerting

Status: done

Rationale: Deployments need useful alerts for failed builds, queue depth, disk
usage, worker crashes, and stale images.

Affected files:

- `crates/server/src/metrics.rs`
- Deployment documentation

Verification:

- Docs include the exported metrics, suggested alerts, and dashboard targets.

## P3 — Core usability features

### P3.1 — Persist or clearly manage client identity

Status: done

Rationale: The CLI supports configurable client IDs, but a generated identity
that changes unexpectedly can break main/follower semantics and server-side
scoping.

Affected files:

- `crates/cli/src/config.rs`
- `README.md`
- `config/server.example.toml`

Verification:

- Client identity lifecycle is either persisted automatically or explicitly
  documented as operator-managed.
- Tests cover config-file and environment-variable precedence.

Result:

- The CLI client identity contract is now documented explicitly: first run
  persists `client_id`, missing IDs are backfilled into the config file, and
  shared identities remain an operator-managed `main`/`follower` choice.
- README, development notes, and the operations guide now describe how identity
  persistence works and when an override is intentional.

### P3.2 — Decide package-category allowlist policy

Status: done

Rationale: A server-side allowlist could reduce abuse and resource risk, but it
may also limit legitimate Gentoo workflows. Decide whether to implement it.

Affected files:

- `crates/types/src/validation.rs`
- `crates/server/src/api.rs`
- `crates/server/src/config.rs`

Verification:

- Decision is documented. If implemented, rejected categories return a single
  expected status code and are covered by tests.

Result:

- The package-category policy is now explicitly documented as unrestricted.
- remerge intentionally does not implement a server-side category allowlist;
  operators are expected to control submissions with authentication,
  reverse-proxy policy, and rate limiting instead of category gating.

### P3.3 — Evaluate base worker image layering

Status: done

Rationale: Publishing or prebuilding a base worker image and layering Portage
configuration at runtime could reduce build latency and improve reliability.

Affected files:

- `crates/server/src/docker.rs`
- `docker/`
- `.github/workflows/docker.yml`
- `.github/workflows/test-image.yml`

Verification:

- Documented decision with benchmark or operational rationale.
- If implemented, tests verify cache invalidation and worker binary upgrades.

Result:

- Worker images are now built in two layers: a cached base image per system
  identity plus a thin runtime layer that injects the current
  `remerge-worker` binary.
- Worker-binary hash invalidation still forces the thin runtime image to be
  rebuilt, while the cached base layer is reused when unchanged.
- README, development notes, operations docs, and the example server config now
  document the layered build model and `worker_base_image` /
  `skip_worker_sync` behavior.

### P3.4 — Improve binpkg signature verification UX

Status: done

Rationale: Server-side package signing exists, but users need clear client-side
verification steps and possibly helper automation.

Affected files:

- `README.md`
- `overlay/README.md`
- Gentoo overlay files

Verification:

- A fresh client can import the signing key and verify packages using the
  documented steps.

Result:

- The server now exports the configured public binpkg-signing key at
  `GET /api/v1/signing-key` and reports the fingerprint plus endpoint in
  `GET /api/v1/info`.
- README, operations docs, and overlay docs now require clients to fetch that
  key and enable Portage binpkg signature verification when signing is enabled.

## P4 — Testing and CI

### P4.1 — Add fuzz testing for Portage parsing and emerge argument filtering

Status: done

Rationale: Portage config parsing and argument filtering are high-value inputs
for fuzzing because malformed local config or hostile arguments could trigger
unexpected behavior.

Affected files:

- `crates/cli/src/portage.rs`
- `crates/cli/src/args.rs`
- `crates/types/src/validation.rs`
- `fuzz/`
- `.github/workflows/ci.yml`

Verification:

- Fuzz targets build and run locally.
- CI has at least a lightweight scheduled fuzz/sanitizer job, or docs explain
  how maintainers run the fuzz suite.

Result:

- Added a dedicated `fuzz/` package with `make_conf_vars` and
  `emerge_arg_filtering` libFuzzer targets.
- Exposed stable helper entry points for make.conf parsing and emerge-argument
  atom extraction.
- CI now runs a nightly-toolchain fuzz smoke job on every push and PR.

### P4.2 — Add load testing for concurrent submissions

Status: done

Rationale: Current tests cover duplicate active workorders and E2E flows, but
not sustained concurrent load, queue fairness, or backpressure.

Affected files:

- `tests/`
- `crates/server/src/api.rs`
- `crates/server/src/queue.rs`

Verification:

- A load test submits many workorders and verifies deterministic rejection,
  queuing, or processing behavior according to the configured limits.

Result:

- Added a dedicated `tests/load_test.rs` suite plus an explicit ignored stress
  harness.
- Fixed a real submission race by serializing admission so
  `max_active_workorders` is enforced atomically under concurrent load.

### P4.3 — Keep CI and local E2E prerequisites explicit

Status: done

Rationale: Full E2E tests depend on Docker and a Gentoo stage3 image. The
failure and skip behavior must remain obvious and non-silent.

Affected files:

- `.github/workflows/ci.yml`
- `.github/workflows/test-image.yml`
- `tests/common/server.rs`
- `docs/archive/integration-test-suite.md`

Verification:

- CI pulls or builds the expected stage3 image.
- Missing Docker/stage3 produces a clear failure or documented prerequisite,
  not an always-pass test.

Result:

- CI now logs whether the prebuilt stage3 image was pulled successfully or the
  local fallback build path will be used.
- Maintainer docs now call out the dedicated load suite and the explicit GHCR
  versus local-build prerequisite flow.

### P4.4 — Add performance or duration regression tracking

Status: done

Rationale: Nextest JUnit output captures durations, but there is no policy for
comparing build/test time across releases.

Affected files:

- `.config/nextest.toml`
- `.config/test-duration-baseline.json`
- `.github/workflows/ci.yml`
- `DEVELOPMENT.md`
- `scripts/test_duration_baseline.py`

Verification:

- CI uploads enough timing data for review, and maintainers have a documented
  threshold for investigating regressions.

Result:

- Added a checked-in nextest timing baseline with 213 current test durations.
- CI now compares JUnit output against that baseline and hard-fails at 25%
  slowdown while uploading a human-readable duration report artifact.

## P5 — Observability and tracing

### P5.1 — Add OpenTelemetry trace context propagation

Status: done

Rationale: Workorders move through CLI submission, server queueing, Docker
worker execution, WebSocket progress, and local installation. Trace context
would make failures easier to diagnose.

Affected files:

- `crates/cli/`
- `crates/server/`
- `crates/worker/`
- `crates/types/`

Verification:

- A submitted workorder carries a trace ID through logs/events across CLI,
  server, and worker.

Result:

- Added a shared observability crate that installs the OpenTelemetry SDK in the
  CLI, server, and worker, while keeping OTLP export environment-driven.
- The CLI now submits a W3C `traceparent` header, the server persists the trace
  context on each workorder, and trace IDs are surfaced in REST/WebSocket
  responses and worker execution context.

### P5.2 — Add additional operational metrics

Status: done

Rationale: Current metrics cover high-level counters and gauges. Operators also
need worker image build duration, container startup latency, per-package build
latency, and cleanup outcomes.

Affected files:

- `crates/server/src/metrics.rs`
- `crates/server/src/docker.rs`
- `crates/server/src/queue.rs`

Verification:

- Prometheus output includes the new metrics and tests assert their presence.

Result:

- Added Prometheus counters for worker image builds, worker container startup,
  cleanup success/failure, and best-effort package timing totals.
- Exposed per-atom package timing series and added unit/integration coverage for
  the new metrics surface.

## Completed milestone archive

The integration-test suite plan is complete and no longer the active project
tracker. A summary is archived in `docs/archive/integration-test-suite.md`.

Completed areas include:

- Test infrastructure and feature gates.
- Type and validation tests.
- CLI Portage reader tests.
- Worker Portage setup tests.
- In-process server API tests.
- Docker integration tests.
- E2E pipeline tests.
- Error-path and edge-case tests.
- CI smoke, integration, E2E, stage3 cache, and nextest/JUnit workflow support.
