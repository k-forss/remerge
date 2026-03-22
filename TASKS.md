# Integration Test Suite — Task Plan

Actionable tasks for building a comprehensive integration test suite for remerge.
All items are ordered by dependency (earlier items unblock later ones).

---

## Phase 0 — Infrastructure & Scaffolding

- [x] **0.1** Create `tests/` directory at workspace root for integration tests
- [x] **0.2** Add `tests/common/mod.rs` with shared helpers (free port allocation,
      temp dir scaffolding, config builders, timeout wrappers)
- [x] **0.3** Add `tests/common/fixtures.rs` with test data builders:
  - [x] Minimal `make.conf` (CFLAGS, CHOST, USE, FEATURES)
  - [x] Sample `package.use`, `package.accept_keywords`, `package.mask`,
        `package.unmask`, `package.env`, env files
  - [x] Minimal `repos.conf` (single `[gentoo]` section with a `location`)
  - [x] Sample `profile/` overlay directory (use.mask)
  - [x] Sample `patches/` tree (category/package/patch files)
- [x] **0.4** Add workspace features (`integration`, `e2e`) and dev-dependencies
      to `Cargo.toml`; created lib.rs for server/worker/CLI crates to expose
      modules for integration testing
- [ ] **0.5** Create a CI job in `.github/workflows/ci.yml` that runs integration
      tests on `ubuntu-latest` with Docker available (GitHub-hosted runners
      have Docker pre-installed)

---

## Phase 1 — Types & Validation (no I/O)

These are pure-logic tests that need no Docker, no server, no filesystem.

- [x] **1.1** `PortageConfig` round-trip: construct → serialize → deserialize →
      assert equality (covers serde defaults like `profile_overlay`,
      `use_flags_resolved`)
- [x] **1.2** `Workorder` round-trip: all status transitions
      (`Pending → Provisioning → Building → Completed/Failed/Cancelled`)
- [x] **1.3** `validate_atom` exhaustive: all legal operator/category/name/version
      combinations vs. all rejection classes (shell injection, empty parts,
      unqualified + versioned)
- [x] **1.4** `MakeConf` field coverage: every `extra_vars` key, empty USE,
      empty FEATURES, `use_flags_resolved = true` vs `false` behaviour
- [x] **1.5** `ClientRole` / `AuthMode` `Display` + `FromStr` round-trips
- [x] **1.6** `WorkorderResult` with mixed built/failed packages, SHA-256 hashes

---

## Phase 2 — CLI Portage Reader (filesystem, no network)

Tests that create a temp directory tree mimicking `/etc/portage/` and `/var/db/pkg/`.

- [ ] **2.1** `read_config` golden path: populate a full temp portage tree in a
      temp dir, set `ROOT` to that dir, call `PortageReader::new()?.read_config()`,
      assert every field (requires portageq — skipped in non-Gentoo CI)
- [ ] **2.2** `read_config` with missing optional dirs (no `package.use/`,
      no `patches/`, no `profile/` overlay) — should succeed with empty maps
      (requires portageq — skipped in non-Gentoo CI)
- [ ] **2.3** `read_config` with `package.use` as a single file vs. a directory
      of files (Portage supports both) (requires portageq — skipped in
      non-Gentoo CI)
- [ ] **2.4** `read_profile_overlay` round-trip: write files into
      `<ROOT>/etc/portage/profile/` in the temp dir, call
      `PortageReader::new()?.read_profile_overlay()`, assert `BTreeMap` keys and
      content (requires portageq — skipped in non-Gentoo CI)
- [ ] **2.5** `read_patches_recursive` with nested `category/package/*.patch`
      (requires portageq — skipped in non-Gentoo CI)
- [ ] **2.6** `read_repos_conf` with multiple `[section]` blocks, verify
      repo names and locations (requires portageq — skipped in non-Gentoo CI)
- [x] **2.7** `is_installed` with version constraints:
  - [x] `category/pkg` — any version matches
  - [x] `=category/pkg-1.2.3` — exact match
  - [x] `=category/pkg-1.2.3-r1` — exact with revision
  - [x] `>=category/pkg-2.0` — satisfied and unsatisfied
  - [x] `<=category/pkg-2.0` — satisfied and unsatisfied
  - [x] `>category/pkg-2.0` — boundary (2.0 should NOT match)
  - [x] `<category/pkg-2.0` — boundary
  - [x] `~category/pkg-1.2.3` — any revision
  - [x] `=category/pkg-1.2*` — glob
  - [x] `@world` — always returns false
  - [x] Uninstalled package — returns false
