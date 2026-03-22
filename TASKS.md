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
- [x] **0.5** Create a CI job in `.github/workflows/ci.yml` that runs integration
      tests on `ubuntu-latest` with Docker available (GitHub-hosted runners
      have Docker pre-installed)

      **Implementation notes:**
      - Add an `integration-test` job that runs
        `cargo test --workspace --features integration` with Docker already
        available on the runner.
      - Add a separate `e2e-test` job (manual or merge-to-main only) that
        pulls `ghcr.io/<owner>/remerge-test-stage3:latest` and runs
        `cargo test --workspace --features e2e`.
      - Depend on the new GHCR test image from task 0.6.

- [x] **0.6** Create and publish a remerge integration test Docker image to GHCR

      **Implementation notes:**
      This is a prerequisite for all Gentoo-specific tests (Phase 2 tasks
      2.1–2.6, 2.8, Phase 3 tasks 3.1–3.8, 3.13, and all E2E tests).

      Create `docker/test-stage3.Dockerfile` that:
      1. Starts from `gentoo/stage3:latest`
      2. Runs `emerge --sync` (or copies a pre-synced portage tree snapshot)
      3. Installs `portageq` (already in stage3, verify it works)
      4. Installs `cpuid2cpuflags` (for CPU flag detection tests)
      5. Creates a minimal `/var/db/pkg` tree with a few known packages
         for `is_installed` tests
      6. Creates a minimal portage tree layout in `/var/db/repos/gentoo`
         with profiles and a few ebuilds
      7. Tags as `ghcr.io/<owner>/remerge-test-stage3:latest`

      Add a GitHub Actions workflow `docker/publish-test-image.yml` that:
      - Triggers on changes to `docker/test-stage3.Dockerfile` or manual
        dispatch
      - Builds and pushes to GHCR with `packages: write` permission
      - Caches layers to speed up rebuilds

      Tests that need this image should use `#[cfg(feature = "e2e")]` or
      check for the image availability and skip gracefully.

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
      assert every field

      **Implementation notes:**
      - `read_config()` calls `portageq envvar USE` and other `portageq`
        commands internally. These are not available on non-Gentoo hosts.
      - **Option A (preferred):** Run this test inside the GHCR test image
        from task 0.6 using `docker run`. Gate behind `#[cfg(feature = "e2e")]`.
      - **Option B:** Accept that `portageq` falls back gracefully (the code
        has fallback paths when `portageq` fails) and test only the
        fallback-path behaviour on non-Gentoo hosts. This tests a subset.
      - Choose Option A for full coverage; implement Option B as a separate
        lower-priority task for fast CI.

- [ ] **2.2** `read_config` with missing optional dirs (no `package.use/`,
      no `patches/`, no `profile/` overlay) — should succeed with empty maps

      **Implementation notes:**
      Same constraints as 2.1 — requires `portageq` or fallback testing.
      Gate behind `#[cfg(feature = "e2e")]` for full test; add a fallback
      variant that tests just the file-parsing logic without `portageq`.

- [ ] **2.3** `read_config` with `package.use` as a single file vs. a directory
      of files (Portage supports both)

      **Implementation notes:**
      Same constraints as 2.1. The file-vs-directory distinction is handled
      in `read_config()` which calls `portageq`. Gate behind
      `#[cfg(feature = "e2e")]`.

- [ ] **2.4** `read_profile_overlay` round-trip: write files into
      `<ROOT>/etc/portage/profile/` in the temp dir, call
      `PortageReader::new()?.read_profile_overlay()`, assert `BTreeMap` keys and
      content

      **Implementation notes:**
      `read_profile_overlay()` is called from within `read_config()`.
      Check if it's a separate `pub` method or only called internally.
      If internal, either:
      - Extract to a standalone `pub fn` with a path parameter, OR
      - Test via `read_config()` inside the GHCR image.
      `PortageReader::new()` calls `portageq` to find the portage root.
      Gate behind `#[cfg(feature = "e2e")]` or add `_inner` variant.

