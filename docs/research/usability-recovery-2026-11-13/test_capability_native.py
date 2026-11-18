"""Native Ultty/Xvfb acceptance for Porygon's composer and transcript.

Requires already-built ULTTY_BINARY, ULTTYCTL_BINARY and
PORYGON_NATIVE_TEST_BINARY. Runs the GPU UI on a private Xvfb + lavapipe,
starts no provider, and binds only a synthetic daemon to 127.0.0.1.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import os
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import types
from datetime import datetime, timedelta
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
tests_package = types.ModuleType("tests")
tests_package.__path__ = [str(PROJECT / "tests")]
sys.modules["tests"] = tests_package
pytest_plugins = ["conftest"]
import conftest  # noqa: F401,E402

from porygon.events import TokenEvent
from porygon.server import auth, main
from porygon.server.conversations import ConversationStore
from test_native_client_contract import _no_lifespan


TOKEN = "porygon-native-gpu-fixture-token"
RAW_REPLY = "assistant body beta visible\n  code  spacing\nhttps://qualification.invalid/raw"
HISTORY_COUNT = 2
ARTIFACT_ROOT = Path(os.environ.get(
    "PORYGON_ARTIFACT_ROOT", "/tmp/porygon-compose-verification"
)).resolve()
OUT_ROOT = ARTIFACT_ROOT / "receipts"


class NativeSession:
    """Deterministic tool request through production dispatch, no model or shell."""
    def __init__(self):
        self.messages = []
        self.executions = []
        self.started = threading.Event()
        self.release_first = threading.Event()

    async def call(self, name, **arguments):
        self.executions.append((name, arguments))
        self.started.set()
        while not self.release_first.is_set():
            await asyncio.sleep(.02)
        return "terminal inspection fixture executed once"

    async def process_request_async(self, message, **kwargs):
        from porygon.events import ToolCallEvent, ToolResultEvent
        from porygon.security.admission import StaticRules, ToolCallAdmission, admission_context
        from porygon.security.types import ToolPermission
        from porygon.tools.executor import ToolExecutor, security_context
        from porygon.tools.toolbox import Toolbox
        self.messages.append(message)
        args = {"argv": ["ulttyctl", "--help"]}
        call_id = f"fixture-command-{len(self.messages)}"
        yield ToolCallEvent(tool_name="run_argv", arguments=args, call_id=call_id)
        toolbox = Toolbox(name="coding", tools=["run_argv"],
                          permissions={"run_argv": ToolPermission.AUTO})
        with security_context(approval_callback=kwargs["approval_callback"]):
            executor = ToolExecutor(None, toolbox, self)
        # Isolate this capability boundary from model risk classification.
        # Backend security tests separately exercise admission hard denials.
        context = json.loads(kwargs["task_context"])
        context["tool_call_id"] = call_id
        with admission_context(ToolCallAdmission(rules=StaticRules())):
            result = await executor.dispatch_tool_call("run_argv", args, json.dumps(context))
        yield ToolResultEvent(tool_name="run_argv", call_id=call_id,
                              result=result.text, success=result.success, duration_ms=0)
        yield TokenEvent(text="inspection fixture finished" if result.success else "denied; no command ran")


def walk_regions(value):
    if isinstance(value, dict):
        if "id" in value and "role" in value:
            yield value
        for child in value.values():
            yield from walk_regions(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk_regions(child)


def all_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from all_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from all_strings(child)


def _wait(predicate, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.1)
    raise AssertionError(f"timed out waiting for {label}")


def _type_native_text(text, *, display_env, window):
    focus = subprocess.run(
        ["xdotool", "windowfocus", "--sync", window],
        env=display_env, capture_output=True, text=True, timeout=10,
    )
    assert focus.returncode == 0, f"could not focus Ultty window {window}: {focus.stderr}"
    typed = subprocess.run(
        ["xdotool", "type", "--clearmodifiers", "--delay", "1", text],
        env=display_env, capture_output=True, text=True, timeout=20,
    )
    assert typed.returncode == 0, f"native X11 text injection failed: {typed.stderr}"


@pytest.mark.timeout(120)
def test_live_capability_native(mock_context, mock_database, monkeypatch):
    out = OUT_ROOT / f"native-{time.time_ns()}"
    out.mkdir(parents=True, exist_ok=False)
    binary = Path(os.environ.get("PORYGON_NATIVE_TEST_BINARY", "")).resolve()
    ultty = Path(os.environ.get("ULTTY_BINARY", "")).resolve()
    ulttyctl = Path(os.environ.get("ULTTYCTL_BINARY", "")).resolve()
    for label, path in (("Porygon binary", binary), ("Ultty binary", ultty), ("ulttyctl binary", ulttyctl)):
        assert str(path) != "." and path.is_file(), f"set path to already-built {label}: {path}"
    (out / "sha256.txt").write_text("\n".join(
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}"
        for path in (binary, ultty, ulttyctl, Path(__file__).resolve())
    ) + "\n")

    session = NativeSession()
    async def seed_workspace():
        from porygon.runs import Run, RunStatus, RunStore
        from porygon.async_tasks.store import TaskStore
        await ConversationStore().create_conversation(title="native-qual-alpha", conv_id="native-qual-alpha")
        await ConversationStore().add_message("native-qual-alpha", "user", "native-history-alpha")
        await ConversationStore().create_conversation(title="native-long-history", conv_id="native-long")
        from sqlalchemy import text
        base = datetime(2026, 1, 1)
        records = [{"id": f"native-long-{i}", "conversation_id": "native-long", "role": "user" if i % 2 == 0 else "assistant", "content": f"native-history-{i:05d}-boundary", "created_at": base + timedelta(seconds=i)} for i in range(HISTORY_COUNT)]
        async with mock_database.begin() as conn:
            await conn.execute(text("INSERT INTO messages (id,conversation_id,role,content,created_at) VALUES (:id,:conversation_id,:role,:content,:created_at)"), records)
            await conn.execute(text("UPDATE conversations SET message_count=:count WHERE id='native-long'"), {"count": HISTORY_COUNT})
        await RunStore().create_run(Run(id="native-qual-run", objective="native-review-objective", status=RunStatus.COMPLETED, result="native-result-body", metadata={"conversation_id": "native-qual-alpha"}))
        await RunStore().add_verification_report(report_id="native-qual-report", run_id="native-qual-run", report_type="qualification", passed=True, summary="native-verification-summary", checks=[{"name": "native-qual-check", "passed": True}], artifacts={"raw_log": "/fixture/native-qual.log"})
        store = TaskStore()
        return store

    task_store = asyncio.run(seed_workspace())
    monkeypatch.setattr(mock_context.user, "_session", session)
    monkeypatch.setattr(auth, "_daemon_token", TOKEN)
    monkeypatch.setattr(auth, "_api_keys", {})
    app = main.create_app()
    authenticated_status = threading.Event()
    approval_posts = []
    approval_post_bodies = []

    @app.middleware("http")
    async def observe_authenticated_status(request, call_next):
        if request.method == "POST" and "/approvals/" in request.url.path:
            approval_posts.append(request.url.path)
            approval_post_bodies.append(json.loads(await request.body()))
        response = await call_next(request)
        if (request.url.path == "/status/tui"
                and request.headers.get("authorization") == f"Bearer {TOKEN}"
                and response.status_code == 200):
            authenticated_status.set()
        return response

    app.router.lifespan_context = _no_lifespan
    app.state.context = mock_context
    app.state.start_time = datetime.now()
    app.state.mcp_manager = None
    app.state.conversation_store = ConversationStore()
    app.state.task_store = task_store

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    port = listener.getsockname()[1]
    server = uvicorn.Server(uvicorn.Config(app, log_level="warning", lifespan="off", access_log=False))
    server_thread = threading.Thread(target=server.run, kwargs={"sockets": [listener]}, daemon=True)
    server_thread.start()

    runtime = Path(tempfile.mkdtemp(prefix="pg-native-", dir="/tmp"))
    runtime.chmod(0o700)
    config_home = out / "config"
    data_home = out / "data"
    cache_home = out / "cache"
    state_home = out / "state"
    for path in (config_home, data_home, cache_home, state_home):
        path.mkdir()
    (config_home / "ultty").mkdir()
    (config_home / "ultty" / "config.toml").write_text(
        "[behavior]\naccessibility = true\ncursor_blink = false\n"
    )
    xvfb_log = (out / "xvfb.log").open("wb")
    ultty_log = (out / "ultty.log").open("wb")
    xvfb = ultty_proc = None
    try:
        _wait(lambda: server.started and server_thread.is_alive(), "fixture daemon startup")
        xvfb = subprocess.Popen(
            ["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=xvfb_log, text=True, start_new_session=True,
        )
        display_number = xvfb.stdout.readline().strip()
        assert display_number.isdigit(), f"private Xvfb failed; see {out / 'xvfb.log'}"

        env = os.environ.copy()
        for key in ("WAYLAND_DISPLAY", "SWAYSOCK", "ULTTY_SOCKET", "ULTTY_TTY", "ULTTY_FORCE_NATIVE", "ULTTY_IN_BAND", "TMUX", "STY"):
            env.pop(key, None)
        env.update(
            DISPLAY=f":{display_number}", XDG_RUNTIME_DIR=str(runtime),
            XDG_CONFIG_HOME=str(config_home), XDG_DATA_HOME=str(data_home),
            XDG_CACHE_HOME=str(cache_home), XDG_STATE_HOME=str(state_home),
            PORYGON_TOKEN=TOKEN, PORYGON_URL=f"http://127.0.0.1:{port}",
            PORYGON_WEATHER="0", SHELL="/bin/sh", PS1="$ ",
        )
        lvp = next(iter(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json")), None)
        if lvp:
            env["VK_ICD_FILENAMES"] = str(lvp)
        ultty_proc = subprocess.Popen(
            [str(ultty)], env=env, stdin=subprocess.DEVNULL,
            stdout=ultty_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        control_env = dict(env, ULTTY_PID=str(ultty_proc.pid))

        def ctl(*args, json_output=False):
            command = [str(ulttyctl), *( ["--json"] if json_output else []), *map(str, args)]
            result = subprocess.run(command, env=control_env, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, f"{command!r}: {result.stdout}\n{result.stderr}"
            return result.stdout

        def find_native_window():
            if ultty_proc.poll() is not None:
                return False
            result = subprocess.run(
                ["xdotool", "search", "--onlyvisible", "--pid", str(ultty_proc.pid)],
                env=env, capture_output=True, text=True, timeout=10,
            )
            if result.returncode == 0 and result.stdout.strip():
                return result.stdout.splitlines()[0]
            return False

        native_window = _wait(find_native_window, "private Ultty X11 window", timeout=30)

        control_socket = runtime / f"ultty-ctl-{ultty_proc.pid}.sock"
        _wait(lambda: control_socket.is_socket() if ultty_proc.poll() is None else False,
              "Ultty control socket", timeout=30)
        ctl("ping", json_output=True)

        def snapshot(name):
            raw = ctl("regions", json_output=True)
            value = json.loads(raw)
            (out / f"{name}.regions.json").write_text(json.dumps(value, indent=2))
            return value

        def retrieve_system_clipboard(name):
            capture = out / f"{name}.txt"
            ctl("new-tab", "/bin/sh", "-c", f"cat > {shlex.quote(str(capture))}; printf '\\nCLIPBOARD_CAPTURE_COMPLETE\\n'; sleep 60")
            time.sleep(.3)
            subprocess.run(["xdotool", "windowfocus", "--sync", native_window,
                            "key", "--clearmodifiers", "ctrl+shift+v"], env=env, check=True, timeout=10)
            ctl("send-key", "d", "ctrl")
            ctl("send-key", "d", "ctrl")
            _wait(lambda: "CLIPBOARD_CAPTURE_COMPLETE" in ctl("text"), "system clipboard capture completes")
            ctl("screenshot", out / f"{name}-shell.png")
            ctl("switch-tab", "1")
            return capture

        def strings(name):
            return list(all_strings(snapshot(name)))

        ctl("resize", "800", "480")
        # Seed the real system clipboard through the terminal's OSC52 path
        # before Porygon owns the native scene. Later paste uses X11 CtrlShiftV.
        clipboard = base64.b64encode(b"draft first\ndraft second").decode()
        ctl("send-text", f"printf '\\033]52;c;{clipboard}\\007'; printf '\\nCLIPBOARD_SEEDED\\n'")
        ctl("send-key", "enter")
        _wait(lambda: "CLIPBOARD_SEEDED" in ctl("text"), "system clipboard seed")
        time.sleep(.3)
        # Native protocol input uses semantic key injection; send-text only
        # writes to the pane PTY and is deliberately reserved for launching.
        ctl("send-text", (
            f"{shlex.quote(str(binary))}; status=$?; "
            "printf '\\nPORYGON_FIXTURE_EXIT:%s\\n' \"$status\""
        ))
        ctl("send-key", "enter")
        _wait(lambda: any("Message Porygon" in s or "chat-input" in s
                          for s in strings("app-start")), "native Porygon composer")
        assert authenticated_status.wait(10), "no authenticated GET /status/tui returned 200"
        def connected_status_visible():
            doc = snapshot("authenticated-connected")
            return doc if any("connected" in s.lower() for s in all_strings(doc)) else False

        connected_doc = _wait(connected_status_visible,
                              "authenticated connection status in native content")
        ctl("resize", "1150", "800")
        decisions = []
        for message, key in (("deny inspection", "n"), ("approve inspection", "y")):
            _type_native_text(message, display_env=env, window=native_window)
            _wait(lambda: any(message in s for s in strings("draft-" + key)), "complete draft")
            ctl("send-key", "enter")
            def approval_visible():
                doc = snapshot("approval-" + key)
                return doc if any("Approval details" in s for s in all_strings(doc)) else False
            _wait(approval_visible, "version-bound capability request", timeout=20)
            ctl("screenshot", out / ("approval-" + key + ".png"))
            listing = asyncio.run(task_store.list_pending_approvals(scopes=["tool_call"], limit=100))
            records = listing["items"]
            assert len(records) == 1, records
            record = records[0]
            assert record["context"]["reason"] == "missing_policy_binding", record
            assert record["context"]["params"] == {"argv": ["ulttyctl", "--help"]}
            assert session.executions == [], "command ran before explicit approval"
            version = record["decision_version"]
            ctl("send-key", key)
            endpoint = "deny" if key == "n" else "approve"
            decisions.append((f'/approvals/pending/{record["id"]}/{endpoint}', version))
            _wait(lambda: len(approval_posts) == len(decisions), "one HTTP decision")
            assert approval_posts[-1] == decisions[-1][0]
            assert approval_post_bodies[-1] == {"expected_version": version}
            if key == "n":
                _wait(lambda: any("denied; no command ran" in s for s in strings("denied")), "denied response")
                assert session.executions == []
                _wait(lambda: any("turn ended" in s for s in strings("denied-ended")), "denied turn ended")
            else:
                assert session.started.wait(5), "approved original invocation never resumed"
                assert len(session.executions) == 1
                pending = snapshot("execution-pending")
                assert not any("run completed" in s for s in all_strings(pending)), pending
                ctl("screenshot", out / "execution-pending.png")
                session.release_first.set()
                _wait(lambda: any("inspection fixture finished" in s for s in strings("finished")), "tool result")
                assert session.messages == ["deny inspection", "approve inspection"]
                assert len(session.executions) == 1
                import urllib.request
                replay = urllib.request.Request(
                    f"http://127.0.0.1:{port}" + decisions[-1][0],
                    data=json.dumps({"expected_version": version}).encode(),
                    headers={"Authorization": "Bearer " + TOKEN, "Content-Type": "application/json"},
                    method="POST")
                with urllib.request.urlopen(replay, timeout=5) as response:
                    assert json.load(response)["status"] == "stale"
                assert len(session.executions) == 1
                ctl("screenshot", out / "finished.png")
        assert not asyncio.run(task_store.has_policy_binding(principal_id="local", capability="tool:run_argv"))
        receipt = {"messages": session.messages, "executions": session.executions,
                   "versioned_decisions": decisions, "no_policy_binding_created": True,
                   "replay_refused": True, "provider_calls": 0, "real_shell_calls": 0}
        (out / "receipt.json").write_text(json.dumps(receipt, indent=2))
        print(json.dumps({"artifact_dir": str(out), **receipt}))
    finally:
        session.release_first.set()
        if ultty_proc and ultty_proc.poll() is None:
            try:
                ctl("screenshot", out / "final-state.png")
            except Exception as error:
                (out / "final-screenshot-error.txt").write_text(str(error))
        if ultty_proc and ultty_proc.poll() is None:
            os.killpg(ultty_proc.pid, signal.SIGTERM)
            try:
                ultty_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(ultty_proc.pid, signal.SIGKILL)
                ultty_proc.wait(timeout=5)
        if xvfb and xvfb.poll() is None:
            os.killpg(xvfb.pid, signal.SIGTERM)
            try:
                xvfb.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(xvfb.pid, signal.SIGKILL)
                xvfb.wait(timeout=5)
        server.should_exit = True
        server_thread.join(10)
        listener.close()
        ultty_log.close()
        xvfb_log.close()
