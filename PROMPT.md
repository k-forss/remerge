# Integration Test Suite — Agent Prompt

You are implementing integration tests for **remerge**, a distributed
Gentoo binary-package builder written in Rust.  Previous agents completed
roughly 75 % of the tasks.  Your job is to finish the remaining **21
unchecked `[ ]` items** in `TASKS.md`.

> **HARD RULES — violation of any of these makes your work invalid:**
>
> 1. `TASKS.md` is the single source of truth.  Read it FIRST.
> 2. You are NOT finished until §10 verification passes AND you have
>    reported results line by line.
> 3. Do NOT mark a task `[x]` unless the test compiles, runs, and has
>    meaningful assertions.  "Deferred" is NOT "done."
> 4. Do NOT batch-check tasks.  One at a time, verified, then checked.
> 5. The stage3 test image may not be published to GHCR yet.  Build
>    it locally with `docker build -f docker/test-stage3.Dockerfile
>    -t ghcr.io/k-forss/remerge/test-stage3:latest .` — do NOT mark
>    tasks as "Blocked" just because the GHCR image isn't pushed.
> 6. Test failures are EXPECTED and DESIRED.  The goal is to find bugs
>    in the production code.  A test that compiles, runs, and asserts
>    the correct behavior — but fails because the production code is
>    broken — is STILL a valid `[x]`.  Document the failure.
> 7. Before stopping, run the §10 checklist and report EVERY line.

---

## 1  Completion Protocol

```
LOOP:
  1. Read TASKS.md, find the highest-priority unchecked [ ] task.
  2. Read the source files needed (use grep/read_file — do NOT guess).
  3. Write the test (or source change).
  4. Run: cargo test --workspace 2>&1 | tail -20
  5. If the test PASSES with real assertions → mark [x] in TASKS.md.
  6. If it FAILS due to a TEST bug → fix the test, re-run, then mark [x].
  7. If it FAILS due to a PRODUCTION CODE bug → mark [x], add a
     "Known failure:" note documenting the bug.  This is expected.
  8. If it CANNOT be implemented (e.g., needs QEMU) → leave [ ],
     add "Blocked:" note.
  9. Every 3–5 tasks: run cargo clippy --workspace --all-targets -D warnings
  10. GOTO 1.

STOP ONLY WHEN:
  - Every [ ] in TASKS.md is either [x] or has a "Blocked:" note.
  - §10 verification checklist passes.
  - You have posted the checklist results.
```

---

## 2  Project Overview

Remerge is a Rust workspace (edition 2024, rust-version 1.88) with four
crates:

| Crate | Path | Role |
|-------|------|------|
| `remerge` | `crates/cli` | CLI binary — drop-in `emerge` wrapper |
| `remerge-server` | `crates/server` | HTTP/WS API, Docker orchestration |
| `remerge-worker` | `crates/worker` | Runs inside Docker, applies portage config, executes `emerge` |
| `remerge-types` | `crates/types` | Shared types: portage, workorder, validation, client, auth |

**Data flow:**

```
CLI reads /etc/portage/ → serializes PortageConfig
  → POST /api/v1/workorders to server
  → server queues Workorder, builds/reuses Docker image
  → starts container with REMERGE_WORKORDER env
  → worker writes /etc/portage/ inside container, runs emerge --buildpkg
  → PTY output → WebSocket → CLI
  → binpkgs to shared volume
  → CLI runs emerge --getbinpkg to install
```

---

## 3  Current State (audited — accurate as of this writing)

### 3.1  Test infrastructure

```
tests/
  common/
    mod.rs          — free_port(), with_timeout()
    fixtures.rs     — portage_tree(), vdb_tree(), minimal_portage_config(),
                      full_portage_config(), minimal_system_identity()
    server.rs       — docker_available(), TestServer::start(),
                      TestServer::start_with_config()
  types_test.rs          — Phase 1: 27+ tests ✅
  cli_portage_test.rs    — Phase 2: 19+ tests ✅
  worker_setup_test.rs   — Phase 3: 38+ tests ✅ (includes
                           ensure_repo_locations_inner 4 tests,
                           apply_config_inner, parse_repo_sections)
  server_api_test.rs     — Phase 4: 16+ tests ✅ (all behind
                           #[cfg(feature = "integration")])
  docker_test.rs         — Phase 5: 6 tests ✅ (connect, image_tag,
                           needs_rebuild, remove/stop error paths)
  e2e_test.rs            — Phase 6: 5 tests behind #[cfg(feature = "e2e")]
                           (API-level only — no full pipeline tests)
  error_test.rs          — Phase 7: 16+ tests ✅
```

### 3.2  Source infrastructure

