#!/usr/bin/env python3
"""Keypress-to-pixels latency on a wlroots compositor (sway), stdlib only.

    python3 -I k2p-wayland.py --title T --terminal NAME --out results.jsonl
                              [--samples N] [--case NAME] [--claim CLASS]

The terminal must run `ubench-echo`. Keys are injected with `wtype`
(virtual-keyboard protocol) and the echo cell is polled with `grim`
(wlr-screencopy). Both are separate processes, so the resolution is coarse:
each poll costs a process spawn plus a full screencopy, typically 10-40 ms.
`resolution_ms` (median poll time) and `inject_ms` (median wtype run time)
are recorded with every sample; treat differences smaller than the
resolution as noise. The latency is measured from wtype's exit, so it can
miss up to `inject_ms` at the start. Use the X11 probe (`ubench-k2p`) for
sub-millisecond numbers.
"""

import argparse
import json
import random
import subprocess
import sys
import time

HERE = __file__.rsplit("/", 1)[0]


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, **kw).stdout


def window_rect(title):
    tree = run(["swaymsg", "-t", "get_tree"])
    out = subprocess.run([sys.executable, "-I", f"{HERE}/sway_find.py", title, "rect"],
                         input=tree, capture_output=True, check=True).stdout.decode().strip()
    xy, wh = out.split(" ")
    x, y = (int(v) for v in xy.split(","))
    w, h = (int(v) for v in wh.split("x"))
    return x, y, w, h


def grab(rect):
    x, y, w, h = rect
    data = run(["grim", "-g", f"{x},{y} {w}x{h}", "-t", "ppm", "-"])
    # P6\n<w> <h>\n<max>\n<rgb...>
    fields = []
    pos = 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    pos += 1
    return int(fields[1]), int(fields[2]), data[pos:]


def changed_box(before, after, rect):
    w, _, a = before
    _, _, b = after
    x0 = y0 = None
    x1 = y1 = 0
    for i in range(0, min(len(a), len(b)), 3):
        if a[i:i + 3] != b[i:i + 3]:
            px, py = (i // 3) % w, (i // 3) // w
            x0 = px if x0 is None else min(x0, px)
            y0 = py if y0 is None else min(y0, py)
            x1, y1 = max(x1, px), max(y1, py)
    if x0 is None:
        return None
    return rect[0] + x0, rect[1] + y0, x1 - x0 + 1, y1 - y0 + 1


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--terminal", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--samples", type=int, default=200)
    ap.add_argument("--case", default="idle")
    ap.add_argument("--claim", default="live-unknown")
    ap.add_argument("--timeout", type=float, default=2.0)
    ap.add_argument("--start-key", help="press this after calibrating (ubench-echo --flood waits for f)")
    args = ap.parse_args(argv)

    x, y, w, h = window_rect(args.title)
    run(["swaymsg", f'[title="^{args.title}$"] focus'])
    # Calibrate on the bottom quarter (ubench-echo draws on the last row),
    # once it is still, and check a second key changes the same box.
    search = (x, y + h - h // 4, w, h // 4)
    before = grab(search)
    for _ in range(20):
        time.sleep(0.25)
        again = grab(search)
        if again == before:
            break
        before = again
    run(["wtype", "x"])
    time.sleep(0.8)
    cell = changed_box(before, grab(search), search)
    if cell is None:
        print("no pixels changed after a key press; is ubench-echo running?", file=sys.stderr)
        return 1
    first = grab(cell)
    run(["wtype", "o"])
    time.sleep(0.8)
    if grab(cell) == first:
        print(f"calibration box {cell} did not change on a second key", file=sys.stderr)
        return 1
    if args.start_key:
        run(["wtype", args.start_key])
        time.sleep(1.5)

    rng = random.Random(1)
    latencies, polls, injects = [], [], []
    timeouts = 0
    for i in range(args.samples):
        time.sleep(rng.uniform(0.05, 0.3))
        baseline = grab(cell)[2]
        t_inject = time.monotonic()
        run(["wtype", "x" if i % 2 == 0 else "o"])
        start = time.monotonic()
        injects.append((start - t_inject) * 1e3)
        while True:
            t = time.monotonic()
            now = grab(cell)[2]
            polls.append((time.monotonic() - t) * 1e3)
            if now != baseline:
                latencies.append((time.monotonic() - start) * 1e3)
                break
            if time.monotonic() - start > args.timeout:
                timeouts += 1
                break

    polls.sort()
    injects.sort()
    resolution = polls[len(polls) // 2] if polls else None
    inject = injects[len(injects) // 2] if injects else None
    with open(args.out, "a", encoding="utf-8") as fh:
        for block, ms in enumerate(latencies):
            fh.write(json.dumps({
                "suite": "lat.k2p.external", "case": args.case, "arm": args.terminal,
                "block": block, "value": ms, "unit": "ms", "method": "wtype+grim",
                "resolution_ms": resolution, "inject_ms": inject, "claim": args.claim,
                "env": {"session": "wayland"},
            }) + "\n")
    s = sorted(latencies)
    print(f"k2p-wayland {args.terminal}: n={len(s)} timeouts={timeouts} "
          f"p50={s[len(s) // 2] if s else None} resolution={resolution} ms", file=sys.stderr)
    if not latencies or timeouts * 10 > args.samples:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
