#!/usr/bin/env python3
"""Keypress-to-pixels latency on a wlroots compositor (sway), stdlib only.

    python3 -I k2p-wayland.py --title T --terminal NAME --out results.jsonl
                              [--samples N] [--case NAME] [--claim CLASS]

The terminal must run `ubench-echo`. The probe is a small Wayland client of
its own: it keeps one virtual keyboard (zwp_virtual_keyboard_v1) for the
whole run and reads the echo cell with wlr-screencopy (zwlr_screencopy_v1),
both over one connection.

Why one long-lived keyboard: a headless sway has no keyboard, so its seat
advertises no keyboard capability. A one-shot `wtype` creates a keyboard,
sends the key and destroys the keyboard at once; the terminal only binds
wl_keyboard after the capability appears, so the key is gone before it
listens. Every Wayland sample failed that way ("no pixels changed").

What it measures: from the key request leaving the probe to the first
composited output frame in which the echo cell differs. The end time is the
compositor's own timestamp for that frame (screencopy `ready`, CLOCK_MONOTONIC,
the same clock as the start), not when the probe read it, so the probe's
read-back cost is not in the number. The compositor only produces pixels on
output frames, so a change is seen at the first frame after the terminal
commits: latency includes the wait for the compositor's next frame, as on any
Wayland desktop, and its resolution is one output frame (`frame_ms`, recorded
with every sample). Frames are requested with copy_with_damage, which
completes on the next frame with damage. Any copy request also makes wlroots
draw a frame at once, so the first request is sent well before the key (see
arm_quietly): sent after the key, its frame used up the idle output and
every echo waited one more frame (`--arm after-key` keeps that order for
comparison). `received_ms` is the same sample timed at the probe's receipt,
as a cross-check; nothing is subtracted from either.
"""

import argparse
import array
import json
import mmap
import os
import random
import select
import socket
import struct
import subprocess
import sys
import time

HERE = __file__.rsplit("/", 1)[0]
Y_INVERT = 1


def now():
    return time.clock_gettime(time.CLOCK_MONOTONIC)


def u32(*values):
    return struct.pack(f"<{len(values)}I", *values)


def i32(*values):
    return struct.pack(f"<{len(values)}i", *values)


def wl_string(text):
    raw = text.encode() + b"\0"
    return u32(len(raw)) + raw + b"\0" * (-len(raw) % 4)


def read_string(payload, pos):
    (length,) = struct.unpack_from("<I", payload, pos)
    text = payload[pos + 4:pos + 4 + length - 1].decode()
    return text, pos + 4 + length + (-length % 4)


class Connection:
    """The Wayland wire protocol: requests out, events to per-object handlers."""

    def __init__(self):
        name = os.environ.get("WAYLAND_DISPLAY", "wayland-0")
        path = name if name.startswith("/") else os.path.join(os.environ["XDG_RUNTIME_DIR"], name)
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.next_id = 2
        self.handlers = {1: self._display_event}
        self.pending = b""

    def new_id(self, handler):
        oid = self.next_id
        self.next_id += 1
        self.handlers[oid] = handler
        return oid

    def request(self, obj, opcode, args=b"", fd=None):
        msg = u32(obj, (8 + len(args)) << 16 | opcode) + args
        if fd is None:
            self.sock.sendall(msg)
        else:
            self.sock.sendmsg([msg], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))])

    def roundtrip(self):
        done = []
        callback = self.new_id(lambda _op, _p: done.append(True))
        self.request(1, 0, u32(callback))  # wl_display.sync
        while not done:
            self.dispatch(None)

    def dispatch(self, timeout):
        """Read and handle events; False if none arrived within timeout."""
        ready, _, _ = select.select([self.sock], [], [], timeout)
        if not ready:
            return False
        data, ancdata, _, _ = self.sock.recvmsg(1 << 16, socket.CMSG_SPACE(32 * 4))
        for level, kind, fds in ancdata:  # no event we bind carries an fd
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                for fd in array.array("i", fds[:len(fds) - len(fds) % 4]):
                    os.close(fd)
        if not data:
            raise ConnectionError("compositor closed the connection")
        self.pending += data
        while len(self.pending) >= 8:
            obj, size_op = struct.unpack_from("<II", self.pending)
            size = size_op >> 16
            if len(self.pending) < size:
                break
            payload, self.pending = self.pending[8:size], self.pending[size:]
            handler = self.handlers.get(obj)
            if handler:
                handler(size_op & 0xFFFF, payload)
        return True

    def _display_event(self, opcode, payload):
        if opcode == 0:  # error
            obj, code = struct.unpack_from("<II", payload)
            message, _ = read_string(payload, 8)
            raise RuntimeError(f"Wayland protocol error on object {obj} (code {code}): {message}")


