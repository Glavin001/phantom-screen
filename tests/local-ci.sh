#!/usr/bin/env bash
# Run all CI checks locally, mirroring the GitHub Actions workflow.
#
# Runs Rust and client checks natively (cargo, npm/node).
# Docker build and E2E tests use Docker.
#
# Usage: ./tests/local-ci.sh [--rust-only | --client-only | --e2e-only | --docker-only]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

source "$SCRIPT_DIR/docker-proxy-helper.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

pass=0
fail=0
skip=0

log_pass() { echo -e "  ${GREEN}PASS${NC} $1"; pass=$((pass + 1)); }
log_fail() { echo -e "  ${RED}FAIL${NC} $1"; fail=$((fail + 1)); }
log_skip() { echo -e "  ${YELLOW}SKIP${NC} $1"; skip=$((skip + 1)); }
log_section() { echo -e "\n${BOLD}=== $1 ===${NC}"; }

RUN_RUST=true
RUN_CLIENT=true
RUN_E2E=true
RUN_DOCKER=true

case "${1:-}" in
  --rust-only)   RUN_CLIENT=false; RUN_E2E=false; RUN_DOCKER=false ;;
  --client-only) RUN_RUST=false;   RUN_E2E=false; RUN_DOCKER=false ;;
  --e2e-only)    RUN_RUST=false;   RUN_CLIENT=false; RUN_DOCKER=false ;;
  --docker-only) RUN_RUST=false;   RUN_CLIENT=false; RUN_E2E=false ;;
esac

run_step() {
  local label="$1"
  shift
  if "$@"; then
    log_pass "$label"
  else
    log_fail "$label"
  fi
}

# ---- Rust CI (fmt, clippy, test) ----
if $RUN_RUST; then
  log_section "Rust CI (fmt, clippy, test)"

  cd "$PROJECT_DIR/server"
  export CARGO_TERM_COLOR=always RUST_BACKTRACE=1 CARGO_INCREMENTAL=0

  run_step "cargo fmt" cargo fmt -- --check
  run_step "cargo clippy" cargo clippy -- -D warnings
  run_step "cargo test" cargo test --verbose

  cd "$PROJECT_DIR"
fi

# ---- Client CI (typecheck, test, build, package, smoke) ----
if $RUN_CLIENT; then
  log_section "Client CI (typecheck, test, build, package)"

  cd "$PROJECT_DIR/client"
  npm ci

  run_step "npm run typecheck" npm run typecheck
  run_step "npm test" npm test
  run_step "npm run build" npm run build

  mkdir -p dist/artifacts
  run_step "npm pack:tarball" sh -c 'npm run --silent pack:tarball >/dev/null'

  log_section "Client package smoke test"
  cd "$PROJECT_DIR"
  run_step "client-package-smoke.sh" bash ./tests/client-package-smoke.sh
fi

# ---- Docker build ----
if $RUN_DOCKER; then
  log_section "Docker build"
  run_step "docker build" docker_build_proxied "$PROJECT_DIR/server/Dockerfile" -t phantom-screen "$PROJECT_DIR"
fi

# ---- E2E tests ----
if $RUN_E2E; then
  log_section "E2E tests (Docker)"
  run_step "e2e.sh" "$PROJECT_DIR/tests/e2e.sh"
fi

# ---- Results ----
echo ""
echo "====================================="
echo -e "  ${GREEN}$pass passed${NC}, ${RED}$fail failed${NC}, ${YELLOW}$skip skipped${NC}"
echo "====================================="

if [ $fail -gt 0 ]; then
  exit 1
fi

exit 0
