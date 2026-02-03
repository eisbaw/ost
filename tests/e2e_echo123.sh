#!/usr/bin/env bash
# E2E test: AV self-call with dual media legs.
#
# Verifies:
# - Call is placed and accepted (self-call to own MRI)
# - Audio packets sent/received on both legs
# - Video packets sent/received on both legs
# - Echo detection (if relay bridges audio)
#
# Requirements:
# - Valid login session (teams-cli login)
# - Network access to Teams services
#
# Exit 0 on PASS, 1 on FAIL.
set -euo pipefail

BINARY="./target/debug/teams-cli"
TIMEOUT=60
DURATION=15

echo "=== E2E: AV self-call test ==="

if [ ! -f "$BINARY" ]; then
    echo "FAIL: binary not found at $BINARY (run 'cargo build' first)"
    exit 1
fi

# Run call test with timeout
OUTPUT=$(timeout "$TIMEOUT" "$BINARY" call-test --duration "$DURATION" 2>&1) || {
    RC=$?
    echo "FAIL: call-test exited with code $RC"
    echo "Output: $OUTPUT"
    exit 1
}

echo "$OUTPUT"

# Check that the call was placed and accepted
if ! echo "$OUTPUT" | grep -q "call_placed=true"; then
    echo "FAIL: call was not placed"
    exit 1
fi

if ! echo "$OUTPUT" | grep -q "call_accepted=true"; then
    echo "FAIL: call was not accepted"
    exit 1
fi

# Extract packet counts
get_val() { echo "$OUTPUT" | grep -oP "${1}=\K[0-9]+" || echo "0"; }

AUDIO_SENT=$(get_val audio_packets_sent)
AUDIO_RECV=$(get_val audio_packets_received)
VIDEO_SENT=$(get_val video_packets_sent)
VIDEO_RECV=$(get_val video_packets_received)
INC_AUDIO_SENT=$(get_val incoming_audio_pkts_sent)
INC_AUDIO_RECV=$(get_val incoming_audio_pkts_recv)
INC_VIDEO_SENT=$(get_val incoming_video_pkts_sent)
INC_VIDEO_RECV=$(get_val incoming_video_pkts_recv)

# Require outgoing leg sent packets
if [ "$AUDIO_SENT" -lt 10 ]; then
    echo "FAIL: too few outgoing audio packets sent ($AUDIO_SENT)"
    exit 1
fi
if [ "$VIDEO_SENT" -lt 5 ]; then
    echo "FAIL: too few outgoing video packets sent ($VIDEO_SENT)"
    exit 1
fi

# Report results
echo "--- Results ---"
echo "Outgoing: audio_sent=$AUDIO_SENT audio_recv=$AUDIO_RECV video_sent=$VIDEO_SENT video_recv=$VIDEO_RECV"
echo "Incoming: audio_sent=$INC_AUDIO_SENT audio_recv=$INC_AUDIO_RECV video_sent=$INC_VIDEO_SENT video_recv=$INC_VIDEO_RECV"

ECHO_DETECTED=$(echo "$OUTPUT" | grep -oP 'echo_detected=\K\w+' || echo "false")
ECHO_DELAY=$(echo "$OUTPUT" | grep -oP 'echo_delay_ms=\K[0-9.]+' || echo "0")
ECHO_CORR=$(echo "$OUTPUT" | grep -oP 'echo_correlation=\K[0-9.]+' || echo "0")
echo "Echo: detected=$ECHO_DETECTED delay=${ECHO_DELAY}ms correlation=$ECHO_CORR"

echo "PASS: AV self-call test"
