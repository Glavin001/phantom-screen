#!/usr/bin/env bash
# Local end-to-end test for Phantom Screen (no Docker required).
#
# Tests the full stack: Xvfb + openbox + server + GStreamer pipeline,
# including BackingStore verification.
#
# Requirements: Xvfb, openbox, xrandr, curl, GStreamer plugins (x264)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

SERVER_BIN="$PROJECT_DIR/server/target/release/phantom-screen-server"
CLIENT_DIR="$PROJECT_DIR/client/dist/standalone"
DISPLAY_NUM=":42"
RESOLUTION="1280x720"
WT_PORT=24443
TIMEOUT=15

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass=0
fail=0

log_pass() { echo -e "  ${GREEN}PASS${NC} $1"; pass=$((pass + 1)); }
log_fail() { echo -e "  ${RED}FAIL${NC} $1"; fail=$((fail + 1)); }
log_info() { echo -e "  ${YELLOW}INFO${NC} $1"; }

PIDS=()
cleanup() {
  log_info "Cleaning up..."
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  # Kill any Xvfb on our display
  pkill -f "Xvfb $DISPLAY_NUM" 2>/dev/null || true
  rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true
}
trap cleanup EXIT

# ---- Prerequisites ----
echo "Checking prerequisites..."
for cmd in Xvfb openbox xrandr curl; do
  if ! command -v "$cmd" &>/dev/null; then
    log_fail "Required command not found: $cmd"
    exit 1
  fi
done

if [ ! -x "$SERVER_BIN" ]; then
  log_fail "Server binary not found at $SERVER_BIN (run: cargo build --release)"
  exit 1
fi

if [ ! -d "$CLIENT_DIR" ]; then
  log_fail "Client build not found at $CLIENT_DIR (run: npm run build)"
  exit 1
fi
log_pass "All prerequisites found"

# ---- Clean stale display ----
pkill -f "Xvfb $DISPLAY_NUM" 2>/dev/null || true
sleep 0.5
rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true

# ---- Start Xvfb manually to verify +bs and RANDR ----
echo ""
echo "Starting Xvfb with BackingStore and RANDR..."
Xvfb "$DISPLAY_NUM" -screen 0 "${RESOLUTION}x24" -ac +bs +extension RANDR &>/tmp/xvfb-test.log &
PIDS+=($!)
sleep 1

if kill -0 "${PIDS[-1]}" 2>/dev/null; then
  log_pass "Xvfb started on display $DISPLAY_NUM"
else
  log_fail "Xvfb failed to start"
  cat /tmp/xvfb-test.log
  exit 1
fi

# ---- Verify Xvfb features ----
XVFB_CMDLINE=$(cat /proc/${PIDS[-1]}/cmdline 2>/dev/null | tr '\0' ' ' || echo "")
if echo "$XVFB_CMDLINE" | grep -q "+bs"; then
  log_pass "Xvfb running with BackingStore (+bs)"
else
  log_fail "Xvfb not running with BackingStore. Cmdline: $XVFB_CMDLINE"
fi

if echo "$XVFB_CMDLINE" | grep -q "RANDR"; then
  log_pass "Xvfb running with RANDR extension"
else
  log_fail "Xvfb not running with RANDR extension. Cmdline: $XVFB_CMDLINE"
fi

# ---- Verify xrandr works on the display ----
export DISPLAY="$DISPLAY_NUM"

XRANDR_OUT=$(xrandr --query 2>&1 || echo "XRANDR_FAILED")
if echo "$XRANDR_OUT" | grep -q "connected"; then
  log_pass "xrandr can query the display (RANDR active)"
else
  log_fail "xrandr cannot query the display: $XRANDR_OUT"
fi

if echo "$XRANDR_OUT" | grep -q "1280x720"; then
  log_pass "Initial resolution is 1280x720"
else
  log_fail "Initial resolution not detected. xrandr output: $XRANDR_OUT"
fi

# ---- Test dynamic resize via Xvfb restart (same approach server uses) ----
echo ""
echo "Testing dynamic resize via Xvfb restart..."

# Kill current Xvfb
kill "${PIDS[-1]}" 2>/dev/null || true
wait "${PIDS[-1]}" 2>/dev/null || true
unset PIDS[-1]
sleep 0.5
rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true

# Start new Xvfb at different resolution (simulating what resize_display does)
Xvfb "$DISPLAY_NUM" -screen 0 "1024x768x24" -ac +bs +extension RANDR &>/tmp/xvfb-resize.log &
PIDS+=($!)
sleep 1

if kill -0 "${PIDS[-1]}" 2>/dev/null; then
  log_pass "Xvfb restarted at 1024x768"
else
  log_fail "Xvfb restart failed"
fi

XRANDR_AFTER=$(xrandr --query 2>&1 || echo "XRANDR_FAILED")
if echo "$XRANDR_AFTER" | grep -q "1024x768.*\*"; then
  log_pass "Display resolution changed to 1024x768 after restart"
else
  log_fail "Resolution not 1024x768 after restart. xrandr: $(echo "$XRANDR_AFTER" | grep '\*')"
fi

# ---- Kill standalone Xvfb, let the server manage its own ----
kill "${PIDS[-1]}" 2>/dev/null || true
wait "${PIDS[-1]}" 2>/dev/null || true
unset PIDS[-1]
sleep 1
rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true

