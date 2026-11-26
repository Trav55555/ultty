#!/usr/bin/env python3
"""Exec to first prompt pixels, with CPU first frames on and off (ADR 0030).

For each launch this records, in milliseconds from exec:

- `title_ms`: the window's name is the probe title (the shell set it);
- `prompt_ms`: the first grid row shows the prompt's pixels (what ADR 0030
  calls the first frame);
- `gpu_frame_ms`: the tab bar shows bright pixels. On `main` only the GPU
  renderer draws the tab bar's text, so this is the GPU's first frame on
  screen. Once the CPU frames draw the tab bar too (ADR 0030 slice 3) it
  is the first frame with chrome; use the `renderer handed over` stage
  for the GPU then;
- `blank_samples`: polls after the prompt first appeared in which it was gone
  again (a blank or torn frame at the handover; should be 0);
- the `ultty::startup` stage points from the log (`stages`);
- `VmHWM` and `VmRSS` one second after the prompt appeared (`mem_kib`).

    scripts/bench/first-prompt.py --display :0 --rounds 15 --out t4.jsonl \\
        off=target/release/ultty:ULTTY_CPU_FIRST_FRAMES=0 \\
        cpu=target/release/ultty:ULTTY_CPU_FIRST_FRAMES=1

Each ARM is `NAME=BINARY[:VAR=VALUE,...]`. Every round runs every arm once,
in a new random order. Each launch gets private XDG runtime, config, data,
state and cache directories (so the default config: no user font, theme or
session), no session bus and no WAYLAND_DISPLAY, and runs
`BINARY -e sh -c '<set title><print prompt>; sleep 8'`. The shell is plain
`sh`, so no rc file adds to the time. Pixels are read with XGetImage on
Ultty's own window (found by WM_CLASS `ultty`), so the window's placement
does not matter; it polls as fast as it can (sub-millisecond on an idle
machine). `--display` omitted starts a private Xvfb; with lavapipe that is
what the 2026-11-20 local numbers in ADR 0030 used.

Prints a summary table (median, p25-p75) when done; `--summary FILE` prints
it for an existing JSONL. Needs only python3 and libX11 (ctypes).

## On the T4 (or any GPU host with an X server)

Nothing here starts, stops or changes an instance. On a host that already
runs Xorg on the GPU as `:0` (as `scripts/bench-aws.sh --role gpu` sets up),
with nothing else drawing on it:

    git checkout <commit to measure>
    cargo build --release -p ultty
    cp target/release/ultty /tmp/ultty-candidate
    # optional baseline without the D4 and ADR 0030 work:
    git worktree add /tmp/ultty-base 0a5b77b
    (cd /tmp/ultty-base && cargo build --release -p ultty)
    scripts/bench/first-prompt.py --display :0 --rounds 15 \\
        --out first-prompt-x11.jsonl \\
        base=/tmp/ultty-base/target/release/ultty \\
        off=/tmp/ultty-candidate:ULTTY_CPU_FIRST_FRAMES=0 \\
        cpu=/tmp/ultty-candidate:ULTTY_CPU_FIRST_FRAMES=1

Round 0 is a cold start (page cache) and is left out of the summary; 15
rounds leaves 14 per arm. Collect the JSONL, the printed table and
`nvidia-smi --query-gpu=name,driver_version --format=csv`. The numbers that
answer ADR 0030 are `prompt_ms` for `off` against `cpu` (target: `cpu`
under 250 ms), `blank_samples` (target: 0 in every launch), and the stage
points `first frame showing shell output (CPU)`, `GPU device ready`,
`renderer handed over` and `drawing the first frame`. The steady-state rows
(keypress to pixel, throughput, RSS) come from `scripts/bench/compare.sh`
as usual, once with `ULTTY_CPU_FIRST_FRAMES=1` exported and once without.

X11 only. On Wayland (the private headless sway that ADR 0030's T4 run
used) there is no cheap pixel read; take the title from `swaymsg -t
get_tree` and the prompt from the `first frame showing shell output (CPU)`
stage, which on X11 lands within about 1 ms of the pixels.

The pixel regions assume scale 1 and the default font: the first grid row
is y 45-62 and the tab bar y 0-42, in window pixels. `--calibrate DIR`
writes a PNG of the window two seconds after each launch, to check them.
"""
import argparse
import ctypes
import ctypes.util
import json
import os
import random
import re
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from datetime import datetime, timezone

# Test and benchmark runs stay out of the user's Ultty log file.
os.environ.setdefault("ULTTY_LOG_FILE", "0")

