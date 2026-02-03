#!/usr/bin/env bash
# E2E test: send a message then read it back via `teams-cli read`.
#
# Prerequisites:
#   - teams-cli built (cargo build)
#   - Valid login session (teams-cli login)
#
# Usage:
#   ./tests/e2e_read.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CLI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CLI="$CLI_DIR/target/debug/teams-cli"

# Self-chat (48:notes is the Teams Notes / personal chat)
SELF_CHAT="48:notes"

PASS=0

cleanup() {
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

echo "=== E2E: Read round-trip test ==="

# 1. Send a timestamped message
TIMESTAMP=$(date -u +%Y%m%dT%H%M%S)
MSG="e2e-read-test-${TIMESTAMP}"
echo "[1/4] Sending message: $MSG"
"$CLI" send --to "$SELF_CHAT" "$MSG" 2>/dev/null

# 2. Wait for propagation
echo "[2/4] Waiting for propagation..."
sleep 2

# 3. Read recent messages
echo "[3/4] Reading messages from $SELF_CHAT..."
READ_OUTPUT=$("$CLI" read "$SELF_CHAT" --limit 5 2>/dev/null)

# 4. Verify
echo "[4/4] Verifying..."
if echo "$READ_OUTPUT" | grep -q "$MSG"; then
    echo "       Message found in read output."
    PASS=1
else
    echo "ERROR: Message not found in read output"
    echo "--- read output ---"
    echo "$READ_OUTPUT"
    echo "---"
fi
