# Portage Compatibility Specification

This document defines the Portage-facing compliance surface for remerge.
It is the primary specification for future compatibility and transparency
tests. The goal is not to restate all of Portage, but to make explicit which
Portage behaviors remerge must preserve, which remerge-specific divergences are
allowed, and which current gaps are known.

Validated against:

- Portage documentation index and dependency-resolution chapters at
  <https://dev.gentoo.org/~zmedico/portage/doc/> on 2026-05-18
- `gentoo/portage` `master` as retrieved from
  <https://github.com/gentoo/portage/tree/master/lib/portage> on 2026-05-18
- current remerge implementation and compatibility tests in this repository on
  2026-05-18

## Purpose

This specification exists to answer one question clearly:

> When remerge wraps `emerge`, which Portage behaviors must remain compatible,
> and where are remerge-specific behaviors intentionally different?

The intended audience is:

- maintainers changing CLI, server, or worker behavior
- reviewers evaluating Portage compatibility regressions
- future test authors building compliance and transparency coverage

## Source hierarchy

When sources disagree or differ in precision, use this precedence order:

1. Portage user-facing documentation from `dev.gentoo.org`
2. Portage implementation in `gentoo/portage`
3. remerge documentation in this repository
4. remerge implementation in this repository

If Portage documentation is silent and Portage code is clear, Portage code is
authoritative for this specification.

## Normative language

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are used as
described in RFC 2119 style language.

## Compatibility model

remerge is not a second package manager. It is a distributed wrapper around
Portage.

That implies three categories of behavior:

1. Exact or near-exact Portage compatibility
   remerge MUST preserve Portage semantics for package selection inputs,
   dependency resolution inputs, and binpkg consumption expectations.
2. Controlled remerge-specific behavior
   remerge MAY add transport, caching, worker orchestration, and other remote
   execution behavior, but these differences MUST be explicit and documented.
3. Out-of-scope behavior
   remerge does not redefine ebuild semantics, Portage dependency solving, or
   repository policy.

## Authoritative upstream references

The following upstream references define the behavior most relevant to remerge:

- Portage dependency resolution: Chapter 3, Chapter 4, Chapter 5
  - <https://dev.gentoo.org/~zmedico/portage/doc/ch03.html>
  - <https://dev.gentoo.org/~zmedico/portage/doc/ch04.html>
  - <https://dev.gentoo.org/~zmedico/portage/doc/ch05.html>
- Portage configuration loading:
  - <https://github.com/gentoo/portage/blob/master/lib/portage/package/ebuild/config.py>
- `package.accept_keywords` behavior:
  - <https://github.com/gentoo/portage/blob/master/lib/portage/package/ebuild/_config/KeywordsManager.py>
- `package.use` behavior:
  - <https://github.com/gentoo/portage/blob/master/lib/portage/package/ebuild/_config/UseManager.py>
- binary repository and `PORTAGE_BINHOST` handling:
  - <https://github.com/gentoo/portage/blob/master/lib/portage/binrepo/config.py>
- `emerge` option definitions:
  - <https://github.com/gentoo/portage/blob/master/lib/_emerge/main.py>

## Requirements

### PC-001 Resolver ownership

Portage remains the source of truth for dependency resolution and task
scheduling.

Requirements:

- remerge MUST NOT implement an independent dependency solver.
- remerge MUST delegate package dependency evaluation, conflict handling, and
  execution ordering to Portage.
- remerge MUST treat persistent configuration, current command parameters, and
  package dependency data as Portage-owned constraints.

Upstream basis:

- Portage Chapter 3 defines dependency resolution as constraint satisfaction,
  including persistent configuration parameters, current command parameters,
  and package dependencies.
- Portage Chapter 4 defines decision making for disjunctive dependencies.
- Portage Chapter 5 defines task execution order through dependency graphs.

remerge evidence:

- the worker executes `emerge` directly in
  [crates/worker/src/builder.rs](../crates/worker/src/builder.rs)
- the CLI executes local `emerge` directly in
  [crates/cli/src/args.rs](../crates/cli/src/args.rs)

Status: aligned

Audit finding:

- verified in current code and contract coverage

### PC-002 Client configuration snapshot coverage

