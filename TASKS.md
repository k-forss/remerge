# Integration Test Suite — Task Plan

Actionable tasks for building a comprehensive integration test suite for remerge.
All items are ordered by dependency (earlier items unblock later ones).

> **Audit performed: every test file was read line-by-line and verified
> against task requirements. A task is `[x]` ONLY if the test exists,
> compiles, and has meaningful assertions for what the task describes.
> Tasks with weak/permissive assertions, missing verification steps,
> `eprintln` instead of `assert!`, or assertions that accept multiple
> outcomes (always-pass) are `[ ]`.**

---

## Phase 0 — Infrastructure & Scaffolding

- [x] **0.1** Create `tests/` directory at workspace root for integration tests
- [x] **0.2** Add `tests/common/mod.rs` with shared helpers
- [x] **0.3** Add `tests/common/fixtures.rs` with test data builders
- [x] **0.4** Add workspace features (`integration`, `e2e`) and dev-dependencies
- [x] **0.5** Create CI job in `.github/workflows/ci.yml` for integration tests
- [x] **0.6** Create and publish remerge integration test Docker image to GHCR

---

## Phase 1 — Types & Validation (no I/O)

- [x] **1.1** `PortageConfig` round-trip — `types_test.rs::portage_config_full_roundtrip`
- [x] **1.2** `Workorder` round-trip — `types_test.rs::workorder_status_all_variants_roundtrip`
- [x] **1.3** `validate_atom` exhaustive — `types_test.rs` 12 atom tests
- [x] **1.4** `MakeConf` field coverage — `types_test.rs::make_conf_defaults`
- [x] **1.5** `ClientRole`/`AuthMode` round-trips — `types_test.rs` 4 tests
- [x] **1.6** `WorkorderResult` — `types_test.rs::workorder_result_roundtrip`

---

## Phase 2 — CLI Portage Reader (filesystem, no network)

- [x] **2.1** `read_config` golden path — `cli_portage_test.rs::read_config_golden_path`
- [x] **2.2** Missing optional dirs — `cli_portage_test.rs::read_config_missing_optional_dirs`
- [x] **2.3** `package.use` as file vs dir — `cli_portage_test.rs::read_config_package_use_single_file`
- [x] **2.4** `read_profile_overlay` — `cli_portage_test.rs::read_profile_overlay_round_trip`
- [x] **2.5** `read_patches_recursive` — `cli_portage_test.rs::read_patches_recursive_nested`
- [x] **2.6** `read_repos_conf` — `cli_portage_test.rs::read_repos_conf_multiple_sections`
- [x] **2.7** `is_installed` with version constraints — 8 tests covering all operators
- [x] **2.8** `expand_set` — `cli_portage_test.rs::expand_set_world` + passthrough
- [x] **2.9** `split_name_version` — 5 tests
- [x] **2.10** `compare_versions` — 7 tests covering suffixes, revisions, depth

---

## Phase 3 — Worker Portage Setup (filesystem, no Docker)

- [x] **3.0** Create `_inner` variants — all pub _inner functions exist
- [x] **3.1** `write_make_conf` golden path — 15 assertions
- [x] **3.2** `use_flags_resolved = true` — `-*` prefix verified
- [x] **3.3** `use_flags_resolved = false` — no `-*` prefix
- [x] **3.4** USE_EXPAND flags as separate variables
- [x] **3.5** `write_package_config` for all types — 6 tests
- [x] **3.6** `write_env_files` — creates files, skips invalid
- [x] **3.7** `write_repos_conf` with remapping — 4 ensure_repo_locations tests
- [x] **3.8** `write_repos_conf` without remapping — 3 tests
- [x] **3.9** `set_profile` symlink — 3 tests
- [x] **3.10** `write_profile_overlay` — 4 tests incl. traversal rejection
- [x] **3.11** `write_patches` — creates structure, rejects traversal
- [x] **3.12** `build_makeopts` — 3 tests (no override, override, partial)
- [x] **3.13** `apply_config` orchestration — 12 assertions

---

## Phase 4 — Server Unit-level (in-process, with Docker)

All behind `#[cfg(feature = "integration")]`.

- [x] **4.0** Fix skip behavior — sentinel test asserts Docker available
- [x] **4.1** Valid submission returns 200 — `submit_workorder_valid`
- [x] **4.2** Invalid atoms returns 400 — `submit_workorder_invalid_atoms`
- [x] **4.3** Duplicate active returns 409 — `submit_workorder_duplicate_active`
- [x] **4.4** GET workorder returns details — `get_workorder`
- [x] **4.5** List workorders — `list_workorders`
- [x] **4.6** Cancel workorder — `cancel_workorder`
- [x] **4.7** Health endpoint — `health_endpoint`
- [x] **4.8** Info endpoint — `info_endpoint`
- [x] **4.9** WebSocket progress — `websocket_progress_stream`
- [x] **4.10** Auth enforcement — `auth_mtls_rejects_without_cert`
- [x] **4.11** Follower without main — `follower_without_main_rejected`
- [x] **4.12** Config diff — unit tests in `registry.rs`
- [x] **4.13** Metrics — `metrics_endpoint`
- [x] **4.14** Nonexistent workorder 404 — `get_nonexistent_workorder`

