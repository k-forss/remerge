# Integration Test Suite — Agent Prompt

You are continuing the implementation of a comprehensive integration test
suite for **remerge**, a distributed Gentoo binary-package builder written
in Rust.  A previous agent started this work; you are picking up where it
left off.

**Read this entire document before writing any code.**
**Read `TASKS.md` for the authoritative task list and status.**

---

## 1  Project Overview

Remerge is a Rust workspace (edition 2024, rust-version 1.88) with four
crates:

| Crate | Path | Role |
|-------|------|------|
| `remerge` | `crates/cli` | CLI binary — drop-in `emerge` wrapper |
| `remerge-server` | `crates/server` | HTTP/WS API, Docker orchestration, FIFO queue |
| `remerge-worker` | `crates/worker` | Runs inside Docker, applies portage config, executes `emerge` |
| `remerge-types` | `crates/types` | Shared types: portage, workorder, validation, client, auth |

**Data flow:**

```
CLI reads /etc/portage/ → serializes PortageConfig
  → HTTP POST to server /api/v1/workorders
  → server queues Workorder, builds/reuses Docker image
  → starts container with REMERGE_WORKORDER env var (JSON)
  → worker deserializes, writes /etc/portage/ inside container
  → worker runs `emerge --buildpkg …`
  → PTY output streamed back via Docker attach → WebSocket → CLI
  → binpkgs written to shared volume
  → CLI runs local `emerge --getbinpkg` to install
```

---

## 2  Current State of Implementation

### What exists and works

The previous agent created the full test file structure:

```
tests/
  common/
    mod.rs          — free_port(), with_timeout()
    fixtures.rs     — portage_tree(), vdb_tree(), minimal_portage_config(),
                      full_portage_config(), minimal_system_identity()
    server.rs       — docker_available(), TestServer::start()
  types_test.rs     — Phase 1: 27 tests, all pass ✅
  cli_portage_test.rs — Phase 2 (partial): 19 tests, all pass ✅
  worker_setup_test.rs — Phase 3 (partial): 15 tests, all pass ✅
  server_api_test.rs   — Phase 4 (partial): 11 tests, pass with Docker ✅
  docker_test.rs       — Phase 5 (partial): 3 tests, pass with Docker ✅
  e2e_test.rs          — Phase 6: PLACEHOLDER ONLY ❌
  error_test.rs        — Phase 7 (partial): 14 tests, all pass ✅
```

The `Cargo.toml` has `integration` and `e2e` features configured.
All four crates expose `pub mod` in their `lib.rs` for testing.
Some worker functions have `_inner` variants for testability:
`write_profile_overlay_inner`, `write_patches_inner`,
`set_profile_inner`, `build_makeopts_inner`, `parse_repo_sections`.

### What is NOT done (your work)

Refer to `TASKS.md` — every unchecked `[ ]` task needs implementation.
Key gaps, in priority order:

1. **Task 3.0** — Create `_inner` variants for all remaining private worker
   functions. This UNBLOCKS tasks 3.1–3.8 and 3.13.
2. **Tasks 3.1–3.8** — Test all portage config writing functions.
3. **Task 4.0** — Fix silent test skipping in Phase 4.
4. **Task 5.1** — Fix no-op Docker availability test.
5. **Tasks 4.9, 4.10, 4.12** — Missing server API tests.
6. **Tasks 5.3–5.7** — Docker lifecycle tests.
7. **Task 0.6** — GHCR test Docker image for Gentoo-specific tests.
8. **Tasks 0.5, 8.1–8.5** — CI integration.
9. **Tasks 7.1–7.8, 7.12** — Error path tests.
10. **Phase 6** — Full E2E tests (currently all placeholders).

### Known issue

There is a failing unit test in the worker crate:
`portage_setup::tests::set_profile_skips_when_profile_not_found`.
This is a pre-existing bug in `crates/worker/src/portage_setup.rs`, not in
the integration tests. Fix it if you can identify the issue, but it is not
your primary task.

---

## 3  Architecture Decisions (established)

