#!/usr/bin/env bash

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BINARY="$SCRIPT_DIR/ununknown"
STATIC_DIR="$PACKAGE_DIR/share/ununknown"

if [[ ! -x "$BINARY" ]]; then
  echo "Production binary not found: $BINARY" >&2
  exit 1
fi
if [[ ! -f "$STATIC_DIR/index.html" ]]; then
  echo "Production frontend not found: $STATIC_DIR/index.html" >&2
  exit 1
fi

if [[ -n "${UNUNKNOWN_DATA_DIR:-}" ]]; then
  DATA_DIR="$UNUNKNOWN_DATA_DIR"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  DATA_DIR="${HOME}/Library/Application Support/Ununknown"
else
  DATA_DIR="${XDG_STATE_HOME:-${HOME}/.local/state}/ununknown"
fi

mkdir -p "$DATA_DIR/cache" "$DATA_DIR/input" "$DATA_DIR/output"
export UNUNKNOWN_DB="${UNUNKNOWN_DB:-$DATA_DIR/cache/ununknown.sqlite}"
export UNUNKNOWN_INPUT_DIR="${UNUNKNOWN_INPUT_DIR:-$DATA_DIR/input}"
export UNUNKNOWN_OUTPUT_DIR="${UNUNKNOWN_OUTPUT_DIR:-$DATA_DIR/output}"
export UNUNKNOWN_STATIC_DIR="${UNUNKNOWN_STATIC_DIR:-$STATIC_DIR}"
export UNUNKNOWN_BIND="${UNUNKNOWN_BIND:-127.0.0.1:7331}"

echo "Starting Ununknown at http://$UNUNKNOWN_BIND"
exec "$BINARY"