- [ ] **2.8** `expand_set` for `@world` (reads world file) and `@system`
      (calls `portageq`) — requires portageq
- [x] **2.9** `split_name_version` edge cases: numeric-only names
      (`dev-libs/1lib`), multi-hyphen (`x11-libs/gtk+-2.0`), no version
- [x] **2.10** `compare_versions` edge cases: suffixes (`_alpha`, `_beta`,
      `_pre`, `_rc`, `_p`), long numeric segments, revision-only differences

---

## Phase 3 — Worker Portage Setup (filesystem, no Docker)

Tests that call `portage_setup` functions against a temp directory.

- [ ] **3.1** `write_make_conf` golden path: provide a `MakeConf`, assert
      generated file content line-by-line (writes to hardcoded `/etc/portage/` —
      requires root or container environment)
- [ ] **3.2** `write_make_conf` with `use_flags_resolved = true` — USE line
      must start with `-* ` (requires root)
- [ ] **3.3** `write_make_conf` with `use_flags_resolved = false` — no `-*`
      prefix (requires root)
- [ ] **3.4** `write_make_conf` with USE_EXPAND flags — must appear as
      separate variables, not inside USE (requires root)
- [ ] **3.5** `write_package_config` for each config type (use, keywords,
      license, mask, unmask, env) — both single-entry and multi-entry
      (requires root)
- [ ] **3.6** `write_env_files` — write, verify content and permissions
      (requires root)
- [ ] **3.7** `write_repos_conf` with server `repos_dir` bind-mount remapping
      (locations must be rewritten to `/var/db/repos/<name>`) (requires root)
- [ ] **3.8** `write_repos_conf` without server repos_dir — locations
      preserved as-is (requires root)
- [x] **3.9** `set_profile` — creates symlink pointing to the correct repo's
      `profiles/<profile>` path; tested via `set_profile_inner` with temp dirs
- [x] **3.10** `write_profile_overlay` — writes files to temp dir,
      rejects path traversal (`..`), rejects absolute paths; tested via
      `write_profile_overlay_inner`
- [x] **3.11** `write_patches` — writes files to temp dir, creates intermediate
      category/package dirs, rejects path traversal; tested via
      `write_patches_inner`
- [x] **3.12** `build_makeopts` — server env REMERGE_PARALLEL_JOBS and
      REMERGE_LOAD_AVERAGE override client MAKEOPTS; absent env falls back
      to client MAKEOPTS; tested via `build_makeopts_inner`
- [ ] **3.13** `apply_config` orchestration — call with a full `PortageConfig`
      and assert that all files are present under the temp root
      (requires root — writes to hardcoded paths)

---

## Phase 4 — Server Unit-level (in-process, with Docker)

Tests that spin up the axum app in-process. Require Docker for AppState
initialization but do not run builds — they test HTTP API responses only.
Tests skip gracefully when Docker is unavailable.

- [x] **4.1** `POST /api/v1/workorders` — valid submission returns 200 +
      workorder ID
- [x] **4.2** `POST /api/v1/workorders` — invalid atoms rejected (400)
- [x] **4.3** `POST /api/v1/workorders` — duplicate active workorder rejected
      (409)
- [x] **4.4** `GET /api/v1/workorders/:id` — returns workorder with correct
      status
- [x] **4.5** `GET /api/v1/workorders` — returns list with at least one entry
- [x] **4.6** `DELETE /api/v1/workorders/:id` — transitions to Cancelled
- [x] **4.7** `GET /api/v1/health` — returns 200 with status "ok"
- [x] **4.8** `GET /api/v1/info` — returns server version, auth mode,
      binhost URL
- [ ] **4.9** WebSocket `/api/v1/workorders/:id/progress` — connects,
      receives text events, binary PTY frames
- [ ] **4.10** Auth enforcement: `None` mode allows all, `Mtls` mode
      rejects missing cert, `Mixed` mode enforces main vs follower rules
      (covered by unit tests in server/auth.rs)