- `Cargo.toml`: `integration` and `e2e` feature flags, all dev-deps.
- All 17 `_inner`/`pub` functions in `portage_setup.rs` including
  `ensure_repo_locations_inner` and `apply_config_inner`.
- `docker/test-stage3.Dockerfile` + `.github/workflows/test-image.yml`
  push to `ghcr.io/k-forss/remerge/test-stage3:latest`.
- `.github/workflows/ci.yml` has `integration-test` and `smoke-test` jobs.

### 3.3  Checked vs unchecked tasks (verified by code audit)

**66 tasks checked `[x]` (do NOT re-implement):**
- Phase 0 (0.1–0.6): ✅ all 6
- Phase 1 (1.1–1.6): ✅ all 6
- Phase 2 (2.1–2.10): ✅ all 10
- Phase 3 (3.0–3.13): ✅ all 14
- Phase 4 (4.0–4.14): ✅ all 15
- Phase 5: ✅ 5.1, 5.2, 5.4, 5.6
- Phase 6: ✅ 6.7, 6.9
- Phase 7: ✅ 7.5, 7.9–7.14
- Phase 8: ✅ 8.1, 8.3

**21 tasks unchecked `[ ]` (your work):**

| ID | Task | Dep | Priority |
|----|------|-----|----------|
| 7.6 | Server config validation errors | None | **P1** |
| 8.4 | Full integration CI job (main-only, e2e) | None | **P1** |
| 7.7 | Workorder TTL eviction | Docker | P2 |
| 7.8 | Max retained workorders cap | Docker | P2 |
| 5.3 | `build_worker_image` — verify image + label | Docker + local stage3 | P2 |
| 5.5 | `start_worker` — container runs, env/mounts correct | 5.3 | P2 |
| 5.7 | Image eviction — `cleanup_idle_images` | 5.3 | P2 |
| 6.8 | Worker binary upgrade detection | 5.3 | P2 |
| 6.1 | Build `app-misc/hello` — verify binpkg + SHA-256 | local stage3 | P3 |
| 6.2 | Build with `--pretend`/`--ask` flags | local stage3 | P3 |
| 6.3 | Build with custom USE flags | local stage3 | P3 |
| 6.4 | Build with `@world` — set expansion | local stage3 | P3 |
| 6.6 | Follower client — inherits main config | local stage3 | P3 |
| 6.10 | WebSocket reconnect — streaming continues | local stage3 | P3 |
| 7.1 | Worker exits non-zero → `Failed` status | local stage3 | P3 |
| 8.2 | Cache stage3 in CI | CI workflow | P3 |
| 7.2 | Missing dependency → `missing_dependencies` event | local stage3 | P4 |
| 7.3 | USE conflict → `use_conflicts` event | local stage3 | P4 |
| 7.4 | Fetch failure → `fetch_failures` event | local stage3 | P4 |
| 6.5 | Cross-arch build — crossdev setup | QEMU user-static | P4 |
| 8.5 | Test duration tracking with nextest | Setup task | P4 |

> **"local stage3" means:** build locally with
> `docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .`
> This is slow (~10–20 min) but removes any GHCR dependency.

---

## 4  Implementation Guide — by Priority

### P1 — No external dependencies (do these first)

#### Task 7.6 — Server config validation errors

Tests go in `tests/error_test.rs`.  Existing auth tests are correct —
keep them.  What's missing:

1. **Non-writable `binpkg_dir` through AppState::new():**
   `AppState::new()` in `state.rs` calls `create_dir_all(binpkg_dir)`.
   `DockerManager::new()` may be called first and fail if Docker is
   unavailable.  Two options:
   - If Docker is available: create `ServerConfig` with
     `binpkg_dir: "/proc/nonexistent/binpkgs"`, call `AppState::new()`,
     assert error.  Gate behind `#[cfg(feature = "integration")]`.
   - If Docker is NOT available: extract the dir-creation logic into
     a testable helper, or test the `create_dir_all` call directly
     WITH the actual `ServerConfig` path (not a raw `tokio::fs` call).

2. **Non-writable `state_dir`:** Same approach as above.

3. **TLS config with missing cert files:** Check `crates/server/src/main.rs`
   for TLS setup.  If validation happens at `axum_server::bind_rustls()`,
   test by starting the server with missing cert paths and asserting
   the error.

#### Task 8.4 — Full integration CI job

Add to `.github/workflows/ci.yml`:

```yaml
  e2e-test:
    name: E2E Test
    runs-on: ubuntu-latest
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    needs: [check, test]
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Pull test image
        run: docker pull ghcr.io/k-forss/remerge/test-stage3:latest || true
      - name: Run E2E tests
        run: cargo test --workspace --features integration,e2e
```

### P2 — Requires Docker daemon

