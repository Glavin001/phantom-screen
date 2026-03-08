#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "usage: $0 <platform> <version> <server-binary> <standalone-client-dir> <output-dir>" >&2
  exit 1
fi

PLATFORM="$1"
VERSION="$2"
SERVER_BINARY="$3"
CLIENT_DIR="$4"
OUTPUT_DIR="$5"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE_NAME="phantom-screen-server-${VERSION}-${PLATFORM}"
WORK_DIR="$OUTPUT_DIR/$BUNDLE_NAME"

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/bin" "$WORK_DIR/client" "$WORK_DIR/tools"

cp "$SERVER_BINARY" "$WORK_DIR/bin/phantom-screen-server"
cp -R "$CLIENT_DIR" "$WORK_DIR/client/standalone"
cp "$ROOT_DIR/README.md" "$WORK_DIR/README.md"
cp "$ROOT_DIR/scripts/generate-dev-cert.sh" "$WORK_DIR/tools/generate-dev-cert.sh"

cat >"$WORK_DIR/run-server.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "$SCRIPT_DIR/bin/phantom-screen-server" \
  --client-dir "$SCRIPT_DIR/client/standalone" \
  "$@"
EOF

chmod +x \
  "$WORK_DIR/bin/phantom-screen-server" \
  "$WORK_DIR/run-server.sh" \
  "$WORK_DIR/tools/generate-dev-cert.sh"
tar -czf "$OUTPUT_DIR/${BUNDLE_NAME}.tar.gz" -C "$OUTPUT_DIR" "$BUNDLE_NAME"