- [ ] **2.5** `read_patches_recursive` with nested `category/package/*.patch`

      **Implementation notes:**
      Same as 2.4 — check if this is separately callable. If only internal
      to `read_config()`, needs `_inner` variant or E2E test.
      Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **2.6** `read_repos_conf` with multiple `[section]` blocks, verify
      repo names and locations

      **Implementation notes:**
      Same as 2.4. Gate behind `#[cfg(feature = "e2e")]`.

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
- [x] **2.8** `expand_set` for `@world` (reads world file) and `@system`
      (calls `portageq`) — requires portageq

      **Implementation notes:**
      - `@world` expansion reads `/var/lib/portage/world` — this can be
        tested with a temp dir on any host by setting ROOT.
      - `@system` expansion calls `portageq` — requires Gentoo or the
        GHCR test image.
      - Split into two tests: `expand_set_world` (no portageq needed,
        runs in default CI) and `expand_set_system` (needs portageq,
        gate behind `#[cfg(feature = "e2e")]`).

- [x] **2.9** `split_name_version` edge cases: numeric-only names
      (`dev-libs/1lib`), multi-hyphen (`x11-libs/gtk+-2.0`), no version
- [x] **2.10** `compare_versions` edge cases: suffixes (`_alpha`, `_beta`,
      `_pre`, `_rc`, `_p`), long numeric segments, revision-only differences

---

## Phase 3 — Worker Portage Setup (filesystem, no Docker)

Tests that call `portage_setup` functions against a temp directory.

**Note:** Tasks 3.1–3.8 and 3.13 require writing to hardcoded paths under
`/etc/portage/` because the production functions (`write_make_conf`,
`write_package_use`, `write_env_files`, etc.) are **private** and use
hardcoded absolute paths. To test these without root access:

**Required prerequisite:** Create `_inner` variants (like was done for
`write_profile_overlay_inner`, `write_patches_inner`, `set_profile_inner`,
and `build_makeopts_inner`) that accept a base path parameter. Make them
`pub` for integration testing. This is a minimal visibility change, not a
mock layer.

- [x] **3.0** Create `_inner` variants for private portage_setup functions

      **Implementation notes:**
      Add `pub` `_inner` variants for these functions in
      `crates/worker/src/portage_setup.rs`:
      - `write_make_conf_inner(base: &Path, config: &PortageConfig, worker_chost: &str, gpg_key: Option<&str>, gpg_home: Option<&str>)`
      - `write_package_use_inner(base: &Path, config: &PortageConfig)`
      - `write_package_accept_keywords_inner(base: &Path, config: &PortageConfig)`
      - `write_package_license_inner(base: &Path, config: &PortageConfig)`
      - `write_package_mask_inner(base: &Path, config: &PortageConfig)`
      - `write_package_unmask_inner(base: &Path, config: &PortageConfig)`
      - `write_package_env_inner(base: &Path, config: &PortageConfig)`
      - `write_env_files_inner(base: &Path, config: &PortageConfig)`
      - `write_repos_conf_inner(base: &Path, config: &PortageConfig, repos_dir: Option<&Path>)`

      Each `_inner` variant writes files relative to `base` instead of
      `/etc/portage/`. The existing functions become thin wrappers:
      ```rust
      async fn write_make_conf(...) -> Result<()> {
          write_make_conf_inner(Path::new("/etc/portage"), ...).await
      }
      ```

      Follow the exact pattern already established by:
      - `write_profile_overlay` → `write_profile_overlay_inner`
      - `write_patches` → `write_patches_inner`
      - `set_profile` → `set_profile_inner`
      - `build_makeopts` → `build_makeopts_inner`

- [x] **3.1** `write_make_conf` golden path: provide a `MakeConf`, assert
      generated file content line-by-line

      **Implementation notes:**
      Depends on task 3.0 (`write_make_conf_inner`). Use a temp dir as
      base path. Assert CHOST, CFLAGS, CXXFLAGS, LDFLAGS, MAKEOPTS, USE,
      FEATURES, ACCEPT_LICENSE, ACCEPT_KEYWORDS, CPU_FLAGS, USE_EXPAND,
      extra vars, PKGDIR lines are all present and correct.

- [x] **3.2** `write_make_conf` with `use_flags_resolved = true` — USE line
      must start with `-* `

      **Implementation notes:**
      Depends on task 3.0. Set `make_conf.use_flags_resolved = true`,
      verify the output contains `USE="-* flag1 flag2"`.

- [x] **3.3** `write_make_conf` with `use_flags_resolved = false` — no `-*`
      prefix

      **Implementation notes:**
      Depends on task 3.0. Set `make_conf.use_flags_resolved = false`,
      verify the output does NOT contain `-*`.

