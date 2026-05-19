#!/usr/bin/env bash
# Install the version-controlled git hooks from .git-hooks/ into .git/hooks/.
#
# Run once after cloning:  scripts/install-hooks.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
HOOK_SRC="$REPO_DIR/.git-hooks"
HOOK_DST="$REPO_DIR/.git/hooks"

for src in "$HOOK_SRC"/*; do
  hook="$(basename "$src")"
  dst="$HOOK_DST/$hook"
  cp "$src" "$dst"
  chmod +x "$dst"
  echo "Installed $hook"
done

echo "Git hooks installed."
