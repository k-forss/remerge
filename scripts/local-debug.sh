#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

PROFILE="${REMERGE_LOCAL_DEBUG_PROFILE:-debug}"
SESSION_DIR="${REMERGE_LOCAL_DEBUG_DIR:-$REPO_DIR/.tmp/remerge-local-debug}"
LISTEN_HOST="${REMERGE_LOCAL_DEBUG_HOST:-127.0.0.1}"
LISTEN_PORT="${REMERGE_LOCAL_DEBUG_PORT:-17654}"
SERVER_URL="http://${LISTEN_HOST}:${LISTEN_PORT}"
RUST_LOG_VALUE="${REMERGE_LOCAL_DEBUG_RUST_LOG:-remerge=trace,remerge_server=trace,remerge_cli=trace,remerge_worker=trace,reqwest=debug,tower_http=debug,hyper=info}"
BUILD_TIMEOUT_SECS="${REMERGE_LOCAL_DEBUG_BUILD_TIMEOUT_SECS:-3600}"
WORKER_IDLE_TIMEOUT="${REMERGE_LOCAL_DEBUG_WORKER_IDLE_TIMEOUT:-600}"
MAX_WORKERS="${REMERGE_LOCAL_DEBUG_MAX_WORKERS:-1}"
MAX_ACTIVE_WORKORDERS="${REMERGE_LOCAL_DEBUG_MAX_ACTIVE_WORKORDERS:-8}"
WORKER_NETWORK_MODE="${REMERGE_LOCAL_DEBUG_WORKER_NETWORK_MODE:-bridge}"

normalize_bool() {
    local value="${1:-false}"
    case "${value,,}" in
        1|true|yes|on)
            printf 'true\n'
            ;;
        0|false|no|off|'')
            printf 'false\n'
            ;;
        *)
            echo "Invalid boolean value '$value' (expected true/false, 1/0, yes/no, or on/off)" >&2
            exit 1
            ;;
    esac
}

ALLOW_REMOTE_BIND="$(normalize_bool "${REMERGE_LOCAL_DEBUG_ALLOW_REMOTE:-false}")"
SKIP_WORKER_SYNC="$(normalize_bool "${REMERGE_LOCAL_DEBUG_SKIP_WORKER_SYNC:-false}")"

is_loopback_host() {
    case "$1" in
        127.*|::1|localhost)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

ensure_safe_listen_host() {
    if is_loopback_host "$LISTEN_HOST"; then
        return
    fi

    if [[ "$ALLOW_REMOTE_BIND" == "true" ]]; then
        echo "WARNING: starting unauthenticated local debug server on non-loopback host '$LISTEN_HOST' because REMERGE_LOCAL_DEBUG_ALLOW_REMOTE=$ALLOW_REMOTE_BIND" >&2
        return
    fi

    cat >&2 <<EOF
Refusing to bind local debug server to non-loopback host '$LISTEN_HOST'.
scripts/local-debug.sh writes [auth] mode = "none" to $SERVER_CONFIG, so a non-local bind would expose an unauthenticated server.
Keep REMERGE_LOCAL_DEBUG_HOST on localhost/127.0.0.1/::1, or set REMERGE_LOCAL_DEBUG_ALLOW_REMOTE=true if you intentionally want a remote bind.
EOF
    exit 1
}

case "$PROFILE" in
    debug) TARGET_DIR="$REPO_DIR/target/debug" ;;
    release) TARGET_DIR="$REPO_DIR/target/release" ;;
    *)
        echo "Unsupported REMERGE_LOCAL_DEBUG_PROFILE='$PROFILE' (expected 'debug' or 'release')" >&2
        exit 1
        ;;
esac

SERVER_BIN="$TARGET_DIR/remerge-server"
CLI_BIN="$TARGET_DIR/remerge"
WORKER_BIN="$TARGET_DIR/remerge-worker"