class ShmBuffer:
    def __init__(self, conn, shm, fmt, width, height, stride):
        size = stride * height
        fd = os.memfd_create("k2p-wayland")
        os.ftruncate(fd, size)
        self.map = mmap.mmap(fd, size)
        pool = conn.new_id(None)
        conn.request(shm, 0, u32(pool) + i32(size), fd=fd)  # wl_shm.create_pool(id, fd, size)
        os.close(fd)
        self.id = conn.new_id(None)
        conn.request(pool, 0, u32(self.id) + i32(0, width, height, stride) + u32(fmt))
        conn.request(pool, 1)  # wl_shm_pool.destroy; the buffer keeps the memory


class Frame:
    """One zwlr_screencopy_frame_v1; collects its events."""

    def __init__(self, probe, region):
        self.buffer_info = None
        self.flags = 0
        self.ready = None  # compositor timestamp, seconds, CLOCK_MONOTONIC
        self.received = None
        self.failed = False
        self.described = False
        self.version = probe.screencopy_version
        conn = probe.conn
        self.id = conn.new_id(self._event)
        x, y, w, h = region
        # capture_output_region(frame, overlay_cursor, output, x, y, w, h)
        conn.request(probe.screencopy, 1, u32(self.id) + i32(0) + u32(probe.output) + i32(x, y, w, h))

    def _event(self, opcode, payload):
        if opcode == 0:  # buffer(format, width, height, stride): shm
            self.buffer_info = struct.unpack_from("<IIII", payload)
            if self.version < 3:
                self.described = True
        elif opcode == 1:
            (self.flags,) = struct.unpack_from("<I", payload)
        elif opcode == 2:
            hi, lo, nsec = struct.unpack_from("<III", payload)
            self.ready = (hi << 32 | lo) + nsec / 1e9
            self.received = now()
        elif opcode == 3:
            self.failed = True
            self.received = now()
        elif opcode == 6:  # buffer_done (v3)
            self.described = True

    @property
    def done(self):
        return self.ready is not None or self.failed


