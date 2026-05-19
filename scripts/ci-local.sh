#!/usr/bin/env bash
# Run the same checks as CI locally.
#
# Mirrors all four CI jobs from .github/workflows/ci.yml:
#   check (fmt + clippy), test (nextest), fuzz smoke-test, full-stack e2e.
#
# fmt and clippy run in auto-fix mode and loop until both are stable.
# If any files were modified the commit is aborted and the touched files
# are listed so you can review and commit again.
#
# Usage:
#   scripts/ci-local.sh                     # run all steps
#   scripts/ci-local.sh --fmt               # formatting only
#   scripts/ci-local.sh --clippy            # clippy only
#   scripts/ci-local.sh --test              # unit/integration tests only
#   scripts/ci-local.sh --fuzz              # fuzz smoke-test only
#   scripts/ci-local.sh --full-stack        # full-stack e2e only
#
# Environment:
#   SKIP_CI_HOOK=1   bypass entirely when set (useful for WIP commits)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
cd "$REPO_DIR"

# ── Colour helpers ────────────────────────────────────────────────────────────
if [[ -t 1 ]] || [[ -t 2 ]]; then
  BOLD=$'\e[1m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; RED=$'\e[31m'; RESET=$'\e[0m'
else
  BOLD=''; GREEN=''; YELLOW=''; RED=''; RESET=''
fi

step() { echo "${BOLD}:: ${*}${RESET}"; }
ok()   { echo "${GREEN}✔  ${*}${RESET}"; }
warn() { echo "${YELLOW}⚠  ${*}${RESET}"; }
fail() { echo "${RED}✘  ${*}${RESET}" >&2; }

# ── Argument parsing ──────────────────────────────────────────────────────────
RUN_FMT=1; RUN_CLIPPY=1; RUN_TEST=1; RUN_FUZZ=1; RUN_FULL_STACK=1
if [[ $# -gt 0 ]]; then
  RUN_FMT=0; RUN_CLIPPY=0; RUN_TEST=0; RUN_FUZZ=0; RUN_FULL_STACK=0
  for arg in "$@"; do
    case "$arg" in
      --fmt)        RUN_FMT=1 ;;
      --clippy)     RUN_CLIPPY=1 ;;
      --test)       RUN_TEST=1 ;;
      --fuzz)       RUN_FUZZ=1 ;;
      --full-stack) RUN_FULL_STACK=1 ;;
      *) echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
  done
fi

# ── Staged-file snapshot ──────────────────────────────────────────────────────
# Snapshot of working-tree mtimes before any auto-fixes, used to detect
# which files were actually modified across iterations.
snapshot_mtimes() {
  # Print "<mtime> <path>" for every tracked .rs/.toml file.
  git ls-files '*.rs' '*.toml' 2>/dev/null | while IFS= read -r f; do
    [[ -f "$f" ]] && stat -c '%Y %n' "$f"
  done
}

# ── fmt + clippy loop ─────────────────────────────────────────────────────────
# Run fmt then clippy repeatedly until neither makes any changes.
# Clippy --fix can introduce formatting drift, and rustfmt can undo some
# code transformations, so a single pass isn't always enough.
if [[ $RUN_FMT -eq 1 ]] || [[ $RUN_CLIPPY -eq 1 ]]; then
  step "fmt + clippy auto-fix loop"
  BEFORE=$(snapshot_mtimes)
  MAX_ITERS=5
  iter=0

  while true; do
    iter=$((iter + 1))
    changed_this_iter=0

    if [[ $RUN_FMT -eq 1 ]]; then
      SNAP=$(snapshot_mtimes)
      cargo fmt --all
      NEW=$(snapshot_mtimes)
      if [[ "$SNAP" != "$NEW" ]]; then
        changed_this_iter=1
      fi
    fi

    if [[ $RUN_CLIPPY -eq 1 ]]; then
      SNAP=$(snapshot_mtimes)
      # --allow-staged / --allow-dirty: necessary when the index differs from
      # the working tree (common in pre-commit).  -D warnings mirrors CI.
      if ! RUSTFLAGS="-Dwarnings" cargo clippy \
          --workspace --all-targets \
          --fix --allow-staged --allow-dirty \
          -- -D warnings 2>&1; then
        fail "Clippy found errors it could not auto-fix"
        exit 1
      fi
      NEW=$(snapshot_mtimes)
      if [[ "$SNAP" != "$NEW" ]]; then
        changed_this_iter=1
      fi
    fi

    if [[ $changed_this_iter -eq 0 ]]; then
      break
    fi

    if [[ $iter -ge $MAX_ITERS ]]; then
      fail "fmt/clippy did not converge after $MAX_ITERS iterations — manual intervention needed"
      exit 1
    fi
  done

  # Compare final state against the pre-hook snapshot.
  AFTER=$(snapshot_mtimes)
  if [[ "$BEFORE" != "$AFTER" ]]; then
    warn "Auto-fixes were applied. Files modified:"
    # Print only the paths that changed.
    diff <(echo "$BEFORE") <(echo "$AFTER") \
      | grep '^[<>]' | awk '{print $3}' | sort -u \
      | while IFS= read -r f; do warn "  $f"; done
    warn "Review the changes above and commit again."
    exit 1
  fi

  ok "fmt + clippy clean (${iter} iteration(s))"
