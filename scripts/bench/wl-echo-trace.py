#!/usr/bin/env python3
"""Per key press, what a Wayland client committed and when it was shown.

Reads a client's `WAYLAND_DEBUG=client` log (other lines, such as Ultty's
tracing output in the same file, are skipped) and prints, for every key press
the client received:

- whether a frame callback the client had asked for was still outstanding
  when the key arrived (winit then holds the next redraw until it comes);
- every content commit (a commit after `wl_surface.attach`) on the window's
  surface until the next key, with the queue that sent it;
- the frame callbacks that completed, and `wp_presentation_feedback`
  timestamps when the client's driver asks for them.

Times are milliseconds after the key, on the compositor's clock: the key
event carries the compositor's (or, with a virtual keyboard, the sender's)
CLOCK_MONOTONIC milliseconds, and the log's own timestamps are mapped onto
that clock with the offset of the earliest-delivered key event, so a mapped
time is late by at most that event's delivery delay (well under 1 ms on an
idle machine). Presentation timestamps are exact.

    wl-echo-trace.py LOG [--surface N]

The window's surface is the one with the most attaches unless --surface is
given.
"""
import argparse
import re
import sys
from collections import Counter

LINE = re.compile(
    r"^\[\s*(\d+\.\d+)\]\s+(?:\{([^}]*)\}\s+)?(->\s+)?(?:discarded\s+)?"
    r"([A-Za-z0-9_]+)#(\d+)\.(\w+)\((.*)\)\s*$"
)


def parse(path):
    rows = []
    with open(path, errors="replace") as f:
        for raw in f:
            m = LINE.match(raw)
            if m:
                t, queue, out, iface, oid, msg, args = m.groups()
                rows.append((float(t), queue or "default", bool(out), iface, int(oid), msg, args))
    return rows


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("log")
    ap.add_argument("--surface", type=int, help="wl_surface id of the window")
    args = ap.parse_args(argv)
    rows = parse(args.log)

    keys = []
    for t, _q, out, iface, _oid, msg, a in rows:
        if iface == "wl_keyboard" and msg == "key" and not out:
            _serial, time_ms, _code, state = (int(x) for x in a.split(", "))
            keys.append((t, time_ms, state))
    if not keys:
        sys.exit("no wl_keyboard.key events in the log")
    offset = max(time_ms - t for t, time_ms, _ in keys)
    presses = [time_ms for _t, time_ms, state in keys if state == 1]

    surface = args.surface
    if surface is None:
        attaches = Counter(oid for _t, _q, out, iface, oid, msg, _a in rows
                           if out and iface == "wl_surface" and msg == "attach")
        if not attaches:
            sys.exit("no wl_surface.attach in the log")
        surface = attaches.most_common(1)[0][0]

    events = []  # (ms on the compositor clock, kind, detail)
    callbacks = {}  # wl_callback id -> request time
    attached = False
    for t, queue, out, iface, oid, msg, a in rows:
        at = t + offset
        if iface == "wl_surface" and oid == surface and out:
            if msg == "attach":
                attached = True
            elif msg == "frame":
                cb = int(re.search(r"wl_callback#(\d+)", a).group(1))
                callbacks[cb] = at
                events.append((at, "frame-request", f"cb#{cb} [{queue}]"))
            elif msg == "commit":
                events.append((at, "COMMIT" if attached else "commit", f"[{queue}]"))
                attached = False
        elif iface == "wl_callback" and msg == "done" and not out and oid in callbacks:
            events.append((at, "frame-done", f"cb#{oid}"))
            del callbacks[oid]
        elif iface == "wp_presentation_feedback" and msg == "presented":
            v = [int(x) for x in a.split(", ")]
            shown = ((v[0] << 32 | v[1]) + v[2] / 1e9) * 1e3
            events.append((shown, "PRESENTED", f"msc {v[5]}"))
        elif iface == "wp_presentation_feedback" and msg == "discarded":
            events.append((at, "discarded", ""))
    events.sort()

    print(f"surface wl_surface#{surface}; log-to-compositor offset {offset:.3f} ms\n")
    table = []
    for i, k in enumerate(presses):
        end = presses[i + 1] if i + 1 < len(presses) else k + 1000.0
        before = [e for e in events if e[0] < k]
        requested = {d.split()[0] for _t, kind, d in before if kind == "frame-request"}
        done = {d for _t, kind, d in before if kind == "frame-done"}
        pending = sorted(requested - done)
        window = [(e[0] - k, e[1], e[2]) for e in events if k <= e[0] < end]
        print(f"key {i} at {k} ms; callbacks outstanding: {', '.join(pending) or 'none'}")
        for dt, kind, detail in window:
            print(f"  {dt:+9.2f}  {kind:<13} {detail}")
        content = [dt for dt, kind, _d in window if kind == "COMMIT"]
        first_done = next((dt for dt, kind, _d in window
                           if kind == "frame-done" and content and dt >= content[0]), None)
        shown = next((dt for dt, kind, _d in window if kind == "PRESENTED"), None)
        table.append((i, bool(pending), len(content), content[0] if content else None,
                      first_done, shown))

    def ms(v):
        return "-" if v is None else f"{v:.1f}"

    print("\nkey  cb-outstanding  content-commits  first-commit  next-frame-done  presented")
    for i, pending, n, first, done_at, shown in table:
        print(f"{i:3d}  {'yes' if pending else 'no':>14}  {n:15d}  {ms(first):>12}  "
              f"{ms(done_at):>15}  {ms(shown):>9}")


if __name__ == "__main__":
    main(sys.argv[1:])
