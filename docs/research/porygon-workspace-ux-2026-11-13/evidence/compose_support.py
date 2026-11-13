"""Actual-terminal acceptance gate for the Porygon composer UX slice.

Run from porygon-ux-land/porygon-ai with the pytest environment already set up:
  PORYGON_NATIVE_TEST_BINARY=/absolute/path/to/porygon-tui \
  PORYGON_ACCEPTANCE_WIDTH=80 uv run --no-sync pytest -q -s \
    /tmp/porygon-compose-verification/test_compose_pty.py

Repeat with PORYGON_ACCEPTANCE_WIDTH=160. Every invocation writes the complete
PTY stream and JSON receipt under /tmp/porygon-compose-verification/receipts/.
The fake session is provider-free and the daemon binds loopback only.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import pty
import re
import select
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import termios
import types
from datetime import datetime
from pathlib import Path

import pytest
import uvicorn

PROJECT = Path(os.environ.get(
    "PORYGON_SOURCE_ROOT",
    "/home/travers/projects/porygon-ai/porygon-ux-land/porygon-ai",
)).resolve()
sys.path.insert(0, str(PROJECT))
sys.path.insert(0, str(PROJECT / "tests"))
sys.path.insert(0, str(PROJECT / "src"))
# Some installed packages also publish a top-level `tests` package. The
# project uses a namespace tests directory, so pin this module to that tree.
tests_package = types.ModuleType("tests")
tests_package.__path__ = [str(PROJECT / "tests")]
sys.modules["tests"] = tests_package

# Run the project test bootstrap before importing Porygon modules: it scrubs
# provider credentials, installs the child-process network guard, and isolates
# data/config paths before their import-time defaults are captured.
pytest_plugins = ["conftest"]
import conftest  # noqa: F401,E402

from porygon.server import auth, main
from porygon.server.conversations import ConversationStore
from porygon.events import TokenEvent
from test_native_client_contract import _no_lifespan


ROOT = Path(os.environ.get(
    "PORYGON_ARTIFACT_ROOT", "/tmp/porygon-compose-verification"
)).resolve()
RECEIPTS = ROOT / "receipts"
TOKEN = "porygon-compose-pty-fixture-token"


class ScriptedSession:
    """Keep turn one busy until cancelled/released; record exact request bodies."""

    def __init__(self) -> None:
        self.messages: list[str] = []
        self.started = threading.Event()
        self.cancelled = threading.Event()
        self.release_first = threading.Event()

    async def process_request_async(self, message, **kwargs):
        self.messages.append(message)
        n = len(self.messages)
        if n == 1:
            self.started.set()
            yield TokenEvent(text="assistant-body-first")
            import asyncio
            cancel_event = kwargs["cancel_event"]
            while not cancel_event.is_set() and not self.release_first.is_set():
                await asyncio.sleep(.02)
            if cancel_event.is_set():
                self.cancelled.set()
                return
            yield TokenEvent(text="assistant-body-finished")
        else:
            yield TokenEvent(text=f"assistant-body-turn-{n}")


class PtyApp:
    def __init__(self, binary: Path, width: int, app, session: ScriptedSession):
        self.binary, self.width, self.height, self.app, self.session = binary, width, (24 if width == 80 else 48), app, session
        self.capture_dir = RECEIPTS / f"stages-{width}-{time.time_ns()}"
        self.capture_dir.mkdir(parents=True, exist_ok=False)
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", int(os.environ.get("PORYGON_FIXTURE_PORT", "0"))))
        self.listener.listen(128)
        self.server = uvicorn.Server(uvicorn.Config(
            app, log_level="warning", lifespan="off", access_log=False
        ))
        self.thread = threading.Thread(
            target=self.server.run, kwargs={"sockets": [self.listener]}, daemon=True
        )
        self.master = self.slave = self.proc = None
        self.output = bytearray()

    def __enter__(self):
        self.thread.start()
        deadline = time.monotonic() + 10
        while not self.server.started and self.thread.is_alive() and time.monotonic() < deadline:
            time.sleep(.01)
        assert self.server.started, "fixture daemon did not start"
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ,
                    struct.pack("HHHH", self.height, self.width, 0, 0))
        env = os.environ.copy()
        env.update(TERM="xterm-256color", PORYGON_TOKEN=TOKEN,
                   PORYGON_URL=f"http://127.0.0.1:{self.listener.getsockname()[1]}",
                   PORYGON_WEATHER="0", PORYGON_CONTRACT_FIXTURE="1")
        for key in ("ULTTY_SOCKET", "ULTTY_TTY", "ULTTY_FORCE_NATIVE", "ULTTY_IN_BAND", "TMUX", "STY"):
            env.pop(key, None)
        self.proc = subprocess.Popen(
            [str(self.binary)], stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env=env, start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(self.slave)
        self.slave = None
        return self

    def __exit__(self, exc_type, exc, tb):
        RECEIPTS.mkdir(parents=True, exist_ok=True)
        stamp = f"{self.width}-{time.time_ns()}"
        (RECEIPTS / f"{stamp}.pty.bin").write_bytes(self.output)
        (RECEIPTS / f"{stamp}.screen.txt").write_text(self.screen())
        (self.capture_dir / "final.pty.bin").write_bytes(self.output)
        (self.capture_dir / "final.screen.txt").write_text(self.screen())
        if self.proc and self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGTERM)
            self.proc.wait(timeout=5)
        if self.master is not None:
            os.close(self.master)
        self.server.should_exit = True
        self.thread.join(10)
        self.listener.close()

    def collect(self, duration=.1):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], min(.05, end-time.monotonic()))
            if ready:
                try:
                    self.output.extend(os.read(self.master, 65536))
                except OSError:
                    return

    def send(self, data: bytes, settle=.2):
        os.write(self.master, data)
        self.collect(settle)

    def save_stage(self, name):
        self.collect(.2)
        (self.capture_dir / f"{name}.pty.bin").write_bytes(self.output)
        (self.capture_dir / f"{name}.screen.txt").write_text(self.screen())

    def screen(self) -> str:
        """Replay common cursor/erase VT ops into current 24xwidth screen."""
        rows = [[" "] * self.width for _ in range(self.height)]
        y = x = 0
        raw = self.output.decode("utf-8", "replace")
        token = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*(?:\x07)|.", re.S)
        for match in token.finditer(raw):
            s = match.group()
            if s.startswith("\x1b["):
                args = s[2:-1].lstrip("?")
                vals = [int(v) if v else 0 for v in args.split(";")] if all(v.isdigit() or not v for v in args.split(";")) else [0]
                a, c = vals[0] if vals else 0, s[-1]
                if c in "Hf":
                    y = max(0, (a or 1)-1); x = max(0, ((vals[1] if len(vals)>1 else 1) or 1)-1)
                elif c == "G": x = max(0, (a or 1)-1)
                elif c == "d": y = max(0, (a or 1)-1)
                elif c == "A": y = max(0, y-(a or 1))
                elif c == "B": y = min(self.height-1, y+(a or 1))
                elif c == "C": x = min(self.width-1, x+(a or 1))
                elif c == "D": x = max(0, x-(a or 1))
                elif c == "J" and a in (2, 3): rows = [[" "]*self.width for _ in range(self.height)]
                elif c == "K" and y < self.height:
                    if a == 0: rows[y][x:] = [" "] * (self.width-x)
                    elif a == 1: rows[y][:x+1] = [" "] * (x+1)
                    elif a == 2: rows[y] = [" "]*self.width
            elif not s.startswith("\x1b"):
                for ch in s:
                    if ch == "\r": x = 0
                    elif ch == "\n": y = min(23, y+1)
                    elif ch == "\b": x = max(0, x-1)
                    elif ord(ch) >= 32 and y < self.height and x < self.width:
                        rows[y][x] = ch; x += 1
        return "\n".join("".join(row).rstrip() for row in rows)

    def wait_screen(self, needle, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.collect(.1)
            if needle in self.screen():
                return
        raise AssertionError(f"screen missing {needle!r}\n{self.screen()}")

    def wait_calls(self, count, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.collect(.1)
            if len(self.session.messages) >= count:
                return
        raise AssertionError(f"daemon saw {self.session.messages!r}, wanted {count} calls")

    def resize(self, width: int, height: int):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", height, width, 0, 0))
        self.width, self.height = width, height
        if self.proc and self.proc.poll() is None:
            try:
                foreground = os.tcgetpgrp(self.master)
                os.killpg(foreground, signal.SIGWINCH)
            except (OSError, ProcessLookupError):
                pass


@pytest.mark.timeout(75)
def test_actual_pty_compose_contract(mock_context, mock_database, monkeypatch):
    binary_value = os.environ.get("PORYGON_NATIVE_TEST_BINARY", "")
    assert binary_value, "set PORYGON_NATIVE_TEST_BINARY to the already-built client"
    binary = Path(binary_value).resolve()
    assert binary.is_file(), f"binary not found: {binary}"
    width = int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80"))
    assert width in (80, 160)

    session = ScriptedSession()
    monkeypatch.setattr(mock_context.user, "_session", session)
    monkeypatch.setattr(auth, "_daemon_token", TOKEN)
    monkeypatch.setattr(auth, "_api_keys", {})
    app = main.create_app()
    authenticated_status = threading.Event()

    @app.middleware("http")
    async def observe_authenticated_status(request, call_next):
        response = await call_next(request)
        if (request.url.path == "/status"
                and request.headers.get("authorization") == f"Bearer {TOKEN}"
                and response.status_code == 200):
            authenticated_status.set()
        return response

    app.router.lifespan_context = _no_lifespan
    app.state.context = mock_context
    app.state.start_time = datetime.now()
    app.state.mcp_manager = None
    app.state.conversation_store = ConversationStore()

    with PtyApp(binary, width, app, session) as term:
        if os.environ.get("PORYGON_ACCEPTANCE_BASELINE") == "1":
            failures = []

            def check(label, ok, evidence):
                if not ok:
                    failures.append({"gate": label, "evidence": evidence})

            def visible(label, timeout=6):
                try:
                    term.wait_screen(label, timeout=timeout)
                    return True
                except AssertionError:
                    return False

            def calls(count, timeout=8):
                try:
                    term.wait_calls(count, timeout)
                    return True
                except AssertionError:
                    return False

            term.collect(.5)
            initial = term.screen()
            legacy_splash = "Press ENTER to continue" in initial
            check("direct_startup", not legacy_splash,
                  "legacy splash prompt visible" if legacy_splash else initial)
            if legacy_splash:
                # Diagnostic-only escape hatch: record the startup regression,
                # then dismiss it so the same frozen binary can reach content
                # and composer gates. Candidate acceptance never takes it.
                term.send(b"\r")
            check("prompt_ready", visible("Message Porygon", 10) or visible("P-Chat", 1), term.screen())
            check("authenticated_status", authenticated_status.wait(8),
                  "GET /status with fixture bearer token returned 200")
            check("connected_indicator", visible("Porygon connected", 1) or visible("●", 4), term.screen())

            term.send(b"\t")
            term.send(b"first-line")
            check("first_draft_visible", visible("first-line", 4), term.screen())
            term.send(b"\r")
            check("first_request_received", calls(1, 8), session.messages)
            check("first_response_started", session.started.wait(3), session.messages)
            check("visible_user_body", visible("first-line", 5), term.screen())
            check("visible_assistant_body", visible("assistant-body-first", 5), term.screen())

            term.send(b"\x1b[200~paste-alpha\npaste-beta\x1b[201~")
            term.collect(.4)
            check("paste_did_not_submit", len(session.messages) == 1, session.messages)
            screen = term.screen()
            check("paste_visible_as_draft",
                  "paste-alpha" in screen and "paste-beta" in screen, screen)

            term.send(b"\x18\r")
            term.send(b"chord-line")
            term.collect(.4)
            check("newline_chord_did_not_submit", len(session.messages) == 1, session.messages)
            screen = term.screen()
            check("newline_chord_draft_visible",
                  all(part in screen for part in ("paste-alpha", "paste-beta", "chord-line")), screen)

            term.send(b"\r")
            term.collect(.4)
            check("busy_enter_did_not_submit", len(session.messages) == 1, session.messages)
            screen = term.screen()
            check("busy_draft_preserved",
                  all(part in screen for part in ("paste-alpha", "paste-beta", "chord-line")), screen)

            session.release_first.set()
            check("first_assistant_completion_visible",
                  visible("assistant-body-finished", 8), term.screen())
            term.send(b"\r")
            got_second = calls(2, 8)
            check("retained_draft_submitted", got_second, session.messages)
            body = session.messages[1] if len(session.messages) > 1 else ""
            check("multiline_body_received",
                  all(part in body for part in ("paste-alpha", "paste-beta", "chord-line"))
                  and body.count("\n") >= 2, repr(body))
            check("second_assistant_body_visible",
                  visible("assistant-body-turn-2", 8), term.screen())

            new_width, new_height = (160, 48) if width == 80 else (80, 24)
            term.resize(new_width, new_height)
            term.collect(.6)
            receipt = {"mode": "baseline-diagnostic", "initial_geometry": [width, term.height],
                       "resized_geometry": [new_width, new_height], "messages": session.messages,
                       "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                       "fixture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                       "gate_failures": failures, "final_screen": term.screen()}
            out = RECEIPTS / f"parent-{width}-baseline-diagnostic.json"
            out.write_text(json.dumps(receipt, indent=2))
            print(json.dumps(receipt))
            assert not failures, f"baseline acceptance failures: {json.dumps(failures)}"
            return

        # New direct startup is authenticated and prompt-ready. Never accept the
        # historical boot splash as application readiness.
        prompt_ready = False
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            term.collect(.1)
            if "Message Porygon" in term.screen() or "P-Chat" in term.screen():
                prompt_ready = True
                break
        assert prompt_ready, f"composer prompt missing after authenticated startup:\n{term.screen()}"
        assert authenticated_status.wait(8), "client did not complete authenticated GET /status"
        term.wait_screen("Porygon connected", timeout=10)
        assert "Press ENTER to continue" not in term.screen()
        term.send(b"\x1bOP")
        term.wait_screen("Keys and commands", timeout=5)
        term.wait_screen("Ctrl+X", timeout=5)
        term.save_stage("help-visible")
        term.send(b"\x1b")
        term.wait_screen("Message Porygon", timeout=5)
        term.save_stage("help-dismissed")
        # A user submission must be visible in the transcript, and the scripted
        # assistant body must be visible while the request remains busy.
        term.send(b"\t")
        term.send(b"first-line")
        term.wait_screen("first-line", timeout=5)
        term.send(b"\r")
        term.wait_calls(1)
        assert session.started.wait(3)
        term.wait_screen("first-line")
        term.wait_screen("assistant-body-first")
        assistant_first_visible = True
        term.save_stage("assistant-first-streaming")

        # Paste two lines as a terminal bracketed-paste transaction. It must
        # remain a draft until a distinct Enter, with no additional daemon call.
        term.send(b"\x1b[200~paste-alpha\npaste-beta\x1b[201~")
        term.collect(.4)
        assert len(session.messages) == 1, f"paste submitted unexpectedly: {session.messages!r}"
        screen = term.screen()
        assert "paste-alpha" in screen and "paste-beta" in screen, screen

        # Product chord: Ctrl+X then Enter inserts a line break into the draft.
        term.send(b"\x18\r")
        term.send(b"chord-line")
        term.collect(.4)
        assert len(session.messages) == 1, f"newline chord submitted: {session.messages!r}"
        screen = term.screen()
        assert all(part in screen for part in ("paste-alpha", "paste-beta", "chord-line")), screen
        term.save_stage("multiline-draft-busy")

        # Enter while the first turn is in flight is disabled; draft survives.
        term.send(b"\r")
        term.collect(.5)
        assert len(session.messages) == 1, f"busy Enter submitted: {session.messages!r}"
        assert "paste-alpha" in term.screen() and "paste-beta" in term.screen(), term.screen()

        # Native and ANSI Ctrl+C must stop active work through the daemon and
        # leave the independently composed draft in the input.
        term.send(b"\x03")
        deadline = time.monotonic() + 10
        while not session.cancelled.is_set() and time.monotonic() < deadline:
            term.collect(.1)
        assert session.cancelled.is_set(), "Ctrl+C did not reach the daemon cancel event"
        term.wait_screen("run cancelled", timeout=10)
        screen = term.screen()
        assert all(part in screen for part in ("paste-alpha", "paste-beta", "chord-line")), screen
        term.save_stage("cancelled-draft-preserved")

        # Submit the retained multiline draft after the cancellation is resolved.
        term.send(b"\r")
        term.wait_calls(2)
        body = session.messages[1]
        expected_body = "paste-alpha\npaste-beta\nchord-line"
        assert body == expected_body, f"unexpected exact submission: {body!r}"
        term.wait_screen("assistant-body-turn-2", timeout=10)
        assistant_second_visible = True
        term.save_stage("assistant-second-visible")

        # Exercise an actual PTY resize and require the app to keep its prompt
        # and transcript usable at the new geometry.
        new_width, new_height = (160, 48) if width == 80 else (80, 24)
        term.resize(new_width, new_height)
        term.wait_screen("Enter", timeout=5)
        assert len(session.messages) == 2
        term.save_stage("resized-prompt")

        # Idle Ctrl+C with an unsent draft is a warning first. Confirm only
        # with a second Ctrl+C, then ensure the app exits cleanly.
        term.send(b"unsent-draft")
        term.wait_screen("unsent-draft", timeout=5)
        term.send(b"\x03")
        term.wait_screen("Unsent draft", timeout=5)
        assert term.proc.poll() is None, "first idle Ctrl+C exited instead of warning"
        assert "unsent-draft" in term.screen(), term.screen()
        term.save_stage("idle-draft-warning")
        term.send(b"\x03")
        term.proc.wait(timeout=8)

        receipt = {"width": width, "calls": session.messages,
                   "source_root": str(PROJECT), "artifact_root": str(ROOT),
                   "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                   "fixture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                   "assistant_first_visible_at_stage": assistant_first_visible,
                   "assistant_second_visible_at_stage": assistant_second_visible,
                   "multiline_submitted_exactly": body == expected_body,
                   "busy_stop_cancelled": session.cancelled.is_set(),
                   "idle_quit_warning_kept_draft": True,
                   "idle_quit_confirmed": term.proc.poll() is not None,
                   "resize_survived": True,
                   "stage_dir": str(term.capture_dir)}
        (RECEIPTS / "latest.json").parent.mkdir(parents=True, exist_ok=True)
        (RECEIPTS / "latest.json").write_text(json.dumps(receipt, indent=2))
        print(json.dumps(receipt))
