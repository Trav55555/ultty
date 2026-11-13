#!/usr/bin/env python3
"""Bounded real GPU reconnect gate through a disposable local SSH relay.

No SSH server, network, enrollment, package installation or Cargo invocation.
Requires only Python's standard library, Xvfb, xdotool, Xlib and a Vulkan
driver already present on the GUI runner. Pass frozen binaries and a fresh
output directory. The relay launches the actual fixed attach-stdio helper.
"""

import argparse
import ctypes
import ctypes.util
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import traceback
import zlib


HERE = Path(__file__).resolve().parent
BUDGET_SECONDS = 120


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def read_frame(stream, maximum):
    def exact(size):
        data = bytearray()
        while len(data) < size:
            part = os.read(stream.fileno(), size - len(data))
            if not part:
                raise EOFError("relay stream closed")
            data.extend(part)
        return bytes(data)
    header = exact(8)
    assert header[:4] == b"UAT1", "invalid relay magic"
    size = struct.unpack("<I", header[4:])[0]
    assert 0 < size <= maximum, "oversized relay frame"
    body = exact(size)
    return header + body, json.loads(body)


def encoded(value):
    body = json.dumps(value, separators=(",", ":")).encode()
    assert len(body) <= 1024 * 1024
    return b"UAT1" + struct.pack("<I", len(body)) + body


def proxy(arguments):
    """Token-free fault receipts around the real helper, never a fake owner."""
    assert arguments[-3:] == ["--", "reconnect-fixture", "ultty-session attach-stdio"]
    directory = Path(os.environ["RECONNECT_PROXY_DIR"])
    binary = os.environ["RECONNECT_SESSION_BIN"]
    log = os.open(directory / "relay.jsonl", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    helper = None

    def record(event, **fields):
        phase = (directory / "phase").read_text().strip()
        # Only these explicitly selected fields enter receipts. In particular,
        # neither Hello credentials nor input/snapshot payloads are serialized.
        value = {"event": event, "phase": phase, "proxy_pid": os.getpid(), **fields}
        os.write(log, (json.dumps(value) + "\n").encode())

    def take_flag(name):
        try:
            (directory / name).unlink()
            return True
        except FileNotFoundError:
            return False

    try:
        record("connection", argv_valid=True, inherited_local_auth="ULTTY_AUTH_TOKEN" in os.environ)
        if (directory / "offline").exists():
            record("offline_refusal")
            return
        with open(directory / f"helper-{os.getpid()}.log", "wb") as error:
            helper = subprocess.Popen([binary, "attach-stdio"], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=error, bufsize=0,
                                      start_new_session=True)
            record("helper", helper_pid=helper.pid)
            while True:
                raw, request = read_frame(sys.stdin.buffer, 1024 * 1024)
                command = request["command"]["type"]
                record("request", command=command, request_id=request["id"], owner=request.get("owner"),
                       hello_controller=request["command"].get("controller") if command == "hello" else None)
                replacement = directory / "replacement.token"
                if command == "hello" and replacement.exists():
                    request["command"]["token"] = replacement.read_text().strip()
                    raw = encoded(request)
                    record("replacement_credential_substituted")
                if command == "native_input" and take_flag("drop-before-input"):
                    record("dropped_before_forward", request_id=request["id"])
                    (directory / "offline").touch()
                    return
                helper.stdin.write(raw)
                helper.stdin.flush()
                reply_raw, reply = read_frame(helper.stdout, 16 * 1024 * 1024)
                record("response", command=command, request_id=request["id"], owner=reply.get("owner"),
                       reply=reply["reply"]["type"], code=reply["reply"].get("code"),
                       last_mutation_id=reply["last_mutation_id"])
                if command == "native_input" and take_flag("drop-after-input"):
                    assert reply["reply"]["type"] == "ok", "input was refused before lost-reply fault"
                    record("dropped_after_ack", request_id=request["id"], acknowledged=True)
                    (directory / "offline").touch()
                    return
                if take_flag("cut-next-reply"):
                    record("transport_cut", command=command)
                    (directory / "offline").touch()
                    return
                sys.stdout.buffer.write(reply_raw)
                sys.stdout.buffer.flush()
    except (EOFError, BrokenPipeError):
        record("stream_closed")
    finally:
        if helper is not None:
            helper.stdin.close()
            try:
                helper.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(helper.pid, signal.SIGKILL)
                helper.wait(timeout=2)
        os.close(log)


def until(description, action, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = action()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(f"deadline expired: {description}")


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def png(path, width, height, rgba):
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))
    stride = width * 4
    raw = b"".join(b"\0" + rgba[y * stride:(y + 1) * stride] for y in range(height))
    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


