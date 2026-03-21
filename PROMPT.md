# Integration Test Suite — Agent Prompt

You are implementing a comprehensive integration test suite for **remerge**,
a distributed Gentoo binary-package builder written in Rust.  This document
is your single source of truth.  Read it in full before writing any code.

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
  → HTTP POST to server /api/v1/workorder
  → server queues Workorder, builds/reuses Docker image
  → starts container with REMERGE_WORKORDER env var (JSON)
  → worker deserializes, writes /etc/portage/ inside container
  → worker runs `emerge --buildpkg …`
  → PTY output streamed back via Docker attach → WebSocket → CLI
  → binpkgs written to shared volume
  → CLI runs local `emerge --getbinpkg` to install
```

---

## 2  Existing Test Coverage

All existing tests are inline `#[cfg(test)]` unit tests (72 total).  There
are **no integration tests**, **no test fixtures**, **no mock infrastructure**,
and **no `tests/` directory**.

| Module | Tests | What they cover |
|--------|-------|-----------------|
| `types/validation.rs` | 11 | `validate_atom` |
| `types/portage.rs` | 2 | Serde round-trips |
| `server/auth.rs` | 14 | All 3 auth modes, fingerprint normalisation |
| `server/registry.rs` | 7 | Client registry (async) |
| `worker/portage_setup.rs` | 8 | MAKEOPTS, USE flags, repo section parsing |
| `cli/config.rs` | 5 | Config file parsing, persistence |
| `cli/portage.rs` | 10 | make.conf parsing, `split_name_version`, version comparison |

### Gaps to fill

- CLI portage reader (`read_config`, `is_installed`, set expansion)
- Server HTTP/WS API endpoints
- Server queue processor
- Docker lifecycle (build, start, attach, remove, image eviction)
- Worker builder (`build_packages`, `sync_repos`)
- End-to-end CLI → binpkg pipeline
- Error paths (missing deps, USE conflicts, path traversal, shell injection)

---

## 3  Architecture Decisions

### 3.1  Test location

Create a top-level `tests/` directory in the workspace root.  Use a single
integration test crate with multiple modules:

```
tests/
  common/
    mod.rs          — shared helpers
    fixtures.rs     — fixture builders
    server.rs       — in-process server harness
  types_test.rs     — Phase 1 (pure logic)
  cli_portage_test.rs — Phase 2 (filesystem)
  worker_setup_test.rs — Phase 3 (filesystem)
  server_api_test.rs   — Phase 4 (in-process HTTP)
  docker_test.rs       — Phase 5 (Docker daemon)
  e2e_test.rs          — Phase 6 (full pipeline)
  error_test.rs        — Phase 7 (error paths)
```

### 3.2  Feature gating

- Phases 1–4 run with `cargo test` (no feature flag).
- Phase 5 (Docker) is gated behind `#[cfg(feature = "integration")]`.
- Phase 6 (E2E) is gated behind `#[cfg(feature = "e2e")]`.
- Add these features to the workspace `Cargo.toml`.

### 3.3  Test helpers (`tests/common/mod.rs`)

Provide the following reusable utilities:

```rust
/// Allocate a random free TCP port and return the address.
pub fn free_port() -> u16;

/// Create a temp directory with a populated `/etc/portage/` tree.
/// Returns (TempDir, PathBuf) — keep TempDir alive for the test duration.
pub fn portage_tree() -> (TempDir, PathBuf);

/// Create a temp directory with a populated `/var/db/pkg/` VDB.
pub fn vdb_tree(packages: &[(&str, &str)]) -> (TempDir, PathBuf);

/// Build a minimal PortageConfig with sensible defaults.
pub fn minimal_portage_config() -> PortageConfig;

/// Build a minimal ServerConfig pointing at temp dirs.
pub fn minimal_server_config(port: u16, binpkg_dir: &Path) -> ServerConfig;

/// Start the axum server in-process and return a handle.
pub async fn start_test_server(config: ServerConfig) -> TestServer;

/// Assert with a timeout — panics if the future doesn't resolve.
pub async fn with_timeout<F: Future>(duration: Duration, f: F) -> F::Output;
```

### 3.4  In-process server harness

For Phase 4 tests, start the axum app on `127.0.0.1:<free_port>` inside the
test process.  Use `reqwest` as the HTTP client and `tokio-tungstenite` for
WebSocket tests.  Do **not** spawn a separate OS process.

### 3.5  Docker tests

