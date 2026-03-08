#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLIENT_DIR="$ROOT_DIR/client"
FIXTURE_DIR="$ROOT_DIR/tests/fixtures/next-smoke"

cd "$CLIENT_DIR"
npm ci
npm run build

mkdir -p dist/artifacts
npm run --silent pack:tarball >/dev/null
TARBALLS=(dist/artifacts/*.tgz)
TARBALL_PATH="$CLIENT_DIR/${TARBALLS[0]}"

node "$ROOT_DIR/tests/html-bundle-smoke.mjs" "$CLIENT_DIR/dist/html/phantom-screen-client.iife.js"

cd "$FIXTURE_DIR"
rm -rf node_modules .next
npm ci
npm install --no-save "$TARBALL_PATH"
npm run build
node -e "const pkg = require('@phantom-screen/web-client'); if (typeof pkg.mountPhantomScreen !== 'function') throw new Error('CommonJS export missing');"
node --input-type=module -e "import { createServerCertificateHashes, mountPhantomScreen } from '@phantom-screen/web-client'; if (typeof mountPhantomScreen !== 'function') throw new Error('ES module export missing'); if (!createServerCertificateHashes('00'.repeat(32))) throw new Error('Hash helper missing');"
