#!/usr/bin/env python3
"""Control-plane latency of a running Ultty (plan item D6).

    scripts/bench/control-latency.py [--pid PID] [-n 2000]

Measures, against the Ultty named by --pid or $ULTTY_PID:

  request   connect, send `ping` with the token, read the reply to EOF:
            what one control request costs a client that is already running
            (ulttyctl adds its own process start on top).
  ulttyctl  the same through the `ulttyctl ping` binary, start to exit.
  event     a subscriber's view of a notification: type OSC 9 into a
            background pane running `cat`, which echoes it; time from sending
            the request to the event arriving on the subscription.
  event+draw  the same with the pane on screen and a title change, so each
            event also draws a frame on the UI thread.

Prints median, p99 and max in milliseconds for each.
"""

import argparse
import json
import os
import socket
import statistics
import subprocess
import sys
import time


def control_paths(pid):
    base = os.environ.get("XDG_RUNTIME_DIR") or os.path.join("/tmp", f"ultty-{os.getuid()}")
    return (
        os.path.join(base, f"ultty-ctl-{pid}.sock"),
        os.path.join(base, f"ultty-auth-{pid}.token"),
    )


def envelope(token, request):
    return json.dumps(
        {"output_format": "json", "request": {"type": "with_token", "token": token, "request": request}}
    ).encode()


def call(sock_path, token, request):
    """One request on its own connection, as ulttyctl sends it."""
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(5)
    s.connect(sock_path)
    s.sendall(envelope(token, request))
    s.shutdown(socket.SHUT_WR)
    chunks = []
    while chunk := s.recv(65536):
        chunks.append(chunk)
    s.close()
    return json.loads(b"".join(chunks))


def summary(name, samples_s):
    ms = sorted(x * 1000 for x in samples_s)
    p99 = ms[min(len(ms) - 1, int(len(ms) * 0.99))]
    print(flush=True, end="")
    print(f"{name:10} n={len(ms):5}  median {statistics.median(ms):7.3f} ms  "
          f"p99 {p99:7.3f} ms  max {ms[-1]:7.3f} ms")


def bench_requests(sock_path, token, n):
    for _ in range(50):
        call(sock_path, token, {"type": "ping"})
    out = []
    for _ in range(n):
        t = time.perf_counter()
        reply = call(sock_path, token, {"type": "ping"})
        out.append(time.perf_counter() - t)
        assert reply.get("status") != "error", reply
    return out


def bench_ulttyctl(pid, n):
    env = dict(os.environ, ULTTY_PID=str(pid))
    out = []
    for _ in range(n):
        t = time.perf_counter()
        subprocess.run(["ulttyctl", "ping"], env=env, check=True, stdout=subprocess.DEVNULL)
        out.append(time.perf_counter() - t)
    return out


def bench_events(sock_path, token, n, hidden):
    """Time from a send_text request to the event its echo raises.

    Visible: the pane is on screen and the probe sets its title, so each
    event also redraws the window on the same thread. Hidden: the pane is in
    a background tab and the probe is a notification, which an unfocused
    pane does not show, so nothing is drawn and only the event path counts.
    """
    tab = call(sock_path, token, {
        "type": "new_tab",
        "command": "/bin/sh",
        # Raw and without echo, so cat sends the probe back unchanged.
        "args": ["-c", "stty raw -echo; cat"],
    })
    pane = tab["pane"]
    time.sleep(0.5)
    if hidden:
        call(sock_path, token, {"type": "switch_tab", "index": 0})
    sub = socket.socket(socket.AF_UNIX)
    sub.settimeout(5)
    sub.connect(sock_path)
    sub.sendall(envelope(token, {"type": "subscribe", "target": pane}) + b"\n")
    reader = sub.makefile("rb")
    assert json.loads(reader.readline()).get("type") == "subscribed"
    wanted = "notification" if hidden else "title_changed"
    out = []
    try:
        for i in range(n + 20):
            probe = "\x1b]9;probe\x07" if hidden else f"\x1b]2;probe {i}\x07"
            t = time.perf_counter()
            call(sock_path, token, {"type": "send_text", "text": probe, "target": pane})
            while json.loads(reader.readline()).get("event") != wanted:
                pass
            if i >= 20:
                out.append(time.perf_counter() - t)
    finally:
        call(sock_path, token, {"type": "close_tab", "index": tab["tab_index"]})
        sub.close()
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pid", type=int, default=int(os.environ.get("ULTTY_PID", "0")) or None)
    parser.add_argument("-n", type=int, default=2000)
    args = parser.parse_args()
    if not args.pid:
        sys.exit("name the Ultty with --pid or ULTTY_PID")
    sock_path, token_path = control_paths(args.pid)
    with open(token_path) as f:
        token = f.read().strip()
    summary("request", bench_requests(sock_path, token, args.n))
    summary("ulttyctl", bench_ulttyctl(args.pid, min(args.n, 300)))
    summary("event", bench_events(sock_path, token, min(args.n, 500), hidden=True))
    summary("event+draw", bench_events(sock_path, token, min(args.n, 500), hidden=False))


if __name__ == "__main__":
    main()
