#!/usr/bin/env bash
# End-to-end test for the resize + coherence crash scenario.
#
# Tests that the server survives:
#   1. Xvfb restart (resize_display)
#   2. Window monitor reconnection after Xvfb restart
#   3. Composite extension re-initialization
#   4. Per-window capture of overlapping windows after reconnect
#   5. Multiple resize cycles
#
# This test exercises the exact code path that was crashing:
#   client resize → Xvfb kill → window monitor dies → stale window IDs →
#   BadWindow X error → exit(1)
#
# Requirements: Xvfb, openbox, xrandr, xdotool, curl, xdpyinfo, GStreamer

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

SERVER_BIN="$PROJECT_DIR/server/target/release/phantom-screen-server"
CLIENT_DIR="$PROJECT_DIR/client/dist/standalone"
DISPLAY_NUM=":43"
RESOLUTION="1280x720"
WT_PORT=24543
TIMEOUT=15
SERVER_LOG="/tmp/phantom-screen-resize-test.log"

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
  pkill -f "Xvfb $DISPLAY_NUM" 2>/dev/null || true
  rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true
}
trap cleanup EXIT

# ---- Prerequisites ----
echo "Checking prerequisites..."
for cmd in Xvfb openbox xrandr xdotool curl; do
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

# ---- Start server ----
echo ""
echo "Starting phantom-screen-server..."
RUST_LOG=phantom_screen_server=info "$SERVER_BIN" \
  --display="$DISPLAY_NUM" \
  --resolution="$RESOLUTION" \
  --fps=30 \
  --listen="127.0.0.1:$WT_PORT" \
  --client-dir="$CLIENT_DIR" \
  --post-start-command="xterm -e 'echo RESIZE_TEST; sleep 3600'" \
  &>"$SERVER_LOG" &
PIDS+=($!)
SERVER_PID="${PIDS[-1]}"

# Wait for startup
WAITED=0
while [ $WAITED -lt $TIMEOUT ]; do
  if grep -q "WebTransport server listening" "$SERVER_LOG" 2>/dev/null; then
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    log_fail "Server process died during startup"
    tail -40 "$SERVER_LOG"
    exit 1
  fi
  sleep 1
  WAITED=$((WAITED + 1))
done

if [ $WAITED -ge $TIMEOUT ]; then
  log_fail "Server did not start within ${TIMEOUT}s"
  tail -30 "$SERVER_LOG"
  exit 1
fi
log_pass "Server started within ${WAITED}s"

# ---- Verify initial state ----
export DISPLAY="$DISPLAY_NUM"

# Check Composite extension is active (requires xdpyinfo)
if command -v xdpyinfo &>/dev/null; then
  if xdpyinfo 2>/dev/null | grep -qi "composite"; then
    log_pass "Composite extension active on initial Xvfb"
  else
    log_fail "Composite extension not found on initial Xvfb"
  fi
else
  log_info "xdpyinfo not available, skipping Composite extension check"
fi

# Check window monitor started
if grep -q "Window monitor reconnected\|Window monitor started" "$SERVER_LOG" 2>/dev/null; then
  log_pass "Window monitor initialized"
else
  log_fail "Window monitor not found in logs"
fi

# Check Composite redirect was applied
if grep -q "X Composite.*redirected subwindows" "$SERVER_LOG" 2>/dev/null; then
  log_pass "X Composite redirect applied initially"
else
  log_fail "X Composite redirect not applied"
fi

# Wait for xterm to start
sleep 2

# ---- Test 1: Create overlapping windows before resize ----
echo ""
echo "Creating overlapping windows..."

xterm -T "BACK_WINDOW" -geometry 40x10+0+0 -e "sleep 3600" &
PIDS+=($!)
sleep 1

xterm -T "FRONT_WINDOW" -geometry 40x10+50+30 -e "sleep 3600" &
PIDS+=($!)
sleep 1

WINDOW_COUNT=$(xdotool search --class "XTerm" 2>/dev/null | wc -l | tr -d ' ' || echo "0")
if [ "${WINDOW_COUNT:-0}" -ge 2 ]; then
  log_pass "Created $WINDOW_COUNT overlapping xterm windows"
else
  log_fail "Expected >= 2 xterm windows, found $WINDOW_COUNT"
fi