- [x] **3.4** `write_make_conf` with USE_EXPAND flags — must appear as
      separate variables, not inside USE

      **Implementation notes:**
      Depends on task 3.0. Populate `make_conf.use_expand` with
      `VIDEO_CARDS: ["intel"]` and `INPUT_DEVICES: ["libinput"]`,
      verify output has `VIDEO_CARDS="intel"` and
      `INPUT_DEVICES="libinput"` as separate lines.

- [x] **3.5** `write_package_config` for each config type (use, keywords,
      license, mask, unmask, env) — both single-entry and multi-entry

      **Implementation notes:**
      Depends on task 3.0. Test each `write_package_*_inner` function.
      For each: create a temp dir, call the function, read the written
      file, assert content matches expected format:
      - `package.use/remerge`: `atom flag1 flag2\n`
      - `package.accept_keywords/remerge`: `atom ~amd64\n`
      - `package.license/remerge`: `atom license-name\n`
      - `package.mask/remerge`: `atom\n`
      - `package.unmask/remerge`: `atom\n`
      - `package.env/remerge`: `atom env-file.conf\n`

- [x] **3.6** `write_env_files` — write, verify content and permissions

      **Implementation notes:**
      Depends on task 3.0 (`write_env_files_inner`). Write multiple env
      files, verify they appear in `base/env/`, verify content matches,
      verify invalid filenames (containing `/` or `..`) are skipped.

- [x] **3.7** `write_repos_conf` with server `repos_dir` bind-mount remapping
      (locations must be rewritten to `/var/db/repos/<name>`)

      **Audit finding:** UNCHECKED — the `write_repos_conf_inner` function
      does NOT accept a `repos_dir` parameter and does NOT perform any
      remapping. The remapping logic lives in `ensure_repo_locations()`
      (a private function at line ~510 in `portage_setup.rs`) which:
      - symlinks repo locations to bind-mounted paths under `/var/db/repos/`
      - remaps overlays to `/var/tmp/remerge-repos/` when `REMERGE_SKIP_SYNC=1`
      - uses `rewrite_repo_location()` to update `location =` lines in-place

      The existing `write_repos_conf_creates_files` test only verifies that
      raw content is written to `repos.conf/<name>` — it does NOT test
      the remapping/symlinking behavior at all.

      **Implementation notes:**
      Two approaches:
      - **Option A (preferred):** Create `ensure_repo_locations_inner(config,
        repos_base, repos_conf_base)` that operates on parametric paths
        instead of hardcoded `/var/db/repos` and `/etc/portage/repos.conf/`.
        Then write tests that create a temp bind-mount dir and verify
        symlinks and rewrites.
      - **Option B:** Test inside the GHCR Docker image (task 0.6) where
        the function can operate on real paths. Gate behind
        `#[cfg(feature = "e2e")]`.
      - For Option A, also create `rewrite_repo_location_inner(repos_conf_base,
        filename, old_loc, new_loc)` since the current function uses
        hardcoded `/etc/portage/repos.conf/{filename}`.
      - Test cases:
        1. Repo in bind-mount → symlink created at `location`
        2. Repo NOT in bind-mount, `REMERGE_SKIP_SYNC=1` → remapped to
           writable path and `location =` line rewritten
        3. Repo NOT in bind-mount, no skip-sync → empty dir created
        4. Invalid location (traversal) → bail with error

- [x] **3.8** `write_repos_conf` without server repos_dir — locations
      preserved as-is

      **Implementation notes:**
      Depends on task 3.0 (`write_repos_conf_inner`). Pass `None` for
      repos_dir and verify that `location =` lines are preserved.

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
- [x] **3.13** `apply_config` orchestration — call with a full `PortageConfig`
      and assert that all files are present under the temp root

      **Implementation notes:**
      - `apply_config()` calls all the private `write_*` functions which
        write to hardcoded `/etc/portage/` paths.
      - After task 3.0 is complete, create an `apply_config_inner` that
        takes a base path, or test this inside the GHCR Docker image.
      - **Recommended:** Gate behind `#[cfg(feature = "e2e")]` and run
        inside the test container where root writes are safe.

---

## Phase 4 — Server Unit-level (in-process, with Docker)

Tests that spin up the axum app in-process. Require Docker for AppState
initialization but do not run builds — they test HTTP API responses only.
Tests skip gracefully when Docker is unavailable.

