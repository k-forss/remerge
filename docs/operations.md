# Remerge Operations Guide

Operator-focused deployment and maintenance guide for production or beta
deployments. This complements [README.md](../README.md), which covers product
overview and installation, and [DEVELOPMENT.md](../DEVELOPMENT.md), which covers
maintainer and release workflows.

## Scope

This guide covers:

- production deployment prerequisites
- Docker Compose and bare-binary deployments
- reverse-proxy and TLS placement
- first-client validation
- backup and restore procedures
- upgrade and rollback runbooks
- monitoring, alerting, and dashboard targets

The examples assume Linux, Docker, and a reverse proxy in front of
`remerge-server` for public deployments.

## Prerequisites

Before deploying `remerge-server`, ensure all of the following exist:

- a host with Docker access to `/var/run/docker.sock`
- persistent storage for:
  - binpkg repository data
  - server state
  - optional signing keyring
  - optional synced Portage repository mirror
- a reachable public or internal binhost URL for clients
- the `remerge-worker` binary available on the server host
- a reverse proxy if you need TLS termination, mTLS, header stripping, or rate
  limits

Critical paths to plan around:

- `binpkg_dir`: binary package repository data
- `state_dir`: persisted workorders, results, and client registry
- `signing.gpg_home`: signing keyring, if enabled
- `repos_dir`: local ebuild tree mirror, if configured

Snapshot compression behavior:

- snapshot blob identity is always the raw SHA256 digest of the uncompressed payload
- the CLI may upload snapshot blobs over the websocket stream using zstd when that reduces transfer size enough to be worthwhile
- the server keeps the raw blob as the source of truth and may retain zstd sidecars for blobs and tree manifests to reduce later transfer cost
- HTTP blob downloads may return `Content-Encoding: zstd`; supported clients decode this transparently

Snapshot retention behavior:

- snapshot blob/tree retention is global-only; there are no differentiated retention classes
- unreferenced snapshot data stays warm-cache reusable for 7 days by default
- unreferenced snapshot data older than 30 days becomes the oldest eviction tier, but the retained-size floor still prevents deletion below the configured minimum, even for hard-delete-eligible entries
- age-based cleanup across both grace-eligible and hard-delete-eligible data only runs when cached snapshot data exceeds the global minimum retained-size floor, which defaults to 10 GiB
- operators can tune these defaults with `snapshot_cache_grace_period_hours`, `snapshot_cache_hard_delete_hours`, and `snapshot_min_retained_bytes` or the matching `REMERGE_*` environment variables
- when cleanup is needed, the server reclaims the oldest eligible unreferenced entries first until retained snapshot data is back at or below the floor
- cleanup runs asynchronously after workorder runtime teardown so builds are not blocked on deletions
- operators can force a one-shot cleanup pass without starting the API server by running `remerge-server --config /path/to/server.toml --cleanup-now`
- extending retention is purely configuration-driven: raise `snapshot_cache_grace_period_hours`, `snapshot_cache_hard_delete_hours`, or `snapshot_min_retained_bytes` and restart the server

Client identity note:

- The CLI persists `client_id` in `/etc/remerge.conf` on first run when the
  config path is writable.
- If you intentionally share one identity across machines, keep one `main`
  client and mark the others `follower`.

## Deployment architecture

Recommended production shape:

1. reverse proxy terminates TLS and optionally enforces mTLS
2. reverse proxy strips any inbound certificate fingerprint header and injects
   the configured one only after successful certificate verification
3. `remerge-server` listens on a private interface or container port
4. `remerge-server` talks to the local Docker daemon to start worker
   containers
5. clients submit workorders over HTTP/WebSocket and download binpkgs from the
   published `binhost_url`

Worker image layering:

- `remerge-server` builds worker images in two layers: a cached base image per
  system identity and a thin runtime layer that injects the current
  `remerge-worker` binary.
- If you set `worker_base_image`, that image becomes the root of the cached
  base-layer build and is the preferred way to provide a pre-synced stage3.

If `auth.mode` is `mixed`, keep `/metrics` on a protected proxy route and allow
`/binpkgs` on the public route. If `auth.mode` is `mtls`, protect both.

