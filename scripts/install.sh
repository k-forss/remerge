#!/usr/bin/env bash
set -euo pipefail

# Install the remerge CLI on a Gentoo system.
#
# This script builds remerge from source and installs it to /usr/local/bin.
# It also sets up a symlink so that `remerge` can optionally replace `emerge`.
#
# Requirements: Rust toolchain (rustup), git

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

cd "$REPO_DIR"

echo "Building remerge CLI…"
cargo build --release --bin remerge

echo "Installing to /usr/local/bin…"
sudo install -m 0755 target/release/remerge /usr/local/bin/remerge

echo ""
echo "remerge installed successfully!"
echo ""
echo "Usage:"
echo "  remerge --server http://your-server:7654 dev-libs/openssl"
echo ""
echo "Or set the server permanently:"
echo "  export REMERGE_SERVER=http://your-server:7654"
echo ""
echo "Optional: to make remerge the default for emerge, add to your shell rc:"
echo "  alias emerge='remerge'"
