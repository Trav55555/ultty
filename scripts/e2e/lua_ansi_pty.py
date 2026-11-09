"""Run a ulttui-lua script under a plain pty (no Ultty) and check its ANSI output.

    python3 scripts/e2e/lua_ansi_pty.py <ulttui-lua binary> <script.luau> <transcript>

Starts the binary on an 80x24 pty with the Ultty environment variables
removed and the in-band probe off, waits for "Count: 0", presses "+" twice,
waits for "Count: 2", presses "q" and checks the program exits on its own.
The raw output goes to <transcript>. Exits non-zero when a check fails.
"""

import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time


CSI = re.compile(rb"\x1b\[([0-9;?]*)([ -/]*)([@-~])")


def screen_text(data: bytes, rows: int = 24, cols: int = 80) -> str:
    """Replay cursor moves, clears and text onto a grid; enough for ratatui."""
    grid = [[" "] * cols for _ in range(rows)]
    row = col = 0
    i = 0
    text = data.decode("utf-8", errors="replace").encode("utf-8")
    while i < len(text):
        if text[i] == 0x1B:
            m = CSI.match(text, i)
            if m:
                params, final = m.group(1).decode(), m.group(3)
                nums = [int(p) if p.isdigit() else 0 for p in params.split(";")]
                if final == b"H":
                    row = max(nums[0] if nums and nums[0] else 1, 1) - 1
                    col = max(nums[1] if len(nums) > 1 and nums[1] else 1, 1) - 1
                elif final == b"J" and nums and nums[0] == 2:
                    grid = [[" "] * cols for _ in range(rows)]
                elif final == b"K" and row < rows:
                    grid[row][col:] = [" "] * (cols - col)
                i = m.end()
                continue
            # Any other escape: skip it and its argument byte.
            i += 2
            continue
        # One UTF-8 character.
        length = 1
        while i + length < len(text) and (text[i + length] & 0xC0) == 0x80:
            length += 1
        ch = text[i : i + length].decode("utf-8", errors="replace")
        i += length
        if ch == "\r":
            col = 0
        elif ch == "\n":
            row += 1
        elif ch.isprintable() and row < rows and col < cols:
            grid[row][col] = ch
            col += 1
    return "\n".join("".join(r) for r in grid)


def main() -> int:
    binary, script, transcript = sys.argv[1:4]
    env = {k: v for k, v in os.environ.items() if not k.startswith("ULTTY")}
    env["ULTTY_IN_BAND"] = "0"
    env["TERM"] = "xterm-256color"

    pid, fd = pty.fork()
    if pid == 0:
        os.execve(binary, [binary, script], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    output = bytearray()

    def read_until(pattern: str, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            # ratatui redraws only changed cells: look at the screen, not the stream.
            if pattern in screen_text(bytes(output)):
                return True
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    return False
                if not chunk:
                    return False
                output.extend(chunk)
        return False

    def fail(message: str) -> int:
        with open(transcript, "wb") as f:
            f.write(output)
        print(f"FAIL: {message} (transcript: {transcript})", file=sys.stderr)
        try:
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
        return 1

    if not read_until("Count: 0", 10):
        return fail("never drew Count: 0")
    if b"\x1b[" not in output:
        return fail("no ANSI escape sequences in the output")
    if b"\x1b_" in output:
        return fail("sent an in-band native (APC) string to a plain terminal")
    os.write(fd, b"+")
    os.write(fd, b"+")
    if not read_until("Count: 2", 5):
        return fail("+ + did not reach Count: 2")
    os.write(fd, b"q")
    deadline = time.monotonic() + 5
    status = None
    while time.monotonic() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        read_until("\x00never", 0.05)
    else:
        return fail("q did not quit")
    with open(transcript, "wb") as f:
        f.write(output)
    if not os.WIFEXITED(status) or os.WEXITSTATUS(status) != 0:
        print(f"FAIL: exit status {status}", file=sys.stderr)
        return 1
    print(f"ansi: ok ({len(output)} bytes of output, transcript {transcript})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
