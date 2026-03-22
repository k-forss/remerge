# Integration Test Suite — Agent Prompt

You are continuing the implementation of a comprehensive integration test
suite for **remerge**, a distributed Gentoo binary-package builder written
in Rust.  Previous agents implemented roughly 60 % of the tasks.  Your job
is to finish the remaining unchecked `[ ]` items in `TASKS.md`.

> **CRITICAL — READ EVERY WORD OF THIS FILE BEFORE WRITING ANY CODE.**
> **`TASKS.md` is the authoritative, line-item task list.**
> **Do NOT declare yourself "finished" until the verification checklist
> in §10 is fully satisfied.**

---

## 1  Completion Rules

These rules override any default tendency to summarise or wrap up early.

1. **One task at a time.** Pick the highest-priority unchecked `[ ]` task
   in `TASKS.md`, implement it fully, verify it, then check it off `[x]`.
2. **Never batch-check.** Do not mark multiple tasks complete in one edit.
3. **Verify before checking.** After implementing a task, run the relevant
   tests and confirm they pass.  Only then mark `[x]`.
4. **No placeholders.** A test is not "done" if it contains
   `eprintln!("skipping")`, `todo!()`, `unimplemented!()`, or an early
   `return` that skips all assertions.
5. **No silent skips.** A test that `return`s early when Docker is
   unavailable is acceptable ONLY if it is behind
   `#[cfg(feature = "integration")]` or `#[cfg(feature = "e2e")]` AND a
   sentinel test asserts that the prerequisite IS available when the
   feature is enabled.
6. **After every 3–5 completed tasks**, run
   `cargo test --workspace 2>&1 | tail -20` and
   `cargo clippy --workspace --all-targets -- -D warnings` to catch
   regressions.
7. **You are NOT finished until every `[ ]` in `TASKS.md` is either
   `[x]` or explicitly deferred with a documented reason.**
8. **Before stopping**, run the full verification checklist in §10 and
   report results line by line.

---

## 2  Project Overview

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

## 3  What Already Exists (verified by audit)

### 3.1  Test infrastructure

```
tests/
  common/
    mod.rs          — free_port(), with_timeout()
    fixtures.rs     — portage_tree(), vdb_tree(), minimal_portage_config(),
                      full_portage_config(), minimal_system_identity()
    server.rs       — docker_available(), TestServer::start()
  types_test.rs     — Phase 1: ~27 tests, verified ✅
  cli_portage_test.rs — Phase 2 (partial): ~19 tests, verified ✅
  worker_setup_test.rs — Phase 3 (partial): ~38 tests, verified ✅
  server_api_test.rs   — Phase 4 (partial): ~11 tests, verified ✅
  docker_test.rs       — Phase 5 (partial): 2 tests, verified ✅
  e2e_test.rs          — Phase 6: PLACEHOLDER ONLY ❌
  error_test.rs        — Phase 7 (partial): ~16 tests, verified ✅
```

### 3.2  Source infrastructure

- `Cargo.toml`: `integration` and `e2e` feature flags, dev-dependencies
  for all four crates, tempfile, reqwest, tokio, uuid, axum.
- All `_inner` variants for worker portage_setup functions exist (13 pub
  functions in `crates/worker/src/portage_setup.rs`).
- `docker/test-stage3.Dockerfile` and `.github/workflows/test-image.yml`
  exist and push to GHCR.
- `.github/workflows/ci.yml` has `integration-test` job.

### 3.3  Checked tasks (do NOT re-implement)

Phases 0, 1 are 100 % complete.
Phase 2: 2.7–2.10 complete; 2.1–2.6 remain.
Phase 3: 3.0–3.6, 3.8–3.12 complete; 3.7 and 3.13 remain.
Phase 4: 4.0–4.8, 4.11, 4.13–4.14 complete; 4.9, 4.10, 4.12 remain.
Phase 5: 5.1–5.2 complete; 5.3–5.7 remain.
Phase 6: All remain (placeholder only).
Phase 7: 7.5, 7.9–7.11, 7.13–7.14 complete; 7.1–7.4, 7.6–7.8, 7.12 remain.
Phase 8: 8.1 complete; 8.2–8.5 remain.

---

## 4  Remaining Tasks — Priority Order