#### Task 7.7 — Workorder TTL eviction

The existing test submits/cancels a workorder but never triggers eviction.
Fix:
1. Search `crates/server/src/state.rs` or `main.rs` for
   `evict_workorders` or `reap_old_workorders`.
2. If it's a `pub` method on `AppState`, call it directly from the test
   after setting `retention_hours = 0` in `ServerConfig`.
3. If it's private, extract to `pub fn evict_workorders(&self)` for
   testability.
4. Submit workorder, cancel it, call eviction, assert it's removed (404).

#### Task 7.8 — Max retained workorders cap

Same pattern: set `max_retained_workorders = 2`, submit/cancel 3
workorders, **trigger eviction**, assert `<= 2` remain in the list.

#### Task 5.3 — `build_worker_image`

Build the local stage3 image first (see §12).  Create a dummy worker
binary (`#!/bin/true` shell script), configure `ServerConfig.worker_binary`.
After build, inspect with `bollard` for the `remerge.worker.sha256` label.
Clean up image in Drop guard.

Gate behind `#[cfg(feature = "integration")]` or `#[cfg(feature = "e2e")]`.
If the test fails because of production code bugs, that's fine — mark
`[x]` and document the failure.

#### Tasks 5.5, 5.7, 6.8 — Depend on 5.3

Implement after 5.3 is working.

### P3 — Requires Gentoo stage3 + Docker (build locally)

All tests gated behind `#[cfg(feature = "e2e")]`.  Build the stage3
image locally per §12.  **Tests that fail due to production code bugs
are expected** — the goal is to surface those bugs.  Mark `[x]` and
add a "Known failure:" note.

#### Tasks 6.1–6.3 — Build pipeline verification

The existing tests in `e2e_test.rs` only test API submission (duplicating
Phase 4 tests).  They must be extended to:
- 6.1: Connect WebSocket, wait for `Finished` event, check `binpkg_dir`
  for `.gpkg.tar`, verify SHA-256.
- 6.2: Verify `--pretend` flag passed through (check build output for
  "These are the packages" without compilation), verify `--ask` filtered.
- 6.3: Inspect container or build output for USE flag application.

#### Task 7.1 — Worker exits non-zero

Fix the assertion in `error_test.rs` — must assert
`WorkorderStatus::Failed { .. }` ONLY (not `Running | Pending`).
Increase the poll timeout to 120s for slow stage3 operations.

#### Tasks 6.4, 6.6, 6.10 — Full pipeline tests

See TASKS.md implementation notes for each.

### P4 — Low priority / complex setup

- 6.5: Cross-arch (needs QEMU user-static — only real blocker)
- 7.2–7.4: Emerge error scenarios (build locally, may fail — that's OK)
- 8.2: CI image caching (write the workflow; it works once GHCR push is live)
- 8.5: nextest setup

---

## 5  Key Source Files

| File | Key contents |
|------|-------------|
| `crates/worker/src/portage_setup.rs` | `apply_config` (L23), all `write_*_inner` functions, `ensure_repo_locations_inner` (L577), `rewrite_repo_location_inner` (L674), `parse_repo_sections` (L908), `build_makeopts_inner` (L957) |
| `crates/server/src/state.rs` | `AppState::new()` (L69), eviction logic |
| `crates/server/src/auth.rs` | `CertRegistry::new()`, `resolve()`, 14 unit tests |
| `crates/server/src/docker.rs` | `DockerManager::new()`, `image_tag()` (L94), `image_needs_rebuild()`, `build_worker_image()`, `start_worker()`, `remove_container()`, `remove_image()` |
| `crates/server/src/config.rs` | `ServerConfig` with serde defaults |
| `crates/server/src/api.rs` | `router()` (L26) — axum routes |
| `crates/types/src/portage.rs` | `PortageConfig`, `MakeConf`, `SystemIdentity` |
| `crates/types/src/validation.rs` | `validate_atom()`, `AtomValidationError` |
| `crates/types/src/api.rs` | HTTP request/response types |
| `crates/cli/src/portage.rs` | `PortageReader`, `is_installed`, `expand_set` |

---

## 6  Architecture Decisions (established — do NOT change)

- **Test location:** Top-level `tests/` directory, one file per phase.
  Shared code in `tests/common/`.
- **Feature gating:** Phases 1–3 = no flag.  Phase 4–5 =
  `#[cfg(feature = "integration")]`.  Phase 6+ = `#[cfg(feature = "e2e")]`.
- **In-process server:** `TestServer::start()` requires Docker (AppState
  creates DockerManager).
- **`_inner` pattern:** Testable function variants with base `Path`
  parameter.  Originals are one-line wrappers.
- **No mocks:** Do not add trait abstractions or mock layers.

---

## 7  Code Quality Requirements

- `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- `cargo fmt --all -- --check` must pass.
- `#[tokio::test]` for async tests.
- Every test has a `/// doc comment`.
- `.expect("descriptive msg")` — no bare `.unwrap()` in tests.
- Clean up temp dirs/containers/images in Drop guards.

---

## 8  Dependencies

Already in `Cargo.toml`:

```toml
[dev-dependencies]
remerge-types, remerge, remerge-worker, remerge-server,
serde_json, tempfile, tokio, uuid, reqwest, axum,
tokio-tungstenite, futures-util
```

Add if needed:
- `bollard = { workspace = true }` in `[dev-dependencies]` for Docker
  inspection tests (task 5.3+).

---

## 9  Constraints

- Do NOT modify production source unless adding `pub _inner` variants
  or changing `pub(crate)` → `pub` for test access.
- Do NOT add trait abstractions, mock layers, or new dependencies
  beyond those listed in §8.
- Do NOT create Docker images in Phase 1–3 tests.
- After every change:
  ```sh
  cargo test --workspace 2>&1 | tail -5
  cargo clippy --workspace --all-targets -- -D warnings
  ```

---

## 10  Verification Checklist

**Run through EVERY line before declaring work complete.**

- [ ] `cargo test --workspace` compiles and runs (failures from
      production bugs are acceptable — document them)
- [ ] `cargo test --workspace --features integration` compiles and runs
- [ ] `cargo test --workspace --features integration,e2e` compiles and runs
      (with locally-built stage3 image)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] No test uses `todo!()`, `unimplemented!()`, or empty assertions
