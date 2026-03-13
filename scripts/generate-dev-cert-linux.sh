#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-./.tmp/dev-cert}"
mkdir -p "$OUT_DIR"

CERT_PATH="$OUT_DIR/cert.pem"
KEY_PATH="$OUT_DIR/key.pem"

openssl req \
  -x509 \
  -newkey ec \
  -pkeyopt ec_paramgen_curve:prime256v1 \
  -sha256 \
  -nodes \
  -days 10 \
  -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1" \
  -keyout "$KEY_PATH" \
  -out "$CERT_PATH" \
  >/dev/null 2>&1

CERT_HASH_HEX="$(
  openssl x509 -in "$CERT_PATH" -outform DER |
    openssl dgst -sha256 -binary |
    od -An -vtx1 |
    tr -d ' \n'
)"

CERT_HASH_BASE64="$(
  openssl x509 -in "$CERT_PATH" -outform DER |
    openssl dgst -sha256 -binary |
    openssl base64 -A
)"

# Write machine-readable hash file alongside the cert for automation
printf '%s' "$CERT_HASH_HEX" > "$OUT_DIR/cert.sha256"

echo "Generated development certificate:"
echo "  cert:            $CERT_PATH"
echo "  key:             $KEY_PATH"
echo "  sha256 file:     $OUT_DIR/cert.sha256"
echo "  sha256 (hex):    $CERT_HASH_HEX"
echo "  sha256 (base64): $CERT_HASH_BASE64"