- [x] **4.11** Client registry: follower registration requires existing main
- [ ] **4.12** Config diff detection: same config → empty diff, changed
      package.use → `portage_changed = true` (covered by unit tests in
      server/registry.rs)
- [x] **4.13** Metrics endpoint (`/metrics`) returns Prometheus text format
- [x] **4.14** GET /api/v1/workorders/{nonexistent} returns 404

---

## Phase 5 — Docker Integration (requires Docker daemon)

These tests need a running Docker daemon. Gate behind
`#[cfg(feature = "integration")]`.

- [x] **5.1** `DockerManager::new` — connects to local Docker socket
- [x] **5.2** `image_tag` derivation from `SystemId` — verify format
- [ ] **5.3** `build_worker_image` — builds an image, verify it exists via
      Docker API, verify `remerge.worker.sha256` label
- [ ] **5.4** `needs_rebuild` — returns `false` for freshly-built image,
      `true` after worker binary changes
- [ ] **5.5** `start_worker` — container starts, env vars are set, mounts
      are present
- [ ] **5.6** Container cleanup — `remove_container` removes the container
- [ ] **5.7** Image eviction — `cleanup_idle_images` preserves the newest
      image per CHOST+profile group, removes older ones

---

## Phase 6 — End-to-End (CLI → Server → Worker → binpkg)

Full pipeline tests. Require Docker, a Gentoo stage3 image, and network
access (for `emerge --sync`). These are slow and should be gated behind
`#[cfg(feature = "e2e")]` or run only in a dedicated CI job.

- [ ] **6.1** Build a single small package (`app-misc/hello` or
      `app-editors/nano`) — verify binpkg appears in output directory with
      correct SHA-256
- [ ] **6.2** Build with `--pretend` / `--ask` flags — verify they are
      correctly filtered or passed
- [ ] **6.3** Build with custom USE flags — verify worker's `package.use`
      matches client's
- [ ] **6.4** Build with `@world` set — verify set expansion and filtering
      of installed packages
- [ ] **6.5** Cross-architecture build (if CI has multi-arch Docker) — verify
      crossdev setup and `emerge-<CHOST>` invocation
- [ ] **6.6** Follower client — verify follower inherits main's config and
      shares the workorder
- [ ] **6.7** Concurrent workorder rejection — submit while another is active,
      verify 409
- [ ] **6.8** Worker binary upgrade detection — change the binary, submit
      again, verify image rebuild
- [ ] **6.9** Cancellation — submit, cancel via API, verify container is
      stopped and removed
- [ ] **6.10** Resume / reconnect — disconnect WebSocket, reconnect, verify
      progress streaming continues

---

## Phase 7 — Error Paths & Edge Cases

- [ ] **7.1** Worker container exits non-zero — verify `Failed` status and
      error propagation
- [ ] **7.2** Missing dependency — verify structured event
      `missing_dependencies` is emitted
- [ ] **7.3** USE conflict — verify structured event `use_conflicts` is
      emitted
- [ ] **7.4** Fetch failure — verify structured event `fetch_failures` is
      emitted
- [ ] **7.5** Docker socket unavailable — verify graceful error
- [ ] **7.6** Server config validation — missing `binpkg_dir`, invalid
      `auth` section, missing TLS cert
- [ ] **7.7** Workorder TTL expiry — verify `reap_old_workorders` removes
      stale entries
- [ ] **7.8** Max retained workorders — verify cap is enforced
- [x] **7.9** Path traversal in `profile_overlay` keys — verify rejection
- [x] **7.10** Path traversal in `patches` keys — verify rejection
- [x] **7.11** Shell injection in atom names — verify rejection
- [ ] **7.12** Oversized workorder — verify graceful handling
- [x] **7.13** Deserialization error paths — empty JSON, invalid JSON,
      wrong type, missing required fields
- [x] **7.14** Validation edge cases — null bytes, newlines, whitespace-only

---

## Phase 8 — CI & Regression

- [ ] **8.1** Add integration test job to CI with Docker
      (`services: docker:dind` or native runner Docker)
- [ ] **8.2** Cache Gentoo stage3 image in CI to speed up E2E tests
- [ ] **8.3** Add a "smoke test" target that runs the fastest subset
      (Phases 1–3) on every PR
- [ ] **8.4** Add a "full integration" target that runs Phases 4–7 on merge
      to `main`
- [ ] **8.5** Record and track test durations to catch regressions
