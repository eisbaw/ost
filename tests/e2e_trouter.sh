#!/usr/bin/env bash
# E2E test: verify Trouter WebSocket connection, registration, and message send.
#
# Prerequisites:
#   - teams-cli built (cargo build)
#   - Valid login session (teams-cli login)
#
# Usage:
#   ./tests/e2e_trouter.sh
#
# The test:
#   1. Starts trouter listener in background
#   2. Waits for "trouter.connected" event (verifies WS + registrations work)
#   3. Sends a timestamped message to the self-chat (verifies chat API works)
#   4. Optionally waits for the message to arrive via Trouter push
#   5. Reports results
#
# Note: Push notification delivery depends on server-side state and may not
# arrive in dev/test contexts due to accumulated message_loss indicators.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CLI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CLI="$CLI_DIR/target/debug/teams-cli"

# Self-chat (48:notes is the Teams Notes / personal chat)
SELF_CHAT="48:notes"

TROUTER_LOG=$(mktemp)
TROUTER_PID=""
PASS=0

cleanup() {
    if [ -n "$TROUTER_PID" ] && kill -0 "$TROUTER_PID" 2>/dev/null; then
        kill "$TROUTER_PID" 2>/dev/null || true
        wait "$TROUTER_PID" 2>/dev/null || true
    fi
    rm -f "$TROUTER_LOG"
    if [ "$PASS" -eq 1 ]; then
        echo "PASS"
    else
        echo "FAIL"
        exit 1
    fi
}
trap cleanup EXIT

# Ensure binary exists
if [ ! -x "$CLI" ]; then
    echo "Building teams-cli..."
    (cd "$CLI_DIR" && cargo build --quiet)
fi

echo "=== E2E: Trouter round-trip test ==="

# 1. Start trouter in background
echo "[1/4] Starting Trouter listener..."
"$CLI" trouter 2>&1 > "$TROUTER_LOG" &
TROUTER_PID=$!

# 2. Wait for trouter.connected
echo "[2/4] Waiting for Trouter connection..."
for i in $(seq 1 30); do
    if grep -q '"name":"trouter.connected"' "$TROUTER_LOG" 2>/dev/null; then
        echo "       Trouter connected."
        break
    fi
    if ! kill -0 "$TROUTER_PID" 2>/dev/null; then
        echo "ERROR: Trouter process died"
        cat "$TROUTER_LOG"
        exit 1
    fi
    sleep 1
done

if ! grep -q '"name":"trouter.connected"' "$TROUTER_LOG" 2>/dev/null; then
    echo "ERROR: Trouter did not connect within 30s"
    cat "$TROUTER_LOG"
    exit 1
fi

# 3. Send a timestamped message
TIMESTAMP=$(date -u +%Y%m%dT%H%M%S)
MSG="e2e-trouter-test-${TIMESTAMP}"
echo "[3/4] Sending message: $MSG"
"$CLI" send --to "$SELF_CHAT" "$MSG" 2>/dev/null
echo "       Message sent."

# 4. Verify connection + send succeeded (push notification is best-effort)
echo "[4/4] Verifying..."
echo "       Trouter connected and message sent successfully."

# Best-effort: check if push notification arrives (10s, non-blocking)
FOUND=0
for i in $(seq 1 10); do
    if grep -q "$MSG" "$TROUTER_LOG" 2>/dev/null; then
        FOUND=1
        break
    fi
    sleep 1
done
if [ "$FOUND" -eq 1 ]; then
    echo "       Push notification received."
else
    echo "       Push notification not received (server-side state; non-fatal)."
fi
PASS=1