TITLE = "first-prompt-probe"
PROMPT = "startup-probe-prompt$ "
LAUNCH_TIMEOUT_S = 30
SETTLE_S = 1.0
BRIGHT = 450  # r + g + b: light text on the dark default theme
ENOUGH = 20  # bright pixels that make a row "drawn"
PROMPT_REGION = (16, 110, 45, 62)  # x0, x1, y0, y1 in window pixels
TAB_BAR_REGION = (16, 110, 0, 42)


class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int), ("data", ctypes.POINTER(ctypes.c_ubyte)),
        ("byte_order", ctypes.c_int), ("bitmap_unit", ctypes.c_int),
        ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int), ("red_mask", ctypes.c_ulong),
        ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong),
        ("obdata", ctypes.c_void_p),
        ("create_image", ctypes.c_void_p),
        ("destroy_image", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)),
    ]


class XClassHint(ctypes.Structure):
    # Pointers, not c_char_p: they are XFree'd, which needs the originals.
    _fields_ = [("res_name", ctypes.c_void_p), ("res_class", ctypes.c_void_p)]


ERROR_HANDLER = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)


class X:
    """The few Xlib calls the probe needs."""

    def __init__(self, display):
        lib = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
        lib.XOpenDisplay.restype = ctypes.c_void_p
        lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        lib.XDefaultRootWindow.restype = ctypes.c_ulong
        lib.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        lib.XQueryTree.argtypes = [
            ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
            ctypes.POINTER(ctypes.c_uint)]
        lib.XGetClassHint.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(XClassHint)]
        lib.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong,
                                   ctypes.POINTER(ctypes.c_void_p)]
        lib.XFree.argtypes = [ctypes.c_void_p]
        lib.XGetImage.restype = ctypes.POINTER(XImage)
        lib.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                  ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        lib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        self.lib = lib
        # XGetImage on a window that is not viewable yet is a BadMatch;
        # Xlib's default handler would exit.
        self._handler = ERROR_HANDLER(lambda _d, _e: 0)
        lib.XSetErrorHandler(self._handler)
        # A server that has just started may not take connections yet.
        for _ in range(50):
            self.dpy = lib.XOpenDisplay(display.encode())
            if self.dpy:
                break
            time.sleep(0.1)
        if not self.dpy:
            sys.exit(f"cannot open display {display}")
        self.root = lib.XDefaultRootWindow(self.dpy)

    def children(self, window):
        root, parent = ctypes.c_ulong(), ctypes.c_ulong()
        kids, n = ctypes.POINTER(ctypes.c_ulong)(), ctypes.c_uint()
        if not self.lib.XQueryTree(self.dpy, window, ctypes.byref(root), ctypes.byref(parent),
                                   ctypes.byref(kids), ctypes.byref(n)):
            return []
        out = [kids[i] for i in range(n.value)]
        if kids:
            self.lib.XFree(ctypes.cast(kids, ctypes.c_void_p))
        return out

    def find_ultty(self, window=None, depth=0):
        """The first window with WM_CLASS `ultty`, under any reparenting."""
        window = self.root if window is None else window
        for child in self.children(window):
            hint = XClassHint()
            if self.lib.XGetClassHint(self.dpy, child, ctypes.byref(hint)):
                name = ctypes.string_at(hint.res_name) if hint.res_name else None
                for p in (hint.res_name, hint.res_class):
                    if p:
                        self.lib.XFree(p)
                if name == b"ultty":
                    return child
            if depth < 2:
                found = self.find_ultty(child, depth + 1)
                if found:
                    return found
        return None

    def name(self, window):
        out = ctypes.c_void_p()
        if not self.lib.XFetchName(self.dpy, window, ctypes.byref(out)) or not out.value:
            return None
        value = ctypes.string_at(out.value).decode(errors="replace")
        self.lib.XFree(out)
        return value

    def image(self, window, width, height):
        """(bytes, bytes_per_line) of the window's top-left corner, or None."""
        img = self.lib.XGetImage(self.dpy, window, 0, 0, width, height, ~0 & 0xFFFFFFFF, 2)
        if not img:
            return None
        ximg = img.contents
        if ximg.bits_per_pixel != 32:
            ximg.destroy_image(ctypes.cast(img, ctypes.c_void_p))
            return None
        data = ctypes.string_at(ximg.data, ximg.bytes_per_line * ximg.height)
        bpl = ximg.bytes_per_line
        ximg.destroy_image(ctypes.cast(img, ctypes.c_void_p))
        return data, bpl