remerge MUST capture and replay the Portage inputs that materially affect
dependency resolution and binpkg compatibility.

Minimum required coverage:

- `make.conf`
- `package.use`
- `package.accept_keywords`
- `package.license`
- `package.mask`
- `package.unmask`
- `package.env`
- referenced files under `env/`
- `repos.conf`
- `patches/`
- `profile/` overlay
- active profile selection
- world set content where set expansion is performed client-side

Upstream basis:

- Portage Chapter 3 names persistent configuration parameters from
  `make.profile`, `make.conf`, and `/etc/portage` as dependency-resolution
  constraints.

remerge evidence:

- snapshot reading is implemented in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- worker-side replay is implemented in
  [crates/worker/src/portage_setup.rs](../crates/worker/src/portage_setup.rs)

Status: aligned in scope

Audit finding:

- verified by the snapshot contract in
  [tests/cli_portage_test.rs](../tests/cli_portage_test.rs)
- worker replay remains verified by
  [tests/worker_setup_test.rs](../tests/worker_setup_test.rs)

### PC-003 `make.conf` path compatibility

remerge MUST support the same effective `make.conf` search behavior that
Portage uses for local configuration.

Upstream basis:

- Portage `config.py` reads both `config_root/etc/make.conf` and
  `config_root/MAKE_CONF_FILE`, using both when they are distinct.

remerge evidence:

- the CLI now reads both `/etc/portage/make.conf` and `/etc/make.conf` via
  `read_make_conf_vars()` in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- dual-path behavior is covered by
  [tests/cli_portage_test.rs](../tests/cli_portage_test.rs)

Status: aligned

Audit finding:

- previously documented as a gap; now implemented and contract-tested

### PC-004 Effective USE and USE_EXPAND semantics

remerge MUST preserve the effective USE state that Portage resolves from the
profile and local configuration.

Requirements:

- remerge MUST prefer Portage-resolved USE values over raw `make.conf` USE when
  available
- remerge MUST preserve USE_EXPAND families separately from plain USE
- remerge MUST avoid duplicating USE_EXPAND-derived flags into `USE`

Upstream basis:

- Portage configuration and dependency resolution are driven by effective
  configuration state, not only raw literals from `make.conf`

remerge evidence:

- the CLI uses `portageq envvar USE` and USE_EXPAND-related queries in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- the worker writes a normalized `USE` and explicit USE_EXPAND variables in
  [crates/worker/src/portage_setup.rs](../crates/worker/src/portage_setup.rs)

Status: aligned

Audit finding:

- verified by the worker-side contract in
  [tests/worker_setup_test.rs](../tests/worker_setup_test.rs)

### PC-005 Recursive directory semantics for user package config

Where remerge claims to capture Portage user package configuration directories,
it SHOULD preserve Portage's directory traversal semantics rather than only the
top-level files.

Upstream basis:

- Portage `KeywordsManager` reads `package.accept_keywords` with
  `grabdict_package(... recursive=1, ...)`
- Portage `UseManager` reads `package.use` with
  `grabdict_package(... recursive=1, ...)`

remerge evidence:

- remerge now traverses package configuration directories recursively through
  `read_package_entries_recursive()` in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- the shared package-entry reader is used for `package.use`,
  `package.accept_keywords`, `package.license`, `package.mask`,
  `package.unmask`, and `package.env`
- nested directory behavior is covered by
  [tests/cli_portage_test.rs](../tests/cli_portage_test.rs)

Status: aligned

Audit finding:

- previously documented as a gap; now implemented for all current
  `package.*` readers that share the recursive loader

### PC-006 Empty `package.accept_keywords` entry semantics

remerge MUST preserve Portage semantics for empty `package.accept_keywords`
entries.

Upstream basis:

- Portage `KeywordsManager` defaults an empty entry to unstable variants of the
  current global `ACCEPT_KEYWORDS` set, not to a universal literal `~*`

remerge evidence:

- the CLI derives empty-entry defaults from the current global
  `ACCEPT_KEYWORDS` via `empty_accept_keywords_defaults()` in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- the behavior is covered by
  [tests/cli_portage_test.rs](../tests/cli_portage_test.rs)

Status: aligned

Audit finding:

- previously documented as a gap; now implemented and contract-tested