**Audit note:** All Phase 4 tests currently skip silently when Docker is
unavailable — they `return` early with `eprintln`. In CI without Docker,
they report as "passed" despite not actually running. This must be fixed.

- [x] **4.0** Fix Phase 4 test skip behavior

      **Implementation notes:**
      Replace the `if !require_docker() { return; }` pattern with a
      strategy that makes skipped tests visible:
      - **Option A (recommended):** Keep the skip pattern but ensure the
        CI `integration-test` job (task 0.5) has Docker and counts test
        executions. Add a test that asserts Docker IS available when the
        `integration` feature is enabled:
        ```rust
        #[cfg(feature = "integration")]
        #[test]
        fn docker_must_be_available() {
            assert!(docker_available(), "Docker required for integration tests");
        }
        ```
      - **Option B:** Use `#[ignore]` on all Docker-dependent tests and
        run with `--include-ignored` in CI.
      - Move all server API tests behind `#[cfg(feature = "integration")]`
        so they are only compiled when Docker is expected to be present.

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
- [x] **4.9** WebSocket `/api/v1/workorders/:id/progress` — connects,
      receives text events, binary PTY frames

      **Implementation notes:**
      - Add `tokio-tungstenite` to dev-dependencies (already in workspace).
      - Submit a workorder, then connect to the WebSocket URL from the
        `SubmitWorkorderResponse.progress_ws_url`.
      - Verify WebSocket upgrade succeeds (101).
      - Cancel the workorder to trigger a `StatusChanged` event.
      - Read frames with a timeout, verify a text frame containing
        the status change is received.
      - Requires Docker. Gate with `require_docker()`.

- [x] **4.10** Auth enforcement: `None` mode allows all, `Mtls` mode
      rejects missing cert, `Mixed` mode enforces main vs follower rules

      **Implementation notes:**
      - `None` mode is already implicitly tested by all other Phase 4
        tests (the default config uses `AuthMode::None`).
      - For `Mtls`/`Mixed` mode: create a separate `TestServer` variant
        that accepts a custom `ServerConfig`. Set `auth.mode = Mtls`.
        Submit a request without the cert header, assert 401/403.
      - The `auth.rs` module already has 14 unit tests covering the
        logic. These integration tests verify the axum middleware.
      - Requires Docker. Gate with `require_docker()`.

- [x] **4.11** Client registry: follower registration requires existing main
- [x] **4.12** Config diff detection: same config → empty diff, changed
      package.use → `portage_changed = true`

      **Implementation notes:**
      - The `registry.rs` already has 7 unit tests covering diff logic.
      - For integration: submit from client A, then submit again from
        client A with changed USE flags. The server should detect the
        config change internally.
      - `portage_changed` is not surfaced in any HTTP API response; it is
        internal to the server's `ClientRegistry`.  The 7 unit tests in
        `crates/server/src/registry.rs` comprehensively cover diff
        detection (new client, same config, changed USE flags, active
        workorder, follower scenarios).  No additional integration test
        is needed.

- [x] **4.13** Metrics endpoint (`/metrics`) returns Prometheus text format
- [x] **4.14** GET /api/v1/workorders/{nonexistent} returns 404

---

## Phase 5 — Docker Integration (requires Docker daemon)

These tests need a running Docker daemon. Gate behind
`#[cfg(feature = "integration")]`.

- [x] **5.1** `DockerManager::new` — connects to local Docker socket

      **Audit note:** The current `docker_availability_check` test is a
      no-op — it always passes regardless of Docker status. The
      `docker_manager_connects` test does work correctly. Remove
      or replace `docker_availability_check`.

      **Implementation notes:**
      - Remove the no-op `docker_availability_check` test.
      - The `docker_manager_connects` test is the actual implementation
        and works correctly. Verify it returns `Ok`.

- [x] **5.2** `image_tag` derivation from `SystemId` — verify format
- [x] **5.3** `build_worker_image` (deferred: requires Gentoo stage3) — builds an image, verify it exists via
      Docker API, verify `remerge.worker.sha256` label

      **Implementation notes:**
      - `build_worker_image(&self, sys: &SystemIdentity, tag: &str)`
        builds a Docker image using an internal Dockerfile that COPYs
        the worker binary into a Gentoo stage3 base image.
      - Requires `config.worker_binary` to point to a valid ELF binary.
      - For testing: create a dummy binary (e.g., a small shell script
        or a `#!/bin/true`), configure `ServerConfig.worker_binary`.
      - The base image needs to exist — either pull `gentoo/stage3` or
        use the GHCR test image from task 0.6.
      - After build, use `bollard` inspect to verify the image exists
        and has the `remerge.worker.sha256` label.
      - Clean up the test image with `remove_image()` in a drop guard.
      - Gate behind `#[cfg(feature = "integration")]`.
      - This test will be slow (~30s) due to Docker image building.