def bright(data, bpl, region):
    x0, x1, y0, y1 = region
    n = 0
    for y in range(y0, y1):
        row = data[y * bpl + x0 * 4: y * bpl + x1 * 4]
        n += sum(1 for i in range(0, len(row), 4) if row[i] + row[i + 1] + row[i + 2] > BRIGHT)
    return n


def write_png(path, data, bpl, width, height):
    rows = []
    for y in range(height):
        row = data[y * bpl: y * bpl + width * 4]
        rows.append(b"\x00" + bytes(b for i in range(0, len(row), 4)
                                    for b in (row[i + 2], row[i + 1], row[i])))

    def chunk(kind, body):
        return (struct.pack(">I", len(body)) + kind + body
                + struct.pack(">I", zlib.crc32(kind + body)))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n"
                + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(b"".join(rows), 1)) + chunk(b"IEND", b""))


STAGE = re.compile(r"^(\S+Z)\s+\w+\s+ultty::startup: (.*)$")


def stages(log_path, t0_wall):
    out = {}
    for line in open(log_path, errors="replace"):
        match = STAGE.match(re.sub(r"\x1b\[[0-9;]*m", "", line).rstrip())
        if not match:
            continue
        ts = datetime.strptime(match.group(1)[:26].rstrip("Z"), "%Y-%m-%dT%H:%M:%S.%f")
        out.setdefault(match.group(2),
                       round((ts.replace(tzinfo=timezone.utc).timestamp() - t0_wall) * 1000, 2))
    return out


def memory(pid):
    mem = {}
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith(("VmHWM", "VmRSS")):
                mem[line.split(":")[0]] = int(line.split()[1])
    except OSError:
        pass
    return mem


