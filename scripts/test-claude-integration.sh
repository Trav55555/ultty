#!/bin/bash
# Test ultty automation hooks with Claude Code
set -e

ULTTYCTL="cargo run --release -p ultty --bin ulttyctl --"

echo "=== Testing ultty automation hooks ==="

echo -n "1. Ping... "
RESULT=$($ULTTYCTL ping)
if [ "$RESULT" = "PONG" ]; then
    echo "OK"
else
    echo "FAILED: $RESULT"
    exit 1
fi

echo -n "2. Version... "
$ULTTYCTL version

echo -n "3. Info... "
$ULTTYCTL info

echo -n "4. JSON output... "
$ULTTYCTL --json info | head -c 100
echo

echo -n "5. Get terminal text... "
TEXT=$($ULTTYCTL text)
echo "${#TEXT} chars"

echo -n "6. Search test... "
$ULTTYCTL search "test" || echo "(no matches)"

echo ""
echo "=== Interactive tests (run inside ultty with claude) ==="
echo ""
echo "# Wait for Claude prompt:"
echo "ulttyctl wait-for '>' --timeout 30000"
echo ""
echo "# Send a message to Claude:"
echo "ulttyctl send-text 'hello claude'"
echo "ulttyctl send-key enter"
echo ""
echo "# Wait for response:"
echo "ulttyctl wait-for '>' --timeout 60000"
echo ""
echo "# Get the conversation:"
echo "ulttyctl text --scrollback"

echo ""
echo "=== All automated tests passed ==="
