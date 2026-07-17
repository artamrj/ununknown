#!/usr/bin/env bash

set -euo pipefail

# Give each background server its own process group so cleanup also stops
# descendants started by cargo-watch and npm.
set -m

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="$PROJECT_DIR/frontend"
LOCAL_DIR="$PROJECT_DIR/.local"

for command_name in cargo cargo-watch npm; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Missing required command: $command_name" >&2
    if [[ "$command_name" == "cargo-watch" ]]; then
      echo "Install it with: cargo install cargo-watch" >&2
    fi
    exit 1
  fi
done

mkdir -p \
  "$LOCAL_DIR/cache" \
  "$LOCAL_DIR/input" \
  "$LOCAL_DIR/output"

if [[ ! -d "$FRONTEND_DIR/node_modules" ]]; then
  echo "Installing frontend dependencies..."
  npm --prefix "$FRONTEND_DIR" install
fi

if command -v lsof >/dev/null 2>&1; then
  backend_busy=false
  frontend_busy=false
  lsof -nP -iTCP:7331 -sTCP:LISTEN >/dev/null 2>&1 && backend_busy=true
  lsof -nP -iTCP:5173 -sTCP:LISTEN >/dev/null 2>&1 && frontend_busy=true
  if $backend_busy && $frontend_busy \
    && curl --silent --fail --max-time 2 http://localhost:7331/api/status >/dev/null \
    && curl --silent --fail --max-time 2 http://localhost:5173/ >/dev/null; then
    echo "Development server is already running with live reload."
    echo "Open: http://localhost:5173"
    exit 0
  fi
  if $backend_busy || $frontend_busy; then
    echo "A required port is occupied by another or incomplete server:" >&2
    $backend_busy && lsof -nP -iTCP:7331 -sTCP:LISTEN >&2
    $frontend_busy && lsof -nP -iTCP:5173 -sTCP:LISTEN >&2
    exit 1
  fi
fi

export UNUNKNOWN_DB="${UNUNKNOWN_DB:-$LOCAL_DIR/cache/ununknown.sqlite}"
export UNUNKNOWN_INPUT_DIR="${UNUNKNOWN_INPUT_DIR:-$LOCAL_DIR/input}"
export UNUNKNOWN_OUTPUT_DIR="${UNUNKNOWN_OUTPUT_DIR:-$LOCAL_DIR/output}"

child_pids=()

cleanup() {
  trap - EXIT INT TERM
  echo
  echo "Stopping development servers..."
  for child_pid in "${child_pids[@]}"; do
    kill -- "-$child_pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}

trap cleanup EXIT INT TERM

echo "Starting backend:  http://localhost:7331"
echo "Starting frontend: http://localhost:5173"
echo "Press Ctrl+C to stop both."
echo

(cd "$PROJECT_DIR" && exec cargo watch -x run) &
child_pids+=("$!")

(cd "$FRONTEND_DIR" && exec npm run dev -- --strictPort) &
child_pids+=("$!")

while kill -0 "${child_pids[0]}" 2>/dev/null && \
      kill -0 "${child_pids[1]}" 2>/dev/null; do
  sleep 1
done

echo "A development server stopped unexpectedly." >&2
exit 1
