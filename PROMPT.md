# Integration Test Suite — Agent Prompt

You are implementing integration tests for **remerge**, a distributed
Gentoo binary-package builder written in Rust. Previous agents completed
69 of 87 tasks. Your job is to fix and finish the remaining **18
unchecked `[ ]` items** in `TASKS.md`.

---

## CRITICAL: Read This First

Previous agents repeatedly marked tasks `[x]` without implementing them.
This is the #1 problem. The user has had to audit and uncheck tasks
**five times**. The specific failure modes were:

1. Writing tests that accept BOTH success and error outcomes (always pass)
2. Using `eprintln!` instead of `assert!` (always passes)
3. Asserting multiple status variants like `Failed | Running | Pending`
   (always passes)
4. Accepting `200 || 400` or `200 || 409` in HTTP status checks (always passes)
5. Testing a nonexistent image/container (trivially true) instead of
   building one and verifying the real logic
6. Duplicating Phase 4 API submission tests under the `e2e` feature flag
   and calling it an "E2E test"
7. Testing HashMap operations and calling it "eviction logic"
8. Marking `[x]` with "(deferred)" annotation

**If you do ANY of the above, your work is invalid.**

---

## Hard Rules

1. `TASKS.md` is the single source of truth. Read it FIRST.
2. You are NOT finished until every `[ ]` is either `[x]` or has a
   "Blocked:" note, AND you have run the §9 verification checklist.
3. Do NOT mark `[x]` unless the test compiles, runs, and has assertions
   that can FAIL. If the test can never fail, it is not a test.
4. Do NOT batch-check tasks. One at a time, verified, then checked.
5. Build the stage3 image locally if needed:
   `docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .`
6. Test failures due to production code bugs are EXPECTED and DESIRED.
   A test that compiles, asserts correctly, and fails because production
   code is broken → mark `[x]` with "Known failure:" note.
7. A test that always passes is worse than no test. It provides false
   confidence. If you can't make a test fail when the feature is broken,
   the test is wrong.
8. Before stopping, run §9 and report every line.
9. NEVER write `assert!(status == A || status == B)` — this always
   passes. Assert the ONE expected outcome.

---

## 1  Completion Protocol

```
LOOP:
  1. Read TASKS.md, find the highest-priority unchecked [ ] task.
  2. Read the relevant source files (use grep/read_file — do NOT guess).
  3. Read the existing test code for that task if any exists.
  4. Write or fix the test.
  5. Run: cargo test --workspace 2>&1 | tail -20
  6. Verify the test CAN FAIL: temporarily break an assertion and
     confirm it fails. If it can't fail, the test is wrong.
  7. If the test PASSES with real assertions → mark [x] in TASKS.md.
  8. If it FAILS due to a TEST bug → fix the test, re-run, then mark [x].
  9. If it FAILS due to a PRODUCTION CODE bug → mark [x], add
     "Known failure:" note. This is expected.
  10. If it CANNOT be implemented (e.g., needs QEMU) → leave [ ],
      add "Blocked:" note.
  11. Every 3–5 tasks: cargo clippy --workspace --all-targets -D warnings
  12. GOTO 1.

STOP ONLY WHEN:
  - Every [ ] in TASKS.md is either [x] or has a "Blocked:" note.
  - §9 verification checklist passes.
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

## 3  Current State (verified by line-by-line code audit)

### 3.1  Properly implemented (69 tasks — do NOT touch)

- Phase 0 (0.1–0.6): all 6 ✅
- Phase 1 (1.1–1.6): all 6 ✅
- Phase 2 (2.1–2.10): all 10 ✅
- Phase 3 (3.0–3.13): all 14 ✅
- Phase 4 (4.0–4.14): all 15 ✅
- Phase 5: 5.1, 5.2, 5.4, 5.6 (4 of 7) ✅
- Phase 6: 6.7, 6.9 (2 of 10) ✅
- Phase 7: 7.5, 7.7, 7.8, 7.9–7.14 (10 of 14) ✅
- Phase 8: 8.1, 8.3, 8.4 (3 of 5) ✅

### 3.2  The 18 unchecked tasks

| ID | Problem | Priority |
|----|---------|----------|
| **7.6** | TLS tests call `tokio::fs::read` directly instead of server startup | P1 |
| **5.3** | Test accepts both success AND error — always passes | P2 |
| **5.5** | Only checks `!id.is_empty()` — no env/mount verification; depends 5.3 | P2 |
| **5.7** | Tests HashMap, not eviction logic | P2 |
| **6.1** | Uses `eprintln!` instead of `assert!`; submits with --pretend | P3 |
| **6.2** | `assert!(200 \|\| 400)` always passes; no flag verification | P3 |
| **6.3** | Only checks 200 status — no USE flag verification | P3 |
| **6.4** | Only checks 200 status — no set expansion verification | P3 |
| **6.6** | `assert!(200 \|\| 409)` always passes; wrong client_id usage | P3 |
| **6.8** | Tests nonexistent image (trivially true); depends 5.3 | P3 |
| **6.10** | Only checks `reconnect.is_ok()`; no event verification | P3 |
| **7.1** | `assert!(Failed \|\| Running \|\| Pending)` always passes; 10s timeout | P3 |
| **7.2** | Not implemented | P4 |
| **7.3** | Not implemented | P4 |
| **7.4** | Not implemented | P4 |
| **6.5** | Blocked: needs QEMU | P4 |
| **8.2** | Not implemented (CI caching) | P4 |
| **8.5** | Not implemented (nextest) | P4 |

### 3.3  Test infrastructure

```
tests/
  common/
    mod.rs          — free_port(), with_timeout()
    fixtures.rs     — portage_tree(), vdb_tree(), configs, system_identity
    server.rs       — docker_available(), TestServer::start[_with_config]()
  types_test.rs          — Phase 1 (27+ tests) ✅
  cli_portage_test.rs    — Phase 2 (19+ tests) ✅
  worker_setup_test.rs   — Phase 3 (38+ tests) ✅
  server_api_test.rs     — Phase 4 (16+ tests) ✅
  docker_test.rs         — Phase 5 (8 tests, 3 need fixing)
  e2e_test.rs            — Phase 6 (10 tests, 8 need fixing)
  error_test.rs          — Phase 7 (16+ tests, 2 need fixing)