---

## Phase 5 — Docker Integration (requires Docker daemon)

Behind `#[cfg(feature = "integration")]`.

- [x] **5.1** `DockerManager::new` — `docker_manager_connects`
- [x] **5.2** `image_tag` — `image_tag_from_system_identity`
- [ ] **5.3** `build_worker_image` — verify image + sha256 label

      **Defect:** `docker_test.rs::build_worker_image_with_label` accepts
      BOTH success and error outcomes. On success it checks
      `image_needs_rebuild` but does NOT inspect for the sha256 label
      directly. On error it just checks error message mentions "stage3"
      and passes. **The test always passes regardless of outcome.**

      **To fix:**
      1. Build stage3 locally: `docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .`
      2. Add `bollard` to dev-deps for image inspection.
      3. On success: use bollard to inspect image, assert
         `remerge.worker.sha256` label exists and is non-empty.
      4. Remove the error-message-matching path that silently passes.
         If stage3 is missing, skip explicitly (don't pretend to pass).
      5. Clean up image in drop guard.

- [x] **5.4** `needs_rebuild` — `needs_rebuild_nonexistent_image`
- [ ] **5.5** `start_worker` — container runs with correct env/mounts

      **Defect:** `docker_test.rs::start_worker_container` accepts both
      success and error. On success only checks `!id.is_empty()`. On
      error accepts "No such image" and passes. **Does NOT verify env
      vars or mounts.** Depends on 5.3.

      **To fix:**
      1. Depends on 5.3 building a valid image.
      2. On success: use bollard to inspect container, assert
         `REMERGE_WORKORDER` env var is set, assert binpkg mount exists.
      3. Stop and remove container after inspection.

- [x] **5.6** Container cleanup — remove/stop error tests
- [ ] **5.7** Image eviction — cleanup preserves newest, removes older

      **Defect:** `docker_test.rs::image_last_used_tracking` only tests
      HashMap insert/read/compare. It does NOT test any eviction logic.
      It just proves you can use a HashMap.

      **To fix:**
      1. The eviction loop is in `main.rs` (~L219). Extract into testable
         `pub` function, or call the loop logic directly.
      2. Create real test images, set timestamps, run eviction, verify
         the older image was actually removed from Docker.
      3. If extraction is too invasive, test the `image_last_used` map
         combined with `remove_image` on a real image to prove the
         complete flow.

---

## Phase 6 — End-to-End (CLI → Server → Worker → binpkg)

Behind `#[cfg(feature = "e2e")]`. Build stage3 locally first:
`docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .`

Test failures due to production bugs are expected. Mark `[x]` with
"Known failure:" notes when the test itself is correct but production
code is broken.

- [ ] **6.1** Build single package — verify binpkg + SHA-256

      **Defect:** `e2e_test.rs::build_single_package` uses `eprintln!`
      instead of `assert!` for binpkg verification. On timeout or
      stream close, silently passes. **No assertion can ever fail.**
      Also submits with `--pretend` via helper (won't produce binpkgs).

      **To fix:**
      1. Submit WITHOUT `--pretend` (don't use `submit_test_workorder`
         helper which adds `--pretend`).
      2. Replace all `eprintln!` with assertions.
      3. On timeout: `panic!("build did not complete in 5 min")`.
      4. Assert binpkg_dir has files after Finished event.
      5. Verify SHA-256 from WorkorderResult matches actual file.

- [ ] **6.2** Build with `--pretend`/`--ask` flags

      **Defect:** Accepts BOTH 200 and 400 for `--ask`
      (`assert!(status == 200 || status == 400)` always passes).
      Does NOT verify `--pretend` was actually passed to emerge.

      **To fix:**
      1. For `--pretend`: connect WebSocket, verify output contains
         pretend-mode output without actual compilation.
      2. For `--ask`: assert ONE specific expected behavior (filtered
         → 200, or rejected → 400). Not both.

- [ ] **6.3** Build with custom USE flags — verify `package.use`

      **Defect:** Only checks 200 status and non-nil ID. Identical to
      Phase 4. **Does NOT verify worker's package.use file.**

      **To fix:**
      1. Connect WebSocket, verify emerge output mentions the USE flags.
      2. Or verify stored workorder config matches submission.
      3. At minimum, verify the submitted config round-tripped correctly.

- [ ] **6.4** Build with `@world` — verify set expansion

      **Defect:** Only checks 200 status. Identical to Phase 4. **Does
      NOT verify set expansion occurred.**

      **To fix:**
      1. Verify workorder's atoms were expanded from `@world` to
         individual packages, OR verify emerge received `@world`.
      2. Connect WebSocket, verify build events reference world packages.

- [ ] **6.5** Cross-arch build — crossdev setup

      **Blocked:** Requires QEMU user-static. Not available locally.

- [ ] **6.6** Follower inherits main config

      **Defect:** Accepts BOTH 200 and 409 for follower
      (`assert!(status == 200 || status == 409)` always passes).
      Uses same `client_id` for both main and follower (wrong —
      followers should be different clients). Does NOT verify
      config inheritance or WebSocket events.

      **To fix:**
      1. Use DIFFERENT `client_id` for follower.
      2. Assert follower is accepted (200). If 409, that's a production
         bug — let the test fail.
      3. Assert follower's workorder_id matches main's.
      4. Connect both to WebSocket, verify both receive events.

- [x] **6.7** Concurrent workorder rejection — `concurrent_workorder_rejection`

- [ ] **6.8** Worker binary upgrade detection

      **Defect:** `e2e_test.rs::worker_binary_upgrade_detection` calls
      `image_needs_rebuild` on a nonexistent image tag. Both managers
      return `true` (trivially — image doesn't exist). **Does NOT build
      image with binary A then check with binary B.** Depends on 5.3.

      **To fix:**
      1. Build image with binary A (depends on 5.3).
      2. `image_needs_rebuild` with manager_a → assert `false`.
      3. `image_needs_rebuild` with manager_b → assert `true`.
      4. This verifies SHA-256 label comparison end-to-end.

- [x] **6.9** Cancellation flow — `cancellation_flow`

- [ ] **6.10** WebSocket reconnect — streaming continues

      **Defect:** Only asserts `reconnect.is_ok()`. On error, accepts
      404/410/101 (always passes). **Does NOT verify events continue
      after reconnect.**

      **To fix:**
      1. Connect, receive at least one event.
      2. Drop connection.
      3. Reconnect.
      4. Cancel workorder to generate a StatusChanged event.
      5. Assert at least one event is received after reconnect.

---

## Phase 7 — Error Paths & Edge Cases

- [ ] **7.1** Worker exits non-zero → `Failed` status

      **Defect:** `error_test.rs::worker_exit_nonzero_sets_failed_status`
      allows `Failed | Running | Pending` in assertion — always passes.
      Polls only 10 seconds (too short).

      **To fix:**
      1. Assert `WorkorderStatus::Failed { .. }` ONLY.
      2. Increase timeout to 120s.
      3. Submit nonexistent package.
      4. Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **7.2** Missing dependency → `missing_dependencies` event

      **Not implemented.** No test exists.
      Gate behind `#[cfg(feature = "e2e")]`. Build stage3 locally.

- [ ] **7.3** USE conflict → `use_conflicts` event

      **Not implemented.** No test exists.
      Gate behind `#[cfg(feature = "e2e")]`. Build stage3 locally.

- [ ] **7.4** Fetch failure → `fetch_failures` event

      **Not implemented.** No test exists.
      Gate behind `#[cfg(feature = "e2e")]`. Build stage3 locally.

- [x] **7.5** Docker socket unavailable — `docker_socket_unavailable_returns_error`
- [ ] **7.6** Server config validation errors

      **Defect:** Auth tests ✅. AppState binpkg/state dir tests ✅
      (behind `#[cfg(feature = "integration")]`). TLS tests ❌ — just
      call `tokio::fs::read("/nonexistent/cert.pem")` directly, testing
      Tokio not remerge.

      **To fix:**
      1. TLS tests must go through server startup or the actual TLS
         config validation path in `crates/server/src/main.rs`.
      2. Check how TLS is set up. If validation is at bind time, start
         server with bad cert paths and assert the error.
      3. Keep existing auth and AppState tests — they're correct.

- [x] **7.7** Workorder TTL eviction — `workorder_ttl_eviction`

      Back-dates `updated_at`, calls `evict_workorders()`, asserts eviction
      count > 0, verifies GET returns 404. Properly implemented.

- [x] **7.8** Max retained workorders cap — `max_retained_workorders_enforced`

      Sets cap=2, submits 3, calls `evict_workorders()`, asserts ≤ 2
      remain. Properly implemented.

- [x] **7.9** Profile overlay path traversal — `profile_overlay_path_traversal_rejected`
- [x] **7.10** Patches path traversal — `patches_path_traversal_rejected`
- [x] **7.11** Shell injection — `shell_injection_in_atoms_rejected`
- [x] **7.12** Oversized workorder — `oversized_workorder_rejected`
- [x] **7.13** Deserialization errors — 4 tests
- [x] **7.14** Validation edge cases — null bytes, newlines

---

## Phase 8 — CI & Regression

- [x] **8.1** Integration test job — `ci.yml::integration-test`
- [ ] **8.2** Cache stage3 image in CI

      **Not implemented.** No buildx/layer caching. `e2e-test` job does
      `docker pull ... || true` but no caching setup.

- [x] **8.3** Smoke test target — `ci.yml::smoke-test` runs Phases 1-3 on PRs
- [x] **8.4** Full integration target — `ci.yml::e2e-test`

      Triggers on push to main only, 30-min timeout, pulls GHCR image,
      runs `--features integration,e2e`. Properly implemented.

- [ ] **8.5** Test duration tracking with nextest

      **Not implemented.** No nextest setup.
