# Archived: Integration Test Agent Prompt

This archive summarizes the old `PROMPT.md` file. The original prompt was an
agent-specific work order for finishing the integration-test suite after earlier
false-positive task completions. It is no longer current project state and must
not be used as an active task tracker.

## Why it was archived

- It claimed that 18 unchecked integration-test tasks remained, which became
  stale after the final integration-test milestone was completed.
- It duplicated project overview and test-quality guidance that now belongs in
  [README.md](README.md) or test documentation.
- It was written as one-off agent instructions, not maintainer documentation.

## Durable guidance retained

Future tests should preserve the useful constraints from the prompt:

- No always-pass assertions such as `assert!(status == A || status == B)`.
- No silent success branches for expected failures.
- No skipped Docker/E2E tests that pass without exercising behavior.
- No `eprintln!` in place of assertions.
- Every task must have a concrete verification step before being marked done.

Use [README.md](README.md) for active project context and
[docs/archive/integration-test-suite.md](docs/archive/integration-test-suite.md)
for the completed milestone summary.