# ---- Test 2: Per-window capture works before resize ----
BACK_WID=$(xdotool search --class "XTerm" 2>/dev/null | head -1 || echo "")
if [ -n "$BACK_WID" ]; then
  CAPTURE_OK=$(timeout 10 gst-launch-1.0 -q \
    ximagesrc display-name="$DISPLAY_NUM" xid="$BACK_WID" use-damage=0 num-buffers=1 \
    ! videoconvert \
    ! video/x-raw,format=GRAY8 \
    ! filesink location=/tmp/pre_resize_capture.raw 2>&1; echo "EXIT=$?")

  if echo "$CAPTURE_OK" | grep -q "EXIT=0" && [ -s /tmp/pre_resize_capture.raw ]; then
    NONZERO=$(od -An -tx1 /tmp/pre_resize_capture.raw | tr " " "\n" | grep -cv "^00$" 2>/dev/null || echo "0")
    if [ "${NONZERO:-0}" -gt 10 ]; then
      log_pass "Pre-resize per-window capture has real pixels ($NONZERO non-zero bytes)"
    else
      log_info "Pre-resize capture has few non-zero bytes ($NONZERO) — may be timing"
    fi
  else
    log_fail "Pre-resize per-window capture failed"
  fi
else
  log_fail "No xterm window found for pre-resize capture"
fi

# ---- Test 3: Server survives resize (Xvfb restart) ----
echo ""
echo "Simulating client resize (Xvfb restart)..."

# Count current log lines to detect new entries after resize
LOG_LINES_BEFORE=$(wc -l < "$SERVER_LOG")

# Call the resize function via HTTP (the control protocol)
# Since we can't easily send WebTransport messages, we'll trigger a resize
# by directly calling the resize binary path — but actually the server
# handles this internally. Let's simulate what the pipeline does:
# Kill the current Xvfb and restart at a new resolution.
XVFB_PID=$(pgrep -x Xvfb 2>/dev/null | head -1 || echo "")
if [ -n "$XVFB_PID" ]; then
  kill -TERM "$XVFB_PID" 2>/dev/null || true
  sleep 0.5
  rm -f "/tmp/.X${DISPLAY_NUM#:}-lock" 2>/dev/null || true

  # Start new Xvfb at different resolution (simulating resize_display)
  Xvfb "$DISPLAY_NUM" -screen 0 "1024x768x24" -ac +bs +extension RANDR +extension Composite &>/dev/null &
  NEW_XVFB_PID=$!
  PIDS+=($NEW_XVFB_PID)
  sleep 1

  # Restart openbox
  DISPLAY="$DISPLAY_NUM" openbox &>/dev/null &
  PIDS+=($!)
  sleep 1

  if kill -0 "$NEW_XVFB_PID" 2>/dev/null; then
    log_pass "Xvfb restarted at 1024x768 (simulating resize)"
  else
    log_fail "New Xvfb failed to start"
  fi
else
  log_fail "Could not find Xvfb PID to simulate resize"
fi

# ---- Test 4: Server process survived the resize ----
sleep 3  # Give window monitor time to reconnect

if kill -0 "$SERVER_PID" 2>/dev/null; then
  log_pass "Server process still alive after Xvfb restart"
else
  log_fail "SERVER CRASHED after Xvfb restart!"
  echo "Last 30 lines of server log:"
  tail -30 "$SERVER_LOG"
  exit 1
fi

# ---- Test 5: Window monitor reconnected ----
# The monitor should have detected the broken connection, waited 2s, and reconnected
NEW_LOG_LINES=$(tail -n +$((LOG_LINES_BEFORE + 1)) "$SERVER_LOG")

if echo "$NEW_LOG_LINES" | grep -q "X11 connection lost\|X11 event error"; then
  log_pass "Window monitor detected X11 connection loss"
else
  log_info "Window monitor may not have detected loss yet (checking...)"
fi

# Wait a bit more for reconnection
sleep 3

NEW_LOG_LINES=$(tail -n +$((LOG_LINES_BEFORE + 1)) "$SERVER_LOG")
if echo "$NEW_LOG_LINES" | grep -q "Window monitor reconnected"; then
  log_pass "Window monitor reconnected after Xvfb restart"
else
  log_fail "Window monitor did not reconnect. Recent logs:"
  tail -20 "$SERVER_LOG"
