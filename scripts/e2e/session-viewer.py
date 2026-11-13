#!/usr/bin/env python3
"""Real GPU attachment lifecycle against a private local owner and counter.

Build fresh binaries first. Set ULTTY, ULTTY_SESSION, COUNTER_APP, and
CARGO_TARGET_DIR. Requires Xvfb, xdotool, Pillow, and a Vulkan driver. Uses a
fresh artifact directory, preserves logs/receipts, and never uses external
networking or an existing desktop/session.
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
import socket
import subprocess
import time

from PIL import Image


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("session_host_gate", HERE / "session-host.py")
HOST_GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HOST_GATE)


class Client(HOST_GATE.Client):
    """Use existing wire framing with an explicit controller choice."""

    def __init__(self, path, token, name, controller=False):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect(str(path))
        self.owner = None
        self.grant = None
        response = self.exchange(0, {"type": "hello", "version": 1, "session": name,
                                     "token": token, "controller": controller})
        if response["reply"]["type"] != "welcome":
            raise RuntimeError(f"attachment hello refused: {response['reply']}")
        self.owner = response["owner"]
        self.grant = response["reply"]["grant_id"]
        self.next_id = response["last_mutation_id"] + 1

    def mutate(self, command):
        response = self.exchange(self.next_id, command)
        self.next_id = response["last_mutation_id"] + 1
        return response["reply"]

    def exchange(self, request_id, command):
        response = super().exchange(request_id, command)
        # Other admin connections share the authoritative mutation watermark.
        # Reads observe it too, just as the production AttachmentClient does.
        self.next_id = max(getattr(self, "next_id", 1), response["last_mutation_id"] + 1)
        return response


def until(description, action, timeout=15):
    return HOST_GATE.until(time.monotonic() + timeout, description, action)


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def identity(snapshot):
    if len(snapshot["panes"]) != 1 or len(snapshot["panes"][0]["native"]) != 1:
        return None
    pane = snapshot["panes"][0]
    native = pane["native"][0]
    if not native["full_scene"] or not native["regions"]:
        return None
    return {"owner": snapshot["owner"], "pane": pane["id"], "pid": pane["pid"],
            "native": native["session_id"]}


def count(snapshot):
    values = []
    for pane in snapshot["panes"]:
        for native in pane["native"]:
            for region in native["regions"]:
                text = region.get("text")
                if not isinstance(text, dict):
                    continue
                value = text.get("Plain")
                if "Rich" in text:
                    value = "".join(span["text"] for span in text["Rich"])
                if isinstance(value, str) and value.lstrip("-").isdigit():
                    values.append(int(value))
    return values[0] if len(values) == 1 else None


def short_runtime(target):
    # Nested native socket names must fit sockaddr_un. /tmp may have a distinct
    # user quota, so reserve a short task-owned directory on the project disk.
    for index in range(100):
        candidate = target.parent / f"v{index}"
        longest = candidate / ("ultty-native-" + "a" * 16) / "ultty-9999999-tab0.sock"
        if len(os.fsencode(longest)) >= 108:
            raise RuntimeError("CARGO_TARGET_DIR parent is too long for native socket fixtures")
        try:
            candidate.mkdir(mode=0o700)
            return candidate
        except FileExistsError:
            continue
    raise RuntimeError("no private short runtime fixture directory is free")


def screenshot(path):
    image = Image.open(path).convert("RGB")
    if image.width < 600 or image.height < 400:
        raise RuntimeError("viewer screenshot has an unexpectedly small viewport")
    crop = image.crop((8, 50, image.width - 8, image.height - 8))
    colours = crop.getcolors(crop.width * crop.height)
    ink = crop.width * crop.height - max(amount for amount, _ in colours)
    # The native counter has a card, text and buttons. Empty VT/cursor/border
    # output cannot satisfy this interior threshold.
    if ink < 2000:
        raise RuntimeError(f"native scene is missing from screenshot: {ink} interior ink pixels")
    return {"file": path.name, "sha256": digest(path), "width": image.width,
            "height": image.height, "interior_ink": ink}


def process_sample(pid):
    status = Path(f"/proc/{pid}/status").read_text().splitlines()
    fields = {line.split(":", 1)[0]: line.split(":", 1)[1].strip() for line in status}
    stat = Path(f"/proc/{pid}/stat").read_text()
    values = stat[stat.rfind(")") + 2:].split()
    return {"pid": pid, "rss_kib": int(fields["VmRSS"].split()[0]),
            "threads": int(fields["Threads"]), "cpu_ticks": int(values[11]) + int(values[12])}


def idle_sample(label, processes):
    before = {name: process_sample(process.pid) for name, process in processes.items()}
    start = time.monotonic()
    time.sleep(1)
    elapsed = time.monotonic() - start
    after = {name: process_sample(process.pid) for name, process in processes.items()}
    return {"label": label, "elapsed_seconds": elapsed, "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "processes": {name: {"before": before[name], "after": after[name],
                                 "cpu_ticks_delta": after[name]["cpu_ticks"] - before[name]["cpu_ticks"]}
                          for name in processes}}


def request_window_close(display_name, window):
    """Send the same WM_DELETE_WINDOW ClientMessage as a window manager.

    xdotool windowclose directly destroys a window and does not request this
    protocol. Structures follow the platform's X11/Xlib.h, including long[24].
    """
    class ClientData(ctypes.Union):
        _fields_ = [("b", ctypes.c_char * 20), ("s", ctypes.c_short * 10), ("l", ctypes.c_long * 5)]
    class ClientMessage(ctypes.Structure):
        _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                    ("display", ctypes.c_void_p), ("window", ctypes.c_ulong),
                    ("message_type", ctypes.c_ulong), ("format", ctypes.c_int), ("data", ClientData)]
    class XEvent(ctypes.Union):
        _fields_ = [("client", ClientMessage), ("pad", ctypes.c_long * 24)]
    x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(XEvent)]
    x11.XSendEvent.restype = ctypes.c_int
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(display_name.encode())
    if not display:
        raise RuntimeError("cannot connect to the private X server for close request")
    try:
        protocols = x11.XInternAtom(display, b"WM_PROTOCOLS", 1)
        close = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 1)
        if not protocols or not close:
            raise RuntimeError("viewer has not advertised WM_DELETE_WINDOW")
        event = XEvent()
        event.client.type = 33  # ClientMessage
        event.client.send_event = 1
        event.client.display = display
        event.client.window = int(window)
        event.client.message_type = protocols
        event.client.format = 32
        event.client.data.l[0] = close
        if not x11.XSendEvent(display, int(window), 0, 0, ctypes.byref(event)):
            raise RuntimeError("X11 refused the close request")
        x11.XFlush(display)
    finally:
        x11.XCloseDisplay(display)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    args = parser.parse_args()
    out = args.artifacts.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise RuntimeError("use a fresh artifact directory; previous receipts are preserved")
    binaries = {name: Path(os.environ[name]).resolve()
                for name in ("ULTTY", "ULTTY_SESSION", "COUNTER_APP")}
    for tool in ("Xvfb", "xdotool"):
        if not shutil.which(tool):
            raise RuntimeError(f"{tool} is required")
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    runtime = short_runtime(target)
    name = f"viewer-gate-{os.getpid()}"
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), SHELL="/bin/sh",
               RUST_LOG="info,wgpu_core=warn,wgpu_hal=warn")
    env.pop("WAYLAND_DISPLAY", None)
    for key in ("ULTTY_SOCKET", "ULTTY_AUTH_TOKEN", "ULTTY_PANE", "ULTTY_PID"):
        env.pop(key, None)
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if "VK_ICD_FILENAMES" not in env and drivers:
        env["VK_ICD_FILENAMES"] = str(drivers[0])
    config = out / "config" / "ultty"
    config.mkdir(parents=True)
    (config / "config.toml").write_text("[behavior]\ncursor_blink = false\n")
    env["XDG_CONFIG_HOME"] = str(config.parent)
    files, viewers = [], []
    host = xvfb = admin = observer = None
    receipt = {"binaries": {key: {"path": str(path), "sha256": digest(path)}
                            for key, path in binaries.items()}, "screenshots": [], "phases": [], "idle_samples": []}
    shutil.copy2(Path(__file__), out / "session-viewer.py")
    shutil.copy2(HERE / "session-host.py", out / "session-host.py")
    save(out / "manifest.json", {"binaries": receipt["binaries"],
                                "harness_sha256": digest(Path(__file__)),
                                "wire_helper_sha256": digest(HERE / "session-host.py")})

    def log(label):
        stream = open(out / f"{label}.log", "wb")
        files.append(stream)
        return stream

    def xdotool(*arguments):
        return subprocess.check_output(["xdotool", *map(str, arguments)], env=env,
                                       stderr=subprocess.DEVNULL, timeout=5).decode().strip()

    def measure(label, processes):
        receipt["idle_samples"].append(idle_sample(label, processes))
        save(out / "resource-samples.json", receipt["idle_samples"])

    def close_viewer(process, window, label):
        started = time.monotonic()
        request_window_close(env["DISPLAY"], window)
        process.wait(timeout=5)
        elapsed = time.monotonic() - started
        if process.returncode != 0 or elapsed >= 5:
            raise RuntimeError(f"viewer close failed: status={process.returncode}, seconds={elapsed}")
        receipt.setdefault("close_elapsed_seconds", {})[label] = elapsed
        save(out / "close-times.json", receipt["close_elapsed_seconds"])

    def alive(expected, label):
        if host.poll() is not None:
            raise RuntimeError(f"owner exited during {label}")
        snapshot = admin.snapshot()
        if identity(snapshot) != expected:
            raise RuntimeError(f"owner/child/native identity changed during {label}")
        os.kill(expected["pid"], 0)
        save(out / f"{label}.json", snapshot)
        return snapshot

    def await_lease_release():
        def claim():
            result = admin.mutate({"type": "claim_control"})
            return result.get("type") == "ok"
        until("released controller lease", claim)
        result = admin.mutate({"type": "release_control"})
        if result.get("type") != "ok":
            raise RuntimeError(f"release refused: {result}")

    def await_count(wanted, label):
        latest = None
        def observed():
            nonlocal latest
            latest = admin.snapshot()
            return latest if count(latest) == wanted else None
        try:
            return until(label, observed)
        finally:
            if latest is not None:
                save(out / f"{label}.json", latest)

    def type_text(window, text):
        # winit's X11 key path requires an active window. Xvfb has no window
        # manager to focus it automatically. XTEST then follows real input.
        xdotool("windowfocus", "--sync", window)
        if int(xdotool("getwindowfocus")) != int(window):
            raise RuntimeError("viewer never received X11 keyboard focus")
        xdotool("type", "--clearmodifiers", text)

    def viewer(label, observe=False):
        image = out / f"{label}.png"
        command = [str(binaries["ULTTY"]), "--attach", name, "--screenshot", str(image)]
        if observe:
            command += ["--observe", "--token-file", str(runtime / "observer.token")]
        else:
            command += ["--exit-after-ms", "12000"]
        process = subprocess.Popen(command, env=env, stdout=log(label), stderr=subprocess.STDOUT)
        viewers.append(process)
        def window():
            if process.poll() is not None:
                raise RuntimeError(f"viewer {label} exited {process.returncode} before drawing")
            try:
                matches = xdotool("search", "--onlyvisible", "--pid", process.pid)
                return matches.splitlines()[0] if matches else None
            except subprocess.CalledProcessError:
                return None
        identifier = until(f"{label} window", window)
        role = "observer" if observe else "controller"
        until(f"{label} role", lambda: role in xdotool("getwindowname", identifier))
        receipt["screenshots"].append(until(f"{label} rendered scene", lambda: screenshot(image)))
        return process, identifier

    try:
        display_file = open(out / "display", "w+b")
        files.append(display_file)
        xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(display_file.fileno()), "-screen", "0", "1280x800x24",
                                 "-nolisten", "tcp", "-noreset"], pass_fds=(display_file.fileno(),), stdout=log("xvfb"),
                                stderr=subprocess.STDOUT)
        def display_ready():
            if xvfb.poll() is not None:
                raise RuntimeError("Xvfb exited before display handoff")
            value = (out / "display").read_text()
            if not value.endswith("\n"):
                return None
            number = value.splitlines()[0]
            return number if number.isdigit() else None
        env["DISPLAY"] = f":{until('Xvfb display', display_ready)}"
        host = subprocess.Popen([str(binaries["ULTTY_SESSION"]), "host", name,
                                 "-e", str(binaries["COUNTER_APP"])], env=env,
                                stdout=log("host"), stderr=subprocess.STDOUT)
        endpoint = runtime / f"ultty-session-{name}.sock"
        credential = endpoint.with_suffix(".admin")
        until("owner startup", lambda: credential.is_file() and endpoint.exists())
        admin = Client(endpoint, credential.read_text().strip(), name)
        def populated():
            snapshot = admin.snapshot()
            return snapshot if identity(snapshot) and count(snapshot) == 0 else None
        first = until("committed native counter", populated)
        expected = identity(first)
        save(out / "initial.json", first)
        granted = admin.mutate({"type": "grant", "capabilities": ["read"], "ttl_seconds": 300})
        if granted.get("type") != "granted":
            raise RuntimeError(f"read grant refused: {granted}")
        observer_file = runtime / "observer.token"
        descriptor = os.open(observer_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as secret:
            secret.write(granted["token"])
        observer = Client(endpoint, granted["token"], name)
        refusal = observer.mutate({"type": "input", "pane": expected["pane"], "data": [43]})
        if refusal.get("type") != "error" or refusal.get("code") != "permission_denied":
            raise RuntimeError(f"observer input was not refused: {refusal}")
        receipt["observer_input"] = {key: refusal[key] for key in ("type", "code")}
        one, window = viewer("first")
        measure("controller-idle", {"host": host, "viewer": one})
        # Let the first controller resize and the next observation converge.
        time.sleep(0.25)
        type_text(window, "+")
        await_count(1, "gui-key-count")
        type_text(window, "+" * 12)
        await_count(13, "gui-burst-count")
        close_viewer(one, window, "first")
        await_lease_release()
        alive(expected, "after-graceful-close")
        receipt["phases"].append("graceful close preserved owner, child, native session and count")
        two, _ = viewer("before-kill")
        two.kill()
        two.wait(timeout=5)
        await_lease_release()
        alive(expected, "after-viewer-kill")
        receipt["phases"].append("SIGKILL viewer preserved owner, child and native session")
        measure("owner-without-gui-viewer", {"host": host})
        read_only, window = viewer("observer", observe=True)
        measure("observer-idle", {"host": host, "viewer": read_only})
        type_text(window, "+")
        time.sleep(0.25)
        if count(admin.snapshot()) != 13:
            raise RuntimeError("observer GUI mutated the native counter")
        read_only.terminate()
        read_only.wait(timeout=5)
        measure("owner-after-observer-close", {"host": host})
        third, window = viewer("reattached")
        close_viewer(third, window, "reattached")
        if count(alive(expected, "reattached")) != 13:
            raise RuntimeError("reattached controller lost the native application state")
        receipt["phases"].append("observer GUI and wire mutations refused; controller reattached to count=13")
        result = admin.mutate({"type": "terminate"})
        if result.get("type") != "ok":
            raise RuntimeError(f"explicit owner termination refused: {result}")
        host.wait(timeout=5)
        if host.returncode != 0:
            raise RuntimeError(f"owner exited {host.returncode}")
        def child_gone():
            try:
                os.kill(expected["pid"], 0)
                return False
            except ProcessLookupError:
                return True
        until("owned counter reaped after explicit owner termination", child_gone, timeout=5)
        if any(digest(path) != receipt["binaries"][key]["sha256"] for key, path in binaries.items()):
            raise RuntimeError("binaries changed during the gate; this is not one frozen build")
        receipt["identity"] = expected
        receipt["result"] = "pass"
        save(out / "receipt.json", receipt)
        print(f"PASS viewer lifecycle owner={expected['owner']['pid']} child={expected['pid']} native={expected['native']}")
    finally:
        for process in viewers:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        if host is not None and host.poll() is None and admin is not None:
            try:
                admin.snapshot()  # Observe mutations from the just-closed admin viewer.
                result = admin.mutate({"type": "terminate"})
                if result.get("type") != "ok":
                    raise RuntimeError(f"fixture termination refused: {result}")
                host.wait(timeout=5)
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                (out / "cleanup.log").write_text(f"explicit fixture termination failed: {error}\n")
        for client in (observer, admin):
            if client is not None:
                client.sock.close()
        for process in (host, xvfb):
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
        for stream in files:
            stream.close()
        shutil.rmtree(runtime)


if __name__ == "__main__":
    main()