fi


# ── Tests ─────────────────────────────────────────────────────────────────────
if [[ $RUN_TEST -eq 1 ]]; then
  if cargo nextest --version &>/dev/null 2>&1; then
    step "Tests (cargo nextest --workspace --profile ci)"
    if ! cargo nextest run --workspace --profile ci; then
      fail "Tests failed"
      exit 1
    fi
  else
    warn "cargo-nextest not found — falling back to cargo test"
    step "Tests (cargo test --workspace)"
    if ! cargo test --workspace; then
      fail "Tests failed"
      exit 1
    fi
  fi
  ok "All tests passed"
fi

# ── Fuzz smoke-test ───────────────────────────────────────────────────────────
if [[ $RUN_FUZZ -eq 1 ]]; then
  step "Fuzz smoke-test (15s per target)"

  # Resolve how to invoke cargo-fuzz and rustc.  Prefer a bare `cargo fuzz`
  # (works when cargo-fuzz is installed for the active nightly toolchain).
  # Fall back to `rustup run nightly cargo fuzz` if rustup is present.
  # Skip gracefully if neither is usable.
  FUZZ_CMD=""
  HOST_TARGET=""

  if cargo fuzz --version &>/dev/null 2>&1; then
    FUZZ_CMD="cargo fuzz"
    HOST_TARGET=$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')
  elif command -v rustup &>/dev/null && rustup run nightly cargo fuzz --help &>/dev/null 2>&1; then
    FUZZ_CMD="rustup run nightly cargo fuzz"
    HOST_TARGET=$(rustup run nightly rustc -vV 2>/dev/null | sed -n 's/^host: //p')
  fi

  if [[ -z "$FUZZ_CMD" ]]; then
    warn "cargo-fuzz not found — skipping fuzz smoke-test (install: cargo +nightly install cargo-fuzz)"
  else
    pushd fuzz >/dev/null
    $FUZZ_CMD run make_conf_vars \
      --target "$HOST_TARGET" --sanitizer none -- -max_total_time=15
    $FUZZ_CMD run emerge_arg_filtering \
      --target "$HOST_TARGET" --sanitizer none -- -max_total_time=15
    popd >/dev/null
    ok "Fuzz smoke-test passed"
  fi
fi

# ── Full-stack e2e ────────────────────────────────────────────────────────────
if [[ $RUN_FULL_STACK -eq 1 ]]; then
  step "Full-stack e2e (cargo test --features integration,e2e)"
  if ! command -v docker &>/dev/null; then
    fail "docker not found — full-stack tests require Docker (matches CI behaviour)"
    exit 1
  elif ! docker info &>/dev/null 2>&1; then
    fail "Docker daemon not running — start Docker and retry"
    exit 1
  else
    step "  Building worker binary"
    cargo build -p remerge-worker
    if ! cargo test --workspace --features integration,e2e; then
      fail "Full-stack tests failed"
      exit 1
    fi
    ok "Full-stack tests passed"
  fi
fi

# ── Done ─────────────────────────────────────────────────────────────────────
echo ""
ok "All checks passed"
