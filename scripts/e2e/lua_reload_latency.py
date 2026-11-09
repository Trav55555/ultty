"""Measure ulttui-lua's reload loop: file write to the next drawn frame.

    python3 scripts/e2e/lua_reload_latency.py --script FILE --log LOG \
        [--spawn ULTTUI_LUA] [--count 20] [--old TEXT]

Edits FILE --count times, each time replacing --old (default "Hello from
Luau") with a numbered label and saving the way editors do (write a new
file, rename it over the old one). For each save it waits for the
"frame after reload" line ulttui-lua writes to LOG when the frame after the
reload has been handed to the terminal, and takes the latency as that
line's frame_unix_us minus the wall-clock time of the rename.

With --spawn, the harness starts ULTTUI_LUA on FILE itself, on an 80x24 pty
outside Ultty (ANSI mode). Without it, ulttui-lua must already be running
on FILE with --log LOG (the native e2e starts it inside Ultty).

Prints one line per reload and the median, p90 and max, and the medians of
the parts the log reports: wait (change seen -> reload starts: the settle
delay and the event loop's next tick), reload (compile, init and trial
view) and render (building and drawing the frame).
"""

import argparse
import os
import pty
import re
import select
import statistics
import struct
import sys
import threading
import time
import fcntl
import termios

FRAME = re.compile(
    r"frame after reload seen_to_frame_us=(\d+) wait_us=(\d+) reload_us=(\d+) "
    r"render_us=(\d+) frame_unix_us=(\d+)"
)


def frames(log: str) -> list[tuple[int, ...]]:
    try:
        with open(log) as f:
            return [tuple(map(int, m.groups())) for m in FRAME.finditer(f.read())]
    except FileNotFoundError:
        return []


def spawn(binary: str, script: str, log: str) -> int:
    env = {k: v for k, v in os.environ.items() if not k.startswith("ULTTY")}
    env["ULTTY_IN_BAND"] = "0"
    env["TERM"] = "xterm-256color"
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(binary, [binary, "--log", log, script], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    def drain() -> None:
        while True:
            ready, _, _ = select.select([fd], [], [], 0.5)
            if ready:
                try:
                    if not os.read(fd, 65536):
                        return
                except OSError:
                    return

    threading.Thread(target=drain, daemon=True).start()
    return pid


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--script", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--spawn")
    parser.add_argument("--count", type=int, default=20)
    parser.add_argument("--old", default="Hello from Luau")
    args = parser.parse_args()

    with open(args.script) as f:
        original = f.read()
    if args.old not in original:
        print(f"{args.script} does not contain {args.old!r}", file=sys.stderr)
        return 2

    pid = None
    if args.spawn:
        if os.path.exists(args.log):
            os.remove(args.log)
        pid = spawn(args.spawn, args.script, args.log)
        time.sleep(1.0)  # first load and first frame

    results = []
    try:
        for i in range(1, args.count + 1):
            before = len(frames(args.log))
            tmp = args.script + ".new"
            with open(tmp, "w") as f:
                f.write(original.replace(args.old, f"{args.old} #{i}"))
            written_us = time.time_ns() // 1000
            os.rename(tmp, args.script)
            deadline = time.monotonic() + 5
            while len(frames(args.log)) <= before:
                if time.monotonic() > deadline:
                    print(f"reload {i}: no frame within 5 s", file=sys.stderr)
                    return 1
                time.sleep(0.005)
            seen_to_frame, wait, reload, render, frame_unix = frames(args.log)[-1]
            total = frame_unix - written_us
            results.append((total, seen_to_frame, wait, reload, render))
            print(
                f"reload {i:2}: write->frame {total / 1000:6.1f} ms "
                f"(seen->frame {seen_to_frame / 1000:5.1f}: wait {wait / 1000:5.1f}, "
                f"reload {reload / 1000:5.1f}, render {render / 1000:5.1f})"
            )
            # Let the next save stand on its own, not coalesce with this one.
            time.sleep(0.15)
    finally:
        with open(args.script, "w") as f:
            f.write(original)
        if pid:
            os.kill(pid, 15)
            os.waitpid(pid, 0)

    totals = sorted(r[0] for r in results)
    med = lambda k: statistics.median(r[k] for r in results) / 1000
    p90 = totals[max(0, int(len(totals) * 0.9) - 1)] / 1000
    print(
        f"write->frame over {len(results)} reloads: median {med(0):.1f} ms, "
        f"p90 {p90:.1f} ms, max {totals[-1] / 1000:.1f} ms; medians: "
        f"write->seen {med(0) - med(1):.1f}, wait {med(2):.1f}, reload {med(3):.1f}, "
        f"render {med(4):.1f} ms"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