### 3.1  Test location

Top-level `tests/` directory with multiple test files (already created).
Each file is a separate integration test crate. Shared code is in
`tests/common/`.

### 3.2  Feature gating

- Phases 1–3 run with `cargo test` (no feature flag).
- Phase 4 currently runs without feature flag but requires Docker —
  consider gating behind `integration`.
- Phase 5 (Docker) is behind `#[cfg(feature = "integration")]`.
- Phase 6 (E2E) is behind `#[cfg(feature = "e2e")]`.

### 3.3  In-process server harness

`TestServer::start()` in `tests/common/server.rs` starts an axum app on a
random port. It requires Docker because `AppState::new()` creates a
`DockerManager`. All server API tests use this harness.

### 3.4  Docker tests

Connect via `bollard`. Tag test images with unique names. Clean up in
`Drop` guards. Skip if Docker unavailable.

---

## 4  Detailed Implementation Guide

### 4.1  Task 3.0 — Create `_inner` variants

This is the highest-priority task. Without it, tasks 3.1–3.8 and 3.13
cannot be implemented.

**Location:** `crates/worker/src/portage_setup.rs`

**Pattern to follow** (already established in the same file):

```rust
// The public function is the private one, called with hardcoded path:
async fn write_profile_overlay(config: &PortageConfig) -> Result<()> {
    write_profile_overlay_inner(Path::new("/etc/portage/profile"), config).await
}

// The testable inner function accepts a path parameter:
pub async fn write_profile_overlay_inner(base: &Path, config: &PortageConfig) -> Result<()> {
    // ... actual implementation that writes to `base`
}
```

**Functions needing `_inner` variants:**

| Private function | `_inner` signature |
|-----------------|-------------------|
| `write_make_conf(config, worker_chost, gpg_key, gpg_home)` | `pub async fn write_make_conf_inner(base: &Path, config: &PortageConfig, worker_chost: &str, gpg_key: Option<&str>, gpg_home: Option<&str>) -> Result<()>` |
| `write_package_use(config)` | `pub async fn write_package_use_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_package_accept_keywords(config)` | `pub async fn write_package_accept_keywords_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_package_license(config)` | `pub async fn write_package_license_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_package_mask(config)` | `pub async fn write_package_mask_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_package_unmask(config)` | `pub async fn write_package_unmask_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_package_env(config)` | `pub async fn write_package_env_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_env_files(config)` | `pub async fn write_env_files_inner(base: &Path, config: &PortageConfig) -> Result<()>` |
| `write_repos_conf(config)` | `pub async fn write_repos_conf_inner(base: &Path, config: &PortageConfig, repos_dir: Option<&Path>) -> Result<()>` |

**Key rules:**
- Move the implementation body into the `_inner` variant.
- Replace hardcoded paths like `"/etc/portage/package.use/remerge"` with
  `base.join("package.use/remerge")`.
- The original private function becomes a one-line wrapper calling
  `_inner` with `Path::new("/etc/portage")` as base.
- For `write_repos_conf`, the `repos_dir` parameter should control
  location remapping (the private version reads from server env vars).
- The `ensure_dir` helper creates directories — make sure it uses the
  `base` path too, or inline the `create_dir_all` call.

### 4.2  Tasks 3.1–3.8 — Test portage config writing

Add tests to `tests/worker_setup_test.rs` for each `_inner` function.

**Test pattern:**
```rust
#[tokio::test]
async fn write_make_conf_golden_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();
    std::fs::create_dir_all(base.join("package.use")).unwrap();

    let config = common::fixtures::full_portage_config();
    portage_setup::write_make_conf_inner(&base, &config, "x86_64-pc-linux-gnu", None, None)
        .await
        .expect("write_make_conf_inner");

    let content = std::fs::read_to_string(base.join("make.conf")).unwrap();
    assert!(content.contains("CHOST="), "must have CHOST");
    assert!(content.contains("CFLAGS="), "must have CFLAGS");
    assert!(content.contains("USE="), "must have USE");
    // ... assert all expected lines
}
```