class XImage(ctypes.Structure):
    """Xlib.h's XImage prefix through its RGB masks; no function-table access."""
    _fields_ = [(name, ctypes.c_int) for name in ("width", "height", "xoffset", "format")] + [
        ("data", ctypes.c_void_p)] + [(name, ctypes.c_int) for name in (
            "byte_order", "bitmap_unit", "bitmap_bit_order", "bitmap_pad", "depth",
            "bytes_per_line", "bits_per_pixel")] + [(name, ctypes.c_ulong) for name in (
                "red_mask", "green_mask", "blue_mask")]


class X11:
    def __init__(self, display):
        self.lib = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
        self.lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.lib.XOpenDisplay.restype = ctypes.c_void_p
        self.lib.XCloseDisplay.argtypes = [ctypes.c_void_p]
        self.lib.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                      ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
        self.lib.XGetImage.restype = ctypes.POINTER(XImage)
        self.lib.XDestroyImage.argtypes = [ctypes.POINTER(XImage)]
        self.lib.XFlush.argtypes = [ctypes.c_void_p]
        self.lib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
        self.lib.XInternAtom.restype = ctypes.c_ulong
        self.lib.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int,
                                       ctypes.c_long, ctypes.c_void_p]
        self.lib.XSendEvent.restype = ctypes.c_int
        # A vanished fixture window must fail the Python gate and run cleanup,
        # rather than let Xlib's default BadDrawable handler exit the process.
        handler_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        self.error_handler = handler_type(lambda _display, _event: 0)
        self.lib.XSetErrorHandler.argtypes = [handler_type]
        self.lib.XSetErrorHandler.restype = ctypes.c_void_p
        self.lib.XSetErrorHandler(self.error_handler)
        self.display = self.lib.XOpenDisplay(display.encode())
        assert self.display, "cannot open private X display"

    def capture(self, window, width, height, path):
        image = self.lib.XGetImage(self.display, int(window), 0, 0, width, height,
                                  ctypes.c_ulong(-1).value, 2)  # AllPlanes, ZPixmap
        assert image, "XGetImage returned no window pixels"
        try:
            view = image.contents
            assert (view.width, view.height) == (width, height)
            assert view.bits_per_pixel == 32 and view.byte_order in (0, 1), "unsupported X image format"
            assert (view.red_mask, view.green_mask, view.blue_mask) == (0xff0000, 0xff00, 0xff)
            raw = ctypes.string_at(view.data, view.bytes_per_line * height)
            rgba = bytearray(width * height * 4)
            for y in range(height):
                row = raw[y * view.bytes_per_line:y * view.bytes_per_line + width * 4]
                target = bytearray(width * 4)
                r, g, b = (2, 1, 0) if view.byte_order == 0 else (1, 2, 3)
                target[0::4], target[1::4], target[2::4] = row[r::4], row[g::4], row[b::4]
                target[3::4] = b"\xff" * width
                rgba[y * width * 4:(y + 1) * width * 4] = target
            png(path, width, height, rgba)
            return bytes(rgba)
        finally:
            self.lib.XDestroyImage(image)

    def close_window(self, window):
        class Data(ctypes.Union):
            _fields_ = [("b", ctypes.c_char * 20), ("s", ctypes.c_short * 10), ("l", ctypes.c_long * 5)]
        class Message(ctypes.Structure):
            _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                        ("display", ctypes.c_void_p), ("window", ctypes.c_ulong),
                        ("message_type", ctypes.c_ulong), ("format", ctypes.c_int), ("data", Data)]
        class Event(ctypes.Union):
            _fields_ = [("client", Message), ("pad", ctypes.c_long * 24)]
        event = Event()
        event.client.type, event.client.send_event = 33, 1
        event.client.display, event.client.window = self.display, int(window)
        event.client.message_type = self.lib.XInternAtom(self.display, b"WM_PROTOCOLS", 1)
        event.client.format = 32
        event.client.data.l[0] = self.lib.XInternAtom(self.display, b"WM_DELETE_WINDOW", 1)
        assert event.client.message_type and event.client.data.l[0]
        assert self.lib.XSendEvent(self.display, int(window), 0, 0, ctypes.byref(event))
        self.lib.XFlush(self.display)

    def close(self):
        self.lib.XCloseDisplay(self.display)