## Deployment path A: Docker Compose

The repository ships an example compose file at
[docker/docker-compose.yml](../docker/docker-compose.yml). Use it as a base,
not as a drop-in internet-facing deployment.

### Host preparation

1. Install Docker Engine and Compose v2.
2. Install `remerge-worker` on the host, for example at
   `/usr/local/bin/remerge-worker`.
3. Create persistent host directories if you prefer bind mounts over named
   volumes.
4. If signing is enabled, prepare a dedicated GPG home with the secret key.
5. If `repos_dir` is enabled, sync the ebuild tree outside the container with a
   timer or cron job.

### Compose deployment steps

1. Copy [docker/docker-compose.yml](../docker/docker-compose.yml) to your
   deployment host.
2. Set `REMERGE_BINHOST_URL` to the external binhost URL clients will use.
3. Set `REMERGE_WORKER_BINARY` to the path mounted inside the server container.
4. Mount the host worker binary read-only into the server container.
5. Mount the Docker socket, binpkg storage, and state storage.
6. If signing is enabled, mount the GPG home read-only and set both
   `REMERGE_GPG_KEY` and `REMERGE_GPG_HOME`.
7. If using a local ebuild mirror, mount it read-only and set
   `REMERGE_REPOS_DIR`.
8. Start the service with `docker compose up -d`.
9. Put a reverse proxy in front of the server before exposing it publicly.

### Compose validation

Run these checks after startup:

```bash
docker compose ps
docker compose logs --tail=200 server
curl -fsS http://127.0.0.1:7654/api/v1/health
curl -fsS http://127.0.0.1:7654/api/v1/info
```

If signing is enabled:

```bash
curl -fsS http://127.0.0.1:7654/api/v1/signing-key | head -5
```

If signing is configured incorrectly or the worker binary is missing, startup
should fail before the service begins accepting workorders.

## Deployment path B: bare binary + system service

Use this path when you want the server installed directly on the host while
still using Docker for worker execution.

### Host preparation

1. Install `remerge-server` and `remerge-worker` on the host.
2. Place the server config at `/etc/remerge/server.toml` or another managed
   path.
3. Create the directories referenced by:
   - `binpkg_dir`
   - `state_dir`
   - optional `signing.gpg_home`
   - optional `repos_dir`
4. Ensure the service account can read the worker binary, config, keyring, and
   Docker socket.
5. Put a reverse proxy in front of the server for public traffic.

### Example service unit

```ini
[Unit]
Description=remerge build coordinator
After=network-online.target docker.service
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/remerge-server --config /etc/remerge/server.toml --listen 127.0.0.1:7654
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=remerge_server=info,tower_http=info

[Install]
WantedBy=multi-user.target
```

### Bare-binary validation

```bash
systemctl daemon-reload
systemctl enable --now remerge-server
systemctl status remerge-server
journalctl -u remerge-server -n 200 --no-pager
curl -fsS http://127.0.0.1:7654/api/v1/health
```

## Reverse proxy requirements

For any public deployment:

- terminate TLS at the proxy or use end-to-end TLS with the proxy still
  stripping and re-injecting the fingerprint header
- strip inbound `X-Client-Cert-Fingerprint` headers from untrusted clients
- inject the configured fingerprint header only after successful client
  certificate validation
- protect `/metrics` from public scraping
- apply rate limits to:
  - `POST /api/v1/workorders`
  - `GET /metrics`
  - `GET /binpkgs/...`

Recommended policy:

- low sustained submission rate with a small burst per source IP or client ID
- tight metrics scrape allowlist and low burst
- broader binpkg download allowance with per-IP limits to reduce scraping and
  bandwidth abuse

remerge does not implement a server-side package-category allowlist. Treat
authentication, proxy controls, and rate limits as the submission policy layer.

## First-client validation

After the server is up, validate the full path from a Gentoo client.

### Client setup

1. Install `remerge` on the Gentoo host.
2. Point the CLI at the server:

```bash
export REMERGE_SERVER=https://remerge.example.com
```

3. If signing is enabled, fetch and import the public signing key before using
  the binhost:

