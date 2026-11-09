#!/usr/bin/env python3
"""Press keys in a headless GNOME Shell through Mutter's RemoteDesktop API.

    gnome-keys.py N [--gap-min S] [--gap-max S]

Needs the system Python's GObject bindings (python3-gi) and the session bus
of the GNOME Shell to drive. Mutter ends a remote desktop session when the
bus connection that created it closes, so one process creates the session
and sends every key. GNOME starts in the overview; Escape leaves it, which
focuses the newest window. Keys alternate between a and b (evdev codes).
"""
import argparse
import random
import time

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

DEST = "org.gnome.Mutter.RemoteDesktop"
SESSION = "org.gnome.Mutter.RemoteDesktop.Session"
KEY_ESC, KEY_A, KEY_B = 1, 30, 48


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("n", type=int)
    ap.add_argument("--gap-min", type=float, default=0.3)
    ap.add_argument("--gap-max", type=float, default=0.6)
    args = ap.parse_args()
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    def call(path, iface, method, params=None, reply=None):
        return bus.call_sync(DEST, path, iface, method, params,
                             GLib.VariantType(reply) if reply else None,
                             Gio.DBusCallFlags.NONE, -1)

    def key(code):
        for pressed in (True, False):
            call(session, SESSION, "NotifyKeyboardKeycode", GLib.Variant("(ub)", (code, pressed)))

    (session,) = call("/org/gnome/Mutter/RemoteDesktop", DEST, "CreateSession",
                      reply="(o)").unpack()
    call(session, SESSION, "Start")
    time.sleep(0.5)
    key(KEY_ESC)
    time.sleep(1.5)
    for i in range(args.n):
        print(f"key {i} sent at CLOCK_MONOTONIC {time.monotonic():.6f}", flush=True)
        key(KEY_A if i % 2 == 0 else KEY_B)
        time.sleep(random.uniform(args.gap_min, args.gap_max))
    call(session, SESSION, "Stop")


if __name__ == "__main__":
    main()
