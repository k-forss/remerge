# Archived: Integration Test Suite Task Plan

This archive summarizes the completed integration-test milestone that used to
be tracked in `TASKS.md`. It is no longer the active project tracker; use
[README.md](../../README.md) and the linked project documentation for current project
state and future work.

## Completion status

The milestone ended with all phases marked complete after Audit #4. The final
state asserted that defects from Audit #3 had been fixed and that every task had
a single-outcome assertion: no `assert!(A || B)`, no ignored results, and no
always-pass branches.

## Completed phases

- Phase 0 — Infrastructure and scaffolding.
- Phase 1 — Types and validation tests with no I/O.
- Phase 2 — CLI Portage reader tests using filesystem fixtures.
- Phase 3 — Worker Portage setup tests without Docker.
- Phase 4 — In-process server API tests behind the `integration` feature.
- Phase 5 — Docker integration tests behind the `integration` feature.
- Phase 6 — End-to-end CLI → server → worker → binpkg tests behind the `e2e`
  feature.
- Phase 7 — Error paths and edge cases.
- Phase 8 — CI and regression workflow support, including stage3 image caching
  and nextest/JUnit reporting.

## Durable testing rules retained from the milestone

- Do not mark tests complete until they compile and run in the intended feature
  set.
- Use one expected outcome per assertion.
- Do not accept both success and failure branches in a test.
- Do not use `eprintln!` as a substitute for an assertion.
- Do not silently skip Docker/E2E prerequisites in a way that makes tests pass
  without exercising behavior.
- If a test is correct but depends on unavailable external state, document the
  prerequisite or known failure explicitly.

## Current verification notes

- `cargo test --workspace` remains the ungated local verification command.
- `cargo test --workspace --features integration` still requires Docker.
- `cargo test --workspace --features integration --test load_test` exercises
  concurrent submission pressure without needing the slower E2E pipeline.
- `cargo test --workspace --features integration,e2e` still requires Docker,
  a usable `remerge-worker` binary, and the `remerge/test-stage3:latest`
  image. CI first tries to pull that image from GHCR and falls back to the
  local test harness build path when it is absent.

## Historical source

The original checklist was removed from the repository root to avoid maintaining
multiple active task trackers. Full historical detail remains available in git
history before the roadmap consolidation.