STATE_DIR="$SESSION_DIR/state"
BINPKG_DIR="$SESSION_DIR/binpkgs"
DISTDIR_DIR="${REMERGE_LOCAL_DEBUG_DISTDIR:-$SESSION_DIR/distfiles}"
LOG_DIR="$SESSION_DIR/logs"
SERVER_CONFIG="$SESSION_DIR/server.toml"
CLI_CONFIG="$SESSION_DIR/remerge.conf"
SERVER_LOG="$LOG_DIR/server.log"
CLIENT_LOG="$LOG_DIR/client.log"
PID_FILE="$SESSION_DIR/server.pid"

usage() {
    cat <<'EOF'
Usage: scripts/local-debug.sh <command> [args...]

Commands:
  prepare               Build local binaries and write temp config/state
  start                 Start remerge-server locally with trace logging
  run -- <emerge args>  Start server if needed and run remerge with --no-local
  run-local -- <args>   Same as run, but allow the final local emerge step
  doctor                Probe local endpoints, including missing-blobs
  env                   Print the paths, env vars, and exact commands in use
  logs                  Tail the local server log
  stop                  Stop the local debug server
  clean                 Stop the server and remove the temp debug directory

Environment overrides:
  REMERGE_LOCAL_DEBUG_PROFILE=debug|release
  REMERGE_LOCAL_DEBUG_DIR=/path/to/session
  REMERGE_LOCAL_DEBUG_HOST=127.0.0.1
  REMERGE_LOCAL_DEBUG_PORT=17654
  REMERGE_LOCAL_DEBUG_RUST_LOG=...
    REMERGE_LOCAL_DEBUG_ALLOW_REMOTE=false
  REMERGE_LOCAL_DEBUG_BUILD_TIMEOUT_SECS=3600
  REMERGE_LOCAL_DEBUG_WORKER_IDLE_TIMEOUT=600
  REMERGE_LOCAL_DEBUG_MAX_WORKERS=1
  REMERGE_LOCAL_DEBUG_MAX_ACTIVE_WORKORDERS=8
  REMERGE_LOCAL_DEBUG_WORKER_NETWORK_MODE=bridge
  REMERGE_LOCAL_DEBUG_SKIP_WORKER_SYNC=false
    REMERGE_LOCAL_DEBUG_DISTDIR=/path/to/distfiles
  REMERGE_LOCAL_DEBUG_WORKER_BASE_IMAGE=ghcr.io/...
  REMERGE_LOCAL_DEBUG_REPOS_DIR=/var/db/repos

Notes:
  - The run command injects --no-local by default so you can exercise the
    remote-build flow without needing root for the final local emerge step.
  - Full worker builds still require Docker access as your user.
EOF
}

ensure_session_dirs() {
    mkdir -p "$STATE_DIR" "$BINPKG_DIR" "$DISTDIR_DIR" "$LOG_DIR"
}

generate_client_id() {
    cat /proc/sys/kernel/random/uuid
}

write_cli_config() {
    ensure_session_dirs
    if [[ -f "$CLI_CONFIG" ]]; then
        return
    fi

    cat >"$CLI_CONFIG" <<EOF
server = "$SERVER_URL"
client_id = "$(generate_client_id)"
role = "main"
EOF
}

write_server_config() {
    ensure_session_dirs
    ensure_safe_listen_host

    local repos_dir="${REMERGE_LOCAL_DEBUG_REPOS_DIR:-}"
    if [[ -z "$repos_dir" && -d /var/db/repos && -r /var/db/repos ]]; then
        repos_dir="/var/db/repos"
    fi

    cat >"$SERVER_CONFIG" <<EOF
binpkg_dir = "$BINPKG_DIR"
binhost_url = "$SERVER_URL/binpkgs"
max_workers = $MAX_WORKERS
max_active_workorders = $MAX_ACTIVE_WORKORDERS
build_timeout_secs = $BUILD_TIMEOUT_SECS
worker_idle_timeout = $WORKER_IDLE_TIMEOUT
worker_network_mode = "$WORKER_NETWORK_MODE"
state_dir = "$STATE_DIR"
worker_binary = "$WORKER_BIN"
log_json = false
skip_worker_sync = $SKIP_WORKER_SYNC
snapshot_min_retained_bytes = 0
EOF

    if [[ -n "$repos_dir" ]]; then
        cat >>"$SERVER_CONFIG" <<EOF
repos_dir = "$repos_dir"
EOF
    fi

    if [[ -n "${REMERGE_LOCAL_DEBUG_WORKER_BASE_IMAGE:-}" ]]; then
        cat >>"$SERVER_CONFIG" <<EOF
worker_base_image = "${REMERGE_LOCAL_DEBUG_WORKER_BASE_IMAGE}"
EOF
    fi

    cat >>"$SERVER_CONFIG" <<EOF

[auth]
mode = "none"
EOF
}