```bash
curl -fsS https://remerge.example.com/api/v1/signing-key \
  -o /etc/portage/gnupg/remerge-binpkg.asc
gpg --homedir /etc/portage/gnupg --import /etc/portage/gnupg/remerge-binpkg.asc
curl -fsS https://remerge.example.com/api/v1/info
```

  Treat binpkg verification as required whenever signing is enabled.
4. If using `auth.mode=mtls` or `mixed`, configure the reverse proxy and client
   certificate path before expecting main-client submissions to succeed.

### Dry-run validation sequence

```bash
remerge --dry-run dev-libs/openssl
remerge --submit-only dev-libs/openssl
remerge dev-libs/openssl
```

Expected outcome:

- the dry run prints the planned submission
- the submit-only path returns a workorder ID and progress URL
- the full run submits, waits, downloads the binpkg, and installs it locally

## Backup procedure

Keep the backup backend generic, but always back up the same logical assets.

### What must be backed up

- `binpkg_dir`: required if you want to retain built packages without rebuilds
- `state_dir`: required for workorder history, results, and client registry
- `signing.gpg_home`: required if signing is enabled and the secret key is not
  recoverable elsewhere
- deployment config:
  - `/etc/remerge/server.toml` or equivalent
  - reverse-proxy config
  - compose file or service unit

### What may be rebuilt instead of backed up

- worker images in Docker cache
- cached worker base layers and thin runtime layers in Docker cache
- temporary workorder runtime channels and in-memory queue state
- `repos_dir` if you can resync it from upstream sources

### Safe backup workflow

1. Record current version and config hash if you maintain one.
2. Stop new submissions at the proxy or place the service in maintenance mode.
3. Wait for active builds to finish, or record that they will be lost.
4. Snapshot or copy:
   - `binpkg_dir`
   - `state_dir`
   - optional `signing.gpg_home`
   - deployment config files
5. Resume traffic.

### Backup validation commands

Run these on the backup copy or staging restore target:

```bash
test -d /backup/remerge/binpkgs
test -d /backup/remerge/state
find /backup/remerge/binpkgs -maxdepth 2 | head
find /backup/remerge/state -maxdepth 2 | head
```

If signing is enabled:

```bash
gpg --homedir /backup/remerge/gnupg --list-secret-keys
```

## Restore procedure

### Restore steps

1. Install the target `remerge-server` and `remerge-worker` version.
2. Restore config files before starting the service.
3. Restore `binpkg_dir` and `state_dir` to their configured paths.
4. Restore `signing.gpg_home` if signing is enabled.
5. Restore reverse-proxy config.
6. Start the service.

### Restore validation

```bash
curl -fsS http://127.0.0.1:7654/api/v1/health
curl -fsS http://127.0.0.1:7654/api/v1/info
curl -fsS http://127.0.0.1:7654/metrics | grep remerge_
```

If signing is enabled:

```bash
curl -fsS http://127.0.0.1:7654/api/v1/signing-key | head -5
```

Then submit one low-risk package build from a test client and confirm the
result downloads successfully.

## Upgrade runbook

Always treat upgrades as stateful operations.

### Pre-upgrade checklist

1. Read the release notes for config, state, or client compatibility changes.
2. Back up `state_dir`, `binpkg_dir`, and optional `signing.gpg_home`.
3. Record the current version and the cached worker-image tags you expect to be rebuilt.
4. Drain or finish active builds before replacing the server.

### Upgrade steps: Docker Compose

1. Pull the new image or update the pinned tag.
2. Confirm the worker binary mount is still valid.
3. Restart the service with `docker compose up -d`.
4. Watch startup logs for worker-binary and signing validation.
5. Submit a low-risk validation build.

### Upgrade steps: bare binary

1. Install the new `remerge-server` binary.
2. Install the matching `remerge-worker` binary.
3. Reload and restart the service.
4. Inspect logs for migration, signing, or startup validation failures.
5. Submit a low-risk validation build.

## Rollback runbook

Rollback assumes full state restore, not just binary replacement.

### Rollback trigger conditions

