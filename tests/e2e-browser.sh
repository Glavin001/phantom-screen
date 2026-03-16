#!/usr/bin/env bash
# Real end-to-end browser test using Playwright + Chromium.
#
# Starts the Phantom Screen server in Docker, opens the actual client page
# in a real headless Chromium, and verifies:
#   - Video frames are received (non-black canvas pixels)
#   - Server survives client resize (Xvfb restart)
#   - Server survives coherence mode toggle
#   - Server survives resize + coherence (the formerly-crashing path)
#
# Usage:
#   ./tests/e2e-browser.sh              # full run (build Docker + run tests)
#   ./tests/e2e-browser.sh --no-build   # skip Docker build (container already running)
#   KEEP_CONTAINER=1 ./tests/e2e-browser.sh   # keep container for inspection

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BROWSER_TEST_DIR="$SCRIPT_DIR/e2e-browser"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "  ${YELLOW}INFO${NC} $1"; }
log_pass() { echo -e "  ${GREEN}PASS${NC} $1"; }
log_fail() { echo -e "  ${RED}FAIL${NC} $1"; }

# ---- Prerequisites ----
echo "Checking prerequisites..."
for cmd in docker node npx; do
  if ! command -v "$cmd" &>/dev/null; then
    log_fail "Required command not found: $cmd"
    exit 1
  fi
done

# ---- Install dependencies ----
echo ""
echo "Installing Playwright and browser..."
cd "$BROWSER_TEST_DIR"

if [ ! -d "node_modules" ]; then
  npm install 2>&1 | tail -5
fi

# Install Chromium if needed
if ! npx playwright install chromium --dry-run &>/dev/null 2>&1; then
  npx playwright install chromium 2>&1 | tail -10
else
  # Try to install anyway — dry-run may not be supported
  npx playwright install chromium 2>&1 | tail -5 || true
fi
log_pass "Playwright installed"

# ---- Build Docker image (unless --no-build) ----
if [ "${1:-}" != "--no-build" ]; then
  echo ""
  echo "Building Docker image..."

  # Source proxy helper if available
  if [ -f "$SCRIPT_DIR/docker-proxy-helper.sh" ]; then
    source "$SCRIPT_DIR/docker-proxy-helper.sh"
    if [ -n "${HTTP_PROXY:-}" ]; then
      docker_build_proxied "$PROJECT_DIR/Dockerfile" -t phantom-screen-e2e "$PROJECT_DIR"
    else
      docker build -t phantom-screen-e2e "$PROJECT_DIR"
    fi
  else
    docker build -t phantom-screen-e2e "$PROJECT_DIR"
  fi
  log_pass "Docker image built"
fi

# ---- Run tests ----
echo ""
echo "Running Playwright browser tests..."
cd "$BROWSER_TEST_DIR"

export KEEP_CONTAINER="${KEEP_CONTAINER:-0}"

if npx playwright test --reporter=list 2>&1; then
  echo ""
  log_pass "All browser e2e tests passed"
  exit 0
else
  echo ""
  log_fail "Browser e2e tests failed"

  # Dump server logs for debugging
  echo ""
  echo "=== Docker container logs ==="
  docker logs --tail 50 phantom-screen-e2e-browser 2>&1 || true

  exit 1
fi
