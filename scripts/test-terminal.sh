#!/bin/bash
# Test hooks for autonomous terminal testing
# Usage: ./scripts/test-terminal.sh <command> [args...]

set -e

ULTTY_SOCKET="${ULTTY_SOCKET:-/tmp/ultty-test.sock}"
ULTTY_PID_FILE="${ULTTY_PID_FILE:-/tmp/ultty-test.pid}"
ULTTY_LOG="${ULTTY_LOG:-/tmp/ultty-test.log}"

case "$1" in
    start)
        # Start ultty in test mode (headless if possible, otherwise minimized)
        echo "Starting ultty for testing..."
        cargo run --bin ultty -- --test-mode 2>"$ULTTY_LOG" &
        echo $! > "$ULTTY_PID_FILE"
        sleep 2
        echo "Ultty started with PID $(cat $ULTTY_PID_FILE)"
        ;;

    stop)
        # Stop test instance
        if [ -f "$ULTTY_PID_FILE" ]; then
            PID=$(cat "$ULTTY_PID_FILE")
            if kill -0 "$PID" 2>/dev/null; then
                kill "$PID"
                echo "Stopped ultty (PID $PID)"
            fi
            rm -f "$ULTTY_PID_FILE"
        fi
        ;;

    send)
        # Send input to the terminal via the PTY
        # This requires the terminal to expose a control socket
        shift
        echo "Sending: $*"
        # For now, use xdotool if available
        if command -v xdotool &>/dev/null; then
            xdotool type --delay 50 "$*"
        else
            echo "xdotool not available, cannot send input"
            exit 1
        fi
        ;;

    capture)
        # Capture terminal state/screenshot
        OUTPUT="${2:-/tmp/ultty-capture.txt}"
        echo "Capturing terminal state to $OUTPUT..."
        # Use the headless test API
        cargo test --package ultty -- --nocapture capture_current_state 2>/dev/null || {
            echo "Note: Use the AppTester API in tests for proper capture"
        }
        ;;

    test-echo)
        # Quick test: echo something and verify
        echo "Testing basic echo..."
        $0 send "echo 'ULTTY_TEST_OK'"
        $0 send ""  # Enter
        sleep 1
        echo "Check terminal output for 'ULTTY_TEST_OK'"
        ;;

    test-colors)
        # Test ANSI colors
        echo "Testing ANSI colors..."
        $0 send 'for i in {0..15}; do printf "\\e[48;5;${i}m  \\e[0m"; done; echo'
        $0 send ""
        sleep 1
        ;;

    test-mouse)
        # Test mouse tracking
        echo "Testing mouse tracking..."
        $0 send 'printf "\\e[?1000h"'  # Enable mouse tracking
        $0 send ""
        sleep 1
        echo "Mouse tracking enabled. Click in terminal to test."
        ;;

    run-compat-tests)
        # Run the compatibility test suite
        echo "Running compatibility tests..."
        cargo test --package ultty --test app_compat -- --nocapture
        ;;

    help|*)
        echo "Ultty Test Hooks"
        echo ""
        echo "Usage: $0 <command> [args...]"
        echo ""
        echo "Commands:"
        echo "  start           Start ultty for testing"
        echo "  stop            Stop test instance"
        echo "  send <text>     Send text input to terminal"
        echo "  capture [file]  Capture terminal state"
        echo "  test-echo       Quick echo test"
        echo "  test-colors     Test ANSI color rendering"
        echo "  test-mouse      Test mouse tracking"
        echo "  run-compat-tests  Run full compatibility test suite"
        echo ""
        echo "Environment:"
        echo "  ULTTY_LOG       Log file path (default: /tmp/ultty-test.log)"
        ;;
esac
