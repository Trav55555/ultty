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
HISTORY_COUNT = int(os.environ.get("PORYGON_HISTORY_MESSAGES", "10000"))
ARTIFACT_ROOT = Path(os.environ.get(
    "PORYGON_ARTIFACT_ROOT", "/tmp/porygon-compose-verification"
)).resolve()
OUT_ROOT = ARTIFACT_ROOT / "receipts"


class NativeSession:
    def __init__(self):
        self.messages: list[str] = []
        self.first_started = threading.Event()
        self.cancelled_first = threading.Event()
        self.release_first = threading.Event()

    async def process_request_async(self, message, **kwargs):
        self.messages.append(message)
        if len(self.messages) == 1:
            self.first_started.set()
            yield TokenEvent(text="assistant body alpha visible")
            cancel_event = kwargs["cancel_event"]
            while not cancel_event.is_set() and not self.release_first.is_set():
                await asyncio.sleep(.02)
            if cancel_event.is_set():
                self.cancelled_first.set()
                return
            yield TokenEvent(text="assistant body alpha complete")
        else:
            yield TokenEvent(text=RAW_REPLY)


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
def test_porygon_native_gpu_composer(mock_context, mock_database, monkeypatch):
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
            ["Xvfb", "-displayfd", "1", "-screen", "0", "2560x1440x24", "-nolisten", "tcp"],
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
        # Independent regression: at the human-trial widths, retain the
        # transcript instead of allocating a decorative sidebar.
        for wanted in (99, 100, 102, 120, 160):
            low, high = 300, 2500
            while low <= high:
                pixels = (low + high) // 2
                ctl("resize", pixels, 800)
                actual = json.loads(ctl("info", json_output=True))["cols"]
                if actual == wanted:
                    break
                if actual < wanted:
                    low = pixels + 1
                else:
                    high = pixels - 1
            assert actual == wanted, (wanted, actual)
            def resized():
                doc = snapshot(f"width-{wanted}")
                roots = [r for r in walk_regions(doc) if r.get("parent") is None]
                transcripts = [r for r in walk_regions(doc)
                               if r.get("metadata", {}).get("id") == "conversation-transcript"]
                # The terminal resizes its root before the client's resize
                # event produces a new frame. Wait for that frame's layout too.
                ready = (any(r.get("width") == wanted for r in roots)
                         and len(transcripts) == 1
                         and transcripts[0]["width"] >= min(wanted, 84))
                return doc if ready else False
            doc = _wait(resized, f"native layout at {wanted} columns")
            ctl("screenshot", out / f"width-{wanted}.png")
            transcripts = [r for r in walk_regions(doc)
                           if r.get("metadata", {}).get("id") == "conversation-transcript"]
            assert len(transcripts) == 1, transcripts
            width = transcripts[0]["width"]
            assert width >= min(wanted, 84), (wanted, width)
            visible = list(all_strings(doc))
            assert not any("MODE:   ACTIVE" in text for text in visible), visible
            assert not any("WEATHER: off" in text for text in visible), visible
        ctl("resize", "800", "480")
        ctl("send-key", "f1")
        def help_visible():
            doc = snapshot("help-visible")
            return doc if any("Keys and commands" in s for s in all_strings(doc)) else False

        help_regions = _wait(help_visible, "native help title")
        help_values = list(all_strings(help_regions))
        assert any("Ctrl+X" in s for s in help_values), help_regions
        ctl("screenshot", out / "help-visible.png")
        ctl("send-key", "escape")
        _wait(lambda: any("Message Porygon" in s or "chat-input" in s
                          for s in strings("help-dismissed")),
              "native help dismissed back to composer")
        compact = snapshot("compact")
        compact_values = list(all_strings(compact))
        assert any("Message Porygon" in s or "chat-input" in s for s in compact_values), compact
        input_regions = [r for r in walk_regions(compact)
                         if r.get("role") in ("input", "textbox")
                         or r.get("label") == "Message Porygon"
                         or "chat-input" in json.dumps(r)]
        assert input_regions, f"native composer lacks an input semantic region: {compact}"
        (out / "compact.info.json").write_text(ctl("info", json_output=True))
        ctl("screenshot", out / "compact.png")

        ctl("send-key", "f2")
        _wait(lambda: any("native-qual-alpha" in s for s in strings("session-picker")), "native session picker")
        ctl("screenshot", out / "session-picker.png")
        _type_native_text("native-long-history", display_env=env, window=native_window)
        started = time.monotonic()
        ctl("send-key", "enter")
        last_boundary = f"native-history-{HISTORY_COUNT - 1:05d}-boundary"
        _wait(lambda: any(last_boundary in s for s in strings("long-history")), f"{HISTORY_COUNT} native history bottom", timeout=10)
        history_load_seconds = time.monotonic() - started
        ctl("send-key", "home", "ctrl")
        _wait(lambda: any("native-history-00000-boundary" in s for s in strings("long-history-oldest")), "10k native oldest boundary", timeout=2)
        ctl("screenshot", out / "long-history-oldest.png")
        ctl("send-key", "end", "ctrl")
        _wait(lambda: any(last_boundary in s for s in strings("long-history-latest")), "native latest boundary", timeout=2)
        edit_samples = []
        draft = ""
        for _ in range(20):
            draft += "x"
            started = time.monotonic()
            _type_native_text("x", display_env=env, window=native_window)
            _wait(lambda: any(s == draft for s in strings("long-history-edit")), "native history edit", timeout=2)
            edit_samples.append(time.monotonic() - started)
        ctl("screenshot", out / "long-history.png")
        memory = {}
        for process in Path("/proc").iterdir():
            if not process.name.isdigit():
                continue
            try:
                if (process / "exe").resolve() == binary and env["PORYGON_URL"].encode() in (process / "environ").read_bytes():
                    status = (process / "status").read_text()
                    memory = {line.split(":", 1)[0]: line.split(":", 1)[1].strip() for line in status.splitlines() if line.startswith(("VmRSS:", "VmHWM:"))}
                    break
            except (OSError, PermissionError):
                pass
        assert memory, "native Porygon process memory receipt unavailable"
        (out / "history-performance.json").write_text(json.dumps({"messages_seeded": HISTORY_COUNT, "open_seconds": history_load_seconds, "edit_samples_seconds": edit_samples, "edit_p95_seconds": sorted(edit_samples)[18], "process_memory": memory, "human_task_trial": False}, indent=2))
        ctl("send-key", "f2")
        ctl("send-key", "n", "ctrl")
        ctl("send-key", "escape")
        ctl("send-key", "f3")
        _wait(lambda: any("native-review-objective" in s for s in strings("work-list")), "native work list")
        ctl("screenshot", out / "work-list.png")
        ctl("send-key", "enter")
        _wait(lambda: any("native-result-body" in s for s in strings("work-detail")), "native durable result")
        ctl("screenshot", out / "work-detail.png")
        for _ in range(30):
            if any("native-qual.log" in s for s in strings("work-evidence")):
                break
            ctl("send-key", "down")
        assert any("native-qual.log" in s for s in strings("work-evidence-final")), "native raw evidence path unreachable"
        ctl("screenshot", out / "work-evidence.png")
        ctl("send-key", "c")
        copied_review = retrieve_system_clipboard("copied-review")
        review_data = json.loads(copied_review.read_text())
        review_text = json.dumps(review_data)
        assert "native-qual-run" in review_text and "/fixture/native-qual.log" in review_text, review_data
        assert "native-qual-check" in review_text, review_data
        ctl("send-key", "a")
        _wait(lambda: any("native-history-alpha" in s for s in strings("work-attached-conversation")), "work attach restores bound conversation")
        ctl("screenshot", out / "work-attached-conversation.png")
        assert session.messages == [], "native work attach must not submit a turn"
        asyncio.run(task_store.add_pending_approval({"id": "native-qual-approval", "source": "tool:write_file", "objective": "native-attention-action", "context": {"tool_name": "write_file", "params": {"path": "/fixture/native-file", "content": "native-exact-body"}, "run_id": "native-pending-run"}}, scope="tool_call"))
        expected_version = asyncio.run(task_store.get_pending_approval("native-qual-approval"))["decision_version"]
        ctl("send-key", "f4")
        _wait(lambda: any("native-attention-action" in s for s in strings("attention-list")), "native attention list")
        ctl("screenshot", out / "attention-list.png")
        ctl("send-key", "enter")
        _wait(lambda: any("write_file" in s for s in strings("attention-inspect")), "native attention inspection")
        seen_path = seen_content = False
        for _ in range(30):
            values = strings("attention-exact-args")
            seen_path |= any("/fixture/native-file" in s for s in values)
            seen_content |= any("native-exact-body" in s for s in values)
            if seen_path and seen_content:
                break
            ctl("send-key", "down")
        assert seen_path and seen_content, "native exact approval args unreachable"
        ctl("screenshot", out / "attention-inspect.png")
        assert approval_posts == [], "native inspection must not authorize or deny"
        ctl("send-key", "escape")
        _wait(lambda: any("native-attention-action" in s for s in strings("attention-dismissed")), "attention list after inspection")
        assert approval_posts == [], "Escape from inspection must not decide"
        ctl("send-key", "enter")
        _wait(lambda: any("Approval details" in s for s in strings("attention-reopened")), "approval details reopened for explicit denial")
        ctl("send-key", "n")
        _wait(lambda: approval_posts == ["/approvals/pending/native-qual-approval/deny"], "one exact native UI denial")
        _wait(lambda: asyncio.run(task_store.get_pending_approval("native-qual-approval")) is None, "daemon removes denied approval")
        _wait(lambda: not any("Approval details" in s for s in strings("attention-denied")), "daemon-confirmed denial closes details")
        ctl("send-key", "escape")

        assert approval_posts == ["/approvals/pending/native-qual-approval/deny"]
        assert approval_post_bodies == [{"expected_version": expected_version}]
        # Restore the independent multiline paste payload after review copy.
        ctl("new-tab", "/bin/sh", "-c", f"printf '\\033]52;c;{clipboard}\\007'; printf 'CLIPBOARD_RESEEDED\\n'; sleep 60")
        _wait(lambda: "CLIPBOARD_RESEEDED" in ctl("text"), "multiline clipboard reseed")
        ctl("switch-tab", "1")
        time.sleep(.2)
        _type_native_text("visible-user-body", display_env=env, window=native_window)
        before = _wait(
            lambda: (doc if any("visible-user-body" in s for s in all_strings(doc)) else False)
            if (doc := snapshot("draft-one")) else False,
            "native first draft text",
        )
        ctl("send-key", "enter")
        _wait(lambda: len(session.messages) == 1, "first exact daemon request")
        assert session.messages[0] == "visible-user-body", session.messages
        assert session.first_started.wait(3)

        def first_body_visible():
            doc = snapshot("assistant-one-streaming")
            return doc if any("assistant body alpha visible" in s for s in all_strings(doc)) else False

        first_doc = _wait(first_body_visible, "assistant body in native region content")
        assert session.messages == ["visible-user-body"]
        ctl("screenshot", out / "assistant-streaming.png")

        # Actual system clipboard paste into the native composer must preserve
        # the newline and never submit implicitly, including while busy.
        subprocess.run(["xdotool", "windowfocus", "--sync", native_window,
                        "key", "--clearmodifiers", "ctrl+shift+v"],
                       env=env, check=True, timeout=10)
        def multiline_draft_visible():
            doc = snapshot("multiline-draft")
            values = list(all_strings(doc))
            return doc if any("draft first" in s for s in values) and any(
                "draft second" in s for s in values
            ) else False

        draft_doc = _wait(multiline_draft_visible, "complete native multiline draft")
        assert session.messages == ["visible-user-body"], "clipboard paste submitted a turn"
        draft_strings = list(all_strings(draft_doc))
        ctl("screenshot", out / "multiline-draft.png")
        ctl("send-key", "enter")
        time.sleep(.4)
        assert len(session.messages) == 1, f"busy Enter submitted: {session.messages!r}"
        preserved = snapshot("busy-draft-preserved")
        preserved_values = list(all_strings(preserved))
        assert any("draft first" in s for s in preserved_values), preserved
        assert any("draft second" in s for s in preserved_values), preserved

        # Verify the real native Ctrl+C conversion, cancellation route, and
        # retention of an unsubmitted draft while stopping active work.
        ctl("send-key", "c", "ctrl")
        _wait(lambda: session.cancelled_first.is_set(), "daemon cancellation event")
        _wait(lambda: any("run cancelled" in s.lower()
                          for s in strings("cancelled-run")),
              "durable cancelled state rendered")
        cancelled_draft = snapshot("cancelled-draft-preserved")
        cancelled_values = list(all_strings(cancelled_draft))
        assert any("draft first" in s for s in cancelled_values), cancelled_draft
        assert any("draft second" in s for s in cancelled_values), cancelled_draft

        ctl("send-key", "enter")
        _wait(lambda: len(session.messages) == 2, "second exact daemon request")
        assert session.messages[1] == "draft first\ndraft second", session.messages

        def second_body_visible():
            doc = snapshot("assistant-two-complete")
            return doc if any("assistant body beta visible" in s for s in all_strings(doc)) else False

        wide = _wait(second_body_visible, "second assistant body in native regions")
        ctl("resize", "1280", "768")
        wide = snapshot("wide")
        wide_values = list(all_strings(wide))
        assert any("visible-user-body" in s for s in wide_values), wide
        assert any("assistant body beta visible" in s for s in wide_values), wide
        ctl("screenshot", out / "wide.png")
        (out / "wide.info.json").write_text(ctl("info", json_output=True))
        # Exercise Porygon's explicit copy action, then retrieve the actual
        # system clipboard through a new ordinary shell pane in this window.
        ctl("send-key", "y", "ctrl")
        copied_reply = retrieve_system_clipboard("copied-reply")
        _wait(lambda: copied_reply.is_file() and copied_reply.read_text() == RAW_REPLY,
              "native CtrlY copies exact raw assistant reply")
        _wait(lambda: any("assistant body beta visible" in s for s in strings("returned-after-copy")), "native app after clipboard retrieval")
        assert (out / "compact.png").stat().st_size > 0
        assert (out / "wide.png").stat().st_size > 0
        assert (out / "assistant-streaming.png").stat().st_size > 0
        assert (out / "multiline-draft.png").stat().st_size > 0

        # Idle Ctrl+C first warns without exiting and keeps the draft; a second
        # Ctrl+C confirms the documented discard-and-quit action.
        _type_native_text("unsent draft", display_env=env, window=native_window)
        _wait(lambda: any(s == "unsent draft" for s in strings("idle-draft-input")),
              "complete native unsent draft")
        ctl("send-key", "c", "ctrl")

        def quit_warning_visible():
            doc = snapshot("idle-draft-quit-warning")
            return doc if any("Unsent draft" in s for s in all_strings(doc)) else False

        quit_warning = _wait(quit_warning_visible, "unsent-draft confirmation warning")
        warning_values = list(all_strings(quit_warning))
        assert any(s == "unsent draft" for s in warning_values), quit_warning
        ctl("screenshot", out / "idle-draft-warning.png")
        ctl("send-key", "c", "ctrl")
        _wait(lambda: not list(walk_regions(snapshot("app-exited"))),
              "Porygon exits after second draft confirmation", timeout=10)
        def app_exit_marker_visible():
            value = ctl("text")
            return value if "PORYGON_FIXTURE_EXIT:0" in value else False

        exit_text = _wait(app_exit_marker_visible, "Porygon exits with status 0", timeout=10)
        (out / "app-exit-text.txt").write_text(exit_text)

        connection_regions = [
            r for r in walk_regions(connected_doc)
            if "Porygon connected" in r.get("text", "")
        ]
        connection_geometry = [
            {"text": r["text"], "width_cells": r.get("width"),
             "required_cells": len(r["text"])}
            for r in connection_regions
        ]
        assert connection_geometry and all(
            item["width_cells"] >= item["required_cells"] for item in connection_geometry
        ), f"connected status label geometry clips ASCII text: {connection_geometry}"

        receipt = {
            "source_root": str(PROJECT),
            "artifact_root": str(ARTIFACT_ROOT),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "ultty_sha256": hashlib.sha256(ultty.read_bytes()).hexdigest(),
            "ulttyctl_sha256": hashlib.sha256(ulttyctl.read_bytes()).hexdigest(),
            "fixture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "gpu_backend_hint": "lavapipe ICD selected when installed; Ultty log retained",
            "window_sizes_pixels": {"compact": [800, 480], "wide": [1280, 768]},
            "messages_received_by_daemon": session.messages,
            "real_system_clipboard_multiline_paste": True,
            "native_ctrl_y_raw_reply_copied_exact": copied_reply.read_text() == RAW_REPLY,
            "native_review_evidence_copied_json": review_data,
            "approval_decision_posts": approval_posts,
            "approval_expected_version_request_matched": approval_post_bodies == [{"expected_version": expected_version}],
            "native_region_text_checks": {
                "user_body": "visible-user-body" in "\n".join(all_strings(first_doc)),
                "assistant_body": "assistant body alpha visible" in "\n".join(all_strings(first_doc)),
                "multiline_draft": "draft first" in "\n".join(draft_strings) and "draft second" in "\n".join(draft_strings),
                "busy_enter_did_not_submit": len(session.messages) == 2,
                "busy_ctrl_c_cancelled_first": session.cancelled_first.is_set(),
                "assistant_body_at_wide_size": "assistant body beta visible" in "\n".join(wide_values),
                "idle_draft_confirmation_shown": any("Unsent draft" in s for s in warning_values),
            "porygon_exit_status": 0 if "PORYGON_FIXTURE_EXIT:0" in exit_text else None,
            "connected_status_label_geometry": connection_geometry,
            },
            "regions_files": ["compact.regions.json", "multiline-draft.regions.json", "wide.regions.json"],
            "screenshots": ["help-visible.png", "compact.png", "assistant-streaming.png", "multiline-draft.png", "wide.png", "idle-draft-warning.png"],
        }
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