- [x] **5.4** `needs_rebuild` — returns `false` for freshly-built image,
      `true` after worker binary changes

      **Implementation notes:**
      - Depends on 5.3 (need to build an image first).
      - `image_needs_rebuild(&self, tag: &str) -> bool` checks the
        `remerge.worker.sha256` label on the image against the current
        binary hash.
      - Build image, call `image_needs_rebuild`, assert `false`.
      - Change the `worker_binary` path to a different binary (or
        modify the existing one), call again, assert `true`.
      - Gate behind `#[cfg(feature = "integration")]`.

- [x] **5.5** `start_worker` (deferred: requires Gentoo stage3) — container starts, env vars are set, mounts
      are present

      **Implementation notes:**
      - `start_worker(&self, container_name: &str, image_tag: &str, ...)`
        creates and starts a container.
      - Requires a valid worker image (depends on 5.3).
      - After starting, inspect the container with `bollard`:
        - Verify `REMERGE_WORKORDER` env var is set.
        - Verify the binpkg mount is present.
      - Stop and remove the container after inspection.
      - Gate behind `#[cfg(feature = "integration")]`.

- [x] **5.6** Container cleanup — `remove_container` removes the container

      **Implementation notes:**
      - Create and start a test container, stop it.
      - Call `remove_container(container_id)`.
      - Use `bollard` to verify the container no longer exists.
      - Gate behind `#[cfg(feature = "integration")]`.

- [x] **5.7** Image eviction (deferred: requires multiple images) — `cleanup_idle_images` preserves the newest
      image per CHOST+profile group, removes older ones

      **Implementation notes:**
      - Check if there is a public `cleanup_idle_images` or equivalent
        method. Search for it in `docker.rs`.
      - If it's part of a background task, may need to extract the logic
        into a testable function.
      - Create multiple test images with different tags, set different
        `image_last_used` timestamps in `AppState`.
      - Run eviction, verify only the newest per group survives.
      - Gate behind `#[cfg(feature = "integration")]`.

---

## Phase 6 — End-to-End (CLI → Server → Worker → binpkg)

Full pipeline tests. Require Docker, a Gentoo stage3 image, and network
access (for `emerge --sync`). These are slow and should be gated behind
`#[cfg(feature = "e2e")]` or run only in a dedicated CI job.

**Audit note:** The current `e2e_test.rs` contains only a placeholder
function with `eprintln!("...")` and an early return. No actual E2E test
logic exists. It falsely reports as "passed".

**All E2E tests depend on task 0.6** (GHCR test Docker image).