- [ ] No test leaves behind temp files, containers, or images
- [ ] `e2e_test.rs` has real assertions (not just `eprintln!` + `return`)
- [ ] Every unchecked `[ ]` in TASKS.md is either `[x]` or has a
      "Blocked:" note explaining why it truly cannot be implemented
- [ ] Feature-gated tests skip only behind `#[cfg(feature = "...")]`
- [ ] Phase 4 sentinel test asserts Docker IS available when
      `integration` feature is enabled
- [ ] All newly added `_inner` functions have corresponding tests
- [ ] Any test failures are documented with "Known failure:" notes
      in TASKS.md explaining the production code bug

---

## 11  Anti-Pattern Blacklist

| Anti-pattern | Consequence |
|---|---|
| Marking `[x]` without running the test | Invalid — test may not compile |
| Writing `eprintln!("skip"); return;` as test body | Falsely passes — use `#[cfg(feature)]` |
| Checking `[x]` with "(deferred: …)" annotation | **THIS IS THE #1 PROBLEM.** Deferred ≠ done. Leave `[ ]`. |
| Implementing 5 tasks then batch-checking | Errors compound — verify one at a time |
| Saying "remaining tasks are straightforward" and stopping | **You are not finished.** Run §10 first. |
| Adding `#[ignore]` to skip a test | Won't run in CI without `--include-ignored` |
| Assuming a function exists without `grep`/`read_file` | Previous agents got burned by this |
| Writing assertions that always pass (e.g., `assert!(true)`) | Every assert must test a computed value |
| Allowing multiple status values in assertions (e.g., `Failed \| Running \| Pending`) | This always passes — assert the EXACT expected status |
| Declaring "all tasks complete" without checking TASKS.md | **Read the file.** Count the `[ ]` items. |

---

## 12  GHCR Test Image — Build Locally

The Docker infrastructure for Gentoo-specific tests already exists:

- `docker/test-stage3.Dockerfile`: FROM `gentoo/stage3:latest`, runs
  `emerge --sync`, installs `app-misc/hello` + `cpuid2cpuflags`.
- `.github/workflows/test-image.yml`: Builds and pushes to
  `ghcr.io/k-forss/remerge/test-stage3:latest` on Dockerfile changes.

**The image may not be published to GHCR yet.  This is NOT a blocker.**
Build it locally before running stage3-dependent tests:

```sh
# Build locally (~10–20 min, requires network for emerge --sync)
docker build -f docker/test-stage3.Dockerfile \
  -t ghcr.io/k-forss/remerge/test-stage3:latest .

# Verify it exists
docker images ghcr.io/k-forss/remerge/test-stage3

# Run all tests including E2E
cargo test --workspace --features integration,e2e
```

**Use the full GHCR tag** (`ghcr.io/k-forss/remerge/test-stage3:latest`)
when building locally so the tests find the image by the same name used
in CI.  This ensures tests work identically locally and in CI once the
image is published.

Some tests will fail because the production code has bugs — that is
expected and desired.  The purpose of implementing these tests now is
to **find** those bugs so they can be fixed.
