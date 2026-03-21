# Integration Test Suite — Task Plan

Actionable tasks for building a comprehensive integration test suite for remerge.
All items are ordered by dependency (earlier items unblock later ones).

---

## Phase 0 — Infrastructure & Scaffolding

- [ ] **0.1** Create `tests/` directory at workspace root for integration tests
- [ ] **0.2** Add `tests/common/mod.rs` with shared helpers (free port allocation,
      temp dir scaffolding, config builders, timeout wrappers)
- [ ] **0.3** Add `tests/fixtures/` with static test data:
  - [ ] Minimal `make.conf` (CFLAGS, CHOST, USE, FEATURES)
  - [ ] Sample `package.use`, `package.accept_keywords`, `package.mask`,
        `package.unmask`, `package.env`, env files
  - [ ] Minimal `repos.conf` (single `[gentoo]` section with a `location`)
  - [ ] Sample `profile/` overlay directory (parent, use.mask, package.mask)
  - [ ] Sample `patches/` tree (category/package/patch files)
  - [ ] Minimal `remerge.conf` (server URL, client_id)
  - [ ] Minimal `remerged.conf` (binpkg_dir, auth = none)
- [ ] **0.4** Add a `[workspace.metadata.test]` or `[[test]]` integration target
      in `Cargo.toml` (or a `tests/` crate) gated behind
      `#[cfg(feature = "integration")]` so `cargo test` still runs fast by
      default
- [ ] **0.5** Create a CI job in `.github/workflows/ci.yml` that runs integration
      tests on `ubuntu-latest` with Docker available (GitHub-hosted runners
      have Docker pre-installed)

---

## Phase 1 — Types & Validation (no I/O)

These are pure-logic tests that need no Docker, no server, no filesystem.

- [ ] **1.1** `PortageConfig` round-trip: construct → serialize → deserialize →
      assert equality (covers serde defaults like `profile_overlay`,
      `use_flags_resolved`)
- [ ] **1.2** `Workorder` round-trip: all status transitions
      (`Pending → Provisioning → Building → Completed/Failed/Cancelled`)
- [ ] **1.3** `validate_atom` exhaustive: all legal operator/category/name/version
      combinations vs. all rejection classes (shell injection, empty parts,
      unqualified + versioned)
- [ ] **1.4** `MakeConf` field coverage: every `extra_vars` key, empty USE,
      empty FEATURES, `use_flags_resolved = true` vs `false` behaviour
- [ ] **1.5** `ClientRole` / `AuthMode` `Display` + `FromStr` round-trips
- [ ] **1.6** `WorkorderResult` with mixed built/failed packages, SHA-256 hashes

---

## Phase 2 — CLI Portage Reader (filesystem, no network)

Tests that create a temp directory tree mimicking `/etc/portage/` and `/var/db/pkg/`.

- [ ] **2.1** `read_config` golden path: populate a full temp portage tree,
      call `PortageReader::new(root).read_config()`, assert every field
- [ ] **2.2** `read_config` with missing optional dirs (no `package.use/`,
      no `patches/`, no `profile/` overlay) — should succeed with empty maps
- [ ] **2.3** `read_config` with `package.use` as a single file vs. a directory
      of files (Portage supports both)
- [ ] **2.4** `read_profile_overlay` round-trip: write files into
      `<root>/etc/portage/profile/`, call reader, assert `BTreeMap` keys and
      content
- [ ] **2.5** `read_patches_recursive` with nested `category/package/*.patch`
- [ ] **2.6** `read_repos_conf` with multiple `[section]` blocks, verify
      repo names and locations
- [ ] **2.7** `is_installed` with version constraints:
  - [ ] `category/pkg` — any version matches
  - [ ] `=category/pkg-1.2.3` — exact match
  - [ ] `=category/pkg-1.2.3-r1` — exact with revision
  - [ ] `>=category/pkg-2.0` — satisfied and unsatisfied
  - [ ] `<=category/pkg-2.0` — satisfied and unsatisfied
  - [ ] `>category/pkg-2.0` — boundary (2.0 should NOT match)
  - [ ] `<category/pkg-2.0` — boundary
  - [ ] `~category/pkg-1.2.3` — any revision
  - [ ] `=category/pkg-1.2*` — glob
  - [ ] `@world` — always returns false
  - [ ] Uninstalled package — returns false
- [ ] **2.8** `expand_set` for `@world` (reads world file) and `@system`
      (calls `portageq`)