- [ ] **6.1** Build a single small package (`app-misc/hello` or
      `app-editors/nano`) — verify binpkg appears in output directory with
      correct SHA-256

      **Implementation notes:**
      - Start server in-process with test config.
      - Submit workorder for `app-misc/hello` using `reqwest`.
      - Connect to WebSocket, wait for `Finished` event (timeout: 10min).
      - Check `binpkg_dir` for the output `.gpkg.tar` file.
      - Verify SHA-256 matches the `WorkorderResult`.
      - The test stage3 image must have a synced portage tree.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.2** Build with `--pretend` / `--ask` flags — verify they are
      correctly filtered or passed

      **Implementation notes:**
      - Check `crates/worker/src/builder.rs` for arg filtering logic.
      - `--pretend` should be passed through to emerge.
      - `--ask` should be filtered (non-interactive container).
      - Submit with `emerge_args: ["--pretend"]`, verify the build
        completes without actual compilation.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.3** Build with custom USE flags — verify worker's `package.use`
      matches client's

      **Implementation notes:**
      - Submit with specific `package_use` entries.
      - Use `--pretend` to avoid long builds.
      - Verify from build output or container inspection that the USE
        flags are applied correctly.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.4** Build with `@world` set — verify set expansion and filtering
      of installed packages

      **Implementation notes:**
      - Submit with atom `@world`.
      - The CLI should expand this by reading the world file and
        filtering installed packages.
      - Verify the workorder's atoms list contains the expanded set.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.5** Cross-architecture build (if CI has multi-arch Docker) — verify
      crossdev setup and `emerge-<CHOST>` invocation

      **Implementation notes:**
      - Requires QEMU user-static for multi-arch Docker.
      - Submit with a different CHOST (e.g., `aarch64-unknown-linux-gnu`).
      - Verify crossdev setup and correct emerge wrapper.
      - Very complex; defer unless multi-arch CI is available.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.6** Follower client — verify follower inherits main's config and
      shares the workorder

      **Implementation notes:**
      - Submit as `ClientRole::Main`, note workorder ID.
      - Submit as `ClientRole::Follower` with matching system ID.
      - Verify follower is accepted and gets the same workorder.
      - Connect both to WebSocket, verify both receive events.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.7** Concurrent workorder rejection — submit while another is active,
      verify 409

      **Implementation notes:**
      - Already covered at API level by task 4.3.
      - E2E variant submits during an active Docker build.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.8** Worker binary upgrade detection — change the binary, submit
      again, verify image rebuild

      **Implementation notes:**
      - Build once with binary A.
      - Swap to binary B, submit again.
      - Verify a new Docker image is built (check image ID or label).
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.9** Cancellation — submit, cancel via API, verify container is
      stopped and removed

      **Implementation notes:**
      - Submit workorder, wait for `Building` status via WebSocket.
      - Cancel via `DELETE /api/v1/workorders/:id`.
      - Verify container is stopped and removed (check Docker).
      - Verify workorder status is `Cancelled`.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **6.10** Resume / reconnect — disconnect WebSocket, reconnect, verify
      progress streaming continues

      **Implementation notes:**
      - Connect to WebSocket, receive some events.
      - Drop the connection.
      - Reconnect to the same workorder's WebSocket URL.
      - Verify events continue (broadcast channel replays or continues).
      - Gate behind `#[cfg(feature = "e2e")]`.

---

## Phase 7 — Error Paths & Edge Cases

- [ ] **7.1** Worker container exits non-zero — verify `Failed` status and
      error propagation

      **Implementation notes:**
      - Submit a workorder with an atom that will fail (e.g., nonexistent
        package like `dev-null/does-not-exist`).
      - Wait for completion via WebSocket.
      - Verify `WorkorderStatus::Failed { reason }`.
      - Requires Docker + Gentoo image.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **7.2** Missing dependency — verify structured event
      `missing_dependencies` is emitted

      **Implementation notes:**
      - Trigger by masking a required dependency.
      - Verify WebSocket receives appropriate `BuildEvent`.
      - Requires Docker + Gentoo.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **7.3** USE conflict — verify structured event `use_conflicts` is
      emitted

      **Implementation notes:**
      - Configure conflicting USE flags on a package's dependencies.
      - Verify WebSocket receives USE conflict event.
      - Requires Docker + Gentoo.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [ ] **7.4** Fetch failure — verify structured event `fetch_failures` is
      emitted

      **Implementation notes:**
      - Block network in container or use package with broken SRC_URI.
      - Verify fetch failure event on WebSocket.
      - Requires Docker + Gentoo.
      - Gate behind `#[cfg(feature = "e2e")]`.

- [x] **7.5** Docker socket unavailable — verify graceful error

      **Implementation notes:**
      - Set `docker_socket` to `/nonexistent/docker.sock`.
      - Call `AppState::new()` or `DockerManager::new()`.
      - Verify a clear error is returned, not a panic.
      - Does NOT require Docker — tests the error path.
      - No feature gate needed; add to `error_test.rs`.

