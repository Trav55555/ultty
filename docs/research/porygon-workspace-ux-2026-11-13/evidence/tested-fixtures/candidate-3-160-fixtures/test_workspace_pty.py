"""Independent user-path qualification; actual authenticated daemon and SQLite.

Run with uv from PORYGON_SOURCE_ROOT, shared venv, and explicit frozen binary.
The support module is a frozen copy of the preceding composer PTY harness.
"""
import json
import os
from datetime import datetime, timedelta
import time
from pathlib import Path

import pytest
from compose_support import (
    PtyApp, ScriptedSession, TOKEN, auth, main, ConversationStore, _no_lifespan,
)

pytest_plugins = ["conftest"]


@pytest.fixture(autouse=True)
def isolate_client_state(monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))


def daemon(mock_context, monkeypatch):
    session = ScriptedSession()
    monkeypatch.setattr(mock_context.user, "_session", session)
    monkeypatch.setattr(auth, "_daemon_token", TOKEN)
    monkeypatch.setattr(auth, "_api_keys", {})
    app = main.create_app()
    app.router.lifespan_context = _no_lifespan
    app.state.context = mock_context
    app.state.start_time = datetime.now()
    app.state.mcp_manager = None
    app.state.conversation_store = ConversationStore()
    return app, session


def binary():
    path = Path(os.environ["PORYGON_NATIVE_TEST_BINARY"]).resolve()
    assert path.is_file(), path
    return path


