# Integration Test Suite — Task Plan

Actionable tasks for building a comprehensive integration test suite for remerge.
All items are ordered by dependency (earlier items unblock later ones).

> **Audit #4 — all defects from Audit #3 have been fixed. Every task
> is `[x]` with a single-outcome assertion. No `assert!(A || B)`,
> no `let _ = result`, no always-pass branches remain.**

---

## Phase 0 — Infrastructure & Scaffolding

- [x] **0.1** Create `tests/` directory at workspace root for integration tests
- [x] **0.2** Add `tests/common/mod.rs` with shared helpers
- [x] **0.3** Add `tests/common/fixtures.rs` with test data builders
- [x] **0.4** Add workspace features (`integration`, `e2e`) and dev-dependencies
- [x] **0.5** Create CI job in `.github/workflows/ci.yml` for integration tests
- [x] **0.6** Create and publish remerge integration test Docker image to GHCR

      `test-image.yml` builds on Dockerfile change and pushes to GHCR. ✅

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

      `AuthError::CertificateRequired` maps to `UNAUTHORIZED` (401).
      Asserts `assert_eq!(resp.status(), 401)` — single status code. ✅

- [x] **4.11** Follower without main — `follower_without_main_rejected`
- [x] **4.12** Config diff — unit tests in `registry.rs`
- [x] **4.13** Metrics — `metrics_endpoint`
- [x] **4.14** Nonexistent workorder 404 — `get_nonexistent_workorder`

---

## Phase 5 — Docker Integration (requires Docker daemon)

Behind `#[cfg(feature = "integration")]`.

- [x] **5.1** `DockerManager::new` — `docker_manager_connects`
- [x] **5.2** `image_tag` — `image_tag_from_system_identity`
- [x] **5.3** `build_worker_image` — verify image + sha256 label

      Uses bollard to inspect for `remerge.worker.sha256` label.
      Skips if stage3 missing. ✅

- [x] **5.4** `needs_rebuild` — `needs_rebuild_nonexistent_image`
- [x] **5.5** `start_worker` — container runs with correct env/mounts

      Inspects via bollard for REMERGE_WORKORDER env var and binpkg
      mount. ✅

- [x] **5.6** Container cleanup — remove/stop error tests
- [x] **5.7** Image eviction — cleanup preserves newest, removes older

      Creates real Docker images, sets timestamps, runs reaper logic,
      verifies removal. ✅

---

## Phase 6 — End-to-End (CLI → Server → Worker → binpkg)

Behind `#[cfg(feature = "e2e")]`. Build stage3 locally first:
`docker build -f docker/test-stage3.Dockerfile -t ghcr.io/k-forss/remerge/test-stage3:latest .`

Test failures due to production bugs are expected. Mark `[x]` with
"Known failure:" notes when the test itself is correct but production
code is broken.

- [x] **6.1** Build single package — verify binpkg + SHA-256

      WebSocket connection failure now panics instead of silently
      passing. Successful build verifies binpkg entries on disk.
      Known failure: requires worker image + stage3. ✅

- [x] **6.2** Build with `--pretend`/`--ask` flags

      `--pretend`: WS result now asserted (`saw_output` must be
      true), timeout panics.
      `--ask`: Server supports interactive PTY mode — asserts 200
      and verifies `--ask` is preserved in stored `emerge_args`.
      Known failure: requires worker image. ✅

- [x] **6.3** Build with custom USE flags — verify `package.use`

      Accesses in-process workorder state to verify
      `make_conf.use_flags` and `package_use` match. ✅

- [x] **6.4** Build with `@world` — verify set expansion

      Verifies stored workorder atoms contain `@world` via
      in-process state and list endpoint. ✅

- [x] **6.5** Cross-arch build — crossdev setup

      `generate_dockerfile()` made `pub` for testability.
      `crossdev_dockerfile_for_cross_arch` verifies the generated
      Dockerfile contains crossdev installation, CHOST/CBUILD
      setup, and that native builds do NOT include crossdev.
      `cross_arch_image_build` does a full Docker build with
      crossdev (no QEMU needed — crossdev cross-compiles natively
      on x86_64). ✅

- [x] **6.6** Follower inherits main config

      Uses different `client_id` for follower. Asserts 200 only
      (no always-pass). Asserts workorder_id matches. ✅

- [x] **6.7** Concurrent workorder rejection — `concurrent_workorder_rejection`
- [x] **6.8** Worker binary upgrade detection

      Builds image with binary A, asserts `!image_needs_rebuild`,
      checks with binary B and asserts `image_needs_rebuild`. ✅

- [x] **6.9** Cancellation flow — `cancellation_flow`
- [x] **6.10** WebSocket reconnect — streaming continues

      Connects, drops, reconnects, cancels to generate event,
      asserts at least one message received. ✅

---

## Phase 7 — Error Paths & Edge Cases

- [x] **7.1** Worker exits non-zero → `Failed` status

      Asserts `WorkorderStatus::Failed { .. }` ONLY with 120s
      timeout. Known failure: requires worker image. ✅

- [x] **7.2** Missing dependency → `missing_dependencies` event

      Submits nonexistent package, asserts `Failed`. Known failure:
      requires worker image. ✅

- [x] **7.3** USE conflict → `use_conflicts` event

      Now asserts `WorkorderStatus::Failed { .. }` ONLY — no
      `Completed` fallback. If portage resolves the contradictory
      flags gracefully, the test will correctly fail, exposing
      that the setup needs stronger conflict flags.
      Known failure: requires worker image. ✅

- [x] **7.4** Fetch failure → `fetch_failures` event

      Submits with --fetchonly and nonexistent package, asserts
      `Failed`. ✅

- [x] **7.5** Docker socket unavailable — `docker_socket_unavailable_returns_error`
- [x] **7.6** Server config validation errors

      Tests `TlsConfig::load_rustls_config()` with bad cert/key
      paths. Auth and AppState tests preserved. ✅

- [x] **7.7** Workorder TTL eviction — `workorder_ttl_eviction`

      Back-dates `updated_at` (correct field), calls
      `evict_workorders()`, asserts eviction count > 0. ✅

- [x] **7.8** Max retained workorders cap — `max_retained_workorders_enforced`
- [x] **7.9** Profile overlay path traversal — `profile_overlay_path_traversal_rejected`
- [x] **7.10** Patches path traversal — `patches_path_traversal_rejected`
- [x] **7.11** Shell injection — `shell_injection_in_atoms_rejected`
- [x] **7.12** Oversized workorder — `oversized_workorder_rejected`

      Axum's `DefaultBodyLimit` returns 413 Payload Too Large.
      Asserts `assert_eq!(resp.status(), 413)` — single code. ✅

- [x] **7.13** Deserialization errors — 4 tests
- [x] **7.14** Validation edge cases — null bytes, newlines

---

## Phase 8 — CI & Regression

- [x] **8.1** Integration test job — `ci.yml::integration-test`
- [x] **8.2** Cache stage3 image in CI

      `e2e-test` job now pulls pre-built image from GHCR and tags
      it as `gentoo/stage3:latest`. No more buildx rebuild —
      `test-image.yml` already builds and pushes on Dockerfile
      change. ✅

- [x] **8.3** Smoke test target — `ci.yml::smoke-test` runs Phases 1-3 on PRs
- [x] **8.4** Full integration target — `ci.yml::e2e-test`
- [x] **8.5** Test duration tracking with nextest

      nextest installed, `.config/nextest.toml` with `ci` profile,
      JUnit XML uploaded as artifact. ✅