### PC-007 Repository and profile fidelity

remerge MUST preserve repository identity and effective profile selection well
enough that the worker resolves packages against the same repository/profile
context as the client.

Requirements:

- `repos.conf` content MUST be transported to the worker
- local profile overlay content under `/etc/portage/profile/` MUST be
  transported to the worker
- active profile selection MUST be reproduced in the worker

remerge evidence:

- the CLI snapshots repo config and profile overlay in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- the worker replays repo config and profile state in
  [crates/worker/src/portage_setup.rs](../crates/worker/src/portage_setup.rs)

Status: aligned

Audit finding:

- verified by the replay contract in
  [tests/worker_setup_test.rs](../tests/worker_setup_test.rs)

### PC-008 Patch and per-package environment fidelity

remerge MUST preserve Portage behaviors that affect build outputs through local
patches and per-package environment overrides.

Requirements:

- files under `/etc/portage/patches/` MUST be transported to the worker
- `package.env` mappings and referenced env files MUST be transported to the
  worker

remerge evidence:

- the CLI snapshots patches and env files in
  [crates/cli/src/portage.rs](../crates/cli/src/portage.rs)
- the worker writes them back in
  [crates/worker/src/portage_setup.rs](../crates/worker/src/portage_setup.rs)

Status: aligned

Audit finding:

- verified by the replay contract in
  [tests/worker_setup_test.rs](../tests/worker_setup_test.rs)

### PC-009 Worker-side `emerge` invocation policy

remerge MAY constrain which `emerge` actions are allowed remotely, but those
constraints MUST be explicit and documented.

Allowed remerge-specific behavior:

- forcing remote builds through `--buildpkg`
- rejecting dangerous or nonsensical remote flags such as `--depclean`,
  `--unmerge`, or search/info actions
- using ephemeral worker-only flags that do not mutate the client host

remerge evidence:

- worker invocation and flag filtering live in
  [crates/worker/src/builder.rs](../crates/worker/src/builder.rs)

Current documented divergence:

- the worker injects `--buildpkg`, `--usepkg`, `--verbose`, `--keep-going`,
  `--newuse`, `--update`, `--autounmask-write`, and
  `--autounmask-continue`
- the worker filters `--pretend`, `--getbinpkg`, `--sync`, `--search`,
  `--depclean`, `--unmerge`, and related flags

Compliance note:

- this divergence is allowed because it is explicit, worker-scoped, and does
  not claim to be raw `emerge` passthrough behavior

Status: aligned with documented divergence

Audit finding:

- the injected and filtered flag policy is now covered by
  [crates/worker/src/builder.rs](../crates/worker/src/builder.rs) unit
  contracts

### PC-010 Local install invocation and binhost handoff

remerge MUST make the post-build local install path transparent and compatible
with Portage's binary package discovery rules.

Upstream basis:

- Portage `main.py` defines `--getbinpkg` and `--usepkg` as normal `emerge`
  options
- Portage `binrepo/config.py` treats `PORTAGE_BINHOST` as a first-class input
  and converts it into implicit binrepo entries

Requirements:

- the local install path MUST invoke `emerge` with binary-package consumption
  flags
- remerge MUST either:
  - configure Portage binrepos out of band and document that requirement, or
  - inject the appropriate runtime binhost configuration, such as
    `PORTAGE_BINHOST`, before local `emerge` execution
- the chosen mechanism MUST be explicit and testable

remerge evidence:

- the CLI currently invokes local `emerge` with `--getbinpkg --usepkg` in
  [crates/cli/src/args.rs](../crates/cli/src/args.rs)
- the server returns `binhost_uri` in workorder results from
  [crates/server/src/queue.rs](../crates/server/src/queue.rs)
- the CLI now consumes `binhost_uri` and sets `PORTAGE_BINHOST` before the
  local `emerge` invocation in
  [crates/cli/src/args.rs](../crates/cli/src/args.rs)
- the CLI now also syncs built `.gpkg` files into the local `PKGDIR` and can
  point the follow-up local install at `file://<PKGDIR>` in
  [crates/cli/src/args.rs](../crates/cli/src/args.rs)
- the local handoff logic is covered by
  [crates/cli/src/args.rs](../crates/cli/src/args.rs)
