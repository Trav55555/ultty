#!/usr/bin/env python3
"""Request order of k2p-wayland.py's measure(), against a fake compositor.

    python3 -I scripts/bench/test_k2p_wayland.py

Every screencopy copy request makes wlroots commit an output frame at once,
so a copy sent between the key and the terminal's echo commit delays the
echo by a whole frame on sway. These tests pin that the probe sends no copy
request after the key until the first frame it asked for has come back.
"""
import importlib.util
import os
import sys
import time
import unittest

sys.dont_write_bytecode = True  # loading the probe would leave a __pycache__ behind
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("k2p_wayland", os.path.join(HERE, "k2p-wayland.py"))
k2p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(k2p)


class FakeBuffer:
    def __init__(self, pixels):
        self.map = pixels


class FakeFrame:
    def __init__(self):
        self.ready = self.received = None
        self.failed = False
        self.buffer = None

    @property
    def done(self):
        return self.ready is not None or self.failed


class FakeProbe:
    """Records requests; the echo shows on the first frame copied after the key.

    `early` frames armed before the key complete on the next dispatch (as
    when the compositor's damage tracking starts full, or a flood keeps
    drawing), to exercise re-arming.
    """

    def __init__(self, early=0):
        self.log = []
        self.early = early
        self.key_sent = False
        self.copied = []
        self.conn = self
        self.in_flight = []

    def grab(self, cell):
        self.log.append("grab")
        return FakeFrame(), b"before"

    def frame(self, cell):
        return FakeFrame()

    def copy(self, frame, slot, damage):
        self.log.append("copy")
        self.copied.append(frame)
        if not self.key_sent and self.early:
            self.early -= 1
            self.in_flight.append(frame)

    def key(self, symbol):
        self.log.append("key")
        self.key_sent = True
        return k2p.now()

    def destroy(self, frame):
        pass

    def frame_ms(self):
        return 1000 / 60

    def dispatch(self, timeout):
        time.sleep(min(timeout, 0.001))
        for frame in self.in_flight:
            self._complete(frame, b"before")
        self.in_flight.clear()
        return False

    def wait(self, frame, deadline):
        if not frame.done:
            self.log.append("frame")
            self._complete(frame, b"after")
        return True

    @staticmethod
    def _complete(frame, pixels):
        frame.buffer = FakeBuffer(pixels)
        frame.ready = k2p.now()
        frame.received = k2p.now()


def requests_between_key_and_first_frame(log):
    after_key = log[log.index("key") + 1:]
    return after_key[:after_key.index("frame")]


class MeasureOrder(unittest.TestCase):
    def test_frame_is_requested_before_the_key(self):
        probe = FakeProbe()
        sample = k2p.measure(probe, (0, 0, 1, 1), "x", gap=0.005, timeout=1.0)
        self.assertIsNotNone(sample)
        self.assertEqual(requests_between_key_and_first_frame(probe.log), [])
        self.assertLess(probe.log.index("copy"), probe.log.index("key"))

    def test_frame_that_completes_before_the_key_is_replaced(self):
        probe = FakeProbe(early=2)
        sample = k2p.measure(probe, (0, 0, 1, 1), "x", gap=0.005, timeout=1.0)
        self.assertIsNotNone(sample)
        self.assertEqual(probe.log[:5], ["grab", "copy", "copy", "copy", "key"])
        self.assertEqual(requests_between_key_and_first_frame(probe.log), [])

    def test_busy_output_still_gives_a_sample(self):
        probe = FakeProbe(early=10**6)  # every frame before the key has damage
        sample = k2p.measure(probe, (0, 0, 1, 1), "x", gap=0.005, timeout=1.0)
        self.assertIsNotNone(sample)
        self.assertEqual(requests_between_key_and_first_frame(probe.log), [])

    def test_after_key_keeps_the_old_order(self):
        probe = FakeProbe()
        k2p.measure(probe, (0, 0, 1, 1), "x", gap=0.005, timeout=1.0, arm_after_key=True)
        self.assertEqual(requests_between_key_and_first_frame(probe.log), ["copy"])


class AnonFdTest(unittest.TestCase):
    def roundtrip(self):
        fd = k2p.anon_fd("test")
        try:
            os.write(fd, b"keymap")
            os.lseek(fd, 0, os.SEEK_SET)
            self.assertEqual(os.read(fd, 16), b"keymap")
        finally:
            os.close(fd)

    def test_memfd(self):
        self.roundtrip()

    def test_without_memfd_create(self):
        # uv's python-build-standalone has no os.memfd_create.
        saved = getattr(os, "memfd_create", None)
        if saved:
            del os.memfd_create
        try:
            self.assertFalse(hasattr(os, "memfd_create"))
            self.roundtrip()
        finally:
            if saved:
                os.memfd_create = saved


if __name__ == "__main__":
    unittest.main()
