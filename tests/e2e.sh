#!/usr/bin/env bash
# End-to-end test for Phantom Screen using Docker.
#
# This script:
#   1. Builds the Docker image
#   2. Starts the container
#   3. Verifies the HTTP server serves the client
#   4. Verifies the WebTransport port is listening
#   5. Verifies Xvfb is running inside the container
#   6. Verifies the GStreamer pipeline is active
#   7. Cleans up
#
# Usage: ./tests/e2e.sh [--no-build]
#
# Requirements: docker, curl

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE_NAME="phantom-screen-e2e-test"
CONTAINER_NAME="phantom-screen-e2e-$$"
HTTP_PORT=14444
WT_PORT=14443
TIMEOUT=30

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass=0
fail=0

log_pass() { echo -e "  ${GREEN}PASS${NC} $1"; pass=$((pass + 1)); }
log_fail() { echo -e "  ${RED}FAIL${NC} $1"; fail=$((fail + 1)); }
log_info() { echo -e "  ${YELLOW}INFO${NC} $1"; }

cleanup() {
  log_info "Cleaning up container..."
  docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# ---- Build ----
if [[ "${1:-}" != "--no-build" ]]; then
  echo "Building Docker image..."
  docker build -t "$IMAGE_NAME" -f "$PROJECT_DIR/server/Dockerfile" "$PROJECT_DIR" || {
    log_fail "Docker build failed"
    exit 1
  }
  log_pass "Docker build succeeded"
else
  log_info "Skipping build (--no-build)"
fi

# ---- Start container ----
echo ""
echo "Starting container..."
docker run -d \
  --name "$CONTAINER_NAME" \
  -p "$WT_PORT:4443/udp" \
  -p "$WT_PORT:4443/tcp" \
  -p "$HTTP_PORT:4444/tcp" \
  "$IMAGE_NAME" \
  --display=:99 --resolution=1280x720 --fps=30 >/dev/null

# Wait for startup
echo "Waiting for server to start..."
WAITED=0
while [ $WAITED -lt $TIMEOUT ]; do
  if docker logs "$CONTAINER_NAME" 2>&1 | grep -q "WebTransport server listening"; then
    break
  fi
  sleep 1
  WAITED=$((WAITED + 1))
done

if [ $WAITED -ge $TIMEOUT ]; then
  log_fail "Server did not start within ${TIMEOUT}s"
  echo "Container logs:"
  docker logs "$CONTAINER_NAME" 2>&1 | tail -20
  exit 1
fi
log_pass "Server started within ${WAITED}s"

# ---- Test: HTTP server serves index.html ----
echo ""
echo "Running tests..."

HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:$HTTP_PORT/" 2>/dev/null || echo "000")
if [ "$HTTP_STATUS" = "200" ]; then
  log_pass "HTTP server returns 200 for /"
else
  log_fail "HTTP server returned $HTTP_STATUS for / (expected 200)"
fi

# Check that the response contains expected HTML
HTTP_BODY=$(curl -s "http://localhost:$HTTP_PORT/" 2>/dev/null || echo "")
if echo "$HTTP_BODY" | grep -q "phantom-screen\|desktop-canvas\|Phantom Screen"; then
  log_pass "HTTP response contains expected client content"
else
  log_fail "HTTP response does not contain expected client content"
fi

# Check JS assets are served
JS_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:$HTTP_PORT/assets/" 2>/dev/null || echo "000")
# 200 or 404 for directory listing is ok — we just need the server to respond
if [ "$JS_STATUS" != "000" ]; then
  log_pass "HTTP server responds for /assets/ path"
else
  log_fail "HTTP server not responding for /assets/ path"
fi

# ---- Test: WebTransport port is listening ----
if docker exec "$CONTAINER_NAME" sh -c "ss -tlnp | grep -q 4443 || ss -ulnp | grep -q 4443" 2>/dev/null; then
  log_pass "WebTransport port 4443 is listening inside container"
else
  # Fall back to checking from host
  if nc -z localhost "$WT_PORT" 2>/dev/null; then
    log_pass "WebTransport port is reachable from host"
  else
    log_fail "WebTransport port 4443 not detected"
  fi
fi

# ---- Test: Xvfb is running ----
if docker exec "$CONTAINER_NAME" pgrep -x Xvfb >/dev/null 2>&1; then
  log_pass "Xvfb process is running"
else
  log_fail "Xvfb process is not running"
fi

# ---- Test: Window manager is running ----
if docker exec "$CONTAINER_NAME" pgrep -x openbox >/dev/null 2>&1; then
  log_pass "Window manager (openbox) is running"
else
  log_fail "Window manager is not running"
fi

# ---- Test: GStreamer pipeline is active ----
if docker logs "$CONTAINER_NAME" 2>&1 | grep -q "GStreamer pipeline running"; then
  log_pass "GStreamer pipeline initialized"
else
  log_fail "GStreamer pipeline did not initialize"
fi

# ---- Test: Server is accepting sessions ----
if docker logs "$CONTAINER_NAME" 2>&1 | grep -q "WebTransport server listening"; then
  log_pass "WebTransport server is accepting sessions"
else
  log_fail "WebTransport server not ready"
fi

# ---- Test: Container health (no crash loops) ----
CONTAINER_STATUS=$(docker inspect -f '{{.State.Status}}' "$CONTAINER_NAME" 2>/dev/null || echo "unknown")
if [ "$CONTAINER_STATUS" = "running" ]; then
  log_pass "Container is still running (no crash)"
else
  log_fail "Container status: $CONTAINER_STATUS (expected running)"
fi

# ---- Test: Resolution was applied ----
if docker logs "$CONTAINER_NAME" 2>&1 | grep -q "1280x720"; then
  log_pass "Custom resolution 1280x720 was applied"
else
  log_fail "Custom resolution not detected in logs"
fi

# ---- Results ----
echo ""
echo "====================================="
echo -e "  Results: ${GREEN}$pass passed${NC}, ${RED}$fail failed${NC}"
echo "====================================="

if [ $fail -gt 0 ]; then
  echo ""
  echo "Container logs:"
  docker logs "$CONTAINER_NAME" 2>&1 | tail -30
  exit 1
fi

exit 0
