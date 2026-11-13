#!/usr/bin/env python3
"""Exercise a real headless owner, native app and attachment reconnect.

Set ULTTY_SESSION, COUNTER_APP, INLINE_APP and FRAME_APP to built binaries. TMPDIR must provide a
short private path suitable for Unix sockets. No display or network is used.
"""

import json
import hashlib
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time


def read_exact(sock, length):
    data = b""
    while len(data) < length:
        part = sock.recv(length - len(data))
        if not part:
            raise RuntimeError("attachment closed mid-frame")
        data += part
    return data


class Client:
    def __init__(self, path, token, session):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect(str(path))
        self.owner = None
        self.grant = None
        self.next_id = 1
        result = self.exchange(0, {"type": "hello", "version": 1, "session": session,
                                   "token": token, "controller": True})
        if result["reply"]["type"] != "welcome":
            raise RuntimeError(f"hello refused: {result['reply']}")
        self.owner = result["owner"]
        self.grant = result["reply"]["grant_id"]
        self.next_id = result["last_mutation_id"] + 1

    def exchange(self, request_id, command):
        body = json.dumps({"id": request_id, "owner": self.owner, "grant_id": self.grant,
                           "command": command}, separators=(",", ":")).encode()
        self.sock.sendall(b"UAT1" + struct.pack("<I", len(body)) + body)
        header = read_exact(self.sock, 8)
        if header[:4] != b"UAT1":
            raise RuntimeError("bad attachment frame")
        size = struct.unpack("<I", header[4:])[0]
        if not 0 < size <= 16 * 1024 * 1024:
            raise RuntimeError("invalid response size")
        result = json.loads(read_exact(self.sock, size))
        if result["id"] != request_id:
            raise RuntimeError("uncorrelated response")
        return result

    def snapshot(self):
        response = self.exchange(0, {"type": "snapshot", "after_sequence": None})
        if response["reply"]["type"] != "snapshot":
            raise RuntimeError(f"snapshot refused: {response['reply']}")
        return response["reply"]["snapshot"]

    def mutate(self, command):
        response = self.exchange(self.next_id, command)
        self.next_id += 1
        return response["reply"]


def until(deadline, description, action):
    last = None
    while time.monotonic() < deadline:
        try:
            result = action()
            if result:
                return result
        except (OSError, RuntimeError, KeyError) as error:
            last = error
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for {description}: {last}")