- [ ] **2.9** `split_name_version` edge cases: numeric-only names
      (`dev-libs/1lib`), multi-hyphen (`x11-libs/gtk+-2.0`), no version
- [ ] **2.10** `compare_versions` edge cases: suffixes (`_alpha`, `_beta`,
      `_pre`, `_rc`, `_p`), long numeric segments, revision-only differences

---

## Phase 3 — Worker Portage Setup (filesystem, no Docker)

Tests that call `portage_setup` functions against a temp directory.

- [ ] **3.1** `write_make_conf` golden path: provide a `MakeConf`, assert
      generated file content line-by-line (CHOST, CBUILD, CFLAGS, USE,
      FEATURES, MAKEOPTS, signing, ACCEPT_KEYWORDS, ACCEPT_LICENSE, extra_vars)
- [ ] **3.2** `write_make_conf` with `use_flags_resolved = true` — USE line
      must start with `-* `
- [ ] **3.3** `write_make_conf` with `use_flags_resolved = false` — no `-*`
      prefix
- [ ] **3.4** `write_make_conf` with USE_EXPAND flags — must appear as
      separate variables, not inside USE
- [ ] **3.5** `write_package_config` for each config type (use, keywords,
      license, mask, unmask, env) — both single-entry and multi-entry
- [ ] **3.6** `write_env_files` — write, verify content and permissions
- [ ] **3.7** `write_repos_conf` with server `repos_dir` bind-mount remapping
      (locations must be rewritten to `/var/db/repos/<name>`)
- [ ] **3.8** `write_repos_conf` without server repos_dir — locations
      preserved as-is
- [ ] **3.9** `set_profile` — creates `/etc/portage/make.profile` symlink
      pointing to the correct repo's `profiles/<profile>` path; test with
      multiple repos to verify correct repo selection
- [ ] **3.10** `write_profile_overlay` — writes files to
      `/etc/portage/profile/`, rejects path traversal (`..`), rejects
      absolute paths
- [ ] **3.11** `write_patches` — writes files to `/etc/portage/patches/`,
      creates intermediate category/package dirs, rejects path traversal
- [ ] **3.12** `build_makeopts` — server env REMERGE_PARALLEL_JOBS and
      REMERGE_LOAD_AVERAGE override client MAKEOPTS; absent env falls back
      to client MAKEOPTS
- [ ] **3.13** `apply_config` orchestration — call with a full `PortageConfig`
      and assert that all files are present under the temp root

---

## Phase 4 — Server Unit-level (in-process, no Docker)

Tests that spin up the axum app in-process with a mock/stub Docker layer.

- [ ] **4.1** `POST /api/v1/workorder` — valid submission returns 200 +
      workorder ID
- [ ] **4.2** `POST /api/v1/workorder` — invalid atoms rejected (400)
- [ ] **4.3** `POST /api/v1/workorder` — duplicate active workorder rejected
      (409)
- [ ] **4.4** `GET /api/v1/workorder/:id` — returns workorder with correct
      status
- [ ] **4.5** `GET /api/v1/workorders` — returns list, respects ordering
- [ ] **4.6** `POST /api/v1/workorder/:id/cancel` — transitions to Cancelled
- [ ] **4.7** `GET /health` — returns 200
- [ ] **4.8** `GET /api/v1/info` — returns server version, auth mode,
      binhost URL
- [ ] **4.9** WebSocket `/api/v1/workorder/:id/progress` — connects,
      receives text events, binary PTY frames
- [ ] **4.10** Auth enforcement: `None` mode allows all, `Mtls` mode
      rejects missing cert, `Mixed` mode enforces main vs follower rules
- [ ] **4.11** Client registry: follower registration requires existing main,
      follower cannot push new config
- [ ] **4.12** Config diff detection: same config → empty diff, changed
      package.use → `portage_changed = true`
- [ ] **4.13** Metrics endpoint (`/metrics`) returns Prometheus text format

---

## Phase 5 — Docker Integration (requires Docker daemon)

These tests need a running Docker daemon. Gate behind
`#[cfg(feature = "integration")]`.

- [ ] **5.1** `DockerManager::new` — connects to local Docker socket
- [ ] **5.2** `image_tag` derivation from `SystemId` — verify format
      `<prefix>-<arch>-<profile>-<gcc>`
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
- [ ] **7.9** Path traversal in `profile_overlay` keys — verify rejection
- [ ] **7.10** Path traversal in `patches` keys — verify rejection
- [ ] **7.11** Shell injection in atom names — verify rejection
- [ ] **7.12** Oversized workorder — verify graceful handling

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