class Probe:
    def __init__(self, keys):
        self.conn = Connection()
        self.globals = {}
        registry = self.conn.new_id(self._global)
        self.conn.request(1, 1, u32(registry))  # wl_display.get_registry
        self.conn.roundtrip()
        need = ["wl_shm", "wl_seat", "wl_output", "zwlr_screencopy_manager_v1",
                "zwp_virtual_keyboard_manager_v1"]
        missing = [n for n in need if n not in self.globals]
        if missing:
            raise RuntimeError(f"compositor lacks {', '.join(missing)}")
        self.output_x = self.output_y = 0
        self.refresh_mhz = 0
        self.output_name = None
        self.shm = self._bind(registry, "wl_shm", 1)
        seat = self._bind(registry, "wl_seat", 1)
        self.output = self._bind(registry, "wl_output", 4, self._output_event)
        self.screencopy_version = min(self.globals["zwlr_screencopy_manager_v1"][1], 3)
        if self.screencopy_version < 2:
            raise RuntimeError("zwlr_screencopy_manager_v1 version 2 (copy_with_damage) is required")
        self.screencopy = self._bind(registry, "zwlr_screencopy_manager_v1", self.screencopy_version)
        manager = self._bind(registry, "zwp_virtual_keyboard_manager_v1", 1)
        self.keyboard = self.conn.new_id(None)
        self.conn.request(manager, 0, u32(seat, self.keyboard))
        self.keycodes = {}
        self._set_keymap(keys)
        self.conn.roundtrip()
        self.buffers = {}

    def _global(self, opcode, payload):
        if opcode == 0:
            (name,) = struct.unpack_from("<I", payload)
            interface, pos = read_string(payload, 4)
            (version,) = struct.unpack_from("<I", payload, pos)
            # The first of each; a headless sway has one output.
            self.globals.setdefault(interface, (name, version))

    def _bind(self, registry, interface, version, handler=None):
        name, advertised = self.globals[interface]
        version = min(version, advertised)
        oid = self.conn.new_id(handler)
        self.conn.request(registry, 0, u32(name) + wl_string(interface) + u32(version, oid))
        return oid

    def _output_event(self, opcode, payload):
        if opcode == 0:  # geometry: x, y, ...
            self.output_x, self.output_y = struct.unpack_from("<ii", payload)
        elif opcode == 1:  # mode(flags, width, height, refresh mHz)
            flags, _, _, refresh = struct.unpack_from("<Iiii", payload)
            if flags & 1:  # current
                self.refresh_mhz = refresh
        elif opcode == 4:
            self.output_name, _ = read_string(payload, 0)

    def _set_keymap(self, keys):
        # The same shape of keymap wtype uploads: one keycode per symbol.
        codes = "".join(f"<K{i}> = {i + 8}; " for i in range(1, len(keys) + 1))
        symbols = "".join(f"key <K{i}> {{[{k}]}}; " for i, k in enumerate(keys, 1))
        text = (
            "xkb_keymap {\n"
            f'xkb_keycodes "(unnamed)" {{ minimum = 8; maximum = {len(keys) + 8}; {codes}}};\n'
            'xkb_types "(unnamed)" { include "complete" };\n'
            'xkb_compatibility "(unnamed)" { include "complete" };\n'
            f'xkb_symbols "(unnamed)" {{ {symbols}}};\n'
            "};\n"
        ).encode() + b"\0"
        fd = os.memfd_create("k2p-keymap")
        os.write(fd, text)
        self.conn.request(self.keyboard, 0, u32(1, len(text)), fd=fd)  # keymap(XKB_V1, fd, size)
        os.close(fd)
        self.keycodes = {k: i for i, k in enumerate(keys, 1)}

    def key(self, symbol):
        """Press and release; returns the time the requests were sent."""
        code = self.keycodes[symbol]
        ms = int(now() * 1000) & 0xFFFFFFFF
        msg = (u32(self.keyboard, 20 << 16 | 1) + u32(ms, code, 1)
               + u32(self.keyboard, 20 << 16 | 1) + u32(ms, code, 0))
        self.conn.sock.sendall(msg)
        return now()

    def frame(self, region):
        """A frame object for region, with its buffer described but not copied yet."""
        frame = Frame(self, region)
        while not frame.described and not frame.failed:
            self.conn.dispatch(None)
        return frame

    def copy(self, frame, slot, damage):
        fmt, width, height, stride = frame.buffer_info
        key = (frame.buffer_info, slot)
        if key not in self.buffers:
            self.buffers[key] = ShmBuffer(self.conn, self.shm, fmt, width, height, stride)
        frame.buffer = self.buffers[key]
        self.conn.request(frame.id, 2 if damage else 0, u32(frame.buffer.id))

    def destroy(self, frame):
        self.conn.request(frame.id, 1)
        self.conn.handlers.pop(frame.id, None)

    def wait(self, frame, deadline):
        while not frame.done:
            left = deadline - now()
            if left <= 0:
                return False
            self.conn.dispatch(left)
        return True

    def grab(self, region):
        """Copy region now (forcing an output frame); returns (frame, pixels)."""
        frame = self.frame(region)
        self.copy(frame, 0, damage=False)
        if not self.wait(frame, now() + 2.0) or frame.failed:
            self.destroy(frame)
            raise RuntimeError(f"screencopy of {region} failed")
        pixels = bytes(frame.buffer.map)
        self.destroy(frame)
        return frame, pixels

    def frame_ms(self):
        return 1e6 / self.refresh_mhz if self.refresh_mhz else None