### 4.3  Task 4.0 — Fix silent test skipping

The Phase 4 tests in `tests/server_api_test.rs` use this pattern:

```rust
if !require_docker() { return; }
```

This makes tests silently pass without running. Options:

**Recommended approach:**
1. Gate Phase 4 tests behind `#[cfg(feature = "integration")]`.
2. Add a sentinel test in `server_api_test.rs`:
   ```rust
   #[cfg(feature = "integration")]
   #[test]
   fn docker_must_be_available_for_integration() {
       assert!(
           common::server::docker_available(),
           "Docker is required for integration tests but was not found"
       );
   }
   ```
3. Keep the `require_docker()` pattern as a safety net for running
   without the feature flag.

### 4.4  Task 5.1 — Fix no-op Docker test

Replace `docker_availability_check` in `tests/docker_test.rs`:

```rust
// REMOVE this no-op test:
#[test]
fn docker_availability_check() { /* always passes */ }

// The existing docker_manager_connects test is the real implementation.
```

### 4.5  Task 4.9 — WebSocket test

```rust
use tokio_tungstenite::connect_async;

#[tokio::test]
async fn websocket_connects_and_receives_events() {
    if !require_docker() { return; }
    let Some(server) = TestServer::start().await else { return; };

    // Submit workorder
    let resp = submit_workorder(&server).await;
    let ws_url = resp.progress_ws_url.replace("http://", "ws://");

    // Connect WebSocket
    let (mut ws, _) = connect_async(&ws_url).await.expect("ws connect");

    // Cancel the workorder to trigger a status change event
    cancel_workorder(&server, resp.workorder_id).await;

    // Read frames with timeout
    let frame = with_timeout(Duration::from_secs(5), ws.next()).await;
    // Assert it's a text frame with status change info
}
```

### 4.6  Task 0.6 — GHCR test Docker image

Create `docker/test-stage3.Dockerfile`:

```dockerfile
FROM gentoo/stage3:latest

# Sync portage tree (this is the slow part — ~5min)
RUN emerge --sync

# Install test dependencies
RUN emerge -1 app-misc/hello cpuid2cpuflags

# Create a known VDB state for is_installed tests
# (app-misc/hello and its deps will already be in /var/db/pkg/)

# Create world file for expand_set tests
RUN echo "app-misc/hello" >> /var/lib/portage/world

# Verify portageq works
RUN portageq envvar USE

LABEL org.opencontainers.image.source=https://github.com/k-forss/remerge
LABEL org.opencontainers.image.description="Remerge integration test image"
```

Create `.github/workflows/test-image.yml`:

```yaml
name: Build Test Image

on:
  push:
    paths: ['docker/test-stage3.Dockerfile']
  workflow_dispatch: {}

permissions:
  contents: read
  packages: write

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - uses: docker/build-push-action@v5
        with:
          context: .
          file: docker/test-stage3.Dockerfile
          push: true
          tags: ghcr.io/${{ github.repository }}/test-stage3:latest
```

### 4.7  Tasks 7.5–7.6 — Error path tests (no Docker needed)

Add to `tests/error_test.rs`:

```rust
/// Docker socket unavailable returns error, not panic.
#[tokio::test]
async fn docker_socket_unavailable_returns_error() {
    let config = remerge_server::config::ServerConfig {
        docker_socket: "/nonexistent/docker.sock".into(),
        ..Default::default()
    };
    let result = remerge_server::docker::DockerManager::new(&config).await;
    assert!(result.is_err(), "should error on bad socket");
}
```

### 4.8  Tasks 2.1–2.6 — Gentoo-specific reader tests

These tests require `portageq` which is only available on Gentoo.
Two strategies:

**For fast CI (non-Gentoo):**
- Test the fallback code paths where `portageq` fails.
- `PortageReader::new()` succeeds even without portageq — it just
  can't resolve some variables.
- Write tests that exercise the file-reading logic without portageq.

**For full E2E CI (with GHCR image):**
- Run inside the test stage3 container where portageq is available.
- Gate behind `#[cfg(feature = "e2e")]`.