Work in this exact order.  Each item references its TASKS.md ID and
contains enough context to implement without guessing.

### Priority 1 — Tasks with no external dependencies

#### 4.1  Task 3.7 — `write_repos_conf` with repos_dir remapping

The repos_dir remapping logic is in `ensure_repo_locations()` (private,
line ~510 of `crates/worker/src/portage_setup.rs`).  It symlinks repos
from `/var/db/repos/<name>` and rewrites `location =` lines when
`REMERGE_SKIP_SYNC=1`.

**What to do:**
1. Create `pub async fn ensure_repo_locations_inner(config, repos_base,
   repos_conf_base)` following the `_inner` pattern.  Move the body of
   `ensure_repo_locations()` to use the parametric paths.  The original
   becomes a one-line wrapper.
2. Similarly create `rewrite_repo_location_inner(repos_conf_base,
   filename, old_loc, new_loc)`.
3. Add tests in `tests/worker_setup_test.rs`:
   - Repo in bind-mount → symlink created.
   - Repo NOT in bind-mount + `REMERGE_SKIP_SYNC=1` → remapped and
     `location =` line rewritten.
   - Repo NOT in bind-mount, no skip-sync → empty dir created.
   - Invalid location (traversal) → error returned.

#### 4.2  Task 7.6 — Server config validation errors

Tests go in `tests/error_test.rs`.  These do NOT require Docker unless
`AppState::new()` reaches `DockerManager` before the validation point.

**Sub-tests:**
1. `auth.mode = Mtls` without cert paths → error from `CertRegistry::new`.
   Check `crates/server/src/auth.rs` for the constructor.
2. Non-writable `binpkg_dir` (e.g. `/proc/nonexistent`) → `create_dir_all`
   in `state.rs:74` fails.
3. Non-writable `state_dir` → same pattern.
4. Invalid TLS cert paths → check if validated eagerly at startup.

If `DockerManager::new()` fails first, call the validation function
directly instead of going through `AppState::new()`.

#### 4.3  Task 4.9 — WebSocket progress test

Add to `tests/server_api_test.rs` behind `#[cfg(feature = "integration")]`.
Add `tokio-tungstenite = "0.26"` to `[workspace.dependencies]` and
`[dev-dependencies]`.

```rust
#[cfg(feature = "integration")]
#[tokio::test]
async fn websocket_progress_stream() {
    if !require_docker() { return; }
    let Some(server) = TestServer::start().await else { return; };

    // 1. Submit workorder
    // 2. Get progress_ws_url from response
    // 3. connect_async to ws_url
    // 4. Cancel the workorder to trigger StatusChanged event
    // 5. Read frames with with_timeout(5s, ws.next())
    // 6. Assert text frame contains status change
}
```

#### 4.4  Task 4.10 — Auth enforcement

Add to `tests/server_api_test.rs` behind `#[cfg(feature = "integration")]`.

- `None` mode is already implicitly tested by all existing Phase 4 tests.
- For `Mtls` mode: create a `TestServer` variant that accepts a custom
  `ServerConfig` with `auth.mode = AuthMode::Mtls`.  Submit a request
  without certs, assert 401 or 403.
- Check `crates/server/src/auth.rs` for the middleware implementation to
  understand what header/cert is checked.

#### 4.5  Task 4.12 — Config diff detection

Check if `portage_changed` or `system_changed` is surfaced in any API
response.  If it's only internal to `ClientRegistry`, this is already
covered by the 7 unit tests in `registry.rs` and can be marked as
"covered by unit tests" with an explanatory note.

#### 4.6  Task 3.13 — `apply_config` orchestration

`apply_config()` (line 23 of `portage_setup.rs`) calls all private
`write_*` functions with hardcoded `/etc/portage/` paths.  Two options:
- Create `apply_config_inner(base, config, …)` that routes all sub-calls
  through the `_inner` variants.
- Or gate behind `#[cfg(feature = "e2e")]` and test inside the GHCR
  container.

### Priority 2 — Docker lifecycle (requires Docker)

These tests go in `tests/docker_test.rs` behind
`#[cfg(feature = "integration")]`.

#### 4.7  Task 5.3 — `build_worker_image`