def window_rect(title):
    tree = subprocess.run(["swaymsg", "-t", "get_tree"], check=True, capture_output=True).stdout
    out = subprocess.run([sys.executable, "-I", f"{HERE}/sway_find.py", title, "rect"],
                         input=tree, capture_output=True, check=True).stdout.decode().strip()
    xy, wh = out.split(" ")
    x, y = (int(v) for v in xy.split(","))
    w, h = (int(v) for v in wh.split("x"))
    return x, y, w, h


def changed_box(frame, before, after, region):
    """Bounding box of differing pixels, in output coordinates."""
    _, width, height, stride = frame.buffer_info
    bpp = stride // width if width else 4
    xs, ys = [], []
    for row in range(height):
        a = before[row * stride:row * stride + width * bpp]
        b = after[row * stride:row * stride + width * bpp]
        if a == b:
            continue
        cols = [c for c in range(width) if a[c * bpp:(c + 1) * bpp] != b[c * bpp:(c + 1) * bpp]]
        ys.append(height - 1 - row if frame.flags & Y_INVERT else row)
        xs += [cols[0], cols[-1]]
    if not ys:
        return None
    # Buffer pixels to logical coordinates (output scale).
    sx, sy = region[2] / width, region[3] / height
    x0, x1 = int(min(xs) * sx), int((max(xs) + 1) * sx + 0.999)
    y0, y1 = int(min(ys) * sy), int((max(ys) + 1) * sy + 0.999)
    return region[0] + x0, region[1] + y0, max(1, x1 - x0), max(1, y1 - y0)


# How long past the gap arm_quietly waits for a quiet output before it
# accepts a busy one (a flood redraws every frame, so it never goes quiet).
BUSY_CAP_S = 0.25


def arm_quietly(probe, cell, gap):
    """A copy_with_damage request in flight, sent well before the key.

    Every screencopy copy request, with damage or not, makes wlroots commit
    an output frame at once (wlr_output_update_needs_frame) when the output
    is idle, and the headless backend then holds the next frame for a full
    period. Arming after the key put that forced frame between the key and
    the terminal's echo commit, so every echo waited one extra period.

    Arming here, `gap` before the key, lets the forced frame and its period
    pass. A copy that completes first (damage tracking starts full, or
    something else drew) is replaced, and the key then waits until the
    latest request is two frame periods old. An output that keeps drawing
    (a flood) never lets a request get that old; after BUSY_CAP_S the latest
    one is used, which costs nothing there: a busy output already has its
    next frame scheduled, so the request forces no extra one.
    """
    settle = 2 * (probe.frame_ms() or 1000 / 60) / 1000
    deadline = now() + gap
    cap = deadline + BUSY_CAP_S
    frame, armed_at = None, None
    while True:
        if frame is None or frame.done:
            if frame is not None:
                probe.destroy(frame)
            frame = probe.frame(cell)
            probe.copy(frame, 0, damage=True)
            armed_at = now()
        t = now()
        quiet_from = max(deadline, armed_at + settle)
        if t >= quiet_from or t >= cap:
            return frame
        probe.conn.dispatch(min(quiet_from, cap) - t)