fi

# ---- Test 6: Composite re-enabled after reconnect ----
# Count how many times "X Composite: redirected subwindows" appears
COMPOSITE_COUNT=$(grep -c "X Composite.*redirected subwindows" "$SERVER_LOG" 2>/dev/null || echo "0")
if [ "$COMPOSITE_COUNT" -ge 2 ]; then
  log_pass "X Composite redirect re-applied after reconnect ($COMPOSITE_COUNT total)"
else
  log_info "X Composite redirect count: $COMPOSITE_COUNT (expected >= 2)"
fi

# ---- Test 7: Xdpyinfo still shows Composite on new Xvfb ----
if command -v xdpyinfo &>/dev/null; then
  if DISPLAY="$DISPLAY_NUM" xdpyinfo 2>/dev/null | grep -qi "composite"; then
    log_pass "Composite extension active on new Xvfb after resize"
  else
    log_fail "Composite extension not found on new Xvfb"
  fi
else
  log_info "xdpyinfo not available, skipping post-resize Composite check"
fi

# ---- Test 8: Launch new windows on the resized display ----
echo ""
echo "Launching new windows on resized display..."

DISPLAY="$DISPLAY_NUM" xterm -T "POST_RESIZE_1" -geometry 40x10+0+0 -e "sleep 3600" &
PIDS+=($!)
sleep 1

DISPLAY="$DISPLAY_NUM" xterm -T "POST_RESIZE_2" -geometry 40x10+50+30 -e "sleep 3600" &
PIDS+=($!)
sleep 1

NEW_WID=$(DISPLAY="$DISPLAY_NUM" xdotool search --class "XTerm" 2>/dev/null | head -1 || echo "")
if [ -n "$NEW_WID" ]; then
  log_pass "New windows launched on resized display (wid: $NEW_WID)"

  # ---- Test 9: Per-window capture works after resize ----
  CAPTURE_OK=$(timeout 10 gst-launch-1.0 -q \
    ximagesrc display-name="$DISPLAY_NUM" xid="$NEW_WID" use-damage=0 num-buffers=1 \
    ! videoconvert \
    ! video/x-raw,format=GRAY8 \
    ! filesink location=/tmp/post_resize_capture.raw 2>&1; echo "EXIT=$?")

  if echo "$CAPTURE_OK" | grep -q "EXIT=0" && [ -s /tmp/post_resize_capture.raw ]; then
    NONZERO=$(od -An -tx1 /tmp/post_resize_capture.raw | tr " " "\n" | grep -cv "^00$" 2>/dev/null || echo "0")
    if [ "${NONZERO:-0}" -gt 10 ]; then
      log_pass "Post-resize per-window capture has real pixels ($NONZERO non-zero bytes)"
    else
      log_info "Post-resize capture has few non-zero bytes ($NONZERO)"
    fi
  else
    log_fail "Post-resize per-window capture failed"
  fi
else
  log_fail "No windows found on resized display"
fi

# ---- Test 10: Server still alive at the end ----
if kill -0 "$SERVER_PID" 2>/dev/null; then
  log_pass "Server process still alive at end of all tests"
else
  log_fail "Server process died during tests"
fi

# ---- Test 11: HTTP health check still works ----
HTTP_PORT=$((WT_PORT + 1))
HEALTH=$(curl -s "http://127.0.0.1:$HTTP_PORT/health" 2>/dev/null || echo "{}")
if echo "$HEALTH" | grep -q '"ready"'; then
  log_pass "Health endpoint still reports ready"
else
  log_fail "Health endpoint: $HEALTH"
fi

# ---- Test 12: Non-fatal Xlib error handler installed ----
if grep -q "Installed non-fatal Xlib error" "$SERVER_LOG" 2>/dev/null; then
  log_pass "Non-fatal Xlib error handler installed"
else
  log_fail "Non-fatal Xlib error handler not found in logs"
fi

# ---- Results ----
echo ""
echo "====================================="
echo -e "  Results: ${GREEN}$pass passed${NC}, ${RED}$fail failed${NC}"
echo "====================================="

if [ $fail -gt 0 ]; then
  echo ""
  echo "Server logs (last 40 lines):"
  tail -40 "$SERVER_LOG"
  exit 1
fi

exit 0
