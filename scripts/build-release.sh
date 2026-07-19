#!/usr/bin/env bash

set -euo pipefail
umask 077

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$PROJECT_DIR/frontend"
DIST_DIR="$PROJECT_DIR/dist"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$PROJECT_DIR/Cargo.toml" | head -1)"
PLATFORM="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
PACKAGE_NAME="ununknown-$VERSION-$PLATFORM"
STAGING_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ununknown-release.XXXXXX")"

cleanup() {
  rm -rf -- "$STAGING_DIR"
}
trap cleanup EXIT

npm --prefix "$FRONTEND_DIR" ci
npm --prefix "$FRONTEND_DIR" audit --audit-level=high
npm --prefix "$FRONTEND_DIR" run format:check
npm --prefix "$FRONTEND_DIR" run lint
npm --prefix "$FRONTEND_DIR" run build
cargo build --manifest-path "$PROJECT_DIR/Cargo.toml" --locked --release

mkdir -p \
  "$STAGING_DIR/$PACKAGE_NAME/bin" \
  "$STAGING_DIR/$PACKAGE_NAME/share/ununknown" \
  "$DIST_DIR"
cp "$PROJECT_DIR/target/release/ununknown" "$STAGING_DIR/$PACKAGE_NAME/bin/ununknown"
cp "$PROJECT_DIR/scripts/run-production.sh" "$STAGING_DIR/$PACKAGE_NAME/bin/ununknown-run"
cp -R "$FRONTEND_DIR/dist/." "$STAGING_DIR/$PACKAGE_NAME/share/ununknown/"
cp "$PROJECT_DIR/LICENSE" "$PROJECT_DIR/README.md" "$STAGING_DIR/$PACKAGE_NAME/"
chmod 755 "$STAGING_DIR/$PACKAGE_NAME/bin/ununknown" "$STAGING_DIR/$PACKAGE_NAME/bin/ununknown-run"

tar -C "$STAGING_DIR" -czf "$DIST_DIR/$PACKAGE_NAME.tar.gz" "$PACKAGE_NAME"
echo "Created $DIST_DIR/$PACKAGE_NAME.tar.gz"
