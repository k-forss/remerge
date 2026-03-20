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

### Option 1: eselect-repository (recommended)

If you have `app-eselect/eselect-repository` installed:

```sh
# Add the overlay
eselect repository add remerge git https://github.com/k-forss/remerge.git

# Sync
emerge --sync remerge

# Install the CLI (build from source — always available)
emerge app-portage/remerge

# Or install a pre-built binary (requires a published release)
emerge app-portage/remerge-bin
```

### Option 2: repos.conf (manual)

Create `/etc/portage/repos.conf/remerge.conf`:

```ini
[remerge]
location = /var/db/repos/remerge
sync-type = git
sync-uri = https://github.com/k-forss/remerge.git
sync-depth = 1
auto-sync = yes
```

Then sync and install:

```sh
emerge --sync remerge
emerge app-portage/remerge
```

### Option 3: Local overlay

```sh
# Clone into your repos directory
git clone https://github.com/k-forss/remerge.git /var/db/repos/remerge

# Register it
cat > /etc/portage/repos.conf/remerge.conf <<EOF
[remerge]
location = /var/db/repos/remerge
EOF

# Install
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