def run(args):
    host_helpers = load("reconnect_host_helpers", HERE / "session-host.py")
    pixels = load("reconnect_screenshot_helpers", HERE / "native_split_check.py")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    private = out / "private"
    private.mkdir(mode=0o700)
    relay = private / "relay"
    relay.mkdir(mode=0o700)
    (relay / "phase").write_text("initial")
    binaries = {name: path.resolve() for name, path in (
        ("ultty", args.ultty), ("session", args.session_bin), ("counter", args.counter))}
    sources = [*binaries.values(), Path(__file__).resolve(), HERE / "session-host.py",
               HERE / "native_split_check.py", HERE.parent / "analyze-inline-surface-screenshots.py"]
    hashes = {str(path): digest(path) for path in sources}
    save(out / "inputs.json", {"sha256": hashes, "budget_seconds": BUDGET_SECONDS,
                               "qualification": "local fake SSH relay; real helper, host and GPU viewer"})
    shutil.copy2(Path(__file__), private / "session-reconnect.py")
    fake_bin = private / "bin"
    fake_bin.mkdir(mode=0o700)
    fake_ssh = fake_bin / "ssh"
    fake_ssh.write_text("#!" + sys.executable + "\nimport os,sys\nos.execv(sys.executable,[sys.executable,"
                        + "'-S'," + repr(str(private / "session-reconnect.py")) + ",'--proxy',*sys.argv[1:]])\n")
    fake_ssh.chmod(0o700)
    runtime = Path(tempfile.mkdtemp(prefix="ur-", dir=os.environ.get("TMPDIR", "/tmp")))
    runtime.chmod(0o700)
    assert len(os.fsencode(runtime / "un-0123456789abcdef" / "ultty-9999999-tab0.sock")) < 108
    name = "reconnect"
    config = private / "config/ultty"
    config.mkdir(parents=True)
    (config / "config.toml").write_text("[behavior]\ncursor_blink = false\n")
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(config.parent),
               SHELL="/bin/sh", RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn",
               PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
               RECONNECT_PROXY_DIR=str(relay), RECONNECT_SESSION_BIN=str(binaries["session"]))
    for key in ("WAYLAND_DISPLAY", "ULTTY_PID", "ULTTY_SOCKET", "ULTTY_AUTH_TOKEN", "ULTTY_PANE"):
        env.pop(key, None)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if drivers and "VK_ICD_FILENAMES" not in env:
        env["VK_ICD_FILENAMES"] = str(drivers[0])
    files, viewers, clients, hosts, secrets, child_pids = [], [], [], [], [], []
    x11 = xvfb = admin = current_host = None
    result = {"passed": False, "phases": [], "screenshots": [], "binary_sha256": hashes}
    started = time.monotonic()

    class Client(host_helpers.Client):
        def __init__(self, token, controller=False):
            self.sock = socket.socket(socket.AF_UNIX)
            self.sock.settimeout(3)
            self.sock.connect(str(runtime / f"ultty-session-{name}.sock"))
            self.owner = self.grant = None
            self.next_id = 1
            response = self.exchange(0, {"type": "hello", "version": 1, "session": name,
                                         "token": token, "controller": controller})
            assert response["reply"]["type"] == "welcome", response
            self.owner = response["owner"]
            self.grant = response["reply"]["grant_id"]
            clients.append(self)

        def exchange(self, identity, command):
            response = super().exchange(identity, command)
            self.next_id = max(getattr(self, "next_id", 1), response["last_mutation_id"] + 1)
            return response

        def mutate(self, command):
            return self.exchange(self.next_id, command)["reply"]

    def log(label):
        output = (out / f"{label}.log").open("wb")
        files.append(output)
        return output

    def phase(label):
        (relay / "phase").write_text(label)

    def events():
        path = relay / "relay.jsonl"
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines()]

    def command(*arguments):
        return subprocess.check_output(["xdotool", *map(str, arguments)], env=env,
                                       stderr=subprocess.DEVNULL, timeout=5).decode().strip()

    def focus(window):
        command("windowfocus", "--sync", window)
        assert int(command("getwindowfocus")) == int(window)

    def key(window, value):
        focus(window)
        command("key", "--clearmodifiers", *value.split())

    def title(window):
        return command("getwindowname", window).lower()

    def titled(window, *words):
        def observed():
            process = next(process for process, identifier in reversed(windows) if identifier == window)
            assert process.poll() is None, f"viewer exited {process.returncode} before title {'/'.join(words)}"
            value = title(window)
            return value if all(word.lower() in value for word in words) else None
        return until("viewer title " + "/".join(words), observed)

    def snapshot():
        value = admin.snapshot()
        pane = value["panes"][0]
        native = pane["native"][0] if pane["native"] else None
        if native is None:
            return None
        text = []
        for region in native["regions"]:
            content = region.get("text")
            if not isinstance(content, dict):
                continue
            plain = content.get("Plain")
            if "Rich" in content:
                plain = "".join(span["text"] for span in content["Rich"])
            if isinstance(plain, str) and plain.lstrip("-").isdigit():
                text.append(int(plain))
        if len(text) != 1:
            return None
        return {"owner": value["owner"], "pane": pane["id"], "pid": pane["pid"],
                "native": native["session_id"], "count": text[0]}

    def counted(expected):
        return until(f"authoritative count {expected}", lambda: (lambda value:
                     value if value and value["count"] == expected else None)(snapshot()))

    def spawn_host(label):
        nonlocal admin, current_host
        current_host = subprocess.Popen([str(binaries["session"]), "host", name,
                                         "-e", str(binaries["counter"])], env=env,
                                        stdout=log(label), stderr=subprocess.STDOUT)
        hosts.append(current_host)
        credential = runtime / f"ultty-session-{name}.admin"
        until("host endpoint", lambda: credential.is_file() and
              (runtime / f"ultty-session-{name}.sock").exists())
        token = credential.read_text().strip()
        secrets.append(token)
        admin = Client(token)
        value = counted(0)
        child_pids.append(value["pid"])
        return value

    def grant(label):
        path = relay / label
        completed = subprocess.run([str(binaries["session"]), "grant", name, "--caps",
                                    "read,input,layout", "--ttl", "300", "--out", str(path)],
                                   env=env, capture_output=True, timeout=5)
        assert completed.returncode == 0, completed.stderr
        assert path.stat().st_mode & 0o777 == 0o600
        token = path.read_text().strip()
        secrets.append(token)
        return path, token

    def open_viewer(label, credential):
        process = subprocess.Popen([str(binaries["ultty"]), "--attach", name,
                                    "--ssh", "reconnect-fixture", "--token-file", str(credential),
                                    "--screenshot", str(out / f"{label}-first.png")], env=env,
                                   stdout=log(label), stderr=subprocess.STDOUT)
        viewers.append(process)
        def visible():
            assert process.poll() is None, f"viewer exited {process.returncode}"
            try:
                return command("search", "--onlyvisible", "--pid", process.pid).splitlines()[0]
            except (subprocess.CalledProcessError, IndexError):
                return None
        window = until("viewer window", visible)
        windows.append((process, window))
        titled(window, "controller")
        until("first rendered replacement", lambda: (out / f"{label}-first.png").is_file())
        focus(window)
        return process, window

    def capture(window, label):
        geometry = dict(line.split("=", 1) for line in command("getwindowgeometry", "--shell", window).splitlines())
        width, height = int(geometry["WIDTH"]), int(geometry["HEIGHT"])
        path = out / f"{label}.png"
        rgba = x11.capture(window, width, height, path)
        w, h, rows = pixels.screenshot_rows(path)
        assert (w, h) == (width, height) and width >= 600 and height >= 400
        assert pixels.ink_pixels(rows, (8, 80, width - 8, height - 8)) > 2000
        result["screenshots"].append({"file": path.name, "sha256": digest(path),
                                      "width": width, "height": height})
        # Exclude the reconnect status band while retaining the counter card,
        # value and buttons. Exact pixels catch blanked/replaced replicas.
        return b"".join(rgba[y * width * 4 + 8 * 4:(y * width + width - 8) * 4]
                        for y in range(max(100, height // 4), height - 50))

    def close_viewer(process, window):
        x11.close_window(window)
        process.wait(timeout=6)
        assert process.returncode == 0, process.returncode

    def assert_no_native_after(index):
        unexpected = [event for event in events()[index:]
                      if event["event"] == "request" and event.get("command") in ("input", "native_input")]
        assert not unexpected, "unknown/offline input was automatically replayed"

    def same_identity(expected):
        current = snapshot()
        assert current and all(current[key] == expected[key] for key in ("owner", "pane", "pid", "native"))
        os.kill(current["pid"], 0)
        return current

    def running(pid):
        status = Path(f"/proc/{pid}/stat")
        try:
            text = status.read_text()
        except FileNotFoundError:
            return False
        return text[text.rfind(")") + 2:].split()[0] != "Z"

    windows = []

    try:
        display = (out / "display").open("w+b")
        files.append(display)
        xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(display.fileno()), "-screen", "0", "1280x800x24",
                                 "-nolisten", "tcp", "-noreset"], pass_fds=(display.fileno(),),
                                stdout=log("xvfb"), stderr=subprocess.STDOUT)
        def display_ready():
            assert xvfb.poll() is None, "Xvfb failed"
            value = (out / "display").read_text()
            return value.strip() if value.endswith("\n") and value.strip().isdigit() else None
        env["DISPLAY"] = ":" + until("private display", display_ready)
        x11 = X11(env["DISPLAY"])
        original = spawn_host("host")
        viewer_token, _ = grant("viewer.token")
        _, competitor_token = grant("competitor.token")
        process, window = open_viewer("after-ack", viewer_token)
        baseline = capture(window, "before-ack-loss")
        phase("after-ack-loss")
        (relay / "drop-after-input").touch()
        key(window, "plus")
        until("ACK lost after real helper", lambda: any(event["event"] == "dropped_after_ack" for event in events()))
        titled(window, "disconnected")
        assert capture(window, "after-ack-offline") == baseline, "last screen was not retained"
        counted(1)
        offline_index = len(events())
        key(window, "plus plus plus")
        counted(1)
        competitor = Client(competitor_token, controller=True)
        (relay / "offline").unlink()
        phase("busy-reconnect")
        key(window, "ctrl+shift+r")
        titled(window, "observer", "control unavailable")
        assert_no_native_after(offline_index)
        key(window, "plus")
        counted(1)
        competitor.sock.close()
        phase("controller-reconnect")
        key(window, "ctrl+shift+r")
        titled(window, "controller")
        assert_no_native_after(offline_index)
        counted(1)
        same_identity(original)
        capture(window, "controller-restored")
        key(window, "plus")
        counted(2)
        result["phases"].append("after-ACK unknown reply not replayed; offline input discarded; busy lease preserved; explicit retry regains control")
        close_viewer(process, window)

        phase("before-forward-loss")
        process, window = open_viewer("before-forward", viewer_token)
        baseline = capture(window, "before-forward-loss")
        (relay / "drop-before-input").touch()
        key(window, "plus")
        until("request lost before owner", lambda: any(event["event"] == "dropped_before_forward" for event in events()))
        titled(window, "disconnected")
        assert capture(window, "before-forward-offline") == baseline
        counted(2)
        offline_index = len(events())
        key(window, "plus plus")
        (relay / "offline").unlink()
        phase("unreserved-reconnect")
        key(window, "ctrl+shift+r")
        titled(window, "observer", "unknown", "reopen")
        key(window, "ctrl+shift+t")
        titled(window, "observer", "unknown", "reopen")
        assert_no_native_after(offline_index)
        counted(2)
        same_identity(original)
        capture(window, "unreserved-read-only")
        competitor = Client(competitor_token, controller=True)
        competitor.sock.close()
        result["phases"].append("before-forward unknown ID remains read-only; explicit claim cannot bypass unresolved input; identity retained")

        phase("owner-replacement-cut")
        baseline = capture(window, "before-owner-replacement")
        (relay / "cut-next-reply").touch()
        titled(window, "disconnected")
        stopped = admin.mutate({"type": "terminate"})
        assert stopped["type"] == "ok", stopped
        current_host.wait(timeout=5)
        assert current_host.returncode == 0
        admin.sock.close()
        replacement = spawn_host("replacement-host")
        assert replacement["owner"] != original["owner"]
        grant("replacement.token")
        (relay / "offline").unlink()
        phase("replacement-owner")
        index = len(events())
        key(window, "ctrl+shift+r")
        titled(window, "refused", "reopen")
        assert capture(window, "replacement-refused") == baseline
        counted(0)
        replacement_events = events()[index:]
        assert any(event["event"] == "replacement_credential_substituted" for event in replacement_events)
        assert all(event.get("command") == "hello" for event in replacement_events if event["event"] == "request"), replacement_events
        result["phases"].append("replacement accepted test credential still refused by exact owner pin before any claim or snapshot; old scene retained")
        result["original_identity"], result["replacement_identity"] = original, replacement
        assert all(digest(Path(path)) == value for path, value in hashes.items()), "frozen inputs changed"
        result["passed"] = True
    except Exception:
        (out / "failure.log").write_text(traceback.format_exc())
        raise
    finally:
        signal.alarm(15)
        try:
            for process in viewers:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=3)
            for client in clients:
                client.sock.close()
            for host in hosts:
                if host.poll() is None:
                    host.terminate()
                    try:
                        host.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        host.kill()
                        host.wait(timeout=3)
            helper_pids = {event["helper_pid"] for event in events() if event["event"] == "helper"}
            for pid in helper_pids:
                if running(pid):
                    argv = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
                    assert argv[:2] == [os.fsencode(binaries["session"]), b"attach-stdio"], "refusing an unowned helper"
                    os.killpg(pid, signal.SIGTERM)
            deadline = time.monotonic() + 2
            while any(running(pid) for pid in helper_pids) and time.monotonic() < deadline:
                time.sleep(0.02)
            for pid in helper_pids:
                if running(pid):
                    os.killpg(pid, signal.SIGKILL)
            until("owned helpers stopped", lambda: not any(running(pid) for pid in helper_pids), seconds=2)
            until("owned counter children stopped", lambda: not any(running(pid) for pid in child_pids), seconds=2)
            if x11 is not None:
                x11.close()
            if xvfb is not None:
                xvfb.terminate()
                xvfb.wait(timeout=3)
            records = events()
            assert all(not event.get("inherited_local_auth", False) for event in records)
            save(out / "relay.jsonl.json", records)
            for path in out.glob("*.log"):
                assert not any(token.encode() in path.read_bytes() for token in secrets), "credential leak in runtime log"
            shutil.rmtree(private)
            shutil.rmtree(runtime)
            result["cleanup"] = {"viewers_stopped": all(process.poll() is not None for process in viewers),
                                 "hosts_stopped": all(host.poll() is not None for host in hosts),
                                 "helpers_stopped": not any(running(pid) for pid in helper_pids),
                                 "counter_children_stopped": not any(running(pid) for pid in child_pids),
                                 "xvfb_stopped": xvfb is None or xvfb.poll() is not None,
                                 "private_tokens_and_runtime_removed": not private.exists() and not runtime.exists()}
        except Exception:
            result["passed"] = False
            result["cleanup"] = "failed; inspect owned fixture receipts"
            raise
        finally:
            for output in files:
                output.close()
            result["seconds"] = time.monotonic() - started
            save(out / "result.json", result)
            signal.alarm(0)
    print(f"PASS reconnect retained/unknown/offline/arbitration/replacement ({out})")


def main():
    if sys.argv[1:2] == ["--proxy"]:
        proxy(sys.argv[2:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ultty", type=Path, required=True)
    parser.add_argument("--session-bin", type=Path, required=True)
    parser.add_argument("--counter", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    for program in ("Xvfb", "xdotool"):
        assert shutil.which(program), program + " is required"
    def expired(_signal, _frame):
        raise TimeoutError("reconnect gate exceeded its bounded deadline")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(BUDGET_SECONDS)
    run(args)


if __name__ == "__main__":
    main()
