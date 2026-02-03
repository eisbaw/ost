#!/usr/bin/env bash
# E2E test: verify `teams-cli teams` lists joined teams and channels.
#
# Prerequisites:
#   - teams-cli built (cargo build)
#   - Valid login session (teams-cli login)
#
# Usage:
#   ./tests/e2e_teams.sh

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

echo "=== E2E: Teams listing test ==="

# 1. Run teams command
echo "[1/3] Fetching teams and channels..."
OUTPUT=$("$CLI" teams 2>/dev/null)

# 2. Verify output contains at least one team
echo "[2/3] Checking for team entries..."
if ! echo "$OUTPUT" | grep -q "^Team:"; then
    echo "ERROR: No teams found in output"
    echo "--- output ---"
    echo "$OUTPUT"
    echo "---"
    exit 1
fi
TEAM_COUNT=$(echo "$OUTPUT" | grep -c "^Team:")
echo "       Found $TEAM_COUNT team(s)."

# 3. Verify channels have thread IDs
echo "[3/3] Checking channel thread IDs..."
if ! echo "$OUTPUT" | grep -q "@thread.tacv2"; then
    echo "ERROR: No channel thread IDs found in output"
    echo "--- output ---"
    echo "$OUTPUT"
    echo "---"
    exit 1
fi
CHANNEL_COUNT=$(echo "$OUTPUT" | grep -c "@thread.tacv2")
echo "       Found $CHANNEL_COUNT channel(s) with thread IDs."

PASS=1
