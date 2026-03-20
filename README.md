# remerge

[![CI](https://github.com/k-forss/remerge/actions/workflows/ci.yml/badge.svg)](https://github.com/k-forss/remerge/actions/workflows/ci.yml)
[![Security audit](https://github.com/k-forss/remerge/actions/workflows/audit.yml/badge.svg)](https://github.com/k-forss/remerge/actions/workflows/audit.yml)
[![License: GPL-2.0](https://img.shields.io/badge/License-GPL--2.0-blue.svg)](LICENSE)

Distributed binary package builder for Gentoo Linux.

**remerge** is a drop-in wrapper for `emerge` that offloads package compilation
to a remote build server running Docker workers, then installs the resulting
binary packages locally via portage's native binhost support.

## Architecture

```
┌──────────────────┐         HTTP / WS         ┌──────────────────────┐
│   Gentoo Host    │ ◄──────────────────────► │    remerge-server     │
│                  │                           │  (main container)     │
│  $ remerge       │   1. submit workorder     │                      │
│    dev-libs/foo  │   2. stream progress      │  ┌─────────────────┐ │
│                  │   3. get binpkgs          │  │ Docker API       │ │
│  reads:          │                           │  │                  │ │
│  - make.conf     │                           │  │ spins up workers │ │
│  - package.use   │                           │  └────────┬────────┘ │
│  - profile       │                           │           │          │
│  - gcc version   │                           │  ┌────────▼────────┐ │
│  - ...           │                           │  │ remerge-worker  │ │
│                  │                           │  │ (Docker ctnr)   │ │
│  then runs:      │   binpkgs via HTTP        │  │                 │ │
│  emerge          │ ◄─────────────────────── │  │ emerge          │ │
│    --getbinpkg   │                           │  │   --buildpkg    │ │
│    --usepkg      │                           │  │                 │ │
└──────────────────┘                           │  └─────────────────┘ │
                                               │                      │
                                               │  /var/cache/binpkgs  │
                                               │  (served via HTTP)   │
                                               └──────────────────────┘
```

## How it works

1. **User runs `remerge <emerge-args>`** on their Gentoo machine.
2. The CLI reads `/etc/portage/make.conf`, `package.use`, `package.accept_keywords`,
   active profile, GCC version, glibc version, etc.
3. A **workorder** is assembled and sent to the remerge server via HTTP.
4. The server **provisions a worker container** matching the requester's toolchain
   (arch, profile, GCC version). Images are built on-demand and cached.
5. Inside the worker, the requester's portage config is applied and
   `emerge --buildpkg` builds the requested packages.
6. **Build progress** is streamed back to the CLI over WebSocket in real-time.
7. Once complete, the binary packages are available in the server's HTTP-served
   binpkg repository.
8. The CLI runs `emerge --getbinpkg --usepkg <original-args>` locally, which
   fetches the pre-built packages from the server and installs them — only
   building from source for packages that couldn't be pre-built.

## Installation

### From GitHub release

Download pre-built binaries from the
[latest release](https://github.com/k-forss/remerge/releases/latest).
Builds are available for amd64, arm64, arm (ARMv7), riscv64, and ppc64:

```bash
# Download and extract (amd64 example)
# Replace <VERSION> with the desired release tag (e.g. v0.2.0).
curl -sL https://github.com/k-forss/remerge/releases/latest/download/remerge-amd64-linux.tar.gz | tar xz
sudo install -m 0755 remerge /usr/local/bin/remerge

# Server binaries are available for amd64 and arm64:
curl -sL https://github.com/k-forss/remerge/releases/latest/download/remerge-server-amd64-linux.tar.gz | tar xz
sudo install -m 0755 remerge-server /usr/local/bin/remerge-server
```

### Via Gentoo overlay

The repository includes a full Gentoo overlay with source and binary ebuilds:

```bash
# Using eselect-repository:
eselect repository add remerge git https://github.com/k-forss/remerge.git
emaint sync -r remerge

# Build from source (live/9999 ebuild — always available):
emerge app-portage/remerge

# Or install a pre-built binary (requires a published release):
emerge app-portage/remerge-bin

# Install server with OpenRC/systemd service files:
emerge app-portage/remerge-server
```

See [`overlay/README.md`](overlay/README.md) for full overlay setup,
alternative installation methods, and signature verification details.

### Build from source

```bash
cargo build --release
```

This produces three binaries in `target/release/`:
- `remerge` — install on Gentoo hosts
- `remerge-server` — run on your build server
- `remerge-worker` — bundled into Docker images automatically

```bash
sudo install -m 0755 target/release/remerge /usr/local/bin/remerge
```

## Quick start

### Start the server

```bash
# Using docker compose:
cd docker
docker compose up -d

# Or run directly:
remerge-server --listen 0.0.0.0:7654 --config config/server.example.toml
```

### Use it

```bash
# Instead of:
emerge -avuDU @world

# Run:
remerge -avuDU @world

# Or explicitly set the server:
remerge --server http://build-server:7654 dev-libs/openssl

# Dry-run to see what would happen:
remerge --dry-run dev-libs/openssl
```

### Optional: alias emerge

```bash
# Add to your shell rc:
alias emerge='remerge'
```

## Configuration

### CLI

| Env var | Flag | Default | Description |
|---------|------|---------|-------------|
| `REMERGE_SERVER` | `--server` | `http://localhost:7654` | Server URL |
| `REMERGE_CLIENT_ID` | `--client-id` | auto-generated | UUID client identity |
| `REMERGE_ROLE` | `--role` | `main` | Client role (`main` or `follower`) |
| — | `--config` | `/etc/remerge.conf` | Path to CLI config file |
| — | `--submit-only` | false | Submit workorder without waiting |
| — | `--no-local` | false | Don't run emerge locally after build |
| — | `--dry-run` | false | Print what would happen |
| — | `--force` | false | Force rebuild even if already installed |

### Server

See [config/server.example.toml](config/server.example.toml) for all options.

All settings can be overridden with `REMERGE_*` environment variables:

| Env var | Config key | Default | Description |
|---------|-----------|---------|-------------|
| `REMERGE_BINPKG_DIR` | `binpkg_dir` | `/var/cache/remerge/binpkgs` | Package storage directory |
| `REMERGE_BINHOST_URL` | `binhost_url` | `http://localhost:7654/binpkgs` | Public binhost URL |
| `REMERGE_DOCKER_SOCKET` | `docker_socket` | `unix:///var/run/docker.sock` | Docker socket |
| `REMERGE_MAX_WORKERS` | `max_workers` | `4` | Max concurrent workers |
| `REMERGE_WORKER_IDLE_TIMEOUT` | `worker_idle_timeout` | `3600` | Idle image timeout (seconds) |
| `REMERGE_PARALLEL_JOBS` | `parallel_jobs` | auto (CPU count) | emerge `-j` flag |
| `REMERGE_LOAD_AVERAGE` | `load_average` | auto (CPU count) | emerge `-l` flag |
| `REMERGE_STATE_DIR` | `state_dir` | `/var/lib/remerge` | Persistent state directory |
| `REMERGE_RETENTION_HOURS` | `retention_hours` | `24` | TTL for completed workorders |
| `REMERGE_MAX_RETAINED_WORKORDERS` | `max_retained_workorders` | `1000` | Max entries cap |
| `REMERGE_LOG_JSON` | `log_json` | `false` | JSON structured log output |
| `REMERGE_BINPKG_DISK_WARN_BYTES` | `binpkg_disk_warn_bytes` | 10 GiB | Disk usage warning threshold |
| `REMERGE_WORKER_BINARY` | `worker_binary` | — | Path to worker binary for injection |
| `REMERGE_AUTH_MODE` | `auth.mode` | `none` | Auth mode: `none`, `mtls`, `mixed` |
| `REMERGE_GPG_KEY` | `signing.gpg_key` | — | GPG key for binpkg signing |
| `REMERGE_GPG_HOME` | `signing.gpg_home` | — | GPG keyring directory |

## API

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/info` | Server info and stats |
| `GET` | `/api/v1/health` | Health / readiness probe |
| `POST` | `/api/v1/workorders` | Submit a new workorder |
| `GET` | `/api/v1/workorders` | List workorders (scoped to client in auth modes) |
| `GET` | `/api/v1/workorders/{id}` | Get workorder status (auth-scoped) |
| `DELETE` | `/api/v1/workorders/{id}` | Cancel a workorder (auth-scoped) |
| `WS` | `/api/v1/workorders/{id}/progress` | Stream build progress |
| `GET` | `/api/v1/clients` | List registered clients |
| `GET` | `/api/v1/clients/{id}` | Get client details |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/binpkgs/...` | Binary package repository (portage binhost) |

## Authentication

remerge supports three authentication modes:

| Mode | Description |
|------|-------------|
| `none` (default) | Clients self-identify via `client_id` in the request body. |
| `mtls` | All clients must present a valid certificate (via reverse proxy header). |
| `mixed` | Main clients require mTLS; followers may self-identify. |

See [`config/server.example.toml`](config/server.example.toml) for configuration.

## Binary package signing

remerge supports OpenPGP signing of binary packages via portage's native
`binpkg-signing` feature.  When configured, the server mounts a GPG keyring
into each worker container and instructs portage to sign all generated `.gpkg`
packages.

### Server setup

1. Create (or use an existing) GPG key for signing.

2. Add to your server configuration:

```toml
[signing]
gpg_key = "0x1234567890ABCDEF"
gpg_home = "/var/cache/remerge/gnupg"
```

Or via environment variables:

```bash
export REMERGE_GPG_KEY=0x1234567890ABCDEF
export REMERGE_GPG_HOME=/var/cache/remerge/gnupg
```

### Client verification

Clients can verify signed packages by importing the signing key into
portage's verification keyring.  See the
[Gentoo Binary Package Guide](https://wiki.gentoo.org/wiki/Binary_package_guide#Verify_binary_package_OpenPGP_signatures)
for details.

## Verifying releases

Every GitHub Release goes through a multi-layer verification pipeline:

| Layer | What it proves | How to verify |
|-------|---------------|---------------|
| **SHA256 checksums** | File integrity | `sha256sum -c remerge-vX.Y.Z-SHA256SUMS.txt` |
| **PGP signatures** | Signed by maintainer | `gpg --verify <file>.asc <file>` |
| **SLSA attestation** | Built by this repo's CI | `gh attestation verify <file> -o k-forss` |
| **VirusTotal scan** | No known malware | Links in release notes |
| **RELEASE.md.asc** | Final release body sealed | `gpg --verify RELEASE.md.asc` |

### Verify PGP signatures

```bash
# Import the release signing key
gpg --keyserver keys.openpgp.org --recv-keys C075B1EFDC2E4D23817A1BB3F5B0BB05FABD6151

# Verify an artifact (replace <VERSION> and <ARCH> with actual values)
gpg --verify remerge-<VERSION>-<ARCH>-linux.tar.gz.asc remerge-<VERSION>-<ARCH>-linux.tar.gz

# Verify the release body seal (covers checksums + VirusTotal links)
gpg --verify RELEASE.md.asc
```

The public key is also available in [`keys/release-signing.pub.asc`](keys/release-signing.pub.asc)
and installed by the `sec-keys/openpgp-keys-remerge` Gentoo package.

### Verify build provenance (SLSA)

```bash
# Requires the GitHub CLI
gh attestation verify remerge-<VERSION>-amd64-linux.tar.gz -o k-forss
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

For CI/CD pipeline details, release signing internals, and maintainer
procedures, see [DEVELOPMENT.md](DEVELOPMENT.md).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md).

## License

GPL-2.0 — see [LICENSE](LICENSE).