# ---- Start the full server ----
echo ""
echo "Starting phantom-screen-server..."
RUST_LOG=phantom_screen_server=info "$SERVER_BIN" \
  --display="$DISPLAY_NUM" \
  --resolution="$RESOLUTION" \
  --fps=30 \
  --listen="127.0.0.1:$WT_PORT" \
  --client-dir="$CLIENT_DIR" \
  &>/tmp/phantom-screen-test.log &
PIDS+=($!)
SERVER_PID="${PIDS[-1]}"

# Wait for startup
WAITED=0
while [ $WAITED -lt $TIMEOUT ]; do
  if grep -q "WebTransport server listening" /tmp/phantom-screen-test.log 2>/dev/null; then
    break
  fi
  if grep -q "GStreamer pipeline running" /tmp/phantom-screen-test.log 2>/dev/null; then
    # Pipeline running but WT might not be ready yet (needs certs)
    :
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    log_fail "Server process died during startup"
    cat /tmp/phantom-screen-test.log | tail -40
    # Check for common issues
    if grep -q "no element" /tmp/phantom-screen-test.log 2>/dev/null; then
      log_info "Missing GStreamer plugin. Install: apt-get install gstreamer1.0-plugins-ugly"
    fi
    exit 1
  fi
  sleep 1
  WAITED=$((WAITED + 1))
done

if [ $WAITED -ge $TIMEOUT ]; then
  log_fail "Server did not start within ${TIMEOUT}s"
  cat /tmp/phantom-screen-test.log | tail -30
  exit 1
fi
log_pass "Server started within ${WAITED}s"

SERVER_LOGS=$(cat /tmp/phantom-screen-test.log)

# ---- Test: GStreamer pipeline running ----
if echo "$SERVER_LOGS" | grep -q "GStreamer pipeline running"; then
  log_pass "GStreamer pipeline initialized"
else
  log_fail "GStreamer pipeline did not initialize"
fi

# ---- Test: Resolution in logs ----
if echo "$SERVER_LOGS" | grep -q "1280x720"; then
  log_pass "Server configured with 1280x720 resolution"
else
  log_fail "Resolution 1280x720 not found in server logs"
fi

# ---- Test: HTTP server serves index.html ----
HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$((WT_PORT + 1))/" 2>/dev/null || echo "000")
if [ "$HTTP_STATUS" = "200" ]; then
  log_pass "HTTP server returns 200 for /"
else
  log_fail "HTTP server returned $HTTP_STATUS for / (expected 200)"
fi

# ---- Test: Health endpoint ----
HEALTH=$(curl -s "http://127.0.0.1:$((WT_PORT + 1))/health" 2>/dev/null || echo "{}")
if echo "$HEALTH" | grep -q '"ready"'; then
  log_pass "Health endpoint reports ready"
else
  log_fail "Health endpoint: $HEALTH"
fi

# ---- Test: HTTP response contains client content ----
HTTP_BODY=$(curl -s "http://127.0.0.1:$((WT_PORT + 1))/" 2>/dev/null || echo "")
if echo "$HTTP_BODY" | grep -q "phantom-screen\|Phantom Screen"; then
  log_pass "HTTP response contains expected client content"
else
  log_fail "HTTP response does not contain expected client content"
fi

# ---- Test: Xvfb is running (started by server) ----
if pgrep -x Xvfb >/dev/null 2>&1; then
  log_pass "Xvfb process is running (started by server)"
else
  log_fail "Xvfb process is not running"
fi

# ---- Test: Server's Xvfb started with +bs ----
SERVER_XVFB_PID=$(pgrep -x Xvfb 2>/dev/null | head -1)
if [ -n "$SERVER_XVFB_PID" ]; then
  SERVER_XVFB_CMD=$(cat /proc/$SERVER_XVFB_PID/cmdline 2>/dev/null | tr '\0' ' ' || echo "")
  if echo "$SERVER_XVFB_CMD" | grep -q "+bs"; then
    log_pass "Server's Xvfb has BackingStore (+bs) enabled"
  else
    log_fail "Server's Xvfb missing +bs. Cmdline: $SERVER_XVFB_CMD"
  fi

  if echo "$SERVER_XVFB_CMD" | grep -q "RANDR"; then
    log_pass "Server's Xvfb has RANDR extension enabled"
  else
    log_fail "Server's Xvfb missing RANDR. Cmdline: $SERVER_XVFB_CMD"
  fi
else
  log_fail "Could not find Xvfb PID for cmdline inspection"
fi

# ---- Test: xrandr works on server's display ----
export DISPLAY="$DISPLAY_NUM"
XRANDR_SERVER=$(xrandr --query 2>&1 || echo "FAILED")
if echo "$XRANDR_SERVER" | grep -q "connected"; then
  log_pass "xrandr works on server's display"
else
  log_fail "xrandr failed on server's display: $XRANDR_SERVER"
fi

# ---- Test: Server still healthy after all tests ----
if kill -0 "$SERVER_PID" 2>/dev/null; then
  log_pass "Server process still alive after all tests"
else
  log_fail "Server process died"
fi

# ---- Results ----
echo ""
echo "====================================="
echo -e "  Results: ${GREEN}$pass passed${NC}, ${RED}$fail failed${NC}"
echo "====================================="

if [ $fail -gt 0 ]; then
  echo ""
  echo "Server logs:"
  cat /tmp/phantom-screen-test.log | tail -30
  exit 1
fi

exit 0