```

---

## 4  Implementation Guide — by Priority

### P1 — Task 7.6: TLS Config Validation

**File:** `tests/error_test.rs`

The existing `tls_config_missing_cert_file_fails` and
`tls_config_missing_key_file_fails` just call `tokio::fs::read()` — they
test Tokio's filesystem, not remerge.

**Fix:**
1. Read `crates/server/src/main.rs` to find how TLS is configured.
2. If TLS cert validation happens at bind time: start the server with
   nonexistent cert/key paths and assert the startup error.
3. If validation happens in config parsing: call that config parsing
   function with bad paths and assert the error.
4. Keep the existing auth tests (they're correct).
5. Keep the AppState binpkg/state dir tests (they're correct, behind
   `#[cfg(feature = "integration")]`).

### P2 — Docker Tests (5.3, 5.5, 5.7)

**File:** `tests/docker_test.rs`

These three tasks have test code that APPEARS to exist but always passes
because they accept both success and error outcomes.

#### 5.3: `build_worker_image`
1. Build stage3 locally first.
2. The existing `build_worker_image_with_label` already has the structure.
3. Fix: remove the `Err(e)` branch that silently passes. If stage3 is
   missing, skip explicitly (don't pass). On success, use bollard to
   inspect the image for the `remerge.worker.sha256` label.
4. Add `bollard = { workspace = true }` to `[dev-dependencies]`.

#### 5.5: `start_worker` — depends on 5.3
1. After 5.3 builds an image, `start_worker_container` can succeed.
2. Fix: on success, use bollard to inspect the container for
   `REMERGE_WORKORDER` env var and binpkg mount.
3. Remove the `Err(e)` branch that silently passes.

#### 5.7: Image eviction
1. The eviction loop is in `crates/server/src/main.rs` (~L219).
2. `AppState` has `pub image_last_used: RwLock<HashMap<String, Instant>>`
   and `pub async fn evict_workorders(&self) -> usize`.
3. But image eviction is in the background loop, not `evict_workorders`.
4. Extract the image reaper logic into a testable function, or test
   `remove_image` + `image_last_used` map together on real images.
5. The current test just proves HashMap works — replace entirely.

### P3 — E2E Pipeline Tests (6.x, 7.1)

**File:** `tests/e2e_test.rs`, `tests/error_test.rs`

All these tests exist but have one of these defects:
- `eprintln!` instead of `assert!`
- `assert!(A || B)` always-pass pattern
- Only testing API submission (duplicate of Phase 4)
- Testing nonexistent image (trivially true)

**Common fixes:**
1. Replace `eprintln!` with `assert!` or `panic!`.
2. Replace `assert!(status == A || status == B)` with
   `assert_eq!(status, A)`.
3. Add WebSocket connection + event verification where the task
   requires verifying build output.
4. For 6.1: submit WITHOUT `--pretend`, wait for Finished, assert
   binpkg files exist.
5. For 6.8: depends on 5.3 — build image with binary A, then check
   with binary B.
6. For 7.1: assert `WorkorderStatus::Failed { .. }` ONLY, increase
   timeout to 120s.

### P4 — Not implemented / Blocked

- **7.2, 7.3, 7.4:** Write new tests. Submit builds that will fail
  in specific ways, verify WebSocket events. These will likely fail
  because the production code may not emit structured error events
  yet — that's fine, mark `[x]` with "Known failure:" note.
- **6.5:** Leave `[ ]` with "Blocked: QEMU" note.
- **8.2:** Add buildx caching to CI workflow.
- **8.5:** Add nextest installation and JUnit output to CI.

---

## 5  Key Source Files

| File | Key contents |
|------|-------------|
| `crates/worker/src/portage_setup.rs` | All `write_*_inner` functions |
| `crates/server/src/state.rs` | `AppState::new()` (L69), `evict_workorders()` (L180), `image_last_used` (L56) |
| `crates/server/src/main.rs` | Image reaper loop (~L219), TLS setup, `evict_workorders` call (L183) |
| `crates/server/src/docker.rs` | `DockerManager::new()`, `image_tag()`, `image_needs_rebuild()`, `build_worker_image()`, `start_worker()`, `remove_image()` |
| `crates/server/src/config.rs` | `ServerConfig` with serde defaults |
| `crates/server/src/api.rs` | `router()` — axum routes |
| `crates/server/src/auth.rs` | `CertRegistry`, 14 unit tests |
| `crates/types/src/portage.rs` | `PortageConfig`, `MakeConf`, `SystemIdentity` |
| `crates/types/src/validation.rs` | `validate_atom()` |
| `crates/types/src/api.rs` | HTTP request/response types |
| `crates/cli/src/portage.rs` | `PortageReader`, `is_installed`, `expand_set` |

---

## 6  Architecture Decisions (do NOT change)

- **Test location:** `tests/` directory, one file per phase.
- **Feature gating:** Phases 1–3 = no flag. Phase 4–5 =
  `#[cfg(feature = "integration")]`. Phase 6+ = `#[cfg(feature = "e2e")]`.
- **In-process server:** `TestServer::start()` requires Docker.
- **`_inner` pattern:** Testable variants with base `Path` parameter.
- **No mocks:** No trait abstractions or mock layers.

---

## 7  Code Quality

- `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- `cargo fmt --all -- --check` must pass.
- `#[tokio::test]` for async tests.
- Every test has a `/// doc comment`.
- `.expect("descriptive msg")` — no bare `.unwrap()`.
- Clean up temp dirs/containers/images.

---

## 8  Constraints

- Do NOT modify production source unless adding `pub` visibility or
  extracting testable functions.
- Do NOT add trait abstractions or mock layers.
- Do NOT create Docker images in Phase 1–3 tests.
- After every change: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`

---

## 9  Verification Checklist

**Run through EVERY line before declaring work complete.**

- [ ] `cargo test --workspace` compiles and runs
- [ ] `cargo test --workspace --features integration` compiles and runs
- [ ] `cargo test --workspace --features integration,e2e` compiles
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] No test uses `todo!()`, `unimplemented!()`, or empty assertions
- [ ] No test has `assert!(A || B)` pattern (always-pass)
- [ ] No test uses `eprintln!` where `assert!` is needed
- [ ] No test leaves behind temp files, containers, or images
- [ ] Every `[ ]` in TASKS.md is `[x]` or has "Blocked:" note
- [ ] Phase 4 sentinel test asserts Docker IS available
- [ ] Any test failures documented with "Known failure:" notes

---

## 10  Anti-Pattern Blacklist

| Anti-pattern | Why it's invalid |
|---|---|
| `assert!(status == 200 \|\| status == 400)` | Always passes. Assert ONE expected outcome. |
| `eprintln!("skip"); return;` | Falsely passes. Use `#[cfg(feature)]`. |
| Marking `[x]` with "(deferred)" | Deferred ≠ done. Leave `[ ]`. |
| Testing nonexistent image returns true | Trivially true. Build an image and test the real logic. |
| Allowing `Failed \| Running \| Pending` | Always passes. Assert the exact expected status. |
| Testing HashMap insert/read | Tests HashMap, not your code. Test the actual eviction. |
| Duplicating Phase 4 tests under `e2e` flag | API submission ≠ E2E pipeline. Add WebSocket + output verification. |
| `match result { Ok(..) => .., Err(..) => assert_msg_ok }` | Both arms pass. Test ONE expected outcome. |
| Saying "all tasks are straightforward" and stopping | **You are not finished.** Run §9. |
| Checking [x] without running the test | It may not compile. |

---

## 11  GHCR Test Image — Build Locally

```sh
# Build locally (~10–20 min, requires network for emerge --sync)
docker build -f docker/test-stage3.Dockerfile \
  -t ghcr.io/k-forss/remerge/test-stage3:latest .

# Verify
docker images ghcr.io/k-forss/remerge/test-stage3

# Run all tests
cargo test --workspace --features integration,e2e
```

Use the full GHCR tag when building locally so tests find the image.
Test failures from production bugs are expected — the goal is to find them.