def launch(x, display, name, binary, extra_env, run_dir, calibrate):
    env = {k: v for k, v in os.environ.items()
           if k not in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS")}
    for d in ("runtime", "config", "data", "state", "cache"):
        os.makedirs(os.path.join(run_dir, d), mode=0o700)
    env.update(DISPLAY=display, XDG_RUNTIME_DIR=f"{run_dir}/runtime",
               XDG_CONFIG_HOME=f"{run_dir}/config", XDG_DATA_HOME=f"{run_dir}/data",
               XDG_STATE_HOME=f"{run_dir}/state", XDG_CACHE_HOME=f"{run_dir}/cache",
               RUST_LOG="ultty::startup=debug,info")
    env.update(extra_env)
    script = f"printf '\\033]2;{TITLE}\\007{PROMPT}'; sleep 8"
    log_path = os.path.join(run_dir, "log")
    log = open(log_path, "w")
    t0_wall = time.time()
    t0 = time.monotonic()
    proc = subprocess.Popen([binary, "-e", "sh", "-c", script], env=env, stdout=log,
                            stderr=log, start_new_session=True)
    rec = {"arm": name, "title_ms": None, "prompt_ms": None, "gpu_frame_ms": None,
           "blank_samples": 0, "samples": 0}
    window = None
    width, height = PROMPT_REGION[1], PROMPT_REGION[3]
    deadline = None
    try:
        while proc.poll() is None:
            now = time.monotonic()
            if now - t0 > LAUNCH_TIMEOUT_S or (deadline and now > deadline):
                break
            if window is None:
                window = x.find_ultty()
                if window is None:
                    continue
            ms = (time.monotonic() - t0) * 1000
            if rec["title_ms"] is None and x.name(window) == TITLE:
                rec["title_ms"] = ms
            shot = x.image(window, width, height)
            if shot is None:
                continue
            rec["samples"] += 1
            prompt = bright(shot[0], shot[1], PROMPT_REGION) >= ENOUGH
            if prompt and rec["prompt_ms"] is None:
                rec["prompt_ms"] = ms
                deadline = time.monotonic() + SETTLE_S
            elif not prompt and rec["prompt_ms"] is not None:
                rec["blank_samples"] += 1
            if rec["gpu_frame_ms"] is None and bright(shot[0], shot[1], TAB_BAR_REGION) >= ENOUGH:
                rec["gpu_frame_ms"] = ms
            # Past the prompt, keep watching until the GPU has drawn too.
            if deadline and now > deadline and rec["gpu_frame_ms"] is None:
                deadline = now + 0.1
        rec["mem_kib"] = memory(proc.pid)
        if calibrate and window is not None:
            time.sleep(max(0.0, 2.0 - (time.monotonic() - t0)))
            full = x.image(window, 640, 200)
            if full:
                write_png(os.path.join(calibrate, f"{os.path.basename(run_dir)}.png"),
                          full[0], full[1], 640, 200)
    finally:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
        log.close()
    rec["stages"] = stages(log_path, t0_wall)
    return rec


SUMMARY_STAGES = [
    "shell started",
    "first frame showing shell output (CPU)",
    "first frame showing shell output (GPU)",
    "GPU device ready",
    "renderer handed over",
    "drawing the first frame",
]


def summarize(records):
    records = [r for r in records if r.get("round", 1) > 0]

    def cell(values):
        values = sorted(v for v in values if v is not None)
        if not values:
            return "-"
        n = len(values)
        return (f"{statistics.median(values):.1f} "
                f"({values[n // 4]:.1f}-{values[(3 * n) // 4]:.1f}, n={n})")

    arms = list(dict.fromkeys(r["arm"] for r in records))
    rows = [("exec to titled window", lambda r: r["title_ms"]),
            ("exec to first prompt pixels", lambda r: r["prompt_ms"]),
            ("exec to tab bar pixels", lambda r: r["gpu_frame_ms"])]
    rows += [(f"stage: {s}", lambda r, s=s: r["stages"].get(s)) for s in SUMMARY_STAGES]
    rows += [("VmHWM MiB", lambda r: r.get("mem_kib", {}).get("VmHWM", 0) / 1024 or None),
             ("blank samples after the prompt (sum)", None)]
    print("| ms from exec, median (p25-p75) | " + " | ".join(arms) + " |")
    print("|---|" + "---:|" * len(arms))
    for label, get in rows:
        if get is None:
            vals = [str(sum(r["blank_samples"] for r in records if r["arm"] == a)) for a in arms]
        else:
            vals = [cell([get(r) for r in records if r["arm"] == a]) for a in arms]
        print(f"| {label} | " + " | ".join(vals) + " |")
    print("\nRound 0 (cold start) is left out.")


def start_xvfb(work):
    read_fd, write_fd = os.pipe()
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0",
                             "1280x800x24", "-br", "-nolisten", "tcp"],
                            pass_fds=(write_fd,), stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    os.close(write_fd)
    display = os.read(read_fd, 32).decode().strip()
    os.close(read_fd)
    if not display:
        sys.exit("Xvfb did not start")
    return xvfb, f":{display}"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("arms", nargs="*", metavar="NAME=BINARY[:VAR=VALUE,...]")
    parser.add_argument("--display", help="X display to use (default: a private Xvfb)")
    parser.add_argument("--rounds", type=int, default=15)
    parser.add_argument("--out", default="first-prompt.jsonl")
    parser.add_argument("--calibrate", metavar="DIR", help="write a PNG of each window")
    parser.add_argument("--summary", metavar="JSONL", help="only summarize an existing run")
    args = parser.parse_args()
    if args.summary:
        summarize([json.loads(line) for line in open(args.summary)])
        return
    if not args.arms:
        parser.error("no arms")
    arms = []
    for spec in args.arms:
        name, _, rest = spec.partition("=")
        binary, _, extra = rest.partition(":")
        env = dict(kv.split("=", 1) for kv in filter(None, extra.split(",")))
        arms.append((name, os.path.abspath(binary), env))
    if args.calibrate:
        os.makedirs(args.calibrate, exist_ok=True)
    work = tempfile.mkdtemp(prefix="ultty-first-prompt-")
    xvfb = None
    display = args.display
    if display is None:
        xvfb, display = start_xvfb(work)
        # The private server has no access control; the desktop's cookie
        # is not for it.
        os.environ.pop("XAUTHORITY", None)
    x = X(display)
    if x.find_ultty() is not None:
        sys.exit(f"an Ultty window is already open on {display}; close it first")
    records = []
    try:
        with open(args.out, "a") as out:
            for rnd in range(args.rounds):
                order = list(arms)
                random.shuffle(order)
                for name, binary, env in order:
                    run_dir = os.path.join(work, f"{name}-{rnd}")
                    rec = launch(x, display, name, binary, env, run_dir, args.calibrate)
                    rec["round"] = rnd
                    records.append(rec)
                    out.write(json.dumps(rec) + "\n")
                    out.flush()
                    print(f"round {rnd} {name}: title {rec['title_ms'] and round(rec['title_ms'])}"
                          f" prompt {rec['prompt_ms'] and round(rec['prompt_ms'])}"
                          f" tab bar {rec['gpu_frame_ms'] and round(rec['gpu_frame_ms'])}"
                          f" blank {rec['blank_samples']}/{rec['samples']}", flush=True)
                    time.sleep(0.3)
    finally:
        if xvfb:
            xvfb.terminate()
            xvfb.wait()
        shutil.rmtree(work, ignore_errors=True)
    print()
    summarize(records)


if __name__ == "__main__":
    main()
