# remerge Gentoo Overlay

This overlay provides Gentoo ebuilds for the remerge distributed binary
package builder.

## Packages

| Package | Description |
|---------|-------------|
| `app-portage/remerge` | CLI — build from source |
| `app-portage/remerge-bin` | CLI — pre-built binary (PGP verified) ¹ |
| `app-portage/remerge-server` | Server — build from source (with OpenRC/systemd) |
| `app-portage/remerge-server-bin` | Server — pre-built binary (PGP verified, with OpenRC/systemd) ¹ |
| `sec-keys/openpgp-keys-remerge` | PGP public key for verifying release signatures |

¹ Binary (`-bin`) ebuilds are only available after a release has been
published on GitHub.  Use the source ebuilds or `9999` live ebuilds
before the first release.

## Quick start

### Option 1: repos.conf with git sparse checkout (recommended)

The overlay lives in the `overlay/` subdirectory of the repo, so a
standard `sync-type = git` won't work directly.  Clone with a sparse
checkout and point portage at the `overlay/` directory:

```sh
# Clone with sparse checkout (only the overlay directory)
git clone --depth 1 --filter=blob:none --sparse \
  https://github.com/k-forss/remerge.git /var/db/repos/remerge-src
cd /var/db/repos/remerge-src && git sparse-checkout set overlay

# Register the overlay subdirectory
cat > /etc/portage/repos.conf/remerge.conf <<'EOF'
[remerge]
location = /var/db/repos/remerge-src/overlay
auto-sync = no
EOF

# Install
emerge app-portage/remerge
```

To update: `cd /var/db/repos/remerge-src && git pull`

### Option 2: Local clone

If you already have the repo cloned (e.g. for development):

```sh
cat > /etc/portage/repos.conf/remerge.conf <<'EOF'
[remerge]
location = /path/to/remerge/overlay
auto-sync = no
EOF

emerge app-portage/remerge
```

## Signature verification

The binary ebuilds (`remerge-bin`, `remerge-server-bin`) support the
`verify-sig` USE flag, which enables automatic PGP signature verification
of downloaded release artifacts.

```sh
# Enable signature verification globally or per-package
echo "app-portage/remerge-bin verify-sig" >> /etc/portage/package.use/remerge
echo "app-portage/remerge-server-bin verify-sig" >> /etc/portage/package.use/remerge

# The openpgp-keys-remerge package is pulled in automatically as a build dep
emerge app-portage/remerge-bin
```

Signing key fingerprint:
```
C075 B1EF DC2E 4D23 817A  1BB3 F5B0 BB05 FABD 6151
```

## Server installation

The server ebuilds include OpenRC and systemd service files:

```sh
# Install
emerge app-portage/remerge-server

# OpenRC
rc-service remerge-server start
rc-update add remerge-server default

# systemd
systemctl enable --now remerge-server
```

Configure in `/etc/remerge/server.toml`.

## Live ebuilds

To track the latest main branch:

```sh
echo "=app-portage/remerge-9999 **" >> /etc/portage/package.accept_keywords/remerge
echo "=app-portage/remerge-server-9999 **" >> /etc/portage/package.accept_keywords/remerge
emerge =app-portage/remerge-9999
```