ensure_binaries() {
    local profile_flag=()
    if [[ "$PROFILE" == "release" ]]; then
        profile_flag+=(--release)
    fi

    if [[ -x "$CLI_BIN" && -x "$SERVER_BIN" && -x "$WORKER_BIN" ]]; then
        return
    fi

    echo "Building remerge local debug binaries ($PROFILE profile)…"
    (
        cd "$REPO_DIR"
        cargo build "${profile_flag[@]}" -p remerge -p remerge-server -p remerge-worker
    )
}

server_pid() {
    if [[ -f "$PID_FILE" ]]; then
        cat "$PID_FILE"
    fi
}

pid_matches_server() {
    local pid="$1"
    local cmdline

    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [[ -r "/proc/$pid/cmdline" ]] || return 1

    cmdline="$(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null || true)"
    [[ -n "$cmdline" ]] || return 1

    grep -Fxq -- "$SERVER_BIN" <<<"$cmdline" || return 1
    grep -Fxq -- "$SERVER_CONFIG" <<<"$cmdline" || return 1
    grep -Fxq -- "${LISTEN_HOST}:${LISTEN_PORT}" <<<"$cmdline" || return 1
}

server_running() {
    local pid
    pid="$(server_pid || true)"
    [[ -n "$pid" ]] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    pid_matches_server "$pid"
}

wait_for_server() {
    local attempts=50
    local body_file="$LOG_DIR/health.body"

    for _ in $(seq 1 "$attempts"); do
        if curl -fsS "$SERVER_URL/api/v1/health" >"$body_file" 2>/dev/null; then
            return
        fi
        sleep 0.2
    done

    echo "Local debug server failed to start. Tail of $SERVER_LOG:" >&2
    tail -n 80 "$SERVER_LOG" >&2 || true
    exit 1
}

start_server() {
    ensure_binaries
    write_cli_config
    write_server_config

    if server_running; then
        echo "Local debug server already running at $SERVER_URL (pid $(server_pid))"
        return
    fi

    if [[ -f "$PID_FILE" ]]; then
        local existing_pid
        existing_pid="$(server_pid || true)"
        if [[ -n "$existing_pid" ]]; then
            if kill -0 "$existing_pid" 2>/dev/null && ! pid_matches_server "$existing_pid"; then
                echo "Ignoring stale pidfile at $PID_FILE for unexpected live pid $existing_pid" >&2
            fi
        fi
        rm -f "$PID_FILE"
    fi

    : >"$SERVER_LOG"
    echo "Starting local debug server at $SERVER_URL"
    (
        cd "$REPO_DIR"
        RUST_LOG="$RUST_LOG_VALUE" \
        RUST_BACKTRACE=1 \
        "$SERVER_BIN" --config "$SERVER_CONFIG" --listen "${LISTEN_HOST}:${LISTEN_PORT}"
    ) >>"$SERVER_LOG" 2>&1 &
    echo $! >"$PID_FILE"

    wait_for_server
    echo "Server ready. Logs: $SERVER_LOG"
}

stop_server() {
    local pid
    pid="$(server_pid || true)"

    if [[ -z "$pid" ]]; then
        rm -f "$PID_FILE"
        echo "Local debug server is not running"
        return
    fi

    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f "$PID_FILE"
        echo "Removed stale pidfile for pid $pid"
        return
    fi

    if ! pid_matches_server "$pid"; then
        echo "Refusing to kill unexpected process from $PID_FILE (pid $pid)" >&2
        return 1
    fi

    kill "$pid"
    wait "$pid" 2>/dev/null || true
    rm -f "$PID_FILE"
    echo "Stopped local debug server (pid $pid)"
}