For Phase 5, connect to the local Docker socket via `bollard`.  Tag test
images with `remerge-test-<uuid>` and clean up in a `Drop` guard.  Skip
tests if Docker is not available (`bollard::Docker::connect_with_local_defaults()`
fails → skip, don't panic).

---

## 4  Key Source Files to Read

Before writing any test, read these files to understand the real interfaces:

| File | What to learn |
|------|---------------|
| `crates/types/src/portage.rs` | `PortageConfig`, `MakeConf`, `SystemId`, field names, serde annotations |
| `crates/types/src/workorder.rs` | `Workorder`, `WorkorderStatus`, `WorkorderResult`, `BuiltPackage`, `FailedPackage` |
| `crates/types/src/validation.rs` | `validate_atom` — all acceptance/rejection rules |
| `crates/types/src/client.rs` | `ClientRole`, `ClientState`, `ConfigDiff` |
| `crates/types/src/auth.rs` | `AuthMode` enum |
| `crates/cli/src/portage.rs` | `PortageReader`, `read_config`, `is_installed`, `expand_set`, `split_name_version`, `compare_versions`, `parse_atom_operator`, `split_revision` |
| `crates/cli/src/args.rs` | `Cli` struct, `run()`, `extract_atoms()`, `run_local_emerge()` |
| `crates/cli/src/cflags.rs` | `resolve_flags()` — march=native resolution |
| `crates/cli/src/config.rs` | `CliConfig`, `load_or_create()` |
| `crates/server/src/main.rs` | Server bootstrap, background tasks, axum router |
| `crates/server/src/config.rs` | `ServerConfig` fields, env var overrides |
| `crates/server/src/api.rs` | All HTTP/WS routes and handlers |
| `crates/server/src/auth.rs` | `AuthState`, `authenticate()`, 3 modes |
| `crates/server/src/registry.rs` | `ClientRegistry`, `register_or_update()`, follower rules |
| `crates/server/src/queue.rs` | `process_queue()`, `process_workorder()`, event parsing |
| `crates/server/src/docker.rs` | `DockerManager`, image build, container lifecycle |
| `crates/server/src/state.rs` | `AppState` — central shared state |
| `crates/worker/src/main.rs` | Worker bootstrap, cross-compilation detection |
| `crates/worker/src/portage_setup.rs` | `apply_config()`, `write_make_conf()`, `set_profile()`, `write_profile_overlay()`, `write_patches()`, `write_repos_conf()` |
| `crates/worker/src/builder.rs` | `build_packages()`, `sync_repos()`, arg filtering |
| `crates/worker/src/crossdev.rs` | `emerge_command()`, `setup_crossdev()` |

---

## 5  Detailed Test Specifications

### Phase 1 — Types & Validation (no I/O)

**Goal:** Verify all shared types serialize, deserialize, validate, and
display correctly.

1. **PortageConfig round-trip** — construct a config with every field
   populated (including `profile_overlay`, `patches`, `repos_conf`,
   `use_flags_resolved = true`).  Serialize to JSON, deserialize, assert
   field-by-field equality.

2. **PortageConfig with defaults** — deserialize a minimal JSON object
   `{}` into `PortageConfig`.  Assert `profile_overlay` is an empty
   `BTreeMap`, `use_flags_resolved` is `false`, etc.

3. **Workorder status transitions** — create a Pending workorder, walk it
   through every status.  Verify timestamps are set correctly.

4. **validate_atom exhaustive** — test every legal form:
   - Qualified: `dev-libs/openssl`
   - Versioned: `=dev-libs/openssl-3.1.0`, `>=dev-libs/openssl-3.0`,
     `~dev-libs/openssl-3.1.0`, `=dev-libs/openssl-3.1*`
   - Sets: `@world`, `@system`
   - Unqualified: `gcc`
   - Rejection: empty, `; rm -rf`, `$(evil)`, `` `evil` ``,
     `=gcc-12` (versioned unqualified)

5. **ClientRole / AuthMode Display+FromStr** — round-trip through string.

### Phase 2 — CLI Portage Reader (filesystem)

**Goal:** Test `PortageReader` against a synthetic portage tree in a temp
directory.

**Setup pattern:**
```rust
let tmp = tempdir()?;
let root = tmp.path();
// Create /etc/portage/make.conf, package.use/, repos.conf, etc.
std::env::set_var("ROOT", root);
let reader = PortageReader::new()?;
let config = reader.read_config()?;
```

Key tests:

1. **Full golden path** — populate every directory and file a real Gentoo
   system would have.  Call `read_config()` and assert every field.

2. **Missing optional directories** — only create `make.conf`.  Assert
   `package_use` is empty, `patches` is empty, `profile_overlay` is empty.

3. **`is_installed` version constraints** — create VDB entries in
   `/var/db/pkg/dev-libs/openssl-3.1.0-r2/` and
   `/var/db/pkg/dev-libs/openssl-1.1.1w/`, then test:
   - `dev-libs/openssl` → true (any version)
   - `=dev-libs/openssl-3.1.0-r2` → true (exact)
   - `=dev-libs/openssl-3.1.0` → false (different revision in VDB)
   - `>=dev-libs/openssl-3.0` → true
   - `<dev-libs/openssl-2.0` → true (1.1.1w matches)
   - `>dev-libs/openssl-4.0` → false
   - `~dev-libs/openssl-3.1.0` → true (any revision of 3.1.0)
   - `=dev-libs/openssl-3.1*` → true (glob)
   - `=dev-libs/openssl-3.2*` → false
   - `@world` → false (set)
   - `dev-libs/nonexistent` → false

4. **`read_profile_overlay`** — create files in `<root>/etc/portage/profile/`
   including nested subdirectories.  Assert the BTreeMap contains all
   relative paths as keys and file contents as values.

5. **`read_patches_recursive`** — create patches in
   `<root>/etc/portage/patches/dev-libs/openssl/fix.patch`.  Assert key
   format and content.

### Phase 3 — Worker Portage Setup (filesystem)

**Goal:** Test worker-side portage config writing against a temp directory.

**Setup pattern:**
```rust
let tmp = tempdir()?;
let root = tmp.path();
std::fs::create_dir_all(root.join("etc/portage"))?;
// call write_make_conf, write_package_config, etc.
// then read files back and assert content
```

Key tests:

1. **`write_make_conf` golden path** — pass a fully-populated `MakeConf`
   with CHOST, CBUILD, CFLAGS, USE, FEATURES, signing, use_expand.
   Read generated file, assert every expected line is present.

2. **`write_make_conf` with `use_flags_resolved = true`** — USE line must
   start with `USE="-* flag1 flag2"`.

3. **`write_make_conf` with `use_flags_resolved = false`** — USE line must
   NOT have `-*` prefix.

4. **`write_make_conf` with USE_EXPAND** — verify `VIDEO_CARDS="intel"`,
   `INPUT_DEVICES="libinput"` appear as separate variables.

5. **`write_package_config`** — for each type (use, keywords, license,
   mask, unmask, env), write multi-entry configs and read back.

6. **`write_repos_conf` with remapping** — set server repos_dir, verify
   location values are rewritten.

7. **`set_profile`** — create repo directory structures with
   `profiles/<profile>` paths, call `set_profile`, verify symlink target.

8. **`write_profile_overlay`** — write overlay files, verify they appear
   in `/etc/portage/profile/`.

9. **`write_profile_overlay` path traversal** — keys containing `..` or
   starting with `/` must be rejected.

10. **`write_patches`** — write patches, verify directory structure and
    file content.

11. **`apply_config` orchestration** — call `apply_config` with a full
    `PortageConfig` and verify that all expected files exist.

### Phase 4 — Server API (in-process, no Docker)

**Goal:** Test the axum HTTP/WS API without Docker.

**Approach:** The server's queue processor calls `DockerManager` — for these
tests you need to either:
- (a) Let the queue idle (don't submit workorders that reach "Building"), or
- (b) Submit workorders and test only the API-level response (status codes,
  JSON shape, auth enforcement) without waiting for completion.

Key tests:

1. **Submit workorder** — POST valid JSON, assert 200, parse workorder ID.
2. **Submit invalid atoms** — POST with `; rm -rf /`, assert 400.
3. **Duplicate active** — submit twice with same client, assert 409.
4. **Get workorder** — fetch by ID, assert fields match.
5. **List workorders** — submit several, list, assert count and ordering.
6. **Cancel** — submit, cancel, verify status = Cancelled.
7. **Health** — GET /health → 200.
8. **Info** — GET /api/v1/info → version, binhost_url, auth_mode.
9. **Auth: None mode** — all requests pass.
10. **Auth: Mtls mode** — missing cert header → 401.
11. **WebSocket** — connect, verify text frame with initial status event.
12. **Metrics** — GET /metrics → contains `remerge_` prefix lines.
13. **Client registry** — submit as follower without main → error.

### Phase 5 — Docker Integration

**Gate:** `#[cfg(feature = "integration")]`

**Pre-condition check:**
```rust
fn docker_available() -> bool {
    bollard::Docker::connect_with_local_defaults().is_ok()
}
```
Skip all tests if Docker is not available.

Key tests:

1. **Connect** — `DockerManager::new()` succeeds.
2. **Build image** — build from Gentoo stage3, verify image exists.
3. **Rebuild detection** — `needs_rebuild` returns `false` after build,
   `true` after binary hash change.
4. **Start container** — verify env vars and mounts.
5. **Cleanup** — remove container, verify gone.
6. **Image eviction** — create multiple images, run cleanup, verify only
   newest per group survives.

### Phase 6 — End-to-End

**Gate:** `#[cfg(feature = "e2e")]`

These are slow (~minutes).  The test process must:
1. Start the server in-process.
2. Use the CLI client library (not the binary) to submit a workorder.
3. Wait for WebSocket completion.
4. Assert binpkg file exists with correct SHA-256.

Candidate packages for E2E: `app-misc/hello`, `virtual/libc` (fast builds).

### Phase 7 — Error Paths

Intentionally trigger every known error path:

1. **Worker exit non-zero** — assert `WorkorderStatus::Failed`.
2. **Missing dep** — assert `missing_dependencies` event.
3. **USE conflict** — assert `use_conflicts` event.
4. **Path traversal in profile_overlay** — assert rejection.
5. **Path traversal in patches** — assert rejection.
6. **Shell injection in atoms** — assert validation rejects.
7. **Workorder TTL expiry** — advance time, verify reaping.

---

## 6  Code Quality Requirements

- All tests must pass `cargo clippy --all-targets -- -D warnings`.
- All tests must be formatted with `cargo fmt`.
- Use `#[tokio::test]` for async tests.
- Use `assert!`, `assert_eq!`, `assert_ne!` with descriptive messages.
- Every test function has a `/// doc comment` explaining what it verifies.
- No `.unwrap()` in production code — use `anyhow::Result` in tests.
- Clean up temp dirs and Docker resources in `Drop` guards.

---

## 7  Dependencies to Add

Add these to the workspace `Cargo.toml` under `[workspace.dependencies]`:

```toml
tempfile = "3"                # temp dirs for filesystem tests
tokio-tungstenite = "0.26"    # WebSocket client for API tests
```

And in whatever crate hosts the integration tests:

```toml
[dev-dependencies]
tempfile = { workspace = true }
tokio-tungstenite = { workspace = true }
reqwest = { workspace = true }
bollard = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
uuid = { workspace = true }
remerge-types = { workspace = true }
```

---

## 8  Audit Checklist

After implementing all tests, verify:

- [ ] `cargo test --workspace` passes (Phases 1–4)
- [ ] `cargo test --workspace --features integration` passes when Docker is
      available (Phases 1–5)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] No test depends on network access (except E2E)
- [ ] No test leaves behind temp files, Docker containers, or images
- [ ] Every `PortageConfig` field is exercised in at least one test
- [ ] Every `validate_atom` rejection class has a dedicated test
- [ ] Every `is_installed` operator variant has at least 2 tests
      (satisfied + unsatisfied)
- [ ] Every server API route has at least one test
- [ ] Path traversal rejection is tested for `profile_overlay` and `patches`
- [ ] Feature-gated tests skip gracefully when prerequisites are missing

---

## 9  File Listing

When you are done, the following files should exist:

```
tests/
  common/
    mod.rs
    fixtures.rs
    server.rs
  types_test.rs
  cli_portage_test.rs
  worker_setup_test.rs
  server_api_test.rs
  docker_test.rs
  e2e_test.rs
  error_test.rs
TASKS.md            (already exists — check off completed items)
```

---

## 10  Constraints

- Do **not** modify any existing source code unless a function is not `pub`
  and needs to be made accessible for testing.  If you change visibility,
  do it minimally (e.g. `pub(crate)` → `pub`).
- Do **not** add trait abstractions or mock layers to production code.
  Test at the boundary (filesystem, HTTP, Docker socket).
- Do **not** add any dependencies that are not listed in §7.
- Do **not** create Docker images during Phase 1–4 tests.
- Follow the code style in `CONTRIBUTING.md`.
