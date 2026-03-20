# Contributing to remerge

Thanks for your interest! Contributions are welcome — bug reports, feature
ideas, docs improvements, and code patches.

## Before you start

- **Open an issue first** for anything beyond a trivial fix. This avoids
  wasted effort and lets us discuss the approach.
- Security vulnerabilities should be reported privately — see
  [SECURITY.md](SECURITY.md).

## Development setup

```bash
# Clone and build
git clone https://github.com/k-forss/remerge.git
cd remerge
cargo build --workspace

# Run the full check suite (same as CI)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

### Workspace layout

| Crate | Path | Description |
|-------|------|-------------|
| `remerge` | `crates/cli` | CLI binary — drop-in emerge wrapper with set expansion and VDB checks |
| `remerge-server` | `crates/server` | HTTP/WS API, Docker orchestration, state persistence, Prometheus metrics |
| `remerge-worker` | `crates/worker` | Runs inside Docker containers, applies portage config, executes builds |
| `remerge-types` | `crates/types` | Shared types (API, auth, client, portage, workorder, validation) |

## Pull request checklist

- [ ] `cargo fmt --all` — code is formatted
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — no warnings
- [ ] `cargo test --workspace` — all tests pass
- [ ] Update `CHANGELOG.md` under `[Unreleased]` if user-facing

## Code style

- Follow existing patterns in the codebase.
- Keep functions focused and well-documented.
- Prefer explicit error handling over `.unwrap()` (tests excepted).
- Use `tracing` for structured logging — not `println!` or `eprintln!`.

## Commit messages

Use conventional-style prefixes where appropriate:

- `feat:` new features
- `fix:` bug fixes
- `refactor:` code changes that neither fix nor add
- `docs:` documentation only
- `ci:` CI/CD changes
- `deps:` dependency updates
- `test:` test additions or corrections

## Release process

Release process details (RC workflow, signing pipeline, key management)
are in [DEVELOPMENT.md](DEVELOPMENT.md#release-process).

Short summary:

1. Create a branch named `rc-X.Y.Z` (e.g. `rc-0.2.0`).
2. The **Prepare RC** workflow automatically updates `CHANGELOG.md` and
   creates a versioned ebuild.
3. Open a PR from the RC branch to `main`.  CI must pass.
4. Merge the PR.  The **Tag Release** workflow creates `vX.Y.Z`.
5. The tag triggers the **Release** workflow which builds binaries,
   generates attestations, and publishes a GitHub Release.  Versioned
   ebuilds are prepared by the RC workflow in step 2.

## License

By contributing you agree that your contributions will be licensed under the
same terms as the project — GPL-2.0-only.