### 4.9  Task 2.8 — `expand_set` tests

Split into two:

```rust
/// @world expansion reads the world file — no portageq needed.
#[test]
fn expand_set_world() {
    let (_tmp, root) = common::fixtures::portage_tree();
    unsafe { std::env::set_var("ROOT", root.to_str().unwrap()); }
    let reader = remerge::portage::PortageReader::new().unwrap();
    let atoms = reader.expand_set("@world");
    assert!(!atoms.is_empty(), "world set should expand");
    assert!(atoms.contains(&"dev-libs/openssl".to_string()));
}

/// @system expansion requires portageq — E2E only.
#[cfg(feature = "e2e")]
#[test]
fn expand_set_system() {
    // ... run inside Gentoo container
}
```

---

## 5  Key Source Files Reference

| File | What to learn |
|------|---------------|
| `crates/types/src/portage.rs` | `PortageConfig`, `MakeConf`, `SystemIdentity`, field names, serde |
| `crates/types/src/workorder.rs` | `Workorder`, `WorkorderStatus`, `BuildEvent`, `WorkorderResult`, `BuiltPackage`, `FailedPackage` |
| `crates/types/src/validation.rs` | `validate_atom`, `AtomValidationError` |
| `crates/types/src/client.rs` | `ClientRole`, `ClientState`, `ConfigDiff` |
| `crates/types/src/auth.rs` | `AuthMode` |
| `crates/types/src/api.rs` | API request/response types: `SubmitWorkorderRequest`, `SubmitWorkorderResponse`, `WorkorderStatusResponse`, `ListWorkordersResponse`, `CancelWorkorderResponse`, `HealthResponse`, `ServerInfoResponse` |
| `crates/cli/src/portage.rs` | `PortageReader`, `is_installed`, `expand_set`, `split_name_version`, `compare_versions`, `parse_atom_operator`, `split_revision`, `AtomOp` |
| `crates/server/src/api.rs` | `router()` — axum routes and handlers |
| `crates/server/src/config.rs` | `ServerConfig` — all fields have serde defaults |
| `crates/server/src/state.rs` | `AppState::new()` — requires Docker for `DockerManager` |
| `crates/server/src/docker.rs` | `DockerManager::new()`, `image_tag()`, `image_needs_rebuild()`, `build_worker_image()`, `start_worker()`, `remove_container()`, `remove_image()`, `stop_container()` |
| `crates/server/src/main.rs` | `run_eviction_task()` — background workorder eviction (lines 177–260) |
| `crates/worker/src/portage_setup.rs` | `apply_config()`, all `write_*` and `*_inner` functions |
| `crates/worker/src/builder.rs` | `build_packages()`, `sync_repos()`, arg filtering |
| `tests/common/mod.rs` | `free_port()`, `with_timeout()` |
| `tests/common/fixtures.rs` | `portage_tree()`, `vdb_tree()`, `minimal_portage_config()`, `full_portage_config()`, `minimal_system_identity()` |
| `tests/common/server.rs` | `docker_available()`, `TestServer::start()` |

---

## 6  Code Quality Requirements

- All tests must pass `cargo clippy --all-targets -- -D warnings`.
- All tests must be formatted with `cargo fmt`.
- Use `#[tokio::test]` for async tests.
- Use `assert!`, `assert_eq!`, `assert_ne!` with descriptive messages.
- Every test function has a `/// doc comment` explaining what it verifies.
- No `.unwrap()` in production code — use `anyhow::Result` in tests where
  appropriate, `.unwrap()` / `.expect()` is fine in test code.
- Clean up temp dirs and Docker resources in `Drop` guards.
- Follow the code style in `CONTRIBUTING.md`.

---

## 7  Dependencies

These are already configured in the workspace `Cargo.toml`:

```toml
[dev-dependencies]
remerge-types = { path = "crates/types" }
remerge = { path = "crates/cli" }
remerge-worker = { path = "crates/worker" }
remerge-server = { path = "crates/server" }
serde_json = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true }
uuid = { workspace = true }
reqwest = { workspace = true }
axum = { workspace = true }
```