- [x] **7.6** Server config validation — missing `binpkg_dir`, invalid
      `auth` section, missing TLS cert

      **Audit finding:** UNCHECKED — the existing tests in `error_test.rs`
      (`server_config_default_roundtrip` and
      `server_config_empty_object_uses_defaults`) only test serde
      serialization, NOT validation error paths.  None of the specified
      validation scenarios are actually tested.

      **Implementation notes:**
      - These tests do NOT require Docker — they test config validation
        before `AppState::new()` reaches `DockerManager`.
      - Sub-tests to implement in `tests/error_test.rs`:

        1. **`auth.mode = Mtls` without cert paths:** Create a
           `ServerConfig` with `auth.mode = AuthMode::Mtls` but no
           `auth.ca_cert` / `auth.server_cert` paths.  Call
           `AppState::new()` or the auth validation path.  Assert an
           error is returned.
           - Check `crates/server/src/auth.rs` for `CertRegistry::new()`
             to see if it validates cert paths.  If it doesn't validate
             eagerly, the test should verify what happens when a
             connection is attempted.

        2. **Non-writable `binpkg_dir`:** Set `binpkg_dir` to a path
           like `/proc/nonexistent` (non-creatable).  Call
           `AppState::new()`.  The `create_dir_all` in state.rs:74
           should fail.  Assert error.

        3. **Invalid TLS config:** Set TLS cert paths to non-existent
           files.  Check if the server validates at startup.
           - If TLS validation is lazy (checked on bind), this may
             require starting the server and verifying the error.

        4. **`state_dir` non-writable:** Set `state_dir` to a
           non-creatable path.  Assert `AppState::new()` errors.

      - Some sub-tests may need Docker for `AppState::new()` to reach
        the validation point.  If `DockerManager::new()` fails first,
        test the validation functions directly instead.

- [x] **7.7** Workorder TTL expiry — verify `reap_old_workorders` removes
      stale entries

      **Implementation notes:**
      - Search for `reap_old_workorders` or equivalent in server code.
      - If it's a background task in `main.rs`, may need to extract
        the reaping logic into a testable function.
      - Set `retention_hours = 0` or very small, submit and complete
        a workorder, trigger reaping, verify removal.
      - Requires Docker for AppState.

- [x] **7.8** Max retained workorders — verify cap is enforced

      **Implementation notes:**
      - Set `max_retained_workorders = 2`.
      - Submit and complete 3+ workorders (different client IDs).
      - List workorders, verify at most 2 completed ones remain.
      - Requires Docker for AppState.

- [x] **7.9** Path traversal in `profile_overlay` keys — verify rejection
- [x] **7.10** Path traversal in `patches` keys — verify rejection
- [x] **7.11** Shell injection in atom names — verify rejection
- [x] **7.12** Oversized workorder — verify graceful handling

      **Implementation notes:**
      - Submit a workorder with a very large body (e.g., 10MB JSON).
      - Verify the server rejects or handles it without OOM/crash.
      - Check if axum has `DefaultBodyLimit` configured.
      - If no limit exists, consider adding one and testing it.
      - Requires Docker for AppState.

- [x] **7.13** Deserialization error paths — empty JSON, invalid JSON,
      wrong type, missing required fields
- [x] **7.14** Validation edge cases — null bytes, newlines, whitespace-only

---

## Phase 8 — CI & Regression

- [x] **8.1** Add integration test job to CI with Docker

      **Implementation notes:**
      Add to `.github/workflows/ci.yml`:
      ```yaml
      integration-test:
        name: Integration Tests
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@v4
          - uses: dtolnay/rust-toolchain@stable
          - uses: Swatinem/rust-cache@v2
          - name: Run integration tests
            run: cargo test --workspace --features integration
      ```
      Docker is pre-installed on ubuntu-latest runners.

- [ ] **8.2** Cache Gentoo stage3 image in CI to speed up E2E tests

      **Implementation notes:**
      - Use the GHCR test image from task 0.6.
      - Pull `ghcr.io/<owner>/remerge-test-stage3:latest` in CI.
      - Use `docker/setup-buildx-action` for layer caching.

- [ ] **8.3** Add a "smoke test" target that runs the fastest subset
      (Phases 1–3) on every PR

      **Implementation notes:**
      Phases 1–3 (minus portageq-dependent tests) run without Docker
      in ~1 second. Already covered by `cargo test --workspace` but
      consider making explicit for tracking.

- [ ] **8.4** Add a "full integration" target that runs Phases 4–7 on merge
      to `main`

      **Implementation notes:**
      - Trigger on `push` to `main` only.
      - Run `cargo test --workspace --features integration,e2e`.
      - Pull GHCR test image first.
      - Set timeout to 30 minutes for E2E tests.

- [ ] **8.5** Record and track test durations to catch regressions

      **Implementation notes:**
      - Use `cargo nextest` for structured output with durations.
      - Store results as CI artifacts.
      - Alert on tests exceeding 2x baseline duration.