def main():
    host = os.environ["ULTTY_SESSION"]
    counter = os.environ["COUNTER_APP"]
    inline = os.environ["INLINE_APP"]
    frame_app = os.environ["FRAME_APP"]
    for label, path in (("host", host), ("counter", counter), ("inline", inline), ("frame", frame_app)):
        with open(path, "rb") as binary:
            print(f"{label}_sha256={hashlib.file_digest(binary, 'sha256').hexdigest()}", flush=True)
    session = "smoke"
    with tempfile.TemporaryDirectory(prefix="uh-") as runtime:
        os.chmod(runtime, 0o700)
        env = dict(os.environ, XDG_RUNTIME_DIR=runtime)
        process = subprocess.Popen([host, "host", session, "-e", counter], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        endpoint = Path(runtime) / f"ultty-session-{session}.sock"
        credential = endpoint.with_suffix(".admin")
        client = None
        try:
            until(time.monotonic() + 15, "owner startup", lambda: credential.is_file() and endpoint.exists())
            token = credential.read_text().strip()
            client = Client(endpoint, token, session)

            def populated():
                snapshot = client.snapshot()
                if snapshot["panes"] and snapshot["panes"][0]["native"]:
                    native = snapshot["panes"][0]["native"][0]
                    if native["regions"]:
                        return snapshot
                return None

            first = until(time.monotonic() + 15, "committed native scene", populated)
            pane = first["panes"][0]
            native = pane["native"][0]
            # Keyboard events use current focus and must survive a stale
            # observation while a fast app renders successive frames.
            key_payload = struct.pack("<IBBIBffB", 0, 4, 0, ord("+"), 0, 0.0, 0.0, 0)
            key_frame = b"UT" + struct.pack("<HI", 0x20, len(key_payload)) + key_payload
            sent = client.mutate({"type": "native_input", "session": native["session_id"],
                                  "generation": native["generation"] + 1000000, "payload": list(key_frame)})
            if sent.get("type") != "ok":
                raise RuntimeError(f"keyboard event was incorrectly fenced by generation: {sent}")
            # Valid native mouse-move frame, intentionally fenced by a future generation.
            payload = struct.pack("<IBffB", 0, 2, 0.0, 0.0, 0)
            frame = b"UT" + struct.pack("<HI", 0x20, len(payload)) + payload
            refusal = client.mutate({"type": "native_input", "session": native["session_id"],
                                    "generation": native["generation"] + 1000000, "payload": list(frame)})
            if refusal.get("type") != "error" or refusal.get("code") != "conflict":
                raise RuntimeError(f"stale generation was not rejected: {refusal}")
            client.sock.close()
            client = None
            time.sleep(0.2)
            if process.poll() is not None:
                raise RuntimeError("owner exited when viewer disconnected")
            client = until(time.monotonic() + 10, "controller reacquisition",
                           lambda: Client(endpoint, token, session))
            second = until(time.monotonic() + 10, "reattached native scene", populated)
            if second["owner"] != first["owner"] or second["panes"][0]["pid"] != pane["pid"]:
                raise RuntimeError("owner or child identity changed across attachment")
            if second["panes"][0]["native"][0]["session_id"] != native["session_id"]:
                raise RuntimeError("native application session was replaced")
            opened = client.mutate({"type": "spawn", "program": "/usr/bin/env",
                                    "args": ["-u", "ULTTY_SOCKET", inline, "30"], "cwd": None})
            if opened.get("type") != "applied" or opened.get("pane") is None:
                raise RuntimeError(f"inline producer launch failed: {opened}")
            inline_pane = opened["pane"]

            def inline_placed():
                snapshot = client.snapshot()
                for candidate in snapshot["panes"]:
                    if candidate["id"] == inline_pane and any(
                            native["placed_surfaces"] for native in candidate["native"]):
                        return snapshot
                return None

            placed = until(time.monotonic() + 15, "in-band inline surface", inline_placed)
            hidden = next(candidate for candidate in placed["panes"] if candidate["id"] == pane["id"])
            if hidden["pid"] != pane["pid"] or not any(
                    scene["session_id"] == native["session_id"] for scene in hidden["native"]):
                raise RuntimeError("hidden tab lost its native session in the snapshot")
            time.sleep(0.3)
            # A held, unchanged inline surface must not invalidate every
            # snapshot merely because the pairing pass ran again.
            current = client.snapshot()
            time.sleep(0.2)
            idle = client.exchange(0, {"type": "snapshot", "after_sequence": current["sequence"]})["reply"]
            if idle.get("type") != "unchanged" or idle.get("sequence") != current["sequence"]:
                raise RuntimeError(f"idle inline snapshot churned: {idle.get('type')}")
            if not placed["panes"]:
                raise RuntimeError("inline snapshot lost its panes")
            release = Path(runtime) / "release-frame"
            launched = client.mutate({"type": "spawn", "program": frame_app,
                                      "args": [str(release)], "cwd": None})
            if launched.get("type") != "applied" or launched.get("pane") is None:
                raise RuntimeError(f"frame producer launch failed: {launched}")
            frame_pane = launched["pane"]
            until(time.monotonic() + 10, "open frame", lambda: Path(str(release) + ".ready").exists())

            def frame_text(needle):
                snapshot = client.snapshot()
                for candidate in snapshot["panes"]:
                    if candidate["id"] == frame_pane:
                        for scene in candidate["native"]:
                            text = json.dumps([region["text"] for region in scene["regions"]]).encode()
                            if needle in text:
                                return text
                return None

            committed = until(time.monotonic() + 10, "committed frame before release",
                              lambda: frame_text(b"FRAME_COMMITTED"))
            if b"FRAME_PENDING" in committed:
                raise RuntimeError("uncommitted native frame leaked into snapshot")
            release.touch()
            until(time.monotonic() + 10, "committed second frame",
                  lambda: frame_text(b"FRAME_PENDING"))
            result = client.mutate({"type": "terminate"})
            if result.get("type") != "ok":
                raise RuntimeError(f"termination refused: {result}")
            process.wait(timeout=5)
            if process.returncode != 0:
                raise RuntimeError(f"host exited {process.returncode}")
            print(f"PASS owner={first['owner']['pid']} child={pane['pid']} native={native['session_id']}")
        finally:
            if client is not None:
                client.sock.close()
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            log = process.stderr.read().decode(errors="replace")
            if process.returncode and log:
                print(log, file=__import__("sys").stderr)


if __name__ == "__main__":
    main()