- published binhost result URLs are covered end to end by
  [tests/e2e_test.rs](../tests/e2e_test.rs)

Status: aligned

Audit finding:

- previously documented as a transparency gap; now implemented with explicit
  runtime handoff and e2e verification of published binhost result URLs

### PC-011 Binpkg output and signing

remerge MAY add worker-side defaults required for remote binpkg production,
provided those defaults are explicit and compatible with Portage.

Allowed remerge-specific behavior:

- forcing `PKGDIR` to the worker-mounted binpkg directory
- enabling `buildpkg`
- enabling binpkg signing features when server signing is configured

remerge evidence:

- worker `make.conf` generation in
  [crates/worker/src/portage_setup.rs](../crates/worker/src/portage_setup.rs)
- signing key publication and result metadata documented in
  [README.md](../README.md) and [docs/operations.md](operations.md)

Status: aligned

Audit finding:

- verified by both worker-side make.conf contracts in
  [tests/worker_setup_test.rs](../tests/worker_setup_test.rs) and queue/result
  publication coverage in [tests/e2e_test.rs](../tests/e2e_test.rs)

### PC-012 Sync and overlay behavior

remerge MAY optimize repository sync behavior for worker containers, but any
deviation from a plain `emerge --sync` flow MUST be explicit.

remerge evidence:

- sync behavior is implemented in
  [crates/worker/src/builder.rs](../crates/worker/src/builder.rs)

Current documented divergence:

- when `REMERGE_SKIP_SYNC` is set, the worker skips the main sync and only
  syncs missing overlays
- overlays that require authenticated transport are skipped in ephemeral
  workers
- non-Gentoo local overlay working trees are snapshotted on the client and
  restored into the worker before build
- distfiles referenced by snapshotted overlay Manifests are transported from
  the client and restored into the worker distfiles cache

Compliance note:

- this divergence is allowed only because it is explicit and operationally
  documented

Status: aligned with documented divergence

Audit finding:

- skip-sync behavior is covered by the worker-side contract in
  [crates/worker/src/builder.rs](../crates/worker/src/builder.rs)

## Current compliance summary

### Aligned or intentionally aligned

- Portage remains the dependency resolver and task scheduler
- client configuration snapshot coverage matches the current supported scope
- `make.conf` path compatibility now covers both `/etc/make.conf` and
  `/etc/portage/make.conf`
- effective USE and USE_EXPAND handling is intentionally normalized
- recursive package directory traversal is implemented for the current
  `package.*` readers
- empty `package.accept_keywords` entries derive unstable keywords from the
  current global `ACCEPT_KEYWORDS`
- repo config, profile overlay, patches, and env files are transported
- non-Gentoo local repo working trees and Manifest-backed distfiles are now
  transported via the staged worker runtime, which is materialized from a
  server-side content-addressed blob store and annotated with blob references
  plus repo tree manifests for future manifest-based transport
- worker-side binpkg output and signing are explicit
- worker-side flag filtering is explicit rather than implicit
- local install now performs explicit runtime binhost handoff via
  `PORTAGE_BINHOST`

### Known gaps in audited scope

- no known implementation gaps remain in the audited PC-001 through PC-012
  scope as of 2026-05-18

### Open questions for later verification

- whether any additional Portage-managed directories outside the current
  snapshot scope should be added for future supported workflows
- whether all remaining `package.*` directories not currently materialized by
  remerge need explicit upstream parity review beyond the current shared loader
- whether any additional Portage config paths beyond the current snapshot scope
  are necessary for supported remerge workflows

## Test-design guidance

Future compatibility tests derived from this specification should separate the
following concerns:

1. Portage parity
   verify that the same effective configuration produces the same package
   selection and binpkg compatibility expectations.
2. remerge transport fidelity
   verify that client-side config is captured and replayed without silent loss.
3. documented divergence
   verify that explicit remerge-only behavior stays explicit and does not drift
   into accidental incompatibility.
4. transparency
   verify that operators and users can tell which behavior came from Portage
   and which came from remerge.

This document should be updated before or alongside any change to:

- config snapshot coverage
- worker `emerge` flag policy
- local install/binhost behavior
- signing/binpkg output behavior
- repository or profile replication behavior