run_cli() {
    local inject_no_local="$1"
    shift

    if [[ $# -eq 0 ]]; then
        echo "No emerge arguments supplied. Use: scripts/local-debug.sh run -- dev-libs/openssl" >&2
        exit 1
    fi

    start_server
    write_cli_config
    : >"$CLIENT_LOG"

    local cli_args=(--config "$CLI_CONFIG" --server "$SERVER_URL")
    if [[ "$inject_no_local" == "yes" ]]; then
        cli_args+=(--no-local)
    fi

    echo "Running local debug CLI against $SERVER_URL"
    echo "Profile: $PROFILE"
    echo "CLI binary: $CLI_BIN"
    echo "Client log: $CLIENT_LOG"

    (
        cd "$REPO_DIR"
        RUST_LOG="$RUST_LOG_VALUE" \
        RUST_BACKTRACE=1 \
        DISTDIR="$DISTDIR_DIR" \
        "$CLI_BIN" "${cli_args[@]}" "$@"
    ) 2>&1 | tee -a "$CLIENT_LOG"
}

doctor() {
    start_server

    echo "== Health =="
    curl -fsS "$SERVER_URL/api/v1/health"
    echo

    echo "== Info =="
    curl -fsS "$SERVER_URL/api/v1/info"
    echo

    local body_file="$LOG_DIR/missing-blobs.body"
    local status
    status="$({
        curl -sS -o "$body_file" -w '%{http_code}' \
            -X POST \
            -H 'content-type: application/json' \
            --data '{"digests":[]}' \
            "$SERVER_URL/api/v1/snapshots/missing-blobs"
    } || true)"

    echo "== Missing Blobs Endpoint =="
    echo "POST $SERVER_URL/api/v1/snapshots/missing-blobs"
    echo "Status: $status"
    if [[ -s "$body_file" ]]; then
        cat "$body_file"
        echo
    fi

    if [[ "$status" == "404" ]]; then
        echo "Hint: a 404 here means you are not talking to the current remerge-server route set." >&2
        echo "Check the server URL, reverse proxy, or whether an older server binary is listening." >&2
        exit 1
    fi
}

show_env() {
    cat <<EOF
Local debug session
  Repo:         $REPO_DIR
  Session dir:  $SESSION_DIR
  Server URL:   $SERVER_URL
  Profile:      $PROFILE
    Distdir:      $DISTDIR_DIR
  Server bin:   $SERVER_BIN
  CLI bin:      $CLI_BIN
  Worker bin:   $WORKER_BIN
  Server cfg:   $SERVER_CONFIG
  CLI cfg:      $CLI_CONFIG
  Server log:   $SERVER_LOG
  Client log:   $CLIENT_LOG
  RUST_LOG:     $RUST_LOG_VALUE

Exact commands
  scripts/local-debug.sh start
  scripts/local-debug.sh doctor
  scripts/local-debug.sh run -- --force dev-libs/openssl
  scripts/local-debug.sh run-local -- --force dev-libs/openssl
EOF
}

clean_session() {
    stop_server || true
    rm -rf "$SESSION_DIR"
    echo "Removed $SESSION_DIR"
}

command="${1:-}"
case "$command" in
    prepare)
        ensure_binaries
        write_cli_config
        write_server_config
        show_env
        ;;
    start)
        start_server
        ;;
    run)
        shift
        if [[ "${1:-}" == "--" ]]; then
            shift
        fi
        run_cli yes "$@"
        ;;
    run-local)
        shift
        if [[ "${1:-}" == "--" ]]; then
            shift
        fi
        run_cli no "$@"
        ;;
    doctor)
        doctor
        ;;
    env)
        write_cli_config
        write_server_config
        show_env
        ;;
    logs)
        mkdir -p "$LOG_DIR"
        touch "$SERVER_LOG"
        tail -n 100 -f "$SERVER_LOG"
        ;;
    stop)
        stop_server
        ;;
    clean)
        clean_session
        ;;
    ""|-h|--help|help)
        usage
        ;;
    *)
        echo "Unknown command: $command" >&2
        usage >&2
        exit 1
        ;;
esac