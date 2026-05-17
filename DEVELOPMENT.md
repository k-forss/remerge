# Development & Maintenance

Internal documentation for maintainers and contributors.  For user-facing
installation and usage instructions, see [README.md](README.md).
For operator deployment, backup, rollback, and monitoring guidance, see
[docs/operations.md](docs/operations.md).
For contribution guidelines, see [CONTRIBUTING.md](CONTRIBUTING.md).
For current project status, release-readiness gates, and implementation-ordered
future work, see [ROADMAP.md](ROADMAP.md).

## Table of contents

- [CI / CD pipeline](#ci--cd-pipeline)
- [Release process](#release-process)
- [Three-phase release signing](#three-phase-release-signing)
- [Required GitHub secrets](#required-github-secrets)
- [PGP key management](#pgp-key-management)
- [Branch and tag rulesets](#branch-and-tag-rulesets)
- [Overlay maintenance](#overlay-maintenance)
- [Multi-arch targets](#multi-arch-targets)
- [Docker images](#docker-images)

---

## CI / CD pipeline

All workflows live in `.github/workflows/`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | push / PR to `main`, `rc-*` | `cargo fmt`, `clippy`, unit tests, fuzz smoke, and full-stack Docker tests |
| `audit.yml` | push / schedule | `cargo deny check` (licenses + advisories) |
| `docker.yml` | push to `main` | Build + push multi-arch Docker image to GHCR |
| `release.yml` | tag `v*` | Multi-arch binary builds, packaging, SLSA attestation, GitHub Release |
| `virustotal.yml` | `release: published` | Scan all release assets, append VirusTotal links to release body |
| `sign-release.yml` | `workflow_run` (after VT) | PGP detached signatures + clear-signed release body seal |
| `rc-prepare.yml` | push to `rc-*` branch | Auto-update changelog and create versioned overlay ebuilds |
| `release-tag.yml` | PR merge from `rc-*` to `main` | Auto-create release tag |

### Test organization and prerequisites

- `cargo test --workspace` covers the ungated unit/filesystem suites.
- `cargo test --workspace --features integration` enables the Docker-backed
  server API and container integration tests.
- `cargo test --workspace --features integration --test load_test` runs the
  concurrent submission/load suite that exercises queue-capacity behavior.
- `cargo test --workspace --features integration,e2e` enables the full
  CLI to server to worker pipeline and is the Docker-backed CI stack on both
  pushes and PRs.
- The main `test` job uses `cargo nextest run --workspace --profile ci`, which
  emits JUnit output via `.config/nextest.toml`.
- CI compares that JUnit output against `.config/test-duration-baseline.json`
  with `scripts/test_duration_baseline.py` and fails when a tracked test slows
  down by 25% or more.
- Integration and E2E runs require Docker. The full-stack CI run also requires
  the `remerge/test-stage3:latest` image; CI pulls it from GHCR first and the
  test harness lazily builds it from `docker/test-stage3.Dockerfile` if needed.
- The fuzz smoke job runs on every push and PR from `fuzz/` with short
  libFuzzer budgets. For deeper local runs, use `cargo fuzz run <target>` from
  the `fuzz/` directory.

### Refreshing the duration baseline

- Run `cargo nextest run --workspace --profile ci`.
- Update the checked-in baseline with:
  `python3 scripts/test_duration_baseline.py --junit target/nextest/ci/junit.xml --baseline .config/test-duration-baseline.json --write-baseline`.
- Review the diff before committing so transient local outliers do not become
  the new baseline.

### Dependency management

- **Dependabot** (`.github/dependabot.yml`) monitors Cargo, GitHub Actions,
  and Docker dependencies with weekly checks.
- **`cargo deny`** (`deny.toml`) enforces license allowlist and advisory
  database checks.

### Worker binary deployment prerequisite

- `remerge-server` requires a readable `remerge-worker` binary before startup.
- Set `worker_binary` in the server config or `REMERGE_WORKER_BINARY` in the
  environment to the installed binary path.
- The server now exits early if the path is missing or unreadable instead of
  waiting for the first worker-image build to fail.

### Client identity contract

- The CLI persists `client_id` in `/etc/remerge.conf` on first run whenever the
  config file is writable.
- If a config file exists without `client_id`, the CLI backfills and rewrites
  one so identities do not drift across restarts.
- Shared identities remain operator-managed: if multiple hosts intentionally use
  the same `client_id`, exactly one should be `main` and the rest should be
  `follower`.

### Deployment hardening notes

- `auth.mode=mtls` protects all certificate-aware endpoints, including `/metrics` and `/binpkgs`.
- `auth.mode=mixed` keeps `/metrics` behind mTLS but intentionally leaves `/binpkgs` public for client binhost downloads.
- Reverse proxies must strip inbound certificate fingerprint headers and re-inject them only after successful client certificate verification.
- Rate limiting is intentionally delegated to the reverse proxy. Apply path-specific policies for workorder submissions, metrics scraping, and binpkg downloads.
- Binary package signing startup validation now fails fast when `gpg_key` / `gpg_home` are partially configured or when the configured secret key is absent from the keyring.
- The example server config now documents request body, queue depth, build timeout, and worker CPU/memory/network limits. Keep those aligned with the reverse-proxy ceilings.
- remerge intentionally does not implement a server-side package-category allowlist. Constrain submissions with auth, proxy policy, and rate limits rather than atom-category gating.

---

## Release process

1. Create a branch named `rc-X.Y.Z` (e.g. `rc-0.2.0`).
2. The **Prepare RC** workflow automatically:
   - Updates `CHANGELOG.md` (moves `[Unreleased]` → `[X.Y.Z]`)
   - Creates versioned overlay ebuilds from the `9999` templates
   - Updates the `CRATES` variable with current Cargo.lock dependencies
3. Open a PR from the RC branch to `main`.  CI must pass.
4. Merge the PR.  The **Tag Release** workflow creates tag `vX.Y.Z`.
5. The tag triggers the three-phase release pipeline (see below).

---

## Three-phase release signing

Every release goes through three sequential phases:

### Phase 1 — Build (`release.yml`)

- Cross-compiles CLI binaries for all supported architectures
- Cross-compiles server binaries for amd64 and arm64
- Packages each binary as `<name>-v<version>-<arch>-linux.tar.gz`
- Generates `<name>-v<version>-SHA256SUMS.txt` covering all archives
- Creates SLSA build provenance attestation (Sigstore)
- Publishes the GitHub Release with all artifacts

### Phase 2 — Scan (`virustotal.yml`)

- Triggered automatically when the release is published
- Downloads and scans all `.tar.gz`, `SHA256SUMS`, and `.asc` files
- Appends VirusTotal analysis links to the release body
- Uses free-tier rate limiting (4 requests/minute)

### Phase 3 — Sign (`sign-release.yml`)

- Triggered automatically after the VirusTotal workflow completes
- Imports the `GPG_PRIVATE_KEY` from GitHub Secrets
- Creates detached PGP signatures (`.asc`) for every `.tar.gz` and
  `SHA256SUMS` file
- Downloads the final release body (which now includes VT links) and
  creates a clear-signed `RELEASE.md.asc` as a cryptographic "seal"
  over the complete release state
- Uploads all `.asc` files to the release

The clear-signed `RELEASE.md.asc` is the maintainer's endorsement that the
release is final — it covers the checksums, attestation references, and
VirusTotal results.

---

## Required GitHub secrets

| Secret | Required | Description |
|--------|----------|-------------|
| `GPG_PRIVATE_KEY` | Yes | ASCII-armored GPG private key (no passphrase) |
| `GPG_PASSPHRASE` | No | Passphrase, if the key is encrypted |
| `VT_API_KEY` | No | VirusTotal API key (free tier works) |

If `GPG_PRIVATE_KEY` is not set, releases are created without PGP signatures
but still include SHA256 checksums and SLSA attestation.

If `VT_API_KEY` is not set, the VirusTotal scan is skipped and Phase 3
signing proceeds without scan results in the release body.

---

## PGP key management

### Current key

| Property | Value |
|----------|-------|
| Fingerprint | `C075 B1EF DC2E 4D23 817A  1BB3 F5B0 BB05 FABD 6151` |
| Algorithm | RSA-4096 |
| Created | 2026-03-20 |
| Expires | 2028-03-19 (2 years) |
| UID | `remerge release signing <kristoffer@forss.cc>` |
| Keyserver | `keys.openpgp.org` |
| Public key | [`keys/release-signing.pub.asc`](keys/release-signing.pub.asc) |
| Private key | GitHub Secrets (`GPG_PRIVATE_KEY`) — **not stored locally** |

### Key rotation procedure

The signing key has a 2-year validity period.  Rotate it before expiry
(or immediately if the key is compromised) using the procedure below.

#### Option A — Extend the current key (preferred if not compromised)

If the key is not compromised, you can extend its expiry without generating
a new key.  This means existing signatures remain verifiable without
importing a new key.

```bash
# 1. Temporarily import the private key from GitHub Secrets
gh secret list -R k-forss/remerge   # verify GPG_PRIVATE_KEY exists

# Export from GitHub is not possible — you need the original key.
# If the private key is truly only in GitHub Secrets and you haven't
# kept a backup, you must generate a new key (Option B).

# If you have a secure backup:
gpg --import /path/to/backup.asc

# 2. Extend the expiry (e.g. another 2 years)
gpg --quick-set-expire <FINGERPRINT> 2y

# 3. Re-export and update GitHub Secrets
gpg --armor --export-secret-keys <FINGERPRINT> | \
  gh secret set GPG_PRIVATE_KEY -R k-forss/remerge

# 4. Update the public key in the repository
gpg --armor --export <FINGERPRINT> > keys/release-signing.pub.asc
cp keys/release-signing.pub.asc \
   overlay/sec-keys/openpgp-keys-remerge/files/remerge-release.asc

# 5. Upload updated public key to keyserver
gpg --keyserver keys.openpgp.org --send-keys <FINGERPRINT>

# 6. Purge the private key from your local keyring
gpg --batch --yes --delete-secret-keys <FINGERPRINT>

# 7. Commit and push
git add keys/ overlay/sec-keys/
git commit -m "chore: extend release signing key expiry"
git push
```

#### Option B — Generate a new key (required if compromised)

```bash
# 1. Generate a new RSA-4096 key
gpg --batch --gen-key <<EOF
%no-protection
Key-Type: RSA
Key-Length: 4096
Name-Real: remerge release signing
Name-Comment: Automated CI release signing for k-forss/remerge
Name-Email: kristoffer@forss.cc
Expire-Date: 2y
%commit
EOF

# 2. Deploy to GitHub Secrets (private key never touches disk)
gpg --armor --export-secret-keys "remerge release signing" | \
  gh secret set GPG_PRIVATE_KEY -R k-forss/remerge

# 3. Export public key
NEW_FP=$(gpg --list-keys --with-colons "remerge release signing" | \
  awk -F: '/^fpr/{print $10; exit}')
gpg --armor --export "$NEW_FP" > keys/release-signing.pub.asc
cp keys/release-signing.pub.asc \
   overlay/sec-keys/openpgp-keys-remerge/files/remerge-release.asc

# 4. Upload to keyserver
gpg --keyserver keys.openpgp.org --send-keys "$NEW_FP"

# 5. Bump the openpgp-keys-remerge ebuild version
cd overlay/sec-keys/openpgp-keys-remerge
# Rename to current date: openpgp-keys-remerge-YYYYMMDD.ebuild
mv openpgp-keys-remerge-*.ebuild \
   "openpgp-keys-remerge-$(date +%Y%m%d).ebuild"
cd -

# 6. Update documentation
#    - README.md: update fingerprint in the verification section
#    - DEVELOPMENT.md: update fingerprint in this table
#    - SECURITY.md: if applicable

# 7. Purge old and new private keys from local keyring
gpg --batch --yes --delete-secret-keys "$NEW_FP"

# 8. Delete the old compromised key from GitHub Secrets
#    (already overwritten in step 2)

# 9. Commit and push
git add keys/ overlay/ README.md DEVELOPMENT.md
git commit -m "chore: rotate release signing key"
git push
```

#### Post-rotation checklist

- [ ] New public key committed to `keys/release-signing.pub.asc`
- [ ] Overlay `files/remerge-release.asc` updated
- [ ] `openpgp-keys-remerge` ebuild version bumped
- [ ] Public key uploaded to `keys.openpgp.org`
- [ ] `GPG_PRIVATE_KEY` secret updated in GitHub
- [ ] Fingerprint updated in README.md verification section
- [ ] Fingerprint updated in this file's key table
- [ ] Private key purged from local keyring
- [ ] Old key not revoked (old releases remain verifiable) unless compromised
- [ ] If compromised: publish revocation certificate for old key

### Security considerations

- **The private key exists only in GitHub Secrets.**  It is never stored on
  disk, in environment variables, or in CI logs.  The deployment command
  pipes it directly: `gpg --export-secret-keys | gh secret set`.
- **No passphrase** is set on the key because CI cannot enter one
  interactively.  The key's security relies entirely on GitHub's secret
  storage and repository access controls.
- **2-year expiry** is a deliberate balance — short enough to force periodic
  review, long enough to avoid churn.  Set a calendar reminder 3 months
  before expiry.
- **Old signatures remain valid** after key extension or rotation — GPG
  verifies signatures using the key state at signing time.

---

## Branch and tag rulesets

Rulesets are defined in `.github/rulesets/` and applied via GitHub's
repository settings.

| Ruleset | Refs | Key rules |
|---------|------|-----------|
| `main.json` | `main` | Require PRs, signed commits, linear history, no force push, no deletion |
| `rc-branches.json` | `rc-*` | Require signed commits, linear history, no force push |
| `tags.json` | `v*` | Only `tag-release.yml` can create, require signed commits, no updates, no deletions |

---

## Overlay maintenance

The Gentoo overlay lives in `overlay/` and provides five packages:

| Package | Type | Description |
|---------|------|-------------|
| `sec-keys/openpgp-keys-remerge` | key | Public key for verify-sig |
| `app-portage/remerge` | source | CLI — builds from crates.io tarball |
| `app-portage/remerge-bin` | binary | CLI — pre-built from GitHub Release |
| `app-portage/remerge-server` | source | Server — builds from crates.io tarball |
| `app-portage/remerge-server-bin` | binary | Server — pre-built from GitHub Release |

### Version bumps

Only `9999` (live) ebuilds are checked into the repository.  The one
exception is `sec-keys/openpgp-keys-remerge`, which uses a date-based
version (e.g. `20260320`) since it ships a static public key file.

Versioned ebuilds for the other packages are generated automatically by the `rc-prepare.yml` workflow when
an `rc-X.Y.Z` branch is pushed:

- **Source ebuilds** are generated from the `9999` templates — the workflow
  replaces `CRATES=" "` with the full list from `Cargo.lock`, removes
  `git-r3` references, and adds `SRC_URI` / `KEYWORDS`.
- **Binary ebuilds** are generated from inline templates in the workflow,
  with `SRC_URI` pointing to the GitHub Release download URLs and
  `verify-sig` support for PGP signature verification.

Do not manually create versioned ebuilds — the RC workflow handles it.

### CRATES variable

The `CRATES` variable in source ebuilds lists all registry dependencies.
It is generated from `Cargo.lock`:

```bash
grep -A1 '^name = ' Cargo.lock | \
  awk -F'"' '/name/{n=$2} /version/{print n"-"$2}' | \
  sort -u | tr '\n' ' '
```

---

## Multi-arch targets

### CLI binaries

| Architecture | Rust target | Release artifact |
|--------------|-------------|-----------------|
| amd64 | `x86_64-unknown-linux-gnu` | ✅ |
| arm64 | `aarch64-unknown-linux-gnu` | ✅ |
| arm (ARMv7) | `armv7-unknown-linux-gnueabihf` | ✅ |
| ppc64 | `powerpc64-unknown-linux-gnu` | ✅ |
| riscv64 | `riscv64gc-unknown-linux-gnu` | ✅ |

### Server binaries

| Architecture | Rust target | Release artifact |
|--------------|-------------|-----------------|
| amd64 | `x86_64-unknown-linux-gnu` | ✅ |
| arm64 | `aarch64-unknown-linux-gnu` | ✅ |

Server binaries are limited to amd64 and arm64 because the server requires
Docker, which has limited support on other architectures.

### Docker images

Multi-arch Docker images are built for `linux/amd64` and `linux/arm64`
using QEMU emulation and `docker buildx`.

---

## Docker images

The server Docker image is published to GHCR:

```
ghcr.io/k-forss/remerge-server:latest
ghcr.io/k-forss/remerge-server:<VERSION>
```

Build locally:

```bash
cd docker
docker compose build
```

Push workflow is in `.github/workflows/docker.yml`.

Worker image layering notes:

- At runtime, the server now builds worker images in two layers: a cached base
  image per system identity plus a thin layer that only injects the current
  `remerge-worker` binary.
- `worker_base_image` seeds the cached base-layer build. Use it for pre-synced
  stage3 images in CI or controlled deployments.
- The cached base layer is disposable and may be rebuilt; it is not part of the
  persistent backup set.