def measure(probe, cell, symbol, gap, timeout, arm_after_key=False):
    """One sample: (latency by frame timestamp, latency by receipt) in ms, or None.

    The baseline grab forces an output frame, and sway composites at most
    once per frame period, so wait `gap` (longer than a frame) before the key;
    otherwise every sample would include the rest of that frame. The first
    frame request is armed before that wait (see arm_quietly);
    `arm_after_key` restores the old order, which adds one frame.
    """
    _, baseline = probe.grab(cell)
    if arm_after_key:
        time.sleep(gap)
        current = probe.frame(cell)
    else:
        current = arm_quietly(probe, cell, gap)
    armed = probe.frame(cell)
    start = probe.key(symbol)
    if arm_after_key:
        probe.copy(current, 0, damage=True)
    slot = 1
    deadline = start + timeout
    try:
        while True:
            if not probe.wait(current, deadline):
                return None
            # Ask for the next damaged frame before looking at this one.
            probe.copy(armed, slot, damage=True)
            if not current.failed and bytes(current.buffer.map) != baseline:
                if not start <= current.ready <= current.received:
                    raise RuntimeError("screencopy timestamps are not on CLOCK_MONOTONIC")
                return (current.ready - start) * 1e3, (current.received - start) * 1e3
            probe.destroy(current)
            current, armed, slot = armed, probe.frame(cell), 1 - slot
    finally:
        probe.destroy(current)
        probe.destroy(armed)


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
    ap.add_argument("--arm", choices=["before-key", "after-key"], default="before-key",
                    help="when to request the first frame; after-key is the old, "
                         "one-frame-late order, kept for comparison")
    args = ap.parse_args(argv)

    keys = ["x", "o"] + ([args.start_key] if args.start_key else [])
    probe = Probe(keys)
    x, y, w, h = window_rect(args.title)
    x, y = x - probe.output_x, y - probe.output_y
    subprocess.run(["swaymsg", f'[title="^{args.title}$"] focus'], check=True, capture_output=True)
    # Give the terminal time to bind wl_keyboard now that the seat has one.
    time.sleep(1.0)

    # Calibrate on the bottom quarter (ubench-echo draws on the last row),
    # once it is still, and check a second key changes the same box.
    search = (x, y + h - h // 4, w, h // 4)
    _, before = probe.grab(search)
    for _ in range(20):
        time.sleep(0.25)
        _, again = probe.grab(search)
        if again == before:
            break
        before = again
    # Two keys, so a cell already showing the first one still changes.
    cell = None
    for symbol in ("x", "o"):
        probe.key(symbol)
        time.sleep(0.8)
        frame, after = probe.grab(search)
        cell = changed_box(frame, before, after, search)
        if cell:
            break
    if cell is None:
        print("no pixels changed after a key press; is ubench-echo running?", file=sys.stderr)
        return 1
    _, first = probe.grab(cell)
    shown = "o" if symbol == "x" else "x"
    probe.key(shown)
    time.sleep(0.8)
    if probe.grab(cell)[1] == first:
        print(f"calibration box {cell} did not change on a second key", file=sys.stderr)
        return 1
    if args.start_key:
        probe.key(args.start_key)  # ubench-echo echoes it too
        shown = args.start_key
        time.sleep(1.5)

    rng = random.Random(1)
    latencies, received = [], []
    timeouts = 0
    for _ in range(args.samples):
        # Always the key the cell is not showing, so every sample changes it.
        shown = "o" if shown == "x" else "x"
        sample = measure(probe, cell, shown, rng.uniform(0.05, 0.3), args.timeout,
                         arm_after_key=args.arm == "after-key")
        if sample is None:
            timeouts += 1
            continue
        latencies.append(sample[0])
        received.append(sample[1])

    frame_ms = probe.frame_ms()
    with open(args.out, "a", encoding="utf-8") as fh:
        for block, (ms, got) in enumerate(zip(latencies, received)):
            fh.write(json.dumps({
                "suite": "lat.k2p.external", "case": args.case, "arm": args.terminal,
                "block": block, "value": ms, "unit": "ms",
                "method": "virtual-keyboard+screencopy", "timestamp": "compositor-frame",
                "frame_request": args.arm,
                "received_ms": got, "resolution_ms": frame_ms, "frame_ms": frame_ms,
                "output": probe.output_name, "claim": args.claim,
                "env": {"session": "wayland"},
            }) + "\n")
    s = sorted(latencies)
    r = sorted(received)
    print(f"k2p-wayland {args.terminal} {args.case}: n={len(s)} timeouts={timeouts} "
          f"p50={s[len(s) // 2] if s else None} received p50={r[len(r) // 2] if r else None} "
          f"frame={frame_ms} ms cell={cell}", file=sys.stderr)
    if not latencies or timeouts * 10 > args.samples:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