Needs `config.worker_binary` pointing to a valid binary (use a small
shell script `#!/bin/true`).  Needs a base image (pull
`gentoo/stage3:latest` or use GHCR test image).  After build, inspect
with `bollard` for `remerge.worker.sha256` label.  Clean up image in
Drop guard.  **This test is slow (~30s).**

Add `bollard = { workspace = true }` to `[dev-dependencies]`.

#### 4.8  Tasks 5.4–5.7 — `needs_rebuild`, `start_worker`, cleanup, eviction

All depend on 5.3.  See `TASKS.md` for detailed implementation notes.
For 5.7, search for `cleanup_idle_images` or equivalent eviction logic in
`crates/server/src/docker.rs` or `main.rs`.

### Priority 3 — Error paths requiring Docker/Gentoo

#### 4.9  Tasks 7.1–7.4 — Build failure error events

These require a Gentoo stage3 image and Docker.  Gate behind
`#[cfg(feature = "e2e")]`.  See `TASKS.md` for per-task details.

#### 4.10  Tasks 7.7, 7.8 — Workorder TTL and max retained

Check `crates/server/src/main.rs` for `run_eviction_task()` (lines 177–260).
Extract reaping logic into a testable function if needed.  Gate behind
`#[cfg(feature = "integration")]`.

#### 4.11  Task 7.12 — Oversized workorder

Check if axum has `DefaultBodyLimit` configured.  Submit a 10 MB JSON
body and verify the server rejects it gracefully.  Gate behind
`#[cfg(feature = "integration")]`.

### Priority 4 — E2E pipeline (Phase 6)

All Phase 6 tasks require Docker + Gentoo stage3.  Gate behind
`#[cfg(feature = "e2e")]`.  The current `e2e_test.rs` is a placeholder
with just `eprintln!()` and `return` — it must be replaced with real
test logic.  See `TASKS.md` tasks 6.1–6.10 for detailed implementation
notes per test.

**Minimum viable E2E:** Implement 6.1 (build single package) first.
If that works, 6.2–6.10 follow the same pattern with variations.

### Priority 5 — CI optimization (Phase 8)

Tasks 8.2–8.5 are CI workflow changes, not Rust code.

- **8.2**: Pull GHCR test image in CI, cache layers.
- **8.3**: Add a PR smoke test job for Phases 1–3 (fast, no Docker).
- **8.4**: Add a merge-to-main full integration job.
- **8.5**: Use `cargo nextest` for structured output with durations.

### Priority 6 — Phase 2 Gentoo-specific reader tests (2.1–2.6)

These require `portageq` which is only available on Gentoo.  Gate behind
`#[cfg(feature = "e2e")]` and run inside the GHCR test container.
See `TASKS.md` for per-task details.

---

## 5  Key Source Files

| File | What to learn |
|------|---------------|
| `crates/worker/src/portage_setup.rs` | `apply_config()` (L23), all `write_*` + `_inner` functions, `ensure_repo_locations` (L510), `rewrite_repo_location` (L615), `parse_repo_sections` (L836), `build_makeopts_inner` (L885) |
| `crates/server/src/state.rs` | `AppState::new()` (L69) — validation order: DockerManager → auth → binpkg_dir → state_dir |
| `crates/server/src/auth.rs` | `CertRegistry::new()`, auth middleware, 14 unit tests |
| `crates/server/src/docker.rs` | `DockerManager::new()`, `image_tag()` (L94), `image_needs_rebuild()`, `build_worker_image()`, `start_worker()`, `remove_container()`, `remove_image()` |
| `crates/server/src/config.rs` | `ServerConfig` — all fields have serde defaults |
| `crates/server/src/api.rs` | `router()` (L26) — axum routes |
| `crates/server/src/main.rs` | `run_eviction_task()` (L177–260) — TTL + max retained |
| `crates/types/src/portage.rs` | `PortageConfig`, `MakeConf`, `SystemIdentity` |
| `crates/types/src/workorder.rs` | `Workorder`, `WorkorderStatus`, `BuildEvent` |
| `crates/types/src/validation.rs` | `validate_atom`, `AtomValidationError` |
| `crates/types/src/api.rs` | Request/response types for HTTP API |
| `crates/cli/src/portage.rs` | `PortageReader`, `is_installed`, `expand_set`, `split_name_version`, `compare_versions` |
| `crates/worker/src/builder.rs` | `build_packages()`, arg filtering |