If you need `tokio-tungstenite` for WebSocket tests, add it:
```toml
tokio-tungstenite = "0.26"
```
to both `[workspace.dependencies]` and `[dev-dependencies]`.

If you need `bollard` in integration tests, add it to `[dev-dependencies]`:
```toml
bollard = { workspace = true }
```

---

## 8  Execution Order

Work through tasks in this order for maximum unblocking:

1. **Task 3.0** — Create `_inner` variants (unblocks 3.1–3.8, 3.13)
2. **Tasks 3.1–3.8** — Portage config writing tests
3. **Task 4.0** — Fix silent skipping
4. **Task 5.1** — Fix no-op Docker test
5. **Tasks 4.9, 4.10** — WebSocket + auth tests
6. **Tasks 7.5, 7.6** — Error paths (no Docker needed)
7. **Task 2.8** — `expand_set` (world part, no portageq needed)
8. **Task 0.6** — GHCR test image
9. **Tasks 0.5, 8.1** — CI jobs
10. **Tasks 5.3–5.7** — Docker lifecycle tests
11. **Tasks 7.1–7.4, 7.7, 7.8, 7.12** — Remaining error paths
12. **Phase 6** — E2E tests
13. **Tasks 8.2–8.5** — CI optimization

---

## 9  Constraints

- Do **not** modify existing source code unless:
  - A function needs a `pub` `_inner` variant for testability
    (task 3.0, following the established pattern).
  - A function needs `pub` visibility to be callable from integration
    tests (e.g., `pub(crate)` → `pub`). Minimize changes.
- Do **not** add trait abstractions or mock layers to production code.
- Do **not** add dependencies not listed in §7.
- Do **not** create Docker images during Phase 1–4 tests.
- Follow the code style in `CONTRIBUTING.md`.
- After every batch of changes, verify with:
  ```sh
  cargo test --workspace 2>&1 | tail -5    # quick check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  ```

---

## 10  Verification Checklist

After implementing all tasks, verify:

- [ ] `cargo test --workspace` passes (Phases 1–4)
- [ ] `cargo test --workspace --features integration` passes with Docker
      (Phases 1–5)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] No test depends on network access (except E2E)
- [ ] No test leaves behind temp files, Docker containers, or images
- [ ] Every `PortageConfig` field is exercised in at least one test
- [ ] Every `validate_atom` rejection class has a dedicated test
- [ ] Every `is_installed` operator variant has at least 2 tests
- [ ] Every server API route has at least one test
- [ ] Path traversal rejection is tested for `profile_overlay` and `patches`
- [ ] Feature-gated tests skip gracefully when prerequisites are missing
- [ ] Phase 4 tests don't silently pass when Docker is unavailable
- [ ] `e2e_test.rs` has real test logic, not just placeholders

---

## 11  File Listing

When done, these files should exist (★ = needs changes, ✓ = exists and done):

```
tests/
  common/
    mod.rs           ✓ (may need minor additions)
    fixtures.rs      ✓ (may need minor additions)
    server.rs        ★ (may need TestServer variants for auth tests)
  types_test.rs      ✓
  cli_portage_test.rs ★ (add expand_set tests)
  worker_setup_test.rs ★ (add 3.1–3.8 tests)
  server_api_test.rs   ★ (add 4.0, 4.9, 4.10, 4.12 tests)
  docker_test.rs       ★ (fix 5.1, add 5.3–5.7 tests)
  e2e_test.rs          ★ (replace placeholder with real tests)
  error_test.rs        ★ (add 7.1–7.8, 7.12 tests)

crates/worker/src/portage_setup.rs ★ (add _inner variants for task 3.0)

docker/
  test-stage3.Dockerfile  ★ (NEW — task 0.6)

.github/workflows/
  ci.yml                  ★ (add integration-test job — task 0.5)
  test-image.yml          ★ (NEW — task 0.6)

TASKS.md                  ✓ (check off tasks as you complete them)
```