- startup validation fails after upgrade
- clients cannot submit or fetch binpkgs successfully
- state appears unreadable or semantically incorrect
- the release notes or observed behavior indicate a state compatibility problem

### Rollback steps

1. Stop the upgraded service.
2. Restore the previous `remerge-server` and `remerge-worker` version.
3. Restore the pre-upgrade backup of:
   - `state_dir`
   - `binpkg_dir` if the upgrade changed or removed repository contents
   - optional `signing.gpg_home`
   - deployment config
4. Start the previous version.
5. Validate with health, info, metrics, and one low-risk client build.

### Rollback validation commands

```bash
curl -fsS http://127.0.0.1:7654/api/v1/health
curl -fsS http://127.0.0.1:7654/api/v1/info
curl -fsS http://127.0.0.1:7654/metrics | grep -E 'remerge_(workorders|builds|queue|binpkg)'
```

## Monitoring and alerting

`remerge-server` currently exports these Prometheus metrics:

- `remerge_workorders_submitted_total`
- `remerge_workorders_completed_total`
- `remerge_workorders_failed_total`
- `remerge_workorders_cancelled_total`
- `remerge_builds_active`
- `remerge_builds_duration_seconds_total`
- `remerge_queue_depth`
- `remerge_binpkg_disk_usage_bytes`
- `remerge_worker_image_builds_total`
- `remerge_worker_image_build_duration_seconds_total`
- `remerge_worker_container_starts_total`
- `remerge_worker_container_startup_duration_seconds_total`
- `remerge_cleanup_success_total`
- `remerge_cleanup_failure_total`
- `remerge_package_builds_total`
- `remerge_package_build_duration_seconds_total`
- `remerge_package_builds_by_atom_total{atom=...}`
- `remerge_package_build_duration_seconds_by_atom_total{atom=...}`

### Suggested Grafana dashboard panels

- submitted, completed, failed, and cancelled workorder rates
- active builds gauge
- queue depth over time
- cumulative build duration growth rate
- binpkg disk usage by instance
- worker image build rate and cumulative build duration
- worker container startup latency and failure correlation
- cleanup success versus cleanup failure totals
- hottest package atoms by build count and cumulative build duration

### Suggested Prometheus alert rules

```yaml
groups:
  - name: remerge
    rules:
      - alert: RemergeQueueDepthHigh
        expr: remerge_queue_depth > 20
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: remerge queue depth is elevated

      - alert: RemergeBuildFailuresSpiking
        expr: rate(remerge_workorders_failed_total[15m]) > 0.1
        for: 15m
        labels:
          severity: warning
        annotations:
          summary: remerge build failures are spiking

      - alert: RemergeWorkersStuckBusy
        expr: remerge_builds_active > 0 and increase(remerge_builds_duration_seconds_total[30m]) == 0
        for: 30m
        labels:
          severity: critical
        annotations:
          summary: remerge reports active builds but build duration is not moving

      - alert: RemergeBinpkgDiskUsageHigh
        expr: remerge_binpkg_disk_usage_bytes > 8 * 1024 * 1024 * 1024
        for: 30m
        labels:
          severity: warning
        annotations:
          summary: remerge binpkg storage is nearing capacity

      - alert: RemergeCleanupFailures
        expr: increase(remerge_cleanup_failure_total[15m]) > 0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: remerge is failing to clean up worker runtime state

      - alert: RemergeWorkerStartupLatencyHigh
        expr: increase(remerge_worker_container_startup_duration_seconds_total[15m]) / clamp_min(increase(remerge_worker_container_starts_total[15m]), 1) > 60
        for: 15m
        labels:
          severity: warning
        annotations:
          summary: remerge worker container startup latency is elevated
```

Tune thresholds to your worker count, retention policy, and disk budget. If
you set `binpkg_disk_warn_bytes`, align the alert threshold with that limit.

## Operational review cadence

Recommended recurring checks:

- weekly: failed build rate, queue depth trends, disk usage growth
- monthly: backup restore spot check on a staging host
- before each upgrade: backup verification and rollback rehearsal
- before public exposure changes: reverse-proxy header stripping and rate-limit
  review