---

## 6  Architecture Decisions (established — do not change)

- **Test location:** Top-level `tests/` directory.  Each `.rs` file is a
  separate integration test crate.  Shared code in `tests/common/`.
- **Feature gating:** Phases 1–3 = no flag.  Phase 4–5 =
  `#[cfg(feature = "integration")]`.  Phase 6 = `#[cfg(feature = "e2e")]`.
- **In-process server:** `TestServer::start()` in
  `tests/common/server.rs`.  Requires Docker because `AppState::new()`
  creates `DockerManager`.
- **_inner pattern:** Testable variants of private functions that accept a
  base `Path` parameter.  Original functions are one-line wrappers.
- **No mocks:** Do not add trait abstractions or mock layers to
  production code.

---

## 7  Code Quality Requirements

- `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- `cargo fmt --all -- --check` must be clean.
- Use `#[tokio::test]` for async tests.
- Every test function has a `/// doc comment` explaining what it verifies.
- Use `.expect("descriptive msg")` in test code, never bare `.unwrap()`.
- Clean up temp dirs, Docker containers, and images in `Drop` guards.
- Follow the code style in `CONTRIBUTING.md`.

---

## 8  Dependencies

Already configured in `Cargo.toml`:

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

**If needed:**

- `tokio-tungstenite = "0.26"` for WebSocket tests (task 4.9).  Add to
  both `[workspace.dependencies]` and `[dev-dependencies]`.
- `bollard = { workspace = true }` for Docker inspection tests (task 5.3+).
  Add to `[dev-dependencies]`.

---

## 9  Constraints

- Do **not** modify production source code unless:
  - Adding a `pub` `_inner` variant for testability (following the
    established pattern — tasks 3.7, 3.13).
  - Changing `pub(crate)` → `pub` for integration test access (minimize).
- Do **not** add trait abstractions or mock layers.
- Do **not** add dependencies not listed in §8.
- Do **not** create Docker images in Phase 1–4 tests.
- After every change, verify:
  ```sh
  cargo test --workspace 2>&1 | tail -5
  cargo clippy --workspace --all-targets -- -D warnings
  ```

---

## 10  Verification Checklist

**You MUST run through this checklist before declaring work complete.**
Report each item's status explicitly.

- [ ] `cargo test --workspace` passes (Phases 1–3 + error tests)
- [ ] `cargo test --workspace --features integration` passes with Docker
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] No test uses `todo!()`, `unimplemented!()`, or empty assertion bodies
- [ ] No test leaves behind temp files, Docker containers, or images
- [ ] `e2e_test.rs` has real test logic, not just `eprintln!()` + `return`
- [ ] Every `PortageConfig` field is exercised in at least one test
- [ ] Every server API route has at least one test
- [ ] Path traversal rejection is tested for `profile_overlay` and `patches`
- [ ] Feature-gated tests skip gracefully when prerequisites are missing
- [ ] Phase 4 tests don't silently pass when Docker is unavailable
- [ ] Every unchecked `[ ]` task in `TASKS.md` is either implemented `[x]`
      or explicitly deferred with a documented reason
- [ ] All newly added `_inner` functions have corresponding tests

---

## 11  Anti-Pattern Warnings

**Do NOT do any of these:**

| Anti-pattern | Why it's wrong |
|---|---|
| Marking a task `[x]` without running the test | The test may not compile or may have logic errors |
| Writing `eprintln!("skipping…"); return;` as a test body | This falsely reports as "passed" — use `#[cfg(feature)]` instead |
| Implementing 5 tasks then batch-checking them | Errors compound; verify one at a time |
| Saying "the remaining tasks are straightforward" and stopping | The user explicitly said previous agents do this; don't repeat it |
| Adding `#[ignore]` to defer a test and calling it done | The test won't run in CI unless `--include-ignored` is used |
| Modifying tests/common/ helpers in ways that break existing tests | Run full test suite after infrastructure changes |
| Assuming a function exists without checking source | Use `grep` or `read_file` to verify signatures |
| Writing a test that always passes (no meaningful assertions) | Every test must have at least one `assert!` on computed values |
