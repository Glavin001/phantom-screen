#!/usr/bin/env bash
# Run CI checks locally. Mirrors .github/workflows/ci.yml.
# Usage:
#   ./scripts/ci-local.sh          # run all checks
#   ./scripts/ci-local.sh rust     # run only Rust checks (in Docker)
#   ./scripts/ci-local.sh client   # run only client checks
#   ./scripts/ci-local.sh e2e      # run only e2e tests (in Docker)
set -euo pipefail
cd "$(dirname "$0")/.."

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
RESET='\033[0m'

step() { printf "\n${BOLD}▸ %s${RESET}\n" "$1"; }
pass() { printf "${GREEN}✓ %s${RESET}\n" "$1"; }
fail() { printf "${RED}✗ %s${RESET}\n" "$1"; exit 1; }

run_rust() {
  step "Building Rust builder image..."
  docker build --target builder -f server/Dockerfile -t phantom-test . -q

  step "cargo fmt --check"
  docker run --rm phantom-test sh -c "rustup component add rustfmt 2>/dev/null && cargo fmt -- --check" \
    && pass "cargo fmt" || fail "cargo fmt"

  step "cargo clippy -- -D warnings"
  docker run --rm phantom-test sh -c "rustup component add clippy 2>/dev/null && cargo clippy -- -D warnings" \
    && pass "cargo clippy" || fail "cargo clippy"

  step "cargo test"
  docker run --rm phantom-test cargo test --verbose \
    && pass "cargo test" || fail "cargo test"
}

run_client() {
  step "npm ci"
  (cd client && npm ci --silent)

  step "npm run typecheck"
  (cd client && npm run typecheck) && pass "typecheck" || fail "typecheck"

  step "npm test"
  (cd client && npm test) && pass "client tests" || fail "client tests"

  step "npm run build"
  (cd client && npm run build) && pass "client build" || fail "client build"
}

run_e2e() {
  step "E2E tests (Docker)"
  ./tests/e2e.sh && pass "e2e" || fail "e2e"
}

target="${1:-all}"

case "$target" in
  rust)   run_rust ;;
  client) run_client ;;
  e2e)    run_e2e ;;
  all)
    run_rust
    run_client
    run_e2e
    printf "\n${GREEN}${BOLD}All CI checks passed.${RESET}\n"
    ;;
  *)
    echo "Usage: $0 [rust|client|e2e|all]"
    exit 1
    ;;
esac