@pytest.mark.timeout(60)
@pytest.mark.asyncio
async def test_picker_switch_and_draft_restart(mock_context, mock_database, monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    store = ConversationStore()
    for suffix in ("alpha", "beta"):
        conv = await store.create_conversation(title=f"qual-{suffix}", conv_id=f"qual-{suffix}")
        await store.add_message(conv.id, "user", f"history-{suffix}")
    app, session = daemon(mock_context, monkeypatch)
    width = int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80"))
    with PtyApp(binary(), width, app, session) as term:
        monkeypatch.setenv("PORYGON_FIXTURE_PORT", str(term.listener.getsockname()[1]))
        term.wait_screen("Message Porygon")
        term.send(b"local-draft-exact")
        term.send(b"\x1bOQ")  # xterm F2
        term.wait_screen("qual-alpha")
        term.send(b"qual-alpha")
        term.send(b"\r")
        term.wait_screen("history-alpha")
        term.send(b"alpha-draft-exact")
        term.send(b"\x1bOQ")
        term.send(b"qual-beta")
        term.send(b"\r")
        term.wait_screen("history-beta")
        assert "alpha-draft-exact" not in term.screen()
        term.send(b"beta-draft-exact")
        term.send(b"\x1bOQ")
        term.send(b"qual-alpha")
        term.send(b"\r")
        term.wait_screen("alpha-draft-exact")
        term.collect(.4)  # persistence debounce is part of the user path
        term.save_stage("draft-alpha-restored")
        assert session.messages == [], "opening/switching must not submit a turn"
    states = list((tmp_path / "state" / "porygon-tui").glob("*/drafts.json"))
    assert len(states) == 1, f"expected one daemon/cwd draft namespace: {states}"
    state = states[0]
    content = state.read_text()
    for draft in ("local-draft-exact", "alpha-draft-exact", "beta-draft-exact"):
        assert draft in content, content
    # New process against same database and XDG state exercises actual reload.
    with PtyApp(binary(), width, app, session) as term:
        term.wait_screen("alpha-draft-exact")
        term.send(b"\x1bOQ")
        term.send(b"qual-beta")
        term.send(b"\r")
        term.wait_screen("beta-draft-exact")
        term.save_stage("restart-beta-restored")
        assert session.messages == []


@pytest.mark.timeout(45)
@pytest.mark.asyncio
async def test_work_result_is_visible(mock_context, mock_database, monkeypatch):
    from porygon.runs import Run, RunStatus, RunStore
    await RunStore().create_run(Run(
        id="qual-run-complete", objective="qual-review-objective", status=RunStatus.COMPLETED,
        result="qual-result-body exact durable result",
    ))
    await RunStore().add_verification_report(
        report_id="qual-report", run_id="qual-run-complete", report_type="fixture-check",
        passed=True, summary="qual-verification-summary", checks=[{"name": "qual-check", "passed": True}],
        artifacts={"raw_log": "/fixture/qual-raw-evidence.log"},
    )
    app, session = daemon(mock_context, monkeypatch)
    with PtyApp(binary(), int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80")), app, session) as term:
        term.wait_screen("Message Porygon")
        term.send(b"\x1bOR")  # xterm F3
        term.wait_screen("qual-review-objective")
        term.save_stage("work-list")
        term.send(b"\r")
        term.wait_screen("qual-result-body")
        term.save_stage("work-detail")
        # Result body is separate from verification, source revision, and merge.
        # Scroll until the independently seeded evidence path becomes visible.
        for _ in range(20):
            if "qual-raw-evidence.log" in term.screen():
                break
            term.send(b"\x1b[B", settle=.03)
        term.wait_screen("qual-raw-evidence.log")
        term.save_stage("work-raw-evidence")
        assert session.messages == []


@pytest.mark.timeout(60)
@pytest.mark.asyncio
async def test_picker_loads_older_conversations_without_losing_search(mock_context, mock_database, monkeypatch):
    store = ConversationStore()
    await store.create_conversation(title="qual-older-target", conv_id="qual-older-target")
    await store.add_message("qual-older-target", "user", "qual-older-history-body")
    for index in range(201):
        await store.create_conversation(title=f"qual-newer-{index:03d}", conv_id=f"qual-newer-{index:03d}")
    app, session = daemon(mock_context, monkeypatch)
    with PtyApp(binary(), int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80")), app, session) as term:
        term.wait_screen("Message Porygon")
        term.send(b"\x1bOQ")
        term.wait_screen("200 loaded")
        term.send(b"qual-older-target")
        term.send(b"\r")
        assert "qual-older-history-body" not in term.screen(), "target must lie beyond first page"
        term.send(b"\x0c")  # Ctrl+L loads the older list page, preserving filter.
        term.wait_screen("all conversations loaded")
        term.save_stage("picker-older-page-filtered")
        term.send(b"\r")
        term.wait_screen("qual-older-history-body")
        term.save_stage("picker-older-conversation-opened")
        assert session.messages == []


@pytest.mark.timeout(60)
@pytest.mark.asyncio
async def test_work_attach_restores_its_conversation_and_isolates_drafts(mock_context, mock_database, monkeypatch):
    from porygon.runs import Run, RunStatus, RunStore
    store = ConversationStore()
    for suffix in ("a", "b"):
        await store.create_conversation(title=f"attach-{suffix}", conv_id=f"attach-{suffix}")
        await store.add_message(f"attach-{suffix}", "user", f"attach-history-{suffix}")
    await RunStore().create_run(Run(id="attach-run-b", objective="attach-work-b", status=RunStatus.COMPLETED,
                                   result="attach-result-b", metadata={"conversation_id": "attach-b"}))
    app, session = daemon(mock_context, monkeypatch)
    with PtyApp(binary(), int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80")), app, session) as term:
        term.wait_screen("Message Porygon")
        for suffix in ("a", "b"):
            term.send(b"\x1bOQ")
            term.send(f"attach-{suffix}".encode())
            term.send(b"\r")
            term.wait_screen(f"attach-history-{suffix}")
            term.send(f"attach-draft-{suffix}".encode())
        term.send(b"\x1bOQ")
        term.send(b"attach-a")
        term.send(b"\r")
        term.wait_screen("attach-draft-a")
        term.send(b"\x1bOR")
        term.wait_screen("attach-work-b")
        term.send(b"\r")
        term.wait_screen("attach-result-b")
        term.send(b"a")
        term.wait_screen("attach-history-b")
        term.wait_screen("attach-draft-b")
        term.save_stage("work-attached-b")
        assert "attach-draft-a" not in term.screen()
        term.send(b"\x1bOQ")
        term.send(b"attach-a")
        term.send(b"\r")
        term.wait_screen("attach-draft-a")
        term.save_stage("work-attach-source-draft-restored")
        assert session.messages == [], "attach must not submit or resume agent work"


@pytest.mark.timeout(45)
@pytest.mark.asyncio
async def test_attention_inspects_without_deciding(mock_context, mock_database, monkeypatch):
    from porygon.async_tasks.store import TaskStore
    store = TaskStore()
    pending = {
        "id": "qual-approval", "source": "tool:write_file", "objective": "qual-attention-action",
        "priority": "HIGH", "context": {"tool_name": "write_file", "params": {"path": "/fixture/qual-file", "content": "qual-exact-body"}, "run_id": "qual-pending-run"},
    }
    app, session = daemon(mock_context, monkeypatch)
    app.state.task_store = store
    decisions = []
    decision_bodies = []

    @app.middleware("http")
    async def observe_decisions(request, call_next):
        if request.method == "POST" and "/approvals/" in request.url.path:
            decisions.append(request.url.path)
            decision_bodies.append(json.loads(await request.body()))
        return await call_next(request)

    with PtyApp(binary(), int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80")), app, session) as term:
        term.wait_screen("Message Porygon")
        await store.add_pending_approval(pending, scope="tool_call")
        expected_version = (await store.get_pending_approval("qual-approval"))["decision_version"]
        term.send(b"\x1bOS")
        term.wait_screen("qual-attention-action")
        term.save_stage("attention-list")
        term.send(b"\r")
        term.wait_screen("write_file")
        seen_path = seen_content = False
        for _ in range(30):
            screen = term.screen()
            seen_path |= "/fixture/qual-file" in screen
            seen_content |= "qual-exact-body" in screen
            if seen_path and seen_content:
                break
            term.send(b"\x1b[B", settle=.03)
        assert seen_path and seen_content, "exact approval args not reachable by scrolling"
        term.save_stage("attention-inspect")
        term.send(b"\x1b")
        term.wait_screen("qual-attention-action")
        assert decisions == [], "opening an attention item must not authorize it"
        term.send(b"\r")
        term.wait_screen("write_file")
        term.send(b"n")
        deadline = time.monotonic() + 5
        while not decisions and time.monotonic() < deadline:
            term.collect(.1)
        assert decisions == ["/approvals/pending/qual-approval/deny"]
        assert decision_bodies == [{"expected_version": expected_version}]
        while await store.get_pending_approval("qual-approval") is not None and time.monotonic() < deadline:
            term.collect(.1)
        assert await store.get_pending_approval("qual-approval") is None
        assert session.messages == []


@pytest.mark.timeout(60)
@pytest.mark.asyncio
async def test_ten_thousand_message_history_remains_interactive(mock_context, mock_database, monkeypatch):
    from sqlalchemy import text
    store = ConversationStore()
    await store.create_conversation(title="qual-long-history", conv_id="qual-long")
    base = datetime(2026, 1, 1)
    records = [{"id": f"long-{i}", "conversation_id": "qual-long",
                "role": "user" if i % 2 == 0 else "assistant",
                "content": f"history-{i:05d}-boundary", "created_at": base + timedelta(seconds=i)}
               for i in range(10000)]
    async with mock_database.begin() as conn:
        await conn.execute(text("INSERT INTO messages (id,conversation_id,role,content,created_at) VALUES (:id,:conversation_id,:role,:content,:created_at)"), records)
        await conn.execute(text("UPDATE conversations SET message_count=10000 WHERE id='qual-long'"))
    app, session = daemon(mock_context, monkeypatch)
    with PtyApp(binary(), int(os.environ.get("PORYGON_ACCEPTANCE_WIDTH", "80")), app, session) as term:
        term.wait_screen("Message Porygon")
        term.send(b"\x1bOQ")
        term.wait_screen("qual-long-history")
        term.send(b"qual-long-history")
        start = time.monotonic()
        term.send(b"\r", settle=.01)
        term.wait_screen("history-09999-boundary", timeout=10)
        load_seconds = time.monotonic() - start
        term.send(b"\x1b[1;5H")
        term.wait_screen("history-00000-boundary", timeout=2)
        term.save_stage("history-10000-oldest")
        term.send(b"\x1b[1;5F")
        term.wait_screen("history-09999-boundary", timeout=2)
        start = time.monotonic()
        term.send(b"draft-after-10000", settle=.01)
        term.wait_screen("draft-after-10000", timeout=2)
        edit_seconds = time.monotonic() - start
        samples = []
        draft = "draft-after-10000"
        for _ in range(20):
            draft += "x"
            start = time.monotonic()
            term.send(b"x", settle=.001)
            term.wait_screen(draft, timeout=2)
            samples.append(time.monotonic() - start)
        status = Path(f"/proc/{term.proc.pid}/status").read_text()
        memory = {line.split(":", 1)[0]: line.split(":", 1)[1].strip()
                  for line in status.splitlines() if line.startswith(("VmRSS:", "VmHWM:"))}
        term.save_stage("history-10000-bottom-and-draft")
        (term.capture_dir / "history-performance.json").write_text(json.dumps({
            "messages_seeded": 10000, "open_seconds": load_seconds,
            "edit_visible_seconds": edit_seconds, "width": term.width,
            "edit_samples_seconds": samples, "edit_p95_seconds": sorted(samples)[18],
            "process_memory": memory,
            "human_task_trial": False,
        }, indent=2))
        assert session.messages == []
