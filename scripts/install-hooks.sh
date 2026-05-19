#!/usr/bin/env bash
# Install the version-controlled git hooks from .git-hooks/ into .git/hooks/.
#
# Run once after cloning:  scripts/install-hooks.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
HOOK_SRC="$REPO_DIR/.git-hooks"
HOOK_DST="$REPO_DIR/.git/hooks"

# Ensure the hooks directory exists (may be absent in a fresh worktree or if
# hooks were manually removed).
mkdir -p "$HOOK_DST"

# nullglob: if .git-hooks/ is empty the glob expands to nothing instead of
# the literal string "$HOOK_SRC/*", preventing a spurious error.
shopt -s nullglob

for src in "$HOOK_SRC"/*; do
  hook="$(basename "$src")"
  dst="$HOOK_DST/$hook"
  cp "$src" "$dst"
  chmod +x "$dst"
  echo "Installed $hook"
done

echo "Git hooks installed."
