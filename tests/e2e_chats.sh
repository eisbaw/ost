#!/usr/bin/env bash
# E2E test: verify `teams-cli chats` lists recent conversations.
#
# Prerequisites:
#   - teams-cli built (cargo build)
#   - Valid login session (teams-cli login)
#
# Usage:
#   ./tests/e2e_chats.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CLI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CLI="$CLI_DIR/target/debug/teams-cli"

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

if [ ! -x "$CLI" ]; then
    echo "Building teams-cli..."
    (cd "$CLI_DIR" && cargo build --quiet)
fi

echo "=== E2E: Chats listing test ==="

# 1. Run chats command
echo "[1/3] Fetching recent chats..."
OUTPUT=$("$CLI" chats 2>/dev/null)

# 2. Verify output contains at least one chat
echo "[2/3] Checking for chat entries..."
if ! echo "$OUTPUT" | grep -q "ID:"; then
    echo "ERROR: No chats found in output"
    echo "--- output ---"
    echo "$OUTPUT"
    echo "---"
    exit 1
fi
CHAT_COUNT=$(echo "$OUTPUT" | grep -c "ID:")
echo "       Found $CHAT_COUNT chat(s)."

# 3. Verify well-known system chats exist (48:notes is always present)
echo "[3/3] Checking for 48:notes system chat..."
if ! echo "$OUTPUT" | grep -q "48:notes"; then
    echo "ERROR: 48:notes not found (expected in every account)"
    echo "--- output ---"
    echo "$OUTPUT"
    echo "---"
    exit 1
fi
echo "       48:notes present."

PASS=1
